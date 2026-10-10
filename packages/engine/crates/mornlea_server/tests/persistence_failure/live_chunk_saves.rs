//! Borrowed real owners drive requests; successful paths never offer manually.
use mornlea_domain::{ChunkPos, Dimension};
use mornlea_server::core::{acquisition::LiveChunkPhase, generation_worker::GenerationPool};
use mornlea_server::store::{
    disk::{DiskOptions, DiskStore},
    mailbox::StoreMailbox,
    scheduler::{AutosaveScheduler, SchedulerConfig},
};
use mornlea_server::{contracts::*, state::AuthorityState};
use mornlea_storage::{
    Chunk, ChunkSave, ContainerSnapshot, Metadata, MetadataChunkPos, StorageKind,
};
use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};
static ROOTS: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "mornlea-live-chunk-saves-{}-{}",
            std::process::id(),
            ROOTS.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn key(x: i32) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(x, 0),
    }
}
fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        42,
    )
    .unwrap()
}
fn deadline() -> Deadline {
    Deadline::after(Instant::now(), Duration::from_secs(10)).unwrap()
}
fn options() -> DiskOptions {
    DiskOptions {
        region_handle_cap: 1,
        create: Metadata {
            format_version: mornlea_storage::METADATA_CURRENT_VERSION,
            seed: 42,
            spawn_dimension: 0,
            spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
            world_time_ticks: 0,
            day_phase_offset: 0,
            weather_kind: 0,
            weather_ticks_remaining: 0,
            depths_spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
            depths_seed_salt: 0,
            difficulty: 0,
        },
    }
}
fn store(root: &Root) -> AutosaveScheduler<DiskStore> {
    scheduler(DiskStore::open(&root.0, options()).unwrap())
}
fn scheduler<B: DiskBackend + Send + 'static>(disk: B) -> AutosaveScheduler<B> {
    AutosaveScheduler::try_new(
        SchedulerConfig::default(),
        StoreMailbox::try_new_background(
            StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
            disk,
        )
        .unwrap(),
    )
    .unwrap()
}
fn chunk() -> Chunk {
    Chunk {
        sections: vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 2,
                palette: vec![],
                packed: vec![]
            };
            24
        ],
        drops: vec![Default::default(); 32],
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    }
}
fn save_at<B: DiskBackend>(
    store: &mut AutosaveScheduler<B>,
    key: ChunkKey,
    chunk: Chunk,
    revision: u64,
) {
    let t = store
        .submit(SaveRequest {
            snapshots: vec![
                OwnedSnapshot::try_new(
                    SaveKey::Chunk(key),
                    revision,
                    1,
                    SaveUrgency::Autosave,
                    SaveValue::Chunk(ChunkSave {
                        key: mornlea_storage::ChunkKey {
                            dimension: i32::from(key.dimension.get()),
                            x: key.pos.x(),
                            z: key.pos.z(),
                        },
                        revision,
                        chunk,
                    }),
                )
                .unwrap(),
            ],
        })
        .unwrap();
    let until = deadline();
    loop {
        store.drive_workers();
        if let SavePoll::Completed(v) = StoreHandle::poll(store, t) {
            assert!(v.error.is_none());
            break;
        }
        assert!(!until.expired(Instant::now()));
        thread::yield_now();
    }
}

