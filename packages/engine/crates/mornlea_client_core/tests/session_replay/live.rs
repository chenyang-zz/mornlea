//! The public endpoint over the real bounded memory transport and wire codecs.

use std::{num::NonZeroU64, sync::Arc, time::Duration};

use mornlea_client_core::{
    ClientConfig, ClientCore, ClientEndpoint, ClientIdentity, ClientLimits, ClientWorkBudget,
    ConnectorRegistry, Endpoint, SessionEpoch, StdMonotonicClock,
    session::io::{MemoryConnector, MemoryPeer},
};
use mornlea_domain::{Dimension, Identities, PlayerId};
use mornlea_protocol::{
    ClientPacket, ForgetChunks, KeepAlive, LoginStart, LoginSuccess, ServerHello, ServerPacket,
    State, read_frame_ref,
};

struct Connection {
    core: ClientCore,
    peer: MemoryPeer,
    epoch: SessionEpoch,
}

impl Connection {
    fn new() -> Self {
        Self::with_limits(ClientLimits::try_new().unwrap())
    }

    fn with_limits(limits: ClientLimits) -> Self {
        let id = NonZeroU64::new(1).unwrap();
        let (connector, peer) = MemoryConnector::pair(limits, id);
        let mut connectors = ConnectorRegistry::new();
        connectors.register(id, connector).unwrap();
        let config = ClientConfig::try_new(
            limits,
            Duration::from_secs(5),
            Duration::from_secs(10),
            Arc::new(StdMonotonicClock),
            Arc::new(connectors),
        )
        .unwrap();
        let mut core = ClientCore::new(config).unwrap();
        let player =
            PlayerId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 1])
                .unwrap();
        let identity =
            ClientIdentity::try_new(LoginStart::new(player, "live", 2).unwrap()).unwrap();
        let epoch = core
            .connect(Endpoint::Memory { connector_id: id }, identity)
            .unwrap();
        peer.drain_sent();
        let mut connection = Self { core, peer, epoch };
        connection.feed(ServerPacket::ServerHello(
            ServerHello::new(Identities::current().protocol).unwrap(),
        ));
        connection.step();
        connection.peer.drain_sent();
        connection.feed(ServerPacket::LoginSuccess(LoginSuccess::new(player, 42)));
        connection.step();
        connection
    }

    fn feed(&self, packet: ServerPacket) {
        let mut codec = mornlea_protocol::ProtocolCodec::new().unwrap();
        let mut frame = vec![0; 2 << 20];
        let length = codec.encode_server_into(&packet, &mut frame).unwrap();
        self.peer
            .feed(&mornlea_protocol::write_frame(packet.key().id, &frame[..length]).unwrap())
            .unwrap();
    }

    fn step(&mut self) {
        self.core
            .step(self.epoch, ClientWorkBudget::try_new(8, 0).unwrap())
            .unwrap();
    }
}

#[test]
fn real_login_publishes_the_complete_family_contract() {
    let connection = Connection::new();
    assert_eq!(
        connection
            .core
            .snapshot(connection.epoch)
            .unwrap()
            .families()
            .len(),
        10
    );
}

#[test]
fn real_play_events_advance_and_consume_distinct_confirmed_revisions() {
    let mut connection = Connection::new();
    for x in [0, 1] {
        connection.feed(ServerPacket::ForgetChunks(
            ForgetChunks::new(Dimension::OVERWORLD, vec![(x, 0)]).unwrap(),
        ));
    }
    connection.step();
    assert_eq!(
        connection
            .core
            .snapshot(connection.epoch)
            .unwrap()
            .confirmed_revision()
            .get(),
        2
    );
    let report = connection
        .core
        .step(connection.epoch, ClientWorkBudget::try_new(0, 0).unwrap())
        .unwrap();
    assert_eq!(report.pending_inbound(), 0);
    assert_eq!(report.confirmed_revision().get(), 2);
}

#[test]
fn real_keepalive_returns_a_codec_checked_reply() {
    let mut connection = Connection::new();
    connection.feed(ServerPacket::KeepAlive(KeepAlive::new(123).unwrap()));
    connection.step();
    connection.step();
    let frames = connection.peer.drain_sent();
    assert_eq!(frames.len(), 1);
    let frame = read_frame_ref(&frames[0]).unwrap();
    let reply =
        mornlea_protocol::decode_client(State::Play, frame.packet_id, frame.payload).unwrap();
    assert!(matches!(reply, ClientPacket::KeepAliveReply(reply) if reply.token == 123));
    assert_eq!(
        connection
            .core
            .snapshot(connection.epoch)
            .unwrap()
            .confirmed_revision()
            .get(),
        0
    );
}

