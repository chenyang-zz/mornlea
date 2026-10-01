//! Background ownership tests use causal gates around real owner operations.

use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use mornlea_server::contracts::{
    Deadline, DiskBackend, LoadedValue, OwnedSnapshot, SaveBudget, SaveCompletion, SaveKey,
    SaveRequest, SaveTicket, SaveUrgency, SaveValue, ServerError, ServerLimits, StoreHandle,
    StoreLimits,
};
use mornlea_server::state::AuthorityState;
use mornlea_server::store::mailbox::StoreMailbox;
use mornlea_storage::HostileMobsSave;

const BOUND: Duration = Duration::from_secs(5);

fn limits() -> StoreLimits {
    StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap()
}
fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        13,
    )
    .unwrap()
}
fn snapshot(revision: u64) -> OwnedSnapshot {
    OwnedSnapshot::try_new(
        SaveKey::Hostiles,
        revision,
        1,
        SaveUrgency::Autosave,
        SaveValue::Hostiles(HostileMobsSave {
            revision,
            records: vec![],
        }),
    )
    .unwrap()
}
fn deadline() -> Deadline {
    Deadline::after(Instant::now(), BOUND).unwrap()
}

/// Release on unwind as well as success, so a failed observation cannot strand a worker.
#[derive(Clone)]
struct Gate {
    entered: mpsc::SyncSender<()>,
    opened: Arc<(Mutex<bool>, Condvar)>,
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
fn gate() -> (Gate, mpsc::Receiver<()>, Release) {
    let (entered, receive) = mpsc::sync_channel(1);
    let opened = Arc::new((Mutex::new(false), Condvar::new()));
    (
        Gate {
            entered,
            opened: opened.clone(),
        },
        receive,
        Release(opened),
    )
}
impl Gate {
    fn hold(&self) {
        self.entered.send(()).unwrap();
        let (lock, wake) = &*self.opened;
        let _guard = wake
            .wait_while(lock.lock().unwrap(), |open| !*open)
            .unwrap();
    }
}
/// The parent keeps the I/O gate and releases it before every assertion. The
/// driver waits for the parent's causal start before observing held operations.
fn observe_held<T: Send + 'static>(
    entered: mpsc::Receiver<()>,
    release: &Release,
    work: impl FnOnce(mpsc::Receiver<()>) -> T + Send + 'static,
) -> T {
    let (start, started) = mpsc::sync_channel(1);
    let (returned, receive) = mpsc::sync_channel(1);
    let driver = thread::spawn(move || returned.send(work(started)).unwrap());
    let gate_entered = entered.recv_timeout(BOUND);
    let _ = start.send(());
    let observed = receive.recv_timeout(Duration::from_secs(1));
    let returned_before_release = observed.is_ok();
    release.open();
    let result = observed.or_else(|_| receive.recv_timeout(BOUND));
    let joined = driver.join();
    assert!(
        gate_entered.is_ok(),
        "backend operation must enter its gate"
    );
    joined.unwrap();
    assert!(
        returned_before_release,
        "held operation must return before gate release"
    );
    result.unwrap()
}

