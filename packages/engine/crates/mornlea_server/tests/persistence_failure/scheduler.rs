//! Autosave scheduling over the accepted durable store mailbox.
//!
//! These cases drive the [`AutosaveScheduler`](mornlea_server::store::scheduler::AutosaveScheduler)
//! provider with the accepted [`StoreMailbox`](mornlea_server::store::mailbox::StoreMailbox)
//! as the backend-facing store: the scheduler selects through the frozen
//! [`SaveAuthority`] seams and retries, while the mailbox owns admission,
//! tickets and the backend commit. Expected values mirror the Go persistence
//! scheduler in `packages/server/server/persistence/world.go` and its retry,
//! backpressure, schedule and metadata tests, which are cited per case:
//!
//! * Selection budgets allow an oversized first snapshot and defer the rest,
//!   while the separate owned-byte ceiling refuses the whole request, from the
//!   budget selection test in `world_schedule_test.go`.
//! * Partial commits acknowledge only listed keys and retain the remainder for
//!   retry, from the partial-commit tests in `world_schedule_test.go` and
//!   `world_retry_test.go`.
//! * A submitted key omitted with no error is a hard failure, from the
//!   nil-error omission test in `world_retry_test.go`.
//! * Stale, above-current and foreign committed revisions never produce a
//!   false ack, from the above-current, ahead-of-snapshot and foreign-content
//!   tests in `world_schedule_test.go`.
//! * Retry backoff doubles from the base with a cap-before-double ceiling and
//!   a saturating next tick, from `retryDelay` and the queue-full test in
//!   `world_retry_test.go`; backpressure enters at the ceiling and exits
//!   strictly below ninety percent, from the hysteresis test in
//!   `world_backpressure_test.go`.
//! * The final metadata barrier preserves its frozen world-time target across
//!   failure and retry, from the metadata schedule in `world.go` and the
//!   autosave tests in `world_metadata_test.go`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mornlea_domain::{ChunkPos, Dimension, PlayerId};
use mornlea_server::contracts::{
    AckReport, ChunkKey, Clock, Deadline, DiskBackend, Operation, OwnedSnapshot, Resource,
    SaveAuthority, SaveBudget, SaveCompletion, SaveKey, SaveMode, SaveRequest, SaveStats,
    SaveTicket, SaveUrgency, SaveValue, ServerError, StoreHandle, StoreLimits,
};
use mornlea_server::store::mailbox::StoreMailbox;
use mornlea_server::store::scheduler::{
    AutosaveScheduler, SchedulerConfig, next_backpressure, retry_delay,
};
use mornlea_storage::{
    CHUNK_CURRENT_SCHEMA, CHUNK_ENVELOPE_LENGTH, ChestSlot, Chunk, ChunkSave, ContainerSnapshot,
    DropSlot, FurnaceSlot, Inventory, ItemStack, MAX_COMPRESSED_CHUNK, METADATA_CURRENT_VERSION,
    Metadata, MetadataChunkPos, PlayerLocation, StorageKind, chunk_logical_len, player_encoded_len,
    world_metadata_encoded_len,
};

/// Chunk reservation the mailbox holds before the worker encodes.
const CHUNK_RESERVATION: usize = MAX_COMPRESSED_CHUNK as usize + CHUNK_ENVELOPE_LENGTH;

/// Retry and cadence shape mirrored from the Go defaults.
const TEST_INTERVAL: u64 = 6;
const TEST_BASE: u64 = 20;
const TEST_MAX: u64 = 1200;

fn accepted_limits() -> StoreLimits {
    StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).expect("accepted maxima")
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn domain_player(tag: u8) -> PlayerId {
    PlayerId::try_from_bytes(uuid(tag)).expect("player identity")
}

fn empty_chunk() -> Chunk {
    Chunk {
        sections: vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: Vec::new(),
                packed: Vec::new(),
            };
            24
        ],
        drops: vec![DropSlot::default(); 32],
        furnaces: vec![FurnaceSlot::default(); 32],
        chests: vec![ChestSlot::default(); 16],
    }
}

fn chunk_key(index: i32) -> SaveKey {
    SaveKey::Chunk(ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(index, 0),
    })
}

fn chunk_save(index: i32, revision: u64) -> ChunkSave {
    ChunkSave {
        key: mornlea_storage::ChunkKey {
            dimension: 0,
            x: index,
            z: 0,
        },
        revision,
        chunk: empty_chunk(),
    }
}

fn chunk_snapshot(index: i32, revision: u64, urgency: SaveUrgency) -> OwnedSnapshot {
    chunk_snapshot_estimated(index, revision, urgency, CHUNK_RESERVATION)
}

fn chunk_snapshot_estimated(
    index: i32,
    revision: u64,
    urgency: SaveUrgency,
    estimated: usize,
) -> OwnedSnapshot {
    let save = chunk_save(index, revision);
    chunk_logical_len(&save, CHUNK_CURRENT_SCHEMA).expect("chunk snapshot validates");
    OwnedSnapshot::try_new(
        chunk_key(index),
        revision,
        estimated,
        urgency,
        SaveValue::Chunk(save),
    )
    .expect("chunk snapshot")
}

