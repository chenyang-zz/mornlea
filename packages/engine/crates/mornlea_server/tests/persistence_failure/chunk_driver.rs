//! Borrowed real owners drive requests; successful paths never offer manually.
use mornlea_domain::{ChunkPos, Dimension};
use mornlea_server::core::{acquisition::LiveChunkPhase, generation_worker::GenerationPool};
use mornlea_server::store::{
    disk::{DiskOptions, DiskStore},
    mailbox::StoreMailbox,
    scheduler::{AutosaveScheduler, SchedulerConfig},
};
use mornlea_server::{
    contracts::*,
    state::{AuthorityState, TickContext},
};
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
            "mornlea-chunk-driver-{}-{}",
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
fn save<B: DiskBackend>(store: &mut AutosaveScheduler<B>, key: ChunkKey, chunk: Chunk) {
    save_at(store, key, chunk, 9)
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
fn slotted_chunk() -> Chunk {
    use mornlea_domain::{BlockPos, FiniteVec3};
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    let mut saved = chunk();
    saved.drops[31].generation = 19;
    saved.furnaces[31].generation = 20;
    saved.chests[15].generation = 21;
    ctx.preload_ready_chunk(
        mornlea_server::core::world::ReadyChunk::try_new(key(0), 1, 8, saved).unwrap(),
    );
    for (pos, block) in [(BlockPos::new(1, 64, 1), 11), (BlockPos::new(2, 80, 2), 9)] {
        let observation = ctx.read().observation(Dimension::OVERWORLD, pos).unwrap();
        ctx.transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(observation, block).unwrap()],
            )
            .unwrap();
    }
    for reference in ctx.read().container_refs(key(0)) {
        let before = ctx.read().container(reference).unwrap();
        let mut after = before.clone();
        match &mut after.slots {
            ContainerSlots::Chest(items) => {
                items[0] = mornlea_storage::ItemStack {
                    item: 2,
                    count: 3,
                    durability: 0,
                }
            }
            ContainerSlots::Furnace {
                slots,
                fuel,
                progress,
            } => {
                slots[0] = mornlea_storage::ItemStack {
                    item: 6,
                    count: 2,
                    durability: 0,
                };
                *fuel = 20;
                *progress = 3;
            }
        }
        ctx.stage(RuleEffect::Container { before, after }).unwrap();
    }
    let batch = DropBatch::try_new(
        DropSource::System {
            rule: SystemRule::Support,
            tick: 0,
            target: BlockPos::new(3, 64, 3),
        },
        Dimension::OVERWORLD,
        FiniteVec3::try_new([3.5, 64.5, 3.5]).unwrap(),
        vec![mornlea_storage::ItemStack {
            item: 2,
            count: 4,
            durability: 0,
        }],
        5,
    )
    .unwrap();
    ctx.stage(RuleEffect::Drops(batch)).unwrap();
    ctx.resident_snapshot().ready_snapshot().remove(0).3
}

struct CountedDisk {
    disk: DiskStore,
    calls: std::sync::Arc<AtomicU64>,
}
impl DiskBackend for CountedDisk {
    fn load(&mut self, key: SaveKey) -> Result<LoadedValue, ServerError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.disk.load(key)
    }
    fn write(&mut self, ticket: SaveTicket, request: SaveRequest) -> SaveCompletion {
        self.disk.write(ticket, request)
    }
    fn sync(&mut self) -> Result<(), ServerError> {
        self.disk.sync()
    }
    fn close(&mut self) -> Result<(), ServerError> {
        self.disk.close()
    }
}
#[test]
fn existing_live_consumer_tick_does_not_start_saved_key_requests() {
    let root = Root::new();
    let calls = std::sync::Arc::new(AtomicU64::new(0));
    let mut store = StoreGuard::new(scheduler(CountedDisk {
        disk: DiskStore::open(&root.0, options()).unwrap(),
        calls: calls.clone(),
    }));
    save(&mut store.owner, key(0), slotted_chunk());
    let mut state = authority();
    state.enable_live_chunks().unwrap();
    state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    let tick = state.advance_tick(TickBudget::full());
    let absent = state
        .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
        .is_none();
    let closed = store.finish();
    closed.unwrap();
    tick.unwrap();
    assert!(absent);
    assert_eq!(
        calls.load(Ordering::Relaxed),
        0,
        "missing runtime driving witness; the consumer-only contract remains valid"
    );
}

use mornlea_server::core::chunk_driver::{ChunkDriver, ChunkPollReport};
use std::collections::BTreeMap;
fn invalid(field: &'static str) -> ServerError {
    ServerError::InvalidInput { field }
}
fn request(id: u64) -> ChunkRequestId {
    ChunkRequestId::try_new(id).unwrap()
}
fn live(keys: impl IntoIterator<Item = ChunkKey>) -> AuthorityState {
    let mut state = authority();
    state.enable_live_chunks().unwrap();
    state
        .replace_chunk_wants(keys.into_iter().collect())
        .unwrap();
    state
}
type PollTrace = Arc<Mutex<Vec<(bool, ChunkRequestId)>>>;

