//! Load owner evidence: gates release before assertions and thread joins.
use mornlea_domain::{ChunkPos, Dimension, PlayerId};
use mornlea_server::contracts::*;
use mornlea_server::store::mailbox::StoreMailbox;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};
const BOUND: Duration = Duration::from_secs(5);
fn deadline() -> Deadline {
    Deadline::after(Instant::now(), BOUND).unwrap()
}
fn limits() -> StoreLimits {
    StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap()
}
fn player() -> PlayerId {
    PlayerId::try_from_bytes([1, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 0]).unwrap()
}
fn key() -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    }
}
#[derive(Clone)]
struct Gate {
    entered: mpsc::SyncSender<()>,
    opened: Arc<(Mutex<bool>, Condvar)>,
}
struct Release(Arc<(Mutex<bool>, Condvar)>);
impl Release {
    fn open(&self) {
        let (l, c) = &*self.0;
        *l.lock().unwrap() = true;
        c.notify_all();
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        self.open();
    }
}
fn gate() -> (Gate, mpsc::Receiver<()>, Release) {
    let (tx, rx) = mpsc::sync_channel(1);
    let a = Arc::new((Mutex::new(false), Condvar::new()));
    (
        Gate {
            entered: tx,
            opened: a.clone(),
        },
        rx,
        Release(a),
    )
}
impl Gate {
    fn hold(&self) {
        self.entered.send(()).unwrap();
        let (l, c) = &*self.opened;
        drop(c.wait_while(l.lock().unwrap(), |x| !*x).unwrap());
    }
}
#[derive(Default)]
struct Calls {
    loads: Vec<(SaveKey, thread::ThreadId)>,
    writes: Vec<thread::ThreadId>,
    closes: usize,
}
struct Backend {
    gate: Option<Gate>,
    calls: Arc<Mutex<Calls>>,
    results: std::collections::VecDeque<Result<LoadedValue, ServerError>>,
    panic_next: bool,
}
fn backend() -> (Backend, Arc<Mutex<Calls>>) {
    let calls = Arc::new(Mutex::new(Calls::default()));
    (
        Backend {
            gate: None,
            calls: calls.clone(),
            results: Default::default(),
            panic_next: false,
        },
        calls,
    )
}
fn missing() -> ServerError {
    ServerError::Io {
        operation: Operation::Load,
        kind: std::io::ErrorKind::NotFound,
    }
}
impl DiskBackend for Backend {
    fn load(&mut self, k: SaveKey) -> Result<LoadedValue, ServerError> {
        self.calls
            .lock()
            .unwrap()
            .loads
            .push((k, thread::current().id()));
        if let Some(g) = self.gate.take() {
            g.hold();
        }
        if std::mem::take(&mut self.panic_next) {
            panic!("load fixture panic")
        }
        self.results.pop_front().unwrap_or(Err(missing()))
    }
    fn write(&mut self, t: SaveTicket, r: SaveRequest) -> SaveCompletion {
        self.calls
            .lock()
            .unwrap()
            .writes
            .push(thread::current().id());
        SaveCompletion {
            ticket: t,
            submitted: r
                .snapshots
                .iter()
                .map(|s| (s.key.clone(), s.revision))
                .collect(),
            committed: r
                .snapshots
                .iter()
                .map(|s| (s.key.clone(), s.revision))
                .collect(),
            snapshots: r.snapshots,
            error: None,
        }
    }
    fn sync(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), ServerError> {
        self.calls.lock().unwrap().closes += 1;
        Ok(())
    }
}
#[test]
fn held_load_keeps_authority_calls_free() {
    let (mut b, _) = backend();
    let (g, entered, release) = gate();
    b.gate = Some(g);
    let (tx, rx) = mpsc::sync_channel(1);
    let (start, started) = mpsc::sync_channel(1);
    let driver = thread::spawn(move || {
        let mut store = StoreMailbox::try_new_background(limits(), b).unwrap();
        let t = store.start(player(), deadline()).unwrap();
        store.drive_workers();
        started.recv().unwrap();
        let pending = PlayerLoadPort::poll(&mut store, t);
        let save = store
            .submit(SaveRequest {
                snapshots: vec![player_snapshot()],
            })
            .unwrap();
        store.drive_workers();
        let save_pending = StoreHandle::poll(&mut store, save);
        tx.send((store, t, pending, save, save_pending)).unwrap();
    });
    let entered_result = entered.recv_timeout(BOUND);
    let _ = start.send(());
    let observed = rx.recv_timeout(BOUND);
    let returned = observed.is_ok();
    release.open();
    let result = observed.or_else(|_| rx.recv_timeout(BOUND));
    let joined = driver.join();
    assert!(entered_result.is_ok());
    assert!(joined.is_ok());
    assert!(returned, "background load must return while held");
    let (mut store, t, pending, save, save_pending) = result.unwrap();
    assert!(matches!(pending, LoadPoll::Pending));
    assert!(matches!(save_pending, SavePoll::Pending));
    assert!(wait_save(&mut store, save).error.is_none());
    assert!(matches!(wait_player(&mut store, t), LoadPoll::Loaded(None)));
    store.close(deadline()).unwrap();
}