fn player_snapshot(tag: u8, revision: u64) -> OwnedSnapshot {
    let save = mornlea_storage::PlayerSave {
        player_id: mornlea_storage::PlayerId::from_bytes(uuid(tag)),
        revision,
        display_name: "Tester".to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position: [0.0, 64.0, 0.0],
        },
        yaw: 0.0,
        pitch: 0.0,
        safe: None,
        inventory: Inventory::default(),
        health: 20,
        hunger: 20,
        saturation_milli: 5_000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0, 0.0, 0.0],
        respawn_dimension: 0,
        armor: [ItemStack::default(); 4],
    };
    let estimated = player_encoded_len(&save).expect("player encodes");
    OwnedSnapshot::try_new(
        SaveKey::Player(domain_player(tag)),
        revision,
        estimated,
        SaveUrgency::Autosave,
        SaveValue::Player(save),
    )
    .expect("player snapshot")
}

fn metadata_value(world_time: u64) -> Metadata {
    Metadata {
        format_version: METADATA_CURRENT_VERSION,
        seed: 7,
        spawn_dimension: 0,
        spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
        world_time_ticks: world_time,
        day_phase_offset: 0,
        weather_kind: 0,
        weather_ticks_remaining: 0,
        depths_spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
        depths_seed_salt: 0,
        difficulty: 0,
    }
}

/// Script shared with the test so mid-flight backend behavior can change
/// between worker drives. The double records what it was asked to write and
/// never claims disk persistence.
#[derive(Default)]
struct BackendScript {
    fail_next_write: Option<ServerError>,
    /// Commits only the first submitted snapshot and fails the rest, for the
    /// partial-commit shape.
    fail_rest: Option<ServerError>,
    /// Commits only the first submitted snapshot with no error, for the
    /// nil-error omission shape.
    omit_rest: bool,
    /// Per-key committed answers merged over the normal echo, for stale and
    /// foreign revision answers to one key while the rest commit normally.
    committed_override: Option<Vec<(SaveKey, u64)>>,
    seen_submitted: Vec<Vec<(SaveKey, u64)>>,
    seen_metadata_time: Vec<u64>,
    sync_calls: usize,
}

struct BackendDouble {
    script: Rc<RefCell<BackendScript>>,
}

impl BackendDouble {
    fn new() -> (Self, Rc<RefCell<BackendScript>>) {
        let script = Rc::new(RefCell::new(BackendScript::default()));
        (
            Self {
                script: Rc::clone(&script),
            },
            script,
        )
    }
}

fn unused_port() -> ServerError {
    ServerError::Internal {
        invariant: "scheduler double: unused port",
    }
}

impl DiskBackend for BackendDouble {
    fn write(&mut self, ticket: SaveTicket, request: SaveRequest) -> SaveCompletion {
        let snapshots = request.snapshots;
        let submitted: Vec<(SaveKey, u64)> = snapshots
            .iter()
            .map(|snapshot| (snapshot.key.clone(), snapshot.revision))
            .collect();
        let mut script = self.script.borrow_mut();
        script.seen_submitted.push(submitted.clone());
        for snapshot in &snapshots {
            if let SaveValue::Metadata(meta) = &snapshot.value {
                script.seen_metadata_time.push(meta.world_time_ticks);
            }
        }
        if let Some(error) = script.fail_next_write.take() {
            return SaveCompletion {
                ticket,
                snapshots,
                submitted,
                committed: Vec::new(),
                error: Some(error),
            };
        }
        if let Some(error) = script.fail_rest.take() {
            let committed = submitted.first().cloned().into_iter().collect();
            return SaveCompletion {
                ticket,
                snapshots,
                submitted,
                committed,
                error: Some(error),
            };
        }
        if script.omit_rest {
            let committed = submitted.first().cloned().into_iter().collect();
            return SaveCompletion {
                ticket,
                snapshots,
                submitted,
                committed,
                error: None,
            };
        }
        if let Some(answers) = script.committed_override.clone() {
            let committed = submitted
                .iter()
                .map(|(key, revision)| {
                    answers
                        .iter()
                        .find(|(held, _)| held == key)
                        .map(|(_, answer)| (key.clone(), *answer))
                        .unwrap_or_else(|| (key.clone(), *revision))
                })
                .collect();
            return SaveCompletion {
                ticket,
                snapshots,
                submitted,
                committed,
                error: None,
            };
        }
        let committed = submitted.clone();
        SaveCompletion {
            ticket,
            snapshots,
            submitted,
            committed,
            error: None,
        }
    }

    fn load(
        &mut self,
        _key: SaveKey,
    ) -> Result<mornlea_server::contracts::LoadedValue, ServerError> {
        Err(unused_port())
    }

    fn sync(&mut self) -> Result<(), ServerError> {
        self.script.borrow_mut().sync_calls += 1;
        Ok(())
    }

