//! Owned tick publication and bounded slow-receiver outbox behavior.
//!
//! These cases drive the publication provider through the frozen state
//! ports: broadcast expansion skips receivers whose outbox is closed, a
//! saturated outbox keeps exactly its frame budget and retires only the
//! slow receiver with the frozen slow-receiver reason, and one slow
//! receiver never blocks the tick or a peer's ordered delivery. No
//! closure ever appends a Disconnect frame; the overflowing frame is
//! dropped silently.

use mornlea_domain::{
    CommandRejection, Event, EventRecipient, PlayerId, RejectReason, RoutedEvent,
};
use mornlea_protocol::{Disconnect, LoginStart, ProtocolCodec, ServerPacket, admit_login};
use mornlea_server::contracts::{
    CloseReason, SessionPhase, TickCounters, TickPublication, TransportKind,
};
use mornlea_server::core::{publication, session};
use mornlea_server::state::AuthorityState;

/// Full outbox contract: the authority limits pin the per-session frame
/// budget this suite saturates.
const OUTBOX_LIMIT: usize = 512;

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        mornlea_server::contracts::ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        7,
    )
    .unwrap()
}

fn login(tag: u8, name: &str) -> mornlea_protocol::AdmittedLogin {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = PlayerId::try_from_bytes(bytes).unwrap();
    let start = LoginStart::new(id, name, 8).unwrap();
    let inbound = LoginStart::decode_inbound(&start.encode().unwrap()).unwrap();
    admit_login(inbound).unwrap()
}

fn rejection(sequence: u64) -> Event {
    Event::CommandRejected(CommandRejection::new(sequence, RejectReason::InvalidRay))
}

/// Encodes one publishable event exactly the way the authority port does,
/// so drained frames can be compared byte for byte against the expectation.
fn frame_of(event: &Event) -> Vec<u8> {
    let packet = ServerPacket::try_from(event.clone()).unwrap();
    let mut codec = ProtocolCodec::new().unwrap();
    let mut buffer = vec![0u8; 64];
    let written = codec.encode_server_into(&packet, &mut buffer).unwrap();
    mornlea_protocol::write_frame(packet.key().id, &buffer[..written]).unwrap()
}

/// A locally built Disconnect frame used only for inequality: no closure
/// path may ever publish a Disconnect encoding into an outbox.
fn disconnect_frame() -> Vec<u8> {
    let packet = ServerPacket::Disconnect(Disconnect::new(1, "closed").unwrap());
    let mut codec = ProtocolCodec::new().unwrap();
    let mut buffer = vec![0u8; 64];
    let written = codec.encode_server_into(&packet, &mut buffer).unwrap();
    mornlea_protocol::write_frame(packet.key().id, &buffer[..written]).unwrap()
}

fn publication(events: Vec<RoutedEvent>) -> TickPublication {
    TickPublication {
        tick: 0,
        events,
        control: Vec::new(),
        counters: TickCounters::default(),
    }
}

fn fill_events(target: mornlea_server::contracts::SessionKey, event: &Event) -> Vec<RoutedEvent> {
    (0..OUTBOX_LIMIT)
        .map(|_| RoutedEvent::new(EventRecipient::Session(target.get()), event.clone()))
        .collect()
}

#[test]
fn mixed_publication_retains_canonical_packet_identity_and_budgeted_suffix() {
    use mornlea_protocol::{LoginSuccess, read_frame_ref, write_frame};
    let mut state = authority();
    let key = session::admit(&mut state, login(1, "Ada"), TransportKind::Memory).unwrap();
    let event = rejection(4);
    let packet = ServerPacket::try_from(event.clone()).unwrap();
    let event_frame = frame_of(&event);
    let control =
        ServerPacket::LoginSuccess(LoginSuccess::new(state.session(key).unwrap().player_id, 7));
    let mut codec = ProtocolCodec::new().unwrap();
    let mut payload = vec![0u8; 64];
    let written = codec.encode_server_into(&control, &mut payload).unwrap();
    let control_frame = write_frame(control.key().id, &payload[..written]).unwrap();
    state
        .publish(TickPublication {
            tick: 0,
            events: vec![RoutedEvent::new(EventRecipient::Session(key.get()), event)],
            control: vec![mornlea_server::contracts::ControlReply {
                session: key,
                packet: control.clone(),
            }],
            counters: TickCounters::default(),
        })
        .unwrap();
    assert!(state.take_outbox(key, 0, 4096).unwrap().is_empty());
    let first = state.take_outbox(key, 8, event_frame.len()).unwrap();
    assert_eq!(first, vec![event_frame]);
    let parsed = read_frame_ref(&first[0]).unwrap();
    assert_eq!(
        (parsed.packet_id, parsed.consumed),
        (packet.key().id, first[0].len())
    );
    // A nonempty outbox transfers one complete frame even under a tiny byte budget.
    let second = state.take_outbox(key, 8, 1).unwrap();
    assert_eq!(second, vec![control_frame]);
    let parsed = read_frame_ref(&second[0]).unwrap();
    assert_eq!(
        (parsed.packet_id, parsed.consumed),
        (control.key().id, second[0].len())
    );
    assert!(state.take_outbox(key, 8, 4096).unwrap().is_empty());
}

