//! Actual disk, Memory login, Acquire and native restoration, recovery, death, Safe and trample recipes.
//! Manual wants qualify the caller without accepting a source subscription producer.
//! Source Snow recipes execute retained native travel, lethal original-cell settlement and quiet activation.

use mornlea_domain::{
    BlockPos, ChunkPos, CompanionId, Dimension, FiniteVec3, HostileId, Identities, LookAngles,
    MotionState, MotionStateParts, PassiveId, PlayerId, SurvivalState, SurvivalStateParts,
};
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
    Chunk, ChunkSave, CompanionBody, ContainerSnapshot, HostileMob, Inventory, ItemStack, Metadata,
    MetadataChunkPos, PassiveMob, PlayerLocation, PlayerSave, StorageKind, StoredCompanionTask,
    player_encoded_len,
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

fn height_floor(y: i32) -> Chunk {
    assert!((-64..320).contains(&y));
    let mut chunk = air();
    let row = ((y + 64) % 16) as usize;
    let mut packed = vec![0; 256];
    packed[row * 16..(row + 1) * 16].fill(0x1111111111111111);
    chunk.sections[((y + 64) / 16) as usize] = ContainerSnapshot {
        kind: StorageKind::Indexed,
        bits: 4,
        single: 0,
        palette: vec![0, 1],
        packed,
    };
    chunk
}

fn height_player_save(position: [f32; 3]) -> PlayerSave {
    let mut save = saved_player();
    save.current.position = position;
    save.yaw = 0.;
    save.pitch = 0.;
    save.health = 20;
    save.hunger = 20;
    save.saturation_milli = 5000;
    save.exhaustion_milli = 0;
    save.armor = [ItemStack::default(); 4];
    save.inventory = Inventory::default();
    save
}

#[test]
fn height_actual_player_lower_uses_live_native_air() {
    let save = height_player_save([8.5, -63., 8.5]);
    let column = key(Dimension::OVERWORLD, 0, 0);
    let (mut fixture, mut state) = Fixture::new(
        Some(save.clone()),
        Dimension::OVERWORLD,
        ChunkPos::new(0, 0),
        vec![(column, air())],
    );
    let (_, _, _, session, _) = handshake(&mut fixture, &mut state);
    let publication = fixture.acquire(&mut state, column);
    activated(
        &state,
        session,
        &publication,
        Dimension::OVERWORLD,
        save.current.position,
        false,
        Some(&save),
    );
    let mut crossed = false;
    for _ in 0..100 {
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        let view = state.settled_read().unwrap();
        let actor = view.actor(ActorKey::Player(session)).unwrap();
        assert_eq!(actor.lifecycle, ActorLifecycle::Active);
        assert!(local(&publication).ready());
        assert!(!local(&publication).reset());
        crossed |= actor.motion.position().get()[1] < -64.;
        if actor.motion.position().get()[1] < -80. {
            break;
        }
    }
    let actor = state
        .settled_read()
        .unwrap()
        .actor(ActorKey::Player(session))
        .unwrap()
        .clone();
    fixture.close();
    assert!(crossed, "actual native motion must cross the lower plane");
    assert!(actor.motion.position().get()[1] < -80.);
    assert!(actor.motion.velocity().get()[1] < 0.);
}

#[test]
fn height_actual_player_upper_uses_live_native_air() {
    let save = height_player_save([8.5, 318., 8.5]);
    let column = key(Dimension::OVERWORLD, 0, 0);
    let (mut fixture, mut state) = Fixture::new(
        Some(save.clone()),
        Dimension::OVERWORLD,
        ChunkPos::new(0, 0),
        vec![(column, height_floor(317))],
    );
    let (mut login, mut transport, connection, session, clock) =
        handshake(&mut fixture, &mut state);
    let publication = fixture.acquire(&mut state, column);
    activated(
        &state,
        session,
        &publication,
        Dimension::OVERWORLD,
        save.current.position,
        true,
        Some(&save),
    );
    let packet = ClientPacket::PlayerInput(
        PlayerInput::new(1, 0, 0, true, 0., 0., false, false, false, false).unwrap(),
    );
    assert!(!matches!(
        transport.send(
            connection,
            MemoryTransport::encode_frame(&packet).unwrap(),
            &mut login.bind(&mut state, &mut fixture.store),
            &clock
        ),
        ConnectionProgress::Closed { .. }
    ));
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    let actor = state
        .settled_read()
        .unwrap()
        .actor(ActorKey::Player(session))
        .unwrap()
        .clone();
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert!(local(&publication).ready());
    assert!(!local(&publication).reset());
    assert_eq!(local(&publication).last_input_sequence(), 1);
    assert_eq!(local(&publication).motion(), actor.motion);
    fixture.close();
    assert!(
        actor.motion.position().get()[1] > 318.3,
        "actual body head must cross the upper plane"
    );
    assert!(actor.motion.velocity().get()[1] > 0.);
}

#[test]
fn height_actual_player_missing_column_stays_blocking() {
    let save = height_player_save([15.5, 65., 8.5]);
    let column = key(Dimension::OVERWORLD, 0, 0);
    let (mut fixture, mut state) = Fixture::new(
        Some(save.clone()),
        Dimension::OVERWORLD,
        ChunkPos::new(0, 0),
        vec![(column, height_floor(64))],
    );
    let (mut login, mut transport, connection, session, clock) =
        handshake(&mut fixture, &mut state);
    let publication = fixture.acquire(&mut state, column);
    activated(
        &state,
        session,
        &publication,
        Dimension::OVERWORLD,
        save.current.position,
        true,
        Some(&save),
    );
    let packet = ClientPacket::PlayerInput(
        PlayerInput::new(1, 1, 0, false, 0., 0., false, false, false, false).unwrap(),
    );
    assert!(!matches!(
        transport.send(
            connection,
            MemoryTransport::encode_frame(&packet).unwrap(),
            &mut login.bind(&mut state, &mut fixture.store),
            &clock
        ),
        ConnectionProgress::Closed { .. }
    ));
    for _ in 0..40 {
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        let view = state.settled_read().unwrap();
        let actor = view.actor(ActorKey::Player(session)).unwrap();
        assert_eq!(actor.lifecycle, ActorLifecycle::Active);
        assert!(local(&publication).ready());
        assert!(!local(&publication).reset());
        assert_eq!(local(&publication).last_input_sequence(), 1);
    }
    let view = state.settled_read().unwrap();
    let actor = view.actor(ActorKey::Player(session)).unwrap();
    let position = actor.motion.position().get();
    assert!(position[0] > 15.5 && position[0] < 15.71);
    assert_eq!(position[1], 65.);
    assert_eq!(
        view.observation(Dimension::OVERWORLD, BlockPos::new(16, 65, 8)),
        None
    );
    assert_eq!(
        state.live_chunk_facts(column).unwrap().phase,
        LiveChunkPhase::Ready
    );
    assert!(
        state
            .live_chunk_facts(key(Dimension::OVERWORLD, 1, 0))
            .is_none()
    );
    assert!(!fixture.wanted.contains(&key(Dimension::OVERWORLD, 1, 0)));
    fixture.close();
}

#[derive(Clone, Copy)]
enum HeightKind {
    Player(SessionKey),
    Companion,
    Passive,
    Hostile,
}
#[derive(Clone, Copy)]
enum HeightScenario {
    Lower,
    Upper,
    Missing,
}
impl HeightScenario {
    fn motion(self) -> ([f32; 3], [f32; 3]) {
        match self {
            Self::Lower => ([8.5, -63.9, 8.5], [0., -10., 0.]),
            Self::Upper => ([8.5, 318., 8.5], [0., 10., 0.]),
            Self::Missing => ([144.5, 65., -47.5], [0., -10., 0.]),
        }
    }
}

// Off-tick checked replay admission qualifies native providers, not actor bootstrap.
fn height_actor(
    kind: HeightKind,
    position: [f32; 3],
    velocity: [f32; 3],
) -> (ActorRecord, ActorRuntime) {
    let mut companion_bytes = [0; 16];
    companion_bytes[0] = 2;
    companion_bytes[6] = 0x40;
    companion_bytes[8] = 0x80;
    let companion_id = CompanionId::try_from_bytes(companion_bytes).unwrap();
    let (key, body, aux) = match kind {
        HeightKind::Player(session) => (
            ActorKey::Player(session),
            ActorBody::Player(height_player_save(position)),
            ActorAux::Player {
                respawn: None,
                workbench: None,
            },
        ),
        HeightKind::Companion => (
            ActorKey::Companion(companion_id),
            ActorBody::Companion(CompanionBody {
                id: mornlea_storage::PlayerId::from_bytes(companion_id.bytes()),
                dimension: 0,
                position,
                yaw: 0.,
                pitch: 0.,
                inventory: Inventory::default(),
            }),
            ActorAux::Companion {
                generation: 1,
                attempt: 1,
                task: StoredCompanionTask::default(),
                mining_target: None,
            },
        ),
        HeightKind::Passive => (
            ActorKey::Passive(PassiveId::try_new(1).unwrap()),
            ActorBody::Passive(PassiveMob {
                id: 1,
                dimension: 0,
                position,
                velocity,
                on_ground: false,
                yaw: 0.,
                health: 20,
            }),
            ActorAux::Passive {
                home: BlockPos::new(
                    position[0].floor() as i32,
                    position[1].floor() as i32,
                    position[2].floor() as i32,
                ),
                flee_ticks: 0,
                flee_from: None,
                graze_ticks: 0,
                graze_at: None,
                fresh: false,
            },
        ),
        HeightKind::Hostile => (
            ActorKey::Hostile(HostileId::try_new(1).unwrap()),
            ActorBody::Hostile(HostileMob {
                id: 1,
                dimension: 0,
                position,
                velocity,
                on_ground: false,
                yaw: 0.,
                health: 20,
                attack_cooldown: 0,
                hurt_cooldown: 0,
                burn_cooldown: 20,
                has_target: false,
                player_id: mornlea_storage::PlayerId::from_bytes([0; 16]),
                next_repath_ticks: 0,
                distant_ticks: 0,
                kind: 0,
            }),
            ActorAux::Hostile {
                distant_ticks: 0,
                shoot_cooldown: 0,
                fresh: false,
            },
        ),
    };
    let actor = ActorRecord::try_new(
        key,
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new(velocity).unwrap(),
            on_ground: false,
        }),
        LookAngles::try_new(0., 0.).unwrap(),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .unwrap(),
        body,
    )
    .unwrap();
    let runtime = ActorRuntime {
        key,
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 20,
        oxygen: 300,
        peak_y: position[1],
        exhaustion_milli: 0,
        saturation_milli: 5000,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux,
    };
    (actor, runtime)
}

fn height_insert(
    state: &mut AuthorityState,
    kind: HeightKind,
    scenario: HeightScenario,
    sparse: bool,
) -> ActorKey {
    let (position, velocity) = scenario.motion();
    let (actor, runtime) = height_actor(kind, position, velocity);
    let actor_key = actor.key;
    let mut residents = state.residents();
    if sparse {
        let ys = match scenario {
            HeightScenario::Lower => -64..=-59,
            HeightScenario::Upper => 314..=319,
            HeightScenario::Missing => unreachable!(),
        };
        let column = key(Dimension::OVERWORLD, 0, 0);
        for y in ys {
            for x in 4..=12 {
                for z in 4..=12 {
                    let pos = BlockPos::new(x, y, z);
                    residents.blocks.insert(
                        (column, pos),
                        BlockObservation::try_new(column, 1, 1, pos, 0).unwrap(),
                    );
                }
            }
        }
    }
    residents.actors.push(actor);
    residents.runtimes.insert(actor_key, runtime);
    if matches!(kind, HeightKind::Player(_)) {
        residents
            .inventories
            .insert(actor_key, InventoryRecord::empty());
    }
    state.commit_residents(residents);
    actor_key
}

