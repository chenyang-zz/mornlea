//! Consumer contract doubles prove receipts and ownership, not an actual outbox.
//! Factory cases separately exercise the existing canonical protocol encoder.

use std::collections::{BTreeMap, VecDeque};

use mornlea_domain::{
    ChunkPos, ChunkSnapshot, ChunkSnapshotParts, Dimension, Event, PalettedSection, PlayerId,
};
use mornlea_protocol::{
    CommandRejected, Direction, Disconnect, LoginStart, LoginSuccess, MAX_FRAME_BYTES,
    ProtocolCodec, ServerHello, ServerPacket, State, admit_login, read_frame_ref, write_frame,
};
use mornlea_server::contracts::{
    CloseReason, ServerError, ServerLimits, SessionKey, SessionPhase, TransportKind,
};
use mornlea_server::core::publication::{EnqueueOutcome, PreparedFrame, PreparedPublicationPort};
use mornlea_server::state::AuthorityState;

fn player(tag: u8) -> PlayerId {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    PlayerId::try_from_bytes(bytes).unwrap()
}

fn sessions() -> [SessionKey; 3] {
    let mut authority = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        0,
    )
    .unwrap();
    [1, 2, 3].map(|tag| {
        let start = LoginStart::new(player(tag), format!("P{tag}"), 8).unwrap();
        let login =
            admit_login(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap()).unwrap();
        authority.admit(login, TransportKind::Memory).unwrap()
    })
}

fn rejection(sequence: u64) -> ServerPacket {
    ServerPacket::CommandRejected(CommandRejected::new(sequence, 1).unwrap())
}

fn prepared(packet: &ServerPacket) -> PreparedFrame {
    PreparedFrame::encode(&mut ProtocolCodec::new().unwrap(), packet).unwrap()
}

fn sequence(frame: &PreparedFrame) -> u64 {
    let decoded = read_frame_ref(frame.as_bytes()).unwrap();
    assert_eq!(decoded.consumed, frame.byte_len());
    assert_eq!(decoded.packet_id, CommandRejected::PACKET_ID);
    CommandRejected::decode(decoded.payload).unwrap().sequence
}

struct ReceiverDouble {
    phase: SessionPhase,
    open: bool,
    close_reason: Option<CloseReason>,
    queue: VecDeque<PreparedFrame>,
}

struct PublicationDouble {
    receivers: BTreeMap<SessionKey, ReceiverDouble>,
}

impl PublicationDouble {
    fn new(keys: [SessionKey; 2]) -> Self {
        Self {
            receivers: keys
                .into_iter()
                .map(|key| {
                    (
                        key,
                        ReceiverDouble {
                            phase: SessionPhase::Active,
                            open: true,
                            close_reason: None,
                            queue: VecDeque::new(),
                        },
                    )
                })
                .collect(),
        }
    }
}

// This executing double owns only phase/open flags and two queued frame owners.
// It neither implements nor qualifies the authority or transport provider.
impl PreparedPublicationPort for PublicationDouble {
    fn enqueue_prepared(
        &mut self,
        session: SessionKey,
        frame: PreparedFrame,
    ) -> Result<EnqueueOutcome, ServerError> {
        let key = frame.packet_key();
        if key.direction != Direction::ServerToClient || key.state != State::Play {
            return Err(ServerError::InvalidInput { field: "packet" });
        }
        let receiver = self
            .receivers
            .get_mut(&session)
            .ok_or(ServerError::StaleSession { session })?;
        if receiver.phase != SessionPhase::Active || !receiver.open {
            return Ok(EnqueueOutcome::Closed);
        }
        if receiver.queue.len() == 2 {
            receiver.open = false;
            receiver.phase = SessionPhase::Retired;
            receiver.close_reason = Some(CloseReason::SlowReceiver);
            return Ok(EnqueueOutcome::Closed);
        }
        receiver.queue.push_back(frame);
        Ok(EnqueueOutcome::Queued)
    }

    fn take_prepared_outbox(
        &mut self,
        session: SessionKey,
        max_frames: usize,
        max_bytes: usize,
    ) -> Result<Vec<PreparedFrame>, ServerError> {
        let receiver = self
            .receivers
            .get_mut(&session)
            .ok_or(ServerError::StaleSession { session })?;
        let mut taken = Vec::new();
        let mut bytes = 0usize;
        while taken.len() < max_frames {
            let Some(next) = receiver.queue.front() else {
                break;
            };
            if !taken.is_empty() && next.byte_len() > max_bytes.saturating_sub(bytes) {
                break;
            }
            let frame = receiver.queue.pop_front().unwrap();
            bytes += frame.byte_len();
            taken.push(frame);
        }
        Ok(taken)
    }
}