fn wait_player<B: DiskBackend>(s: &mut StoreMailbox<B>, t: LoginTicket) -> LoadPoll {
    let until = Instant::now() + BOUND;
    loop {
        s.drive_workers();
        let v = PlayerLoadPort::poll(s, t);
        if !matches!(v, LoadPoll::Pending) {
            return v;
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
}
fn wait_chunk<B: DiskBackend>(s: &mut StoreMailbox<B>, t: ChunkRequestId) -> ChunkLoadPoll {
    let until = Instant::now() + BOUND;
    loop {
        s.drive_workers();
        let v = s.poll_chunk(t);
        if !matches!(v, ChunkLoadPoll::Pending) {
            return v;
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
}

#[test]
fn bounded_lanes_hold_queued_completed_and_consumed_identity() {
    let (b, calls) = backend();
    let mut s = StoreMailbox::try_new(limits(), b).unwrap();
    assert_eq!(
        s.start_chunk(key(), 0, deadline()),
        Err(ServerError::InvalidInput {
            field: "chunk_generation"
        })
    );
    let p: Vec<_> = (0..16)
        .map(|_| s.start(player(), deadline()).unwrap())
        .collect();
    let c: Vec<_> = (0..8)
        .map(|_| s.start_chunk(key(), 1, deadline()).unwrap())
        .collect();
    assert_eq!(p[0].get(), 1);
    assert_eq!(c[0].get(), 1);
    let pfull = ServerError::Capacity {
        resource: Resource::PendingLogins,
        limit: 16,
        observed: 17,
    };
    let cfull = ServerError::Capacity {
        resource: Resource::ChunkResults,
        limit: 8,
        observed: 9,
    };
    assert_eq!(s.start(player(), deadline()), Err(pfull));
    assert_eq!(s.start_chunk(key(), 1, deadline()), Err(cfull));
    assert!(matches!(
        PlayerLoadPort::poll(&mut s, p[0]),
        LoadPoll::Pending
    ));
    assert!(calls.lock().unwrap().loads.is_empty());
    for _ in 0..12 {
        s.drive_workers();
    }
    assert_eq!(calls.lock().unwrap().loads.len(), 24);
    assert_eq!(s.start(player(), deadline()), Err(pfull));
    assert_eq!(s.start_chunk(key(), 1, deadline()), Err(cfull));
    assert!(matches!(
        PlayerLoadPort::poll(&mut s, p[0]),
        LoadPoll::Loaded(None)
    ));
    assert!(matches!(s.poll_chunk(c[0]), ChunkLoadPoll::Loaded(None)));
    assert_eq!(s.start(player(), deadline()).unwrap().get(), 17);
    assert_eq!(s.start_chunk(key(), 1, deadline()).unwrap().get(), 9);
    assert!(matches!(
        PlayerLoadPort::poll(&mut s, p[0]),
        LoadPoll::Pending
    ));
    assert!(matches!(s.poll_chunk(c[0]), ChunkLoadPoll::Pending));
    for t in p {
        s.cancel(t).unwrap();
        s.cancel(t).unwrap();
    }
    for t in c {
        s.cancel_chunk(t).unwrap();
        s.cancel_chunk(t).unwrap();
    }
    s.cancel(LoginTicket::try_from_raw(17).unwrap()).unwrap();
    s.cancel_chunk(ChunkRequestId::try_new(9).unwrap()).unwrap();
    s.close(deadline()).unwrap();
}
#[test]
fn inline_non_send_cancel_and_expiry_do_not_load_until_drive() {
    struct Local(std::rc::Rc<std::cell::Cell<usize>>);
    impl DiskBackend for Local {
        fn load(&mut self, _: SaveKey) -> Result<LoadedValue, ServerError> {
            self.0.set(self.0.get() + 1);
            Err(missing())
        }
        fn write(&mut self, _: SaveTicket, _: SaveRequest) -> SaveCompletion {
            unreachable!()
        }
        fn sync(&mut self) -> Result<(), ServerError> {
            Ok(())
        }
        fn close(&mut self) -> Result<(), ServerError> {
            Ok(())
        }
    }
    let calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut s = StoreMailbox::try_new(limits(), Local(calls.clone())).unwrap();
    let t = s.start(player(), deadline()).unwrap();
    s.cancel(t).unwrap();
    s.drive_workers();
    assert_eq!(calls.get(), 0);
    let t = s
        .start(
            player(),
            Deadline::at(Instant::now() - Duration::from_secs(1)),
        )
        .unwrap();
    assert!(matches!(PlayerLoadPort::poll(&mut s, t), LoadPoll::Pending));
    s.drive_workers();
    assert_eq!(
        wait_player(&mut s, t),
        LoadPoll::Failed(ServerError::Timeout {
            operation: Operation::Load
        })
    );
    assert_eq!(calls.get(), 0);
    let t = s.start(player(), deadline()).unwrap();
    assert!(matches!(PlayerLoadPort::poll(&mut s, t), LoadPoll::Pending));
    assert_eq!(calls.get(), 0);
    s.drive_workers();
    assert_eq!(wait_player(&mut s, t), LoadPoll::Loaded(None));
    assert_eq!(calls.get(), 1);
    s.close(deadline()).unwrap();
}
#[test]
fn typed_failures_panic_and_missing_keep_owner_alive() {
    let (b, calls) = backend();
    let mut b = b;
    let corruption = ServerError::Io {
        operation: Operation::Load,
        kind: std::io::ErrorKind::InvalidData,
    };
    let wrong_notfound = ServerError::Io {
        operation: Operation::Sync,
        kind: std::io::ErrorKind::NotFound,
    };
    b.results = vec![
        Err(corruption),
        Err(wrong_notfound),
        Ok(LoadedValue::Hostiles(mornlea_storage::HostileMobs {
            revision: 1,
            records: vec![],
        })),
        Err(missing()),
    ]
    .into();
    b.panic_next = true;
    let mut s = StoreMailbox::try_new_background(limits(), b).unwrap();
    for expected in [
        ServerError::Internal {
            invariant: "store load panic",
        },
        corruption,
        wrong_notfound,
        ServerError::Internal {
            invariant: "store load family",
        },
    ] {
        let t = s.start(player(), deadline()).unwrap();
        assert_eq!(wait_player(&mut s, t), LoadPoll::Failed(expected));
    }
    let t = s.start(player(), deadline()).unwrap();
    assert_eq!(wait_player(&mut s, t), LoadPoll::Loaded(None));
    let t = s.start_chunk(key(), 7, deadline()).unwrap();
    assert!(matches!(wait_chunk(&mut s, t), ChunkLoadPoll::Loaded(None)));
    s.close(deadline()).unwrap();
    assert_eq!(calls.lock().unwrap().loads.len(), 6);
}
#[test]
fn started_cancel_retains_capacity_and_close_freezes_admission() {
    let (mut b, calls) = backend();
    let (g, entered, release) = gate();
    b.gate = Some(g);
    let mut s = StoreMailbox::try_new_background(limits(), b).unwrap();
    let t = s.start(player(), deadline()).unwrap();
    s.drive_workers();
    let entered_result = entered.recv_timeout(BOUND);
    s.cancel(t).unwrap();
    let others: Vec<_> = (0..15)
        .map(|_| s.start(player(), deadline()).unwrap())
        .collect();
    let full = s.start(player(), deadline());
    let closing = s.close(deadline());
    let frozen = s.start_chunk(key(), 2, deadline());
    let never_calls = calls.lock().unwrap().closes;
    for t in others {
        s.cancel(t).unwrap();
    }
    release.open();
    assert!(entered_result.is_ok());
    assert!(matches!(full, Err(ServerError::Capacity { limit: 16, .. })));
    assert_eq!(
        closing,
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closing
        })
    );
    assert_eq!(
        frozen,
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closing
        })
    );
    assert_eq!(never_calls, 0);
    let until = Instant::now() + BOUND;
    loop {
        s.drive_workers();
        assert!(matches!(PlayerLoadPort::poll(&mut s, t), LoadPoll::Pending));
        match s.close(deadline()) {
            Ok(()) => break,
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing,
            }) => {
                assert!(Instant::now() < until);
                thread::yield_now();
            }
            x => panic!("unexpected close {x:?}"),
        }
    }
    assert_eq!(
        s.start(player(), deadline()),
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closed
        })
    );
    assert_eq!(calls.lock().unwrap().loads.len(), 1);
}