fn height_managed_provider(kind: HeightKind, scenario: HeightScenario) {
    let column = key(Dimension::OVERWORLD, 0, 0);
    let (mut fixture, mut state) = Fixture::new(
        None,
        Dimension::OVERWORLD,
        ChunkPos::new(0, 0),
        vec![(column, air())],
    );
    fixture.acquire(&mut state, column);
    let actor_key = height_insert(&mut state, kind, scenario, false);
    if matches!(scenario, HeightScenario::Missing) {
        assert_eq!(
            state
                .settled_read()
                .unwrap()
                .observation(Dimension::OVERWORLD, BlockPos::new(144, 65, -48)),
            None
        );
        assert!(
            state
                .live_chunk_facts(key(Dimension::OVERWORLD, 9, -3))
                .is_none()
        );
    }
    state.advance_tick(TickBudget::full()).unwrap();
    let actor = state
        .settled_read()
        .unwrap()
        .actor(actor_key)
        .unwrap()
        .clone();
    fixture.close();
    match scenario {
        HeightScenario::Lower if matches!(kind, HeightKind::Passive | HeightKind::Hostile) => {
            // Existing source floor removal retains the pre-removal pose.
            assert_eq!(actor.lifecycle, ActorLifecycle::Dead);
        }
        HeightScenario::Lower => {
            assert_eq!(actor.lifecycle, ActorLifecycle::Active);
            assert!(actor.motion.position().get()[1] < -64.);
            assert!(actor.motion.velocity().get()[1] < 0.);
        }
        HeightScenario::Upper => {
            assert_eq!(actor.lifecycle, ActorLifecycle::Active);
            assert!(actor.motion.position().get()[1] > 318.3);
            assert!(actor.motion.velocity().get()[1] > 0.);
        }
        HeightScenario::Missing => {
            assert_eq!(actor.lifecycle, ActorLifecycle::Active);
            assert!(actor.motion.position().get()[1] >= 65.);
            assert_eq!(actor.motion.velocity().get()[1], 0.);
        }
    }
}

// Sparse disabled controls exercise full ticks without live acquisition owners.
fn height_disabled_provider(kind: Option<HeightKind>) {
    for scenario in [HeightScenario::Lower, HeightScenario::Upper] {
        let mut state = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            42,
        )
        .unwrap();
        let kind = kind.unwrap_or_else(|| {
            let start = LoginStart::new(player(), "Ada", 8).unwrap();
            let inbound = LoginStart::decode_inbound(&start.encode().unwrap()).unwrap();
            HeightKind::Player(
                state
                    .allocate(
                        mornlea_protocol::admit_login(inbound).unwrap(),
                        TransportKind::Memory,
                    )
                    .unwrap(),
            )
        });
        let actor_key = height_insert(&mut state, kind, scenario, true);
        state.advance_tick(TickBudget::full()).unwrap();
        let view = state.settled_read().unwrap();
        let actor = view.actor(actor_key).unwrap();
        assert_eq!(actor.lifecycle, ActorLifecycle::Active);
        match scenario {
            HeightScenario::Lower => assert!(actor.motion.position().get()[1] >= -64.),
            HeightScenario::Upper => assert!(actor.motion.position().get()[1] < 318.3),
            HeightScenario::Missing => unreachable!(),
        }
        assert_eq!(actor.motion.velocity().get()[1], 0.);
    }
}

#[test]
fn height_actual_companion_lower() {
    height_managed_provider(HeightKind::Companion, HeightScenario::Lower);
}

#[test]
fn height_actual_companion_upper() {
    height_managed_provider(HeightKind::Companion, HeightScenario::Upper);
}

#[test]
fn height_actual_companion_missing_column() {
    height_managed_provider(HeightKind::Companion, HeightScenario::Missing);
}

#[test]
fn height_actual_passive_lower() {
    height_managed_provider(HeightKind::Passive, HeightScenario::Lower);
}

#[test]
fn height_actual_passive_upper() {
    height_managed_provider(HeightKind::Passive, HeightScenario::Upper);
}

#[test]
fn height_actual_passive_missing_column() {
    height_managed_provider(HeightKind::Passive, HeightScenario::Missing);
}

#[test]
fn height_actual_hostile_lower() {
    height_managed_provider(HeightKind::Hostile, HeightScenario::Lower);
}

#[test]
fn height_actual_hostile_upper() {
    height_managed_provider(HeightKind::Hostile, HeightScenario::Upper);
}

#[test]
fn height_actual_hostile_missing_column() {
    height_managed_provider(HeightKind::Hostile, HeightScenario::Missing);
}

#[test]
fn height_disabled_player_controls() {
    height_disabled_provider(None);
}

#[test]
fn height_disabled_companion_controls() {
    height_disabled_provider(Some(HeightKind::Companion));
}

#[test]
fn height_disabled_passive_controls() {
    height_disabled_provider(Some(HeightKind::Passive));
}

#[test]
fn height_disabled_hostile_controls() {
    height_disabled_provider(Some(HeightKind::Hostile));
}

fn recovery_observed(
    state: &AuthorityState,
    session: SessionKey,
) -> (ActorRecord, ActorRuntime, InventoryRecord) {
    let view = state.settled_read().unwrap();
    let actor_key = ActorKey::Player(session);
    (
        view.actor(actor_key).unwrap().clone(),
        view.runtime(actor_key).unwrap().clone(),
        *view.inventory(actor_key).unwrap(),
    )
}
fn recovery_rich_preserved(
    before: &(ActorRecord, ActorRuntime, InventoryRecord),
    after: &(ActorRecord, ActorRuntime, InventoryRecord),
) {
    assert_eq!(after.0.body, before.0.body);
    assert_eq!(after.0.look, before.0.look);
    assert_eq!(after.2, before.2);
    assert_eq!(after.0.survival.health(), before.0.survival.health());
    assert_eq!(after.0.survival.hunger(), before.0.survival.hunger());
    assert_eq!(after.1.exhaustion_milli, before.1.exhaustion_milli);
    assert_eq!(after.1.saturation_milli, before.1.saturation_milli);
}
// External resident preparation uses checked off-tick values, not an online command API.
fn recovery_off_tick_pose(state: &mut AuthorityState, session: SessionKey, position: [f32; 3]) {
    let mut residents = state.residents();
    let actor = residents
        .actors
        .iter_mut()
        .find(|actor| actor.key == ActorKey::Player(session))
        .unwrap();
    actor.motion = MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new(position).unwrap(),
        velocity: FiniteVec3::try_new([0.; 3]).unwrap(),
        on_ground: true,
    });
    state.commit_residents(residents);
}
#[test]
fn recovery_actual_native_fall_reactivates_captured_anchor() {
    let mut save = saved_player();
    save.current.position = [8.5, -63., 8.5];
    save.health = 7;
    save.hunger = 9;
    let current = key(Dimension::OVERWORLD, 0, 0);
    let anchor = key(Dimension::OVERWORLD, -2, 3);
    let (mut fixture, mut state) = Fixture::new(
        Some(save.clone()),
        Dimension::DEPTHS,
        ChunkPos::new(-2, 3),
        vec![(current, air()), (anchor, height_floor(64))],
    );
    let (mut login, mut transport, connection, session, clock) =
        handshake(&mut fixture, &mut state);
    let initial_publication = fixture.acquire(&mut state, current);
    let initial = recovery_observed(&state, session);
    let packet = ClientPacket::PlayerInput(
        PlayerInput::new(
            3, 0, 0, false, save.yaw, save.pitch, false, false, false, false,
        )
        .unwrap(),
    );
    let input_progress = transport.send(
        connection,
        MemoryTransport::encode_frame(&packet).unwrap(),
        &mut login.bind(&mut state, &mut fixture.store),
        &clock,
    );
    let mut fall = Vec::new();
    for _ in 0..100 {
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        let observed = recovery_observed(&state, session);
        let crossed = observed.0.motion.position().get()[1] < -80.;
        fall.push((observed, *local(&publication)));
        if crossed {
            break;
        }
    }
    let before_recovery = recovery_observed(&state, session);
    let reset_publication = state.advance_tick(TickBudget::full()).unwrap();
    let reset = recovery_observed(&state, session);
    let reset_local = *local(&reset_publication);
    let waiting_publication = state.advance_tick(TickBudget::full()).unwrap();
    let waiting = recovery_observed(&state, session);
    let waiting_local = *local(&waiting_publication);
    let reacquired_publication = fixture.acquire(&mut state, anchor);
    let reacquired = recovery_observed(&state, session);
    let reacquired_local = *local(&reacquired_publication);
    let next_publication = state.advance_tick(TickBudget::full()).unwrap();
    let next_local = *local(&next_publication);
    fixture.close();
    assert!(!matches!(input_progress, ConnectionProgress::Closed { .. }));
    assert_eq!(initial.0.lifecycle, ActorLifecycle::Active);
    assert_eq!(initial.0.dimension, Dimension::OVERWORLD);
    assert_eq!(initial.0.motion.position().get(), save.current.position);
    assert!(local(&initial_publication).ready() && local(&initial_publication).reset());
    assert!(fall.len() <= 100);
    assert!(before_recovery.0.motion.position().get()[1] < -80.);
    assert!(before_recovery.0.motion.velocity().get()[1] < 0.);
    for (observed, local) in &fall {
        assert_eq!(observed.0.lifecycle, ActorLifecycle::Active);
        assert!(local.ready() && !local.reset());
        assert_eq!(local.last_input_sequence(), 3);
    }
    assert_eq!(reset.0.lifecycle, ActorLifecycle::Pending);
    assert_eq!(reset.0.dimension, Dimension::OVERWORLD);
    assert_eq!(reset.0.motion.position().get(), [-31.5, 321., 48.5]);
    assert_eq!(reset.0.motion.velocity().get(), [0.; 3]);
    assert!(!reset.0.motion.on_ground());
    assert_eq!(reset.0.survival.oxygen(), 300);
    assert_eq!(reset.1.peak_y, 321.);
    assert!(!reset.1.reset && !reset_local.ready() && !reset_local.reset());
    assert_eq!(reset_local.last_input_sequence(), 3);
    recovery_rich_preserved(&before_recovery, &reset);
    recovery_rich_preserved(&initial, &reset);
    assert_eq!(
        (reset.0.survival.health(), reset.0.survival.hunger()),
        (7, 9)
    );
    assert_eq!(waiting.0.lifecycle, ActorLifecycle::Pending);
    assert_eq!(waiting.0.motion, reset.0.motion);
    assert!(!waiting_local.ready() && !waiting_local.reset());
    assert_eq!(waiting_local.last_input_sequence(), 3);
    assert_eq!(reacquired.0.lifecycle, ActorLifecycle::Active);
    assert_eq!(reacquired.0.dimension, Dimension::OVERWORLD);
    assert_eq!(reacquired.0.motion.position().get(), [-31.5, 65., 48.5]);
    assert_eq!(reacquired.0.motion.velocity().get(), [0.; 3]);
    assert!(reacquired.0.motion.on_ground());
    assert!(reacquired_local.ready() && reacquired_local.reset());
    assert_eq!(reacquired_local.last_input_sequence(), 3);
    recovery_rich_preserved(&initial, &reacquired);
    assert!(next_local.ready() && !next_local.reset());
    assert_eq!(next_local.last_input_sequence(), 3);
}
#[test]
fn recovery_actual_embedded_pose_lifts_before_native() {
    let save = saved_player();
    let column = key(Dimension::OVERWORLD, 0, 0);
    let (mut fixture, mut state) = Fixture::new(
        Some(save),
        Dimension::OVERWORLD,
        ChunkPos::new(2, 0),
        vec![(column, height_floor(64))],
    );
    let (_, _, _, session, _) = handshake(&mut fixture, &mut state);
    fixture.acquire(&mut state, column);
    let before = recovery_observed(&state, session);
    recovery_off_tick_pose(&mut state, session, [8.5, 64.75, 8.5]);
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    let after = recovery_observed(&state, session);
    let local = *local(&publication);
    fixture.close();
    assert_eq!(after.0.lifecycle, ActorLifecycle::Active);
    assert_eq!(after.0.motion.position().get(), [8.5, 65., 8.5]);
    assert_eq!(after.0.motion.velocity().get(), [0.; 3]);
    assert!(after.0.motion.on_ground());
    assert!(local.ready() && !local.reset());
    assert_eq!(local.last_input_sequence(), 0);
    // Successful recovery proceeds through native motion and records its supported Safe pose.
    let mut expected = before;
    let ActorBody::Player(body) = &mut expected.0.body else {
        unreachable!()
    };
    body.safe = Some(PlayerLocation {
        dimension: 0,
        position: [8.5, 65., 8.5],
    });
    recovery_rich_preserved(&expected, &after);
}
#[test]
fn recovery_actual_blocked_pose_enters_pending() {
    let column = key(Dimension::OVERWORLD, 0, 0);
    let (mut fixture, mut state) = Fixture::new(
        Some(saved_player()),
        Dimension::OVERWORLD,
        ChunkPos::new(2, 0),
        vec![(column, air())],
    );
    let (_, _, _, session, _) = handshake(&mut fixture, &mut state);
    fixture.acquire(&mut state, column);
    let before = recovery_observed(&state, session);
    // This checked off-tick transaction prepares the obstruction against actual Ready AIR.
    let prepared = state.residents();
    let mut context = mornlea_server::state::TickContext::harness(&mut state, TickBudget::full());
    for (key, generation, revision, chunk) in prepared.ready_snapshot() {
        context.preload_ready_chunk(
            mornlea_server::core::world::ReadyChunk::try_new(key, generation, revision, chunk)
                .unwrap(),
        );
    }
    for actor in prepared.actors {
        context.stage(RuleEffect::Actor(actor)).unwrap();
    }
    for runtime in prepared.runtimes.into_values() {
        context.stage(RuleEffect::Runtime(runtime)).unwrap();
    }
    for (key, inventory) in prepared.inventories {
        context.preload_inventory(key, inventory);
    }
    if let Some(environment) = prepared.environment {
        context.stage(RuleEffect::Environment(environment)).unwrap();
    }
    if let Some(sleep) = prepared.sleep_record {
        context.stage(RuleEffect::Sleep(sleep)).unwrap();
    }
    for y in 64..=67 {
        let observed = context
            .read()
            .observation(Dimension::OVERWORLD, BlockPos::new(8, y, 8))
            .unwrap();
        context
            .transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(observed, 2).unwrap()],
            )
            .unwrap();
    }
    let residents = context.resident_snapshot();
    drop(context);
    state.commit_residents(residents);
    recovery_off_tick_pose(&mut state, session, [8.5, 64.75, 8.5]);
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    let after = recovery_observed(&state, session);
    let local = *local(&publication);
    fixture.close();
    assert_eq!(after.0.lifecycle, ActorLifecycle::Pending);
    assert_eq!(after.0.motion.position().get(), [32.5, 321., 0.5]);
    assert_eq!(after.0.motion.velocity().get(), [0.; 3]);
    assert!(!after.0.motion.on_ground());
    assert!(!local.ready() && !local.reset());
    assert_eq!(local.last_input_sequence(), 0);
    recovery_rich_preserved(&before, &after);
}
#[test]
fn recovery_actual_unknown_footprint_enters_pending() {
    let mut save = saved_player();
    save.current.position = [15.5, 65., 8.5];
    let column = key(Dimension::OVERWORLD, 0, 0);
    let (mut fixture, mut state) = Fixture::new(
        Some(save),
        Dimension::OVERWORLD,
        ChunkPos::new(2, 0),
        vec![(column, height_floor(64))],
    );
    let (_, _, _, session, _) = handshake(&mut fixture, &mut state);
    fixture.acquire(&mut state, column);
    let before = recovery_observed(&state, session);
    recovery_off_tick_pose(&mut state, session, [15.85, 65., 8.5]);
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    let after = recovery_observed(&state, session);
    let local = *local(&publication);
    let unknown = state
        .settled_read()
        .unwrap()
        .observation(Dimension::OVERWORLD, BlockPos::new(16, 65, 8));
    fixture.close();
    assert_eq!(after.0.lifecycle, ActorLifecycle::Pending);
    assert_eq!(after.0.motion.position().get(), [32.5, 321., 0.5]);
    assert_eq!(after.0.motion.velocity().get(), [0.; 3]);
    assert!(!local.ready() && !local.reset());
    assert_eq!(local.last_input_sequence(), 0);
    assert_eq!(unknown, None);
    recovery_rich_preserved(&before, &after);
}