#[test]
fn actual_factory_matches_canonical_control_frames_and_decodes_values() {
    let rejected = CommandRejected::new(u64::MAX, 15).unwrap();
    let login = LoginSuccess::new(player(5), 0x1234_5678_90ab_cdef);
    for packet in [
        ServerPacket::CommandRejected(rejected),
        ServerPacket::LoginSuccess(login),
    ] {
        let frame = prepared(&packet);
        let mut codec = ProtocolCodec::new().unwrap();
        let mut payload = [0u8; 64];
        let written = codec.encode_server_into(&packet, &mut payload).unwrap();
        let canonical = write_frame(packet.key().id, &payload[..written]).unwrap();
        assert_eq!(frame.as_bytes(), canonical);
        assert_eq!(frame.packet_key(), packet.key());
        assert_eq!(frame.byte_len(), canonical.len());
        assert!(frame.byte_len() <= MAX_FRAME_BYTES as usize + 5);
        let wire = read_frame_ref(frame.as_bytes()).unwrap();
        assert_eq!(wire.consumed, frame.byte_len());
        assert_eq!(wire.packet_id, packet.key().id);
        match packet {
            ServerPacket::CommandRejected(_) => {
                assert_eq!(CommandRejected::decode(wire.payload).unwrap(), rejected)
            }
            ServerPacket::LoginSuccess(_) => {
                assert_eq!(LoginSuccess::decode(wire.payload).unwrap(), login)
            }
            _ => unreachable!(),
        }
    }
    let hello = ServerPacket::ServerHello(ServerHello::new(45).unwrap());
    let frame = prepared(&hello);
    assert_eq!(frame.packet_key(), hello.key());
    let wire = read_frame_ref(frame.as_bytes()).unwrap();
    assert_eq!(wire.consumed, frame.byte_len());
    assert_eq!(
        ServerHello::decode(wire.payload).unwrap(),
        ServerHello::new(45).unwrap()
    );
}

fn air_snapshot() -> ChunkSnapshot {
    ChunkSnapshot::try_new(ChunkSnapshotParts {
        dimension: Dimension::OVERWORLD,
        chunk: ChunkPos::new(-7, 19),
        revision: 33,
        sections: Box::new(std::array::from_fn(|_| PalettedSection::single(0).unwrap())),
    })
    .unwrap()
}

#[test]
fn actual_factory_roundtrips_checked_domain_air_snapshot() {
    let domain = air_snapshot();
    let packet = ServerPacket::try_from(Event::ChunkSnapshot(domain.clone())).unwrap();
    let mut codec = ProtocolCodec::new().unwrap();
    let frame = PreparedFrame::encode(&mut codec, &packet).unwrap();
    assert_eq!(frame.packet_key(), packet.key());
    assert!(frame.byte_len() <= MAX_FRAME_BYTES as usize + 5);
    let wire = read_frame_ref(frame.as_bytes()).unwrap();
    assert_eq!(wire.consumed, frame.byte_len());
    assert_eq!(wire.packet_id, 0);
    let decoded = codec.decode_snapshot(wire.payload).unwrap();
    assert_eq!(ServerPacket::ChunkSnapshot(decoded.clone()), packet);
    assert_eq!(
        Event::try_from(ServerPacket::ChunkSnapshot(decoded)).unwrap(),
        Event::ChunkSnapshot(domain)
    );
}

#[test]
fn actual_factory_refuses_mutated_snapshot_without_an_owner() {
    let original = ServerPacket::try_from(Event::ChunkSnapshot(air_snapshot())).unwrap();
    let ServerPacket::ChunkSnapshot(snapshot) = original else {
        unreachable!()
    };
    assert_eq!(snapshot.sections.len(), 24);
    let mut codec = ProtocolCodec::new().unwrap();
    for corrupt in [0, 1, 2] {
        let mut bad = snapshot.clone();
        match corrupt {
            0 => bad.revision = 0,
            1 => {
                bad.sections.pop();
            }
            _ => bad.sections[0].bits = 4,
        }
        let result = PreparedFrame::encode(&mut codec, &ServerPacket::ChunkSnapshot(bad));
        assert!(matches!(
            result,
            Err(ServerError::InvalidInput { field: "packet" })
        ));
    }
    assert_eq!(
        sequence(&PreparedFrame::encode(&mut codec, &rejection(77)).unwrap()),
        77
    );
}