#[derive(Default)]
struct Loads {
    starts: Vec<(ChunkKey, u64)>,
    polls: Vec<ChunkRequestId>,
    cancels: Vec<ChunkRequestId>,
    trace: Option<PollTrace>,
    alias: Option<ChunkRequestId>,
    refusal: Option<ServerError>,
    results: BTreeMap<ChunkRequestId, ChunkLoadPoll>,
    missing: bool,
}
impl ChunkLoadPort for Loads {
    fn start_chunk(
        &mut self,
        key: ChunkKey,
        generation: u64,
        _: Deadline,
    ) -> Result<ChunkRequestId, ServerError> {
        self.starts.push((key, generation));
        if let Some(error) = self.refusal {
            return Err(error);
        }
        Ok(self
            .alias
            .unwrap_or_else(|| request(self.starts.len() as u64)))
    }
    fn poll_chunk(&mut self, id: ChunkRequestId) -> ChunkLoadPoll {
        self.polls.push(id);
        if let Some(trace) = &self.trace {
            trace.lock().unwrap().push((false, id));
        }
        self.results.remove(&id).unwrap_or(if self.missing {
            ChunkLoadPoll::Loaded(None)
        } else {
            ChunkLoadPoll::Pending
        })
    }
    fn cancel_chunk(&mut self, id: ChunkRequestId) -> Result<(), ServerError> {
        self.cancels.push(id);
        Ok(())
    }
}
#[derive(Default)]
struct Generations {
    starts: Vec<(ChunkKey, u64)>,
    polls: Vec<ChunkRequestId>,
    cancels: Vec<ChunkRequestId>,
    alias: Option<ChunkRequestId>,
    trace: Option<PollTrace>,
    refusal: Option<ServerError>,
    results: BTreeMap<ChunkRequestId, GenerationPoll>,
}
impl GenerationPort for Generations {
    fn start_generation(
        &mut self,
        key: ChunkKey,
        generation: u64,
    ) -> Result<ChunkRequestId, ServerError> {
        self.starts.push((key, generation));
        if let Some(error) = self.refusal {
            return Err(error);
        }
        Ok(self
            .alias
            .unwrap_or_else(|| request(self.starts.len() as u64)))
    }
    fn poll_generation(&mut self, id: ChunkRequestId) -> GenerationPoll {
        self.polls.push(id);
        if let Some(trace) = &self.trace {
            trace.lock().unwrap().push((true, id));
        }
        self.results.remove(&id).unwrap_or(GenerationPoll::Pending)
    }
    fn cancel_generation(&mut self, id: ChunkRequestId) -> Result<(), ServerError> {
        self.cancels.push(id);
        Ok(())
    }
}
#[test]
fn declaration_and_reservation_refusals_precede_provider_calls() {
    let mut driver = ChunkDriver::default();
    let mut loads = Loads::default();
    let mut state = authority();
    assert!(
        driver
            .start_load(&mut state, &mut loads, key(0), deadline())
            .is_err()
    );
    state.enable_live_chunks().unwrap();
    assert!(
        driver
            .start_load(&mut state, &mut loads, key(0), deadline())
            .is_err()
    );
    state.begin_close();
    assert_eq!(
        driver.start_load(&mut state, &mut loads, key(0), deadline()),
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closing
        })
    );
    assert!(loads.starts.is_empty());
    assert_eq!(
        (
            driver.pending_loads(),
            driver.pending_generations(),
            driver.held_completions(),
            driver.last_error()
        ),
        (0, 0, 0, None)
    );
    assert_eq!(
        ChunkPollReport::default(),
        ChunkPollReport {
            polled: 0,
            offered: 0,
            pending: 0,
            retained: 0,
            first_error: None
        }
    );
}
#[test]
fn typed_start_refusals_abort_exactly_without_cancellation() {
    for error in [
        ServerError::Cancelled,
        ServerError::Io {
            operation: Operation::Load,
            kind: std::io::ErrorKind::PermissionDenied,
        },
        ServerError::Storage {
            family: "chunk",
            kind: StorageFailure::FutureVersion,
        },
    ] {
        let mut state = live([key(0)]);
        let mut driver = ChunkDriver::new();
        let mut loads = Loads {
            refusal: Some(error),
            ..Default::default()
        };
        assert_eq!(
            driver.start_load(&mut state, &mut loads, key(0), deadline()),
            Err(error)
        );
        assert_eq!(state.live_chunk_error(key(0)), Some(&error));
        assert_eq!(
            state.live_chunk_facts(key(0)).unwrap().phase,
            LiveChunkPhase::Failed
        );
        assert_eq!(driver.pending_loads(), 0);
        assert_eq!(driver.last_error(), None);
        assert!(loads.cancels.is_empty());
        loads.refusal = None;
        driver
            .start_load(&mut state, &mut loads, key(0), deadline())
            .unwrap();
        loads.missing = true;
        assert_eq!(
            driver
                .poll(&mut state, &mut loads, &mut Generations::default())
                .offered,
            1
        );
        state.advance_tick(TickBudget::full()).unwrap();
        let mut generations = Generations {
            refusal: Some(error),
            ..Default::default()
        };
        assert_eq!(
            driver.start_generation(&mut state, &mut generations, key(0)),
            Err(error)
        );
        assert_eq!(state.live_chunk_error(key(0)), Some(&error));
        assert_eq!(driver.pending_generations(), 0);
        assert!(generations.cancels.is_empty());
    }
}
#[test]
fn eight_per_source_limit_and_duplicate_refuse_before_provider_work() {
    let mut state = live((0..18).map(key));
    let mut driver = ChunkDriver::new();
    let mut loads = Loads::default();
    let mut generations = Generations::default();
    driver
        .start_load(&mut state, &mut loads, key(0), deadline())
        .unwrap();
    assert_eq!(
        driver.start_load(&mut state, &mut loads, key(0), deadline()),
        Err(invalid("chunk_driver_request"))
    );
    for x in 1..8 {
        driver
            .start_load(&mut state, &mut loads, key(x), deadline())
            .unwrap();
    }
    let full = ServerError::Capacity {
        resource: Resource::ChunkRequests,
        limit: 8,
        observed: 9,
    };
    assert_eq!(
        driver.start_load(&mut state, &mut loads, key(8), deadline()),
        Err(full)
    );
    assert_eq!(loads.starts.len(), 8);
    loads.missing = true;
    assert_eq!(
        driver
            .poll(&mut state, &mut loads, &mut generations)
            .offered,
        8
    );
    state.advance_tick(TickBudget::full()).unwrap();
    for x in 0..8 {
        driver
            .start_generation(&mut state, &mut generations, key(x))
            .unwrap();
    }
    for x in 8..16 {
        driver
            .start_load(&mut state, &mut loads, key(x), deadline())
            .unwrap();
    }
    driver.poll(&mut state, &mut loads, &mut generations);
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        driver.start_generation(&mut state, &mut generations, key(8)),
        Err(full)
    );
    assert_eq!(generations.starts.len(), 8);
    assert_eq!(driver.pending_generations(), 8);
    assert!(generations.cancels.is_empty());
}
#[test]
fn offers_keep_authority_lane_charged_until_actual_acquire() {
    let mut state = live((0..9).map(key));
    let mut driver = ChunkDriver::new();
    let mut loads = Loads {
        missing: true,
        ..Default::default()
    };
    let mut generations = Generations::default();
    for x in (0..8).rev() {
        driver
            .start_load(&mut state, &mut loads, key(x), deadline())
            .unwrap();
    }
    assert_eq!(
        driver.poll(&mut state, &mut loads, &mut generations),
        ChunkPollReport {
            polled: 8,
            offered: 8,
            pending: 0,
            retained: 0,
            first_error: None
        }
    );
    assert_eq!(loads.polls, (1..=8).rev().map(request).collect::<Vec<_>>());
    assert_eq!(
        driver.start_load(&mut state, &mut loads, key(8), deadline()),
        Err(ServerError::Capacity {
            resource: Resource::ChunkRequests,
            limit: 8,
            observed: 9
        })
    );
    assert_eq!(loads.starts.len(), 8);
    state.advance_tick(TickBudget::full()).unwrap();
    driver
        .start_load(&mut state, &mut loads, key(8), deadline())
        .unwrap();
    assert_eq!(loads.starts.len(), 9);
}
#[test]
fn duplicate_alias_quarantines_unbound_start_without_polling_or_cancel() {
    let mut state = live([key(0), key(1), key(2)]);
    let mut driver = ChunkDriver::new();
    let mut loads = Loads {
        alias: Some(request(1)),
        ..Default::default()
    };
    driver
        .start_load(&mut state, &mut loads, key(0), deadline())
        .unwrap();
    let original = state.live_chunk_facts(key(0)).unwrap();
    let error = invalid("chunk_request_identity");
    assert_eq!(
        driver.start_load(&mut state, &mut loads, key(1), deadline()),
        Err(error)
    );
    assert_eq!(driver.last_error(), Some(error));
    assert_eq!(
        driver.start_load(&mut state, &mut loads, key(2), deadline()),
        Err(error)
    );
    loads.missing = true;
    let report = driver.poll(&mut state, &mut loads, &mut Generations::default());
    assert_eq!((report.polled, report.offered, report.retained), (1, 1, 1));
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().generation,
        original.generation
    );
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().phase,
        LiveChunkPhase::NeedsGeneration
    );
    assert_eq!(
        state.live_chunk_facts(key(1)).unwrap().phase,
        LiveChunkPhase::Loading
    );
    driver.poll(&mut state, &mut loads, &mut Generations::default());
    assert_eq!(loads.polls, vec![request(1)]);
    assert_eq!(loads.starts.len(), 2);
    assert!(loads.cancels.is_empty());
}
fn prepared(k: ChunkKey, g: u64) -> mornlea_server::core::world::PreparedChunk {
    mornlea_server::core::world::PreparedChunk::try_new(
        k,
        g,
        RecoveredChunk {
            chunk: chunk(),
            revision: 9,
            persisted_revision: 7,
            needs_rewrite: true,
            recovered: true,
        },
    )
    .unwrap()
}
#[test]
fn wrong_prepared_identity_retains_original_and_independent_source_continues() {
    let mut state = live([key(0), key(1), key(2)]);
    let mut driver = ChunkDriver::new();
    let mut loads = Loads {
        missing: true,
        ..Default::default()
    };
    let mut generations = Generations::default();
    driver
        .start_load(&mut state, &mut loads, key(1), deadline())
        .unwrap();
    driver.poll(&mut state, &mut loads, &mut generations);
    state.advance_tick(TickBudget::full()).unwrap();
    let gen_id = driver
        .start_generation(&mut state, &mut generations, key(1))
        .unwrap();
    let load_id = driver
        .start_load(&mut state, &mut loads, key(0), deadline())
        .unwrap();
    loads
        .results
        .insert(load_id, ChunkLoadPoll::Loaded(Some(prepared(key(99), 17))));
    generations
        .results
        .insert(gen_id, GenerationPoll::Failed(ServerError::Cancelled));
    let report = driver.poll(&mut state, &mut loads, &mut generations);
    let error = invalid("chunk_completion_identity");
    assert_eq!(
        (
            report.polled,
            report.offered,
            report.retained,
            report.first_error
        ),
        (2, 1, 1, Some(error))
    );
    assert_eq!(driver.held_completions(), 1);
    assert_eq!(driver.last_error(), Some(error));
    assert_eq!(
        driver.start_generation(&mut state, &mut generations, key(2)),
        Err(error)
    );
    let before = loads.polls.len();
    let again = driver.poll(&mut state, &mut loads, &mut generations);
    assert_eq!(
        (again.polled, again.first_error, again.retained),
        (0, Some(error), 1)
    );
    assert_eq!(loads.polls.len(), before);
    assert!(loads.cancels.is_empty());
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.live_chunk_error(key(1)),
        Some(&ServerError::Cancelled)
    );
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().phase,
        LiveChunkPhase::Loading
    );
}
#[test]
fn closed_authority_retains_completion_without_permanent_fault_and_stop_is_idempotent() {
    let mut state = live([key(0)]);
    let mut driver = ChunkDriver::new();
    let mut loads = Loads {
        missing: true,
        ..Default::default()
    };
    driver
        .start_load(&mut state, &mut loads, key(0), deadline())
        .unwrap();
    state.mark_closed();
    let closed = ServerError::InvalidState {
        phase: ServerPhase::Closed,
    };
    let r = driver.poll(&mut state, &mut loads, &mut Generations::default());
    assert_eq!((r.polled, r.retained, r.first_error), (1, 1, Some(closed)));
    assert_eq!(driver.last_error(), None);
    assert_eq!(driver.held_completions(), 1);
    driver.stop_new();
    driver.stop_new();
    assert_eq!(
        driver.start_load(&mut state, &mut loads, key(0), deadline()),
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closing
        })
    );
    let r = driver.poll(&mut state, &mut loads, &mut Generations::default());
    assert_eq!((r.polled, r.first_error), (0, Some(closed)));
    assert_eq!(loads.starts.len(), 1);
    assert!(loads.cancels.is_empty());
}