// Off-tick palette edits preserve every untargeted cell and the persisted floor.
fn death_chunk_cell(chunk: &mut Chunk, pos: BlockPos, form: u16) {
    assert!((0..16).contains(&pos.x()) && (0..16).contains(&pos.z()));
    assert!((-64..320).contains(&pos.y()));
    let section = &mut chunk.sections[((pos.y() + 64) / 16) as usize];
    if section.kind == StorageKind::Single {
        let old = section.single;
        *section = ContainerSnapshot {
            kind: StorageKind::Indexed,
            bits: 4,
            single: 0,
            palette: vec![old],
            packed: vec![0; 256],
        };
    }
    assert_eq!(section.kind, StorageKind::Indexed);
    assert_eq!(section.bits, 4);
    assert_eq!(section.packed.len(), 256);
    let palette = match section.palette.iter().position(|value| *value == form) {
        Some(index) => index,
        None => {
            assert!(section.palette.len() < 16);
            section.palette.push(form);
            section.palette.len() - 1
        }
    };
    let index = ((pos.y() + 64) % 16) as usize * 256 + pos.z() as usize * 16 + pos.x() as usize;
    let shift = (index % 16) * 4;
    section.packed[index / 16] =
        (section.packed[index / 16] & !(0xf_u64 << shift)) | ((palette as u64) << shift);
}
#[derive(Clone, Copy, Debug)]
enum DeathProducer {
    Starvation,
    Drowning,
    Landing,
    LandingControl,
    Melee,
    Projectile,
}
fn death_fixture_hostile(id: u64, position: [f32; 3]) -> (ActorRecord, ActorRuntime) {
    let key = ActorKey::Hostile(HostileId::try_new(id).unwrap());
    let record = ActorRecord::try_new(
        key,
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.; 3]).unwrap(),
            on_ground: true,
        }),
        LookAngles::try_new(0., 0.).unwrap(),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .unwrap(),
        ActorBody::Hostile(HostileMob {
            id,
            dimension: 0,
            position,
            velocity: [0.; 3],
            on_ground: true,
            yaw: 0.,
            health: 20,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 20,
            has_target: false,
            player_id: mornlea_storage::PlayerId::from_bytes([0; 16]),
            next_repath_ticks: u64::MAX,
            distant_ticks: 0,
            kind: 0,
        }),
    )
    .unwrap();
    let runtime = ActorRuntime {
        key,
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 20,
        oxygen: 0,
        peak_y: 0.,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Hostile {
            distant_ticks: 0,
            shoot_cooldown: 0,
            fresh: false,
        },
    };
    (record, runtime)
}
// Actual login and acquisition complete the original scan before retained producer inputs change.
fn death_fixture(producer: DeathProducer, bed: bool) -> (Fixture, AuthorityState, SessionKey) {
    let position = [if bed { 10.5 } else { 8.5 }, 65., 8.5];
    let mut save = height_player_save(position);
    save.health = 1;
    save.inventory.hotbar.selected = 3;
    save.inventory.hotbar.slots[3] = ItemStack {
        item: 1,
        count: 7,
        durability: 0,
    };
    assert!(!save.respawn_present);
    let current = key(Dimension::OVERWORLD, 0, 0);
    let anchor = key(Dimension::OVERWORLD, -2, 3);
    let mut chunk = height_floor(63);
    if matches!(producer, DeathProducer::Drowning) {
        death_chunk_cell(&mut chunk, BlockPos::new(8, 66, 8), 27);
    }
    if bed {
        death_chunk_cell(&mut chunk, BlockPos::new(8, 64, 8), 76);
        death_chunk_cell(&mut chunk, BlockPos::new(8, 64, 9), 80);
    }
    let (mut fixture, mut state) = Fixture::new(
        Some(save),
        Dimension::DEPTHS,
        ChunkPos::new(-2, 3),
        vec![(current, chunk), (anchor, height_floor(64))],
    );
    let (mut login, mut transport, connection, session, clock) =
        handshake(&mut fixture, &mut state);
    let publication = fixture.acquire(&mut state, current);
    assert!(local(&publication).ready() && local(&publication).reset());
    assert_eq!(
        recovery_observed(&state, session).0.lifecycle,
        ActorLifecycle::Active
    );
    let mut residents = state.residents();
    let player_key = ActorKey::Player(session);
    let actor = residents
        .actors
        .iter_mut()
        .find(|a| a.key == player_key)
        .unwrap();
    actor.motion = MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new(position).unwrap(),
        velocity: FiniteVec3::try_new([
            0.,
            if matches!(
                producer,
                DeathProducer::Landing | DeathProducer::LandingControl
            ) {
                -40.
            } else {
                0.
            },
            0.,
        ])
        .unwrap(),
        on_ground: false,
    });
    actor.survival = SurvivalState::try_new(SurvivalStateParts {
        health: 1,
        hunger: if matches!(producer, DeathProducer::Starvation) {
            0
        } else {
            20
        },
        oxygen: if matches!(producer, DeathProducer::Drowning) {
            0
        } else {
            300
        },
        saturation_zero: matches!(producer, DeathProducer::Starvation),
        armor_points: 0,
    })
    .unwrap();
    let runtime = residents.runtimes.get_mut(&player_key).unwrap();
    runtime.controls = None;
    runtime.reset = false;
    runtime.attack_cooldown = 0;
    runtime.hurt_cooldown = 0;
    runtime.burn_cooldown = 0;
    runtime.oxygen = if matches!(producer, DeathProducer::Drowning) {
        0
    } else {
        300
    };
    runtime.peak_y = match producer {
        DeathProducer::Landing => 68.,
        DeathProducer::LandingControl => 67.,
        _ => 65.,
    };
    runtime.saturation_milli = if matches!(producer, DeathProducer::Starvation) {
        0
    } else {
        5000
    };
    runtime.exhaustion_milli = 0;
    runtime.since_damage_ticks = if matches!(producer, DeathProducer::Starvation) {
        10
    } else {
        0
    };
    runtime.starvation_ticks = if matches!(producer, DeathProducer::Starvation) {
        79
    } else {
        0
    };
    runtime.drown_ticks = if matches!(producer, DeathProducer::Drowning) {
        19
    } else {
        0
    };
    runtime.eating = None;
    runtime.bow = None;
    runtime.aux = ActorAux::Player {
        respawn: bed.then_some((Dimension::OVERWORLD, BlockPos::new(8, 64, 8))),
        workbench: None,
    };
    residents.environment.as_mut().unwrap().difficulty =
        if matches!(producer, DeathProducer::Starvation) {
            2
        } else {
            0
        };
    if matches!(producer, DeathProducer::Melee | DeathProducer::Projectile) {
        let position = if matches!(producer, DeathProducer::Melee) {
            [9.5, 65., 8.5]
        } else {
            [12.5, 65., 12.5]
        };
        let (walker, runtime) = death_fixture_hostile(11, position);
        residents.runtimes.insert(walker.key, runtime);
        residents.actors.push(walker);
    }
    if matches!(producer, DeathProducer::Projectile) {
        residents.projectiles.push(ProjectileRecord {
            id: mornlea_domain::ProjectileId::try_new(1).unwrap(),
            owner: ActorKey::Hostile(HostileId::try_new(11).unwrap()),
            dimension: Dimension::OVERWORLD,
            position: FiniteVec3::try_new([8.5, 65.9, 8.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.; 3]).unwrap(),
            kind: mornlea_domain::ProjectileKind::Shard,
            damage: 1,
            age: 0,
        });
    }
    state.commit_residents(residents);
    let input = ClientPacket::PlayerInput(
        PlayerInput::new(3, 0, 0, false, 0., 0., false, false, false, false).unwrap(),
    );
    assert!(!matches!(
        transport.send(
            connection,
            MemoryTransport::encode_frame(&input).unwrap(),
            &mut login.bind(&mut state, &mut fixture.store),
            &clock
        ),
        ConnectionProgress::Closed { .. }
    ));
    (fixture, state, session)
}
fn death_ground(state: &AuthorityState) -> Vec<DropRecord> {
    state
        .settled_read()
        .unwrap()
        .drops(key(Dimension::OVERWORLD, 0, 0))
        .to_vec()
}
fn death_conserved(state: &AuthorityState, session: SessionKey) {
    let (_, _, inv) = recovery_observed(state, session);
    let ground = death_ground(state);
    assert!(ground.iter().all(|d| d.stack.item == 1));
    assert_eq!(
        inv.slots
            .iter()
            .chain(inv.armor.iter())
            .chain(inv.crafting.iter())
            .map(|s| u32::from(s.count))
            .sum::<u32>()
            + ground.iter().map(|d| u32::from(d.stack.count)).sum::<u32>(),
        7
    );
    assert!(inv.slots.iter().all(|s| *s == ItemStack::default()));
    assert_eq!(inv.selected.get(), 3);
    assert_eq!(
        ground.iter().map(|d| u32::from(d.stack.count)).sum::<u32>(),
        7
    );
}
fn death_no_hit(publication: &TickPublication) {
    assert!(
        !publication
            .events
            .iter()
            .any(|e| matches!(e.event(), mornlea_domain::Event::CombatHit(_)))
    );
}
fn death_pending(state: &AuthorityState, session: SessionKey, publication: &TickPublication) {
    pending(state, session, Dimension::OVERWORLD, [-31.5, 321., 48.5]);
    let (actor, runtime, _) = recovery_observed(state, session);
    assert_eq!(
        (
            actor.survival.health(),
            actor.survival.hunger(),
            actor.survival.oxygen()
        ),
        (20, 20, 300)
    );
    assert_eq!(
        (runtime.saturation_milli, runtime.exhaustion_milli),
        (5000, 0)
    );
    assert_eq!(local(publication).last_input_sequence(), 3);
    let ActorBody::Player(body) = &actor.body else {
        unreachable!()
    };
    assert_eq!(body.current.dimension, 0);
    assert!(!body.respawn_present);
    assert!(!local(publication).ready() && !local(publication).reset());
    death_conserved(state, session);
}
fn death_activated(
    state: &AuthorityState,
    session: SessionKey,
    publication: &TickPublication,
    position: [f32; 3],
) {
    let (actor, runtime, _) = recovery_observed(state, session);
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert_eq!(actor.dimension, Dimension::OVERWORLD);
    assert_eq!(actor.motion.position().get(), position);
    assert_eq!(actor.motion.velocity().get(), [0.; 3]);
    assert_eq!(
        (
            actor.survival.health(),
            actor.survival.hunger(),
            actor.survival.oxygen()
        ),
        (20, 20, 300)
    );
    assert!(local(publication).ready() && local(publication).reset());
    assert!(!runtime.reset);
    assert_eq!(local(publication).last_input_sequence(), 3);
    death_no_hit(publication);
    death_conserved(state, session);
}
fn death_actual_producer(producer: DeathProducer) {
    let (mut fixture, mut state, session) = death_fixture(producer, false);
    let tick = state.next_tick();
    let result = state.advance_tick(TickBudget::full());
    assert!(
        result.is_ok(),
        "{producer:?} intended full-tick early-death boundary: {result:?}"
    );
    let publication = result.unwrap();
    if matches!(
        producer,
        DeathProducer::Starvation | DeathProducer::Drowning | DeathProducer::Landing
    ) {
        let hits: Vec<_> = publication
            .events
            .iter()
            .filter(|e| matches!(e.event(), mornlea_domain::Event::CombatHit(_)))
            .collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].recipient(),
            mornlea_domain::EventRecipient::Session(session.get())
        );
        let mornlea_domain::Event::CombatHit(hit) = hits[0].event() else {
            unreachable!()
        };
        assert_eq!(hit.damage(), 1);
        assert_eq!(hit.server_tick(), tick);
    } else {
        death_no_hit(&publication);
    }
    if matches!(producer, DeathProducer::Melee | DeathProducer::Projectile) {
        let view = state.settled_read().unwrap();
        let ActorBody::Hostile(body) = &view
            .actor(ActorKey::Hostile(HostileId::try_new(11).unwrap()))
            .unwrap()
            .body
        else {
            unreachable!()
        };
        assert_eq!(
            body.attack_cooldown,
            if matches!(producer, DeathProducer::Melee) {
                20
            } else {
                0
            }
        );
        assert_eq!(
            body.position[0],
            if matches!(producer, DeathProducer::Melee) {
                9.5
            } else {
                12.5
            }
        );
        assert!(view.projectiles().is_empty());
    }
    death_pending(&state, session, &publication);
    if matches!(producer, DeathProducer::Landing) {
        assert!(
            death_ground(&state)
                .iter()
                .all(|d| d.position.get()[1] == 64.5)
        );
    }
    let waiting = state.advance_tick(TickBudget::full()).unwrap();
    death_pending(&state, session, &waiting);
    death_no_hit(&waiting);
    let acquired = fixture.acquire(&mut state, key(Dimension::OVERWORLD, -2, 3));
    death_activated(&state, session, &acquired, [-31.5, 65., 48.5]);
    let following = state.advance_tick(TickBudget::full()).unwrap();
    assert!(local(&following).ready() && !local(&following).reset());
    assert_eq!(local(&following).last_input_sequence(), 3);
    assert_eq!(
        recovery_observed(&state, session).0.lifecycle,
        ActorLifecycle::Active
    );
    death_no_hit(&following);
    death_conserved(&state, session);
    fixture.close();
}
#[test]
fn death_actual_starvation_restarts_current_dimension() {
    death_actual_producer(DeathProducer::Starvation);
}
#[test]
fn death_actual_drowning_restarts_current_dimension() {
    death_actual_producer(DeathProducer::Drowning);
}
#[test]
fn death_actual_native_landing_restarts_current_dimension() {
    let (mut fixture, mut state, session) = death_fixture(DeathProducer::LandingControl, false);
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    let (actor, _, inventory) = recovery_observed(&state, session);
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert_eq!(actor.survival.health(), 1);
    assert_eq!(actor.motion.position().get(), [8.5, 64., 8.5]);
    assert!(actor.motion.on_ground());
    assert_eq!(inventory.slots[3].count, 7);
    assert!(death_ground(&state).is_empty());
    death_no_hit(&publication);
    fixture.close();
    death_actual_producer(DeathProducer::Landing);
}
#[test]
fn death_actual_melee_restarts_current_dimension() {
    death_actual_producer(DeathProducer::Melee);
}
#[test]
fn death_actual_projectile_restarts_current_dimension() {
    death_actual_producer(DeathProducer::Projectile);
}
#[test]
fn death_actual_live_bed_activates_on_following_tick() {
    let (mut fixture, mut state, session) = death_fixture(DeathProducer::Starvation, true);
    let tick = state.next_tick();
    let result = state.advance_tick(TickBudget::full());
    assert!(
        result.is_ok(),
        "live bed intended full-tick early-death boundary: {result:?}"
    );
    let publication = result.unwrap();
    let hits: Vec<_> = publication
        .events
        .iter()
        .filter(|e| matches!(e.event(), mornlea_domain::Event::CombatHit(_)))
        .collect();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].recipient(),
        mornlea_domain::EventRecipient::Session(session.get())
    );
    let mornlea_domain::Event::CombatHit(hit) = hits[0].event() else {
        unreachable!()
    };
    assert_eq!((hit.damage(), hit.server_tick()), (1, tick));
    death_pending(&state, session, &publication);
    let following = state.advance_tick(TickBudget::full()).unwrap();
    death_activated(&state, session, &following, [8.5, 64.5625, 8.5]);
    let next = state.advance_tick(TickBudget::full()).unwrap();
    assert!(local(&next).ready() && !local(&next).reset());
    assert_eq!(local(&next).last_input_sequence(), 3);
    death_no_hit(&next);
    death_conserved(&state, session);
    fixture.close();
}
#[test]
fn death_actual_impossible_repack_fences_partial_tick() {
    let (mut fixture, mut state, session) = death_fixture(DeathProducer::Starvation, false);
    let mut residents = state.residents();
    let inv = residents
        .inventories
        .get_mut(&ActorKey::Player(session))
        .unwrap();
    inv.slots = [ItemStack {
        item: 1,
        count: 64,
        durability: 0,
    }; 36];
    inv.crafting[0] = ItemStack {
        item: 1,
        count: 1,
        durability: 0,
    };
    inv.crafting_size = mornlea_domain::CraftingSize::Workbench;
    let expected = *inv;
    state.commit_residents(residents);
    let tick = state.next_tick();
    let error = ServerError::Internal {
        invariant: "source player death crafting",
    };
    let result = state.advance_tick(TickBudget::full());
    fixture.close();
    assert_eq!(result, Err(error));
    assert_eq!(state.next_tick(), tick);
    assert_eq!(state.phase(), ServerPhase::Closing);
    assert_eq!(state.advance_tick(TickBudget::full()), Err(error));
    assert!(matches!(state.settled_read(), Err(e) if e == error));
    assert_eq!(
        state.capture_chunk_snapshot(key(Dimension::OVERWORLD, 0, 0), SaveUrgency::Autosave),
        None
    );
    assert_eq!(state.try_metadata_snapshot(), Err(error));
    let residents = state.residents();
    let actor = residents
        .actors
        .iter()
        .find(|a| a.key == ActorKey::Player(session))
        .unwrap();
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert_eq!(actor.survival.health(), 0);
    assert_eq!(actor.survival.hunger(), 0);
    assert_eq!(residents.inventories[&ActorKey::Player(session)], expected);
    assert!(!residents.runtimes[&ActorKey::Player(session)].reset);
    assert!(
        residents
            .ready_snapshot()
            .iter()
            .all(|(_, _, _, chunk)| chunk.drops.iter().all(|d| !d.active))
    );
}