    fn close(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
}

/// Authority double mirroring the frozen per-key acknowledgment policy: an
/// equal commit advances the persisted revision and clears only the matching
/// in-flight entry, a commit strictly between the submitted and current
/// revisions releases the old entry with a bounded persisted update, a commit
/// at or past current claims no foreign content, and a stale commit can never
/// clear a newer in-flight entry. Selection excludes in-flight keys, sorts
/// unload urgency before key order, and allows an oversized first snapshot
/// against the checked budget.
struct AuthorityDouble {
    dirty: Vec<OwnedSnapshot>,
    in_flight: Vec<OwnedSnapshot>,
    persisted: Vec<(SaveKey, u64)>,
    current: Vec<(SaveKey, u64)>,
    acked: Vec<(SaveKey, u64)>,
    stale: usize,
    apply_calls: usize,
    meta_time: u64,
    meta_seq: u64,
    meta_committed_time: u64,
}

impl AuthorityDouble {
    fn new() -> Self {
        Self {
            dirty: Vec::new(),
            in_flight: Vec::new(),
            persisted: Vec::new(),
            current: Vec::new(),
            acked: Vec::new(),
            stale: 0,
            apply_calls: 0,
            meta_time: 600,
            meta_seq: 9,
            meta_committed_time: 600,
        }
    }

    fn persisted_of(&self, key: &SaveKey) -> u64 {
        self.persisted
            .iter()
            .find(|(held, _)| held == key)
            .map(|(_, revision)| *revision)
            .unwrap_or(0)
    }

    fn current_of(&self, key: &SaveKey) -> u64 {
        self.current
            .iter()
            .find(|(held, _)| held == key)
            .map(|(_, revision)| *revision)
            .unwrap_or(0)
    }

    fn bump_persisted(&mut self, key: &SaveKey, revision: u64) {
        if let Some(slot) = self.persisted.iter_mut().find(|(held, _)| held == key) {
            slot.1 = slot.1.max(revision);
        } else {
            self.persisted.push((key.clone(), revision));
        }
    }

    /// Stages a dirty revision and advances the authority current revision, as
    /// a mutation would. Selection marks in-flight separately.
    fn add_dirty(&mut self, snapshot: OwnedSnapshot) {
        if let Some(slot) = self
            .current
            .iter_mut()
            .find(|(held, _)| held == &snapshot.key)
        {
            slot.1 = slot.1.max(snapshot.revision);
        } else {
            self.current.push((snapshot.key.clone(), snapshot.revision));
        }
        self.dirty.push(snapshot);
    }

    fn dirty_keys(&self) -> Vec<(SaveKey, u64)> {
        self.dirty
            .iter()
            .map(|snapshot| (snapshot.key.clone(), snapshot.revision))
            .collect()
    }

    fn in_flight_keys(&self) -> Vec<(SaveKey, u64)> {
        self.in_flight
            .iter()
            .map(|snapshot| (snapshot.key.clone(), snapshot.revision))
            .collect()
    }
}

fn family_rank(key: &SaveKey) -> u8 {
    match key {
        SaveKey::Player(_) => 0,
        SaveKey::Companions => 1,
        SaveKey::Hostiles => 2,
        SaveKey::Passives => 3,
        SaveKey::Metadata => 4,
        SaveKey::Chunk(_) => 5,
    }
}

impl SaveAuthority for AuthorityDouble {
    fn select(&mut self, mode: SaveMode, budget: SaveBudget) -> Vec<OwnedSnapshot> {
        let mut candidates: Vec<OwnedSnapshot> = self
            .dirty
            .iter()
            .filter(|snapshot| !matches!(snapshot.key, SaveKey::Metadata))
            .filter(|snapshot| snapshot.revision > self.persisted_of(&snapshot.key))
            .filter(|snapshot| !self.in_flight.iter().any(|held| held.key == snapshot.key))
            .filter(|snapshot| mode == SaveMode::All || snapshot.urgency == SaveUrgency::Unload)
            .cloned()
            .collect();
        // Unload urgency sorts before key order, mirroring the Go realm
        // selection; chunks sort by dimension and coordinates.
        candidates.sort_by(|left, right| {
            let unload_left = left.urgency == SaveUrgency::Unload;
            let unload_right = right.urgency == SaveUrgency::Unload;
            if unload_left != unload_right {
                return if unload_left {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                };
            }
            match (&left.key, &right.key) {
                (SaveKey::Chunk(a), SaveKey::Chunk(b)) => a.cmp(b),
                (SaveKey::Chunk(_), _) => std::cmp::Ordering::Less,
                (_, SaveKey::Chunk(_)) => std::cmp::Ordering::Greater,
                _ => family_rank(&left.key).cmp(&family_rank(&right.key)),
            }
        });
        // Checked budgets stop after the first snapshot, so an oversized
        // first selection stays legitimate while the rest defers.
        let mut selected = Vec::new();
        let mut bytes = 0usize;
        for candidate in candidates {
            if !selected.is_empty()
                && (selected.len() >= budget.chunks
                    || bytes + candidate.estimated_bytes > budget.estimated_bytes)
            {
                break;
            }
            bytes = bytes.saturating_add(candidate.estimated_bytes);
            self.in_flight.push(candidate.clone());
            selected.push(candidate);
        }
        selected
    }

