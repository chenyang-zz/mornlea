//! Memory adapter parity cases: the in-process transport over the shared core.
//!
//! These cases drive `MemoryTransport` end to end over the frozen ports: a
//! real `AuthorityState` behind the session and publication seams, an
//! immediately resolving load double, and an injected monotonic clock. Every
//! client byte reaches the authority only as an owned frame through the shared
//! connection core, and every authority frame leaves only through the
//! connection queue or the session outbox drain. The expected login payloads
//! mirror the Go oracle `TestHostLoginSuccessCarriesStoreSeedAcrossTransports`
//! (`memory` row: `LoginSuccess.WorldSeed` carries the store seed) and the
//! slow-receiver retirement mirrors the bounded-outbox semantics.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Instant;

use mornlea_domain::{Identities, PlayerId};
use mornlea_protocol::{
    AdmittedLogin, ClientHello, ClientPacket, CloseContainer, LoginStart, LoginSuccess, PlayIntent,
    ProtocolCodec, ServerHello, ServerPacket, State, encode_uvarint, read_frame_ref, write_frame,
};
use mornlea_server::contracts::{
    Clock, CloseReason, CompanionActionEnvelope, CompanionReceipt, ConnectionId,
    ConnectionProgress, ControlReply, Deadline, LoadPoll, LoginPoll, LoginTicket, PlayerLoadPort,
    PublicationPort, ServerEndpoint, ServerError, ServerLimits, SessionKey, SessionPhase,
    ShutdownFailure, SubmissionReceipt, TickBudget, TickCounters, TickPublication, TransportKind,
};
use mornlea_server::state::AuthorityState;
use mornlea_server::transport::common::TransportAuthority;
use mornlea_server::transport::memory::MemoryTransport;

const WORLD_SEED: i64 = 7;

/// Injected monotonic clock. No case sleeps; the tests never advance time.
struct StepClock {
    now: RefCell<Instant>,
}

impl StepClock {
    fn new() -> Self {
        Self {
            now: RefCell::new(Instant::now()),
        }
    }
}

impl Clock for StepClock {
    fn monotonic(&self) -> Instant {
        *self.now.borrow()
    }

    fn unix_ms(&self) -> i64 {
        1_000
    }
}

/// Immediately resolving load double: every started ticket holds an absent
/// stored record, so the next poll loads a canonical new player.
#[derive(Default)]
struct ImmediateLoad {
    next_ticket: u64,
    live: BTreeMap<u64, ()>,
    last: Option<LoginTicket>,
    cancelled: Vec<u64>,
}

impl PlayerLoadPort for ImmediateLoad {
    fn start(
        &mut self,
        _player: PlayerId,
        _deadline: Deadline,
    ) -> Result<LoginTicket, ServerError> {
        self.next_ticket += 1;
        let ticket = LoginTicket::try_from_raw(self.next_ticket)
            .expect("load tickets are nonzero by construction");
        self.live.insert(ticket.get(), ());
        self.last = Some(ticket);
        Ok(ticket)
    }

    fn poll(&mut self, ticket: LoginTicket) -> LoadPoll {
        if self.live.contains_key(&ticket.get()) {
            LoadPoll::Loaded(None)
        } else {
            LoadPoll::Pending
        }
    }

    fn cancel(&mut self, ticket: LoginTicket) -> Result<(), ServerError> {
        self.live.remove(&ticket.get());
        self.cancelled.push(ticket.get());
        Ok(())
    }
}

struct TicketRecord {
    session: SessionKey,
    player: PlayerId,
    committed: bool,
}

/// Executing transport-authority double over one real `AuthorityState`.
struct MemoryDouble {
    authority: AuthorityState,
    loads: ImmediateLoad,
    tickets: BTreeMap<u64, TicketRecord>,
    commits: Vec<u64>,
    cancels: Vec<u64>,
    closes: Vec<SessionKey>,
    submits: Vec<SubmissionReceipt>,
}

impl MemoryDouble {
    fn new() -> Self {
        let limits = ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap();
        Self {
            authority: AuthorityState::try_new(limits, WORLD_SEED).unwrap(),
            loads: ImmediateLoad::default(),
            tickets: BTreeMap::new(),
            commits: Vec::new(),
            cancels: Vec::new(),
            closes: Vec::new(),
            submits: Vec::new(),
        }
    }