#[test]
fn consumer_double_saturation_retains_fifo_and_isolates_peer() {
    let [slow, peer, unknown] = sessions();
    let mut port = PublicationDouble::new([slow, peer]);
    for (key, value) in [(slow, 10), (peer, 20), (slow, 11), (peer, 21)] {
        assert_eq!(
            port.enqueue_prepared(key, prepared(&rejection(value))),
            Ok(EnqueueOutcome::Queued)
        );
    }
    assert_eq!(
        port.enqueue_prepared(slow, prepared(&rejection(12))),
        Ok(EnqueueOutcome::Closed)
    );
    let slow_state = &port.receivers[&slow];
    assert_eq!(slow_state.phase, SessionPhase::Retired);
    assert!(!slow_state.open);
    assert_eq!(slow_state.close_reason, Some(CloseReason::SlowReceiver));
    assert_eq!(slow_state.queue.len(), 2);
    assert_eq!(
        port.enqueue_prepared(slow, prepared(&rejection(13))),
        Ok(EnqueueOutcome::Closed)
    );
    assert_eq!(
        port.enqueue_prepared(unknown, prepared(&rejection(99))),
        Err(ServerError::StaleSession { session: unknown })
    );
    let taken = port.take_prepared_outbox(slow, 2, usize::MAX).unwrap();
    assert_eq!(taken.iter().map(sequence).collect::<Vec<_>>(), [10, 11]);
    let taken = port.take_prepared_outbox(peer, 2, usize::MAX).unwrap();
    assert_eq!(taken.iter().map(sequence).collect::<Vec<_>>(), [20, 21]);
    assert_eq!(port.receivers[&peer].phase, SessionPhase::Active);
    assert!(port.receivers[&peer].open);
    assert_eq!(
        port.enqueue_prepared(peer, prepared(&rejection(22))),
        Ok(EnqueueOutcome::Queued)
    );
    assert_eq!(
        sequence(&port.take_prepared_outbox(peer, 1, 0).unwrap()[0]),
        22
    );
    assert!(
        matches!(port.take_prepared_outbox(unknown, 0, 0), Err(ServerError::StaleSession { session }) if session == unknown)
    );
}

#[test]
fn consumer_double_nonplay_refusal_precedes_queue_mutation() {
    let [first, peer, _] = sessions();
    let mut port = PublicationDouble::new([first, peer]);
    for packet in [
        ServerPacket::LoginSuccess(LoginSuccess::new(player(4), 7)),
        ServerPacket::ServerHello(ServerHello::new(45).unwrap()),
    ] {
        assert_eq!(
            port.enqueue_prepared(first, prepared(&packet)),
            Err(ServerError::InvalidInput { field: "packet" })
        );
        assert!(port.receivers[&first].queue.is_empty());
        assert!(port.receivers[&first].open);
    }
    port.receivers.get_mut(&first).unwrap().open = false;
    assert_eq!(
        port.enqueue_prepared(first, prepared(&rejection(1))),
        Ok(EnqueueOutcome::Closed)
    );
    port.receivers.get_mut(&peer).unwrap().phase = SessionPhase::Prepared;
    assert_eq!(
        port.enqueue_prepared(peer, prepared(&rejection(2))),
        Ok(EnqueueOutcome::Closed)
    );
    assert!(
        port.receivers
            .values()
            .all(|receiver| receiver.queue.is_empty())
    );
}