fn action(epoch: SessionEpoch) -> mornlea_client_core::InputBatch {
    use mornlea_client_core::input::{ClientIntent, InputAction, InputBatch};
    use mornlea_domain::{HeldActions, LookAngles, Movement, PlayerControl, PlayerControlParts};
    InputBatch::try_new(
        epoch,
        vec![InputAction {
            intent: ClientIntent::PlayerInput(PlayerControl::new(PlayerControlParts {
                movement: Movement {
                    move_x: 1,
                    move_z: 0,
                    jump: false,
                },
                look: LookAngles::try_new(0.0, 0.0).unwrap(),
                actions: HeldActions {
                    primary: false,
                    eating: false,
                    sprinting: false,
                    sneaking: false,
                },
            })),
            container: None,
            crafting: None,
        }],
    )
    .unwrap()
}

fn authority(tick: u64, ack: u64, dimension: Dimension) -> ServerPacket {
    ServerPacket::PlayerState(
        mornlea_protocol::PlayerState::new(
            tick,
            ack,
            dimension,
            [3.5, 2.0, 5.5],
            [0.0; 3],
            0.0,
            0.0,
            true,
            true,
            false,
            false,
            mornlea_protocol::BlockPos::ZERO,
            0,
            0,
            false,
            20,
            300,
            20,
            false,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        )
        .unwrap(),
    )
}

#[test]
fn real_input_sends_once_zero_work_retains_and_authority_acknowledges() {
    use mornlea_client_core::presentation::FamilyRecords;
    let mut connection = Connection::new();
    connection
        .core
        .submit_input(connection.epoch, action(connection.epoch))
        .unwrap();
    let report = connection
        .core
        .step(connection.epoch, ClientWorkBudget::try_new(0, 0).unwrap())
        .unwrap();
    assert_eq!(report.pending_input(), 1);
    assert!(connection.peer.drain_sent().is_empty());
    connection.step();
    let sent = connection.peer.drain_sent();
    assert_eq!(sent.len(), 1);
    let frame = read_frame_ref(&sent[0]).unwrap();
    assert!(
        matches!(mornlea_protocol::decode_client(State::Play, frame.packet_id, frame.payload).unwrap(), ClientPacket::PlayerInput(input) if input.sequence == 1)
    );
    connection.feed(authority(1, 1, Dimension::DEPTHS));
    connection.step();
    let frame = connection.core.snapshot(connection.epoch).unwrap();
    assert!(frame.families().iter().any(|family| matches!(family.records(), FamilyRecords::PlayerView(records) if !records.is_empty())));
    connection.step();
    assert!(connection.peer.drain_sent().is_empty());
}

#[test]
fn future_acknowledgment_is_rejected_without_poisoning_the_next_frame() {
    let mut connection = Connection::new();
    connection.feed(authority(1, 1, Dimension::OVERWORLD));
    connection.step();
    assert_eq!(
        connection
            .core
            .snapshot(connection.epoch)
            .unwrap()
            .confirmed_revision()
            .get(),
        0
    );
    connection.feed(authority(1, 0, Dimension::OVERWORLD));
    connection.step();
    assert_eq!(
        connection
            .core
            .snapshot(connection.epoch)
            .unwrap()
            .confirmed_revision()
            .get(),
        1
    );
}

#[test]
fn duplicate_rejection_does_not_stall_the_real_session() {
    let mut connection = Connection::new();
    connection
        .core
        .submit_input(connection.epoch, action(connection.epoch))
        .unwrap();
    connection.step();
    for _ in 0..2 {
        connection.feed(ServerPacket::CommandRejected(
            mornlea_protocol::CommandRejected::new(1, 7).unwrap(),
        ));
        connection.step();
    }
    assert_eq!(
        connection
            .core
            .snapshot(connection.epoch)
            .unwrap()
            .confirmed_revision()
            .get(),
        2
    );
}

fn stone_snapshot(dimension: Dimension) -> ServerPacket {
    use mornlea_domain::{ChunkPos, ChunkSnapshot, ChunkSnapshotParts, Event, PalettedSection};
    let mut sections = Box::new(std::array::from_fn(|_| PalettedSection::single(0).unwrap()));
    let mut words = vec![0; 1024];
    let index = (8 * 16 + 8) * 16 + 8;
    words[index / 4] = 2u64 << ((index % 4) * 15);
    sections[4] = PalettedSection::direct(words.into_boxed_slice()).unwrap();
    ServerPacket::try_from(Event::ChunkSnapshot(
        ChunkSnapshot::try_new(ChunkSnapshotParts {
            dimension,
            chunk: ChunkPos::new(0, 0),
            revision: 1,
            sections,
        })
        .unwrap(),
    ))
    .unwrap()
}

