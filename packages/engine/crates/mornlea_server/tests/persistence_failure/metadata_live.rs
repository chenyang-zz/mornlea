//! Live climate capture and restart continuity through the actual authority,
//! scheduler, background store owner, and temporary filesystem provider.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mornlea_domain::Weather;
use mornlea_server::contracts::{
    AckReport, Clock, Deadline, DiskBackend, LoadedValue, OwnedSnapshot, SaveAuthority, SaveBudget,
    SaveCompletion, SaveKey, SaveMode, SaveRequest, SaveStats, SaveTicket, SaveUrgency, SaveValue,
    ServerError, ServerLimits, StoreHandle, StoreLimits, TickBudget,
};
use mornlea_server::state::AuthorityState;
use mornlea_server::store::disk::{DiskOptions, DiskStore};
use mornlea_server::store::mailbox::StoreMailbox;
use mornlea_server::store::scheduler::{AutosaveScheduler, SchedulerConfig};
use mornlea_storage::{
    METADATA_CURRENT_VERSION, Metadata, MetadataChunkPos, world_metadata_encoded_len,
};

fn limits() -> ServerLimits {
    ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap()
}

fn metadata() -> Metadata {
    Metadata {
        format_version: METADATA_CURRENT_VERSION,
        seed: 42,
        spawn_dimension: 0,
        spawn_anchor: MetadataChunkPos { x: 3, z: -4 },
        depths_spawn_anchor: MetadataChunkPos { x: -8, z: 9 },
        depths_seed_salt: 0x123456789abcdef0,
        world_time_ticks: 1200,
        day_phase_offset: 6000,
        weather_kind: Weather::Rain.wire_id(),
        weather_ticks_remaining: 100,
        difficulty: 2,
    }
}

fn body(snapshot: &OwnedSnapshot) -> &Metadata {
    let SaveValue::Metadata(metadata) = &snapshot.value else {
        panic!("expected metadata snapshot");
    };
    metadata
}

#[test]
fn actual_tick_reaches_metadata_target() {
    let mut authority = AuthorityState::try_new(limits(), 42).unwrap();
    authority.advance_tick(TickBudget::full()).unwrap();
    let snapshot = authority.try_metadata_snapshot().unwrap();
    assert_eq!(body(&snapshot).world_time_ticks, 1);
}

#[test]
fn new_world_uses_canonical_depths_salt() {
    let authority = AuthorityState::try_new(limits(), 42).unwrap();
    assert_eq!(
        body(&authority.metadata_snapshot()).depths_seed_salt,
        0x9e3779b97f4a7c15
    );
}

#[test]
fn captures_distinct_live_targets_and_preserves_owned_startup_fields() {
    let startup = metadata();
    let mut authority = AuthorityState::try_new_with_metadata(limits(), startup.clone()).unwrap();
    let initial = authority.try_metadata_snapshot().unwrap();
    assert_eq!(body(&initial), &startup);
    assert_eq!(initial.revision, 1);
    assert_eq!(initial.key, SaveKey::Metadata);
    assert_eq!(initial.urgency, SaveUrgency::Autosave);
    assert_eq!(
        initial.estimated_bytes,
        world_metadata_encoded_len(&startup).unwrap()
    );

    authority.advance_tick(TickBudget::full()).unwrap();
    let mut expected = startup.clone();
    expected.world_time_ticks = 1201;
    expected.weather_ticks_remaining = 99;
    let first = authority.try_metadata_snapshot().unwrap();
    assert_eq!(body(&first), &expected);
    assert_eq!(first.revision, 2);
    assert_eq!(authority.try_metadata_snapshot().unwrap(), first);
    assert_eq!(authority.metadata_snapshot(), first);
    assert_eq!(body(&initial), &startup);

    authority.advance_tick(TickBudget::full()).unwrap();
    expected.world_time_ticks = 1202;
    expected.weather_ticks_remaining = 98;
    let second = authority.try_metadata_snapshot().unwrap();
    assert_eq!(body(&second), &expected);
    assert_eq!(second.revision, 3);
    assert_eq!(body(&first).world_time_ticks, 1201);
    assert_eq!(body(&first).weather_ticks_remaining, 99);
}