use mornlea_server::core::chunk_driver::ChunkDriver;
fn generated(store: &mut AutosaveScheduler<DiskStore>) -> AuthorityState {
    let mut state = authority();
    state.enable_live_chunks().unwrap();
    state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    let mut pool = GenerationPool::try_new(42, false, 1).unwrap();
    let mut driver = ChunkDriver::new();
    driver
        .start_load(&mut state, store, key(0), deadline())
        .unwrap();
    drain(&mut driver, &mut state, store, &mut pool);
    state.advance_tick(TickBudget::full()).unwrap();
    driver
        .start_generation(&mut state, &mut pool, key(0))
        .unwrap();
    drain(&mut driver, &mut state, store, &mut pool);
    state.advance_tick(TickBudget::full()).unwrap();
    pool.close(deadline()).unwrap();
    state
}
fn drain(
    driver: &mut ChunkDriver,
    state: &mut AuthorityState,
    store: &mut AutosaveScheduler<DiskStore>,
    pool: &mut GenerationPool,
) {
    let until = deadline();
    loop {
        store.drive_workers();
        let r = driver.poll(state, store, pool);
        if r.retained == 0 {
            assert!(r.first_error.is_none());
            return;
        }
        assert!(!until.expired(Instant::now()));
        thread::yield_now();
    }
}
fn completion(store: &mut AutosaveScheduler<DiskStore>, ticket: SaveTicket) -> SaveCompletion {
    let until = deadline();
    loop {
        store.drive_workers();
        if let SavePoll::Completed(v) = StoreHandle::poll(store, ticket) {
            return v;
        }
        assert!(!until.expired(Instant::now()));
        thread::yield_now();
    }
}
#[test]
fn actual_generated_dirty_chunk_is_automatically_selected() {
    let root = Root::new();
    let mut store = store(&root);
    let mut state = generated(&mut store);
    let stats = state.save_stats();
    let selected = state.select(SaveMode::All, SaveBudget::default());
    store.close(deadline()).unwrap();
    assert_eq!(stats.dirty, 1);
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].key, SaveKey::Chunk(key(0)));
}
#[test]
fn actual_disk_ack_advances_resident_durability_and_reopens_exact_body() {
    let root = Root::new();
    let mut store = store(&root);
    let mut state = generated(&mut store);
    let selected = state.select(SaveMode::All, SaveBudget::default());
    let snapshot = selected[0].clone();
    let ticket = store
        .submit(SaveRequest {
            snapshots: selected,
        })
        .unwrap();
    let result = completion(&mut store, ticket);
    assert!(result.error.is_none());
    let report = state.apply_completion(result);
    let facts = state.live_chunk_facts(key(0)).unwrap();
    store.close(deadline()).unwrap();
    let mut reopened = DiskStore::open(&root.0, options()).unwrap();
    let loaded = reopened.load(SaveKey::Chunk(key(0))).unwrap();
    reopened.close().unwrap();
    let SaveValue::ChunkView(view) = snapshot.value else {
        panic!("view");
    };
    let LoadedValue::Chunk(loaded) = loaded else {
        panic!("chunk");
    };
    assert_eq!(loaded.chunk, view.materialize().chunk);
    assert_eq!(report.acked, 1);
    assert_eq!(facts.persisted_revision, facts.revision);
    assert_eq!(state.save_stats().dirty, 0);
}

fn loaded(store: &mut AutosaveScheduler<DiskStore>) -> AuthorityState {
    let mut state = authority();
    state.enable_live_chunks().unwrap();
    state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    let mut pool = GenerationPool::try_new(42, false, 1).unwrap();
    let mut driver = ChunkDriver::new();
    driver
        .start_load(&mut state, store, key(0), deadline())
        .unwrap();
    drain(&mut driver, &mut state, store, &mut pool);
    state.advance_tick(TickBudget::full()).unwrap();
    pool.close(deadline()).unwrap();
    state
}
#[test]
fn actual_recovered_body_latest_ack_clears_rewrite_and_keeps_unloading_identity() {
    use mornlea_storage::{
        BANK_A_START_SECTOR, BANK_B_START_SECTOR, BANK_SIZE, RegionKey, SECTOR_SIZE,
        decode_region_bank,
    };
    let root = Root::new();
    let mut store = store(&root);
    let expected = chunk();
    save_at(&mut store, key(0), expected.clone(), 7);
    let mut newer = chunk();
    newer.sections[0].single = 3;
    save_at(&mut store, key(0), newer, 8);
    store.close(deadline()).unwrap();
    let path = root.0.join("dimensions/0/regions/r.0.0.region");
    let mut bytes = fs::read(&path).unwrap();
    let rk = RegionKey {
        dimension: 0,
        x: 0,
        z: 0,
    };
    let banks: Vec<_> = [BANK_A_START_SECTOR, BANK_B_START_SECTOR]
        .into_iter()
        .map(|sector| {
            let at = sector as usize * SECTOR_SIZE as usize;
            decode_region_bank(rk, &bytes[at..at + BANK_SIZE], bytes.len() as i64).unwrap()
        })
        .collect();
    let active = banks.iter().max_by_key(|b| b.generation).unwrap();
    bytes[active.entries[0].offset_sector as usize * SECTOR_SIZE as usize] ^= 0xff;
    fs::write(&path, bytes).unwrap();
    let mut store = self::store(&root);
    let mut state = loaded(&mut store);
    let before = state.live_chunk_facts(key(0)).unwrap();
    assert_eq!(
        (
            before.revision,
            before.persisted_revision,
            before.needs_rewrite,
            before.recovered
        ),
        (9, 7, true, true)
    );
    state.replace_chunk_wants(BTreeSet::new()).unwrap();
    let snapshots = state.select(SaveMode::Urgent, SaveBudget::default());
    assert_eq!(snapshots.len(), 1);
    let SaveValue::ChunkView(view) = &snapshots[0].value else {
        panic!("view");
    };
    assert_eq!(view.materialize().chunk, expected);
    let ticket = store.submit(SaveRequest { snapshots }).unwrap();
    assert_eq!(
        state.apply_completion(completion(&mut store, ticket)).acked,
        1
    );
    let f = state.live_chunk_facts(key(0)).unwrap();
    assert_eq!(
        (
            f.phase,
            f.revision,
            f.persisted_revision,
            f.needs_rewrite,
            f.recovered
        ),
        (LiveChunkPhase::Unloading, 9, 9, false, true)
    );
    assert_eq!(state.save_stats(), SaveStats::default());
    state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().generation,
        before.generation
    );
    store.close(deadline()).unwrap();
    let mut reopened = DiskStore::open(&root.0, options()).unwrap();
    let LoadedValue::Chunk(v) = reopened.load(SaveKey::Chunk(key(0))).unwrap() else {
        panic!("chunk");
    };
    reopened.close().unwrap();
    assert_eq!(v.chunk, expected);
    assert_eq!(v.persisted_revision, 9);
}