// Panic cleanup releases any held backend first, then explicitly closes its
// owner. Only the observed successful close below qualifies a joined owner.
struct StoreGuard<B: DiskBackend> {
    owner: AutosaveScheduler<B>,
    release: Option<Release>,
    closed: bool,
}
impl<B: DiskBackend> StoreGuard<B> {
    fn new(owner: AutosaveScheduler<B>) -> Self {
        Self {
            owner,
            release: None,
            closed: false,
        }
    }
    fn finish(&mut self) -> Result<(), ServerError> {
        if let Some(release) = &self.release {
            release.open();
        }
        let closed = self.owner.close(deadline());
        self.closed = closed.is_ok();
        closed
    }
}
impl<B: DiskBackend> Drop for StoreGuard<B> {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.finish();
        }
    }
}
struct PoolGuard {
    owner: GenerationPool,
    closed: bool,
}
impl PoolGuard {
    fn new() -> Self {
        Self {
            owner: GenerationPool::try_new(42, false, 1).unwrap(),
            closed: false,
        }
    }
    fn finish(&mut self) -> Result<(), ServerError> {
        let closed = self.owner.close(deadline());
        self.closed = closed.is_ok();
        closed
    }
}
impl Drop for PoolGuard {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.finish();
        }
    }
}
fn drive_until_empty<B: DiskBackend>(
    driver: &mut ChunkDriver,
    state: &mut AuthorityState,
    loads: &mut AutosaveScheduler<B>,
    generations: &mut dyn GenerationPort,
) -> ChunkPollReport {
    let until = deadline();
    let mut total = ChunkPollReport::default();
    loop {
        loads.drive_workers();
        let r = driver.poll(state, loads, generations);
        total.polled += r.polled;
        total.offered += r.offered;
        total.pending += r.pending;
        total.retained = r.retained;
        total.first_error = total.first_error.or(r.first_error);
        if r.retained == 0 {
            return total;
        }
        assert!(!until.expired(Instant::now()), "bounded off-tick test wait");
        thread::yield_now();
    }
}
fn materialized(state: &AuthorityState, k: ChunkKey) -> Chunk {
    let SaveValue::ChunkView(view) = state
        .capture_chunk_snapshot(k, SaveUrgency::Unload)
        .unwrap()
        .value
    else {
        panic!("immutable resident view")
    };
    view.materialize().chunk
}
#[test]
fn actual_scheduler_saved_load_installs_exact_slots_heights_and_facts_at_acquire() {
    let root = Root::new();
    let expected = slotted_chunk();
    let mut store = StoreGuard::new(store(&root));
    save(&mut store.owner, key(0), expected.clone());
    let mut state = live([key(0)]);
    let mut driver = ChunkDriver::new();
    let mut pool = PoolGuard::new();
    driver
        .start_load(&mut state, &mut store.owner, key(0), deadline())
        .unwrap();
    let tick_before = state.advance_tick(TickBudget::full());
    let unavailable_before = state
        .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
        .is_none();
    let report = drive_until_empty(&mut driver, &mut state, &mut store.owner, &mut pool.owner);
    let unavailable_staged = state
        .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
        .is_none();
    let acquired = state.advance_tick(TickBudget::full());
    let store_closed = store.finish();
    let pool_closed = pool.finish();
    store_closed.unwrap();
    pool_closed.unwrap();
    tick_before.unwrap();
    acquired.unwrap();
    assert!(unavailable_before && unavailable_staged);
    assert_eq!(
        (report.offered, report.retained, report.first_error),
        (1, 0, None)
    );
    let f = state.live_chunk_facts(key(0)).unwrap();
    assert_eq!(
        (
            f.phase,
            f.generation,
            f.revision,
            f.persisted_revision,
            f.needs_rewrite,
            f.recovered
        ),
        (LiveChunkPhase::Ready, 1, 9, 9, false, false)
    );
    assert_eq!(materialized(&state, key(0)), expected);
}
#[test]
fn actual_missing_load_then_native_generation_has_pending_and_ready_durability() {
    use mornlea_server::core::generation::ChunkGenerator;
    let root = Root::new();
    let mut store = StoreGuard::new(store(&root));
    let mut pool = PoolGuard::new();
    let mut state = live([key(0)]);
    let mut driver = ChunkDriver::new();
    driver
        .start_load(&mut state, &mut store.owner, key(0), deadline())
        .unwrap();
    let loaded = drive_until_empty(&mut driver, &mut state, &mut store.owner, &mut pool.owner);
    let missing_tick = state.advance_tick(TickBudget::full());
    let missing_phase = state.live_chunk_facts(key(0)).unwrap().phase;
    // These real native jobs make the driver's job follow existing CPU work;
    // they are not a deterministic held-native integration gate.
    let warm: Vec<_> = (0..7)
        .map(|_| pool.owner.start_generation(key(1), 1).unwrap())
        .collect();
    driver
        .start_generation(&mut state, &mut pool.owner, key(0))
        .unwrap();
    let pending = driver.poll(&mut state, &mut store.owner, &mut pool.owner);
    let pending_tick = state.advance_tick(TickBudget::full());
    let unavailable = state
        .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
        .is_none();
    let until = deadline();
    for id in warm {
        loop {
            match pool.owner.poll_generation(id) {
                GenerationPoll::Ready(_) => break,
                GenerationPoll::Failed(e) => panic!("native warm job {e:?}"),
                GenerationPoll::Pending => {
                    assert!(!until.expired(Instant::now()));
                    thread::yield_now();
                }
            }
        }
    }
    let completed = drive_until_empty(&mut driver, &mut state, &mut store.owner, &mut pool.owner);
    let acquired = state.advance_tick(TickBudget::full());
    let store_closed = store.finish();
    let pool_closed = pool.finish();
    store_closed.unwrap();
    pool_closed.unwrap();
    missing_tick.unwrap();
    pending_tick.unwrap();
    acquired.unwrap();
    assert_eq!(loaded.offered, 1);
    assert_eq!(missing_phase, LiveChunkPhase::NeedsGeneration);
    assert_eq!(
        (pending.polled, pending.pending, pending.offered),
        (1, 1, 0)
    );
    assert!(unavailable);
    assert_eq!(completed.offered, 1);
    let f = state.live_chunk_facts(key(0)).unwrap();
    assert_eq!(
        (f.phase, f.revision, f.persisted_revision),
        (LiveChunkPhase::Ready, 1, 0)
    );
    let expected = ChunkGenerator::try_new(42, false)
        .unwrap()
        .generate(key(0))
        .unwrap();
    assert_eq!(materialized(&state, key(0)), expected);
}
#[test]
fn actual_staged_loads_keep_eight_aliases_charged_until_tick() {
    let root = Root::new();
    let mut store = StoreGuard::new(store(&root));
    let mut pool = PoolGuard::new();
    let mut state = live((0..9).map(key));
    let mut driver = ChunkDriver::new();
    for x in 0..8 {
        driver
            .start_load(&mut state, &mut store.owner, key(x), deadline())
            .unwrap();
    }
    let staged = drive_until_empty(&mut driver, &mut state, &mut store.owner, &mut pool.owner);
    let refusal = driver.start_load(&mut state, &mut store.owner, key(8), deadline());
    let tick = state.advance_tick(TickBudget::full());
    let fresh = driver.start_load(&mut state, &mut store.owner, key(8), deadline());
    let completed = drive_until_empty(&mut driver, &mut state, &mut store.owner, &mut pool.owner);
    let store_closed = store.finish();
    let pool_closed = pool.finish();
    store_closed.unwrap();
    pool_closed.unwrap();
    tick.unwrap();
    fresh.unwrap();
    assert_eq!(staged.offered, 8);
    assert_eq!(
        refusal,
        Err(ServerError::Capacity {
            resource: Resource::ChunkRequests,
            limit: 8,
            observed: 9
        })
    );
    assert_eq!(completed.offered, 1);
    assert_eq!(driver.pending_loads(), 0);
}
#[test]
fn actual_corrupt_future_reads_offer_typed_failure_and_never_start_generation() {
    use mornlea_storage::{
        BANK_A_START_SECTOR, BANK_B_START_SECTOR, BANK_SIZE, RegionKey, SECTOR_SIZE, crc32c,
        decode_region_bank, encode_region_bank,
    };
    for failure in [StorageFailure::Corrupt, StorageFailure::FutureVersion] {
        let root = Root::new();
        let mut initial = StoreGuard::new(store(&root));
        save(&mut initial.owner, key(0), chunk());
        initial.finish().unwrap();
        let path = root.0.join("dimensions/0/regions/r.0.0.region");
        let mut bytes = fs::read(&path).unwrap();
        let rk = RegionKey {
            dimension: 0,
            x: 0,
            z: 0,
        };
        for sector in [BANK_A_START_SECTOR, BANK_B_START_SECTOR] {
            let at = sector as usize * SECTOR_SIZE as usize;
            let mut bank =
                decode_region_bank(rk, &bytes[at..at + BANK_SIZE], bytes.len() as i64).unwrap();
            let e = &mut bank.entries[0];
            if e.offset_sector == 0 {
                continue;
            }
            let payload = e.offset_sector as usize * SECTOR_SIZE as usize;
            let end = payload + e.payload_length as usize;
            if failure == StorageFailure::Corrupt {
                bytes[payload] ^= 0xff;
            } else {
                bytes[payload + 8..payload + 12].copy_from_slice(&u32::MAX.to_le_bytes());
            }
            e.payload_crc32c = crc32c(&bytes[payload..end]);
            bytes[at..at + BANK_SIZE].copy_from_slice(&encode_region_bank(rk, &bank).unwrap());
        }
        fs::write(&path, &bytes).unwrap();
        let mut store = StoreGuard::new(store(&root));
        let mut pool = PoolGuard::new();
        let mut state = live([key(0)]);
        let mut driver = ChunkDriver::new();
        driver
            .start_load(&mut state, &mut store.owner, key(0), deadline())
            .unwrap();
        let report = drive_until_empty(&mut driver, &mut state, &mut store.owner, &mut pool.owner);
        let tick = state.advance_tick(TickBudget::full());
        let generation = driver.start_generation(&mut state, &mut pool.owner, key(0));
        let native_jobs = pool.owner.owned_jobs();
        let store_closed = store.finish();
        let pool_closed = pool.finish();
        store_closed.unwrap();
        pool_closed.unwrap();
        tick.unwrap();
        assert_eq!(
            (report.offered, report.first_error, driver.last_error()),
            (1, None, None)
        );
        assert_eq!(
            state.live_chunk_error(key(0)),
            Some(&ServerError::Storage {
                family: if failure == StorageFailure::Corrupt {
                    "region"
                } else {
                    "chunk"
                },
                kind: failure
            })
        );
        assert_eq!(
            state.live_chunk_facts(key(0)).unwrap().phase,
            LiveChunkPhase::Failed
        );
        assert!(generation.is_err());
        assert_eq!(native_jobs, 0);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn actual_sixty_four_missing_and_failed_forget_cycles_leave_no_driver_history() {
    let root = Root::new();
    let mut store = StoreGuard::new(store(&root));
    let mut pool = PoolGuard::new();
    let mut state = live([]);
    let mut driver = ChunkDriver::new();
    let mut observations = Vec::new();
    for n in 1..=64 {
        state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
        let until = if n % 2 == 0 {
            Deadline::at(Instant::now())
        } else {
            deadline()
        };
        driver
            .start_load(&mut state, &mut store.owner, key(0), until)
            .unwrap();
        let completed =
            drive_until_empty(&mut driver, &mut state, &mut store.owner, &mut pool.owner);
        state.advance_tick(TickBudget::full()).unwrap();
        let facts = state.live_chunk_facts(key(0)).unwrap();
        let error = state.live_chunk_error(key(0)).copied();
        state.replace_chunk_wants(BTreeSet::new()).unwrap();
        observations.push((
            n,
            completed,
            facts,
            error,
            state.live_chunk_facts(key(0)),
            driver.pending_loads(),
            driver.pending_generations(),
            driver.held_completions(),
        ));
    }
    let store_closed = store.finish();
    let pool_closed = pool.finish();
    store_closed.unwrap();
    pool_closed.unwrap();
    for (n, completed, facts, error, forgotten, loads, generations, held) in observations {
        assert_eq!((completed.offered, completed.first_error), (1, None));
        assert_eq!(facts.generation, n);
        assert_eq!(
            facts.phase,
            if n % 2 == 0 {
                LiveChunkPhase::Failed
            } else {
                LiveChunkPhase::NeedsGeneration
            }
        );
        assert_eq!(
            error,
            if n % 2 == 0 {
                Some(ServerError::Timeout {
                    operation: Operation::Load,
                })
            } else {
                None
            }
        );
        assert!(forgotten.is_none());
        assert_eq!((loads, generations, held), (0, 0, 0));
    }
    assert_eq!(driver.last_error(), None);
}

use std::sync::{Arc, Condvar, Mutex, mpsc};
struct Release(Arc<(Mutex<bool>, Condvar)>);
impl Release {
    fn open(&self) {
        let (lock, cv) = &*self.0;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        self.open();
    }
}
// This gates entry to the real DiskStore load, not a native read syscall.
struct HeldDisk {
    disk: DiskStore,
    entered: mpsc::SyncSender<()>,
    release: Arc<(Mutex<bool>, Condvar)>,
    threads: Arc<Mutex<Vec<thread::ThreadId>>>,
}
impl DiskBackend for HeldDisk {
    fn load(&mut self, k: SaveKey) -> Result<LoadedValue, ServerError> {
        self.threads.lock().unwrap().push(thread::current().id());
        self.entered.send(()).unwrap();
        let (lock, cv) = &*self.release;
        drop(cv.wait_while(lock.lock().unwrap(), |v| !*v).unwrap());
        self.disk.load(k)
    }
    fn write(&mut self, t: SaveTicket, r: SaveRequest) -> SaveCompletion {
        self.disk.write(t, r)
    }
    fn sync(&mut self) -> Result<(), ServerError> {
        self.disk.sync()
    }
    fn close(&mut self) -> Result<(), ServerError> {
        self.disk.close()
    }
}
#[test]
fn held_actual_disk_entry_repeated_driver_polls_ticks_and_forget_do_not_block() {
    for saved in [true, false] {
        let root = Root::new();
        let expected = slotted_chunk();
        if saved {
            let mut initial = StoreGuard::new(store(&root));
            save(&mut initial.owner, key(0), expected.clone());
            initial.finish().unwrap();
        }
        let (tx, entered) = mpsc::sync_channel(1);
        let release = Release(Arc::new((Mutex::new(false), Condvar::new())));
        let threads = Arc::new(Mutex::new(vec![]));
        let disk = HeldDisk {
            disk: DiskStore::open(&root.0, options()).unwrap(),
            entered: tx,
            release: release.0.clone(),
            threads: threads.clone(),
        };
        let mut store = StoreGuard::new(scheduler(disk));
        store.release = Some(Release(release.0.clone()));
        let (start, started) = mpsc::sync_channel(1);
        let (tx, receive) = mpsc::sync_channel(1);
        let caller = thread::spawn(move || {
            let caller = thread::current().id();
            let mut state = live([key(0)]);
            let mut driver = ChunkDriver::new();
            let mut pool = PoolGuard::new();
            driver
                .start_load(&mut state, &mut store.owner, key(0), deadline())
                .unwrap();
            store.owner.drive_workers();
            let _ = started.recv();
            let mut observations = Vec::new();
            for _ in 0..32 {
                let report = driver.poll(&mut state, &mut store.owner, &mut pool.owner);
                let tick = state.advance_tick(TickBudget::full());
                let absent = state
                    .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
                    .is_none();
                observations.push((report, tick, absent));
            }
            let forgotten = state.replace_chunk_wants(BTreeSet::new());
            tx.send((store, pool, state, driver, observations, forgotten, caller))
                .unwrap();
        });
        let entry = entered.recv_timeout(Duration::from_secs(5));
        let _ = start.send(());
        let before = receive.recv_timeout(Duration::from_secs(2));
        let nonblocking = before.is_ok();
        release.open();
        let result = before.or_else(|_| receive.recv_timeout(Duration::from_secs(5)));
        let joined = caller.join();
        let (mut store, mut pool, mut state, mut driver, observations, forgotten, caller) =
            result.unwrap();
        let completed =
            drive_until_empty(&mut driver, &mut state, &mut store.owner, &mut pool.owner);
        let tick = state.advance_tick(TickBudget::full());
        let store_closed = store.finish();
        let pool_closed = pool.finish();
        store_closed.unwrap();
        pool_closed.unwrap();
        joined.unwrap();
        assert!(entry.is_ok());
        assert!(
            nonblocking,
            "driver and ticks must return before gate release"
        );
        forgotten.unwrap();
        tick.unwrap();
        for (report, tick, absent) in observations {
            tick.unwrap();
            assert!(absent);
            assert_eq!(
                (
                    report.polled,
                    report.pending,
                    report.offered,
                    report.retained
                ),
                (1, 1, 0, 1)
            );
        }
        assert_eq!(completed.offered, 1);
        let owner_threads = threads.lock().unwrap();
        assert_eq!(owner_threads.len(), 1);
        assert_ne!(owner_threads[0], caller);
        if saved {
            let f = state.live_chunk_facts(key(0)).unwrap();
            assert_eq!(
                (
                    f.phase,
                    f.wanted,
                    f.generation,
                    f.revision,
                    f.persisted_revision
                ),
                (LiveChunkPhase::Unloading, false, 1, 9, 9)
            );
            let old = state
                .capture_chunk_snapshot(key(0), SaveUrgency::Unload)
                .unwrap();
            state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
            let ready = state.live_chunk_facts(key(0)).unwrap();
            assert_eq!(
                (ready.phase, ready.generation),
                (LiveChunkPhase::Ready, f.generation)
            );
            let SaveValue::ChunkView(old_view) = old.value else {
                panic!("retained capture")
            };
            assert_eq!(materialized(&state, key(0)), old_view.materialize().chunk);
            assert_eq!(materialized(&state, key(0)), expected);
        } else {
            assert!(state.live_chunk_facts(key(0)).is_none());
            assert_eq!(driver.pending_loads(), 0);
        }
    }
}
#[test]
fn actual_stopped_driver_offers_existing_completion_while_authority_closes() {
    let root = Root::new();
    let mut store = StoreGuard::new(store(&root));
    save(&mut store.owner, key(0), slotted_chunk());
    let mut pool = PoolGuard::new();
    let mut state = live([key(0), key(1)]);
    let mut driver = ChunkDriver::new();
    driver
        .start_load(&mut state, &mut store.owner, key(0), deadline())
        .unwrap();
    driver.stop_new();
    driver.stop_new();
    state.begin_close();
    let no_load = driver.start_load(&mut state, &mut store.owner, key(1), deadline());
    let no_native = driver.start_generation(&mut state, &mut pool.owner, key(1));
    let completed = drive_until_empty(&mut driver, &mut state, &mut store.owner, &mut pool.owner);
    let store_closed = store.finish();
    let pool_closed = pool.finish();
    store_closed.unwrap();
    pool_closed.unwrap();
    let error = ServerError::InvalidState {
        phase: ServerPhase::Closing,
    };
    assert_eq!(no_load, Err(error));
    assert_eq!(no_native, Err(error));
    assert_eq!(
        (completed.offered, completed.retained, completed.first_error),
        (1, 0, None)
    );
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().phase,
        LiveChunkPhase::Loading,
        "Closing retains the final staged batch for its runtime owner"
    );
    assert!(state.live_chunk_facts(key(1)).is_none());
}
#[test]
fn actual_forgotten_native_success_installs_unloading_without_cancel() {
    let root = Root::new();
    let mut store = StoreGuard::new(store(&root));
    let mut pool = PoolGuard::new();
    let mut state = live([key(0)]);
    let mut driver = ChunkDriver::new();
    driver
        .start_load(&mut state, &mut store.owner, key(0), deadline())
        .unwrap();
    drive_until_empty(&mut driver, &mut state, &mut store.owner, &mut pool.owner);
    state.advance_tick(TickBudget::full()).unwrap();
    driver
        .start_generation(&mut state, &mut pool.owner, key(0))
        .unwrap();
    let generation = state.live_chunk_facts(key(0)).unwrap().generation;
    state.replace_chunk_wants(BTreeSet::new()).unwrap();
    let completed = drive_until_empty(&mut driver, &mut state, &mut store.owner, &mut pool.owner);
    let tick = state.advance_tick(TickBudget::full());
    let store_closed = store.finish();
    let pool_closed = pool.finish();
    store_closed.unwrap();
    pool_closed.unwrap();
    tick.unwrap();
    assert_eq!(completed.offered, 1);
    let f = state.live_chunk_facts(key(0)).unwrap();
    assert_eq!(
        (
            f.phase,
            f.generation,
            f.revision,
            f.persisted_revision,
            f.wanted
        ),
        (LiveChunkPhase::Unloading, generation, 1, 0, false)
    );
    let old = state
        .capture_chunk_snapshot(key(0), SaveUrgency::Unload)
        .unwrap();
    state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    let SaveValue::ChunkView(old_view) = old.value else {
        panic!("retained capture")
    };
    assert_eq!(materialized(&state, key(0)), old_view.materialize().chunk);
}

#[test]
fn separate_source_aliases_with_equal_numeric_ids_offer_independently() {
    let mut state = live([key(0), key(1)]);
    let mut driver = ChunkDriver::new();
    let mut loads = Loads {
        missing: true,
        alias: Some(request(1)),
        ..Default::default()
    };
    let mut generations = Generations::default();
    driver
        .start_load(&mut state, &mut loads, key(0), deadline())
        .unwrap();
    driver.poll(&mut state, &mut loads, &mut generations);
    state.advance_tick(TickBudget::full()).unwrap();
    let generated = driver
        .start_generation(&mut state, &mut generations, key(0))
        .unwrap();
    let loaded = driver
        .start_load(&mut state, &mut loads, key(1), deadline())
        .unwrap();
    assert_eq!(generated, loaded);
    generations
        .results
        .insert(generated, GenerationPoll::Failed(ServerError::Cancelled));
    assert_eq!(
        driver.poll(&mut state, &mut loads, &mut generations),
        ChunkPollReport {
            polled: 2,
            offered: 2,
            pending: 0,
            retained: 0,
            first_error: None
        }
    );
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().phase,
        LiveChunkPhase::Failed
    );
    assert_eq!(
        state.live_chunk_facts(key(1)).unwrap().phase,
        LiveChunkPhase::NeedsGeneration
    );
    assert!(loads.cancels.is_empty() && generations.cancels.is_empty());
}

