//! Durable store mailbox behavior over the frozen `StoreHandle` surface.
//!
//! These cases drive the `StoreMailbox` provider with an in-memory backend
//! double that records committed revisions, scripts write failures and stale
//! ticket completions, and in the end-to-end case keeps its commit markers in
//! a temporary world directory the test owns and removes. The double never
//! claims disk persistence; the real region and atomic-file backends join at
//! their own nodes. The authority double mirrors the frozen dirty/in-flight
//! ack semantics: an acked key clears only the matching in-flight revision,
//! uncommitted ownership stays dirty for retry, and a stale revision cannot
//! clear a newer in-flight snapshot.

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mornlea_domain::{ChunkPos, Dimension, PlayerId};
use mornlea_server::contracts::{
    AckReport, ChunkKey, Clock, Deadline, DiskBackend, FlushReport, Operation, OwnedSnapshot,
    Resource, SaveAuthority, SaveBudget, SaveCompletion, SaveKey, SaveMode, SaveOccupancy,
    SavePoll, SaveRequest, SaveStats, SaveTicket, SaveUrgency, SaveValue, ServerError, StoreHandle,
    StoreLimits,
};
use mornlea_server::store::mailbox::StoreMailbox;
use mornlea_storage::{
    CHUNK_CURRENT_SCHEMA, CHUNK_ENVELOPE_LENGTH, ChestSlot, Chunk, ChunkCodec, ChunkSave,
    CompanionSave, ContainerSnapshot, DropSlot, FurnaceSlot, HostileMobsSave, Inventory, ItemStack,
    MAX_COMPRESSED_CHUNK, METADATA_CURRENT_VERSION, Metadata, MetadataChunkPos, PassiveMobsSave,
    PlayerLocation, PlayerSave, StorageKind, chunk_logical_len, companions_encoded_len,
    hostile_mobs_encoded_len, passive_mobs_encoded_len, player_encoded_len,
    world_metadata_encoded_len,
};

/// The frozen chunk reservation: compression maximum plus envelope.
const CHUNK_RESERVATION: usize = MAX_COMPRESSED_CHUNK as usize + CHUNK_ENVELOPE_LENGTH;

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

fn player_save(tag: u8, revision: u64) -> PlayerSave {
    PlayerSave {
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
    }
}