#[test]
fn broadcast_excludes_closed_session() {
    let mut state = authority();
    let first = session::admit(&mut state, login(1, "Ada"), TransportKind::Memory).unwrap();
    let second = session::admit(&mut state, login(2, "Bea"), TransportKind::Memory).unwrap();
    let third = session::admit(&mut state, login(3, "Cara"), TransportKind::Memory).unwrap();
    session::close_session(&mut state, third, CloseReason::PeerGone).unwrap();

    let event = rejection(4);
    let expected = frame_of(&event);
    publication::publish_tick(
        &mut state,
        publication(vec![RoutedEvent::new(EventRecipient::Broadcast, event)]),
    )
    .unwrap();

    assert_eq!(state.session(third).unwrap().phase, SessionPhase::Retired);
    assert!(
        publication::drain_outbox(&mut state, third, 8, 4096)
            .unwrap()
            .is_empty(),
        "a retired receiver keeps nothing from a later broadcast"
    );

    let left = publication::drain_outbox(&mut state, first, 8, 4096).unwrap();
    let right = publication::drain_outbox(&mut state, second, 8, 4096).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(right.len(), 1);
    assert_eq!(left[0], expected);
    assert_eq!(right[0], expected);
    assert_eq!(
        left[0], right[0],
        "one broadcast expands to identical frames"
    );

    // A receiver whose outbox was closed without retirement is excluded the
    // same way: the follow-up broadcast reaches only the open peer.
    publication::close_receiver(&mut state, first, CloseReason::SlowReceiver);
    let follow = rejection(5);
    let follow_frame = frame_of(&follow);
    publication::publish_tick(
        &mut state,
        publication(vec![RoutedEvent::new(EventRecipient::Broadcast, follow)]),
    )
    .unwrap();
    assert!(
        publication::drain_outbox(&mut state, first, 8, 4096)
            .unwrap()
            .is_empty()
    );
    let more = publication::drain_outbox(&mut state, second, 8, 4096).unwrap();
    assert_eq!(more.len(), 1);
    assert_eq!(more[0], follow_frame);
}

#[test]
fn outbox_512_then_513() {
    let mut state = authority();
    let key = session::admit(&mut state, login(4, "Dora"), TransportKind::Memory).unwrap();
    assert_eq!(state.limits().session_outbox(), OUTBOX_LIMIT);

    // One publication carrying the full frame budget fills the outbox to
    // exactly the limit without closing it.
    let event = rejection(7);
    let expected = frame_of(&event);
    publication::publish_tick(&mut state, publication(fill_events(key, &event))).unwrap();
    assert_eq!(state.session(key).unwrap().phase, SessionPhase::Active);

    // The next single append saturates the budget: the frame is dropped, the
    // receiver flips closed, and the publish call itself still succeeds.
    publication::publish_tick(
        &mut state,
        publication(vec![RoutedEvent::new(
            EventRecipient::Session(key.get()),
            event.clone(),
        )]),
    )
    .unwrap();
    assert_eq!(state.session(key).unwrap().phase, SessionPhase::Retired);

    let drained = publication::drain_outbox(&mut state, key, OUTBOX_LIMIT + 8, 1_048_576).unwrap();
    assert_eq!(drained.len(), OUTBOX_LIMIT, "no 513th frame is retained");
    assert!(
        drained.iter().all(|frame| frame == &expected),
        "every retained frame is the published event frame"
    );
    let disconnect = disconnect_frame();
    assert!(drained.iter().all(|frame| frame != &disconnect));
    assert!(
        publication::drain_outbox(&mut state, key, OUTBOX_LIMIT + 8, 1_048_576)
            .unwrap()
            .is_empty(),
        "the saturated outbox holds nothing after the full drain"
    );
}