struct GatedBackend {
    write: Gate,
}
impl DiskBackend for GatedBackend {
    fn write(&mut self, ticket: SaveTicket, request: SaveRequest) -> SaveCompletion {
        self.write.hold();
        let submitted = request
            .snapshots
            .iter()
            .map(|s| (s.key.clone(), s.revision))
            .collect::<Vec<_>>();
        SaveCompletion {
            ticket,
            snapshots: request.snapshots,
            committed: submitted.clone(),
            submitted,
            error: None,
        }
    }
    fn load(&mut self, _: SaveKey) -> Result<LoadedValue, ServerError> {
        unreachable!()
    }
    fn sync(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
}

#[test]
fn drive_returns_while_backend_write_is_held() {
    let (write, entered, release) = gate();
    let (returned, receive) = mpsc::sync_channel(1);
    let driver = thread::spawn(move || {
        let mut store = StoreMailbox::try_new_background(limits(), GatedBackend { write }).unwrap();
        let ticket = store
            .submit(SaveRequest {
                snapshots: vec![snapshot(3)],
            })
            .unwrap();
        let mut state = authority();
        store
            .poll_tick(1, SaveBudget::default(), &mut state)
            .unwrap();
        store.drive_workers();
        returned.send((store, ticket)).unwrap();
    });
    entered.recv_timeout(BOUND).unwrap();
    let observed = receive.recv_timeout(Duration::from_millis(500));
    // Cleanup precedes the assertion even in the synchronous behavioral RED.
    release.open();
    let nonblocking = observed.is_ok();
    let (mut store, ticket) = observed.unwrap_or_else(|_| receive.recv_timeout(BOUND).unwrap());
    driver.join().unwrap();
    let until = Instant::now() + BOUND;
    loop {
        store.drive_workers();
        if let mornlea_server::contracts::SavePoll::Completed(completion) = store.poll(ticket) {
            assert_eq!(completion.snapshots, vec![snapshot(3)]);
            assert_eq!(completion.committed, vec![(SaveKey::Hostiles, 3)]);
            break;
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    store.close(deadline()).unwrap();
    assert!(
        nonblocking,
        "drive must return while the backend write gate is held"
    );
}

struct RealClock;
impl mornlea_server::contracts::Clock for RealClock {
    fn monotonic(&self) -> Instant {
        Instant::now()
    }
    fn unix_ms(&self) -> i64 {
        0
    }
}

fn wait_completion<B: DiskBackend>(
    store: &mut StoreMailbox<B>,
    ticket: SaveTicket,
) -> SaveCompletion {
    let until = Instant::now() + BOUND;
    loop {
        store.drive_workers();
        if let mornlea_server::contracts::SavePoll::Completed(completion) = store.poll(ticket) {
            return completion;
        }
        assert!(Instant::now() < until, "save completion did not arrive");
        thread::yield_now();
    }
}

/// Lifecycle counters and gates expose owner calls without sharing the backend itself.
#[derive(Default)]
struct Calls {
    writes: usize,
    syncs: usize,
    closes: usize,
    drops: usize,
    threads: Vec<thread::ThreadId>,
}
struct ScriptedBackend {
    calls: Arc<Mutex<Calls>>,
    write_gate: Option<Gate>,
    sync_gate: Option<Gate>,
    close_gate: Option<Gate>,
    drop_gate: Option<Gate>,
    wrong_ticket: bool,
    panic_write: bool,
    partial: bool,
    fail_close: bool,
    panic_drop: bool,
}
fn scripted() -> (ScriptedBackend, Arc<Mutex<Calls>>) {
    let calls = Arc::new(Mutex::new(Calls::default()));
    (
        ScriptedBackend {
            calls: calls.clone(),
            write_gate: None,
            sync_gate: None,
            close_gate: None,
            drop_gate: None,
            wrong_ticket: false,
            panic_write: false,
            partial: false,
            fail_close: false,
            panic_drop: false,
        },
        calls,
    )
}
fn fault() -> ServerError {
    ServerError::Io {
        operation: mornlea_server::contracts::Operation::Sync,
        kind: std::io::ErrorKind::Other,
    }
}
impl Drop for ScriptedBackend {
    fn drop(&mut self) {
        self.calls.lock().unwrap().drops += 1;
        if let Some(gate) = self.drop_gate.take() {
            gate.hold();
        }
        assert!(!self.panic_drop, "explicit destructor panic");
    }
}
impl DiskBackend for ScriptedBackend {
    fn write(&mut self, ticket: SaveTicket, request: SaveRequest) -> SaveCompletion {
        {
            let mut calls = self.calls.lock().unwrap();
            calls.writes += 1;
            calls.threads.push(thread::current().id());
        }
        if let Some(gate) = self.write_gate.take() {
            gate.hold();
        }
        assert!(!self.panic_write, "explicit backend panic");
        let submitted = request
            .snapshots
            .iter()
            .map(|s| (s.key.clone(), s.revision))
            .collect::<Vec<_>>();
        let committed = if self.partial {
            submitted.iter().take(1).cloned().collect()
        } else {
            submitted.clone()
        };
        SaveCompletion {
            ticket: if self.wrong_ticket {
                SaveTicket::try_from_raw(900).unwrap()
            } else {
                ticket
            },
            snapshots: vec![],
            submitted,
            committed,
            error: self.partial.then(fault),
        }
    }
    fn load(&mut self, _: SaveKey) -> Result<LoadedValue, ServerError> {
        unreachable!()
    }
    fn sync(&mut self) -> Result<(), ServerError> {
        {
            let mut calls = self.calls.lock().unwrap();
            calls.syncs += 1;
            calls.threads.push(thread::current().id());
        }
        if let Some(gate) = self.sync_gate.take() {
            gate.hold();
        }
        Ok(())
    }
    fn close(&mut self) -> Result<(), ServerError> {
        {
            let mut calls = self.calls.lock().unwrap();
            calls.closes += 1;
            calls.threads.push(thread::current().id());
        }
        if let Some(gate) = self.close_gate.take() {
            gate.hold();
        }
        if std::mem::take(&mut self.fail_close) {
            Err(fault())
        } else {
            Ok(())
        }
    }
}

#[test]
fn slots_cancellation_partial_results_and_occupancy_are_retained() {
    let (mut backend, calls) = scripted();
    let (write, entered, release) = gate();
    backend.write_gate = Some(write);
    backend.partial = true;
    let mut store = StoreMailbox::try_new_background(limits(), backend).unwrap();
    let first = vec![snapshot(1), snapshot(2)];
    let a = store
        .submit(SaveRequest {
            snapshots: first.clone(),
        })
        .unwrap();
    let b = store
        .submit(SaveRequest {
            snapshots: vec![snapshot(3)],
        })
        .unwrap();
    let cancelled = OwnedSnapshot::try_new(
        SaveKey::Passives,
        4,
        1,
        SaveUrgency::Autosave,
        SaveValue::Passives(PassiveMobsSave {
            revision: 4,
            records: vec![],
        }),
    )
    .unwrap();
    let c = store
        .submit(SaveRequest {
            snapshots: vec![cancelled.clone()],
        })
        .unwrap();
    let held_calls = calls.clone();
    let (mut store, workers, queued, dirty, returned, remaining, cancelled_poll, jobs, writes) =
        observe_held(entered, &release, move |started| {
            let mut state = authority();
            store
                .poll_tick(1, SaveBudget::default(), &mut state)
                .unwrap();
            started.recv().unwrap();
            let workers = store.worker_jobs();
            let queued = store.queued_jobs();
            state.remember_dirty(snapshot(20));
            let dirty = store
                .poll_tick(2, SaveBudget::default(), &mut state)
                .unwrap()
                .stats
                .dirty;
            let returned = store.cancel_pending().unwrap();
            let remaining = store.queued_jobs();
            let cancelled_poll = store.poll(c);
            let jobs = store.occupancy().jobs;
            let writes = held_calls.lock().unwrap().writes;
            (
                store,
                workers,
                queued,
                dirty,
                returned,
                remaining,
                cancelled_poll,
                jobs,
                writes,
            )
        });
    assert_eq!(workers, 2);
    assert_eq!(queued, 1);
    assert_eq!(dirty, 1);
    assert_eq!(returned, vec![cancelled]);
    assert_eq!(remaining, 0);
    assert!(matches!(
        cancelled_poll,
        mornlea_server::contracts::SavePoll::Pending
    ));
    assert_eq!(jobs, 2);
    assert_eq!(writes, 1, "one serialized backend owner");
    let until = Instant::now() + BOUND;
    while store.held_completions() < 2 {
        store.drive_workers();
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    assert_eq!(
        store.occupancy().jobs,
        2,
        "unpolled replies retain admission"
    );
    let result = wait_completion(&mut store, a);
    assert_eq!(result.snapshots, first);
    assert_eq!(result.committed, vec![(SaveKey::Hostiles, 1)]);
    assert_eq!(result.error, Some(fault()));
    assert_eq!(store.occupancy().jobs, 1);
    assert_eq!(wait_completion(&mut store, b).snapshots, vec![snapshot(3)]);
    assert_eq!(store.occupancy(), Default::default());
    store.sync(deadline()).unwrap();
    store.close(deadline()).unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls.drops, 1,
        "successful close joins through backend drop"
    );
    assert!(
        calls
            .threads
            .iter()
            .all(|id| *id == calls.threads[0] && *id != thread::current().id())
    );
}

#[test]
fn wrong_ticket_and_panic_preserve_original_snapshots_and_owner() {
    for panic_write in [false, true] {
        let (mut backend, calls) = scripted();
        backend.panic_write = panic_write;
        backend.wrong_ticket = !panic_write;
        let mut store = StoreMailbox::try_new_background(limits(), backend).unwrap();
        let original = vec![snapshot(1)];
        let ticket = store
            .submit(SaveRequest {
                snapshots: original.clone(),
            })
            .unwrap();
        let completion = wait_completion(&mut store, ticket);
        assert_eq!(completion.snapshots, original);
        assert!(completion.committed.is_empty());
        assert!(matches!(
            completion.error,
            Some(ServerError::Internal { .. })
        ));
        store.sync(deadline()).unwrap();
        store.close(deadline()).unwrap();
        assert_eq!(calls.lock().unwrap().drops, 1);
    }
}

#[test]
fn started_sync_timeout_retry_consumes_one_outcome() {
    let (mut backend, calls) = scripted();
    let (sync, entered, release) = gate();
    backend.sync_gate = Some(sync);
    let store = StoreMailbox::try_new_background(limits(), backend).unwrap();
    let (mut store, result, different_kind) = observe_held(entered, &release, move |started| {
        let mut store = store;
        let result =
            store.sync(Deadline::after(Instant::now(), Duration::from_millis(500)).unwrap());
        started.recv().unwrap();
        let different_kind = store.close(deadline());
        (store, result, different_kind)
    });
    assert_eq!(
        result,
        Err(ServerError::Timeout {
            operation: mornlea_server::contracts::Operation::Sync
        })
    );
    assert!(matches!(
        different_kind,
        Err(ServerError::InvalidState { .. })
    ));
    store.sync(deadline()).unwrap();
    assert_eq!(calls.lock().unwrap().syncs, 1);
    store.close(deadline()).unwrap();
}

#[test]
fn queued_expired_sync_performs_no_backend_call() {
    let (mut backend, calls) = scripted();
    let (write, entered, release) = gate();
    backend.write_gate = Some(write);
    let mut store = StoreMailbox::try_new_background(limits(), backend).unwrap();
    let ticket = store
        .submit(SaveRequest {
            snapshots: vec![snapshot(1)],
        })
        .unwrap();
    let (mut store, result) = observe_held(entered, &release, move |started| {
        store.drive_workers();
        started.recv().unwrap();
        let result =
            store.sync(Deadline::after(Instant::now(), Duration::from_millis(30)).unwrap());
        (store, result)
    });
    assert_eq!(
        result,
        Err(ServerError::Timeout {
            operation: mornlea_server::contracts::Operation::Sync
        })
    );
    assert!(wait_completion(&mut store, ticket).error.is_none());
    assert_eq!(
        store.sync(deadline()),
        Err(ServerError::Timeout {
            operation: mornlea_server::contracts::Operation::Sync
        })
    );
    assert_eq!(calls.lock().unwrap().syncs, 0);
    store.sync(deadline()).unwrap();
    assert_eq!(calls.lock().unwrap().syncs, 1);
    store.close(deadline()).unwrap();
}

#[test]
fn failed_close_retries_on_same_owner_and_success_joins() {
    let (mut backend, calls) = scripted();
    backend.fail_close = true;
    let mut store = StoreMailbox::try_new_background(limits(), backend).unwrap();
    assert_eq!(store.close(deadline()), Err(fault()));
    assert_eq!(calls.lock().unwrap().drops, 0);
    store.close(deadline()).unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls.closes, 2);
    assert_eq!(calls.drops, 1);
    assert_eq!(calls.threads[0], calls.threads[1]);
}

#[test]
fn started_close_timeout_retains_success_for_join_without_replay() {
    let (mut backend, calls) = scripted();
    let (close, entered, release) = gate();
    backend.close_gate = Some(close);
    let store = StoreMailbox::try_new_background(limits(), backend).unwrap();
    let (mut store, result, different_kind) = observe_held(entered, &release, move |started| {
        let mut store = store;
        let result =
            store.close(Deadline::after(Instant::now(), Duration::from_millis(500)).unwrap());
        started.recv().unwrap();
        let different_kind = store.sync(deadline());
        (store, result, different_kind)
    });
    assert_eq!(
        result,
        Err(ServerError::Timeout {
            operation: mornlea_server::contracts::Operation::Close
        })
    );
    assert!(matches!(
        different_kind,
        Err(ServerError::InvalidState { .. })
    ));
    store.close(deadline()).unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls.closes, 1);
    assert_eq!(calls.drops, 1);
}

use mornlea_domain::{ChunkPos, Dimension, PlayerId};
use mornlea_server::contracts::{ChunkKey, IoFaultPoint, SaveValue as Value};
use mornlea_server::store::disk::{DiskOptions, DiskStore};
use mornlea_server::store::io::{DiskIo, IoPhase};
use mornlea_server::store::scheduler::{AutosaveScheduler, SchedulerConfig};
use mornlea_storage::{
    ChestSlot, Chunk, ChunkSave, CompanionSave, ContainerSnapshot, DropSlot, FurnaceSlot,
    Inventory, ItemStack, METADATA_CURRENT_VERSION, Metadata, MetadataChunkPos, PassiveMobsSave,
    PlayerLocation, PlayerSave, StorageKind,
};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

static ROOT_COUNTER: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mornlea-background-{}-{}",
            std::process::id(),
            ROOT_COUNTER.fetch_add(1, Ordering::Relaxed)
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
fn metadata(seed: i64) -> Metadata {
    Metadata {
        format_version: METADATA_CURRENT_VERSION,
        seed,
        spawn_dimension: 0,
        spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
        world_time_ticks: 0,
        day_phase_offset: 0,
        weather_kind: 0,
        weather_ticks_remaining: 0,
        depths_spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
        depths_seed_salt: 0,
        difficulty: 0,
    }
}
fn options() -> DiskOptions {
    DiskOptions {
        create: metadata(13),
        region_handle_cap: 1,
    }
}
fn player_id() -> [u8; 16] {
    [1, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 0]
}
fn player_snapshot() -> OwnedSnapshot {
    OwnedSnapshot::try_new(
        SaveKey::Player(PlayerId::try_from_bytes(player_id()).unwrap()),
        9,
        1,
        SaveUrgency::Autosave,
        Value::Player(PlayerSave {
            player_id: mornlea_storage::PlayerId::from_bytes(player_id()),
            revision: 9,
            display_name: "Ada".to_owned(),
            current: PlayerLocation {
                dimension: 0,
                position: [10.5, 65.0, -3.25],
            },
            yaw: 1.2,
            pitch: 0.3,
            safe: None,
            inventory: Inventory::default(),
            health: 15,
            hunger: 17,
            saturation_milli: 9000,
            exhaustion_milli: 250,
            respawn_present: false,
            respawn_position: [0.0; 3],
            respawn_dimension: 0,
            armor: [ItemStack::default(); 4],
        }),
    )
    .unwrap()
}
fn chunk_snapshot() -> OwnedSnapshot {
    OwnedSnapshot::try_new(
        SaveKey::Chunk(ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(3, -1),
        }),
        9,
        1,
        SaveUrgency::Autosave,
        Value::Chunk(ChunkSave {
            key: mornlea_storage::ChunkKey {
                dimension: 0,
                x: 3,
                z: -1,
            },
            revision: 9,
            chunk: Chunk {
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
                drops: vec![DropSlot::default(); 32],
                furnaces: vec![FurnaceSlot::default(); 32],
                chests: vec![ChestSlot::default(); 16],
            },
        }),
    )
    .unwrap()
}
struct HeldIo {
    point: IoFaultPoint,
    nth: usize,
    count: Arc<AtomicUsize>,
    gate: Gate,
}
impl DiskIo for HeldIo {
    fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> std::io::Result<()> {
        if point == self.point
            && phase == IoPhase::Before
            && self.count.fetch_add(1, Ordering::SeqCst) + 1 == self.nth
        {
            self.gate.hold();
        }
        Ok(())
    }
}
fn disk_with_gate(root: &Root, point: IoFaultPoint, nth: usize, gate: Gate) -> DiskStore {
    let mut initial = DiskStore::open(&root.0, options()).unwrap();
    initial.close().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    DiskStore::with_io(
        &root.0,
        options(),
        Box::new(move || {
            Box::new(HeldIo {
                point,
                nth,
                count: count.clone(),
                gate: gate.clone(),
            })
        }),
    )
    .unwrap()
}
fn check_reopen(root: &Root) {
    let mut reopened = DiskStore::open(&root.0, options()).unwrap();
    match reopened.load(player_snapshot().key).unwrap() {
        LoadedValue::Player(player) => {
            assert_eq!(player.current.position, [10.5, 65.0, -3.25]);
            assert_eq!(player.health, 15);
            assert_eq!(player.revision, 9);
        }
        other => panic!("unexpected loaded family {other:?}"),
    }
    match reopened.load(chunk_snapshot().key).unwrap() {
        LoadedValue::Chunk(chunk) => {
            assert_eq!(chunk.revision, 9);
            assert_eq!(chunk.persisted_revision, 9);
            assert!(chunk.chunk.sections.iter().all(|s| s.single == 2));
        }
        other => panic!("unexpected loaded family {other:?}"),
    }
    reopened.close().unwrap();
}