fn resource_keys(
    connection: &Connection,
) -> Vec<mornlea_client_core::preparation::PreparedResourceKey> {
    use mornlea_client_core::presentation::FamilyRecords;
    connection
        .core
        .snapshot(connection.epoch)
        .unwrap()
        .families()
        .iter()
        .flat_map(|family| {
            if let FamilyRecords::Terrain(records) = family.records() {
                records
                    .iter()
                    .filter_map(|record| record.resource().copied())
                    .collect()
            } else {
                Vec::new()
            }
        })
        .collect()
}

#[test]
fn public_core_owns_actual_near_and_far_geometry_and_retires_after_forget_reset() {
    use mornlea_client_core::{ClientError, preparation::PreparedGeometry};
    let mut connection = Connection::new();
    connection.feed(stone_snapshot(Dimension::OVERWORLD));
    connection.feed(authority(1, 0, Dimension::OVERWORLD));
    let report = connection
        .core
        .step(connection.epoch, ClientWorkBudget::try_new(8, 24).unwrap())
        .unwrap();
    assert_eq!(report.processed_meshes(), 24);
    let near = resource_keys(&connection).into_iter().find(|key| {
        matches!(connection.core.prepared_resource(key).as_deref(), Ok(PreparedGeometry::Near(quads)) if !quads.is_empty())
    }).expect("real nonempty near geometry");
    assert_eq!(connection.core.prepared_resource(&near).unwrap().quads(), 6);
    for _ in 0..3 {
        connection
            .core
            .step(connection.epoch, ClientWorkBudget::try_new(0, 1).unwrap())
            .unwrap();
    }
    let far = resource_keys(&connection).into_iter().find(|key| {
        matches!(connection.core.prepared_resource(key).as_deref(), Ok(PreparedGeometry::Far(quads)) if !quads.is_empty())
    }).expect("real seed-derived far geometry");
    connection.feed(ServerPacket::ForgetChunks(
        ForgetChunks::new(Dimension::OVERWORLD, vec![(0, 0)]).unwrap(),
    ));
    connection.step();
    assert_eq!(
        connection.core.prepared_resource(&near).err(),
        Some(ClientError::InvalidInput)
    );
    let previous = connection.epoch;
    connection.epoch = connection.core.reset(previous).unwrap();
    assert_eq!(
        connection.core.prepared_resource(&far).err(),
        Some(ClientError::StaleEpoch)
    );
    assert_eq!(
        connection.core.snapshot(previous).err(),
        Some(ClientError::StaleEpoch)
    );
}

#[test]
fn control_and_input_share_the_selected_outbound_capacity() {
    let base = ClientLimits::try_new().unwrap();
    let limits = ClientLimits::try_new_with(
        base.queued_input_events(),
        base.inbound_observations(),
        base.inbound_bytes(),
        1,
        base.outbound_bytes(),
        base.prediction_journal(),
        base.message_work(),
        base.mesh_work(),
        base.preparation_results(),
        base.preparation_bytes(),
        base.family_records(),
        base.frame_bytes(),
    )
    .unwrap();
    let mut connection = Connection::with_limits(limits);
    connection.feed(ServerPacket::KeepAlive(KeepAlive::new(7).unwrap()));
    connection.step();
    assert_eq!(
        connection
            .core
            .submit_input(connection.epoch, action(connection.epoch)),
        Err(mornlea_client_core::ClientError::Capacity)
    );
    connection.step();
    assert!(matches!(
        connection
            .core
            .submit_input(connection.epoch, action(connection.epoch))
            .unwrap(),
        mornlea_client_core::InputReceipt::Queued {
            first_sequence: Some(1),
            ..
        }
    ));
}

#[test]
fn active_dimension_retires_prior_geometry_without_waiting_for_a_forget() {
    let mut connection = Connection::new();
    connection.feed(stone_snapshot(Dimension::OVERWORLD));
    connection.feed(authority(1, 0, Dimension::OVERWORLD));
    connection
        .core
        .step(connection.epoch, ClientWorkBudget::try_new(8, 24).unwrap())
        .unwrap();
    let keys = resource_keys(&connection);
    assert!(!keys.is_empty());
    connection.feed(authority(2, 0, Dimension::DEPTHS));
    connection.step();
    assert!(
        keys.iter()
            .all(|key| connection.core.prepared_resource(key).is_err())
    );
}