    fn return_dirty(&mut self, snapshot: OwnedSnapshot) {
        self.in_flight
            .retain(|held| held.key != snapshot.key || held.revision != snapshot.revision);
        if snapshot.revision > self.persisted_of(&snapshot.key)
            && !self
                .dirty
                .iter()
                .any(|held| held.key == snapshot.key && held.revision == snapshot.revision)
        {
            self.dirty.push(snapshot);
        }
    }

    fn apply_completion(&mut self, completion: SaveCompletion) -> AckReport {
        self.apply_calls += 1;
        let mut acked = 0usize;
        let mut released = 0usize;
        let mut retry = Vec::new();
        let mut errors: Vec<ServerError> = completion.error.into_iter().collect();
        // A metadata completion echoes the process-local sequence and carries
        // the frozen world time in its snapshot value, never a wall clock.
        let is_metadata = completion
            .submitted
            .iter()
            .any(|(key, _)| matches!(key, SaveKey::Metadata));
        for (key, revision) in &completion.submitted {
            if matches!(key, SaveKey::Metadata) {
                let echoed = completion
                    .committed
                    .iter()
                    .find(|(held, _)| held == key)
                    .map(|(_, committed)| *committed);
                match echoed {
                    Some(committed) if committed == *revision => {
                        if let Some(snapshot) = completion
                            .snapshots
                            .iter()
                            .find(|held| held.key == *key && held.revision == *revision)
                            && let SaveValue::Metadata(meta) = &snapshot.value
                        {
                            self.meta_committed_time = meta.world_time_ticks;
                        }
                        acked += 1;
                    }
                    _ => {
                        if errors.is_empty() {
                            errors.push(ServerError::Internal {
                                invariant: "save omitted submitted snapshot",
                            });
                        }
                        for snapshot in completion
                            .snapshots
                            .iter()
                            .filter(|held| held.key == *key && held.revision == *revision)
                        {
                            retry.push(snapshot.clone());
                        }
                    }
                }
                continue;
            }
            let committed = completion
                .committed
                .iter()
                .find(|(held, _)| held == key)
                .map(|(_, committed)| *committed);
            match committed {
                Some(committed) if committed == *revision => {
                    self.bump_persisted(key, committed);
                    self.in_flight
                        .retain(|held| held.key != *key || held.revision != *revision);
                    self.dirty
                        .retain(|held| held.key != *key || held.revision != *revision);
                    self.acked.push((key.clone(), *revision));
                    acked += 1;
                }
                Some(committed) if committed > *revision => {
                    // The old in-flight entry releases first; only a commit
                    // strictly below current advances the persisted revision,
                    // and a commit at or past current claims no content.
                    self.in_flight
                        .retain(|held| held.key != *key || held.revision != *revision);
                    released += 1;
                    if committed < self.current_of(key) {
                        self.bump_persisted(key, committed);
                    } else if committed > self.current_of(key) {
                        errors.push(ServerError::Internal {
                            invariant: "save committed above current",
                        });
                    }
                }
                Some(_) => {
                    // A stale commit is reported and can never clear the
                    // newer in-flight snapshot.
                    self.stale += 1;
                    errors.push(ServerError::Internal {
                        invariant: "save stale committed revision",
                    });
                }
                None => {
                    // Uncommitted ownership stays dirty for retry; an omitted
                    // key with no error is a hard failure, never silent.
                    if errors.is_empty() {
                        errors.push(ServerError::Internal {
                            invariant: "save omitted submitted snapshot",
                        });
                    }
                    for snapshot in completion
                        .snapshots
                        .iter()
                        .filter(|held| held.key == *key && held.revision == *revision)
                    {
                        retry.push(snapshot.clone());
                    }
                }
            }
        }
        // A metadata-only completion still counts its call; chunk counters
        // stay untouched by the metadata lane.
        if is_metadata && acked == 0 && errors.is_empty() {
            errors.push(ServerError::Internal {
                invariant: "save omitted submitted snapshot",
            });
        }
        AckReport {
            acked,
            released,
            retry,
            errors,
        }
    }

    fn save_stats(&self) -> SaveStats {
        let estimated_unsaved_bytes = self
            .dirty
            .iter()
            .chain(self.in_flight.iter())
            .fold(0usize, |sum, snapshot| {
                sum.saturating_add(snapshot.estimated_bytes)
            });
        SaveStats {
            dirty: self.dirty.len(),
            in_flight: self.in_flight.len(),
            estimated_unsaved_bytes,
        }
    }

    fn metadata_snapshot(&self) -> OwnedSnapshot {
        let value = metadata_value(self.meta_time);
        let estimated = world_metadata_encoded_len(&value).expect("metadata encodes");
        OwnedSnapshot::try_new(
            SaveKey::Metadata,
            self.meta_seq,
            estimated,
            SaveUrgency::Autosave,
            SaveValue::Metadata(value),
        )
        .expect("metadata snapshot carries a nonzero sequence")
    }
}

struct FixedClock {
    now: Cell<Instant>,
    wall: i64,
}

impl Clock for FixedClock {
    fn monotonic(&self) -> Instant {
        self.now.get()
    }

