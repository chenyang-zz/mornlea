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
            "mornlea-chunk-encoding-{}-{}",
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

use mornlea_domain::{Event, PlayerId};
use mornlea_protocol::{LoginStart, ProtocolCodec, ServerPacket, admit_login, read_frame_ref};
use mornlea_server::core::chunk_driver::ChunkDriver;
use mornlea_server::core::chunk_encoding::{
    ChunkEncodePoll, ChunkEncodePort, EncodedChunkSnapshot,
};
use mornlea_server::core::encoding_worker::ChunkEncodingPool;
use mornlea_server::core::publication::EnqueueOutcome;
use mornlea_server::core::world::ChunkSaveView;
use mornlea_server::transport::memory::MemoryTransport;

// Cleanup precedes the disposable root even during an assertion unwind.
struct Fixture {
    encoding: ChunkEncodingPool,
    generation: GenerationPool,
    store: AutosaveScheduler<DiskStore>,
    root: Root,
}
impl Fixture {
    fn new() -> Self {
        let root = Root::new();
        let store = store(&root);
        Self {
            encoding: ChunkEncodingPool::try_new(1).unwrap(),
            generation: GenerationPool::try_new(42, false, 1).unwrap(),
            store,
            root,
        }
    }
    fn drain(&mut self, driver: &mut ChunkDriver, state: &mut AuthorityState) {
        let until = deadline();
        loop {
            self.store.drive_workers();
            let r = driver.poll(state, &mut self.store, &mut self.generation);
            if r.retained == 0 {
                assert!(r.first_error.is_none());
                return;
            }
            assert!(!until.expired(Instant::now()));
            thread::yield_now();
        }
    }
    fn encode(&mut self, token: ChunkSaveView) -> EncodedChunkSnapshot {
        let id = self.encoding.start_encode(token).unwrap();
        let until = deadline();
        loop {
            match self.encoding.poll_encode(id) {
                ChunkEncodePoll::Ready(v) => return v,
                ChunkEncodePoll::Failed(e) => panic!("actual encoding error: {e:?}"),
                ChunkEncodePoll::Pending => assert!(!until.expired(Instant::now())),
            }
            thread::yield_now();
        }
    }
    fn close(&mut self) {
        self.encoding.close(deadline()).unwrap();
        self.generation.close(deadline()).unwrap();
        self.store.close(deadline()).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let encoded = self.encoding.close(deadline());
        let generated = self.generation.close(deadline());
        let stored = self.store.close(deadline());
        // A failed cleanup preserves its disposable tree rather than deleting
        // files while a timed-out owner could still hold the world lease.
        self.root.1 = encoded.is_ok() && generated.is_ok() && stored.is_ok();
    }
}
fn capture(state: &AuthorityState) -> ChunkSaveView {
    let value = state
        .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
        .unwrap()
        .value;
    let SaveValue::ChunkView(v) = value else {
        panic!("actual lazy capture")
    };
    v
}
fn actual_frame(output: &EncodedChunkSnapshot) -> mornlea_protocol::ChunkSnapshot {
    let expected = ServerPacket::try_from(Event::ChunkSnapshot(
        output.capture().network_snapshot().unwrap().0,
    ))
    .unwrap();
    assert_eq!(output.frame().packet_key(), expected.key());
    let frame = read_frame_ref(output.frame().as_bytes()).unwrap();
    assert_eq!(frame.consumed, output.frame().byte_len());
    let decoded = ProtocolCodec::new()
        .unwrap()
        .decode_snapshot(frame.payload)
        .unwrap();
    assert_eq!(ServerPacket::ChunkSnapshot(decoded.clone()), expected);
    decoded
}
fn live() -> AuthorityState {
    let mut state = authority();
    state.enable_live_chunks().unwrap();
    state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    state
}
// Direct protocol admission creates Active sessions. This is explicit session
// setup, not a transport LoginDriver handshake or peer acknowledgment proof.
fn direct_session(state: &mut AuthorityState, tag: u8) -> SessionKey {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let player = PlayerId::try_from_bytes(bytes).unwrap();
    let start = LoginStart::new(player, format!("P{tag}"), 8).unwrap();
    let admitted =
        admit_login(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap()).unwrap();
    state.admit(admitted, TransportKind::Memory).unwrap()
}
#[test]
fn actual_saved_revision_nine_load_capture_cpu_fifo_memory_and_disk_reopen() {
    let mut fixture = Fixture::new();
    let stored = chunk();
    save_at(&mut fixture.store, key(0), stored.clone(), 9);
    let mut state = live();
    let mut driver = ChunkDriver::new();
    driver
        .start_load(&mut state, &mut fixture.store, key(0), deadline())
        .unwrap();
    fixture.drain(&mut driver, &mut state);
    state.advance_tick(TickBudget::full()).unwrap();
    let facts = state.live_chunk_facts(key(0)).unwrap();
    assert_eq!(facts.phase, LiveChunkPhase::Ready);
    assert_eq!((facts.revision, facts.persisted_revision), (9, 9));
    let token = capture(&state);
    let encoded = fixture.encode(token.clone());
    assert_eq!(encoded.capture(), &token);
    assert_eq!(
        (token.key(), token.generation(), token.revision()),
        (key(0), facts.generation, 9)
    );
    assert_eq!(encoded.section_payload_bytes(), 48);
    assert_eq!(actual_frame(&encoded).revision, 9);
    let frame = encoded.into_frame();
    let ptr = frame.as_bytes().as_ptr();
    let sessions = [direct_session(&mut state, 1), direct_session(&mut state, 2)];
    for session in sessions {
        assert_eq!(
            state.enqueue_prepared(session, frame.clone()).unwrap(),
            EnqueueOutcome::Queued
        );
    }
    for session in sessions {
        let received =
            MemoryTransport::drain_prepared_session(&mut state, session, 8, usize::MAX).unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].as_bytes().as_ptr(), ptr);
        assert_eq!(received[0].packet_key(), frame.packet_key());
        assert_eq!(received[0].as_bytes(), frame.as_bytes());
        assert!(
            MemoryTransport::drain_prepared_session(&mut state, session, 8, usize::MAX)
                .unwrap()
                .is_empty()
        );
    }
    fixture.close();
    let mut reopened = DiskStore::open(&fixture.root.0, options()).unwrap();
    let loaded = reopened.load(SaveKey::Chunk(key(0))).unwrap();
    reopened.close().unwrap();
    assert!(matches!(loaded,LoadedValue::Chunk(v) if v.revision==9 && v.chunk==stored));
}
#[test]
fn actual_missing_native_generation_then_deliberate_unloading_capture_encodes() {
    let mut fixture = Fixture::new();
    let mut state = live();
    let mut driver = ChunkDriver::new();
    driver
        .start_load(&mut state, &mut fixture.store, key(0), deadline())
        .unwrap();
    fixture.drain(&mut driver, &mut state);
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().phase,
        LiveChunkPhase::NeedsGeneration
    );
    driver
        .start_generation(&mut state, &mut fixture.generation, key(0))
        .unwrap();
    fixture.drain(&mut driver, &mut state);
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().phase,
        LiveChunkPhase::Ready
    );
    let token = capture(&state);
    let output = fixture.encode(token.clone());
    assert_eq!(output.capture(), &token);
    let generated = actual_frame(&output);
    state.replace_chunk_wants(BTreeSet::new()).unwrap();
    assert_eq!(
        state.live_chunk_facts(key(0)).unwrap().phase,
        LiveChunkPhase::Unloading
    );
    // Encoding a deliberately supplied save capture does not grant publication
    // eligibility. The source publisher must qualify wanted Ready captures.
    let unloading = capture(&state);
    let encoded = fixture.encode(unloading.clone());
    assert_eq!(encoded.capture(), &unloading);
    assert_eq!(actual_frame(&encoded), generated);
    fixture.close();
    let mut reopened = DiskStore::open(&fixture.root.0, options()).unwrap();
    let loaded = reopened.load(SaveKey::Chunk(key(0)));
    reopened.close().unwrap();
    assert!(matches!(
        loaded,
        Err(ServerError::Io {
            operation: Operation::Load,
            kind: std::io::ErrorKind::NotFound
        })
    ));
}