#[test]
fn consumer_double_whole_frame_budgets_preserve_the_suffix() {
    let [first, peer, _] = sessions();
    let mut port = PublicationDouble::new([first, peer]);
    let small = prepared(&rejection(1));
    let large = prepared(&ServerPacket::Disconnect(
        Disconnect::new(1, "x".repeat(80)).unwrap(),
    ));
    assert!(large.byte_len() > small.byte_len());
    for max_bytes in [0, small.byte_len() - 1] {
        port.enqueue_prepared(first, large.clone()).unwrap();
        port.enqueue_prepared(first, small.clone()).unwrap();
        assert!(
            port.take_prepared_outbox(first, 0, max_bytes)
                .unwrap()
                .is_empty()
        );
        let taken = port.take_prepared_outbox(first, 2, max_bytes).unwrap();
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].as_bytes().as_ptr(), large.as_bytes().as_ptr());
        assert_eq!(port.receivers[&first].queue.len(), 1);
        assert_eq!(
            sequence(&port.take_prepared_outbox(first, 1, 0).unwrap()[0]),
            1
        );
    }
    let total = small.byte_len() + large.byte_len();
    for (budget, count) in [(total - 1, 1), (total, 2)] {
        port.enqueue_prepared(peer, small.clone()).unwrap();
        port.enqueue_prepared(peer, large.clone()).unwrap();
        let taken = port.take_prepared_outbox(peer, 2, budget).unwrap();
        assert_eq!(taken.len(), count);
        assert_eq!(taken[0].as_bytes().as_ptr(), small.as_bytes().as_ptr());
        if count == 1 {
            let suffix = port.take_prepared_outbox(peer, 1, 0).unwrap();
            assert_eq!(suffix[0].as_bytes().as_ptr(), large.as_bytes().as_ptr());
        } else {
            assert_eq!(taken[1].as_bytes().as_ptr(), large.as_bytes().as_ptr());
        }
    }
    port.enqueue_prepared(first, small.clone()).unwrap();
    port.enqueue_prepared(first, large.clone()).unwrap();
    assert_eq!(
        port.take_prepared_outbox(first, 1, usize::MAX)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(port.receivers[&first].queue.len(), 1);
}

#[test]
fn consumer_mirror_revision_advances_only_on_queued() {
    let [first, peer, unknown] = sessions();
    let mut port = PublicationDouble::new([first, peer]);
    let mut mirror_revision = 0;
    let login = prepared(&ServerPacket::LoginSuccess(LoginSuccess::new(player(5), 0)));
    for (session, revision, frame, expected) in [
        (
            first,
            1,
            prepared(&rejection(1)),
            Ok(EnqueueOutcome::Queued),
        ),
        (
            first,
            2,
            prepared(&rejection(2)),
            Ok(EnqueueOutcome::Queued),
        ),
        (
            first,
            3,
            prepared(&rejection(3)),
            Ok(EnqueueOutcome::Closed),
        ),
        (
            first,
            4,
            prepared(&rejection(4)),
            Ok(EnqueueOutcome::Closed),
        ),
        (
            unknown,
            5,
            prepared(&rejection(5)),
            Err(ServerError::StaleSession { session: unknown }),
        ),
        (
            peer,
            6,
            login,
            Err(ServerError::InvalidInput { field: "packet" }),
        ),
    ] {
        let outcome = port.enqueue_prepared(session, frame);
        if outcome == Ok(EnqueueOutcome::Queued) {
            mirror_revision = revision;
        }
        assert_eq!(outcome, expected);
        assert_eq!(mirror_revision, revision.min(2));
    }
}

#[test]
fn immutable_owner_survives_codec_packet_and_double_lifetimes() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<PreparedFrame>();
    let (retained, moved, expected_key, expected_bytes) = {
        let packet = rejection(31337);
        let mut codec = ProtocolCodec::new().unwrap();
        let original = PreparedFrame::encode(&mut codec, &packet).unwrap();
        (
            original.clone(),
            original.clone(),
            packet.key(),
            original.as_bytes().to_vec(),
        )
    };
    let pointer = retained.as_bytes().as_ptr();
    assert_eq!(moved.as_bytes().as_ptr(), pointer);
    let moved = std::thread::spawn(move || {
        assert_eq!(sequence(&moved), 31337);
        moved
    })
    .join()
    .unwrap();
    assert_eq!(moved.as_bytes().as_ptr(), pointer);
    let [first, peer, _] = sessions();
    let mut port = PublicationDouble::new([first, peer]);
    assert_eq!(
        port.enqueue_prepared(first, moved),
        Ok(EnqueueOutcome::Queued)
    );
    let taken = port.take_prepared_outbox(first, 1, 0).unwrap();
    assert_eq!(taken[0].as_bytes().as_ptr(), pointer);
    drop(port);
    drop(taken);
    assert_eq!(retained.packet_key(), expected_key);
    assert_eq!(retained.as_bytes(), expected_bytes);
    let debug = format!("{retained:?}");
    assert!(debug.contains("PacketKey"));
    assert!(debug.contains(&retained.byte_len().to_string()));
    assert!(!debug.contains("31337"));
    assert!(debug.len() < 160);
}