    fn unix_ms(&self) -> i64 {
        self.wall
    }
}

fn test_clock() -> FixedClock {
    FixedClock {
        now: Cell::new(Instant::now()),
        // Deliberately unrelated to any frozen world time, so a test that
        // commits this value would prove a wall-clock substitution.
        wall: 1_700_000_001_234,
    }
}

fn far_deadline(clock: &FixedClock) -> Deadline {
    Deadline::after(clock.monotonic(), Duration::from_secs(60)).expect("far deadline")
}

fn scheduler(
    backend: BackendDouble,
    limits: StoreLimits,
    ceiling: usize,
) -> AutosaveScheduler<BackendDouble> {
    scheduler_with_interval(backend, limits, TEST_INTERVAL, TEST_BASE, TEST_MAX, ceiling)
}

fn scheduler_with_interval(
    backend: BackendDouble,
    limits: StoreLimits,
    interval: u64,
    base: u64,
    maximum: u64,
    ceiling: usize,
) -> AutosaveScheduler<BackendDouble> {
    let store = StoreMailbox::try_new(limits, backend).expect("mailbox builds");
    let config = SchedulerConfig::try_new(interval, base, maximum, ceiling).expect("test config");
    AutosaveScheduler::try_new(config, store).expect("scheduler builds")
}

fn write_error() -> ServerError {
    ServerError::Io {
        operation: Operation::WritePayload,
        kind: std::io::ErrorKind::PermissionDenied,
    }
}

/// Selection budget one with a first snapshot estimate of two selects the
/// first and defers the second; the separate hard owned-byte ceiling refuses
/// the whole request and returns it dirty; an unloaded chunk's unsaved work
/// is retained and dispatches urgent before autosave.
#[test]
fn selection_first_oversized() {
    // Checked budget: first snapshot estimate two against budget one.
    let (backend, _script) = BackendDouble::new();
    let mut store = scheduler(backend, accepted_limits(), 512 << 20);
    let mut authority = AuthorityDouble::new();
    authority.add_dirty(chunk_snapshot_estimated(0, 1, SaveUrgency::Autosave, 2));
    authority.add_dirty(chunk_snapshot_estimated(1, 1, SaveUrgency::Autosave, 2));

    let report = store
        .poll_tick(
            TEST_INTERVAL,
            SaveBudget {
                chunks: 1,
                estimated_bytes: 1,
            },
            &mut authority,
        )
        .expect("tick schedules");
    assert_eq!(report.autosave, 1);
    assert_eq!(report.urgent, 0);
    assert_eq!(report.retry, 0);
    // The chunk selection plus the cadence metadata target share the tick.
    assert_eq!(store.tracked_submits(), 2);
    assert_eq!(authority.in_flight_keys(), vec![(chunk_key(0), 1)]);
    assert_eq!(
        authority.dirty_keys(),
        vec![(chunk_key(0), 1), (chunk_key(1), 1)]
    );

    // Unload retention: the deferred unload snapshot survives the tick and
    // dispatches urgent while autosave stays quiet off cadence.
    let (backend, _script) = BackendDouble::new();
    let mut store = scheduler(backend, accepted_limits(), 512 << 20);
    let mut authority = AuthorityDouble::new();
    authority.add_dirty(chunk_snapshot(2, 1, SaveUrgency::Unload));
    authority.add_dirty(chunk_snapshot(3, 1, SaveUrgency::Autosave));

    let report = store
        .poll_tick(TEST_INTERVAL + 1, SaveBudget::default(), &mut authority)
        .expect("tick schedules");
    assert_eq!(report.urgent, 1);
    assert_eq!(report.autosave, 0);
    assert_eq!(authority.in_flight_keys(), vec![(chunk_key(2), 1)]);
    assert_eq!(
        authority.dirty_keys(),
        vec![(chunk_key(2), 1), (chunk_key(3), 1)]
    );

    // Hard owned-byte ceiling: the whole request refuses and returns dirty.
    let (backend, _script) = BackendDouble::new();
    let tiny = StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 100).expect("tiny byte ceiling");
    let mut store = scheduler(backend, tiny, 512 << 20);
    let mut authority = AuthorityDouble::new();
    authority.add_dirty(chunk_snapshot(0, 1, SaveUrgency::Autosave));
    authority.add_dirty(chunk_snapshot(1, 1, SaveUrgency::Autosave));

    let report = store
        .poll_tick(TEST_INTERVAL, SaveBudget::default(), &mut authority)
        .expect("tick schedules");
    assert_eq!(report.autosave, 0);
    // The chunk request refuses whole while the small cadence metadata
    // target still fits the tiny ceiling.
    assert_eq!(store.tracked_submits(), 1);
    assert!(authority.in_flight_keys().is_empty());
    assert_eq!(authority.dirty_keys().len(), 2);
    assert_eq!(
        store.last_error(),
        Some(ServerError::Capacity {
            resource: Resource::SaveBytes,
            limit: 100,
            observed: CHUNK_RESERVATION,
        })
    );
}