fn safe_actual_value(
    state: &AuthorityState,
    session: SessionKey,
) -> Option<mornlea_storage::PlayerLocation> {
    let observed = recovery_observed(state, session);
    let ActorBody::Player(body) = observed.0.body else {
        unreachable!()
    };
    body.safe
}
fn safe_actual_expect(state: &AuthorityState, session: SessionKey, position: [f32; 3]) {
    assert_eq!(
        safe_actual_value(state, session),
        Some(mornlea_storage::PlayerLocation {
            dimension: 0,
            position
        })
    );
}

#[test]
fn safe_actual_native_landing_updates_current_dimension() {
    let (mut fixture, mut state, session) = death_fixture(DeathProducer::LandingControl, false);
    let mut residents = state.residents();
    let actor = residents
        .actors
        .iter_mut()
        .find(|a| a.key == ActorKey::Player(session))
        .unwrap();
    let ActorBody::Player(body) = &mut actor.body else {
        unreachable!()
    };
    body.safe = Some(mornlea_storage::PlayerLocation {
        dimension: 1,
        position: [56.5, 64., 8.5],
    });
    state.commit_residents(residents);
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    let (actor, _, inventory) = recovery_observed(&state, session);
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert_eq!(actor.dimension, Dimension::OVERWORLD);
    assert_eq!(actor.survival.health(), 1);
    assert_eq!(actor.motion.position().get(), [8.5, 64., 8.5]);
    assert!(actor.motion.on_ground());
    assert_eq!(inventory.slots[3].count, 7);
    assert!(death_ground(&state).is_empty());
    death_no_hit(&publication);
    assert_eq!(local(&publication).last_input_sequence(), 3);
    safe_actual_expect(&state, session, [8.5, 64., 8.5]);
    let following = state.advance_tick(TickBudget::full()).unwrap();
    safe_actual_expect(&state, session, [8.5, 64., 8.5]);
    death_no_hit(&following);
    assert!(death_ground(&state).is_empty());
    assert_eq!(recovery_observed(&state, session).2.slots[3].count, 7);
    fixture.close();
}