#[test]
fn queued_close_freezes_and_cancellation_releases_without_disk() {
    let (b, calls) = backend();
    let mut s = StoreMailbox::try_new_background(limits(), b).unwrap();
    let t = s.start_chunk(key(), 1, deadline()).unwrap();
    assert_eq!(
        s.close(deadline()),
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closing
        })
    );
    assert_eq!(
        s.start(player(), deadline()),
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closing
        })
    );
    s.cancel_chunk(t).unwrap();
    s.close(deadline()).unwrap();
    let calls = calls.lock().unwrap();
    assert!(calls.loads.is_empty());
    assert_eq!(calls.closes, 1);
}

use mornlea_server::store::disk::{DiskOptions, DiskStore};
use mornlea_storage::{
    ChestSlot, Chunk, ChunkSave, ContainerSnapshot, DropSlot, FurnaceSlot, Inventory, ItemStack,
    METADATA_CURRENT_VERSION, Metadata, MetadataChunkPos, PlayerLocation, PlayerSave, StorageKind,
};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
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
        SaveValue::Player(PlayerSave {
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
        SaveValue::Chunk(ChunkSave {
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

fn wait_save<B: DiskBackend>(s: &mut StoreMailbox<B>, t: SaveTicket) -> SaveCompletion {
    let until = Instant::now() + BOUND;
    loop {
        s.drive_workers();
        if let SavePoll::Completed(v) = StoreHandle::poll(s, t) {
            return v;
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
}
/// Explicit test-only delegation: the gate precedes the real disk decoder.
struct GatedRealLoad {
    disk: DiskStore,
    gate: Option<Gate>,
}
impl DiskBackend for GatedRealLoad {
    fn load(&mut self, k: SaveKey) -> Result<LoadedValue, ServerError> {
        if let Some(g) = self.gate.take() {
            g.hold();
        }
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
fn real_background_save_load_values_missing_read_gate_and_world_lease() {
    let root = Root::new();
    let (g, entered, release) = gate();
    let mut store = StoreMailbox::try_new_background(
        limits(),
        GatedRealLoad {
            disk: DiskStore::open(&root.0, options()).unwrap(),
            gate: Some(g),
        },
    )
    .unwrap();
    let expected_player = player_snapshot();
    let expected_chunk = chunk_snapshot();
    let t = store
        .submit(SaveRequest {
            snapshots: vec![expected_player.clone(), expected_chunk.clone()],
        })
        .unwrap();
    let completion = wait_save(&mut store, t);
    assert_eq!(
        completion.snapshots,
        vec![expected_player.clone(), expected_chunk.clone()]
    );
    assert_eq!(
        completion.committed,
        vec![
            (expected_chunk.key.clone(), 9),
            (expected_player.key.clone(), 9)
        ]
    );
    assert!(completion.error.is_none());
    let pt = store.start(player(), deadline()).unwrap();
    store.drive_workers();
    let entered_result = entered.recv_timeout(BOUND);
    let (tx, rx) = mpsc::sync_channel(1);
    let chunk_key = match expected_chunk.key {
        SaveKey::Chunk(k) => k,
        _ => unreachable!(),
    };
    let driver = thread::spawn(move || {
        let ct = store.start_chunk(chunk_key, 33, deadline()).unwrap();
        store.drive_workers();
        let p = PlayerLoadPort::poll(&mut store, pt);
        let c = store.poll_chunk(ct);
        tx.send((store, ct, p, c)).unwrap();
    });
    let observed = rx.recv_timeout(BOUND);
    let returned = observed.is_ok();
    release.open();
    let result = observed.or_else(|_| rx.recv_timeout(BOUND));
    let joined = driver.join();
    assert!(entered_result.is_ok());
    assert!(joined.is_ok());
    assert!(returned);
    let (mut store, ct, p, c) = result.unwrap();
    assert!(matches!(p, LoadPoll::Pending));
    assert!(matches!(c, ChunkLoadPoll::Pending));
    assert!(DiskStore::open(&root.0, options()).is_err());
    let LoadPoll::Loaded(Some(loaded)) = wait_player(&mut store, pt) else {
        panic!("real player load")
    };
    let SaveValue::Player(saved) = expected_player.value else {
        unreachable!()
    };
    let bytes = mornlea_storage::encode_player(&saved).unwrap();
    assert_eq!(
        loaded,
        mornlea_storage::decode_player(saved.player_id, &bytes).unwrap()
    );
    let ChunkLoadPoll::Loaded(Some(loaded)) = wait_chunk(&mut store, ct) else {
        panic!("real chunk load")
    };
    assert_eq!(
        (
            loaded.key(),
            loaded.generation(),
            loaded.revision(),
            loaded.persisted_revision(),
            loaded.needs_rewrite(),
            loaded.recovered()
        ),
        (chunk_key, 33, 9, 9, false, false)
    );
    let (ready, persisted, rewrite, recovered) = loaded.into_parts();
    let observed = observe_ready(ready);
    let SaveValue::Chunk(saved) = expected_chunk.value else {
        unreachable!()
    };
    assert_eq!(observed, saved.chunk);
    assert_eq!((persisted, rewrite, recovered), (9, false, false));
    let mut id = player().bytes();
    id[0] = 2;
    let absent = store
        .start(PlayerId::try_from_bytes(id).unwrap(), deadline())
        .unwrap();
    assert_eq!(wait_player(&mut store, absent), LoadPoll::Loaded(None));
    let absent = store.start_chunk(key(), 1, deadline()).unwrap();
    assert!(matches!(
        wait_chunk(&mut store, absent),
        ChunkLoadPoll::Loaded(None)
    ));
    store.close(deadline()).unwrap();
    let mut reopened = DiskStore::open(&root.0, options()).unwrap();
    reopened.close().unwrap();
}

fn observe_ready(ready: mornlea_server::core::world::ReadyChunk) -> Chunk {
    let mut state = mornlea_server::state::AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        13,
    )
    .unwrap();
    let mut ctx = mornlea_server::state::TickContext::harness(&mut state, TickBudget::full());
    ctx.preload_ready_chunk(ready);
    ctx.resident_snapshot().ready_snapshot().remove(0).3
}
#[test]
fn real_recovered_chunk_preserves_facts_and_does_not_rewrite_disk() {
    use mornlea_storage::{
        BANK_A_START_SECTOR, BANK_SIZE, RegionKey, SECTOR_SIZE, decode_region_bank,
    };
    let root = Root::new();
    let mut store =
        StoreMailbox::try_new_background(limits(), DiskStore::open(&root.0, options()).unwrap())
            .unwrap();
    let mut saved = match chunk_snapshot().value {
        SaveValue::Chunk(v) => v,
        _ => unreachable!(),
    };
    saved.key.x = 0;
    saved.key.z = 0;
    saved.revision = 7;
    let original = saved.chunk.clone();
    for revision in [7, 8] {
        saved.revision = revision;
        saved.chunk.sections[0].single = revision as u16;
        let t = store
            .submit(SaveRequest {
                snapshots: vec![
                    OwnedSnapshot::try_new(
                        SaveKey::Chunk(key()),
                        revision,
                        1,
                        SaveUrgency::Autosave,
                        SaveValue::Chunk(saved.clone()),
                    )
                    .unwrap(),
                ],
            })
            .unwrap();
        assert!(wait_save(&mut store, t).error.is_none());
    }
    let mut standby = original;
    standby.sections[0].single = 7;
    store.close(deadline()).unwrap();
    let path = root.0.join("dimensions/0/regions/r.0.0.region");
    let mut bytes = fs::read(&path).unwrap();
    let at = BANK_A_START_SECTOR as usize * SECTOR_SIZE as usize;
    let active = decode_region_bank(
        RegionKey {
            dimension: 0,
            x: 0,
            z: 0,
        },
        &bytes[at..at + BANK_SIZE],
        bytes.len() as i64,
    )
    .unwrap();
    let payload = active.entries[0].offset_sector as usize * SECTOR_SIZE as usize;
    bytes[payload] ^= 0xff;
    fs::write(&path, &bytes).unwrap();
    let mut store =
        StoreMailbox::try_new_background(limits(), DiskStore::open(&root.0, options()).unwrap())
            .unwrap();
    let t = store.start_chunk(key(), 99, deadline()).unwrap();
    let ChunkLoadPoll::Loaded(Some(v)) = wait_chunk(&mut store, t) else {
        panic!("expected real recovery")
    };
    assert_eq!(
        (
            v.key(),
            v.generation(),
            v.revision(),
            v.persisted_revision(),
            v.needs_rewrite(),
            v.recovered()
        ),
        (key(), 99, 9, 7, true, true)
    );
    let (ready, persisted, rewrite, recovered) = v.into_parts();
    assert_eq!(observe_ready(ready), standby);
    assert_eq!((persisted, rewrite, recovered), (7, true, true));
    assert_eq!(fs::read(&path).unwrap(), bytes);
    store.close(deadline()).unwrap();
}
#[test]
fn started_expiry_keeps_real_outcome_and_queued_expiry_never_calls() {
    let (mut b, calls) = backend();
    let (g, entered, release) = gate();
    b.gate = Some(g);
    let mut s = StoreMailbox::try_new_background(limits(), b).unwrap();
    let d = deadline();
    let t = s.start(player(), d).unwrap();
    s.drive_workers();
    if let Err(error) = entered.recv_timeout(BOUND) {
        release.open();
        panic!("load did not enter before the test bound: {error}");
    }
    let queued = s
        .start_chunk(
            key(),
            1,
            Deadline::at(Instant::now() - Duration::from_secs(1)),
        )
        .unwrap();
    s.drive_workers();
    let remaining = d.instant().saturating_duration_since(Instant::now());
    let (_tx, rx) = mpsc::sync_channel::<()>(1);
    let _ = rx.recv_timeout(remaining);
    let expired = d.expired(Instant::now());
    release.open();
    assert!(expired);
    assert_eq!(wait_player(&mut s, t), LoadPoll::Loaded(None));
    assert!(matches!(
        wait_chunk(&mut s, queued),
        ChunkLoadPoll::Failed(ServerError::Timeout {
            operation: Operation::Load
        })
    ));
    assert_eq!(calls.lock().unwrap().loads.len(), 1);
    s.close(deadline()).unwrap();
}
#[test]
fn mixed_save_load_fifo_fairness_and_same_owner() {
    let (b, calls) = backend();
    let mut s = StoreMailbox::try_new_background(limits(), b).unwrap();
    let main = thread::current().id();
    let pt = s.start(player(), deadline()).unwrap();
    let ct = s.start_chunk(key(), 5, deadline()).unwrap();
    let t = s
        .submit(SaveRequest {
            snapshots: vec![player_snapshot()],
        })
        .unwrap();
    assert!(wait_save(&mut s, t).error.is_none());
    assert_eq!(wait_player(&mut s, pt), LoadPoll::Loaded(None));
    assert!(matches!(
        wait_chunk(&mut s, ct),
        ChunkLoadPoll::Loaded(None)
    ));
    s.close(deadline()).unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls.loads[0].0, SaveKey::Player(player()));
    assert_eq!(calls.loads[1].0, SaveKey::Chunk(key()));
    assert!(
        calls
            .loads
            .iter()
            .all(|(_, id)| *id == calls.writes[0] && *id != main)
    );
}
#[test]
fn wrong_player_future_corrupt_and_malformed_chunk_are_typed() {
    let saved = match player_snapshot().value {
        SaveValue::Player(v) => v,
        _ => unreachable!(),
    };
    let bytes = mornlea_storage::encode_player(&saved).unwrap();
    let mut stored = mornlea_storage::decode_player(saved.player_id, &bytes).unwrap();
    stored.player_id = mornlea_storage::PlayerId::from_bytes([
        2, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 0,
    ]);
    let (mut b, _) = backend();
    b.results.push_back(Ok(LoadedValue::Player(stored)));
    for kind in [StorageFailure::FutureVersion, StorageFailure::Corrupt] {
        b.results.push_back(Err(ServerError::Storage {
            family: "player",
            kind,
        }));
    }
    let mut malformed = match chunk_snapshot().value {
        SaveValue::Chunk(v) => v.chunk,
        _ => unreachable!(),
    };
    malformed.sections.clear();
    b.results.push_back(Ok(LoadedValue::Chunk(RecoveredChunk {
        chunk: malformed,
        revision: 9,
        persisted_revision: 4,
        needs_rewrite: true,
        recovered: true,
    })));
    let mut s = StoreMailbox::try_new_background(limits(), b).unwrap();
    for e in [
        ServerError::InvalidInput {
            field: "loaded_player",
        },
        ServerError::Storage {
            family: "player",
            kind: StorageFailure::FutureVersion,
        },
        ServerError::Storage {
            family: "player",
            kind: StorageFailure::Corrupt,
        },
    ] {
        let t = s.start(player(), deadline()).unwrap();
        assert_eq!(wait_player(&mut s, t), LoadPoll::Failed(e));
    }
    let t = s.start_chunk(key(), 1, deadline()).unwrap();
    assert!(matches!(
        wait_chunk(&mut s, t),
        ChunkLoadPoll::Failed(ServerError::InvalidInput {
            field: "ready_chunk"
        })
    ));
    s.close(deadline()).unwrap();
}

#[test]
fn close_timeout_keeps_load_admission_frozen_across_retry() {
    let (b, calls) = backend();
    let mut s = StoreMailbox::try_new_background(limits(), b).unwrap();
    assert_eq!(
        s.close(Deadline::at(Instant::now() - Duration::from_secs(1))),
        Err(ServerError::Timeout {
            operation: Operation::Close
        })
    );
    assert_eq!(
        s.start(player(), deadline()),
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closing
        })
    );
    assert_eq!(
        s.start_chunk(key(), 1, deadline()),
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closing
        })
    );
    s.close(deadline()).unwrap();
    assert_eq!(calls.lock().unwrap().closes, 1);
}
#[test]
fn flush_preserves_load_ownership_and_consumption() {
    let (b, calls) = backend();
    let mut s = StoreMailbox::try_new(limits(), b).unwrap();
    let t = s.start_chunk(key(), 1, deadline()).unwrap();
    let mut state = mornlea_server::state::AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        13,
    )
    .unwrap();
    s.submit(SaveRequest {
        snapshots: vec![player_snapshot()],
    })
    .unwrap();
    s.flush(deadline(), &mut state, &HostClock).unwrap();
    assert_eq!(calls.lock().unwrap().loads.len(), 1);
    assert_eq!(
        s.close(deadline()),
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closing
        })
    );
    assert!(matches!(s.poll_chunk(t), ChunkLoadPoll::Loaded(None)));
    assert!(matches!(s.poll_chunk(t), ChunkLoadPoll::Pending));
    s.close(deadline()).unwrap();
}
struct HostClock;
impl Clock for HostClock {
    fn monotonic(&self) -> Instant {
        Instant::now()
    }
    fn unix_ms(&self) -> i64 {
        0
    }
}