fn player_snapshot(tag: u8, revision: u64) -> OwnedSnapshot {
    let save = player_save(tag, revision);
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

fn companions_save(revision: u64) -> CompanionSave {
    CompanionSave {
        revision,
        agent_namespace_id: mornlea_storage::PlayerId::from_bytes(uuid(0xd0)),
        records: Vec::new(),
        lifecycles: Vec::new(),
        queues: Vec::new(),
    }
}

fn companions_snapshot(revision: u64) -> OwnedSnapshot {
    let save = companions_save(revision);
    let estimated = companions_encoded_len(&save).expect("companion aggregate encodes");
    OwnedSnapshot::try_new(
        SaveKey::Companions,
        revision,
        estimated,
        SaveUrgency::Autosave,
        SaveValue::Companions(save),
    )
    .expect("companions snapshot")
}

fn hostiles_save(revision: u64) -> HostileMobsSave {
    HostileMobsSave {
        revision,
        records: Vec::new(),
    }
}

fn hostiles_snapshot(revision: u64) -> OwnedSnapshot {
    let save = hostiles_save(revision);
    let estimated = hostile_mobs_encoded_len(&save).expect("hostile aggregate encodes");
    OwnedSnapshot::try_new(
        SaveKey::Hostiles,
        revision,
        estimated,
        SaveUrgency::Autosave,
        SaveValue::Hostiles(save),
    )
    .expect("hostiles snapshot")
}

fn passives_snapshot(revision: u64) -> OwnedSnapshot {
    let save = PassiveMobsSave {
        revision,
        records: Vec::new(),
    };
    let estimated = passive_mobs_encoded_len(&save).expect("passive aggregate encodes");
    OwnedSnapshot::try_new(
        SaveKey::Passives,
        revision,
        estimated,
        SaveUrgency::Autosave,
        SaveValue::Passives(save),
    )
    .expect("passives snapshot")
}

fn metadata_value() -> Metadata {
    Metadata {
        format_version: METADATA_CURRENT_VERSION,
        seed: 7,
        spawn_dimension: 0,
        spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
        world_time_ticks: 600,
        day_phase_offset: 0,
        weather_kind: 0,
        weather_ticks_remaining: 0,
        depths_spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
        depths_seed_salt: 0,
        difficulty: 0,
    }
}

fn snapshot_metadata(revision: u64) -> OwnedSnapshot {
    let value = metadata_value();
    let estimated = world_metadata_encoded_len(&value).expect("metadata encodes");
    OwnedSnapshot::try_new(
        SaveKey::Metadata,
        revision,
        estimated,
        SaveUrgency::Autosave,
        SaveValue::Metadata(value),
    )
    .expect("metadata snapshot")
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

fn chunk_snapshot(index: i32, revision: u64) -> OwnedSnapshot {
    let save = ChunkSave {
        key: mornlea_storage::ChunkKey {
            dimension: 0,
            x: index,
            z: 0,
        },
        revision,
        chunk: empty_chunk(),
    };
    chunk_logical_len(&save, CHUNK_CURRENT_SCHEMA).expect("chunk snapshot validates");
    OwnedSnapshot::try_new(
        chunk_key(index),
        revision,
        CHUNK_RESERVATION,
        SaveUrgency::Autosave,
        SaveValue::Chunk(save),
    )
    .expect("chunk snapshot")
}

fn request(snapshots: Vec<OwnedSnapshot>) -> SaveRequest {
    SaveRequest { snapshots }
}

/// Encoded length the mailbox must shrink a sample chunk reservation to.
/// Chunk coordinates live inside the compressed frame, so the length is per
/// coordinate even though the content is otherwise identical.
fn sample_chunk_encoded_len(index: i32) -> usize {
    let save = ChunkSave {
        key: mornlea_storage::ChunkKey {
            dimension: 0,
            x: index,
            z: 0,
        },
        revision: 1,
        chunk: empty_chunk(),
    };
    let mut codec = ChunkCodec::try_new().expect("codec builds");
    let mut scratch = vec![0u8; CHUNK_RESERVATION];
    codec
        .encode_into(&save, &mut scratch)
        .expect("sample chunk encodes")
}

fn unused() -> ServerError {
    ServerError::Internal {
        invariant: "mailbox double: unused port",
    }
}

static WORLD_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Temporary world directory for the one end-to-end case; removed on drop.
struct TempWorld {
    path: PathBuf,
}

impl TempWorld {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mornlea-server-mailbox-{}-{}-{}",
            label,
            std::process::id(),
            WORLD_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create temp world");
        Self { path }
    }

    fn commit_markers(&self) -> usize {
        fs::read_dir(&self.path)
            .expect("world dir readable")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("commit-"))
            .count()
    }
}

impl Drop for TempWorld {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Script shared with the test so mid-flight behavior can change between
/// worker drives. The double records commits; it never persists a save.
#[derive(Default)]
struct BackendScript {
    fail_next_write: Option<ServerError>,
    /// When the backend receives the write for the first ticket, it returns a
    /// completion wearing the second ticket instead — a duplicate or stale
    /// ticket completion.
    stale_write: Option<(SaveTicket, SaveTicket)>,
    commits: Vec<(SaveTicket, Vec<(SaveKey, u64)>)>,
    sync_calls: usize,
    close_calls: usize,
}

struct BackendDouble {
    script: Rc<RefCell<BackendScript>>,
    commit_world: Option<PathBuf>,
}

impl BackendDouble {
    fn new() -> (Self, Rc<RefCell<BackendScript>>) {
        Self::with_world(None)
    }

    fn with_world(commit_world: Option<PathBuf>) -> (Self, Rc<RefCell<BackendScript>>) {
        let script = Rc::new(RefCell::new(BackendScript::default()));
        (
            Self {
                script: Rc::clone(&script),
                commit_world,
            },
            script,
        )
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
        if let Some(error) = script.fail_next_write.take() {
            return SaveCompletion {
                ticket,
                snapshots,
                submitted,
                committed: Vec::new(),
                error: Some(error),
            };
        }
        if let Some((job, stale)) = script.stale_write
            && job == ticket
        {
            let committed = script
                .commits
                .iter()
                .find(|(held, _)| held == &stale)
                .map(|(_, committed)| committed.clone())
                .unwrap_or_default();
            return SaveCompletion {
                ticket: stale,
                snapshots,
                submitted,
                committed,
                error: None,
            };
        }
        let committed = submitted.clone();
        script.commits.push((ticket, committed.clone()));
        drop(script);
        if let Some(world) = self.commit_world.clone() {
            for position in 0..committed.len() {
                let marker = world.join(format!("commit-{}-{}", ticket.get(), position));
                fs::write(marker, []).expect("commit marker written");
            }
        }
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
        Err(unused())
    }