#[test]
fn live_capture_reads_display_offset_and_weather_but_keeps_startup_difficulty() {
    let mut authority = AuthorityState::try_new_with_metadata(limits(), metadata()).unwrap();
    authority.advance_tick(TickBudget::full()).unwrap();
    let mut residents = authority.residents();
    let environment = residents.environment.as_mut().unwrap();
    environment.day_phase_offset = 12345;
    environment.weather = Weather::Thunder;
    environment.difficulty = 0;
    authority.commit_residents(residents);

    let mut expected = metadata();
    expected.world_time_ticks = 1201;
    expected.weather_ticks_remaining = 99;
    expected.day_phase_offset = 12345;
    expected.weather_kind = Weather::Thunder.wire_id();
    let snapshot = authority.try_metadata_snapshot().unwrap();
    assert_eq!(body(&snapshot), &expected);
    assert_eq!(snapshot.revision, 2);
}

#[test]
fn startup_retains_raw_codec_fields_until_live_capture() {
    let mut raw = metadata();
    raw.spawn_anchor = MetadataChunkPos {
        x: i32::MIN,
        z: i32::MAX,
    };
    raw.depths_spawn_anchor = MetadataChunkPos {
        x: i32::MAX,
        z: i32::MIN,
    };
    raw.depths_seed_salt = 0;
    raw.day_phase_offset = u64::MAX;
    raw.weather_kind = 255;
    let mut authority = AuthorityState::try_new_with_metadata(limits(), raw.clone()).unwrap();
    let snapshot = authority.try_metadata_snapshot().unwrap();
    assert_eq!(body(&snapshot), &raw);
    assert_eq!(snapshot.revision, 1);
}

#[test]
fn startup_refuses_invalid_metadata() {
    let mut invalid = metadata();
    invalid.difficulty = 3;
    assert!(matches!(
        AuthorityState::try_new_with_metadata(limits(), invalid),
        Err(ServerError::InvalidInput { field: "metadata" })
    ));
}

const BOUND: Duration = Duration::from_secs(5);
static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mornlea-live-metadata-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct RealClock;
impl Clock for RealClock {
    fn monotonic(&self) -> Instant {
        Instant::now()
    }
    fn unix_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }
}
fn deadline() -> Deadline {
    Deadline::after(Instant::now(), BOUND).unwrap()
}
fn options() -> DiskOptions {
    DiskOptions {
        create: metadata(),
        region_handle_cap: 1,
    }
}
fn store_limits() -> StoreLimits {
    StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap()
}
fn config() -> SchedulerConfig {
    SchedulerConfig::try_new(6, 20, 1200, 512 << 20).unwrap()
}

/// Observe real submissions without replacing any disk or durability operation.
struct ObservedDisk {
    disk: DiskStore,
    writes: Arc<Mutex<Vec<OwnedSnapshot>>>,
}
impl ObservedDisk {
    fn open(root: &Root) -> (Self, Arc<Mutex<Vec<OwnedSnapshot>>>) {
        let writes = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                disk: DiskStore::open(&root.0, options()).unwrap(),
                writes: writes.clone(),
            },
            writes,
        )
    }
}
impl DiskBackend for ObservedDisk {
    fn write(&mut self, ticket: SaveTicket, request: SaveRequest) -> SaveCompletion {
        self.writes
            .lock()
            .unwrap()
            .extend(request.snapshots.iter().cloned());
        self.disk.write(ticket, request)
    }
    fn load(&mut self, key: SaveKey) -> Result<LoadedValue, ServerError> {
        self.disk.load(key)
    }
    fn sync(&mut self) -> Result<(), ServerError> {
        self.disk.sync()
    }
    fn close(&mut self) -> Result<(), ServerError> {
        self.disk.close()
    }
}