#[test]
fn slow_session_does_not_block_peer() {
    let mut state = authority();
    let slow = session::admit(&mut state, login(5, "Eve"), TransportKind::Memory).unwrap();
    let peer = session::admit(&mut state, login(6, "Faye"), TransportKind::Memory).unwrap();

    let fill_event = rejection(8);
    let fill_frame = frame_of(&fill_event);
    publication::publish_tick(&mut state, publication(fill_events(slow, &fill_event))).unwrap();
    assert_eq!(state.session(slow).unwrap().phase, SessionPhase::Active);

    // The broadcast that overflows the slow receiver still returns Ok and
    // still delivers the peer's first frame.
    let first = rejection(9);
    let first_frame = frame_of(&first);
    publication::publish_tick(
        &mut state,
        publication(vec![RoutedEvent::new(EventRecipient::Broadcast, first)]),
    )
    .unwrap();
    assert_eq!(state.session(slow).unwrap().phase, SessionPhase::Retired);

    let peer_first = publication::drain_outbox(&mut state, peer, 8, 4096).unwrap();
    assert_eq!(peer_first.len(), 1);
    assert_eq!(peer_first[0], first_frame);

    // A second broadcast after the slow closure publishes and delivers in
    // order: the peer's frames arrive as a prefix-stable ordered sequence.
    let second = rejection(10);
    let second_frame = frame_of(&second);
    publication::publish_tick(
        &mut state,
        publication(vec![RoutedEvent::new(EventRecipient::Broadcast, second)]),
    )
    .unwrap();
    let peer_second = publication::drain_outbox(&mut state, peer, 8, 4096).unwrap();
    assert_eq!(peer_second.len(), 1);
    assert_eq!(peer_second[0], second_frame);
    assert_ne!(first_frame, second_frame);
    let ordered = [peer_first, peer_second].concat();
    assert_eq!(
        ordered.first(),
        Some(&first_frame),
        "delivery keeps publish order"
    );

    // The retired receiver keeps its own queued prefix and gains nothing new.
    let slow_drained =
        publication::drain_outbox(&mut state, slow, OUTBOX_LIMIT + 8, 1_048_576).unwrap();
    assert_eq!(slow_drained.len(), OUTBOX_LIMIT);
    assert!(slow_drained.iter().all(|frame| frame == &fill_frame));
    assert!(slow_drained.iter().all(|frame| frame != &first_frame));
    assert!(
        publication::drain_outbox(&mut state, slow, OUTBOX_LIMIT + 8, 1_048_576)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn silent_slow_receiver() {
    let mut state = authority();
    let slow = session::admit(&mut state, login(7, "Gia"), TransportKind::Memory).unwrap();
    let peer = session::admit(&mut state, login(8, "Hana"), TransportKind::Memory).unwrap();

    let fill_event = rejection(11);
    let fill_frame = frame_of(&fill_event);
    publication::publish_tick(&mut state, publication(fill_events(slow, &fill_event))).unwrap();
    assert_eq!(state.session(slow).unwrap().phase, SessionPhase::Active);

    // One incoming event over a receiver already holding the full budget:
    // the slow connection closes with no Disconnect packet, the peer still
    // receives the ordered event, and the tick does not block.
    let incoming = rejection(12);
    let incoming_frame = frame_of(&incoming);
    let disconnect = disconnect_frame();
    let result = publication::publish_tick(
        &mut state,
        publication(vec![RoutedEvent::new(EventRecipient::Broadcast, incoming)]),
    );
    assert_eq!(state.session(slow).unwrap().phase, SessionPhase::Retired);
    assert!(result.is_ok(), "a slow receiver never blocks the tick");

    let slow_drained =
        publication::drain_outbox(&mut state, slow, OUTBOX_LIMIT + 8, 1_048_576).unwrap();
    assert_eq!(slow_drained.len(), OUTBOX_LIMIT);
    assert!(
        slow_drained.iter().all(|frame| frame == &fill_frame),
        "the overflow frame is dropped, not appended"
    );
    assert!(
        slow_drained.iter().all(|frame| frame != &disconnect),
        "slow closure never appends a Disconnect encoding"
    );

    let peer_drained = publication::drain_outbox(&mut state, peer, 8, 4096).unwrap();
    assert_eq!(peer_drained.len(), 1);
    assert_eq!(peer_drained[0], incoming_frame);
}