    fn sync(&mut self) -> Result<(), ServerError> {
        self.script.borrow_mut().sync_calls += 1;
        Ok(())
    }

    fn close(&mut self) -> Result<(), ServerError> {
        self.script.borrow_mut().close_calls += 1;
        Ok(())
    }
}

fn mailbox(backend: BackendDouble) -> StoreMailbox<BackendDouble> {
    StoreMailbox::try_new(accepted_limits(), backend).expect("mailbox builds")
}

/// Authority double with the frozen per-key ack policy in miniature.
struct AuthorityDouble {
    dirty: Vec<OwnedSnapshot>,
    in_flight: Vec<OwnedSnapshot>,
    acked: Vec<(SaveKey, u64)>,
    stale_acks: usize,
    apply_calls: usize,
}

impl AuthorityDouble {
    fn new() -> Self {
        Self {
            dirty: Vec::new(),
            in_flight: Vec::new(),
            acked: Vec::new(),
            stale_acks: 0,
            apply_calls: 0,
        }
    }

    /// Mirrors selection: the record stays dirty while its clone goes in flight.
    fn mark_selected(&mut self, snapshot: &OwnedSnapshot) {
        self.dirty.push(snapshot.clone());
        self.in_flight.push(snapshot.clone());
    }
}

impl SaveAuthority for AuthorityDouble {
    fn select(&mut self, _mode: SaveMode, _budget: SaveBudget) -> Vec<OwnedSnapshot> {
        // Selection cadence is the scheduler node's contract; these cases
        // drive submit and poll directly.
        Vec::new()
    }

    fn return_dirty(&mut self, snapshot: OwnedSnapshot) {
        self.in_flight
            .retain(|held| held.key != snapshot.key || held.revision != snapshot.revision);
        self.dirty.push(snapshot);
    }

    fn apply_completion(&mut self, completion: SaveCompletion) -> AckReport {
        self.apply_calls += 1;
        let mut acked = 0usize;
        let mut released = 0usize;
        let mut retry = Vec::new();
        let mut survivors = Vec::new();
        for held in self.in_flight.drain(..) {
            let committed = completion
                .committed
                .iter()
                .find(|(key, _)| key == &held.key)
                .map(|(_, revision)| *revision);
            match committed {
                Some(revision) if revision == held.revision => {
                    acked += 1;
                    self.acked.push((held.key.clone(), held.revision));
                    self.dirty.retain(|record| {
                        record.key != held.key || record.revision != held.revision
                    });
                }
                Some(revision) if revision > held.revision => {
                    // A newer revision committed: the old in-flight copy is
                    // released and the selection-time dirty copy stays.
                    released += 1;
                    retry.push(held.clone());
                }
                // A stale revision ack cannot clear a newer in-flight snapshot.
                Some(_) => {
                    self.stale_acks += 1;
                    survivors.push(held);
                }
                // Uncommitted ownership goes back for retry while the
                // selection-time dirty copy stays.
                None => {
                    released += 1;
                    retry.push(held.clone());
                }
            }
        }
        self.in_flight = survivors;
        AckReport {
            acked,
            released,
            retry,
            errors: completion.error.into_iter().collect(),
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
        snapshot_metadata(1)
    }
}

struct FixedClock {
    now: Instant,
}

impl Clock for FixedClock {
    fn monotonic(&self) -> Instant {
        self.now
    }