    fn committed_session(&self, ticket: LoginTicket) -> SessionKey {
        self.tickets[&ticket.get()].session
    }
}

impl ServerEndpoint for MemoryDouble {
    fn admit(
        &mut self,
        login: AdmittedLogin,
        transport: TransportKind,
    ) -> Result<SessionKey, ServerError> {
        self.authority.admit(login, transport)
    }

    fn submit(
        &mut self,
        session: SessionKey,
        intent: PlayIntent,
    ) -> Result<SubmissionReceipt, ServerError> {
        let receipt = self.authority.submit(session, intent)?;
        self.submits.push(receipt);
        Ok(receipt)
    }

    fn submit_companion(
        &mut self,
        candidate: CompanionActionEnvelope,
    ) -> Result<CompanionReceipt, ServerError> {
        self.authority.submit_companion(candidate)
    }

    fn advance_tick(&mut self, work: TickBudget) -> Result<TickPublication, ServerError> {
        self.authority.advance_tick(work)
    }

    fn close_session(
        &mut self,
        session: SessionKey,
        reason: CloseReason,
    ) -> Result<(), ServerError> {
        self.closes.push(session);
        self.authority.close_session(session, reason)
    }

    fn shutdown(
        &mut self,
        _deadline: Deadline,
    ) -> Result<mornlea_server::contracts::ShutdownReport, ShutdownFailure> {
        unimplemented!("shutdown stays outside the memory adapter surface")
    }
}

impl PublicationPort for MemoryDouble {
    fn publish(&mut self, publication: TickPublication) -> Result<(), ServerError> {
        self.authority.publish(publication)
    }

    fn take_outbox(
        &mut self,
        session: SessionKey,
        max_frames: usize,
        max_bytes: usize,
    ) -> Result<Vec<Vec<u8>>, ServerError> {
        self.authority.take_outbox(session, max_frames, max_bytes)
    }

    fn close_outbox(&mut self, session: SessionKey, reason: CloseReason) {
        self.authority.close_outbox(session, reason)
    }
}

impl TransportAuthority for MemoryDouble {
    fn begin_login(
        &mut self,
        login: AdmittedLogin,
        kind: TransportKind,
        deadline: Deadline,
    ) -> Result<LoginTicket, ServerError> {
        let session = self.authority.prepare(login.clone(), kind)?;
        let player = login.player_id();
        let ticket = self.loads.start(player, deadline)?;
        self.tickets.insert(
            ticket.get(),
            TicketRecord {
                session,
                player,
                committed: false,
            },
        );
        Ok(ticket)
    }

    fn poll_login(&mut self, ticket: LoginTicket) -> LoginPoll {
        match self.loads.poll(ticket) {
            LoadPoll::Pending => LoginPoll::Pending,
            LoadPoll::Loaded(stored) => {
                let record = &self.tickets[&ticket.get()];
                self.authority.install(record.session, stored).unwrap();
                LoginPoll::Ready {
                    session: record.session,
                    success: ServerPacket::LoginSuccess(LoginSuccess::new(
                        record.player,
                        self.world_seed() as u64,
                    )),
                }
            }
            LoadPoll::Failed { .. } => unreachable!("the immediate load never fails"),
        }
    }

    fn commit_login(&mut self, ticket: LoginTicket) -> Result<SessionKey, ServerError> {
        let record = self.tickets.get_mut(&ticket.get()).expect("known ticket");
        self.authority.activate(record.session)?;
        record.committed = true;
        self.commits.push(ticket.get());
        Ok(record.session)
    }

    fn cancel_login(&mut self, ticket: LoginTicket) {
        if let Some(record) = self.tickets.get_mut(&ticket.get())
            && !record.committed
        {
            self.loads.cancel(ticket).unwrap();
            self.authority
                .retire(record.session, CloseReason::PeerGone)
                .unwrap();
            self.cancels.push(ticket.get());
        }
    }

    fn world_seed(&self) -> i64 {
        WORLD_SEED
    }
}

fn protocol() -> u32 {
    Identities::current().protocol
}

