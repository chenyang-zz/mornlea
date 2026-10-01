//! Real Memory login and native tick feed one background disk save recipe.
//! This does not accept live actor targets or authoritative save ACK policy.
use mornlea_domain::{ChunkPos, Dimension, Identities, PlayerId};
use mornlea_protocol::{
    ClientHello, ClientPacket, LoginStart, PlayerInput, ProtocolCodec, SelectHotbar, ServerPacket,
    State, encode_uvarint, read_frame_ref,
};
use mornlea_server::core::{
    acquisition::LiveChunkPhase, actor_projection::project_player, chunk_driver::ChunkDriver,
    generation_worker::GenerationPool,
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
    PlayerLocation, PlayerSave, StorageKind, StoredPlayer, player_encoded_len,
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
            "mornlea-actor-projection-{}-{}",
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
fn key() -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    }
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
        inventory: Inventory::default(),
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
    generation: GenerationPool,
    store: AutosaveScheduler<DiskStore>,
    root: Root,
}
impl Fixture {
    fn new() -> (Self, AuthorityState) {
        let root = Root::new();
        let mut disk = DiskStore::open(&root.0, options()).unwrap();
        let player = saved_player();
        let ticket = SaveTicket::try_from_raw(1).unwrap();
        let snapshots = vec![
            OwnedSnapshot::try_new(
                SaveKey::Player(self::player()),
                player.revision,
                player_encoded_len(&player).unwrap(),
                SaveUrgency::Autosave,
                SaveValue::Player(player),
            )
            .unwrap(),
            OwnedSnapshot::try_new(
                SaveKey::Chunk(key()),
                9,
                1,
                SaveUrgency::Autosave,
                SaveValue::Chunk(ChunkSave {
                    key: mornlea_storage::ChunkKey {
                        dimension: 0,
                        x: 0,
                        z: 0,
                    },
                    revision: 9,
                    chunk: air(),
                }),
            )
            .unwrap(),
        ];
        let completion = disk.write(
            ticket,
            SaveRequest {
                snapshots: snapshots.clone(),
            },
        );
        assert_eq!(completion.ticket, ticket);
        assert_eq!(completion.snapshots, snapshots);
        assert_eq!(completion.error, None);
        assert_eq!(
            completion.committed,
            vec![
                (SaveKey::Chunk(key()), 9),
                (SaveKey::Player(self::player()), 9)
            ]
        );
        let state = AuthorityState::try_new_with_metadata(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            disk.metadata().clone(),
        )
        .unwrap();
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
                generation: GenerationPool::try_new(42, false, 1).unwrap(),
                store,
                root,
            },
            state,
        )
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