    fn unix_ms(&self) -> i64 {
        0
    }
}

fn far_deadline() -> (Deadline, FixedClock) {
    let clock = FixedClock {
        now: Instant::now(),
    };
    let deadline = Deadline::after(clock.now, Duration::from_secs(60)).expect("deadline");
    (deadline, clock)
}

/// Seeds one queued, one worker-held, one completed-unconsumed and one retry
/// job with distinct tickets across lanes and proves the encoded byte total
/// charges each allocation exactly once.
#[test]
fn mixed_lanes_count_once() {
    let (backend, script) = BackendDouble::new();
    script.borrow_mut().fail_next_write = Some(ServerError::Io {
        operation: Operation::WritePayload,
        kind: std::io::ErrorKind::PermissionDenied,
    });
    let mut store = mailbox(backend);
    let mut authority = AuthorityDouble::new();

    // A retry job's ownership returns through a failed completion; the
    // consumer re-submits the original snapshots under a fresh ticket.
    let original = snapshot_metadata(41);
    authority.mark_selected(&original);
    let first = store
        .submit(request(vec![original.clone()]))
        .expect("first submission");
    store
        .poll_tick(7, SaveBudget::default(), &mut authority)
        .expect("tick dispatch");
    store.drive_workers();
    let SavePoll::Completed(failure) = store.poll(first) else {
        panic!("failed completion must be ready")
    };
    assert_eq!(failure.ticket, first);
    assert!(failure.committed.is_empty());
    assert!(failure.error.is_some());
    let ack = authority.apply_completion(failure);
    assert_eq!(ack.retry, vec![original.clone()]);

    // One queued (plain), one worker-held, one completed-unconsumed and one
    // retry-queued job, with distinct tickets across four lanes.
    let player_ticket = store
        .submit(request(vec![player_snapshot(1, 5)]))
        .expect("player submission");
    store
        .poll_tick(8, SaveBudget::default(), &mut authority)
        .expect("tick dispatch");
    store.drive_workers();
    let companions_ticket = store
        .submit(request(vec![companions_snapshot(6)]))
        .expect("companions submission");
    store
        .poll_tick(9, SaveBudget::default(), &mut authority)
        .expect("tick dispatch");
    let hostiles_ticket = store
        .submit(request(vec![hostiles_snapshot(7)]))
        .expect("hostiles submission");
    // The retry re-submission happens after the last dispatch, so the retry
    // job stays queued with a fresh ticket.
    let retry_ticket = store.submit(request(ack.retry)).expect("retry submission");
    assert_ne!(retry_ticket, first);

    assert_eq!(store.held_completions(), 1);
    assert_eq!(store.worker_jobs(), 1);
    assert_eq!(store.queued_jobs(), 2);
    assert_ne!(
        player_ticket, companions_ticket,
        "distinct tickets across lanes"
    );
    assert_ne!(companions_ticket, hostiles_ticket);

    // Total owned encoded bytes: each allocation counted exactly once even
    // though it sits completed-unconsumed, worker-held or retry-queued.
    let expected = player_encoded_len(&player_save(1, 5)).expect("player encodes")
        + companions_encoded_len(&companions_save(6)).expect("companions encodes")
        + hostile_mobs_encoded_len(&hostiles_save(7)).expect("hostiles encodes")
        + world_metadata_encoded_len(&metadata_value()).expect("metadata encodes");
    assert_eq!(store.occupancy().encoded_bytes, expected);
    assert_eq!(store.occupancy().jobs, 4);

    // The next job is the cap+1 request and comes back whole, unchanged.
    let passives_request = request(vec![passives_snapshot(8)]);
    let refusal = store
        .submit(passives_request.clone())
        .expect_err("fifth owned job must be refused");
    assert_eq!(
        refusal.error,
        ServerError::Capacity {
            resource: Resource::Snapshots,
            limit: 4,
            observed: 5,
        }
    );
    assert_eq!(refusal.request, passives_request);
    assert_eq!(store.occupancy().jobs, 4);
    assert_eq!(store.occupancy().encoded_bytes, expected);

    // Each state is observable through its ticket.
    assert!(matches!(store.poll(player_ticket), SavePoll::Completed(_)));
    assert!(matches!(store.poll(companions_ticket), SavePoll::Pending));
    assert!(matches!(store.poll(hostiles_ticket), SavePoll::Pending));
    assert!(matches!(store.poll(retry_ticket), SavePoll::Pending));
}

/// Fills one non-chunk lane to its accepted cap; the next ticket for that
/// lane is rejected whole with no count or reservation change.
#[test]
fn non_chunk_snapshot_cap_plus_one() {
    let (backend, _script) = BackendDouble::new();
    let mut store = mailbox(backend);
    for revision in 1..=3 {
        store
            .submit(request(vec![companions_snapshot(revision)]))
            .expect("inside the companions lane cap");
    }
    let before = store.occupancy();
    assert_eq!(before.companions, 3);

    let overflow = request(vec![companions_snapshot(4)]);
    let refusal = store
        .submit(overflow.clone())
        .expect_err("cap+1 must be refused");
    assert_eq!(
        refusal.error,
        ServerError::Capacity {
            resource: Resource::Snapshots,
            limit: 3,
            observed: 4,
        }
    );
    assert_eq!(refusal.request, overflow);
    assert_eq!(store.occupancy(), before);
    assert_eq!(store.queued_jobs(), 3);
}

/// Eight owned chunks are admitted across three bounded waves; the ninth
/// chunk request returns whole with no count or reservation change.
#[test]
fn chunk_lane_eight_then_nine() {
    let (backend, _script) = BackendDouble::new();
    let mut store = mailbox(backend);
    let mut authority = AuthorityDouble::new();
    // Coordinates live inside the compressed frame, so each chunk in the wave
    // carries its own encoded length.
    let wave_len: usize = (0..3).map(sample_chunk_encoded_len).sum();
    let tail_len: usize = (0..2).map(sample_chunk_encoded_len).sum();

    for width in [3usize, 3, 2] {
        let snapshots = (0..width)
            .map(|index| chunk_snapshot(index as i32, 1))
            .collect();
        store
            .submit(request(snapshots))
            .expect("each wave fits every ceiling");
        store
            .poll_tick(1, SaveBudget::default(), &mut authority)
            .expect("tick dispatch");
        store.drive_workers();
    }

    assert_eq!(store.occupancy().chunks, 8);
    // Encoded chunks shrink from the compression maximum to their actual
    // encoded length while the store holds them.
    assert_eq!(store.occupancy().encoded_bytes, 2 * wave_len + tail_len);

    let ninth = request(vec![chunk_snapshot(9, 1)]);
    let refusal = store
        .submit(ninth.clone())
        .expect_err("ninth chunk refused");
    assert_eq!(
        refusal.error,
        ServerError::Capacity {
            resource: Resource::SaveChunks,
            limit: 8,
            observed: 9,
        }
    );
    assert_eq!(refusal.request, ninth);
    assert_eq!(store.occupancy().chunks, 8);
    assert_eq!(store.occupancy().encoded_bytes, 2 * wave_len + tail_len);
}

/// Reservation pressure: three simultaneous maximum chunk reservations fit
/// the encoded ceiling, a fourth is refused without allocation, and the
/// reservation shrinks only after the worker actually encodes.
#[test]
fn encoded_ceiling_plus_one() {
    let (backend, _script) = BackendDouble::new();
    let mut store = mailbox(backend);
    let mut authority = AuthorityDouble::new();

    // Validation before enqueue comes from the existing codec length
    // functions; a malformed record is refused with no allocation and no
    // invented encoded length.
    let malformed = ChunkSave {
        key: mornlea_storage::ChunkKey {
            dimension: 0,
            x: 30,
            z: 0,
        },
        revision: 1,
        chunk: Chunk {
            sections: Vec::new(),
            drops: Vec::new(),
            furnaces: Vec::new(),
            chests: Vec::new(),
        },
    };
    let malformed_request = request(vec![
        OwnedSnapshot::try_new(
            chunk_key(30),
            1,
            CHUNK_RESERVATION,
            SaveUrgency::Autosave,
            SaveValue::Chunk(malformed),
        )
        .expect("snapshot shape"),
    ]);
    let refused = store
        .submit(malformed_request.clone())
        .expect_err("invalid chunk refused before enqueue");
    assert_eq!(
        refused.error,
        ServerError::InvalidInput {
            field: "save_value",
        }
    );
    assert_eq!(refused.request, malformed_request);

    let invalid_player = player_save(2, 0);
    let invalid_request = request(vec![
        OwnedSnapshot::try_new(
            SaveKey::Player(domain_player(2)),
            0,
            128,
            SaveUrgency::Autosave,
            SaveValue::Player(invalid_player),
        )
        .expect("snapshot shape"),
    ]);
    let refused = store
        .submit(invalid_request.clone())
        .expect_err("invalid player refused before enqueue");
    assert_eq!(
        refused.error,
        ServerError::InvalidInput {
            field: "save_value",
        }
    );
    assert_eq!(refused.request, invalid_request);
    assert_eq!(store.occupancy(), SaveOccupancy::default());

    for index in 0..3 {
        store
            .submit(request(vec![chunk_snapshot(index, 1)]))
            .expect("third maximum reservation is inside the ceiling");
    }
    // The compressed size is unavailable before compression: every owned
    // chunk holds the full maximum reservation, not a claimed length.
    assert_eq!(store.occupancy().encoded_bytes, 3 * CHUNK_RESERVATION);
    assert_eq!(store.occupancy().chunks, 3);

    let fourth = request(vec![chunk_snapshot(3, 1)]);
    let refusal = store
        .submit(fourth.clone())
        .expect_err("fourth maximum reservation exceeds the ceiling");
    assert_eq!(
        refusal.error,
        ServerError::Capacity {
            resource: Resource::SaveBytes,
            limit: 4_194_304,
            observed: 4 * CHUNK_RESERVATION,
        }
    );
    assert_eq!(refusal.request, fourth);
    assert_eq!(store.occupancy().encoded_bytes, 3 * CHUNK_RESERVATION);

    // Two workers hold two simultaneous maximum reservations.
    store
        .poll_tick(2, SaveBudget::default(), &mut authority)
        .expect("tick dispatch");
    assert_eq!(store.worker_jobs(), 2);
    assert_eq!(store.occupancy().encoded_bytes, 3 * CHUNK_RESERVATION);

    // After encoding, the two worker-held chunks shrink to their actual
    // encoded lengths; the queued third stays at the maximum.
    let per_first = sample_chunk_encoded_len(0);
    let per_second = sample_chunk_encoded_len(1);
    store.drive_workers();
    assert_eq!(
        store.occupancy().encoded_bytes,
        per_first + per_second + CHUNK_RESERVATION
    );
}

/// A backend write failure returns the original snapshots through the
/// completion with no committed revisions, leaving dirty retention intact.
#[test]
fn failed_write_returns_snapshots() {
    let (backend, script) = BackendDouble::new();
    script.borrow_mut().fail_next_write = Some(ServerError::Io {
        operation: Operation::WritePayload,
        kind: std::io::ErrorKind::PermissionDenied,
    });
    let mut store = mailbox(backend);
    let mut authority = AuthorityDouble::new();

    let snapshot = player_snapshot(3, 11);
    authority.mark_selected(&snapshot);
    let stats_before = authority.save_stats();

    let ticket = store
        .submit(request(vec![snapshot.clone()]))
        .expect("submission");
    store
        .poll_tick(1, SaveBudget::default(), &mut authority)
        .expect("tick dispatch");
    store.drive_workers();

    let SavePoll::Completed(completion) = store.poll(ticket) else {
        panic!("failed completion must be ready")
    };
    assert_eq!(completion.ticket, ticket);
    assert_eq!(completion.snapshots, vec![snapshot.clone()]);
    assert!(completion.committed.is_empty());
    assert_eq!(
        completion.error,
        Some(ServerError::Io {
            operation: Operation::WritePayload,
            kind: std::io::ErrorKind::PermissionDenied,
        })
    );

    // Dirty retention is intact: nothing was applied and the authority's
    // dirty and in-flight accounting is exactly as it was.
    assert_eq!(authority.apply_calls, 0);
    assert_eq!(authority.save_stats(), stats_before);
    assert_eq!(store.occupancy(), SaveOccupancy::default());
}

/// Cancel before durability returns unstarted snapshots; after the backend
/// commit the completion stands and cancel does not revoke it.
#[test]
fn cancel_before_after_durability() {
    let (backend, _script) = BackendDouble::new();
    let mut store = mailbox(backend);
    let mut authority = AuthorityDouble::new();

    let player = player_snapshot(4, 21);
    let hostiles = hostiles_snapshot(22);
    let running = store
        .submit(request(vec![player.clone()]))
        .expect("running submission");
    store
        .poll_tick(1, SaveBudget::default(), &mut authority)
        .expect("tick dispatch");
    let unstarted_ticket = store
        .submit(request(vec![hostiles.clone()]))
        .expect("queued submission");

    let cancelled = store.cancel_pending().expect("cancel");
    assert_eq!(cancelled, vec![hostiles.clone()]);
    assert_eq!(store.queued_jobs(), 0);
    assert_eq!(store.worker_jobs(), 1);
    assert_eq!(store.occupancy().players, 1);
    assert_eq!(store.occupancy().hostiles, 0);
    assert!(matches!(store.poll(unstarted_ticket), SavePoll::Pending));

    store.drive_workers();
    assert_eq!(
        store.cancel_pending().expect("second cancel"),
        Vec::new(),
        "running and completed ownership stays store-held"
    );

    let SavePoll::Completed(completion) = store.poll(running) else {
        panic!("committed completion must be ready")
    };
    assert_eq!(completion.error, None);
    assert_eq!(
        completion.committed,
        vec![(SaveKey::Player(domain_player(4)), 21)]
    );
    assert_eq!(store.occupancy(), SaveOccupancy::default());
}

/// End-to-end in a temporary world directory: submit, backend double commit,
/// poll applies through the authority double, stats transitions match the
/// frozen semantics, and a stale duplicate completion is reported without
/// clearing anything newer.
#[test]
fn dirty_retention_temp_world() {
    let world = TempWorld::new("dirty-retention");
    let (backend, script) = BackendDouble::with_world(Some(world.path.clone()));
    let mut store = mailbox(backend);
    let mut authority = AuthorityDouble::new();

    let metadata = snapshot_metadata(1);
    authority.mark_selected(&metadata);
    let metadata_ticket = store
        .submit(request(vec![metadata.clone()]))
        .expect("metadata submission");

    // Dirty retention until the ack: the authority still counts the record
    // dirty and in flight after the store accepted it.
    let retention = authority.save_stats();
    assert_eq!(retention.dirty, 1);
    assert_eq!(retention.in_flight, 1);

    store
        .poll_tick(1, SaveBudget::default(), &mut authority)
        .expect("tick dispatch");
    store.drive_workers();
    let SavePoll::Completed(completion) = store.poll(metadata_ticket) else {
        panic!("metadata completion must be ready")
    };
    assert_eq!(completion.error, None);
    assert_eq!(completion.committed, vec![(SaveKey::Metadata, 1)]);
    assert_eq!(world.commit_markers(), 1);

    let ack = authority.apply_completion(completion);
    assert_eq!(ack.acked, 1);
    assert_eq!(authority.save_stats(), SaveStats::default());

    // A duplicate/stale ticket completion: the newer chunk write comes back
    // wearing the older ticket.
    let old_chunk = chunk_snapshot(5, 5);
    let new_chunk = chunk_snapshot(5, 7);
    authority.mark_selected(&old_chunk);
    authority.mark_selected(&new_chunk);
    let old_ticket = store
        .submit(request(vec![old_chunk.clone()]))
        .expect("old chunk submission");
    let new_ticket = store
        .submit(request(vec![new_chunk.clone()]))
        .expect("new chunk submission");
    store
        .poll_tick(2, SaveBudget::default(), &mut authority)
        .expect("tick dispatch");
    script.borrow_mut().stale_write = Some((new_ticket, old_ticket));
    store.drive_workers();

    // The genuine completion is reported with its per-key durable revision.
    let SavePoll::Completed(old_completion) = store.poll(old_ticket) else {
        panic!("old chunk completion must be ready")
    };
    assert_eq!(old_completion.committed, vec![(chunk_key(5), 5)]);

    // The stale duplicate is reported: the ticket mismatch is an error and
    // none of its revisions were adopted.
    let SavePoll::Completed(new_completion) = store.poll(new_ticket) else {
        panic!("new chunk completion must be ready")
    };
    assert_eq!(new_completion.ticket, new_ticket);
    assert_eq!(new_completion.committed, Vec::new());
    assert_eq!(
        new_completion.error,
        Some(ServerError::Internal {
            invariant: "store completion ticket",
        })
    );

    let old_ack = authority.apply_completion(old_completion);
    assert_eq!(old_ack.acked, 1);
    // The stale revision ack cannot clear the newer in-flight snapshot.
    assert_eq!(authority.stale_acks, 1);
    assert_eq!(authority.in_flight.len(), 1);
    assert_eq!(authority.in_flight[0].revision, 7);

    let new_ack = authority.apply_completion(new_completion);
    assert_eq!(new_ack.acked, 0);
    assert_eq!(new_ack.errors.len(), 1);
    // The newer snapshot is cleared by nothing: it stays dirty for retry.
    assert_eq!(authority.in_flight.len(), 0);
    assert_eq!(authority.dirty.len(), 1);
    assert_eq!(authority.dirty[0].revision, 7);

    // The store itself ends fully drained and the durable markers match the
    // two genuine backend commits.
    let (deadline, clock) = far_deadline();
    let report = store
        .flush(deadline, &mut authority, &clock)
        .expect("flush");
    assert_eq!(
        report,
        FlushReport {
            durable: 0,
            failed: 0,
            outstanding: 0,
        }
    );
    assert_eq!(store.occupancy(), SaveOccupancy::default());
    assert_eq!(world.commit_markers(), 2);
}