#[test]
fn safe_actual_killing_landing_precedes_death() {
    let (mut fixture, mut state, session) = death_fixture(DeathProducer::Landing, false);
    let tick = state.next_tick();
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    let hits = publication
        .events
        .iter()
        .filter(|e| matches!(e.event(), mornlea_domain::Event::CombatHit(_)))
        .collect::<Vec<_>>();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].recipient(),
        mornlea_domain::EventRecipient::Session(session.get())
    );
    let mornlea_domain::Event::CombatHit(hit) = hits[0].event() else {
        unreachable!()
    };
    assert_eq!(hit.damage(), 1);
    assert_eq!(hit.server_tick(), tick);
    death_pending(&state, session, &publication);
    safe_actual_expect(&state, session, [8.5, 64., 8.5]);
    let waiting = state.advance_tick(TickBudget::full()).unwrap();
    death_pending(&state, session, &waiting);
    death_no_hit(&waiting);
    safe_actual_expect(&state, session, [8.5, 64., 8.5]);
    let acquired = fixture.acquire(&mut state, key(Dimension::OVERWORLD, -2, 3));
    death_activated(&state, session, &acquired, [-31.5, 65., 48.5]);
    safe_actual_expect(&state, session, [8.5, 64., 8.5]);
    let following = state.advance_tick(TickBudget::full()).unwrap();
    assert!(local(&following).ready() && !local(&following).reset());
    assert_eq!(local(&following).last_input_sequence(), 3);
    let actor = recovery_observed(&state, session).0;
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert_eq!(actor.motion.position().get(), [-31.5, 65., 48.5]);
    assert!(actor.motion.on_ground());
    safe_actual_expect(&state, session, [-31.5, 65., 48.5]);
    death_no_hit(&following);
    death_conserved(&state, session);
    fixture.close();
}

#[test]
fn safe_actual_top_floor_accepts_out_of_height_head() {
    let mut save = height_player_save([8.5, 318., 8.5]);
    save.safe = None;
    let current = key(Dimension::OVERWORLD, 0, 0);
    let (mut fixture, mut state) = Fixture::new(
        Some(save),
        Dimension::OVERWORLD,
        ChunkPos::new(0, 0),
        vec![(current, height_floor(318))],
    );
    let (mut login, mut transport, connection, session, clock) =
        handshake(&mut fixture, &mut state);
    let acquired = fixture.acquire(&mut state, current);
    let (actor, runtime, _) = recovery_observed(&state, session);
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert_eq!(actor.motion.position().get(), [0.5, 319., 0.5]);
    assert!(actor.motion.on_ground());
    assert!(local(&acquired).ready() && local(&acquired).reset());
    assert!(!runtime.reset);
    assert_eq!(safe_actual_value(&state, session), None);
    let input = mornlea_protocol::ClientPacket::PlayerInput(
        mornlea_protocol::PlayerInput::new(3, 0, 0, false, 0., 0., false, false, false, false)
            .unwrap(),
    );
    assert!(!matches!(
        transport.send(
            connection,
            MemoryTransport::encode_frame(&input).unwrap(),
            &mut login.bind(&mut state, &mut fixture.store),
            &clock
        ),
        ConnectionProgress::Closed { .. }
    ));
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    let (actor, runtime, _) = recovery_observed(&state, session);
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert_eq!(actor.dimension, Dimension::OVERWORLD);
    assert_eq!(actor.motion.position().get(), [0.5, 319., 0.5]);
    assert!(actor.motion.on_ground());
    assert!(actor.motion.position().get()[1] + 1.8 > 320.);
    assert!(local(&publication).ready() && !local(&publication).reset());
    assert!(!runtime.reset);
    assert_eq!(local(&publication).last_input_sequence(), 3);
    death_no_hit(&publication);
    assert!(death_ground(&state).is_empty());
    safe_actual_expect(&state, session, [0.5, 319., 0.5]);
    fixture.close();
}

fn trample_actual_fixture(
    crop: bool,
    activation: bool,
) -> (Fixture, AuthorityState, SessionKey, TickPublication) {
    let position = [8.5, if activation { 63.9375 } else { 65. }, 8.5];
    let mut save = height_player_save(position);
    save.health = 1;
    save.safe = None;
    save.inventory.hotbar.selected = 3;
    save.inventory.hotbar.slots[3] = ItemStack {
        item: 1,
        count: 7,
        durability: 0,
    };
    assert!(!save.respawn_present);
    let current = key(Dimension::OVERWORLD, 0, 0);
    let mut chunk = height_floor(63);
    death_chunk_cell(&mut chunk, BlockPos::new(8, 63, 8), 35);
    if crop {
        death_chunk_cell(&mut chunk, BlockPos::new(8, 64, 8), 44);
    }
    let (mut fixture, mut state) = Fixture::new(
        Some(save),
        Dimension::DEPTHS,
        ChunkPos::new(-2, 3),
        vec![
            (current, chunk),
            (key(Dimension::OVERWORLD, -2, 3), height_floor(64)),
        ],
    );
    let (mut login, mut transport, connection, session, clock) =
        handshake(&mut fixture, &mut state);
    let publication = fixture.acquire(&mut state, current);
    assert!(local(&publication).ready() && local(&publication).reset());
    let (actor, runtime, inv) = recovery_observed(&state, session);
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert_eq!(actor.dimension, Dimension::OVERWORLD);
    assert_eq!(actor.motion.position().get(), position);
    assert!(!runtime.reset);
    assert_eq!(inv.slots[3].count, 7);
    assert_eq!(local(&publication).last_input_sequence(), 0);
    if !activation {
        let mut residents = state.residents();
        let player_key = ActorKey::Player(session);
        let actor = residents
            .actors
            .iter_mut()
            .find(|a| a.key == player_key)
            .unwrap();
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0., -40., 0.]).unwrap(),
            on_ground: false,
        });
        actor.survival = SurvivalState::try_new(SurvivalStateParts {
            health: 1,
            hunger: 20,
            oxygen: 300,
            saturation_zero: false,
            armor_points: 0,
        })
        .unwrap();
        let runtime = residents.runtimes.get_mut(&player_key).unwrap();
        runtime.controls = None;
        runtime.reset = false;
        runtime.peak_y = if crop { 68. } else { 67. };
        runtime.attack_cooldown = 0;
        runtime.hurt_cooldown = 0;
        runtime.burn_cooldown = 0;
        runtime.oxygen = 300;
        runtime.saturation_milli = 5000;
        runtime.exhaustion_milli = 0;
        runtime.since_damage_ticks = 0;
        runtime.starvation_ticks = 0;
        runtime.drown_ticks = 0;
        runtime.eating = None;
        runtime.bow = None;
        runtime.aux = ActorAux::Player {
            respawn: None,
            workbench: None,
        };
        state.commit_residents(residents);
    }
    let input = ClientPacket::PlayerInput(
        PlayerInput::new(3, 0, 0, false, 0., 0., false, false, false, false).unwrap(),
    );
    assert!(!matches!(
        transport.send(
            connection,
            MemoryTransport::encode_frame(&input).unwrap(),
            &mut login.bind(&mut state, &mut fixture.store),
            &clock
        ),
        ConnectionProgress::Closed { .. }
    ));
    (fixture, state, session, publication)
}
fn trample_actual_cells(state: &AuthorityState, ground: u16, crop: u16) {
    let view = state.settled_read().unwrap();
    assert_eq!(
        view.block(Dimension::OVERWORLD, BlockPos::new(8, 63, 8)),
        Some(ground)
    );
    assert_eq!(
        view.block(Dimension::OVERWORLD, BlockPos::new(8, 64, 8)),
        Some(crop)
    );
}
fn trample_actual_conserved(state: &AuthorityState, session: SessionKey) {
    trample_actual_cells(state, 3, 0);
    let inv = recovery_observed(state, session).2;
    assert_eq!(inv.selected.get(), 3);
    assert!(
        inv.slots
            .iter()
            .chain(inv.armor.iter())
            .chain(inv.crafting.iter())
            .all(|s| *s == ItemStack::default())
    );
    let mut totals = std::collections::BTreeMap::new();
    for drop in death_ground(state) {
        *totals.entry(drop.stack.item).or_insert(0u32) += u32::from(drop.stack.count);
        assert_eq!(drop.stack.durability, 0);
    }
    assert_eq!(
        totals,
        std::collections::BTreeMap::from([(1, 7), (35, 3), (34, 3)])
    );
}
fn trample_actual_pending(
    state: &AuthorityState,
    session: SessionKey,
    publication: &TickPublication,
) {
    pending(state, session, Dimension::OVERWORLD, [-31.5, 321., 48.5]);
    let (actor, runtime, _) = recovery_observed(state, session);
    assert_eq!(
        (
            actor.survival.health(),
            actor.survival.hunger(),
            actor.survival.oxygen()
        ),
        (20, 20, 300)
    );
    assert_eq!(
        (runtime.saturation_milli, runtime.exhaustion_milli),
        (5000, 0)
    );
    assert!(!runtime.reset);
    assert_eq!(local(publication).last_input_sequence(), 3);
    assert!(!local(publication).ready() && !local(publication).reset());
    trample_actual_conserved(state, session);
}
fn trample_actual_activation(
    state: &AuthorityState,
    session: SessionKey,
    publication: &TickPublication,
) {
    let (actor, runtime, _) = recovery_observed(state, session);
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert_eq!(actor.dimension, Dimension::OVERWORLD);
    assert_eq!(actor.motion.position().get(), [-31.5, 65., 48.5]);
    assert_eq!(actor.motion.velocity().get(), [0.; 3]);
    assert_eq!(
        (
            actor.survival.health(),
            actor.survival.hunger(),
            actor.survival.oxygen()
        ),
        (20, 20, 300)
    );
    assert!(local(publication).ready() && local(publication).reset());
    assert!(!runtime.reset);
    assert_eq!(local(publication).last_input_sequence(), 3);
    death_no_hit(publication);
    trample_actual_conserved(state, session);
}
#[test]
fn trample_actual_lethal_native_crop_landing() {
    let (mut fixture, mut state, session, _) = trample_actual_fixture(true, false);
    assert_eq!(state.next_tick(), 1);
    let tick = state.next_tick();
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    let hits = publication
        .events
        .iter()
        .filter(|e| matches!(e.event(), mornlea_domain::Event::CombatHit(_)))
        .collect::<Vec<_>>();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].recipient(),
        mornlea_domain::EventRecipient::Session(session.get())
    );
    let mornlea_domain::Event::CombatHit(hit) = hits[0].event() else {
        unreachable!()
    };
    assert_eq!(hit.damage(), 1);
    assert_eq!(hit.server_tick(), tick);
    safe_actual_expect(&state, session, [8.5, 63.9375, 8.5]);
    trample_actual_pending(&state, session, &publication);
    let drops = death_ground(&state);
    let waiting = state.advance_tick(TickBudget::full()).unwrap();
    trample_actual_pending(&state, session, &waiting);
    death_no_hit(&waiting);
    assert_eq!(death_ground(&state), drops);
    let acquired = fixture.acquire(&mut state, key(Dimension::OVERWORLD, -2, 3));
    trample_actual_activation(&state, session, &acquired);
    assert_eq!(death_ground(&state), drops);
    let following = state.advance_tick(TickBudget::full()).unwrap();
    assert!(local(&following).ready() && !local(&following).reset());
    assert_eq!(local(&following).last_input_sequence(), 3);
    death_no_hit(&following);
    trample_actual_conserved(&state, session);
    assert_eq!(death_ground(&state), drops);
    fixture.close();
}
#[test]
fn trample_actual_surviving_native_bare_landing() {
    let (mut fixture, mut state, session, _) = trample_actual_fixture(false, false);
    assert_eq!(state.next_tick(), 1);
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    let (actor, _, inv) = recovery_observed(&state, session);
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert_eq!(actor.survival.health(), 1);
    assert!(actor.motion.on_ground());
    assert_eq!(actor.motion.position().get(), [8.5, 63.9375, 8.5]);
    safe_actual_expect(&state, session, [8.5, 63.9375, 8.5]);
    assert_eq!(inv.slots[3].count, 7);
    assert_eq!(local(&publication).last_input_sequence(), 3);
    death_no_hit(&publication);
    assert!(death_ground(&state).is_empty());
    trample_actual_cells(&state, 3, 0);
    let following = state.advance_tick(TickBudget::full()).unwrap();
    let (actor, _, inv) = recovery_observed(&state, session);
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert_eq!(actor.survival.health(), 1);
    assert!(actor.motion.on_ground());
    assert_eq!(actor.motion.position().get(), [8.5, 64., 8.5]);
    assert_eq!(inv.slots[3].count, 7);
    assert_eq!(local(&following).last_input_sequence(), 3);
    assert!(!local(&following).reset());
    death_no_hit(&following);
    assert!(death_ground(&state).is_empty());
    trample_actual_cells(&state, 3, 0);
    fixture.close();
}
#[test]
fn trample_actual_activation_is_not_a_landing() {
    let (mut fixture, mut state, session, activated) = trample_actual_fixture(true, true);
    assert!(recovery_observed(&state, session).0.motion.on_ground());
    assert_eq!(safe_actual_value(&state, session), None);
    assert!(local(&activated).ready() && local(&activated).reset());
    trample_actual_cells(&state, 35, 44);
    assert!(death_ground(&state).is_empty());
    assert_eq!(recovery_observed(&state, session).2.slots[3].count, 7);
    assert_eq!(state.next_tick(), 1);
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    let (actor, runtime, inv) = recovery_observed(&state, session);
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert!(actor.motion.on_ground());
    assert_eq!(actor.motion.position().get(), [8.5, 63.9375, 8.5]);
    assert_eq!(inv.slots[3].count, 7);
    assert!(!runtime.reset);
    assert!(local(&publication).ready() && !local(&publication).reset());
    assert_eq!(local(&publication).last_input_sequence(), 3);
    safe_actual_expect(&state, session, [8.5, 63.9375, 8.5]);
    trample_actual_cells(&state, 35, 44);
    assert!(death_ground(&state).is_empty());
    death_no_hit(&publication);
    fixture.close();
}