#[test]
fn numeric_tcp_endpoint_uses_the_same_real_login_path() {
    use mornlea_client_core::presentation::FamilyRecords;
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let player =
        PlayerId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 3]).unwrap();
    let login = LoginStart::new(player, "tcp", 2).unwrap();
    let expected_login =
        mornlea_protocol::write_frame(LoginStart::PACKET_ID, &login.encode().unwrap()).unwrap();
    let hello = mornlea_protocol::ClientHello::new(Identities::current().protocol).unwrap();
    let expected_hello = mornlea_protocol::write_frame(
        mornlea_protocol::ClientHello::PACKET_ID,
        &hello.encode().unwrap(),
    )
    .unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = vec![0; expected_hello.len()];
        stream.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, expected_hello);
        stream
            .write_all(&crate::support::frame_server_packet(
                &ServerPacket::ServerHello(
                    ServerHello::new(Identities::current().protocol).unwrap(),
                ),
            ))
            .unwrap();
        bytes.resize(expected_login.len(), 0);
        stream.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, expected_login);
        stream
            .write_all(&crate::support::frame_server_packet(
                &ServerPacket::LoginSuccess(LoginSuccess::new(player, 42)),
            ))
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
    });
    let config = ClientConfig::try_new(
        ClientLimits::try_new().unwrap(),
        Duration::from_secs(5),
        Duration::from_secs(5),
        Arc::new(StdMonotonicClock),
        Arc::new(ConnectorRegistry::new()),
    )
    .unwrap();
    let mut core = ClientCore::new(config).unwrap();
    let epoch = core
        .connect(
            Endpoint::Tcp(address),
            ClientIdentity::try_new(login).unwrap(),
        )
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        core.step(epoch, ClientWorkBudget::try_new(8, 0).unwrap())
            .unwrap();
        let frame = core.snapshot(epoch).unwrap();
        if frame.families().iter().any(|family| matches!(family.records(), FamilyRecords::Session(records) if records.iter().any(|record| record.phase() == &mornlea_client_core::SessionPhase::Admitted))) { break; }
        assert!(
            std::time::Instant::now() < deadline,
            "nonblocking TCP login deadline"
        );
        std::thread::yield_now();
    }
    core.close().unwrap();
    server.join().unwrap();
}

#[test]
fn diagnostic_high_water_survives_successful_outbound_drain() {
    use mornlea_client_core::presentation::FamilyRecords;
    let mut connection = Connection::new();
    connection
        .core
        .submit_input(connection.epoch, action(connection.epoch))
        .unwrap();
    connection.step();
    connection.step();
    let frame = connection.core.snapshot(connection.epoch).unwrap();
    let queues = frame
        .families()
        .iter()
        .find_map(|family| match family.records() {
            FamilyRecords::Diagnostics(records) => {
                records.first().map(|record| record.queue_high_water())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(queues.outbound_records(), 1);
    assert!(queues.outbound_bytes() > 0);
}

#[test]
fn live_prompt_uses_loaded_block_and_clears_when_forgotten() {
    use mornlea_client_core::presentation::{FamilyRecords, WorldUiView};
    let mut connection = Connection::new();
    connection.feed(stone_snapshot(Dimension::DEPTHS));
    let ServerPacket::PlayerState(mut state) = authority(1, 0, Dimension::DEPTHS) else {
        unreachable!()
    };
    state.position = [3.5, 8.5, 8.5];
    state.yaw = -std::f32::consts::FRAC_PI_2;
    connection.feed(ServerPacket::PlayerState(state));
    connection.step();
    let frame = connection.core.snapshot(connection.epoch).unwrap();
    assert!(frame.families().iter().any(|family| matches!(family.records(), FamilyRecords::WorldUi(records) if records.iter().any(|record| matches!(record.view(), WorldUiView::Prompt(Some(prompt)) if prompt.label().as_str() == "石头")))));
    connection.feed(ServerPacket::ForgetChunks(
        ForgetChunks::new(Dimension::DEPTHS, vec![(0, 0)]).unwrap(),
    ));
    connection.step();
    let frame = connection.core.snapshot(connection.epoch).unwrap();
    assert!(frame.families().iter().any(|family| matches!(family.records(), FamilyRecords::WorldUi(records) if records.iter().any(|record| matches!(record.view(), WorldUiView::Prompt(None))))));
}