#[test]
fn background_autosave_and_final_flush_resume_actual_climate_after_reopen() {
    let root = Root::new();
    let (disk, writes) = ObservedDisk::open(&root);
    let store = StoreMailbox::try_new_background(store_limits(), disk).unwrap();
    let mut scheduler = AutosaveScheduler::try_new(config(), store).unwrap();
    let mut authority = AuthorityState::try_new_with_metadata(limits(), metadata()).unwrap();

    authority.advance_tick(TickBudget::full()).unwrap();
    scheduler
        .poll_tick(6, SaveBudget::default(), &mut authority)
        .unwrap();
    let first = scheduler
        .flush(deadline(), &mut authority, &RealClock)
        .unwrap();
    authority.advance_tick(TickBudget::full()).unwrap();
    scheduler
        .poll_tick(12, SaveBudget::default(), &mut authority)
        .unwrap();
    authority.advance_tick(TickBudget::full()).unwrap();
    let final_flush = scheduler
        .flush(deadline(), &mut authority, &RealClock)
        .unwrap();
    let captured = authority.try_metadata_snapshot().unwrap();
    scheduler.sync(deadline()).unwrap();
    scheduler.close(deadline()).unwrap();

    assert_eq!(first.durable, 1);
    assert_eq!(final_flush.durable, 2);
    assert_eq!(final_flush.outstanding, 0);
    let seen = writes.lock().unwrap().clone();
    assert_eq!(
        seen.iter()
            .map(|s| (s.revision, body(s).world_time_ticks))
            .collect::<Vec<_>>(),
        vec![(2, 1201), (3, 1202), (4, 1203)]
    );
    assert_eq!(seen.last(), Some(&captured));

    let mut reopened = DiskStore::open(&root.0, options()).unwrap();
    let LoadedValue::Metadata(restored) = reopened.load(SaveKey::Metadata).unwrap() else {
        panic!("expected loaded metadata");
    };
    reopened.close().unwrap();
    let mut expected = metadata();
    expected.world_time_ticks = 1203;
    expected.weather_ticks_remaining = 97;
    assert_eq!(restored, expected);
    assert_eq!(body(&captured), &restored);

    let mut restarted = AuthorityState::try_new_with_metadata(limits(), restored).unwrap();
    assert_eq!(restarted.try_metadata_snapshot().unwrap().revision, 1);
    restarted.advance_tick(TickBudget::full()).unwrap();
    expected.world_time_ticks = 1204;
    expected.weather_ticks_remaining = 96;
    assert_eq!(body(&restarted.try_metadata_snapshot().unwrap()), &expected);
}

const CAPTURE_ERROR: ServerError = ServerError::InvalidInput { field: "metadata" };

/// Only the capture port refuses; selection and acknowledgment use the actual owner.
struct CaptureRefusal {
    authority: AuthorityState,
    refuse: bool,
    captures: usize,
}
impl CaptureRefusal {
    fn new() -> Self {
        let mut authority = AuthorityState::try_new_with_metadata(limits(), metadata()).unwrap();
        authority.advance_tick(TickBudget::full()).unwrap();
        Self {
            authority,
            refuse: true,
            captures: 0,
        }
    }
}
impl SaveAuthority for CaptureRefusal {
    fn select(&mut self, mode: SaveMode, budget: SaveBudget) -> Vec<OwnedSnapshot> {
        self.authority.select(mode, budget)
    }
    fn return_dirty(&mut self, snapshot: OwnedSnapshot) {
        self.authority.return_dirty(snapshot)
    }
    fn apply_completion(&mut self, completion: SaveCompletion) -> AckReport {
        self.authority.apply_completion(completion)
    }
    fn save_stats(&self) -> SaveStats {
        self.authority.save_stats()
    }
    fn metadata_snapshot(&self) -> OwnedSnapshot {
        self.authority.metadata_snapshot()
    }
    fn try_metadata_snapshot(&mut self) -> Result<OwnedSnapshot, ServerError> {
        self.captures += 1;
        if self.refuse {
            Err(CAPTURE_ERROR)
        } else {
            self.authority.try_metadata_snapshot()
        }
    }
}