fn player(tag: u8) -> PlayerId {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    PlayerId::try_from_bytes(bytes).unwrap()
}

fn hello_packet() -> ClientPacket {
    let payload = encode_uvarint(protocol());
    ClientPacket::ClientHello(ClientHello::decode_inbound(&payload).unwrap())
}

fn login_packet(tag: u8, name: &str) -> ClientPacket {
    let start = LoginStart::new(player(tag), name, 8).unwrap();
    ClientPacket::LoginStart(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap())
}

fn play_packet(sequence: u64) -> ClientPacket {
    ClientPacket::CloseContainer(CloseContainer::new(sequence))
}

/// Frames one owned client packet through the adapter's own envelope path, so
/// every sent byte exercises the adapter framing under test.
fn frame(packet: &ClientPacket) -> Vec<u8> {
    MemoryTransport::encode_frame(packet).unwrap()
}

/// Independently encodes the payload used by the complete frame expectation.
fn control_payload(packet: &ServerPacket) -> Vec<u8> {
    let mut codec = ProtocolCodec::new().unwrap();
    let mut buffer = vec![0u8; 64];
    loop {
        match codec.encode_server_into(packet, &mut buffer) {
            Ok(written) => break buffer[..written].to_vec(),
            Err(mornlea_protocol::ProtocolError::OutputTooSmall { needed, .. })
                if needed > buffer.len() =>
            {
                buffer.resize(needed, 0);
            }
            Err(other) => panic!("independent encode failed: {other:?}"),
        }
    }
}
fn control_frame(packet: &ServerPacket) -> Vec<u8> {
    write_frame(packet.key().id, &control_payload(packet)).unwrap()
}

fn decode_server(state: State, frame: &[u8]) -> ServerPacket {
    let parsed = read_frame_ref(frame).unwrap();
    ProtocolCodec::new()
        .unwrap()
        .decode_server(state, parsed.packet_id, parsed.payload)
        .unwrap()
}

fn expect_advanced(progress: ConnectionProgress, frames: usize) {
    assert_eq!(progress, ConnectionProgress::Advanced { frames });
}

fn expect_closed(progress: ConnectionProgress) -> (CloseReason, Option<ServerError>) {
    match progress {
        ConnectionProgress::Closed { reason, class } => (reason, class),
        other => panic!("expected a closed connection, got {other:?}"),
    }
}

/// Drives one memory connection from `connect` through the acknowledged
/// success handoff and returns the connection, its active session, and the
/// raw `LoginSuccess` frame the peer received.
fn drive_to_play(
    link: &mut MemoryTransport,
    endpoint: &mut MemoryDouble,
    clock: &StepClock,
    tag: u8,
    name: &str,
) -> (ConnectionId, SessionKey, Vec<u8>) {
    let id = link.connect(clock.monotonic()).unwrap();
    expect_advanced(link.send(id, frame(&hello_packet()), endpoint, clock), 1);
    let hello_out = link.receive(id, 8, 1 << 20);
    assert_eq!(hello_out.len(), 1);
    assert_eq!(
        hello_out[0],
        control_frame(&ServerPacket::ServerHello(
            ServerHello::new(protocol()).unwrap()
        ))
    );
    expect_advanced(link.acknowledge(id, 1, endpoint), 1);
    expect_advanced(
        link.send(id, frame(&login_packet(tag, name)), endpoint, clock),
        1,
    );
    let ticket = endpoint.loads.last.expect("login started");
    expect_advanced(link.poll(id, endpoint, clock), 1);
    let mut success_out = link.receive(id, 8, 1 << 20);
    assert_eq!(success_out.len(), 1);
    let expected = ServerPacket::LoginSuccess(LoginSuccess::new(player(tag), WORLD_SEED as u64));
    assert_eq!(success_out[0], control_frame(&expected));
    expect_advanced(link.acknowledge(id, 1, endpoint), 1);
    let session = endpoint.committed_session(ticket);
    assert_eq!(
        endpoint.authority.session(session).unwrap().phase,
        SessionPhase::Active
    );
    (id, session, success_out.remove(0))
}

