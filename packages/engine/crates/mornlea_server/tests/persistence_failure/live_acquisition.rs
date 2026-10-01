//! Actual providers followed through the authoritative Acquire row.
use mornlea_domain::{ChunkPos, Dimension};
use mornlea_server::store::{
    disk::{DiskOptions, DiskStore},
    mailbox::StoreMailbox,
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
            "mornlea-live-acquisition-{}-{}",
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
fn store(root: &Root) -> StoreMailbox<DiskStore> {
    StoreMailbox::try_new_background(
        StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
        DiskStore::open(&root.0, options()).unwrap(),
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
fn save(store: &mut StoreMailbox<DiskStore>, key: ChunkKey, chunk: Chunk) {
    save_at(store, key, chunk, 9)
}
fn save_at(store: &mut StoreMailbox<DiskStore>, key: ChunkKey, chunk: Chunk, revision: u64) {
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
fn load(store: &mut StoreMailbox<DiskStore>, t: ChunkRequestId) -> ChunkLoadPoll {
    let until = deadline();
    loop {
        store.drive_workers();
        let v = store.poll_chunk(t);
        if !matches!(v, ChunkLoadPoll::Pending) {
            return v;
        }
        assert!(!until.expired(Instant::now()));
        thread::yield_now();
    }
}

#[test]
fn legacy_prepared_completion_missing_installation_witness() {
    let root = Root::new();
    let mut store = store(&root);
    let expected = slotted_chunk();
    save(&mut store, key(0), expected.clone());
    let t = store.start_chunk(key(0), 1, deadline()).unwrap();
    let loaded = load(&mut store, t);
    store.close(deadline()).unwrap();
    let ChunkLoadPoll::Loaded(Some(prepared)) = loaded else {
        panic!("actual disk prepared load")
    };
    let mut state = authority();
    let result = ChunkResult {
        key: key(0),
        generation: prepared.generation(),
        request: t,
        result: Ok(expected),
    };
    state.admit_chunk(result).unwrap();
    let drained = state.drain_chunks(16);
    let mut wants = mornlea_server::rules::world_acquisition::ChunkWants::new();
    wants.want(key(0), 1, t);
    assert_eq!(
        mornlea_server::rules::world_acquisition::apply_drained(&mut wants, &drained).applied,
        1
    );
    state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        state
            .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
            .is_none(),
        "the legacy fixture ledger retains its original contract"
    );
}

use mornlea_server::core::acquisition::{AcquiredChunkEvent, LiveChunkPhase};
#[test]
fn actual_saved_background_load_installs_only_at_acquire() {
    let root = Root::new();
    let mut store = store(&root);
    let expected = slotted_chunk();
    save(&mut store, key(0), expected.clone());
    let mut state = authority();
    state.enable_live_chunks().unwrap();
    state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    let reservation = state.reserve_chunk_load(key(0)).unwrap();
    let t = store
        .start_chunk(reservation.key(), reservation.generation(), deadline())
        .unwrap();
    state.bind_chunk_load(reservation, t).unwrap();
    let loaded = load(&mut store, t);
    store.close(deadline()).unwrap();
    let ChunkLoadPoll::Loaded(Some(prepared)) = loaded else {
        panic!("actual saved prepared")
    };
    state
        .offer_acquired(AcquiredChunkEvent::Load {
            key: key(0),
            generation: reservation.generation(),
            request: t,
            result: Ok(Some(prepared)),
        })
        .unwrap();
    assert!(
        state
            .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
            .is_none()
    );
    state.advance_tick(TickBudget::full()).unwrap();
    let facts = state.live_chunk_facts(key(0)).unwrap();
    assert_eq!(
        (
            facts.phase,
            facts.generation,
            facts.revision,
            facts.persisted_revision,
            facts.needs_rewrite,
            facts.recovered
        ),
        (LiveChunkPhase::Ready, 1, 9, 9, false, false)
    );
    let snapshot = state
        .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
        .unwrap();
    let SaveValue::ChunkView(v) = snapshot.value else {
        panic!("immutable capture")
    };
    assert_eq!(v.materialize().chunk, expected);
}
#[test]
fn actual_missing_disk_then_native_generation_installs() {
    use mornlea_server::core::{generation::ChunkGenerator, generation_worker::GenerationPool};
    let root = Root::new();
    let mut store = store(&root);
    let mut state = authority();
    state.enable_live_chunks().unwrap();
    state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    let load_reservation = state.reserve_chunk_load(key(0)).unwrap();
    let t = store
        .start_chunk(key(0), load_reservation.generation(), deadline())
        .unwrap();
    state.bind_chunk_load(load_reservation, t).unwrap();
    let loaded = load(&mut store, t);
    store.close(deadline()).unwrap();
    assert!(matches!(loaded, ChunkLoadPoll::Loaded(None)));
    state
        .offer_acquired(AcquiredChunkEvent::Load {
            key: key(0),
            generation: load_reservation.generation(),
            request: t,
            result: Ok(None),
        })
        .unwrap();
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().phase,
        LiveChunkPhase::NeedsGeneration
    );
    let r = state.reserve_chunk_generation(key(0)).unwrap();
    let mut pool = GenerationPool::try_new(42, false, 1).unwrap();
    let warm: Vec<_> = (0..7)
        .map(|_| pool.start_generation(key(1), 1).unwrap())
        .collect();
    let t = pool.start_generation(r.key(), r.generation()).unwrap();
    state.bind_chunk_generation(r, t).unwrap();
    let pending = pool.poll_generation(t);
    assert!(matches!(pending, GenerationPoll::Pending));
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().phase,
        LiveChunkPhase::Generating
    );
    assert!(
        state
            .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
            .is_none()
    );
    for ticket in warm {
        let until = deadline();
        loop {
            match pool.poll_generation(ticket) {
                GenerationPoll::Ready(_) => break,
                GenerationPoll::Failed(e) => panic!("native warm job {e:?}"),
                GenerationPoll::Pending => {
                    assert!(!until.expired(Instant::now()));
                    thread::yield_now();
                }
            }
        }
    }

    let until = deadline();
    let prepared = loop {
        match pool.poll_generation(t) {
            GenerationPoll::Ready(v) => break v,
            GenerationPoll::Failed(e) => panic!("native generation {e:?}"),
            GenerationPoll::Pending => {
                assert!(!until.expired(Instant::now()));
                thread::yield_now();
            }
        }
    };
    pool.close(deadline()).unwrap();
    state
        .offer_acquired(AcquiredChunkEvent::Generated {
            key: key(0),
            generation: r.generation(),
            request: t,
            result: Ok(prepared),
        })
        .unwrap();
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().phase,
        LiveChunkPhase::Generating
    );
    state.advance_tick(TickBudget::full()).unwrap();
    let facts = state.live_chunk_facts(key(0)).unwrap();
    assert_eq!(
        (facts.phase, facts.revision, facts.persisted_revision),
        (LiveChunkPhase::Ready, 1, 0)
    );
    let expected = ChunkGenerator::try_new(42, false)
        .unwrap()
        .generate(key(0))
        .unwrap();
    let SaveValue::ChunkView(v) = state
        .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
        .unwrap()
        .value
    else {
        panic!("capture")
    };
    assert_eq!(v.materialize().chunk, expected);
}