use mornlea_server::store::io::{DiskIo, IoPhase};
use std::sync::{Arc, Condvar, Mutex, atomic::AtomicU8, mpsc};
struct WriteHook {
    mode: Arc<AtomicU8>,
    entered: mpsc::SyncSender<()>,
    opened: Arc<(Mutex<bool>, Condvar)>,
}
impl DiskIo for WriteHook {
    fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> std::io::Result<()> {
        if point == IoFaultPoint::PayloadWrite && phase == IoPhase::Before {
            match self.mode.swap(0, Ordering::SeqCst) {
                1 => return Err(std::io::ErrorKind::Other.into()),
                2 => {
                    self.entered.send(()).unwrap();
                    let (lock, wake) = &*self.opened;
                    drop(
                        wake.wait_while(lock.lock().unwrap(), |open| !*open)
                            .unwrap(),
                    );
                }
                _ => (),
            }
        }
        Ok(())
    }
}
struct Release(Arc<(Mutex<bool>, Condvar)>);
impl Release {
    fn open(&self) {
        let (lock, wake) = &*self.0;
        *lock.lock().unwrap() = true;
        wake.notify_all();
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        self.open();
    }
}
type HookOwner = (
    AutosaveScheduler<DiskStore>,
    Arc<AtomicU8>,
    mpsc::Receiver<()>,
    Release,
);
fn hooked(root: &Root) -> HookOwner {
    let mode = Arc::new(AtomicU8::new(0));
    let opened = Arc::new((Mutex::new(false), Condvar::new()));
    let (entered, receive) = mpsc::sync_channel(1);
    let hook_mode = mode.clone();
    let hook_opened = opened.clone();
    let disk = DiskStore::with_io(
        &root.0,
        options(),
        Box::new(move || {
            Box::new(WriteHook {
                mode: hook_mode.clone(),
                entered: entered.clone(),
                opened: hook_opened.clone(),
            })
        }),
    )
    .unwrap();
    let store = AutosaveScheduler::try_new(
        SchedulerConfig::try_new(1, 20, 1200, 4_194_304).unwrap(),
        StoreMailbox::try_new_background(
            StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
            disk,
        )
        .unwrap(),
    )
    .unwrap();
    (store, mode, receive, Release(opened))
}
#[test]
fn actual_held_write_preserves_flight_and_rewant_generation_without_blocking_ticks() {
    let root = Root::new();
    let (mut store, mode, entered, release) = hooked(&root);
    let mut state = generated(&mut store);
    let before = state.live_chunk_facts(key(0)).unwrap();
    let selected = state.select(SaveMode::All, SaveBudget::default());
    let exact = selected[0].clone();
    mode.store(2, Ordering::SeqCst);
    let ticket = store
        .submit(SaveRequest {
            snapshots: selected,
        })
        .unwrap();
    store.drive_workers();
    let gated = entered.recv_timeout(Duration::from_secs(10));
    // A supervised observer permits cleanup even if a tick unexpectedly blocks.
    let (returned, receive) = mpsc::sync_channel(1);
    let observer = thread::spawn(move || {
        for _ in 0..32 {
            state.advance_tick(TickBudget::full()).unwrap();
            assert!(
                state
                    .select(SaveMode::All, SaveBudget::default())
                    .is_empty()
            );
            let _ = state.save_stats();
        }
        state.replace_chunk_wants(BTreeSet::new()).unwrap();
        let unloading = state.live_chunk_facts(key(0)).unwrap();
        state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
        returned.send((state, unloading)).unwrap();
    });
    let observed = receive.recv_timeout(Duration::from_secs(1));
    let nonblocking = observed.is_ok();
    release.open();
    let (mut state, unloading) =
        observed.unwrap_or_else(|_| receive.recv_timeout(Duration::from_secs(10)).unwrap());
    observer.join().unwrap();
    let result = completion(&mut store, ticket);
    let exact_echo = result.snapshots == vec![exact];
    let report = state.apply_completion(result);
    store.close(deadline()).unwrap();
    assert!(gated.is_ok() && nonblocking);
    assert_eq!(unloading.phase, LiveChunkPhase::Unloading);
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().generation,
        before.generation
    );
    assert!(exact_echo);
    assert_eq!(report.acked, 1);
    assert_eq!(state.save_stats(), SaveStats::default());
}
struct RealClock;
impl Clock for RealClock {
    fn monotonic(&self) -> Instant {
        Instant::now()
    }
    fn unix_ms(&self) -> i64 {
        0
    }
}
#[test]
fn actual_write_failure_retains_flight_through_scheduler_backoff_and_flush() {
    let root = Root::new();
    let (mut store, mode, _entered, release) = hooked(&root);
    let mut state = generated(&mut store);
    mode.store(1, Ordering::SeqCst);
    store
        .poll_tick(1, SaveBudget::default(), &mut state)
        .unwrap();
    let until = deadline();
    while store.pending_retry_jobs() == 0 {
        store.drive_workers();
        store
            .poll_tick(2, SaveBudget::default(), &mut state)
            .unwrap();
        assert!(!until.expired(Instant::now()));
        thread::yield_now();
    }
    assert_eq!(store.pending_retry_state(), vec![(1, 22)]);
    let retained = state.save_stats();
    assert_eq!((retained.dirty, retained.in_flight), (1, 1));
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().persisted_revision,
        0
    );
    for tick in 3..22 {
        store
            .poll_tick(tick, SaveBudget::default(), &mut state)
            .unwrap();
        assert_eq!(state.save_stats(), retained);
    }
    assert!(
        state
            .select(SaveMode::All, SaveBudget::default())
            .is_empty()
    );
    release.open();
    let flushed = store.flush(deadline(), &mut state, &RealClock).unwrap();
    store.close(deadline()).unwrap();
    assert_eq!(flushed.outstanding, 0);
    assert_eq!(state.save_stats(), SaveStats::default());
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().persisted_revision,
        1
    );
    let mut reopened = DiskStore::open(&root.0, options()).unwrap();
    let loaded = reopened.load(SaveKey::Chunk(key(0))).unwrap();
    reopened.close().unwrap();
    assert!(matches!(loaded,LoadedValue::Chunk(v) if v.revision==1));
}
#[test]
fn actual_admission_refusal_returns_exact_capture_and_allows_fresh_recapture() {
    let root = Root::new();
    let mut store = store(&root);
    let mut state = generated(&mut store);
    store.close(deadline()).unwrap();
    let disk = DiskStore::open(&root.0, options()).unwrap();
    let mut refusing = StoreMailbox::try_new_background(
        StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 1).unwrap(),
        disk,
    )
    .unwrap();
    let selected = state.select(SaveMode::All, SaveBudget::default());
    let old = selected[0].clone();
    let refused = refusing
        .submit(SaveRequest {
            snapshots: selected,
        })
        .unwrap_err();
    assert_eq!(refused.request.snapshots, vec![old.clone()]);
    assert!(matches!(
        refused.error,
        ServerError::Capacity {
            resource: Resource::SaveBytes,
            ..
        }
    ));
    state.return_dirty(refused.request.snapshots.into_iter().next().unwrap());
    let fresh = state
        .select(SaveMode::All, SaveBudget::default())
        .pop()
        .unwrap();
    assert_eq!(
        (fresh.revision, fresh.estimated_bytes),
        (old.revision, old.estimated_bytes)
    );
    assert_ne!(fresh, old);
    refusing.close(deadline()).unwrap();
    let mut store = self::store(&root);
    let ticket = store
        .submit(SaveRequest {
            snapshots: vec![fresh],
        })
        .unwrap();
    assert_eq!(
        state.apply_completion(completion(&mut store, ticket)).acked,
        1
    );
    store.close(deadline()).unwrap();
    assert_eq!(state.save_stats(), SaveStats::default());
}