#[test]
fn login_play_close_round_trip() {
    let mut link = MemoryTransport::new();
    let mut endpoint = MemoryDouble::new();
    let clock = StepClock::new();
    let (id, session, success_frame) = drive_to_play(&mut link, &mut endpoint, &clock, 1, "Ada");

    // The admitted login carries the store seed, mirroring the Go memory row.
    assert_eq!(
        decode_server(State::Login, &success_frame),
        ServerPacket::LoginSuccess(LoginSuccess::new(player(1), WORLD_SEED as u64))
    );

    // Play frames convert and submit in order with consecutive arrival indexes.
    for (index, sequence) in [5u64, 6, 7].iter().enumerate() {
        expect_advanced(
            link.send(id, frame(&play_packet(*sequence)), &mut endpoint, &clock),
            1,
        );
        assert_eq!(
            endpoint.submits[index],
            SubmissionReceipt::QueuedForTick {
                tick: 0,
                arrival_index: index as u64,
            }
        );
    }

    // The session outbox drains through the adapter without world access.
    endpoint
        .publish(TickPublication {
            tick: 0,
            events: Vec::new(),
            control: vec![ControlReply {
                session,
                packet: ServerPacket::LoginSuccess(LoginSuccess::new(player(1), WORLD_SEED as u64)),
            }],
            counters: TickCounters::default(),
        })
        .unwrap();
    let drained = MemoryTransport::drain_session(&mut endpoint, session, 8, 1 << 20).unwrap();
    assert_eq!(drained.len(), 1);

    // Close retires the session; later frames replay the close with no effect.
    link.close(id, CloseReason::PeerGone, &mut endpoint);
    assert_eq!(endpoint.closes, vec![session]);
    assert_eq!(
        endpoint.authority.session(session).unwrap().phase,
        SessionPhase::Retired
    );
    let (reason, class) =
        expect_closed(link.send(id, frame(&play_packet(8)), &mut endpoint, &clock));
    assert_eq!(reason, CloseReason::PeerGone);
    assert_eq!(class, None);
    assert_eq!(endpoint.submits.len(), 3);
    let (reason, _) = expect_closed(link.poll(id, &mut endpoint, &clock));
    assert_eq!(reason, CloseReason::PeerGone);
}

#[test]
fn outbox_pressure_512_513() {
    let mut link = MemoryTransport::new();
    let mut endpoint = MemoryDouble::new();
    let clock = StepClock::new();
    let (slow_id, slow, _) = drive_to_play(&mut link, &mut endpoint, &clock, 1, "Ada");
    let (peer_id, peer, _) = drive_to_play(&mut link, &mut endpoint, &clock, 2, "Bea");

    // Fill the slow receiver with exactly 512 ordered frames in one publish.
    let mut expected = Vec::new();
    let mut replies = Vec::new();
    for seed in 0..512u64 {
        let packet = ServerPacket::LoginSuccess(LoginSuccess::new(player(1), seed));
        expected.push(control_frame(&packet));
        replies.push(ControlReply {
            session: slow,
            packet,
        });
    }
    endpoint
        .publish(TickPublication {
            tick: 0,
            events: Vec::new(),
            control: replies,
            counters: TickCounters::default(),
        })
        .unwrap();

    // The 513th frame retires only the slow receiver: the outbox still holds
    // the 512 undrained frames, so the next append overflows, drops the frame,
    // and retires the receiver without appending a Disconnect.
    endpoint
        .publish(TickPublication {
            tick: 1,
            events: Vec::new(),
            control: vec![ControlReply {
                session: slow,
                packet: ServerPacket::LoginSuccess(LoginSuccess::new(player(1), 512)),
            }],
            counters: TickCounters::default(),
        })
        .unwrap();
    assert_eq!(
        endpoint.authority.session(slow).unwrap().phase,
        SessionPhase::Retired
    );

    // Successive drains keep publication order across polls, and a retired
    // receiver still yields the frames it already held: exactly the 512.
    let mut drained = Vec::new();
    for _ in 0..2 {
        drained.extend(MemoryTransport::drain_session(&mut endpoint, slow, 200, 1 << 20).unwrap());
    }
    drained.extend(MemoryTransport::drain_session(&mut endpoint, slow, 200, 1 << 20).unwrap());
    assert_eq!(drained, expected);

    // The peer is unaffected: still active, still receives, still submits.
    assert_eq!(
        endpoint.authority.session(peer).unwrap().phase,
        SessionPhase::Active
    );
    let peer_packet = ServerPacket::LoginSuccess(LoginSuccess::new(player(2), 9));
    endpoint
        .publish(TickPublication {
            tick: 2,
            events: Vec::new(),
            control: vec![ControlReply {
                session: peer,
                packet: peer_packet.clone(),
            }],
            counters: TickCounters::default(),
        })
        .unwrap();
    assert_eq!(
        MemoryTransport::drain_session(&mut endpoint, peer, 8, 1 << 20).unwrap(),
        vec![control_frame(&peer_packet)]
    );
    expect_advanced(
        link.send(peer_id, frame(&play_packet(1)), &mut endpoint, &clock),
        1,
    );
    assert_eq!(endpoint.submits.len(), 1);
    let _ = slow_id;
}