#[test]
fn poll_capture_failure_preserves_pending_target_and_recovers_next_tick() {
    let root = Root::new();
    let (disk, writes) = ObservedDisk::open(&root);
    let store = StoreMailbox::try_new(store_limits(), disk).unwrap();
    let mut scheduler = AutosaveScheduler::try_new(config(), store).unwrap();
    let mut authority = CaptureRefusal::new();
    for tick in [6, 7] {
        assert_eq!(
            scheduler.poll_tick(tick, SaveBudget::default(), &mut authority),
            Err(CAPTURE_ERROR)
        );
        assert_eq!(scheduler.last_error(), Some(CAPTURE_ERROR));
        assert!(scheduler.metadata_pending());
        assert_eq!(scheduler.tracked_submits(), 0);
        assert_eq!(scheduler.pending_retry_jobs(), 0);
        assert!(writes.lock().unwrap().is_empty());
    }
    assert_eq!(authority.captures, 2);
    authority.refuse = false;
    scheduler
        .poll_tick(8, SaveBudget::default(), &mut authority)
        .unwrap();
    scheduler
        .flush(deadline(), &mut authority, &RealClock)
        .unwrap();
    scheduler.close(deadline()).unwrap();
    assert!(!scheduler.metadata_pending());
    let seen = writes.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].revision, 2);
    assert_eq!(body(&seen[0]).world_time_ticks, 1201);
}

#[test]
fn flush_capture_failure_retains_owner_and_retries_without_submitting_stale_bytes() {
    let root = Root::new();
    let (disk, writes) = ObservedDisk::open(&root);
    let store = StoreMailbox::try_new_background(store_limits(), disk).unwrap();
    let mut scheduler = AutosaveScheduler::try_new(config(), store).unwrap();
    let mut authority = CaptureRefusal::new();
    assert_eq!(
        scheduler.flush(deadline(), &mut authority, &RealClock),
        Err(CAPTURE_ERROR)
    );
    assert_eq!(scheduler.last_error(), Some(CAPTURE_ERROR));
    assert!(scheduler.metadata_pending());
    assert_eq!(scheduler.tracked_submits(), 0);
    assert!(writes.lock().unwrap().is_empty());

    authority.refuse = false;
    let report = scheduler
        .flush(deadline(), &mut authority, &RealClock)
        .unwrap();
    scheduler.close(deadline()).unwrap();
    assert_eq!(report.durable, 1);
    assert_eq!(report.outstanding, 0);
    assert!(!scheduler.metadata_pending());
    let seen = writes.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(body(&seen[0]).world_time_ticks, 1201);
}

#[test]
fn flush_capture_failure_drains_tracked_metadata_before_same_owner_recovery() {
    let root = Root::new();
    let (disk, writes) = ObservedDisk::open(&root);
    let store = StoreMailbox::try_new_background(store_limits(), disk).unwrap();
    let mut scheduler = AutosaveScheduler::try_new(config(), store).unwrap();
    let mut authority = CaptureRefusal::new();
    authority.refuse = false;
    scheduler
        .poll_tick(6, SaveBudget::default(), &mut authority)
        .unwrap();
    assert_eq!(scheduler.tracked_submits(), 1);
    authority
        .authority
        .advance_tick(TickBudget::full())
        .unwrap();
    authority.refuse = true;
    assert_eq!(
        scheduler.flush(deadline(), &mut authority, &RealClock),
        Err(CAPTURE_ERROR)
    );
    assert!(scheduler.metadata_pending());
    assert_eq!(scheduler.tracked_submits(), 0);
    assert_eq!(writes.lock().unwrap().len(), 1);

    authority.refuse = false;
    let recovered = scheduler
        .flush(deadline(), &mut authority, &RealClock)
        .unwrap();
    scheduler.close(deadline()).unwrap();
    assert_eq!(recovered.durable, 1);
    assert_eq!(recovered.outstanding, 0);
    let seen = writes.lock().unwrap();
    assert_eq!(
        seen.iter()
            .map(|s| (s.revision, body(s).world_time_ticks))
            .collect::<Vec<_>>(),
        vec![(2, 1201), (3, 1202)]
    );
}
