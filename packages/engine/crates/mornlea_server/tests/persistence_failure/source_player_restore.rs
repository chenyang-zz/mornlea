//! Actual background disk, Memory login and Acquire initial restoration recipes.
//! Manual wants qualify the caller without accepting a source subscription producer.
use mornlea_domain::{ChunkPos, Dimension, Identities, PlayerId};
use mornlea_protocol::{
    ClientHello, ClientPacket, LoginStart, PlayerInput, ProtocolCodec, SelectHotbar, ServerPacket,
    State, encode_uvarint, read_frame_ref,
};
use mornlea_server::core::{
    acquisition::LiveChunkPhase, chunk_driver::ChunkDriver, generation_worker::GenerationPool,
};
use mornlea_server::store::{
    disk::{DiskOptions, DiskStore},
    mailbox::StoreMailbox,
    scheduler::{AutosaveScheduler, SchedulerConfig},
};
use mornlea_server::transport::{
    common::TransportAuthority, live::LoginDriver, memory::MemoryTransport,
};
use mornlea_server::{contracts::*, state::AuthorityState};
use mornlea_storage::{
    Chunk, ChunkSave, ContainerSnapshot, Inventory, ItemStack, Metadata, MetadataChunkPos,
    PlayerLocation, PlayerSave, StorageKind, player_encoded_len,
};
use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