#[test]
fn real_payload_write_keeps_tick_free_and_lease_until_close_join() {
    let root = Root::new();
    let (write, entered, release) = gate();
    let disk = disk_with_gate(&root, IoFaultPoint::PayloadWrite, 1, write);
    let mut store = StoreMailbox::try_new_background(limits(), disk).unwrap();
    let snapshots = vec![chunk_snapshot(), player_snapshot()];
    let ticket = store
        .submit(SaveRequest {
            snapshots: snapshots.clone(),
        })
        .unwrap();
    let reserved = store.occupancy().encoded_bytes;
    let (mut store, dirty, pending, flush) = observe_held(entered, &release, move |started| {
        let mut state = authority();
        store
            .poll_tick(1, SaveBudget::default(), &mut state)
            .unwrap();
        store.drive_workers();
        started.recv().unwrap();
        state.remember_dirty(snapshot(30));
        let dirty = store
            .poll_tick(2, SaveBudget::default(), &mut state)
            .unwrap()
            .stats
            .dirty;
        let pending = store.poll(ticket);
        let flush = store.flush(
            Deadline::after(Instant::now(), Duration::from_millis(30)).unwrap(),
            &mut state,
            &RealClock,
        );
        (store, dirty, pending, flush)
    });
    assert_eq!(dirty, 1);
    assert!(matches!(
        pending,
        mornlea_server::contracts::SavePoll::Pending
    ));
    assert!(
        DiskStore::open(&root.0, options()).is_err(),
        "the worker holds the world lease until complete close"
    );
    assert_eq!(
        flush,
        Err(ServerError::Timeout {
            operation: mornlea_server::contracts::Operation::Flush
        })
    );
    assert_eq!(
        store.occupancy().jobs,
        1,
        "timeout retains original save ownership"
    );
    // Collect without polling to inspect the worker's one-time reservation shrink.
    let until = Instant::now() + BOUND;
    while store.held_completions() == 0 {
        store.drive_workers();
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    assert!(store.occupancy().encoded_bytes < reserved);
    let shrunk = store.occupancy().encoded_bytes;
    store.drive_workers();
    assert_eq!(store.occupancy().encoded_bytes, shrunk);
    let completion = wait_completion(&mut store, ticket);
    assert_eq!(completion.snapshots, snapshots);
    assert_eq!(
        completion.committed,
        vec![(chunk_snapshot().key, 9), (player_snapshot().key, 9)]
    );
    assert!(completion.error.is_none());
    assert_eq!(store.occupancy(), Default::default());
    store.sync(deadline()).unwrap();
    store.close(deadline()).unwrap();
    check_reopen(&root);
}

#[test]
fn scheduler_flush_waits_for_real_final_metadata_and_all_families() {
    let root = Root::new();
    let (write, entered, release) = gate();
    // Four standalone selected families precede the final metadata replacement.
    let disk = disk_with_gate(&root, IoFaultPoint::TempWrite, 5, write);
    let store = StoreMailbox::try_new_background(limits(), disk).unwrap();
    let mut scheduler = AutosaveScheduler::try_new(SchedulerConfig::default(), store).unwrap();
    let mut state = authority();
    state.remember_dirty(chunk_snapshot());
    state.remember_dirty(player_snapshot());
    state.remember_dirty(snapshot(9));
    state.remember_dirty(
        OwnedSnapshot::try_new(
            SaveKey::Passives,
            9,
            1,
            SaveUrgency::Autosave,
            Value::Passives(PassiveMobsSave {
                revision: 9,
                records: vec![],
            }),
        )
        .unwrap(),
    );
    state.remember_dirty(
        OwnedSnapshot::try_new(
            SaveKey::Companions,
            9,
            1,
            SaveUrgency::Autosave,
            Value::Companions(CompanionSave {
                revision: 9,
                agent_namespace_id: mornlea_storage::PlayerId::from_bytes(player_id()),
                records: vec![],
                lifecycles: vec![],
                queues: vec![],
            }),
        )
        .unwrap(),
    );
    let expected_metadata = state.metadata_snapshot();
    let (finished, receive) = mpsc::sync_channel(1);
    let driver = thread::spawn(move || {
        let result = scheduler.flush(deadline(), &mut state, &RealClock);
        finished.send((scheduler, state, result)).unwrap();
    });
    entered.recv_timeout(BOUND).unwrap();
    // The exact metadata write has begun; flush must still own its pending ticket.
    assert!(receive.try_recv().is_err());
    release.open();
    let (mut scheduler, state, result) = receive.recv_timeout(BOUND).unwrap();
    driver.join().unwrap();
    let report = result.unwrap();
    assert_eq!(
        report.durable, 2,
        "one selected job and the metadata barrier"
    );
    assert_eq!(report.outstanding, 0);
    assert_eq!(state.save_stats().in_flight, 0);
    scheduler.close(deadline()).unwrap();
    check_reopen(&root);
    let mut disk = DiskStore::open(&root.0, options()).unwrap();
    assert_eq!(
        disk.load(SaveKey::Metadata),
        match expected_metadata.value {
            Value::Metadata(value) => Ok(LoadedValue::Metadata(value)),
            _ => unreachable!(),
        }
    );
    assert!(matches!(
        disk.load(SaveKey::Hostiles),
        Ok(LoadedValue::Hostiles(_))
    ));
    assert!(matches!(
        disk.load(SaveKey::Passives),
        Ok(LoadedValue::Passives(_))
    ));
    assert!(matches!(
        disk.load(SaveKey::Companions),
        Ok(LoadedValue::Companions(_))
    ));
    disk.close().unwrap();
}

#[test]
fn scheduler_metadata_timeout_keeps_ticket_for_retry_without_duplicate_write() {
    let (mut backend, calls) = scripted();
    let (write, entered, release) = gate();
    backend.write_gate = Some(write);
    let store = StoreMailbox::try_new_background(limits(), backend).unwrap();
    let mut scheduler = AutosaveScheduler::try_new(SchedulerConfig::default(), store).unwrap();
    let mut state = authority();
    let (mut scheduler, mut state, result) = observe_held(entered, &release, move |started| {
        let result = scheduler.flush(
            Deadline::after(Instant::now(), Duration::from_millis(500)).unwrap(),
            &mut state,
            &RealClock,
        );
        started.recv().unwrap();
        (scheduler, state, result)
    });
    assert_eq!(
        result,
        Err(ServerError::Timeout {
            operation: mornlea_server::contracts::Operation::Flush
        })
    );
    assert_eq!(scheduler.tracked_submits(), 1);
    assert_eq!(
        scheduler
            .flush(deadline(), &mut state, &RealClock)
            .unwrap()
            .durable,
        1
    );
    assert_eq!(
        calls.lock().unwrap().writes,
        1,
        "retry consumes the original metadata ticket"
    );
    assert_eq!(scheduler.tracked_submits(), 0);
    scheduler.close(deadline()).unwrap();
}

#[test]
fn maximum_jobs_stay_charged_through_completed_replies() {
    let (mut backend, _) = scripted();
    let (write, entered, release) = gate();
    backend.write_gate = Some(write);
    let mut store = StoreMailbox::try_new_background(limits(), backend).unwrap();
    let request = SaveRequest {
        snapshots: vec![player_snapshot()],
    };
    let tickets = (0..4)
        .map(|_| store.submit(request.clone()).unwrap())
        .collect::<Vec<_>>();
    let held_request = request.clone();
    let (mut store, workers, queued, refused) = observe_held(entered, &release, move |started| {
        store.drive_workers();
        started.recv().unwrap();
        let workers = store.worker_jobs();
        let queued = store.queued_jobs();
        let refused = store.submit(held_request).unwrap_err();
        (store, workers, queued, refused)
    });
    assert_eq!(workers, 2);
    assert_eq!(queued, 2);
    assert_eq!(refused.request, request);
    assert!(matches!(
        refused.error,
        ServerError::Capacity {
            limit: 4,
            observed: 5,
            ..
        }
    ));
    let until = Instant::now() + BOUND;
    while store.held_completions() < 4 {
        store.drive_workers();
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    assert_eq!(store.occupancy().jobs, 4);
    assert_eq!(store.occupancy().players, 4);
    assert_eq!(store.submit(request.clone()).unwrap_err().request, request);
    for ticket in tickets {
        assert_eq!(
            wait_completion(&mut store, ticket).snapshots,
            request.snapshots
        );
    }
    assert_eq!(store.occupancy(), Default::default());
    store.close(deadline()).unwrap();
}

struct FrozenClock(Instant);
impl mornlea_server::contracts::Clock for FrozenClock {
    fn monotonic(&self) -> Instant {
        self.0
    }
    fn unix_ms(&self) -> i64 {
        0
    }
}

#[test]
fn flush_checks_caller_and_host_time_while_inline_keeps_virtual_time() {
    // A host-expired deadline is still future time for this deterministic inline double.
    let expired_host = Deadline::at(Instant::now() - Duration::from_secs(1));
    let virtual_clock = FrozenClock(expired_host.instant() - Duration::from_secs(1));
    let (backend, _) = scripted();
    let mut inline = StoreMailbox::try_new(limits(), backend).unwrap();
    inline
        .submit(SaveRequest {
            snapshots: vec![snapshot(1)],
        })
        .unwrap();
    assert_eq!(
        inline
            .flush(expired_host, &mut authority(), &virtual_clock)
            .unwrap()
            .durable,
        1
    );
    inline.close(expired_host).unwrap();

    let (backend, _) = scripted();
    let mut background = StoreMailbox::try_new_background(limits(), backend).unwrap();
    background
        .submit(SaveRequest {
            snapshots: vec![snapshot(1)],
        })
        .unwrap();
    assert_eq!(
        background.flush(expired_host, &mut authority(), &virtual_clock),
        Err(ServerError::Timeout {
            operation: mornlea_server::contracts::Operation::Flush
        })
    );
    assert_eq!(background.queued_jobs(), 1);
    let future_deadline = deadline();
    assert_eq!(
        background.flush(
            future_deadline,
            &mut authority(),
            &FrozenClock(future_deadline.instant())
        ),
        Err(ServerError::Timeout {
            operation: mornlea_server::contracts::Operation::Flush
        })
    );
    assert_eq!(background.queued_jobs(), 1);
    assert_eq!(
        background
            .flush(deadline(), &mut authority(), &RealClock)
            .unwrap()
            .durable,
        1
    );
    background.close(deadline()).unwrap();
}

#[test]
fn stationary_caller_clock_cannot_extend_background_flush_wait() {
    let (mut backend, _) = scripted();
    let (write, entered, release) = gate();
    backend.write_gate = Some(write);
    let mut store = StoreMailbox::try_new_background(limits(), backend).unwrap();
    let ticket = store
        .submit(SaveRequest {
            snapshots: vec![snapshot(1)],
        })
        .unwrap();
    let (mut store, held_deadline, result) = observe_held(entered, &release, move |started| {
        store.drive_workers();
        started.recv().unwrap();
        let now = Instant::now();
        let held_deadline = Deadline::after(now, Duration::from_millis(30)).unwrap();
        let result = store.flush(held_deadline, &mut authority(), &FrozenClock(now));
        (store, held_deadline, result)
    });
    assert_eq!(
        result,
        Err(ServerError::Timeout {
            operation: mornlea_server::contracts::Operation::Flush
        })
    );
    assert!(held_deadline.expired(Instant::now()));
    assert_eq!(store.occupancy().jobs, 1);
    assert_eq!(
        wait_completion(&mut store, ticket).snapshots,
        vec![snapshot(1)]
    );
    store
        .close(Deadline::after(Instant::now(), BOUND).unwrap())
        .unwrap();
}

#[test]
fn unexpected_owner_exit_retains_lifecycle_failure() {
    let (mut backend, calls) = scripted();
    backend.panic_drop = true;
    let mut store = StoreMailbox::try_new_background(limits(), backend).unwrap();
    let error = store.close(deadline()).unwrap_err();
    assert!(matches!(error, ServerError::Internal { .. }));
    assert!(
        matches!(
            store.sync(deadline()),
            Err(ServerError::InvalidState { .. })
        ),
        "failed join retains unresolved close ownership"
    );
    assert!(matches!(
        store.close(deadline()),
        Err(ServerError::Internal { .. })
    ));
    assert_eq!(calls.lock().unwrap().closes, 1);
}

#[test]
fn close_deadline_bounds_backend_drop_and_retry_joins_once() {
    let (mut backend, calls) = scripted();
    let (drop_gate, entered, release) = gate();
    backend.drop_gate = Some(drop_gate);
    let store = StoreMailbox::try_new_background(limits(), backend).unwrap();
    let (mut store, result, different_kind) = observe_held(entered, &release, move |started| {
        let mut store = store;
        let result =
            store.close(Deadline::after(Instant::now(), Duration::from_millis(500)).unwrap());
        started.recv().unwrap();
        // Backend close succeeded, but destruction still owns the thread.
        let different_kind = store.sync(deadline());
        (store, result, different_kind)
    });
    assert_eq!(
        result,
        Err(ServerError::Timeout {
            operation: mornlea_server::contracts::Operation::Close
        })
    );
    assert!(matches!(
        different_kind,
        Err(ServerError::InvalidState { .. })
    ));
    store.close(deadline()).unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls.closes, 1, "retry consumes the retained success");
    assert_eq!(calls.drops, 1, "retry joins the original owner");
}