#[test]
fn player_reply_moves_allocation_and_preserves_rewrite_fact() {
    let SaveValue::Player(saved) = player_snapshot().value else {
        unreachable!()
    };
    let bytes = mornlea_storage::encode_player(&saved).unwrap();
    let mut stored = mornlea_storage::decode_player(saved.player_id, &bytes).unwrap();
    stored.needs_rewrite = true;
    let name_allocation = stored.display_name.as_ptr() as usize;
    let expected = stored.clone();
    let (mut b, _) = backend();
    b.results.push_back(Ok(LoadedValue::Player(stored)));
    let mut s = StoreMailbox::try_new_background(limits(), b).unwrap();
    let t = s.start(player(), deadline()).unwrap();
    let LoadPoll::Loaded(Some(loaded)) = wait_player(&mut s, t) else {
        panic!("expected player")
    };
    assert_eq!(loaded, expected);
    assert_eq!(loaded.display_name.as_ptr() as usize, name_allocation);
    assert!(matches!(PlayerLoadPort::poll(&mut s, t), LoadPoll::Pending));
    s.close(deadline()).unwrap();
}
#[test]
fn background_chunk_capacity_remains_charged_through_started_and_ready() {
    let (mut b, calls) = backend();
    let (g, entered, release) = gate();
    b.gate = Some(g);
    let mut s = StoreMailbox::try_new_background(limits(), b).unwrap();
    let tickets: Vec<_> = (0..8)
        .map(|_| s.start_chunk(key(), 71, deadline()).unwrap())
        .collect();
    s.drive_workers();
    let entered_result = entered.recv_timeout(BOUND);
    let refused = s.start_chunk(key(), 72, deadline());
    s.drive_workers();
    let before_release_calls = calls.lock().unwrap().loads.len();
    release.open();
    assert!(entered_result.is_ok());
    assert_eq!(
        refused,
        Err(ServerError::Capacity {
            resource: Resource::ChunkResults,
            limit: 8,
            observed: 9
        })
    );
    assert_eq!(before_release_calls, 1);
    let until = Instant::now() + BOUND;
    while calls.lock().unwrap().loads.len() < 8 {
        s.drive_workers();
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    s.drive_workers();
    assert_eq!(
        s.start_chunk(key(), 72, deadline()),
        Err(ServerError::Capacity {
            resource: Resource::ChunkResults,
            limit: 8,
            observed: 9
        })
    );
    for t in tickets {
        assert!(matches!(wait_chunk(&mut s, t), ChunkLoadPoll::Loaded(None)));
    }
    let next = s.start_chunk(key(), 72, deadline()).unwrap();
    assert_eq!(next.get(), 9);
    s.cancel_chunk(next).unwrap();
    s.close(deadline()).unwrap();
}