const BOUND: Duration = Duration::from_secs(5);
static ROOTS: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf, bool);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mornlea-source-player-restore-{}-{}",
            std::process::id(),
            ROOTS.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path, true)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        if self.1 {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn deadline() -> Deadline {
    Deadline::after(Instant::now(), BOUND).unwrap()
}
fn player() -> PlayerId {
    let mut bytes = [0; 16];
    bytes[0] = 1;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    PlayerId::try_from_bytes(bytes).unwrap()
}
fn key(dimension: Dimension, x: i32, z: i32) -> ChunkKey {
    ChunkKey {
        dimension,
        pos: ChunkPos::new(x, z),
    }
}
fn options(dimension: Dimension, anchor: ChunkPos) -> DiskOptions {
    DiskOptions {
        region_handle_cap: 1,
        create: Metadata {
            format_version: mornlea_storage::METADATA_CURRENT_VERSION,
            seed: 42,
            spawn_dimension: i32::from(dimension.get()),
            spawn_anchor: MetadataChunkPos {
                x: anchor.x(),
                z: anchor.z(),
            },
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
fn saved_player() -> PlayerSave {
    let mut armor = [ItemStack::default(); 4];
    armor[1] = ItemStack {
        item: 5,
        count: 1,
        durability: 0,
    };
    PlayerSave {
        player_id: mornlea_storage::PlayerId::from_bytes(player().bytes()),
        revision: 9,
        display_name: "Ada".into(),
        current: PlayerLocation {
            dimension: 0,
            position: [8.5, 65.0, 8.5],
        },
        yaw: 0.1,
        pitch: 0.2,
        safe: None,
        inventory: {
            let mut inv = Inventory::default();
            inv.hotbar.selected = 3;
            inv.hotbar.slots[3] = ItemStack {
                item: 1,
                count: 7,
                durability: 0,
            };
            inv
        },
        health: 15,
        hunger: 17,
        saturation_milli: 9000,
        exhaustion_milli: 250,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor,
    }
}
fn air() -> Chunk {
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
struct StepClock(Instant);
impl Clock for StepClock {
    fn monotonic(&self) -> Instant {
        self.0
    }
    fn unix_ms(&self) -> i64 {
        1000
    }
}
// Both owners must close before the world directory is reused or removed.
struct Fixture {
    wanted: BTreeSet<ChunkKey>,
    chunks: ChunkDriver,
    generation: GenerationPool,
    store: AutosaveScheduler<DiskStore>,
    root: Root,
}
impl Fixture {
    fn new(
        save: Option<PlayerSave>,
        dimension: Dimension,
        anchor: ChunkPos,
        chunks: Vec<(ChunkKey, Chunk)>,
    ) -> (Self, AuthorityState) {
        assert_eq!(Identities::current().engine_abi, 11);
        let root = Root::new();
        let mut disk = DiskStore::open(&root.0, options(dimension, anchor)).unwrap();
        let mut snapshots = Vec::new();
        if let Some(save) = save {
            snapshots.push(
                OwnedSnapshot::try_new(
                    SaveKey::Player(player()),
                    save.revision,
                    player_encoded_len(&save).unwrap(),
                    SaveUrgency::Autosave,
                    SaveValue::Player(save),
                )
                .unwrap(),
            );
        }
        for (key, chunk) in chunks {
            snapshots.push(
                OwnedSnapshot::try_new(
                    SaveKey::Chunk(key),
                    9,
                    1,
                    SaveUrgency::Autosave,
                    SaveValue::Chunk(ChunkSave {
                        key: mornlea_storage::ChunkKey {
                            dimension: i32::from(key.dimension.get()),
                            x: key.pos.x(),
                            z: key.pos.z(),
                        },
                        revision: 9,
                        chunk,
                    }),
                )
                .unwrap(),
            );
        }
        let expected: Vec<_> = snapshots
            .iter()
            .map(|s| (s.key.clone(), s.revision))
            .collect();
        let completion = disk.write(
            SaveTicket::try_from_raw(1).unwrap(),
            SaveRequest {
                snapshots: snapshots.clone(),
            },
        );
        assert_eq!(completion.snapshots, snapshots);
        assert_eq!(completion.error, None);
        assert_eq!(completion.committed.len(), expected.len());
        for key in expected {
            assert!(completion.committed.contains(&key));
        }
        let mut state = AuthorityState::try_new_with_metadata(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            disk.metadata().clone(),
        )
        .unwrap();
        state.enable_source_player_restoration(1).unwrap();
        state.enable_live_chunks().unwrap();
        let store = AutosaveScheduler::try_new(
            SchedulerConfig::default(),
            StoreMailbox::try_new_background(
                StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
                disk,
            )
            .unwrap(),
        )
        .unwrap();
        (
            Self {
                wanted: BTreeSet::new(),
                chunks: ChunkDriver::new(),
                generation: GenerationPool::try_new(42, false, 1).unwrap(),
                store,
                root,
            },
            state,
        )
    }
    fn acquire(&mut self, state: &mut AuthorityState, key: ChunkKey) -> TickPublication {
        self.wanted.insert(key);
        state.replace_chunk_wants(self.wanted.clone()).unwrap();
        self.chunks
            .start_load(state, &mut self.store, key, deadline())
            .unwrap();
        let until = Instant::now() + BOUND;
        loop {
            self.store.drive_workers();
            let report = self
                .chunks
                .poll(state, &mut self.store, &mut self.generation);
            assert_eq!(report.first_error, None);
            if report.retained == 0 {
                break;
            }
            assert!(Instant::now() < until, "actual chunk poll stalled");
            thread::yield_now();
        }
        assert_eq!(self.chunks.pending_generations(), 0);
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(
            state.live_chunk_facts(key).unwrap().phase,
            LiveChunkPhase::Ready
        );
        assert_eq!(state.live_chunk_facts(key).unwrap().persisted_revision, 9);
        publication
    }
    fn close(&mut self) {
        self.generation.close(deadline()).unwrap();
        self.store.close(deadline()).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let generated = self.generation.close(deadline());
        let stored = self.store.close(deadline());
        // Failed joins preserve the fixture tree for the still-owning process.
        self.root.1 = generated.is_ok() && stored.is_ok();
    }
}
fn decode(bytes: &[u8], state: State) -> ServerPacket {
    let frame = read_frame_ref(bytes).unwrap();
    assert_eq!(frame.consumed, bytes.len());
    ProtocolCodec::new()
        .unwrap()
        .decode_server(state, frame.packet_id, frame.payload)
        .unwrap()
}
fn receive(transport: &mut MemoryTransport, id: ConnectionId) -> Vec<u8> {
    let until = Instant::now() + BOUND;
    loop {
        let mut frames = transport.receive(id, 1, 1 << 20);
        if let Some(frame) = frames.pop() {
            return frame;
        }
        assert!(Instant::now() < until, "Memory frame delivery stalled");
        thread::yield_now();
    }
}

fn handshake(
    fixture: &mut Fixture,
    state: &mut AuthorityState,
) -> (
    LoginDriver,
    MemoryTransport,
    ConnectionId,
    SessionKey,
    StepClock,
) {
    let mut login = LoginDriver::new();
    let clock = StepClock(Instant::now());
    let mut transport = MemoryTransport::new();
    let connection = transport.connect(clock.monotonic()).unwrap();
    let hello = ClientPacket::ClientHello(
        ClientHello::decode_inbound(&encode_uvarint(Identities::current().protocol)).unwrap(),
    );
    transport.send(
        connection,
        MemoryTransport::encode_frame(&hello).unwrap(),
        &mut login.bind(state, &mut fixture.store),
        &clock,
    );
    let hello = decode(&receive(&mut transport, connection), State::Handshake);
    assert_eq!(
        hello,
        ServerPacket::ServerHello(
            mornlea_protocol::ServerHello::new(Identities::current().protocol).unwrap()
        )
    );
    transport.acknowledge(connection, 1, &mut login.bind(state, &mut fixture.store));
    let start = LoginStart::new(player(), "Ada", 8).unwrap();
    let start =
        ClientPacket::LoginStart(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap());
    transport.send(
        connection,
        MemoryTransport::encode_frame(&start).unwrap(),
        &mut login.bind(state, &mut fixture.store),
        &clock,
    );
    let until = Instant::now() + BOUND;
    let (session, success) = loop {
        fixture.store.drive_workers();
        transport.poll(
            connection,
            &mut login.bind(state, &mut fixture.store),
            &clock,
        );
        match login
            .bind(state, &mut fixture.store)
            .poll_login(LoginTicket::try_from_raw(1).unwrap())
        {
            LoginPoll::Ready { session, success } => break (session, success),
            LoginPoll::Failed { error, .. } => panic!("actual login load failed: {error:?}"),
            LoginPoll::Pending => {
                assert!(Instant::now() < until);
                thread::yield_now();
            }
        }
    };
    assert_eq!(
        state.session(session).unwrap().phase,
        SessionPhase::Prepared
    );
    transport.poll(
        connection,
        &mut login.bind(state, &mut fixture.store),
        &clock,
    );
    assert_eq!(
        decode(&receive(&mut transport, connection), State::Login),
        success
    );
    assert!(matches!(success, ServerPacket::LoginSuccess(_)));
    transport.acknowledge(connection, 1, &mut login.bind(state, &mut fixture.store));
    assert_eq!(login.pending(), 0);
    let active = state.session(session).unwrap();
    assert_eq!(active.phase, SessionPhase::Active);
    assert_eq!(active.player_id, player());

    (login, transport, connection, session, clock)
}
fn floor() -> Chunk {
    let mut chunk = air();
    chunk.sections[7].single = 1;
    chunk
}
fn solid() -> Chunk {
    let mut chunk = air();
    for section in &mut chunk.sections {
        section.single = 1;
    }
    chunk
}
fn local(publication: &TickPublication) -> &mornlea_domain::PlayerState {
    publication
        .events
        .iter()
        .find_map(|event| match event.event() {
            mornlea_domain::Event::PlayerState(state) => Some(state),
            _ => None,
        })
        .expect("actual local player state")
}
fn pending(state: &AuthorityState, session: SessionKey, dimension: Dimension, position: [f32; 3]) {
    let view = state.settled_read().unwrap();
    let actor = view.actor(ActorKey::Player(session)).unwrap();
    assert_eq!(actor.lifecycle, ActorLifecycle::Pending);
    assert_eq!(actor.dimension, dimension);
    assert_eq!(actor.motion.position().get(), position);
    assert_eq!(actor.motion.velocity().get(), [0.; 3]);
    assert!(!actor.motion.on_ground());
    let runtime = view.runtime(actor.key).unwrap();
    assert_eq!(runtime.controls, None);
    assert!(!runtime.reset);
    assert_eq!(runtime.peak_y, 321.);
}
fn activated(
    state: &AuthorityState,
    session: SessionKey,
    publication: &TickPublication,
    dimension: Dimension,
    position: [f32; 3],
    ground: bool,
    original: Option<&PlayerSave>,
) {
    let view = state.settled_read().unwrap();
    let key = ActorKey::Player(session);
    let actor = view.actor(key).unwrap();
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert_eq!(actor.dimension, dimension);
    assert_eq!(actor.motion.position().get(), position);
    assert_eq!(actor.motion.velocity().get(), [0.; 3]);
    assert_eq!(actor.motion.on_ground(), ground);
    assert_eq!(view.runtime(key).unwrap().controls, None);
    assert!(!view.runtime(key).unwrap().reset);
    assert_eq!(view.runtime(key).unwrap().peak_y, position[1]);
    let local = local(publication);
    assert!(local.ready());
    assert!(local.reset());
    assert_eq!(local.last_input_sequence(), 0);
    assert_eq!(local.motion(), actor.motion);
    if let Some(original) = original {
        assert_eq!(
            (actor.look.yaw(), actor.look.pitch()),
            (original.yaw, original.pitch)
        );
        assert_eq!(actor.survival.health(), original.health);
        assert_eq!(actor.survival.hunger(), original.hunger);
        assert_eq!(
            view.inventory(key).unwrap().selected.get(),
            original.inventory.hotbar.selected
        );
        assert_eq!(
            view.inventory(key).unwrap().slots[..9],
            original.inventory.hotbar.slots
        );
        assert_eq!(view.inventory(key).unwrap().armor, original.armor);
        let ActorBody::Player(body) = &actor.body else {
            panic!("player body")
        };
        assert_eq!(body.safe, original.safe);
    }
}
#[test]
fn actual_loaded_current_waits_for_acquire_and_pending_commands_stay_inert() {
    let mut save = saved_player();
    save.current.dimension = 1;
    save.safe = Some(PlayerLocation {
        dimension: 0,
        position: [56.5, 64., 8.5],
    });
    let current = key(Dimension::DEPTHS, 0, 0);
    let safe = key(Dimension::OVERWORLD, 3, 0);
    let (mut fixture, mut state) = Fixture::new(
        Some(save.clone()),
        Dimension::OVERWORLD,
        ChunkPos::new(2, -1),
        vec![(current, air()), (safe, floor())],
    );
    let (mut login, mut transport, connection, session, clock) =
        handshake(&mut fixture, &mut state);
    pending(&state, session, Dimension::OVERWORLD, [32.5, 321., -15.5]);
    let waiting = fixture.acquire(&mut state, safe);
    assert!(!local(&waiting).ready());
    assert!(!local(&waiting).reset());
    pending(&state, session, Dimension::OVERWORLD, [32.5, 321., -15.5]);
    for packet in [
        ClientPacket::PlayerInput(
            PlayerInput::new(1, 1, 0, false, 1.2, 0.3, false, false, false, false).unwrap(),
        ),
        ClientPacket::SelectHotbar(SelectHotbar::new(2, 5).unwrap()),
    ] {
        assert!(!matches!(
            transport.send(
                connection,
                MemoryTransport::encode_frame(&packet).unwrap(),
                &mut login.bind(&mut state, &mut fixture.store),
                &clock
            ),
            ConnectionProgress::Closed { .. }
        ));
    }
    let publication = fixture.acquire(&mut state, current);
    assert_eq!(publication.counters.commands, 2);
    assert_eq!(state.session(session).unwrap().last_applied_sequence, 2);
    activated(
        &state,
        session,
        &publication,
        Dimension::DEPTHS,
        save.current.position,
        false,
        Some(&save),
    );
    for packet in [
        ClientPacket::PlayerInput(
            PlayerInput::new(3, 1, 0, false, 1.2, 0.3, false, false, false, false).unwrap(),
        ),
        ClientPacket::SelectHotbar(SelectHotbar::new(4, 5).unwrap()),
    ] {
        assert!(!matches!(
            transport.send(
                connection,
                MemoryTransport::encode_frame(&packet).unwrap(),
                &mut login.bind(&mut state, &mut fixture.store),
                &clock
            ),
            ConnectionProgress::Closed { .. }
        ));
    }
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    let view = state.settled_read().unwrap();
    let actor = view.actor(ActorKey::Player(session)).unwrap();
    let pos = actor.motion.position().get();
    assert!(
        pos[0] != save.current.position[0] || pos[2] != save.current.position[2],
        "actual native horizontal motion"
    );
    assert_eq!(publication.counters.commands, 2);
    assert_eq!(state.session(session).unwrap().last_applied_sequence, 4);
    assert_eq!((actor.look.yaw(), actor.look.pitch()), (1.2, 0.3));
    assert_eq!(local(&publication).last_input_sequence(), 3);
    assert!(!local(&publication).reset());
    assert_eq!(view.inventory(actor.key).unwrap().selected.get(), 5);
    fixture.close();
}
#[test]
fn actual_missing_player_ignores_fully_ready_canonical_current() {
    let keys = [
        key(Dimension::OVERWORLD, -1, -1),
        key(Dimension::OVERWORLD, -1, 0),
        key(Dimension::OVERWORLD, 0, -1),
        key(Dimension::OVERWORLD, 0, 0),
    ];
    let anchor = key(Dimension::DEPTHS, 2, 0);
    let mut bodies: Vec<_> = keys.into_iter().map(|key| (key, air())).collect();
    bodies.push((anchor, floor()));
    let (mut fixture, mut state) =
        Fixture::new(None, Dimension::DEPTHS, ChunkPos::new(2, 0), bodies);
    for key in keys {
        fixture.acquire(&mut state, key);
    }
    let (_, _, _, session, _) = handshake(&mut fixture, &mut state);
    pending(&state, session, Dimension::DEPTHS, [32.5, 321., 0.5]);
    let waiting = state.advance_tick(TickBudget::full()).unwrap();
    assert!(!local(&waiting).ready());
    pending(&state, session, Dimension::DEPTHS, [32.5, 321., 0.5]);
    let publication = fixture.acquire(&mut state, anchor);
    activated(
        &state,
        session,
        &publication,
        Dimension::DEPTHS,
        [32.5, 64., 0.5],
        true,
        None,
    );
    fixture.close();
}
#[test]
fn actual_solid_current_waits_then_activates_supported_safe() {
    let mut save = saved_player();
    save.safe = Some(PlayerLocation {
        dimension: 1,
        position: [56.5, 64., 8.5],
    });
    let current = key(Dimension::OVERWORLD, 0, 0);
    let safe = key(Dimension::DEPTHS, 3, 0);
    let (mut fixture, mut state) = Fixture::new(
        Some(save.clone()),
        Dimension::OVERWORLD,
        ChunkPos::new(2, 0),
        vec![(current, solid()), (safe, floor())],
    );
    let (_, _, _, session, _) = handshake(&mut fixture, &mut state);
    let waiting = fixture.acquire(&mut state, current);
    assert!(!local(&waiting).ready());
    pending(&state, session, Dimension::OVERWORLD, [32.5, 321., 0.5]);
    let publication = fixture.acquire(&mut state, safe);
    activated(
        &state,
        session,
        &publication,
        Dimension::DEPTHS,
        save.safe.as_ref().unwrap().position,
        true,
        Some(&save),
    );
    fixture.close();
}
#[test]
fn actual_solid_current_and_finite_invalid_safe_fall_back_to_anchor() {
    let mut save = saved_player();
    save.safe = Some(PlayerLocation {
        dimension: 1,
        position: [56.5, -100., 8.5],
    });
    let current = key(Dimension::OVERWORLD, 0, 0);
    let anchor = key(Dimension::OVERWORLD, 2, 0);
    let (mut fixture, mut state) = Fixture::new(
        Some(save.clone()),
        Dimension::OVERWORLD,
        ChunkPos::new(2, 0),
        vec![(current, solid()), (anchor, floor())],
    );
    let (_, _, _, session, _) = handshake(&mut fixture, &mut state);
    let waiting = fixture.acquire(&mut state, current);
    assert!(!local(&waiting).ready());
    let publication = fixture.acquire(&mut state, anchor);
    activated(
        &state,
        session,
        &publication,
        Dimension::OVERWORLD,
        [32.5, 64., 0.5],
        true,
        Some(&save),
    );
    fixture.close();
}