fn generated_eight(store: &mut AutosaveScheduler<DiskStore>) -> AuthorityState {
    let mut state = authority();
    state.enable_live_chunks().unwrap();
    state
        .replace_chunk_wants((0..8).map(key).collect())
        .unwrap();
    let mut pool = GenerationPool::try_new(42, false, 2).unwrap();
    let mut driver = ChunkDriver::new();
    for x in 0..8 {
        driver
            .start_load(&mut state, store, key(x), deadline())
            .unwrap();
    }
    drain(&mut driver, &mut state, store, &mut pool);
    state.advance_tick(TickBudget::full()).unwrap();
    for x in 0..8 {
        driver
            .start_generation(&mut state, &mut pool, key(x))
            .unwrap();
    }
    drain(&mut driver, &mut state, store, &mut pool);
    state.advance_tick(TickBudget::full()).unwrap();
    pool.close(deadline()).unwrap();
    state
}

#[test]
fn capacity_prefix_actual_eight_generated_chunks_save_and_reopen() {
    let root = Root::new();
    let mut store = store(&root);
    let mut state = generated_eight(&mut store);
    let expected = state.residents().ready_snapshot();
    let first = store
        .poll_tick(6000, SaveBudget::default(), &mut state)
        .unwrap();
    let until = deadline();
    if first.autosave > 0 {
        let mut tick = 6001;
        while state.save_stats() != SaveStats::default() {
            assert!(!until.expired(Instant::now()));
            store.drive_workers();
            store
                .poll_tick(tick, SaveBudget::default(), &mut state)
                .unwrap();
            tick += 1;
            thread::yield_now();
        }
    }
    let stats = state.save_stats();
    let facts: Vec<_> = (0..8)
        .map(|x| state.live_chunk_facts(key(x)).unwrap())
        .collect();
    store.close(deadline()).unwrap();
    assert!(
        first.autosave > 0,
        "all targets refused instead of an admitted prefix"
    );
    assert_eq!(stats, SaveStats::default());
    assert_eq!(expected.len(), 8);
    let mut reopened = DiskStore::open(&root.0, options()).unwrap();
    let loaded: Vec<_> = expected
        .iter()
        .map(|(key, _, _, _)| reopened.load(SaveKey::Chunk(*key)).unwrap())
        .collect();
    reopened.close().unwrap();
    for ((expected, loaded), facts) in expected.iter().zip(loaded).zip(facts) {
        let LoadedValue::Chunk(loaded) = loaded else {
            panic!("chunk");
        };
        assert_eq!(loaded.chunk, expected.3);
        assert_eq!(loaded.revision, expected.2);
        assert_eq!(facts.generation, expected.1);
        assert_eq!(facts.persisted_revision, facts.revision);
    }
}

#[test]
fn capacity_prefix_actual_fresh_flush_drains_eight_without_prior_autosave() {
    let root = Root::new();
    let mut store = store(&root);
    let mut state = generated_eight(&mut store);
    let expected = state.residents().ready_snapshot();
    assert!(state.begin_close());
    let flushed = store.flush(deadline(), &mut state, &RealClock);
    let stats = state.save_stats();
    store.close(deadline()).unwrap();
    assert!(flushed.is_ok(), "fresh final flush failed: {flushed:?}");
    assert_eq!(stats, SaveStats::default());
    let mut reopened = DiskStore::open(&root.0, options()).unwrap();
    let loaded: Vec<_> = expected
        .iter()
        .map(|(key, _, _, _)| reopened.load(SaveKey::Chunk(*key)).unwrap())
        .collect();
    reopened.close().unwrap();
    for (expected, loaded) in expected.iter().zip(loaded) {
        let LoadedValue::Chunk(loaded) = loaded else {
            panic!("chunk");
        };
        assert_eq!(loaded.chunk, expected.3);
        assert_eq!(loaded.revision, expected.2);
    }
}
