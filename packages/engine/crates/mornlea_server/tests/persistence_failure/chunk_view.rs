//! Immutable authority captures through the sole filesystem owner, including
//! refusal, partial failure, exact retries, and reopened persistent values.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use mornlea_domain::{BlockPos, ChunkPos, Dimension, FiniteVec3};
use mornlea_server::contracts::*;
use mornlea_server::core::world::{ChunkSaveView, PreparedChunk, ReadyChunk};
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_server::store::disk::{DiskOptions, DiskStore};
use mornlea_server::store::io::{DiskIo, IoPhase};
use mornlea_server::store::mailbox::StoreMailbox;
use mornlea_storage::{
    Chunk, ChunkSave, ContainerSnapshot, ItemStack, Metadata, MetadataChunkPos, StorageKind,
};

const BOUND: Duration = Duration::from_secs(5);
static ROOTS: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mornlea-chunk-view-{}-{}",
            std::process::id(),
            ROOTS.fetch_add(1, Ordering::Relaxed)
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
fn options() -> DiskOptions {
    DiskOptions {
        region_handle_cap: 1,
        create: Metadata {
            format_version: mornlea_storage::METADATA_CURRENT_VERSION,
            seed: 13,
            spawn_dimension: 0,
            spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
            depths_spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
            depths_seed_salt: 0,
            world_time_ticks: 0,
            day_phase_offset: 0,
            weather_kind: 0,
            weather_ticks_remaining: 0,
            difficulty: 0,
        },
    }
}
fn limits() -> StoreLimits {
    StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap()
}
fn deadline() -> Deadline {
    Deadline::after(Instant::now(), BOUND).unwrap()
}
fn key() -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(-2, -3),
    }
}
fn empty() -> Chunk {
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
fn authority() -> AuthorityState {
    authority_at(key())
}
fn authority_at(chunk_key: ChunkKey) -> AuthorityState {
    let mut state = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        13,
    )
    .unwrap();
    let prepared = PreparedChunk::try_new(
        chunk_key,
        7,
        RecoveredChunk {
            chunk: empty(),
            revision: 5,
            persisted_revision: 5,
            needs_rewrite: false,
            recovered: false,
        },
    )
    .unwrap();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ctx.preload_ready_chunk(prepared.into_parts().0);
    let residents = ctx.resident_snapshot();
    drop(ctx);
    state.commit_residents(residents);
    state
}
fn view(snapshot: &OwnedSnapshot) -> &ChunkSaveView {
    let SaveValue::ChunkView(view) = &snapshot.value else {
        panic!("capture must remain lazy")
    };
    view
}
fn capture(state: &AuthorityState) -> OwnedSnapshot {
    state
        .capture_chunk_snapshot(key(), SaveUrgency::Autosave)
        .unwrap()
}
fn committed(snapshot: OwnedSnapshot, ticket: u64) -> SaveCompletion {
    let identity = (snapshot.key.clone(), snapshot.revision);
    SaveCompletion {
        ticket: SaveTicket::try_from_raw(ticket).unwrap(),
        snapshots: vec![snapshot],
        submitted: vec![identity.clone()],
        committed: vec![identity],
        error: None,
    }
}
fn select_capture(state: &mut AuthorityState, snapshot: &OwnedSnapshot) {
    state.remember_dirty(snapshot.clone()).unwrap();
    assert_eq!(
        state.select(SaveMode::All, SaveBudget::default()),
        vec![snapshot.clone()]
    );
}
#[test]
fn independent_captures_acknowledge_only_the_echoed_token() {
    for reverse in [false, true] {
        let mut state = authority();
        let first = capture(&state);
        let second = capture(&state);
        assert_eq!(first.key, second.key);
        assert_eq!(first.revision, second.revision);
        assert_ne!(first, second);
        assert_eq!(view(&first).materialize(), view(&second).materialize());
        select_capture(&mut state, &first);
        select_capture(&mut state, &second);
        assert_eq!(state.save_stats().in_flight, 2);
        let order = if reverse {
            [second, first]
        } else {
            [first, second]
        };
        for (index, snapshot) in order.into_iter().enumerate() {
            let ack = state.apply_completion(committed(snapshot, index as u64 + 1));
            assert_eq!(ack.acked, 1);
            assert_eq!(ack.released, 0);
            assert!(ack.retry.is_empty());
            assert!(ack.errors.is_empty());
            assert_eq!(state.save_stats().in_flight, 1 - index);
        }
    }
}
#[test]
fn independent_captures_return_dirty_only_the_echoed_token() {
    let mut state = authority();
    let first = capture(&state);
    let second = capture(&state);
    assert_ne!(first, second);
    assert_eq!(view(&first).materialize(), view(&second).materialize());
    select_capture(&mut state, &first);
    select_capture(&mut state, &second);
    state.return_dirty(first.clone());
    assert_eq!(state.save_stats().in_flight, 1);
    assert_eq!(state.save_stats().dirty, 1);
    let ack = state.apply_completion(committed(second, 2));
    assert_eq!(ack.acked, 1);
    assert!(ack.errors.is_empty());
    assert!(ack.retry.is_empty());
    assert_eq!(state.save_stats().in_flight, 0);
    assert_eq!(
        state.select(SaveMode::All, SaveBudget::default()),
        vec![first]
    );
}
#[test]
fn forged_sibling_refuses_and_unknown_and_direct_metadata_lanes_remain() {
    let mut state = authority();
    let first = capture(&state);
    let second = capture(&state);
    select_capture(&mut state, &first);
    select_capture(&mut state, &second);
    let forged = capture(&state);
    assert_ne!(forged, first);
    assert_ne!(forged, second);
    assert_eq!(view(&forged).materialize(), view(&first).materialize());
    let ack = state.apply_completion(committed(forged, 3));
    assert_eq!(ack.acked, 0);
    assert_eq!(ack.released, 0);
    assert!(ack.retry.is_empty());
    assert_eq!(
        ack.errors,
        vec![ServerError::Internal {
            invariant: "save completion identity"
        }]
    );
    assert_eq!(state.save_stats().in_flight, 2);
    let mut unselected = authority();
    let ack = unselected.apply_completion(committed(capture(&unselected), 4));
    assert_eq!(ack.acked, 0);
    assert!(ack.errors.is_empty());
    assert!(ack.retry.is_empty());
    assert_eq!(unselected.save_stats().in_flight, 0);
    let metadata = unselected.metadata_snapshot();
    let ack = unselected.apply_completion(committed(metadata, 5));
    assert_eq!(ack.acked, 1);
    assert!(ack.errors.is_empty());
    assert!(ack.retry.is_empty());
    assert_eq!(unselected.save_stats().in_flight, 0);
}
fn write(ctx: &mut TickContext<'_>, pos: BlockPos, block: u16) {
    let observed = ctx.read().observation(Dimension::OVERWORLD, pos).unwrap();
    ctx.transaction()
        .try_system(
            SystemRule::Support,
            vec![BlockWrite::try_new(observed, block).unwrap()],
        )
        .unwrap();
}
// Detached setup explicitly restores prepared data off the tick, then executes
// the real transaction and resident finalization APIs used by replay clients.
fn mutate(state: &mut AuthorityState, block: u16, slots: bool) {
    let (key, generation, revision, chunk) = state.residents().ready_snapshot().remove(0);
    let mut ctx = TickContext::harness(state, TickBudget::full());
    ctx.preload_ready_chunk(ReadyChunk::try_new(key, generation, revision, chunk).unwrap());
    write(&mut ctx, BlockPos::new(-31, -64, -47), block);
    write(&mut ctx, BlockPos::new(-30, 319, -46), block);
    if slots {
        write(&mut ctx, BlockPos::new(-29, 64, -45), 11);
        write(&mut ctx, BlockPos::new(-28, 80, -44), 9);
        let records = ctx.resident_snapshot().container_records();
        for record in records.into_values() {
            let before = ctx.read().container(record.reference).unwrap();
            let mut after = before.clone();
            match &mut after.slots {
                ContainerSlots::Chest(items) => {
                    items[0] = ItemStack {
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
                    slots[0] = ItemStack {
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
        let drops = DropBatch::try_new(
            DropSource::System {
                rule: SystemRule::Support,
                tick: ctx.read().tick(),
                target: BlockPos::new(-27, 64, -43),
            },
            Dimension::OVERWORLD,
            FiniteVec3::try_new([-26.5, 64.5, -42.5]).unwrap(),
            vec![ItemStack {
                item: 2,
                count: 4,
                durability: 0,
            }],
            5,
        )
        .unwrap();
        ctx.transaction()
            .try_system_with_drops(SystemRule::Support, vec![], drops)
            .unwrap();
    }
    let residents = ctx.resident_snapshot();
    drop(ctx);
    state.commit_residents(residents);
}
fn wait<B: DiskBackend>(store: &mut StoreMailbox<B>, ticket: SaveTicket) -> SaveCompletion {
    let until = Instant::now() + BOUND;
    loop {
        store.drive_workers();
        if let SavePoll::Completed(done) = StoreHandle::poll(store, ticket) {
            return done;
        }
        assert!(Instant::now() < until, "completion must arrive");
        thread::yield_now();
    }
}
fn reopen(root: &Root, expected: &ChunkSave) {
    let mut disk = DiskStore::open(&root.0, options()).unwrap();
    let LoadedValue::Chunk(loaded) = disk.load(SaveKey::Chunk(key())).unwrap() else {
        panic!("chunk family")
    };
    assert_eq!(loaded.revision, expected.revision);
    assert_eq!(loaded.persisted_revision, expected.revision);
    assert_eq!(loaded.chunk, expected.chunk);
    disk.close().unwrap();
}
#[test]
fn checked_capture_identity_and_zero_generation_refuse_without_scheduling() {
    let mut state = authority();
    let captured = capture(&state);
    assert_eq!(state.save_stats().dirty, 0);
    assert_eq!(state.save_stats().in_flight, 0);
    assert_eq!(view(&captured).key(), key());
    assert_eq!(view(&captured).generation(), 7);
    assert_eq!(view(&captured).revision(), 5);
    assert_eq!(view(&captured), view(&captured.clone()));
    assert_ne!(view(&captured), view(&capture(&state)));
    for (outer, revision) in [
        (
            SaveKey::Chunk(ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, 0),
            }),
            5,
        ),
        (captured.key.clone(), 6),
        (SaveKey::Metadata, 5),
    ] {
        assert!(
            OwnedSnapshot::try_new(
                outer,
                revision,
                1,
                SaveUrgency::Autosave,
                captured.value.clone()
            )
            .is_err()
        );
    }
    assert!(
        state
            .capture_chunk_snapshot(
                ChunkKey {
                    dimension: Dimension::OVERWORLD,
                    pos: ChunkPos::new(0, 0)
                },
                SaveUrgency::Autosave
            )
            .is_none()
    );
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ctx.preload_ready_chunk(ReadyChunk::try_new(key(), 0, 5, empty()).unwrap());
    let residents = ctx.resident_snapshot();
    drop(ctx);
    state.commit_residents(residents);
    assert!(
        state
            .capture_chunk_snapshot(key(), SaveUrgency::Autosave)
            .is_none()
    );
}
#[test]
fn direct_disk_normalizes_equal_independent_captures_and_echoes_originals() {
    let root = Root::new();
    let mut state = authority();
    mutate(&mut state, 4, true);
    let first = capture(&state);
    let equal = capture(&state);
    let expected = view(&first).materialize();
    assert_eq!(expected.revision, 6);
    assert!(expected.chunk.drops[0].active);
    assert!(expected.chunk.furnaces[0].active);
    assert!(expected.chunk.chests[0].active);
    let originals = vec![first, equal];
    let mut disk = DiskStore::open(&root.0, options()).unwrap();
    let done = disk.write(
        SaveTicket::try_from_raw(1).unwrap(),
        SaveRequest {
            snapshots: originals.clone(),
        },
    );
    assert_eq!(done.snapshots, originals);
    assert_eq!(done.committed, vec![(SaveKey::Chunk(key()), 6)]);
    assert!(done.error.is_none());
    disk.close().unwrap();
    reopen(&root, &expected);
}

#[derive(Clone)]
struct Gate(Arc<(Mutex<bool>, Condvar)>, mpsc::SyncSender<()>);
struct Release(Arc<(Mutex<bool>, Condvar)>);
impl Release {
    fn open(&self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        self.open();
    }
}
fn gate() -> (Gate, mpsc::Receiver<()>, Release) {
    let opened = Arc::new((Mutex::new(false), Condvar::new()));
    let (send, recv) = mpsc::sync_channel(1);
    (Gate(opened.clone(), send), recv, Release(opened))
}
impl Gate {
    fn hold(&self) {
        self.1.send(()).unwrap();
        let _guard = self
            .0
            .1
            .wait_while(self.0.0.lock().unwrap(), |open| !*open)
            .unwrap();
    }
}
struct HeldIo {
    gate: Gate,
    entered: bool,
}
impl DiskIo for HeldIo {
    fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> std::io::Result<()> {
        if !self.entered && point == IoFaultPoint::PayloadWrite && phase == IoPhase::Before {
            self.entered = true;
            self.gate.hold();
        }
        Ok(())
    }
}
#[derive(Clone, Copy)]
enum Fault {
    None,
    Partial,
    WrongTicket,
    Panic,
}
type DiskCalls = Arc<Mutex<Vec<(thread::ThreadId, Vec<ChunkSave>)>>>;

struct DelegatedDisk {
    disk: DiskStore,
    fault: Fault,
    calls: DiskCalls,
}
impl DiskBackend for DelegatedDisk {
    fn write(&mut self, ticket: SaveTicket, request: SaveRequest) -> SaveCompletion {
        let raw = request
            .snapshots
            .iter()
            .map(|s| {
                let SaveValue::Chunk(save) = &s.value else {
                    panic!("backend owner must receive expanded chunks")
                };
                save.clone()
            })
            .collect::<Vec<_>>();
        self.calls
            .lock()
            .unwrap()
            .push((thread::current().id(), raw));
        let fault = std::mem::replace(&mut self.fault, Fault::None);
        if matches!(fault, Fault::Panic) {
            panic!("explicit owner write panic");
        }
        if matches!(fault, Fault::Partial) {
            let submitted = request
                .snapshots
                .iter()
                .map(|s| (s.key.clone(), s.revision))
                .collect();
            let mut done = self.disk.write(
                ticket,
                SaveRequest {
                    snapshots: vec![request.snapshots[0].clone()],
                },
            );
            done.snapshots = request.snapshots;
            done.submitted = submitted;
            done.error = Some(ServerError::Io {
                operation: Operation::Replace,
                kind: std::io::ErrorKind::Other,
            });
            return done;
        }
        let mut done = self.disk.write(ticket, request);
        if matches!(fault, Fault::WrongTicket) {
            done.ticket = SaveTicket::try_from_raw(ticket.get() + 1).unwrap();
        }
        done
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
fn held_real_io_preserves_lazy_ownership_and_reuses_one_backend_thread() {
    let root = Root::new();
    let mut initial = DiskStore::open(&root.0, options()).unwrap();
    initial.close().unwrap();
    let (hold, entered, release) = gate();
    let disk = DiskStore::with_io(
        &root.0,
        options(),
        Box::new(move || {
            Box::new(HeldIo {
                gate: hold.clone(),
                entered: false,
            })
        }),
    )
    .unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let observed_calls = calls.clone();
    let mut state = authority();
    mutate(&mut state, 4, true);
    let old = capture(&state);
    let old_value = view(&old).materialize();
    state.remember_dirty(old.clone()).unwrap();
    let selected = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(selected, vec![old.clone()]);
    let mut store = StoreMailbox::try_new_background(
        limits(),
        DelegatedDisk {
            disk,
            fault: Fault::None,
            calls,
        },
    )
    .unwrap();
    let ticket = store
        .submit(SaveRequest {
            snapshots: selected,
        })
        .unwrap();
    store.drive_workers();
    let held = entered.recv_timeout(BOUND);
    // Record observations while held; release before assertions, retries or join.
    let pending = StoreHandle::poll(&mut store, ticket);
    let occupancy = store.occupancy();
    let tick = store.poll_tick(1, SaveBudget::default(), &mut state);
    mutate(&mut state, 5, false);
    let new = capture(&state);
    let new_value = view(&new).materialize();
    let close = store.close(Deadline::after(Instant::now(), Duration::from_millis(20)).unwrap());
    release.open();
    assert!(held.is_ok());
    assert!(tick.is_ok());
    assert!(matches!(pending, SavePoll::Pending));
    assert_eq!(occupancy.chunks, 1);
    assert!(occupancy.encoded_bytes <= 4_194_304);
    assert!(matches!(close, Err(ServerError::Timeout { .. })));
    let done = wait(&mut store, ticket);
    assert_eq!(done.snapshots, vec![old.clone()]);
    assert!(done.error.is_none());
    let ack = state.apply_completion(done);
    assert_eq!(ack.acked, 1);
    assert!(ack.retry.is_empty());
    assert_eq!(view(&old).materialize(), old_value);
    assert_ne!(view(&old), view(&new));
    // Closing freezes admission, so complete this owner and start the next lifecycle.
    assert!(matches!(
        store.close(deadline()),
        Err(ServerError::Timeout {
            operation: Operation::Close
        })
    ));
    store.close(deadline()).unwrap();
    reopen(&root, &old_value);
    let disk = DiskStore::open(&root.0, options()).unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let mut store = StoreMailbox::try_new_background(
        limits(),
        DelegatedDisk {
            disk,
            fault: Fault::None,
            calls: calls.clone(),
        },
    )
    .unwrap();
    for _ in 0..2 {
        let ticket = store
            .submit(SaveRequest {
                snapshots: vec![new.clone()],
            })
            .unwrap();
        let done = wait(&mut store, ticket);
        assert_eq!(done.snapshots, vec![new.clone()]);
        assert!(done.error.is_none());
    }
    store.close(deadline()).unwrap();
    reopen(&root, &new_value);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, calls[1].0);
    assert_ne!(calls[0].0, thread::current().id());
    assert_eq!(calls[0].1, vec![new_value]);
    assert_eq!(observed_calls.lock().unwrap()[0].1, vec![old_value]);
}
#[test]
fn failure_retry_keeps_exact_capture_and_newer_authority_target() {
    for fault in [Fault::Partial, Fault::WrongTicket, Fault::Panic] {
        let root = Root::new();
        let mut state = authority();
        mutate(&mut state, 4, true);
        let old = capture(&state);
        let other_key = ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        };
        let other = authority_at(other_key)
            .capture_chunk_snapshot(other_key, SaveUrgency::Autosave)
            .unwrap();
        state.remember_dirty(old.clone()).unwrap();
        state.remember_dirty(other.clone()).unwrap();
        let selected = state.select(SaveMode::All, SaveBudget::default());
        assert_eq!(selected, vec![old.clone(), other.clone()]);
        let disk = DiskStore::open(&root.0, options()).unwrap();
        let calls = Arc::new(Mutex::new(vec![]));
        let mut store = StoreMailbox::try_new_background(
            limits(),
            DelegatedDisk {
                disk,
                fault,
                calls: calls.clone(),
            },
        )
        .unwrap();
        let ticket = store
            .submit(SaveRequest {
                snapshots: selected.clone(),
            })
            .unwrap();
        mutate(&mut state, 5, false);
        let newer = capture(&state);
        let done = wait(&mut store, ticket);
        assert!(done.error.is_some());
        assert_eq!(done.snapshots[0], old);
        assert_eq!(done.snapshots, selected);
        let first_ack = state.apply_completion(done);
        let first_count = first_ack.acked;
        assert_eq!(first_count, usize::from(matches!(fault, Fault::Partial)));
        let retry = first_ack.retry;
        assert_eq!(
            retry,
            if matches!(fault, Fault::Partial) {
                vec![other.clone()]
            } else {
                selected
            }
        );
        let ticket = store
            .submit(SaveRequest {
                snapshots: retry.clone(),
            })
            .unwrap();
        let done = wait(&mut store, ticket);
        assert_eq!(done.snapshots, retry);
        assert!(done.error.is_none());
        let ack = state.apply_completion(done);
        assert_eq!(ack.acked, 2 - first_count);
        assert!(ack.retry.is_empty());
        assert_eq!(state.save_stats().in_flight, 0);
        assert_eq!(
            view(&capture(&state)).materialize(),
            view(&newer).materialize()
        );
        store.close(deadline()).unwrap();
        reopen(&root, &view(&old).materialize());
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, calls[1].0);
    }
}
#[test]
fn chunk_lane_refusal_returns_original_captures_and_outer_tampering_refuses() {
    let root = Root::new();
    let state = authority();
    let target = capture(&state);
    let disk = DiskStore::open(&root.0, options()).unwrap();
    let mut store = StoreMailbox::try_new(limits(), disk).unwrap();
    let oversized = SaveRequest {
        snapshots: vec![target.clone(); 9],
    };
    assert_eq!(
        store.submit(oversized.clone()).unwrap_err().request,
        oversized
    );
    let request = SaveRequest {
        snapshots: vec![target.clone(); 3],
    };
    let ticket = store.submit(request.clone()).unwrap();
    assert_eq!(store.occupancy().chunks, 3);
    assert!(store.occupancy().encoded_bytes <= 4_194_304);
    let refused = store
        .submit(SaveRequest {
            snapshots: vec![target.clone()],
        })
        .unwrap_err();
    assert_eq!(refused.request.snapshots, vec![target.clone()]);
    store
        .poll_tick(1, SaveBudget::default(), &mut authority())
        .unwrap();
    let done = wait(&mut store, ticket);
    assert_eq!(done.snapshots, request.snapshots);
    assert!(done.error.is_none());
    let mut invalid = target.clone();
    invalid.revision += 1;
    let error = store
        .submit(SaveRequest {
            snapshots: vec![invalid.clone()],
        })
        .unwrap_err();
    assert_eq!(error.request.snapshots, vec![invalid]);
    assert_eq!(store.occupancy(), Default::default());
    store.close(deadline()).unwrap();
}