/// Two keys at revision seven where the worker commits the first and fails
/// the second: the first persists with in-flight cleared, the original second
/// stays dirty for retry, and the error surfaces instead of hiding.
#[test]
fn partial_ack() {
    let (backend, script) = BackendDouble::new();
    let mut store = scheduler(backend, accepted_limits(), 512 << 20);
    let mut authority = AuthorityDouble::new();
    authority.add_dirty(chunk_snapshot(0, 7, SaveUrgency::Autosave));
    authority.add_dirty(chunk_snapshot(1, 7, SaveUrgency::Autosave));

    let report = store
        .poll_tick(TEST_INTERVAL, SaveBudget::default(), &mut authority)
        .expect("tick schedules");
    assert_eq!(report.autosave, 2);

    script.borrow_mut().fail_rest = Some(write_error());
    store.drive_workers();
    let report = store
        .poll_tick(TEST_INTERVAL + 1, SaveBudget::default(), &mut authority)
        .expect("tick drains");
    assert_eq!(report.retry, 0);
    assert_eq!(authority.acked, vec![(chunk_key(0), 7)]);
    assert_eq!(authority.persisted_of(&chunk_key(0)), 7);
    // The failed original stays dirty and in flight for its retry cohort.
    assert_eq!(authority.dirty_keys(), vec![(chunk_key(1), 7)]);
    assert_eq!(authority.in_flight_keys(), vec![(chunk_key(1), 7)]);
    assert_eq!(store.pending_retry_jobs(), 1);
    assert_eq!(store.last_error(), Some(write_error()));

    // Past the backoff the original second dispatches once and persists.
    let report = store
        .poll_tick(
            TEST_INTERVAL + 1 + TEST_BASE,
            SaveBudget::default(),
            &mut authority,
        )
        .expect("retry dispatches");
    assert_eq!(report.retry, 1);
    store.drive_workers();
    store
        .poll_tick(
            TEST_INTERVAL + 2 + TEST_BASE,
            SaveBudget::default(),
            &mut authority,
        )
        .expect("retry drains");
    assert!(authority.dirty_keys().is_empty());
    assert!(authority.in_flight_keys().is_empty());
    assert_eq!(authority.persisted_of(&chunk_key(1)), 7);
    assert!(!store.autosave_active());
    let committed: Vec<(SaveKey, u64)> = script
        .borrow()
        .seen_submitted
        .iter()
        .flatten()
        .cloned()
        .collect();
    assert_eq!(
        committed
            .iter()
            .filter(|(_, revision)| *revision == 7)
            .count(),
        3,
        "first plus failed second plus retried second"
    );
}

/// Same shape with no error but the second key omitted: a hard error raises,
/// the second retries, and the first acknowledgment stands.
#[test]
fn nil_error_omission() {
    let (backend, script) = BackendDouble::new();
    let mut store = scheduler(backend, accepted_limits(), 512 << 20);
    let mut authority = AuthorityDouble::new();
    authority.add_dirty(chunk_snapshot(0, 7, SaveUrgency::Autosave));
    authority.add_dirty(chunk_snapshot(1, 7, SaveUrgency::Autosave));

    store
        .poll_tick(TEST_INTERVAL, SaveBudget::default(), &mut authority)
        .expect("tick schedules");
    script.borrow_mut().omit_rest = true;
    store.drive_workers();
    store
        .poll_tick(TEST_INTERVAL + 1, SaveBudget::default(), &mut authority)
        .expect("tick drains");

    assert_eq!(authority.acked, vec![(chunk_key(0), 7)]);
    assert_eq!(authority.persisted_of(&chunk_key(0)), 7);
    assert_eq!(authority.dirty_keys(), vec![(chunk_key(1), 7)]);
    assert_eq!(authority.in_flight_keys(), vec![(chunk_key(1), 7)]);
    assert_eq!(store.pending_retry_jobs(), 1);
    assert_eq!(
        store.last_error(),
        Some(ServerError::Internal {
            invariant: "save omitted submitted snapshot",
        })
    );
}