#[test]
fn actual_player_pose_and_inventory_projection_survive_disk_reopen() {
    let (mut fixture, mut state) = Fixture::new();
    let original = saved_player();
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
        &mut login.bind(&mut state, &mut fixture.store),
        &clock,
    );
    let hello = decode(&receive(&mut transport, connection), State::Handshake);
    assert_eq!(
        hello,
        ServerPacket::ServerHello(
            mornlea_protocol::ServerHello::new(Identities::current().protocol).unwrap()
        )
    );
    transport.acknowledge(
        connection,
        1,
        &mut login.bind(&mut state, &mut fixture.store),
    );
    let start = LoginStart::new(player(), "Ada", 8).unwrap();
    let start =
        ClientPacket::LoginStart(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap());
    transport.send(
        connection,
        MemoryTransport::encode_frame(&start).unwrap(),
        &mut login.bind(&mut state, &mut fixture.store),
        &clock,
    );
    let until = Instant::now() + BOUND;
    let (session, success) = loop {
        fixture.store.drive_workers();
        transport.poll(
            connection,
            &mut login.bind(&mut state, &mut fixture.store),
            &clock,
        );
        match login
            .bind(&mut state, &mut fixture.store)
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
        &mut login.bind(&mut state, &mut fixture.store),
        &clock,
    );
    assert_eq!(
        decode(&receive(&mut transport, connection), State::Login),
        success
    );
    assert!(matches!(success, ServerPacket::LoginSuccess(_)));
    transport.acknowledge(
        connection,
        1,
        &mut login.bind(&mut state, &mut fixture.store),
    );
    assert_eq!(login.pending(), 0);
    let active = state.session(session).unwrap();
    assert_eq!(active.phase, SessionPhase::Active);
    assert_eq!(active.player_id, player());

    state.enable_live_chunks().unwrap();
    state.replace_chunk_wants(BTreeSet::from([key()])).unwrap();
    let mut chunks = ChunkDriver::new();
    chunks
        .start_load(&mut state, &mut fixture.store, key(), deadline())
        .unwrap();
    let until = Instant::now() + BOUND;
    loop {
        fixture.store.drive_workers();
        let report = chunks.poll(&mut state, &mut fixture.store, &mut fixture.generation);
        assert_eq!(report.first_error, None);
        if report.retained == 0 {
            break;
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    assert_eq!(chunks.pending_generations(), 0);
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.live_chunk_facts(key()).unwrap().phase,
        LiveChunkPhase::Ready
    );
    assert_eq!(state.live_chunk_facts(key()).unwrap().persisted_revision, 9);
    let actor_key = ActorKey::Player(session);
    // End the immutable authority loan before transport intake and tick execution.
    let old = {
        let before = state.settled_read().unwrap();
        let pre = before.actor(actor_key).unwrap();
        assert_eq!((pre.look.yaw(), pre.look.pitch()), (0.1, 0.2));
        assert_eq!(before.inventory(actor_key).unwrap().selected.get(), 0);
        pre.motion.position().get()
    };
    let frames = transport.receive(connection, 64, 1 << 20);
    transport.acknowledge(
        connection,
        frames.len(),
        &mut login.bind(&mut state, &mut fixture.store),
    );
    let earlier = MemoryTransport::drain_session(
        &mut login.bind(&mut state, &mut fixture.store),
        session,
        512,
        1 << 20,
    )
    .unwrap();
    assert!(!earlier.is_empty());
    for packet in [
        ClientPacket::PlayerInput(
            PlayerInput::new(1, 1, 0, false, 1.2, 0.3, false, false, false, false).unwrap(),
        ),
        ClientPacket::SelectHotbar(SelectHotbar::new(2, 5).unwrap()),
    ] {
        let progress = transport.send(
            connection,
            MemoryTransport::encode_frame(&packet).unwrap(),
            &mut login.bind(&mut state, &mut fixture.store),
            &clock,
        );
        assert!(!matches!(progress, ConnectionProgress::Closed { .. }));
    }
    transport.poll(
        connection,
        &mut login.bind(&mut state, &mut fixture.store),
        &clock,
    );
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(publication.counters.commands, 2);
    assert_eq!(state.session(session).unwrap().last_applied_sequence, 2);
    // Project current owners while the healthy committed authority is borrowed.
    let projected = {
        let settled = state.settled_read().unwrap();
        assert_eq!(
            settled.world_time(),
            settled.environment().unwrap().world_time
        );
        assert_eq!(settled.tick(), state.next_tick());
        let actor = settled.actor(actor_key).unwrap();
        let inventory = settled.inventory(actor_key).unwrap();
        let runtime = settled.runtime(actor_key).unwrap();
        let position = actor.motion.position().get();
        assert!(
            position[0] != old[0] || position[2] != old[2],
            "PlayerInput must cause native horizontal motion beyond PRE-COMMAND pose"
        );
        assert_eq!((actor.look.yaw(), actor.look.pitch()), (1.2, 0.3));
        assert_eq!(inventory.selected.get(), 5);
        let ActorBody::Player(stale) = &actor.body else {
            panic!("actual player body")
        };
        assert_eq!(stale.current, original.current);
        assert_eq!(stale.inventory.hotbar.selected, 0);
        let projected =
            project_player(actor, Some(inventory), Some(runtime), original.revision + 1).unwrap();
        assert_eq!(
            projected.current.dimension,
            i32::from(actor.dimension.get())
        );
        assert_eq!(projected.current.position, position);
        assert_eq!((projected.yaw, projected.pitch), (1.2, 0.3));
        assert_eq!(projected.inventory.hotbar.selected, 5);
        assert_eq!(projected.inventory.hotbar.slots, inventory.slots[..9]);
        assert_eq!(projected.inventory.backpack, inventory.slots[9..]);
        assert_eq!(projected.armor, inventory.armor);
        assert_eq!(projected.health, actor.survival.health());
        assert_eq!(projected.hunger, actor.survival.hunger());
        assert_eq!(
            projected.saturation_milli,
            u16::try_from(runtime.saturation_milli).unwrap()
        );
        assert_eq!(
            projected.exhaustion_milli,
            u16::try_from(runtime.exhaustion_milli).unwrap()
        );
        assert_eq!(projected.player_id, original.player_id);
        assert_eq!(projected.display_name, original.display_name);
        assert_eq!(projected.safe, original.safe);
        let ActorAux::Player { respawn, .. } = &runtime.aux else {
            panic!("actual player aux")
        };
        let expected_respawn = respawn
            .map(|(dim, pos)| {
                (
                    true,
                    i32::from(dim.get()),
                    [pos.x() as f32, pos.y() as f32, pos.z() as f32],
                )
            })
            .unwrap_or((false, 0, [0.0; 3]));
        assert_eq!(
            (
                projected.respawn_present,
                projected.respawn_dimension,
                projected.respawn_position
            ),
            expected_respawn
        );
        projected
    };
    let snapshot = OwnedSnapshot::try_new(
        SaveKey::Player(player()),
        projected.revision,
        player_encoded_len(&projected).unwrap(),
        SaveUrgency::Autosave,
        SaveValue::Player(projected.clone()),
    )
    .unwrap();
    let ticket = fixture
        .store
        .submit(SaveRequest {
            snapshots: vec![snapshot.clone()],
        })
        .unwrap();
    let until = Instant::now() + BOUND;
    let completion = loop {
        fixture.store.drive_workers();
        match StoreHandle::poll(&mut fixture.store, ticket) {
            SavePoll::Completed(v) => break v,
            SavePoll::Pending => {
                assert!(Instant::now() < until);
                thread::yield_now();
            }
        }
    };
    assert_eq!(completion.ticket, ticket);
    assert_eq!(completion.snapshots, vec![snapshot]);
    assert_eq!(
        completion.submitted,
        vec![(SaveKey::Player(player()), projected.revision)]
    );
    assert_eq!(completion.committed, completion.submitted);
    assert_eq!(completion.error, None);
    // Direct provider completion proves disk flow, not AuthorityState actor ACK.
    fixture.close();
    let mut reopened = DiskStore::open(&fixture.root.0, options()).unwrap();
    let loaded = reopened.load(SaveKey::Player(player())).unwrap();
    reopened.close().unwrap();
    let LoadedValue::Player(loaded) = loaded else {
        panic!("actual player reopen")
    };
    assert_eq!(
        loaded,
        StoredPlayer {
            player_id: projected.player_id,
            revision: projected.revision,
            display_name: projected.display_name,
            current: projected.current,
            yaw: projected.yaw,
            pitch: projected.pitch,
            safe: projected.safe,
            inventory: projected.inventory,
            health: projected.health,
            hunger: projected.hunger,
            saturation_milli: projected.saturation_milli,
            exhaustion_milli: projected.exhaustion_milli,
            respawn_present: projected.respawn_present,
            respawn_position: projected.respawn_position,
            respawn_dimension: projected.respawn_dimension,
            armor: projected.armor,
            needs_rewrite: false
        }
    );
}