fn request(id: u64) -> ChunkRequestId {
    ChunkRequestId::try_new(id).unwrap()
}
fn prepared(key: ChunkKey, generation: u64) -> mornlea_server::core::world::PreparedChunk {
    mornlea_server::core::world::PreparedChunk::try_new(
        key,
        generation,
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
fn prepared_mismatch_returns_original_body_and_typed_failure_never_generates() {
    let mut state = authority();
    state.enable_live_chunks().unwrap();
    state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    let r = state.reserve_chunk_load(key(0)).unwrap();
    state.bind_chunk_load(r, request(1)).unwrap();
    for (k, g) in [(key(1), r.generation()), (key(0), r.generation() + 1)] {
        let rejected = state
            .offer_acquired(AcquiredChunkEvent::Load {
                key: key(0),
                generation: r.generation(),
                request: request(1),
                result: Ok(Some(prepared(k, g))),
            })
            .unwrap_err();
        assert_eq!(
            rejected.error,
            ServerError::InvalidInput {
                field: "chunk_completion_identity"
            }
        );
        let AcquiredChunkEvent::Load {
            result: Ok(Some(original)),
            ..
        } = rejected.event
        else {
            panic!("whole original")
        };
        assert_eq!((original.key(), original.generation()), (k, g));
    }
    let error = ServerError::Storage {
        family: "chunk",
        kind: StorageFailure::FutureVersion,
    };
    state
        .offer_acquired(AcquiredChunkEvent::Load {
            key: key(0),
            generation: r.generation(),
            request: request(1),
            result: Err(error),
        })
        .unwrap();
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(state.live_chunk_error(key(0)), Some(&error));
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().phase,
        LiveChunkPhase::Failed
    );
    assert!(state.reserve_chunk_generation(key(0)).is_err());
}
#[test]
fn enabled_mode_refuses_raw_results_and_closing_accepts_final_prepared_batch() {
    let mut state = authority();
    assert!(state.replace_chunk_wants(BTreeSet::new()).is_err());
    state.enable_live_chunks().unwrap();
    state.enable_live_chunks().unwrap();
    state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    let r = state.reserve_chunk_load(key(0)).unwrap();
    state.bind_chunk_load(r, request(1)).unwrap();
    state.cancel_chunk(request(1));
    let raw = ChunkResult {
        key: key(0),
        generation: 1,
        request: request(1),
        result: Ok(chunk()),
    };
    assert_eq!(state.admit_chunk(raw.clone()), Err(raw));
    assert_eq!(state.chunk_discard_counts(), (0, 0));
    state.begin_close();
    assert!(state.reserve_chunk_load(key(1)).is_err());
    assert!(state.replace_chunk_wants(BTreeSet::new()).is_err());
    state
        .offer_acquired(AcquiredChunkEvent::Load {
            key: key(0),
            generation: r.generation(),
            request: request(1),
            result: Ok(Some(prepared(key(0), r.generation()))),
        })
        .unwrap();
    state.mark_closed();
    let rejected = state
        .offer_acquired(AcquiredChunkEvent::Load {
            key: key(0),
            generation: 1,
            request: request(1),
            result: Ok(None),
        })
        .unwrap_err();
    assert_eq!(
        rejected.error,
        ServerError::InvalidState {
            phase: ServerPhase::Closed
        }
    );
}

use std::sync::{Arc, Condvar, Mutex, mpsc};
struct HeldDisk {
    disk: DiskStore,
    entered: mpsc::SyncSender<()>,
    release: Arc<(Mutex<bool>, Condvar)>,
    calls: Arc<AtomicU64>,
}
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
impl DiskBackend for HeldDisk {
    fn load(&mut self, key: SaveKey) -> Result<LoadedValue, ServerError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.entered.send(()).unwrap();
        let (lock, cv) = &*self.release;
        drop(cv.wait_while(lock.lock().unwrap(), |v| !*v).unwrap());
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
fn held_real_disk_read_does_not_block_tick_and_forgotten_results_are_retained_or_removed() {
    for saved in [true, false] {
        let root = Root::new();
        if saved {
            let mut s = store(&root);
            save(&mut s, key(0), chunk());
            s.close(deadline()).unwrap();
        }
        let (tx, entered) = mpsc::sync_channel(1);
        let release = Release(Arc::new((Mutex::new(false), Condvar::new())));
        let calls = Arc::new(AtomicU64::new(0));
        let mut s = StoreMailbox::try_new_background(
            StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
            HeldDisk {
                disk: DiskStore::open(&root.0, options()).unwrap(),
                entered: tx,
                release: release.0.clone(),
                calls: calls.clone(),
            },
        )
        .unwrap();
        let mut state = authority();
        state.enable_live_chunks().unwrap();
        state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
        let r = state.reserve_chunk_load(key(0)).unwrap();
        let t = s.start_chunk(key(0), r.generation(), deadline()).unwrap();
        state.bind_chunk_load(r, t).unwrap();
        s.drive_workers();
        let entry = entered.recv_timeout(Duration::from_secs(5));
        state.replace_chunk_wants(BTreeSet::new()).unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        let driver = thread::spawn(move || {
            let tick = state.advance_tick(TickBudget::full());
            tx.send((state, tick)).unwrap();
        });
        let before = rx.recv_timeout(Duration::from_secs(2));
        let nonblocking = before.is_ok();
        release.open();
        let (mut state, tick) = before
            .or_else(|_| rx.recv_timeout(Duration::from_secs(5)))
            .unwrap();
        let joined = driver.join();
        let until = deadline();
        let result = loop {
            s.drive_workers();
            match s.poll_chunk(t) {
                ChunkLoadPoll::Pending => {
                    assert!(!until.expired(Instant::now()));
                    thread::yield_now();
                }
                ChunkLoadPoll::Loaded(v) => break Ok(v),
                ChunkLoadPoll::Failed(e) => break Err(e),
            }
        };
        s.close(deadline()).unwrap();
        assert!(entry.is_ok());
        assert!(nonblocking);
        assert!(joined.is_ok());
        tick.unwrap();
        assert!(
            state
                .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
                .is_none()
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        state
            .offer_acquired(AcquiredChunkEvent::Load {
                key: key(0),
                generation: r.generation(),
                request: t,
                result,
            })
            .unwrap();
        state.advance_tick(TickBudget::full()).unwrap();
        if saved {
            let facts = state.live_chunk_facts(key(0)).unwrap();
            assert_eq!(
                (
                    facts.phase,
                    facts.wanted,
                    facts.generation,
                    facts.revision,
                    facts.persisted_revision
                ),
                (LiveChunkPhase::Unloading, false, r.generation(), 9, 9)
            );
            let SaveValue::ChunkView(v) = state
                .capture_chunk_snapshot(key(0), SaveUrgency::Unload)
                .unwrap()
                .value
            else {
                panic!("retained")
            };
            assert_eq!(v.materialize().chunk, chunk());
            state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
            let ready = state.live_chunk_facts(key(0)).unwrap();
            assert_eq!(
                (ready.phase, ready.generation, ready.revision),
                (LiveChunkPhase::Ready, facts.generation, facts.revision)
            );
        } else {
            assert!(state.live_chunk_facts(key(0)).is_none());
            assert!(state.reserve_chunk_generation(key(0)).is_err());
        }
    }
}

#[test]
fn actual_corrupt_and_future_chunk_files_preserve_their_typed_errors() {
    use mornlea_storage::{
        BANK_A_START_SECTOR, BANK_B_START_SECTOR, BANK_SIZE, RegionKey, SECTOR_SIZE, crc32c,
        decode_region_bank, encode_region_bank,
    };
    for failure in [StorageFailure::Corrupt, StorageFailure::FutureVersion] {
        let root = Root::new();
        let mut s = store(&root);
        save(&mut s, key(0), chunk());
        s.close(deadline()).unwrap();
        let path = root.0.join("dimensions/0/regions/r.0.0.region");
        let mut bytes = fs::read(&path).unwrap();
        let rk = RegionKey {
            dimension: 0,
            x: 0,
            z: 0,
        };
        for sector in [BANK_A_START_SECTOR, BANK_B_START_SECTOR] {
            let bank_at = sector as usize * SECTOR_SIZE as usize;
            let mut bank =
                decode_region_bank(rk, &bytes[bank_at..bank_at + BANK_SIZE], bytes.len() as i64)
                    .unwrap();
            let e = &mut bank.entries[0];
            if e.offset_sector == 0 {
                continue;
            }
            let payload_at = e.offset_sector as usize * SECTOR_SIZE as usize;
            let end = payload_at + e.payload_length as usize;
            if failure == StorageFailure::Corrupt {
                bytes[payload_at] ^= 0xff;
            } else {
                bytes[payload_at + 8..payload_at + 12].copy_from_slice(&u32::MAX.to_le_bytes());
            }
            e.payload_crc32c = crc32c(&bytes[payload_at..end]);
            bytes[bank_at..bank_at + BANK_SIZE]
                .copy_from_slice(&encode_region_bank(rk, &bank).unwrap());
        }
        fs::write(&path, &bytes).unwrap();
        let mut s = store(&root);
        let mut state = authority();
        state.enable_live_chunks().unwrap();
        state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
        let r = state.reserve_chunk_load(key(0)).unwrap();
        let t = s.start_chunk(key(0), r.generation(), deadline()).unwrap();
        state.bind_chunk_load(r, t).unwrap();
        let outcome = load(&mut s, t);
        s.close(deadline()).unwrap();
        let ChunkLoadPoll::Failed(error) = outcome else {
            panic!("actual malformed file")
        };
        assert_eq!(
            error,
            ServerError::Storage {
                family: if failure == StorageFailure::Corrupt {
                    "region"
                } else {
                    "chunk"
                },
                kind: failure
            }
        );
        state
            .offer_acquired(AcquiredChunkEvent::Load {
                key: key(0),
                generation: r.generation(),
                request: t,
                result: Err(error),
            })
            .unwrap();
        state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(state.live_chunk_error(key(0)), Some(&error));
        assert_eq!(
            state.live_chunk_facts(key(0)).unwrap().phase,
            LiveChunkPhase::Failed
        );
        assert!(state.reserve_chunk_generation(key(0)).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

fn slotted_chunk() -> Chunk {
    use mornlea_domain::{BlockPos, FiniteVec3};
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ctx.preload_ready_chunk(
        mornlea_server::core::world::ReadyChunk::try_new(key(0), 1, 8, chunk()).unwrap(),
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

#[test]
fn actual_recovered_prepared_load_retains_rewrite_facts_when_forgotten() {
    use mornlea_storage::{
        BANK_A_START_SECTOR, BANK_B_START_SECTOR, BANK_SIZE, RegionKey, SECTOR_SIZE,
        decode_region_bank,
    };
    let root = Root::new();
    let mut s = store(&root);
    let expected = slotted_chunk();
    save_at(&mut s, key(0), expected.clone(), 7);
    let mut newer = chunk();
    newer.sections[0].single = 3;
    save_at(&mut s, key(0), newer, 8);
    s.close(deadline()).unwrap();
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
    let mut s = store(&root);
    let mut a = authority();
    a.enable_live_chunks().unwrap();
    a.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    let r = a.reserve_chunk_load(key(0)).unwrap();
    let t = s.start_chunk(key(0), r.generation(), deadline()).unwrap();
    a.bind_chunk_load(r, t).unwrap();
    a.replace_chunk_wants(BTreeSet::new()).unwrap();
    let outcome = load(&mut s, t);
    s.close(deadline()).unwrap();
    let ChunkLoadPoll::Loaded(Some(prepared)) = outcome else {
        panic!("actual disk recovery")
    };
    assert_eq!(
        (
            prepared.revision(),
            prepared.persisted_revision(),
            prepared.needs_rewrite(),
            prepared.recovered()
        ),
        (9, 7, true, true)
    );
    a.offer_acquired(AcquiredChunkEvent::Load {
        key: key(0),
        generation: r.generation(),
        request: t,
        result: Ok(Some(prepared)),
    })
    .unwrap();
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
        (LiveChunkPhase::Unloading, r.generation(), 9, 7, true, true)
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