#[test]
fn stale_session_refused() {
    let mut link = MemoryTransport::new();
    let mut endpoint = MemoryDouble::new();
    let clock = StepClock::new();
    let (id, session, _) = drive_to_play(&mut link, &mut endpoint, &clock, 1, "Ada");
    expect_advanced(
        link.send(id, frame(&play_packet(1)), &mut endpoint, &clock),
        1,
    );
    let before = endpoint.authority.session(session).unwrap();
    assert_eq!(endpoint.submits.len(), 1);

    link.close(id, CloseReason::PeerGone, &mut endpoint);
    assert_eq!(
        endpoint.authority.session(session).unwrap().phase,
        SessionPhase::Retired
    );

    // Frames on the retired connection replay the close and submit nothing.
    let (reason, _) = expect_closed(link.send(id, frame(&play_packet(2)), &mut endpoint, &clock));
    assert_eq!(reason, CloseReason::PeerGone);
    assert_eq!(endpoint.submits.len(), 1);

    // A direct submit against the retired session is refused with no effect.
    let refused = endpoint.authority.submit(
        session,
        PlayIntent::try_from(ClientPacket::CloseContainer(CloseContainer::new(3))).unwrap(),
    );
    assert_eq!(refused, Err(ServerError::StaleSession { session }));
    let after = endpoint.authority.session(session).unwrap();
    assert_eq!(after.next_arrival, before.next_arrival);
    assert_eq!(after.last_applied_sequence, before.last_applied_sequence);
}

#[test]
fn local_direct_mutation_impossible() {
    let mut link = MemoryTransport::new();
    let mut endpoint = MemoryDouble::new();
    let clock = StepClock::new();
    let (id, session, _) = drive_to_play(&mut link, &mut endpoint, &clock, 1, "Ada");
    let tick_before = endpoint.authority.next_tick();
    let facts_before = endpoint.authority.session(session).unwrap();

    // Every non-frame adapter path leaves authority state untouched.
    assert_eq!(
        link.poll(id, &mut endpoint, &clock),
        ConnectionProgress::AwaitMore
    );
    assert!(link.receive(id, 8, 1 << 20).is_empty());
    assert_eq!(
        link.acknowledge(id, 1, &mut endpoint),
        ConnectionProgress::AwaitMore
    );
    assert_eq!(link.retained_len(id), Some(0));
    assert!(
        MemoryTransport::drain_session(&mut endpoint, session, 8, 1 << 20)
            .unwrap()
            .is_empty()
    );
    assert_eq!(endpoint.submits.len(), 0);
    assert_eq!(endpoint.authority.session(session).unwrap(), facts_before);
    assert_eq!(endpoint.authority.next_tick(), tick_before);

    // The frame path is the only mutation: one frame advances arrival by one.
    expect_advanced(
        link.send(id, frame(&play_packet(4)), &mut endpoint, &clock),
        1,
    );
    assert_eq!(endpoint.submits.len(), 1);
    let facts_after = endpoint.authority.session(session).unwrap();
    assert_eq!(facts_after.next_arrival, facts_before.next_arrival + 1);
    assert_eq!(
        facts_after.last_applied_sequence,
        facts_before.last_applied_sequence
    );
}