/// Snapshot seven against current nine with independent answers six, ten,
/// eight and nine: six and ten never ack, eight applies a bounded persisted
/// update after the old release, nine claims no foreign content, and no newer
/// in-flight entry clears.
#[test]
fn future_stale_foreign_ack() {
    for (committed, want_persisted, want_errors, want_in_flight) in [
        (6u64, 0u64, 1usize, 7u64),
        (10u64, 0u64, 1usize, 9u64),
        (8u64, 8u64, 0usize, 9u64),
        (9u64, 0u64, 0usize, 9u64),
    ] {
        let (backend, script) = BackendDouble::new();
        script.borrow_mut().committed_override = Some(vec![(chunk_key(5), committed)]);
        let mut store = scheduler(backend, accepted_limits(), 512 << 20);
        let mut authority = AuthorityDouble::new();
        authority.add_dirty(chunk_snapshot(5, 7, SaveUrgency::Autosave));
        store
            .poll_tick(TEST_INTERVAL, SaveBudget::default(), &mut authority)
            .expect("snapshot submits");
        // A newer revision arrives while revision seven stays in flight.
        authority.dirty.retain(|held| held.key != chunk_key(5));
        authority.add_dirty(chunk_snapshot(5, 9, SaveUrgency::Autosave));

        store.drive_workers();
        {
            let seen = script.borrow();
            assert_eq!(
                seen.seen_submitted.len(),
                2,
                "chunk job plus cadence metadata job"
            );
            assert_eq!(
                seen.seen_submitted[0],
                vec![(chunk_key(5), 7)],
                "answer applies to the submitted revision seven"
            );
        }
        let before = authority.apply_calls;
        store
            .poll_tick(TEST_INTERVAL + 1, SaveBudget::default(), &mut authority)
            .expect("answer drains");
        // The chunk answer plus the cadence metadata acknowledgment.
        assert_eq!(authority.apply_calls, before + 2);
        assert!(authority.acked.is_empty(), "no false ack for {committed}");
        assert_eq!(
            authority.persisted_of(&chunk_key(5)),
            want_persisted,
            "bounded update only for {committed}"
        );
        assert_eq!(
            store.last_error().is_some(),
            want_errors == 1,
            "invalid answers report for {committed}"
        );
        assert_eq!(
            authority.in_flight_keys(),
            vec![(chunk_key(5), want_in_flight)],
            "the stale answer retains seven, later answers release it and the
             newer nine selects in the same tick"
        );
        assert_eq!(authority.dirty_keys(), vec![(chunk_key(5), 9)]);
    }

    // After the bounded eight update the newer nine submits exactly once and
    // never selects twice.
    let (backend, script) = BackendDouble::new();
    script.borrow_mut().committed_override = Some(vec![(chunk_key(5), 8)]);
    let mut store = scheduler(backend, accepted_limits(), 512 << 20);
    let mut authority = AuthorityDouble::new();
    authority.add_dirty(chunk_snapshot(5, 7, SaveUrgency::Autosave));
    store
        .poll_tick(TEST_INTERVAL, SaveBudget::default(), &mut authority)
        .expect("snapshot submits");
    authority.dirty.retain(|held| held.key != chunk_key(5));
    authority.add_dirty(chunk_snapshot(5, 9, SaveUrgency::Autosave));
    store.drive_workers();
    store
        .poll_tick(TEST_INTERVAL + 1, SaveBudget::default(), &mut authority)
        .expect("answer drains");
    assert_eq!(authority.persisted_of(&chunk_key(5)), 8);
    assert_eq!(authority.in_flight_keys(), vec![(chunk_key(5), 9)]);
    assert_eq!(store.tracked_submits(), 1);
    assert!(
        authority
            .select(SaveMode::All, SaveBudget::default())
            .is_empty()
    );
}