fn snow_actual_fixture() -> (
    Fixture,
    AuthorityState,
    SessionKey,
    TickPublication,
    LoginDriver,
    MemoryTransport,
    ConnectionId,
    StepClock,
) {
    let mut save = height_player_save([8.1, 64., 8.5]);
    save.health = 20;
    save.safe = None;
    save.armor = Default::default();
    save.hunger = 20;
    save.saturation_milli = 5000;
    save.exhaustion_milli = 0;
    save.respawn_present = false;
    save.inventory.hotbar.selected = 3;
    save.inventory.hotbar.slots[3] = ItemStack {
        item: 1,
        count: 7,
        durability: 0,
    };
    let current = key(Dimension::OVERWORLD, 0, 0);
    let mut chunk = height_floor(63);
    for x in [8, 9] {
        death_chunk_cell(&mut chunk, BlockPos::new(x, 64, 8), 87);
    }
    let (mut f, mut state) = Fixture::new(
        Some(save),
        Dimension::DEPTHS,
        ChunkPos::new(-2, 3),
        vec![
            (current, chunk),
            (key(Dimension::OVERWORLD, -2, 3), height_floor(64)),
        ],
    );
    let (login, transport, connection, session, clock) = handshake(&mut f, &mut state);
    let publication = f.acquire(&mut state, current);
    let (actor, runtime, inv) = recovery_observed(&state, session);
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert_eq!(actor.dimension, Dimension::OVERWORLD);
    assert_eq!(actor.motion.position().get(), [8.1, 64., 8.5]);
    assert!(actor.motion.on_ground());
    assert!(!runtime.reset);
    assert_eq!(inv.slots[3].count, 7);
    assert!(local(&publication).ready() && local(&publication).reset());
    assert_eq!(local(&publication).last_input_sequence(), 0);
    (
        f,
        state,
        session,
        publication,
        login,
        transport,
        connection,
        clock,
    )
}
fn snow_actual_input(
    f: &mut Fixture,
    state: &mut AuthorityState,
    login: &mut LoginDriver,
    transport: &mut MemoryTransport,
    connection: ConnectionId,
    clock: &StepClock,
    sequence: u64,
) {
    let input = ClientPacket::PlayerInput(
        PlayerInput::new(sequence, 0, 0, false, 0., 0., false, false, false, false).unwrap(),
    );
    assert!(!matches!(
        transport.send(
            connection,
            MemoryTransport::encode_frame(&input).unwrap(),
            &mut login.bind(state, &mut f.store),
            clock
        ),
        ConnectionProgress::Closed { .. }
    ));
}
fn snow_actual_velocity(state: &mut AuthorityState, session: SessionKey) {
    let mut r = state.residents();
    let actor = r
        .actors
        .iter_mut()
        .find(|a| a.key == ActorKey::Player(session))
        .unwrap();
    let old = actor.motion;
    actor.motion = MotionState::new(MotionStateParts {
        position: old.position(),
        velocity: FiniteVec3::try_new([4., 0., 0.]).unwrap(),
        on_ground: old.on_ground(),
    });
    state.commit_residents(r);
}
fn snow_actual_cells(state: &AuthorityState) -> (u16, u16) {
    let view = state.settled_read().unwrap();
    (
        view.block(Dimension::OVERWORLD, BlockPos::new(8, 64, 8))
            .unwrap(),
        view.block(Dimension::OVERWORLD, BlockPos::new(9, 64, 8))
            .unwrap(),
    )
}
#[test]
fn snow_actual_native_travel_crosses_two_cells() {
    let (mut f, mut state, s, _, mut login, mut transport, connection, clock) =
        snow_actual_fixture();
    for tick in 1..=18 {
        snow_actual_velocity(&mut state, s);
        snow_actual_input(
            &mut f,
            &mut state,
            &mut login,
            &mut transport,
            connection,
            &clock,
            tick + 2,
        );
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        let (actor, runtime, inv) = recovery_observed(&state, s);
        assert!(actor.motion.on_ground());
        let p = actor.motion.position().get();
        assert_eq!((p[1], p[2]), (64., 8.5));
        assert_eq!(inv.slots[3].count, 7);
        assert!(!runtime.reset);
        assert!(local(&publication).ready() && !local(&publication).reset());
        assert_eq!(local(&publication).last_input_sequence(), tick + 2);
        death_no_hit(&publication);
        assert!(death_ground(&state).is_empty());
        if tick == 9 {
            assert_eq!(p[0].to_bits(), 0x410c6665);
            assert_eq!(snow_actual_cells(&state), (86, 87));
        }
        if tick == 18 {
            assert_eq!(p[0].to_bits(), 0x41173330);
            assert_eq!(snow_actual_cells(&state), (86, 86));
        }
    }
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(snow_actual_cells(&state), (86, 86));
    assert_eq!(local(&publication).last_input_sequence(), 20);
    death_no_hit(&publication);
    assert!(death_ground(&state).is_empty());
    f.close();
}
#[test]
fn snow_actual_lethal_landing_settles_original_cell() {
    let (mut f, mut state, s, _, mut login, mut transport, connection, clock) =
        snow_actual_fixture();
    for tick in 1..=8 {
        snow_actual_velocity(&mut state, s);
        snow_actual_input(
            &mut f,
            &mut state,
            &mut login,
            &mut transport,
            connection,
            &clock,
            tick + 2,
        );
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        death_no_hit(&publication);
        assert!(recovery_observed(&state, s).0.motion.on_ground());
    }
    let warm = snow_actual_cells(&state);
    let mut r = state.residents();
    let k = ActorKey::Player(s);
    let actor = r.actors.iter_mut().find(|a| a.key == k).unwrap();
    actor.motion = MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new([9.1, 65., 8.5]).unwrap(),
        velocity: FiniteVec3::try_new([4., -40., 0.]).unwrap(),
        on_ground: false,
    });
    actor.survival = SurvivalState::try_new(SurvivalStateParts {
        health: 1,
        hunger: 20,
        oxygen: 300,
        saturation_zero: false,
        armor_points: 0,
    })
    .unwrap();
    let runtime = r.runtimes.get_mut(&k).unwrap();
    runtime.peak_y = 68.;
    runtime.reset = false;
    runtime.controls = None;
    state.commit_residents(r);
    snow_actual_input(
        &mut f,
        &mut state,
        &mut login,
        &mut transport,
        connection,
        &clock,
        11,
    );
    assert_eq!(state.next_tick(), 9);
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    let hits: Vec<_> = publication
        .events
        .iter()
        .filter(|e| matches!(e.event(), mornlea_domain::Event::CombatHit(_)))
        .collect();
    assert_eq!(hits.len(), 1);
    let mornlea_domain::Event::CombatHit(hit) = hits[0].event() else {
        unreachable!()
    };
    assert_eq!((hit.damage(), hit.server_tick()), (1, 9));
    let safe = safe_actual_value(&state, s).unwrap();
    assert_eq!(safe.dimension, 0);
    assert!((9.0..10.0).contains(&safe.position[0]));
    assert_eq!((safe.position[1], safe.position[2]), (64., 8.5));
    let (actor, runtime, inv) = recovery_observed(&state, s);
    assert_eq!(actor.lifecycle, ActorLifecycle::Pending);
    assert_eq!(actor.dimension, Dimension::OVERWORLD);
    assert_eq!(actor.motion.position().get(), [-31.5, 321., 48.5]);
    assert_eq!(
        (
            actor.survival.health(),
            actor.survival.hunger(),
            actor.survival.oxygen()
        ),
        (20, 20, 300)
    );
    assert_eq!(
        (runtime.saturation_milli, runtime.exhaustion_milli),
        (5000, 0)
    );
    assert!(!runtime.reset);
    assert!(inv.slots.iter().all(|i| i.count == 0));
    assert_eq!(local(&publication).last_input_sequence(), 11);
    assert!(!local(&publication).ready() && !local(&publication).reset());
    assert_eq!(snow_actual_cells(&state).1, 86);
    assert_eq!(warm, (87, 87));
    let drops = death_ground(&state);
    assert_eq!(
        drops.iter().map(|d| d.stack).collect::<Vec<_>>(),
        vec![ItemStack {
            item: 1,
            count: 7,
            durability: 0
        }]
    );
    let waiting = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(snow_actual_cells(&state).1, 86);
    assert_eq!(death_ground(&state), drops);
    death_no_hit(&waiting);
    assert!(!local(&waiting).ready());
    assert_eq!(local(&waiting).last_input_sequence(), 11);
    let acquired = f.acquire(&mut state, key(Dimension::OVERWORLD, -2, 3));
    assert!(local(&acquired).ready() && local(&acquired).reset());
    assert_eq!(local(&acquired).last_input_sequence(), 11);
    let following = state.advance_tick(TickBudget::full()).unwrap();
    assert!(local(&following).ready() && !local(&following).reset());
    assert_eq!(local(&following).last_input_sequence(), 11);
    assert_eq!(snow_actual_cells(&state).1, 86);
    assert_eq!(death_ground(&state), drops);
    death_no_hit(&following);
    f.close();
}
#[test]
fn snow_actual_activation_is_quiet() {
    let (mut f, mut state, s, activated, mut login, mut transport, connection, clock) =
        snow_actual_fixture();
    assert_eq!(safe_actual_value(&state, s), None);
    assert_eq!(local(&activated).last_input_sequence(), 0);
    assert_eq!(snow_actual_cells(&state), (87, 87));
    assert!(death_ground(&state).is_empty());
    snow_actual_input(
        &mut f,
        &mut state,
        &mut login,
        &mut transport,
        connection,
        &clock,
        3,
    );
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    let (actor, runtime, inv) = recovery_observed(&state, s);
    assert!(actor.motion.on_ground());
    assert_eq!(actor.motion.position().get(), [8.1, 64., 8.5]);
    assert!(!runtime.reset);
    assert_eq!(inv.slots[3].count, 7);
    assert!(local(&publication).ready() && !local(&publication).reset());
    assert_eq!(local(&publication).last_input_sequence(), 3);
    safe_actual_expect(&state, s, [8.1, 64., 8.5]);
    assert_eq!(snow_actual_cells(&state), (87, 87));
    assert!(death_ground(&state).is_empty());
    death_no_hit(&publication);
    f.close();
}

