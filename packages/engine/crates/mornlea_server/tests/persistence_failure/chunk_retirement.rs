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
struct Root(PathBuf, bool);
impl Root {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "mornlea-chunk-retirement-{}-{}",
            std::process::id(),
            ROOTS.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p, true)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        if self.1 {
            let _ = fs::remove_dir_all(&self.0);
        }
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
                single: 0,
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

use mornlea_domain::BlockPos;
use mornlea_server::core::{
    chunk_driver::ChunkDriver,
    chunk_retirement::{ChunkRetirementPort, RetiredChunkId},
    retirement_worker::BackgroundChunkRetirement,
    world::ChunkSaveView,
};

// Worker closes precede root deletion, including assertion-unwind cleanup.
struct Fixture {
    retirement: BackgroundChunkRetirement,
    generation: GenerationPool,
    store: AutosaveScheduler<DiskStore>,
    root: Root,
}
impl Fixture {
    fn new() -> Self {
        let root = Root::new();
        let store = store(&root);
        Self {
            retirement: BackgroundChunkRetirement::try_new().unwrap(),
            generation: GenerationPool::try_new(42, false, 1).unwrap(),
            store,
            root,
        }
    }
    fn drain(&mut self, driver: &mut ChunkDriver, state: &mut AuthorityState) {
        let until = deadline();
        loop {
            self.store.drive_workers();
            let report = driver.poll(state, &mut self.store, &mut self.generation);
            if report.retained == 0 {
                assert!(report.first_error.is_none());
                return;
            }
            assert!(!until.expired(Instant::now()));
            thread::yield_now();
        }
    }
    fn load(&mut self, state: &mut AuthorityState) {
        let mut driver = ChunkDriver::new();
        driver
            .start_load(state, &mut self.store, key(0), deadline())
            .unwrap();
        self.drain(&mut driver, state);
        state.advance_tick(TickBudget::full()).unwrap();
    }
    fn disposed(&mut self) -> RetiredChunkId {
        let until = deadline();
        loop {
            let reports = self.retirement.collect(1).unwrap();
            if let Some(id) = reports.first() {
                assert_eq!(reports.len(), 1);
                assert_eq!(self.retirement.occupied(), 0);
                return *id;
            }
            assert!(!until.expired(Instant::now()));
            thread::yield_now();
        }
    }
    fn close(&mut self) {
        self.retirement.close(deadline()).unwrap();
        self.generation.close(deadline()).unwrap();
        self.store.close(deadline()).unwrap();
    }
    fn reopened(&mut self) -> RecoveredChunk {
        self.close();
        let mut disk = DiskStore::open(&self.root.0, options()).unwrap();
        let LoadedValue::Chunk(loaded) = disk.load(SaveKey::Chunk(key(0))).unwrap() else {
            panic!("actual reopened chunk")
        };
        disk.close().unwrap();
        loaded
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let retired = self.retirement.close(deadline());
        let generated = self.generation.close(deadline());
        let stored = self.store.close(deadline());
        self.root.1 = retired.is_ok() && generated.is_ok() && stored.is_ok();
    }
}
fn live() -> AuthorityState {
    let mut state = authority();
    state.enable_live_chunks().unwrap();
    state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    state
}
fn capture(state: &AuthorityState) -> ChunkSaveView {
    let SaveValue::ChunkView(v) = state
        .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
        .unwrap()
        .value
    else {
        panic!("actual lazy capture")
    };
    v
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
fn assert_dues(state: &AuthorityState, pos: BlockPos) {
    assert_eq!(
        state.fluid_schedule().pending_fluid(Dimension::OVERWORLD),
        1
    );
    assert_eq!(
        state
            .farmland_schedule()
            .pending_candidates(Dimension::OVERWORLD),
        1
    );
    assert_eq!(
        state.fluid_schedule().fluid_due(Dimension::OVERWORLD, pos),
        Some(100)
    );
    assert_eq!(
        state
            .farmland_schedule()
            .candidate_due(Dimension::OVERWORLD, pos),
        Some(100)
    );
}
#[test]
fn actual_loaded_clean_retire_rewant_and_disk_reload_preserve_body() {
    let mut fixture = Fixture::new();
    let stored = chunk();
    save_at(&mut fixture.store, key(0), stored.clone(), 9);
    let mut state = live();
    fixture.load(&mut state);
    let original = state.live_chunk_facts(key(0)).unwrap();
    assert_eq!(
        (
            original.phase,
            original.revision,
            original.persisted_revision
        ),
        (LiveChunkPhase::Ready, 9, 9)
    );
    let old = capture(&state);
    state.replace_chunk_wants(BTreeSet::new()).unwrap();
    state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().generation,
        original.generation
    );
    assert_eq!(
        state
            .retire_unwanted_chunks(&mut fixture.retirement, 1)
            .unwrap()
            .retired,
        0
    );
    state.replace_chunk_wants(BTreeSet::new()).unwrap();
    let pos = BlockPos::new(0, -64, 0);
    state.fluid_schedule_mut().enqueue_fluid(key(0), pos, 100);
    state
        .farmland_schedule_mut()
        .enqueue_candidate(key(0), pos, 100);
    assert_dues(&state, pos);
    let report = state
        .retire_unwanted_chunks(&mut fixture.retirement, 1)
        .unwrap();
    assert_eq!((report.retired, report.first_error), (1, None));
    assert!(state.live_chunk_facts(key(0)).is_none());
    assert!(
        state
            .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
            .is_none()
    );
    let residents = state.residents();
    assert!(residents.ready_snapshot().is_empty());
    assert!(residents.blocks.is_empty());
    assert_dues(&state, pos);
    let id = fixture.disposed();
    assert_eq!((id.key(), id.generation()), (key(0), original.generation));
    state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    fixture.load(&mut state);
    let reloaded = state.live_chunk_facts(key(0)).unwrap();
    assert!(reloaded.generation > original.generation);
    assert_eq!((reloaded.revision, reloaded.persisted_revision), (9, 9));
    assert_eq!(capture(&state).materialize().chunk, stored);
    assert_eq!(
        (old.generation(), old.revision(), old.materialize().chunk),
        (original.generation, 9, stored.clone())
    );
    assert_dues(&state, pos);
    let reopened = fixture.reopened();
    assert_eq!((reopened.revision, reopened.chunk), (9, stored));
}
#[test]
fn actual_generated_dirty_and_inflight_retire_only_after_real_disk_ack() {
    let mut fixture = Fixture::new();
    let mut state = live();
    fixture.load(&mut state);
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().phase,
        LiveChunkPhase::NeedsGeneration
    );
    let mut driver = ChunkDriver::new();
    driver
        .start_generation(&mut state, &mut fixture.generation, key(0))
        .unwrap();
    fixture.drain(&mut driver, &mut state);
    state.advance_tick(TickBudget::full()).unwrap();
    let original = state.live_chunk_facts(key(0)).unwrap();
    assert_eq!(
        (
            original.revision,
            original.persisted_revision,
            original.needs_rewrite,
            original.recovered
        ),
        (1, 0, false, false)
    );
    let old = capture(&state);
    let body = old.materialize().chunk;
    state.replace_chunk_wants(BTreeSet::new()).unwrap();
    assert_eq!(
        state
            .retire_unwanted_chunks(&mut fixture.retirement, 1)
            .unwrap()
            .retired,
        0
    );
    assert_eq!(fixture.retirement.occupied(), 0);
    let selected = state.select(SaveMode::Urgent, SaveBudget::default());
    assert_eq!(selected.len(), 1);
    assert_eq!(
        state
            .retire_unwanted_chunks(&mut fixture.retirement, 1)
            .unwrap()
            .retired,
        0
    );
    let ticket = fixture
        .store
        .submit(SaveRequest {
            snapshots: selected,
        })
        .unwrap();
    let completed = completion(&mut fixture.store, ticket);
    assert!(completed.error.is_none());
    assert_eq!(state.save_stats().in_flight, 1);
    assert_eq!(
        state
            .retire_unwanted_chunks(&mut fixture.retirement, 1)
            .unwrap()
            .retired,
        0
    );
    let ack = state.apply_completion(completed);
    assert_eq!(ack.acked, 1);
    assert!(ack.errors.is_empty());
    let facts = state.live_chunk_facts(key(0)).unwrap();
    assert_eq!(facts.revision, facts.persisted_revision);
    assert!(!facts.needs_rewrite);
    let report = state
        .retire_unwanted_chunks(&mut fixture.retirement, 1)
        .unwrap();
    assert_eq!((report.retired, report.first_error), (1, None));
    assert!(state.live_chunk_facts(key(0)).is_none());
    let id = fixture.disposed();
    assert_eq!((id.key(), id.generation()), (key(0), original.generation));
    state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    fixture.load(&mut state);
    let reload = state.live_chunk_facts(key(0)).unwrap();
    assert!(reload.generation > original.generation);
    assert_eq!(
        (reload.revision, reload.persisted_revision),
        (old.revision(), old.revision())
    );
    assert_eq!(capture(&state).materialize().chunk, body);
    assert_eq!(old.materialize().chunk, body);
    let disk = fixture.reopened();
    assert_eq!((disk.revision, disk.chunk), (old.revision(), body));
}