/// Backoff delays double from the base with a cap-before-double ceiling, the
/// next tick saturates instead of wrapping, a full queue preserves the cohort
/// attempt, and backpressure enters at the ceiling and exits strictly below
/// ninety percent.
#[test]
fn retry_and_hysteresis() {
    // Delays for attempts one through eight with base twenty and max 1200.
    let delays: Vec<u64> = (1..=8)
        .map(|attempt| retry_delay(20, 1200, attempt))
        .collect();
    assert_eq!(delays, vec![20, 40, 80, 160, 320, 640, 1200, 1200]);

    // Next-tick saturation: a backoff that would wrap stops at the maximum.
    let (backend, script) = BackendDouble::new();
    let store_config =
        SchedulerConfig::try_new(1000, u64::MAX, u64::MAX, 100).expect("saturating config");
    let mailbox = StoreMailbox::try_new(accepted_limits(), backend).expect("mailbox builds");
    let mut store = AutosaveScheduler::try_new(store_config, mailbox).expect("scheduler builds");
    let mut authority = AuthorityDouble::new();
    authority.add_dirty(chunk_snapshot(0, 1, SaveUrgency::Autosave));
    store
        .poll_tick(1000, SaveBudget::default(), &mut authority)
        .expect("snapshot submits");
    script.borrow_mut().fail_next_write = Some(write_error());
    store.drive_workers();
    store
        .poll_tick(1001, SaveBudget::default(), &mut authority)
        .expect("failure drains");
    // Without saturation the sum would wrap far below the maximum tick.
    assert_eq!(store.pending_retry_state(), vec![(1, u64::MAX)]);

    // A full queue preserves the cohort without advancing its attempt.
    let (backend, script) = BackendDouble::new();
    let mut store = scheduler_with_interval(backend, accepted_limits(), 1000, 20, 1200, 100);
    let mut authority = AuthorityDouble::new();
    authority.add_dirty(chunk_snapshot(0, 1, SaveUrgency::Autosave));
    store
        .poll_tick(1000, SaveBudget::default(), &mut authority)
        .expect("snapshot submits");
    script.borrow_mut().fail_next_write = Some(write_error());
    store.drive_workers();
    store
        .poll_tick(1001, SaveBudget::default(), &mut authority)
        .expect("failure cohorts");
    assert_eq!(store.pending_retry_state(), vec![(1, 1021)]);
    // Four direct submits fill the four-job ceiling around the cohort.
    for tag in [1u8, 2, 3, 4] {
        store
            .submit(SaveRequest {
                snapshots: vec![player_snapshot(tag, 1)],
            })
            .expect("fills the queue");
    }
    let report = store
        .poll_tick(1021, SaveBudget::default(), &mut authority)
        .expect("full queue blocks dispatch");
    assert_eq!(report.retry, 0);
    assert_eq!(store.pending_retry_state(), vec![(1, 1021)]);
    assert_eq!(store.tracked_submits(), 0);
    assert_eq!(authority.dirty_keys(), vec![(chunk_key(0), 1)]);
    assert_eq!(authority.in_flight_keys(), vec![(chunk_key(0), 1)]);
    assert_eq!(
        store.last_error(),
        Some(ServerError::Capacity {
            resource: Resource::Snapshots,
            limit: 4,
            observed: 5,
        })
    );

    // Hysteresis boundaries mirrored from the Go backpressure test.
    assert!(next_backpressure(false, 100, 100));
    assert!(next_backpressure(true, 90, 100));
    assert!(!next_backpressure(true, 89, 100));
    assert!(!next_backpressure(true, 90, 101));
    assert!(next_backpressure(true, 91, 101));

    // Scheduler wiring: unsaved estimates enter backpressure and clear after
    // the acknowledgments drain them below ninety percent.
    let (backend, _script) = BackendDouble::new();
    let mut store = scheduler_with_interval(backend, accepted_limits(), 1000, 20, 1200, 100);
    let mut authority = AuthorityDouble::new();
    authority.add_dirty(chunk_snapshot(0, 1, SaveUrgency::Autosave));
    let report = store
        .poll_tick(1000, SaveBudget::default(), &mut authority)
        .expect("tick schedules");
    assert!(report.backpressured);
    assert!(store.backpressured());
    store.drive_workers();
    let report = store
        .poll_tick(1001, SaveBudget::default(), &mut authority)
        .expect("tick drains");
    assert!(!report.backpressured);
    assert!(!store.backpressured());
}

/// Frozen world time seven thousand against old committed six thousand: a
/// failed final metadata flush preserves the seven-thousand target, the retry
/// succeeds before the store sync, and no wall-clock value substitutes; an
/// expired deadline reports without applying anything.
#[test]
fn final_metadata() {
    let clock = test_clock();
    let (backend, script) = BackendDouble::new();
    let mut store = scheduler(backend, accepted_limits(), 512 << 20);
    let mut authority = AuthorityDouble::new();
    authority.meta_time = 7000;
    authority.meta_seq = 41;
    authority.meta_committed_time = 6000;

    script.borrow_mut().fail_next_write = Some(write_error());
    let failed = store.flush(far_deadline(&clock), &mut authority, &clock);
    assert_eq!(failed, Err(write_error()));
    assert!(store.metadata_pending());
    assert_eq!(authority.meta_committed_time, 6000);
    assert_eq!(script.borrow().seen_metadata_time, vec![7000]);
    assert_eq!(script.borrow().sync_calls, 0);

    let report = store
        .flush(far_deadline(&clock), &mut authority, &clock)
        .expect("retry flushes");
    assert_eq!(report.failed, 0);
    assert_eq!(report.durable, 1);
    assert_eq!(authority.meta_committed_time, 7000);
    assert_eq!(script.borrow().seen_metadata_time, vec![7000, 7000]);
    assert!(!store.metadata_pending());

    // The durability barrier follows the successful metadata flush.
    store
        .sync(far_deadline(&clock))
        .expect("store syncs after flush");
    assert_eq!(script.borrow().sync_calls, 1);

    // An expired deadline reports and preserves every ownership.
    let clock = test_clock();
    let (backend, script) = BackendDouble::new();
    let mut store = scheduler(backend, accepted_limits(), 512 << 20);
    let mut authority = AuthorityDouble::new();
    authority.add_dirty(chunk_snapshot(0, 3, SaveUrgency::Autosave));
    clock.now.set(clock.now.get() + Duration::from_secs(10));
    let expired = Deadline::at(clock.now.get() - Duration::from_secs(1));
    let timed_out = store.flush(expired, &mut authority, &clock);
    assert_eq!(
        timed_out,
        Err(ServerError::Timeout {
            operation: Operation::Flush,
        })
    );
    assert!(script.borrow().seen_submitted.is_empty());
    assert_eq!(authority.dirty_keys(), vec![(chunk_key(0), 3)]);
    assert!(authority.in_flight_keys().is_empty());
    assert_eq!(store.tracked_submits(), 0);
}