use mornlea_protocol::TillSoil;

struct ActionCostFixture {
    fixture: Fixture,
    state: AuthorityState,
    session: SessionKey,
    login: LoginDriver,
    transport: MemoryTransport,
    connection: ConnectionId,
    clock: StepClock,
}
impl ActionCostFixture {
    fn new(saturation: u16, exhaustion: u16, held: ItemStack, mining_block: u16) -> Self {
        let mut save = height_player_save([8.5, 64., 8.5]);
        save.health = 20;
        save.hunger = 20;
        save.saturation_milli = saturation;
        save.exhaustion_milli = exhaustion;
        save.safe = None;
        save.armor = Default::default();
        save.respawn_present = false;
        save.inventory.hotbar.selected = 3;
        save.inventory.hotbar.slots[3] = held;
        let current = key(Dimension::OVERWORLD, 0, 0);
        let mut chunk = height_floor(63);
        death_chunk_cell(&mut chunk, BlockPos::new(8, 63, 6), 3);
        death_chunk_cell(&mut chunk, BlockPos::new(8, 64, 6), 0);
        death_chunk_cell(&mut chunk, BlockPos::new(8, 64, 5), mining_block);
        let (mut fixture, mut state) = Fixture::new(
            Some(save),
            Dimension::DEPTHS,
            ChunkPos::new(-2, 3),
            vec![
                (current, chunk),
                (key(Dimension::OVERWORLD, -2, 3), height_floor(64)),
            ],
        );
        let (mut login, transport, connection, session, clock) =
            handshake(&mut fixture, &mut state);
        let publication = fixture.acquire(&mut state, current);
        let (actor, runtime, inventory) = recovery_observed(&state, session);
        assert_eq!(actor.motion.position().get(), [8.5, 64., 8.5]);
        assert!(actor.motion.on_ground());
        assert!(!runtime.reset);
        assert_eq!(inventory.slots[3], held);
        assert!(local(&publication).ready() && local(&publication).reset());
        MemoryTransport::drain_session(
            &mut login.bind(&mut state, &mut fixture.store),
            session,
            512,
            1 << 20,
        )
        .unwrap();
        Self {
            fixture,
            state,
            session,
            login,
            transport,
            connection,
            clock,
        }
    }
    fn send(&mut self, packet: ClientPacket) {
        assert!(!matches!(
            self.transport.send(
                self.connection,
                MemoryTransport::encode_frame(&packet).unwrap(),
                &mut self.login.bind(&mut self.state, &mut self.fixture.store),
                &self.clock
            ),
            ConnectionProgress::Closed { .. }
        ));
    }
    fn input(&mut self, sequence: u64, mining: bool) {
        self.send(ClientPacket::PlayerInput(
            PlayerInput::new(sequence, 0, 0, false, 0., -0.5, mining, false, false, false).unwrap(),
        ));
    }
    fn till(&mut self) {
        self.send(ClientPacket::TillSoil(TillSoil::new(4, 0., -0.65).unwrap()));
    }
    fn tick(&mut self) -> TickPublication {
        self.state.advance_tick(TickBudget::full()).unwrap()
    }
    fn cell(&self, pos: BlockPos) -> u16 {
        self.state
            .settled_read()
            .unwrap()
            .block(Dimension::OVERWORLD, pos)
            .unwrap()
    }
    fn assert_cost(&mut self, p: &TickPublication, want: (u8, u32, u32), ack: u64) {
        let (a, r, _) = recovery_observed(&self.state, self.session);
        let ActorBody::Player(body) = &a.body else {
            panic!("player")
        };
        assert_eq!(
            (a.survival.hunger(), r.saturation_milli, r.exhaustion_milli),
            want,
            "late action scalar settlement"
        );
        assert_eq!(
            (
                body.hunger,
                u32::from(body.saturation_milli),
                u32::from(body.exhaustion_milli)
            ),
            want
        );
        assert_eq!(a.survival.saturation_zero(), want.1 == 0);
        let local = local(p);
        assert!(local.ready());
        assert_eq!(local.last_input_sequence(), ack);
        assert_eq!(local.survival().hunger(), want.0);
        assert_eq!(local.survival().saturation_zero(), want.1 == 0);
        let frames = MemoryTransport::drain_session(
            &mut self.login.bind(&mut self.state, &mut self.fixture.store),
            self.session,
            512,
            1 << 20,
        )
        .unwrap();
        let mut found = false;
        for bytes in frames {
            let frame = read_frame_ref(&bytes).unwrap();
            let packet = ProtocolCodec::new()
                .unwrap()
                .decode_server(State::Play, frame.packet_id, frame.payload)
                .unwrap();
            if let ServerPacket::PlayerState(wire) = packet {
                assert_eq!(wire.hunger, want.0);
                assert_eq!(wire.saturation_zero, want.1 == 0);
                found = true;
            }
        }
        assert!(found, "queued PlayerState");
    }
    fn no_drops(&self) {
        assert!(
            self.state
                .settled_read()
                .unwrap()
                .drops(key(Dimension::OVERWORLD, 0, 0))
                .is_empty()
        );
    }
    fn release_and_idle(&mut self, want: (u8, u32, u32), sequence: u64) {
        self.input(sequence, false);
        let p = self.tick();
        self.assert_cost(&p, want, sequence);
        assert!(
            self.state
                .settled_read()
                .unwrap()
                .mining(ActorKey::Player(self.session))
                .is_none()
        );
        let p = self.tick();
        self.assert_cost(&p, want, sequence);
    }
}
#[test]
fn action_costs_actual_late_till() {
    for saturation in [500, 0] {
        let mut f = ActionCostFixture::new(
            saturation,
            3999,
            ItemStack {
                item: 30,
                count: 1,
                durability: 1,
            },
            0,
        );
        f.input(3, false);
        f.till();
        let p = f.tick();
        assert_eq!(f.cell(BlockPos::new(8, 63, 6)), 35, "native Till target");
        assert_eq!(
            recovery_observed(&f.state, f.session).2.slots[3],
            ItemStack {
                item: 32,
                count: 1,
                durability: 0
            }
        );
        f.no_drops();
        let want = (if saturation == 0 { 19 } else { 20 }, 0, 4);
        f.assert_cost(&p, want, 3);
        let p = f.tick();
        f.assert_cost(&p, want, 3);
    }
}
#[test]
fn action_costs_actual_clear_only_mining() {
    let mut f = ActionCostFixture::new(
        500,
        3999,
        ItemStack {
            item: 30,
            count: 1,
            durability: 2,
        },
        85,
    );
    f.input(3, true);
    let p = f.tick();
    assert_eq!(f.cell(BlockPos::new(8, 64, 5)), 0, "native Snow target");
    assert_eq!(
        recovery_observed(&f.state, f.session).2.slots[3],
        ItemStack {
            item: 30,
            count: 1,
            durability: 1
        }
    );
    f.no_drops();
    f.assert_cost(&p, (20, 0, 4), 3);
    f.release_and_idle((20, 0, 4), 4);
}
#[test]
fn action_costs_actual_till_then_mining() {
    let mut f = ActionCostFixture::new(
        0,
        3994,
        ItemStack {
            item: 30,
            count: 1,
            durability: 3,
        },
        85,
    );
    f.input(3, true);
    f.till();
    let p = f.tick();
    assert_eq!(f.cell(BlockPos::new(8, 63, 6)), 35, "native Till target");
    assert_eq!(f.cell(BlockPos::new(8, 64, 5)), 0, "native Snow target");
    assert_eq!(
        recovery_observed(&f.state, f.session).2.slots[3],
        ItemStack {
            item: 30,
            count: 1,
            durability: 1
        }
    );
    f.no_drops();
    f.assert_cost(&p, (19, 0, 4), 3);
    f.release_and_idle((19, 0, 4), 5);
}
#[test]
fn action_costs_actual_refused_and_incomplete() {
    let held = ItemStack {
        item: 1,
        count: 7,
        durability: 0,
    };
    let mut f = ActionCostFixture::new(500, 3999, held, 2);
    f.input(3, true);
    f.till();
    let p = f.tick();
    assert_eq!(f.cell(BlockPos::new(8, 63, 6)), 3);
    assert_eq!(f.cell(BlockPos::new(8, 64, 5)), 2);
    assert_eq!(recovery_observed(&f.state, f.session).2.slots[3], held);
    f.no_drops();
    let view = f.state.settled_read().unwrap();
    let progress = view.mining(ActorKey::Player(f.session)).unwrap();
    assert_eq!((progress.elapsed, progress.required), (1, 30));
    f.assert_cost(&p, (20, 500, 3999), 3);
    f.release_and_idle((20, 500, 3999), 5);
}

// Passive source Snow capture (plan 103): the appended actual cases drive one
// staged cow through the same real seams as the player Snow recipes above —
// disk/Memory login, Acquire and `advance_tick` — along the frozen glide line.

// Mature wheat (`core.ItemWheat`, the 36th entry of the frozen item table in
// `packages/shared/core/item.go`).
const PASSIVE_SNOW_WHEAT: u16 = 35;
// The frozen graze-roll constants (`PassiveGrazeRollSalt` and
// `PassiveGrazePeriodTicks`, `packages/server/updates/sampler.go`).
const PASSIVE_SNOW_GRAZE_SALT: u64 = 0x51ab_3e4d_07c3_f291;
const PASSIVE_SNOW_GRAZE_PERIOD: u64 = 600;