#[test]
fn actual_driver_recovered_load_retains_original_rewrite_and_body_when_rewanted() {
    use mornlea_storage::{
        BANK_A_START_SECTOR, BANK_B_START_SECTOR, BANK_SIZE, RegionKey, SECTOR_SIZE,
        decode_region_bank,
    };
    let root = Root::new();
    let mut s = StoreGuard::new(store(&root));
    let expected = slotted_chunk();
    save_at(&mut s.owner, key(0), expected.clone(), 7);
    let mut newer = chunk();
    newer.sections[0].single = 3;
    save_at(&mut s.owner, key(0), newer, 8);
    s.finish().unwrap();
    let path = root.0.join("dimensions/0/regions/r.0.0.region");
    let mut bytes = fs::read(&path).unwrap();
    let rk = RegionKey {
        dimension: 0,
        x: 0,
        z: 0,
    };
    let banks: Vec<_> = [BANK_A_START_SECTOR, BANK_B_START_SECTOR]
        .iter()
        .map(|sector| {
            let at = *sector as usize * SECTOR_SIZE as usize;
            decode_region_bank(rk, &bytes[at..at + BANK_SIZE], bytes.len() as i64).unwrap()
        })
        .collect();
    let active = banks.iter().max_by_key(|b| b.generation).unwrap();
    let at = active.entries[0].offset_sector as usize * SECTOR_SIZE as usize;
    bytes[at] ^= 0xff;
    fs::write(&path, &bytes).unwrap();
    let mut s = StoreGuard::new(store(&root));
    let mut a = authority();
    a.enable_live_chunks().unwrap();
    a.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    let mut pool = PoolGuard::new();
    let mut driver = ChunkDriver::new();
    driver
        .start_load(&mut a, &mut s.owner, key(0), deadline())
        .unwrap();
    let generation = a.live_chunk_facts(key(0)).unwrap().generation;
    a.replace_chunk_wants(BTreeSet::new()).unwrap();
    let report = drive_until_empty(&mut driver, &mut a, &mut s.owner, &mut pool.owner);
    let store_closed = s.finish();
    let pool_closed = pool.finish();
    store_closed.unwrap();
    pool_closed.unwrap();
    assert_eq!((report.offered, report.first_error), (1, None));
    a.advance_tick(TickBudget::full()).unwrap();
    let f = a.live_chunk_facts(key(0)).unwrap();
    assert_eq!(
        (
            f.phase,
            f.generation,
            f.revision,
            f.persisted_revision,
            f.needs_rewrite,
            f.recovered
        ),
        (LiveChunkPhase::Unloading, generation, 9, 7, true, true)
    );
    let SaveValue::ChunkView(v) = a
        .capture_chunk_snapshot(key(0), SaveUrgency::Unload)
        .unwrap()
        .value
    else {
        panic!("retained recovered body")
    };
    assert_eq!(v.materialize().chunk, expected);
    a.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    let ready = a.live_chunk_facts(key(0)).unwrap();
    assert_eq!(
        (
            ready.phase,
            ready.generation,
            ready.revision,
            ready.persisted_revision,
            ready.needs_rewrite,
            ready.recovered
        ),
        (LiveChunkPhase::Ready, f.generation, 9, 7, true, true)
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn sixteen_bound_polls_follow_source_then_key_order_without_history() {
    let trace = Arc::new(Mutex::new(vec![]));
    let mut state = live((0..16).map(key));
    let mut driver = ChunkDriver::new();
    let mut loads = Loads {
        missing: true,
        trace: Some(trace.clone()),
        ..Default::default()
    };
    let mut generations = Generations {
        trace: Some(trace.clone()),
        ..Default::default()
    };
    for x in 0..8 {
        driver
            .start_load(&mut state, &mut loads, key(x), deadline())
            .unwrap();
    }
    driver.poll(&mut state, &mut loads, &mut generations);
    state.advance_tick(TickBudget::full()).unwrap();
    loads.missing = false;
    for x in (0..8).rev() {
        driver
            .start_generation(&mut state, &mut generations, key(x))
            .unwrap();
    }
    for x in (8..16).rev() {
        driver
            .start_load(&mut state, &mut loads, key(x), deadline())
            .unwrap();
    }
    trace.lock().unwrap().clear();
    assert_eq!(
        driver.poll(&mut state, &mut loads, &mut generations),
        ChunkPollReport {
            polled: 16,
            offered: 0,
            pending: 16,
            retained: 16,
            first_error: None
        }
    );
    let expected: Vec<_> = (9..=16)
        .rev()
        .map(|id| (false, request(id)))
        .chain((1..=8).rev().map(|id| (true, request(id))))
        .collect();
    assert_eq!(*trace.lock().unwrap(), expected);
    assert_eq!(
        (
            driver.pending_loads(),
            driver.pending_generations(),
            driver.held_completions()
        ),
        (8, 8, 0)
    );
}
#[test]
fn duplicate_generation_alias_quarantines_only_ambiguous_start() {
    let mut state = live([key(0), key(1), key(2)]);
    let mut driver = ChunkDriver::new();
    let mut loads = Loads {
        missing: true,
        ..Default::default()
    };
    let mut generations = Generations {
        alias: Some(request(1)),
        ..Default::default()
    };
    for x in 0..2 {
        driver
            .start_load(&mut state, &mut loads, key(x), deadline())
            .unwrap();
    }
    driver.poll(&mut state, &mut loads, &mut generations);
    state.advance_tick(TickBudget::full()).unwrap();
    driver
        .start_generation(&mut state, &mut generations, key(0))
        .unwrap();
    assert_eq!(
        driver.start_generation(&mut state, &mut generations, key(0)),
        Err(invalid("chunk_driver_request"))
    );
    let original = state.live_chunk_facts(key(0)).unwrap();
    assert_eq!(
        driver.start_generation(&mut state, &mut generations, key(1)),
        Err(invalid("chunk_request_identity"))
    );
    generations
        .results
        .insert(request(1), GenerationPoll::Failed(ServerError::Cancelled));
    let report = driver.poll(&mut state, &mut loads, &mut generations);
    assert_eq!((report.polled, report.offered, report.retained), (1, 1, 1));
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().generation,
        original.generation
    );
    assert_eq!(
        state.live_chunk_facts(key(1)).unwrap().phase,
        LiveChunkPhase::Generating
    );
    driver.poll(&mut state, &mut loads, &mut generations);
    assert_eq!(generations.polls, vec![request(1)]);
    assert!(generations.cancels.is_empty());
    assert_eq!(driver.pending_generations(), 1);
}
#[test]
fn prematurely_consumed_alias_returns_stale_completion_without_repolling() {
    use mornlea_server::core::acquisition::AcquiredChunkEvent;
    let mut state = live([key(0), key(1)]);
    let mut driver = ChunkDriver::new();
    let mut loads = Loads {
        missing: true,
        ..Default::default()
    };
    let mut generations = Generations::default();
    let id = driver
        .start_load(&mut state, &mut loads, key(0), deadline())
        .unwrap();
    let original = state.live_chunk_facts(key(0)).unwrap();
    // Only this negative ownership fixture prematurely consumes a driver's
    // alias. Real-provider success recipes never deliver events manually.
    state
        .offer_acquired(AcquiredChunkEvent::Load {
            key: key(0),
            generation: original.generation,
            request: id,
            result: Ok(None),
        })
        .unwrap();
    state.advance_tick(TickBudget::full()).unwrap();
    let report = driver.poll(&mut state, &mut loads, &mut generations);
    let error = invalid("chunk_completion_identity");
    assert_eq!(
        (
            report.polled,
            report.offered,
            report.retained,
            report.first_error
        ),
        (1, 0, 1, Some(error))
    );
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().generation,
        original.generation
    );
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().phase,
        LiveChunkPhase::NeedsGeneration
    );
    assert_eq!(
        driver.start_load(&mut state, &mut loads, key(1), deadline()),
        Err(error)
    );
    assert_eq!(
        driver.poll(&mut state, &mut loads, &mut generations).polled,
        0
    );
    assert_eq!(loads.polls, vec![id]);
    assert!(loads.cancels.is_empty());
}

#[test]
fn current_key_duplicate_across_sources_refuses_before_provider_start() {
    use mornlea_server::core::acquisition::AcquiredChunkEvent;
    let mut state = live([key(0)]);
    let mut driver = ChunkDriver::new();
    let mut loads = Loads::default();
    let mut generations = Generations::default();
    let id = driver
        .start_load(&mut state, &mut loads, key(0), deadline())
        .unwrap();
    let generation = state.live_chunk_facts(key(0)).unwrap().generation;
    // Premature settlement is an ownership fault fixture. Its stale driver
    // record must still fence the key before another source can start it.
    state
        .offer_acquired(AcquiredChunkEvent::Load {
            key: key(0),
            generation,
            request: id,
            result: Ok(None),
        })
        .unwrap();
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        driver.start_generation(&mut state, &mut generations, key(0)),
        Err(invalid("chunk_driver_request"))
    );
    assert!(generations.starts.is_empty());
    loads.missing = true;
    driver.poll(&mut state, &mut loads, &mut generations);
    assert_eq!(driver.pending_loads(), 1);
    assert_eq!(driver.pending_generations(), 0);
}