// The cited integer mirrors of `tests/server_replay/passives.rs`
// (`Sampler.SplitMix64` and `Sampler.PassiveGrazeHit`).
fn passive_snow_splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}
fn passive_snow_graze_hit(seed: i64, tick: u64, id: u64) -> bool {
    let hash = passive_snow_splitmix64(
        passive_snow_splitmix64((seed as u64) ^ PASSIVE_SNOW_GRAZE_SALT) ^ tick,
    );
    passive_snow_splitmix64(hash ^ id) % PASSIVE_SNOW_GRAZE_PERIOD == 0
}
/// A cow id that misses every graze roll across the whole tested window, so
/// no tick freezes the glide. The seed is the fixture's staged Metadata seed
/// (42 in `options` above), never an assumed zero.
fn passive_snow_actual_cow_id() -> u64 {
    (1..)
        .find(|&id| (0..=20u64).all(|tick| !passive_snow_graze_hit(42, tick, id)))
        .expect("a graze-free cow id exists in the scan window")
}

/// One actual Snow scene on the frozen glide line: the accepted player fixture
/// plus a staged Active cow at [8.1, 64., 8.5] with velocity [4, 0, 0] each
/// frame. The stationary holder parks inside the 2.5 stop distance with wheat
/// selected, so the cow's intent stays neutral for the whole window (the glide
/// stays within 1.35 blocks) and the zero move input bypasses the shared Snow
/// slowdown exactly like the frozen player calibration; the player never
/// moves, so no player Snow sample double-writes the line.
fn passive_snow_actual_stage(aux: ActorAux) -> (Fixture, AuthorityState, SessionKey, ActorKey) {
    let (fixture, mut state, session, _publication, _login, _transport, _connection, _clock) =
        snow_actual_fixture();
    let id = passive_snow_actual_cow_id();
    let cow = ActorKey::Passive(PassiveId::try_new(id).unwrap());
    let position = [8.1, 64., 8.5];
    let velocity = [4., 0., 0.];
    let record = ActorRecord::try_new(
        cow,
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new(velocity).unwrap(),
            on_ground: true,
        }),
        LookAngles::try_new(0., 0.).unwrap(),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .unwrap(),
        ActorBody::Passive(PassiveMob {
            id,
            dimension: 0,
            position,
            velocity,
            on_ground: true,
            yaw: 0.,
            health: 20,
        }),
    )
    .unwrap();
    let runtime = ActorRuntime {
        key: cow,
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 0,
        peak_y: 0.,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux,
    };
    let mut residents = state.residents();
    let holder = residents
        .inventories
        .get_mut(&ActorKey::Player(session))
        .unwrap();
    holder.slots[3].item = PASSIVE_SNOW_WHEAT;
    residents.runtimes.insert(cow, runtime);
    residents.actors.push(record);
    state.commit_residents(residents);
    (fixture, state, session, cow)
}

/// Re-pins the cow's glide velocity each frame, mirroring the motion state
/// into the passive body like the accepted fixtures.
fn passive_snow_actual_velocity(state: &mut AuthorityState, cow: ActorKey) {
    let mut residents = state.residents();
    let actor = residents.actors.iter_mut().find(|a| a.key == cow).unwrap();
    let old = actor.motion;
    actor.motion = MotionState::new(MotionStateParts {
        position: old.position(),
        velocity: FiniteVec3::try_new([4., 0., 0.]).unwrap(),
        on_ground: old.on_ground(),
    });
    let ActorBody::Passive(body) = &mut actor.body else {
        unreachable!("passive body");
    };
    body.velocity = [4., 0., 0.];
    state.commit_residents(residents);
}
fn passive_snow_actual_cow(state: &AuthorityState, cow: ActorKey) -> ActorRecord {
    state.settled_read().unwrap().actor(cow).unwrap().clone()
}
fn passive_snow_actual_cells(state: &AuthorityState) -> (u16, u16) {
    snow_actual_cells(state)
}

/// Retained passive travel: eighteen real ticks at the frozen player-glide
/// stride (0.0749998093, each under the 0.6 threshold) accumulate in a
/// retained tracker, so the ninth tick samples the oracle foot cell and the
/// eighteenth the next one, with a quiet tick afterwards. RED today: the
/// per-tick generic tracker loses the travel and both cells stay 87 while
/// every native step stays under the threshold.
#[test]
fn passive_snow_actual_retained_travel_crosses_cell() {
    let (mut fixture, mut state, _session, cow) = passive_snow_actual_stage(ActorAux::Passive {
        home: BlockPos::new(8, 64, 8),
        flee_ticks: 0,
        flee_from: None,
        graze_ticks: 0,
        graze_at: None,
        fresh: false,
    });
    let mut previous = 8.1f32;
    let mut first_sample: Option<(u64, (u16, u16))> = None;
    for tick in 1..=18u64 {
        passive_snow_actual_velocity(&mut state, cow);
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        death_no_hit(&publication);
        let actor = passive_snow_actual_cow(&state, cow);
        assert_eq!(actor.lifecycle, ActorLifecycle::Active, "tick {tick}");
        assert!(actor.motion.on_ground(), "tick {tick}");
        let p = actor.motion.position().get();
        assert_eq!((p[1], p[2]), (64., 8.5), "tick {tick}");
        let step = p[0] - previous;
        assert!(
            step > 0. && step < 0.6,
            "tick {tick}: native stride {step} must stay under the threshold"
        );
        assert!((8.0..10.0).contains(&p[0]), "tick {tick}: {p:?}");
        let ActorBody::Passive(body) = &actor.body else {
            unreachable!("passive body");
        };
        assert_eq!(body.position, p, "tick {tick}");
        previous = p[0];
        let cells = passive_snow_actual_cells(&state);
        if cells != (87, 87) && first_sample.is_none() {
            first_sample = Some((tick, cells));
        }
        if tick == 9 {
            // Intended RED: the retained stride must sample the oracle cell
            // while every per-tick step stays below the threshold.
            assert_eq!(
                cells,
                (86, 87),
                "retained passive Snow travel lost at tick {tick}"
            );
            // Player-glide oracle bits, valid iff the neutral-intent cow
            // stride matches the frozen player calibration (host run
            // confirms or corrects the literal).
            assert_eq!(p[0].to_bits(), 0x410c6665, "tick {tick}");
        }
    }
    assert_eq!(first_sample, Some((9, (86, 87))));
    assert_eq!(passive_snow_actual_cells(&state), (86, 86));
    assert_eq!(previous.to_bits(), 0x41173330);
    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    death_no_hit(&quiet);
    assert_eq!(passive_snow_actual_cells(&state), (86, 86));
    fixture.close();
}

/// Pre-death capture: the movement pass runs before the same-phase death
/// settlement, so the lethal tick's post-physics destination cell is captured
/// and settles in the late region beside the beef loot, and the following
/// quiet tick writes nothing more. RED today: the late generic collector
/// demands an Active row and the moved cell never settles.
#[test]
fn passive_snow_actual_death_settles_captured_cell() {
    let (mut fixture, mut state, _session, cow) = passive_snow_actual_stage(ActorAux::Passive {
        home: BlockPos::new(8, 64, 8),
        flee_ticks: 0,
        flee_from: None,
        graze_ticks: 0,
        graze_at: None,
        fresh: false,
    });
    for _tick in 1..=8u64 {
        passive_snow_actual_velocity(&mut state, cow);
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        death_no_hit(&publication);
        assert!(passive_snow_actual_cow(&state, cow).motion.on_ground());
    }
    // Eight strides warm the tracker below the threshold without a sample.
    assert_eq!(passive_snow_actual_cells(&state), (87, 87));
    // Lethal health staged while the cow is still Active: movement still runs
    // before the same-phase deaths, the ninth stride crosses, and the
    // captured destination settles after the settlement.
    {
        let mut residents = state.residents();
        let actor = residents.actors.iter_mut().find(|a| a.key == cow).unwrap();
        let old = actor.motion;
        actor.motion = MotionState::new(MotionStateParts {
            position: old.position(),
            velocity: FiniteVec3::try_new([4., 0., 0.]).unwrap(),
            on_ground: old.on_ground(),
        });
        actor.survival = SurvivalState::try_new(SurvivalStateParts {
            health: 0,
            hunger: 20,
            oxygen: 300,
            saturation_zero: false,
            armor_points: 0,
        })
        .unwrap();
        state.commit_residents(residents);
    }
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    death_no_hit(&publication);
    let actor = passive_snow_actual_cow(&state, cow);
    assert_eq!(actor.lifecycle, ActorLifecycle::Dead);
    let p = actor.motion.position().get();
    assert_eq!((p[1], p[2]), (64., 8.5));
    assert!((8.0..9.0).contains(&p[0]), "captured destination {p:?}");
    // The captured destination cell settles beside the fixed beef loot.
    assert!(!death_ground(&state).is_empty());
    assert_eq!(
        passive_snow_actual_cells(&state),
        (86, 87),
        "the pre-death captured cell must settle in the late region"
    );
    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    death_no_hit(&quiet);
    assert_eq!(passive_snow_actual_cells(&state), (86, 87));
    fixture.close();
}

/// Deterministic boundary pins around the capture: a fresh runtime tick and a
/// mid-graze tick displace nothing, and a home far outside the neighborhood
/// rolls every pinned stride back, so no cell changes while the cow stays
/// resident. These pin GREEN on both sides of the implementation.
#[test]
fn passive_snow_actual_boundary_pins() {
    // Fresh runtime: the first tick skips movement outright.
    {
        let (mut fixture, mut state, _session, cow) =
            passive_snow_actual_stage(ActorAux::Passive {
                home: BlockPos::new(8, 64, 8),
                flee_ticks: 0,
                flee_from: None,
                graze_ticks: 0,
                graze_at: None,
                fresh: true,
            });
        passive_snow_actual_velocity(&mut state, cow);
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        death_no_hit(&publication);
        assert_eq!(
            passive_snow_actual_cow(&state, cow).motion.position().get(),
            [8.1, 64., 8.5]
        );
        assert_eq!(passive_snow_actual_cells(&state), (87, 87));
        fixture.close();
    }
    // Graze freeze: a mid-graze cow holds its pose for the tick.
    {
        let (mut fixture, mut state, _session, cow) =
            passive_snow_actual_stage(ActorAux::Passive {
                home: BlockPos::new(8, 64, 8),
                flee_ticks: 0,
                flee_from: None,
                graze_ticks: 4,
                graze_at: Some(BlockPos::new(8, 64, 8)),
                fresh: false,
            });
        passive_snow_actual_velocity(&mut state, cow);
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        death_no_hit(&publication);
        assert_eq!(
            passive_snow_actual_cow(&state, cow).motion.position().get(),
            [8.1, 64., 8.5]
        );
        assert_eq!(passive_snow_actual_cells(&state), (87, 87));
        fixture.close();
    }
    // Home rollback: a home far outside the neighborhood restores the
    // pre-step pose every tick, so nothing samples across repeated strides.
    {
        let (mut fixture, mut state, _session, cow) =
            passive_snow_actual_stage(ActorAux::Passive {
                home: BlockPos::new(1000, 64, 8),
                flee_ticks: 0,
                flee_from: None,
                graze_ticks: 0,
                graze_at: None,
                fresh: false,
            });
        for _tick in 0..4 {
            passive_snow_actual_velocity(&mut state, cow);
            let publication = state.advance_tick(TickBudget::full()).unwrap();
            death_no_hit(&publication);
            assert_eq!(
                passive_snow_actual_cow(&state, cow).motion.position().get(),
                [8.1, 64., 8.5]
            );
        }
        assert_eq!(passive_snow_actual_cells(&state), (87, 87));
        fixture.close();
    }
}
