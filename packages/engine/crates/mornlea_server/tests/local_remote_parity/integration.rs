//! Real local/remote integration over one shared transcript.
//!
//! Both adapters serve the same two-player input through the same authority
//! seams: a real `AuthorityState` behind session and publication, an
//! immediately resolving load port, and an injected monotonic clock. The
//! transcript logs in Ada then Bea, submits sequenced play intents, runs two
//! real reducer ticks, publishes each tick, and drains both outboxes. The
//! second tick resubmits Ada's older sequence on purpose, so the ordered
//! domain outputs carry one stale refusal. The case asserts the exact session
//! numbering, receipts, counters, ordered events, and drained control frames
//! on each adapter, then asserts the two transcripts are identical. The login
//! seed expectation mirrors the Go oracle
//! `TestHostLoginSuccessCarriesStoreSeedAcrossTransports`: the success frame
//! carries the store seed on both transports.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mornlea_domain::{Identities, PlayerId};
use mornlea_protocol::{
    AdmittedLogin, BoneMeal, CONTAINER_KIND_FURNACE, ChatCommand, ClientHello, ClientPacket,
    CloseContainer, CollectWater, ContainerRef, DropSelectedItem, DropStack, EquipArmor,
    KeepAliveReply, LoginStart, LoginSuccess, MoveContainerStack, MoveCraftingStack,
    MoveInventoryStack, MoveStackPartial, OpenContainer, PlaceBlock, PlaceWater, PlayIntent,
    PlayerInput, ProtocolCodec, QuickMoveStack, RequestChunkResync, SelectHotbar, ServerHello,
    ServerPacket, State, TakeCraftingOutput, TillSoil, admit_login, encode_uvarint, read_frame_ref,
    write_frame,
};
use mornlea_server::contracts::{
    Clock, CloseReason, CompanionActionEnvelope, CompanionReceipt, ConnectionId,
    ConnectionProgress, Deadline, LoadPoll, LoginPoll, LoginTicket, PlayerLoadPort,
    PublicationPort, ServerEndpoint, ServerError, ServerLimits, SessionFacts, SessionKey,
    SessionPhase, ShutdownFailure, SubmissionReceipt, TickBudget, TickPublication, TransportKind,
};
use mornlea_server::state::AuthorityState;
use mornlea_server::transport::common::TransportAuthority;
use mornlea_server::transport::memory::MemoryTransport;
use mornlea_server::transport::tcp::TcpTransport;

const WORLD_SEED: i64 = 7;
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Injected monotonic clock. No case sleeps for protocol behavior; the TCP
/// peer sockets carry timeouts only so a stuck server fails loudly.
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

/// Immediately resolving load port: every started ticket holds an absent
/// stored record, so the next poll loads a canonical new player. A staged
/// player record overrides the absent form for that identity's next login,
/// which is how the publication cases place logged-in players at exact
/// positions and inventories.
#[derive(Default)]
struct ImmediateLoad {
    next_ticket: u64,
    live: BTreeMap<u64, Option<mornlea_storage::StoredPlayer>>,
    staged: BTreeMap<[u8; 16], mornlea_storage::StoredPlayer>,
    last: Option<LoginTicket>,
}

impl ImmediateLoad {
    /// Stages one stored record for the named player's next login.
    fn stage_player(&mut self, player: PlayerId, stored: mornlea_storage::StoredPlayer) {
        self.staged.insert(player.bytes(), stored);
    }
}

impl PlayerLoadPort for ImmediateLoad {
    fn start(&mut self, player: PlayerId, _deadline: Deadline) -> Result<LoginTicket, ServerError> {
        self.next_ticket += 1;
        let ticket = LoginTicket::try_from_raw(self.next_ticket)
            .expect("load tickets are nonzero by construction");
        self.live
            .insert(ticket.get(), self.staged.get(&player.bytes()).cloned());
        self.last = Some(ticket);
        Ok(ticket)
    }

    fn poll(&mut self, ticket: LoginTicket) -> LoadPoll {
        if let Some(stored) = self.live.get(&ticket.get()) {
            LoadPoll::Loaded(stored.clone())
        } else {
            LoadPoll::Pending
        }
    }

    fn cancel(&mut self, ticket: LoginTicket) -> Result<(), ServerError> {
        self.live.remove(&ticket.get());
        Ok(())
    }
}

struct TicketRecord {
    session: SessionKey,
    player: PlayerId,
    committed: bool,
}

/// Executing endpoint double shared by both adapters: the frozen session,
/// publication, and endpoint seams over one real authority state. The only
/// behavior under test is the adapter in front of it.
struct RealEndpoint {
    authority: AuthorityState,
    loads: ImmediateLoad,
    tickets: BTreeMap<u64, TicketRecord>,
    submits: Vec<SubmissionReceipt>,
    closes: Vec<SessionKey>,
}

impl RealEndpoint {
    fn new() -> Self {
        let limits = ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576)
            .unwrap()
            .with_view_radius(2);
        Self {
            authority: AuthorityState::try_new(limits, WORLD_SEED).unwrap(),
            loads: ImmediateLoad::default(),
            tickets: BTreeMap::new(),
            submits: Vec::new(),
            closes: Vec::new(),
        }
    }

    fn committed_session(&self, ticket: LoginTicket) -> SessionKey {
        self.tickets[&ticket.get()].session
    }
}

impl ServerEndpoint for RealEndpoint {
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
        unimplemented!("shutdown stays outside the adapter integration surface")
    }
}

impl PublicationPort for RealEndpoint {
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

impl TransportAuthority for RealEndpoint {
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

fn login_packet_with_view(tag: u8, name: &str, view: u8) -> ClientPacket {
    let start = LoginStart::new(player(tag), name, view).unwrap();
    ClientPacket::LoginStart(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap())
}

fn login_packet(tag: u8, name: &str) -> ClientPacket {
    login_packet_with_view(tag, name, 8)
}

fn play_packet(sequence: u64) -> ClientPacket {
    ClientPacket::CloseContainer(CloseContainer::new(sequence))
}

/// One ordered record of everything the transcript produced: session
/// numbering, submit receipts, per-tick counters and domain events, and the
/// drained control frames per session.
#[derive(Debug, PartialEq)]
struct Transcript {
    sessions: Vec<u64>,
    submits: Vec<SubmissionReceipt>,
    ticks: Vec<u64>,
    commands: Vec<usize>,
    carried: Vec<usize>,
    stale: Vec<usize>,
    events: Vec<Vec<String>>,
    drained: Vec<Vec<Vec<u8>>>,
}

fn frame(packet: &ClientPacket) -> Vec<u8> {
    MemoryTransport::encode_frame(packet).unwrap()
}

fn control_frame(packet: &ServerPacket) -> Vec<u8> {
    let mut codec = ProtocolCodec::new().unwrap();
    let mut buffer = vec![0u8; 64];
    let payload = loop {
        match codec.encode_server_into(packet, &mut buffer) {
            Ok(written) => break buffer[..written].to_vec(),
            Err(mornlea_protocol::ProtocolError::OutputTooSmall { needed, .. })
                if needed > buffer.len() =>
            {
                buffer.resize(needed, 0);
            }
            Err(other) => panic!("independent encode failed: {other:?}"),
        }
    };
    write_frame(packet.key().id, &payload).unwrap()
}

fn decode_server(frame: &[u8]) -> ServerPacket {
    let parsed = read_frame_ref(frame).unwrap();
    ProtocolCodec::new()
        .unwrap()
        .decode_server(State::Login, parsed.packet_id, parsed.payload)
        .unwrap()
}

fn tick_counters(publication: &TickPublication) -> (usize, usize, usize) {
    (
        publication.counters.commands,
        publication.counters.carried,
        publication.counters.stale,
    )
}

fn event_order(publication: &TickPublication) -> Vec<String> {
    publication
        .events
        .iter()
        .map(|event| format!("{event:?}"))
        .collect()
}

/// Runs the shared transcript over the memory adapter: two logins, three
/// sequenced play intents, two real reducer ticks with a stale resubmission
/// between them, publication of each tick, and an outbox drain per session.
fn run_memory_transcript() -> Transcript {
    let mut link = MemoryTransport::new();
    let mut endpoint = RealEndpoint::new();
    let clock = StepClock::new();

    let mut sessions = Vec::new();
    let mut actives = Vec::new();
    for (tag, name) in [(1u8, "Ada"), (2u8, "Bea")] {
        let id = link.connect(clock.monotonic()).unwrap();
        assert_eq!(
            link.send(id, frame(&hello_packet()), &mut endpoint, &clock),
            ConnectionProgress::Advanced { frames: 1 }
        );
        let hello_out = link.receive(id, 8, 1 << 20);
        assert_eq!(hello_out.len(), 1);
        assert_eq!(
            hello_out[0],
            control_frame(&ServerPacket::ServerHello(
                ServerHello::new(protocol()).unwrap()
            ))
        );
        assert_eq!(
            link.acknowledge(id, 1, &mut endpoint),
            ConnectionProgress::Advanced { frames: 1 }
        );
        assert_eq!(
            link.send(id, frame(&login_packet(tag, name)), &mut endpoint, &clock),
            ConnectionProgress::Advanced { frames: 1 }
        );
        let ticket = endpoint.loads.last.expect("login started");
        assert_eq!(
            link.poll(id, &mut endpoint, &clock),
            ConnectionProgress::Advanced { frames: 1 }
        );
        let success_out = link.receive(id, 8, 1 << 20);
        assert_eq!(success_out.len(), 1);
        assert_eq!(
            decode_server(&success_out[0]),
            ServerPacket::LoginSuccess(LoginSuccess::new(player(tag), WORLD_SEED as u64))
        );
        assert_eq!(
            link.acknowledge(id, 1, &mut endpoint),
            ConnectionProgress::Advanced { frames: 1 }
        );
        let session = endpoint.committed_session(ticket);
        assert_eq!(
            endpoint.authority.session(session).unwrap().phase,
            SessionPhase::Active
        );
        sessions.push(session.get());
        actives.push((id, session));
    }
    assert_eq!(sessions, vec![1, 2], "session numbering starts at one");

    // Ada submits sequences 5 and 6, Bea submits 1; arrival order follows the
    // transcript on both adapters.
    let (ada_id, ada) = actives[0];
    let (bea_id, bea) = actives[1];
    for sequence in [5u64, 6] {
        assert_eq!(
            link.send(ada_id, frame(&play_packet(sequence)), &mut endpoint, &clock),
            ConnectionProgress::Advanced { frames: 1 }
        );
    }
    assert_eq!(
        link.send(bea_id, frame(&play_packet(1)), &mut endpoint, &clock),
        ConnectionProgress::Advanced { frames: 1 }
    );
    assert_eq!(
        endpoint.submits,
        vec![
            SubmissionReceipt::QueuedForTick {
                tick: 0,
                arrival_index: 0
            },
            SubmissionReceipt::QueuedForTick {
                tick: 0,
                arrival_index: 1
            },
            SubmissionReceipt::QueuedForTick {
                tick: 0,
                arrival_index: 0
            },
        ]
    );

    // The first real tick drains all three envelopes with nothing stale.
    let first = endpoint.authority.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(first.tick, 0);
    assert_eq!(tick_counters(&first), (3, 0, 0));
    let first_events = event_order(&first);
    endpoint.authority.publish(first).unwrap();

    // Ada resubmits her older sequence 4 while Bea advances to 2: the older
    // sequence sits below her applied watermark, so the second tick reports
    // exactly one stale refusal beside one fresh command.
    assert_eq!(
        link.send(ada_id, frame(&play_packet(4)), &mut endpoint, &clock),
        ConnectionProgress::Advanced { frames: 1 }
    );
    assert_eq!(
        link.send(bea_id, frame(&play_packet(2)), &mut endpoint, &clock),
        ConnectionProgress::Advanced { frames: 1 }
    );
    let second = endpoint.authority.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(second.tick, 1);
    assert_eq!(tick_counters(&second), (2, 0, 1));
    let second_events = event_order(&second);
    endpoint.authority.publish(second).unwrap();

    let mut drained = Vec::new();
    for (_, session) in &actives {
        drained.push(MemoryTransport::drain_session(&mut endpoint, *session, 8, 1 << 20).unwrap());
    }

    for (id, session) in actives {
        link.close(id, CloseReason::PeerGone, &mut endpoint);
        assert_eq!(
            endpoint.authority.session(session).unwrap().phase,
            SessionPhase::Retired
        );
    }
    assert_eq!(endpoint.closes, vec![ada, bea]);

    Transcript {
        sessions,
        submits: endpoint.submits,
        ticks: vec![0, 1],
        commands: vec![3, 2],
        carried: vec![0, 0],
        stale: vec![0, 1],
        events: vec![first_events, second_events],
        drained,
    }
}

/// Blocking loopback peer. Reads carry a generous timeout so a stuck server
/// fails the case instead of hanging the suite; the timeout never drives
/// protocol behavior.
struct ClientConn {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl ClientConn {
    fn connect(addr: SocketAddr) -> Self {
        let stream = TcpStream::connect(addr).expect("loopback connect succeeds");
        stream
            .set_read_timeout(Some(CLIENT_TIMEOUT))
            .expect("read timeout installs");
        stream
            .set_write_timeout(Some(CLIENT_TIMEOUT))
            .expect("write timeout installs");
        Self {
            stream,
            buf: Vec::new(),
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.stream
            .write_all(bytes)
            .expect("loopback send succeeds");
    }

    /// Reads one complete envelope, keeping any coalesced suffix buffered.
    fn next_frame(&mut self) -> Vec<u8> {
        loop {
            match read_frame_ref(&self.buf) {
                Ok(frame) => {
                    let envelope = self.buf[..frame.consumed].to_vec();
                    self.buf.drain(..frame.consumed);
                    return envelope;
                }
                Err(mornlea_protocol::ProtocolError::Truncated) => {
                    let mut chunk = [0u8; 4096];
                    match self.stream.read(&mut chunk) {
                        Ok(0) => panic!("stream ended before the owed frame"),
                        Ok(read) => self.buf.extend_from_slice(&chunk[..read]),
                        Err(error) => panic!("loopback read failed: {error:?}"),
                    }
                }
                Err(error) => panic!("server sent a malformed frame: {error:?}"),
            }
        }
    }
}

struct TcpHarness {
    server: TcpTransport,
    endpoint: RealEndpoint,
    clock: StepClock,
}

impl TcpHarness {
    fn new() -> Self {
        Self {
            server: TcpTransport::bind_loopback().expect("loopback bind succeeds"),
            endpoint: RealEndpoint::new(),
            clock: StepClock::new(),
        }
    }

    fn addr(&self) -> SocketAddr {
        self.server.local_addr().expect("listener has an address")
    }
}

/// Kernel delivery waits do not advance the injected protocol clock.
fn delivery_turn(deadline: Instant) {
    std::thread::sleep(
        deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(1)),
    );
}

fn accept_next(harness: &mut TcpHarness) -> ConnectionId {
    let deadline = Instant::now() + DELIVERY_TIMEOUT;
    while Instant::now() < deadline {
        match harness
            .server
            .accept_one(&mut harness.endpoint, &harness.clock)
        {
            Ok(Some(id)) => return id,
            Ok(None) => delivery_turn(deadline),
            Err(error) => panic!("accept failed: {error:?}"),
        }
    }
    panic!("listener produced no connection");
}

/// Allows delayed loopback delivery while retaining virtual protocol time.
fn spin_frames(harness: &mut TcpHarness, id: ConnectionId, want: usize) {
    let mut total = 0;
    let deadline = Instant::now() + DELIVERY_TIMEOUT;
    while Instant::now() < deadline {
        match harness
            .server
            .pump_in(id, &mut harness.endpoint, &harness.clock)
        {
            ConnectionProgress::Advanced { frames } => {
                total += frames;
                if total >= want {
                    return;
                }
            }
            ConnectionProgress::AwaitMore => delivery_turn(deadline),
            closed => panic!("connection closed while waiting for frames: {closed:?}"),
        }
    }
    panic!("ingress spin exhausted waiting for {want} frames, got {total}");
}

fn hello_frame() -> Vec<u8> {
    write_frame(0, &encode_uvarint(protocol())).unwrap()
}

fn login_start_frame_with_view(tag: u8, name: &str, view: u8) -> Vec<u8> {
    let start = LoginStart::new(player(tag), name, view).unwrap();
    write_frame(0, &start.encode().unwrap()).unwrap()
}

fn login_start_frame(tag: u8, name: &str) -> Vec<u8> {
    login_start_frame_with_view(tag, name, 8)
}

fn tcp_play_frame(sequence: u64) -> Vec<u8> {
    let record = CloseContainer::new(sequence);
    write_frame(CloseContainer::PACKET_ID, &record.encode().unwrap()).unwrap()
}

/// Drives one loopback client from connect through the acknowledged success
/// handoff, checking every wire byte against an independent encoding.
fn tcp_login(
    harness: &mut TcpHarness,
    tag: u8,
    name: &str,
) -> (ConnectionId, ClientConn, SessionKey) {
    let mut client = ClientConn::connect(harness.addr());
    let id = accept_next(harness);
    client.send(&hello_frame());
    spin_frames(&mut *harness, id, 1);
    let (sent, _) = harness.server.flush_out(id, &mut harness.endpoint);
    assert_eq!(sent, 1, "hello answer flushes exactly one frame");
    let expected_hello = ServerPacket::ServerHello(ServerHello::new(protocol()).unwrap());
    assert_eq!(client.next_frame(), control_frame(&expected_hello));

    client.send(&login_start_frame(tag, name));
    spin_frames(&mut *harness, id, 1);
    let ticket = harness.endpoint.loads.last.expect("login started");
    match harness
        .server
        .poll(id, &mut harness.endpoint, &harness.clock)
    {
        ConnectionProgress::Advanced { frames } => assert_eq!(frames, 1),
        other => panic!("ready load queues the success frame, got {other:?}"),
    }
    let (sent, _) = harness.server.flush_out(id, &mut harness.endpoint);
    assert_eq!(sent, 1, "success flushes exactly once");
    let expected_success =
        ServerPacket::LoginSuccess(LoginSuccess::new(player(tag), WORLD_SEED as u64));
    assert_eq!(client.next_frame(), control_frame(&expected_success));
    let session = harness.endpoint.committed_session(ticket);
    (id, client, session)
}

/// Runs the same transcript over the TCP adapter: the same logins, the same
/// play intents, the same two real ticks, the same drains. The outbox drain
/// reads through the endpoint seam so the payload bytes stay comparable with
/// the memory run; the handshake above already proved the socket framing.
fn run_tcp_transcript() -> Transcript {
    let mut harness = TcpHarness::new();

    let (ada_id, mut ada_client, ada) = tcp_login(&mut harness, 1, "Ada");
    let (bea_id, mut bea_client, bea) = tcp_login(&mut harness, 2, "Bea");
    assert_eq!(ada.get(), 1, "first login takes the first number");
    assert_eq!(bea.get(), 2, "numbering continues across logins");

    ada_client.send(&tcp_play_frame(5));
    spin_frames(&mut harness, ada_id, 1);
    ada_client.send(&tcp_play_frame(6));
    spin_frames(&mut harness, ada_id, 1);
    bea_client.send(&tcp_play_frame(1));
    spin_frames(&mut harness, bea_id, 1);
    assert_eq!(
        harness.endpoint.submits,
        vec![
            SubmissionReceipt::QueuedForTick {
                tick: 0,
                arrival_index: 0
            },
            SubmissionReceipt::QueuedForTick {
                tick: 0,
                arrival_index: 1
            },
            SubmissionReceipt::QueuedForTick {
                tick: 0,
                arrival_index: 0
            },
        ]
    );

    let first = harness
        .endpoint
        .authority
        .advance_tick(TickBudget::full())
        .unwrap();
    assert_eq!(first.tick, 0);
    assert_eq!(tick_counters(&first), (3, 0, 0));
    let first_events = event_order(&first);
    harness.endpoint.authority.publish(first).unwrap();

    ada_client.send(&tcp_play_frame(4));
    spin_frames(&mut harness, ada_id, 1);
    bea_client.send(&tcp_play_frame(2));
    spin_frames(&mut harness, bea_id, 1);
    let second = harness
        .endpoint
        .authority
        .advance_tick(TickBudget::full())
        .unwrap();
    assert_eq!(second.tick, 1);
    assert_eq!(tick_counters(&second), (2, 0, 1));
    let second_events = event_order(&second);
    harness.endpoint.authority.publish(second).unwrap();

    let mut drained = Vec::new();
    for session in [ada, bea] {
        drained.push(harness.endpoint.take_outbox(session, 8, 1 << 20).unwrap());
    }

    harness
        .server
        .close(ada_id, CloseReason::PeerGone, &mut harness.endpoint);
    harness
        .server
        .close(bea_id, CloseReason::PeerGone, &mut harness.endpoint);
    assert_eq!(
        harness.endpoint.authority.session(ada).unwrap().phase,
        SessionPhase::Retired
    );
    assert_eq!(
        harness.endpoint.authority.session(bea).unwrap().phase,
        SessionPhase::Retired
    );
    let _ = (ada_client, bea_client);

    Transcript {
        sessions: vec![ada.get(), bea.get()],
        submits: harness.endpoint.submits,
        ticks: vec![0, 1],
        commands: vec![3, 2],
        carried: vec![0, 0],
        stale: vec![0, 1],
        events: vec![first_events, second_events],
        drained,
    }
}

/// The complete provider inventory replays identically over Memory and TCP:
/// ordered receipts, counters, domain events, and drained control outputs
/// match exactly, so neither adapter invents or drops transcript behavior.
#[test]
fn shared_transcript_matches_across_adapters() {
    let memory = run_memory_transcript();
    let tcp = run_tcp_transcript();
    assert_eq!(memory.sessions, vec![1, 2]);
    assert_eq!(tcp.sessions, vec![1, 2]);
    assert_eq!(memory, tcp, "adapters serve one transcript identically");
}

/// A TCP disconnect with a save still pending retires the session but keeps
/// the selected snapshots in flight: the reported outcome is a retired
/// session beside a fully acked save once the store completion lands.
#[test]
fn tcp_disconnect_during_pending_save_keeps_save_in_flight() {
    use mornlea_domain::{ChunkPos, Dimension};
    use mornlea_server::contracts::{
        ChunkKey, OwnedSnapshot, SaveBudget, SaveCompletion, SaveKey, SaveMode, SaveUrgency,
        SaveValue,
    };
    use mornlea_storage::{
        ChestSlot, Chunk, ContainerSnapshot, DropSlot, FurnaceSlot, StorageKind,
    };

    let mut harness = TcpHarness::new();
    let (id, mut client, session) = tcp_login(&mut harness, 1, "Ada");
    client.send(&tcp_play_frame(5));
    spin_frames(&mut harness, id, 1);
    assert_eq!(harness.endpoint.submits.len(), 1);

    // Stage one pending chunk save through the real authority dirty lane,
    // then select it: the snapshot is in flight when the peer drops.
    let key = SaveKey::Chunk(ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    });
    let snapshot = OwnedSnapshot::try_new(
        key.clone(),
        9,
        1024,
        SaveUrgency::Autosave,
        SaveValue::Chunk(mornlea_storage::ChunkSave {
            key: mornlea_storage::ChunkKey {
                dimension: 0,
                x: 0,
                z: 0,
            },
            revision: 9,
            chunk: Chunk {
                sections: vec![
                    ContainerSnapshot {
                        kind: StorageKind::Single,
                        bits: 0,
                        single: 2,
                        palette: Vec::new(),
                        packed: Vec::new(),
                    };
                    24
                ],
                drops: vec![DropSlot::default(); 32],
                furnaces: vec![FurnaceSlot::default(); 32],
                chests: vec![ChestSlot::default(); 16],
            },
        }),
    )
    .unwrap();
    harness.endpoint.authority.remember_dirty(snapshot).unwrap();
    let selected = harness.endpoint.authority.select(
        SaveMode::All,
        SaveBudget {
            chunks: 8,
            estimated_bytes: 4 << 20,
        },
    );
    assert_eq!(selected.len(), 1, "one chunk save is pending");
    assert_eq!(harness.endpoint.authority.save_stats().in_flight, 1);

    // The peer disconnects mid-save: the connection closes, the session
    // retires, and the pending save stays in flight instead of vanishing.
    harness
        .server
        .close(id, CloseReason::PeerGone, &mut harness.endpoint);
    assert_eq!(
        harness.endpoint.authority.session(session).unwrap().phase,
        SessionPhase::Retired,
        "disconnect retires the session"
    );
    assert_eq!(
        harness.endpoint.authority.save_stats().in_flight,
        1,
        "the pending save survives the disconnect"
    );

    // The store completion still lands against the retained snapshot: the
    // save acks exactly the selected key and revision after the close.
    let submitted: Vec<(SaveKey, u64)> = selected
        .iter()
        .map(|snapshot| (snapshot.key.clone(), snapshot.revision))
        .collect();
    let report = harness.endpoint.authority.apply_completion(SaveCompletion {
        ticket: mornlea_server::contracts::SaveTicket::try_from_raw(1).unwrap(),
        snapshots: selected,
        submitted,
        committed: vec![(key.clone(), 9)],
        error: None,
    });
    assert_eq!(report.acked, 1, "pending save acks after disconnect");
    assert!(report.retry.is_empty());
    assert_eq!(harness.endpoint.authority.save_stats().in_flight, 0);
    let _ = client;
}

/// Number of inventory packets submitted before the first tick; the
/// remaining packets plus one stale resubmission land between the two ticks.
const INVENTORY_FIRST_TICK_PACKETS: usize = 12;

/// One valid wire packet for every `command.protocol.client.*` row of the
/// capability inventory: the nineteen sequenced command families, the chat
/// channel, and the keep alive reply. Every record comes from its family's
/// checked constructor with the parameters the protocol crate's own suites
/// pin, so admission can never depend on a hand-packed byte. Ownership
/// alternates Ada (tag 1) and Bea (tag 2) in row order, so each player's
/// sequenced families carry strictly increasing sequence numbers.
fn inventory_packets() -> Vec<(&'static str, u8, ClientPacket)> {
    let furnace = ContainerRef {
        dimension: 0,
        chunk_x: -1,
        chunk_z: 2,
        kind: CONTAINER_KIND_FURNACE,
        slot: 31,
        generation: 1,
    };
    vec![
        (
            "BoneMeal",
            1,
            ClientPacket::BoneMeal(BoneMeal::new(1, -0.0, 1.5).unwrap()),
        ),
        (
            "ChatCommand",
            1,
            ClientPacket::ChatCommand(ChatCommand::new("chop oak".to_owned()).unwrap()),
        ),
        (
            "CloseContainer",
            2,
            ClientPacket::CloseContainer(CloseContainer::new(1)),
        ),
        (
            "CollectWater",
            1,
            ClientPacket::CollectWater(CollectWater::new(2, -0.0, 1.5).unwrap()),
        ),
        (
            "DropSelectedItem",
            2,
            ClientPacket::DropSelectedItem(DropSelectedItem::new(2)),
        ),
        (
            "DropStack",
            1,
            ClientPacket::DropStack(DropStack::new(3, ContainerRef::NONE, 0, 35).unwrap()),
        ),
        (
            "EquipArmor",
            2,
            ClientPacket::EquipArmor(EquipArmor::new(3)),
        ),
        (
            "KeepAliveReply",
            2,
            ClientPacket::KeepAliveReply(KeepAliveReply::new(6).unwrap()),
        ),
        (
            "MoveContainerStack",
            1,
            ClientPacket::MoveContainerStack(MoveContainerStack::new(4, furnace, 0, 37).unwrap()),
        ),
        (
            "MoveCraftingStack",
            2,
            ClientPacket::MoveCraftingStack(MoveCraftingStack::new(4, 8, 44).unwrap()),
        ),
        (
            "MoveInventoryStack",
            1,
            ClientPacket::MoveInventoryStack(MoveInventoryStack::new(5, 0, 35).unwrap()),
        ),
        (
            "MoveStackPartial",
            2,
            ClientPacket::MoveStackPartial(
                MoveStackPartial::new(5, ContainerRef::NONE, 1, 9, 44, false).unwrap(),
            ),
        ),
        (
            "OpenContainer",
            1,
            ClientPacket::OpenContainer(OpenContainer::new(6, -0.0, 1.5).unwrap()),
        ),
        (
            "PlaceBlock",
            2,
            ClientPacket::PlaceBlock(PlaceBlock::new(6, 1.25, -0.0, 8).unwrap()),
        ),
        (
            "PlaceWater",
            1,
            ClientPacket::PlaceWater(PlaceWater::new(7, -0.0, 1.5).unwrap()),
        ),
        (
            "PlayerInput",
            2,
            ClientPacket::PlayerInput(
                PlayerInput::new(7, 1, -1, true, -0.0, 1.5, true, false, true, false).unwrap(),
            ),
        ),
        (
            "QuickMoveStack",
            1,
            ClientPacket::QuickMoveStack(
                QuickMoveStack::new(8, ContainerRef::NONE, 1, 44).unwrap(),
            ),
        ),
        (
            "RequestChunkResync",
            2,
            ClientPacket::RequestChunkResync(RequestChunkResync::new(
                8,
                mornlea_domain::Dimension::OVERWORLD,
                -2,
                5,
                9,
            )),
        ),
        (
            "SelectHotbar",
            1,
            ClientPacket::SelectHotbar(SelectHotbar::new(9, 8).unwrap()),
        ),
        (
            "TakeCraftingOutput",
            2,
            ClientPacket::TakeCraftingOutput(TakeCraftingOutput::new(9).unwrap()),
        ),
        (
            "TillSoil",
            1,
            ClientPacket::TillSoil(TillSoil::new(10, -0.0, 1.5).unwrap()),
        ),
    ]
}

/// Submits one slice of the inventory transcript through the endpoint seam,
/// converting each packet with the same packet-to-intent conversion the
/// transports run, so the transport-free replay feeds the authority
/// identical intents with the identical sequence layout.
fn submit_inventory_slice(
    endpoint: &mut RealEndpoint,
    packets: &[(&'static str, u8, ClientPacket)],
    ada: SessionKey,
    bea: SessionKey,
) {
    for (_, owner, packet) in packets {
        let session = if *owner == 1 { ada } else { bea };
        let intent = PlayIntent::try_from(packet.clone()).unwrap();
        endpoint.submit(session, intent).unwrap();
    }
}

/// Runs the complete command inventory over the memory adapter: both logins,
/// the first packet slice, a real tick, the remaining packets plus Ada's
/// stale resubmission of her already-applied CollectWater sequence, a second
/// real tick, and an outbox drain per session.
fn run_inventory_memory() -> (Transcript, Vec<SessionFacts>) {
    let mut link = MemoryTransport::new();
    let mut endpoint = RealEndpoint::new();
    let clock = StepClock::new();

    let mut sessions = Vec::new();
    let mut actives = Vec::new();
    for (tag, name) in [(1u8, "Ada"), (2u8, "Bea")] {
        let id = link.connect(clock.monotonic()).unwrap();
        assert_eq!(
            link.send(id, frame(&hello_packet()), &mut endpoint, &clock),
            ConnectionProgress::Advanced { frames: 1 }
        );
        let hello_out = link.receive(id, 8, 1 << 20);
        assert_eq!(hello_out.len(), 1);
        assert_eq!(
            hello_out[0],
            control_frame(&ServerPacket::ServerHello(
                ServerHello::new(protocol()).unwrap()
            ))
        );
        assert_eq!(
            link.acknowledge(id, 1, &mut endpoint),
            ConnectionProgress::Advanced { frames: 1 }
        );
        assert_eq!(
            link.send(id, frame(&login_packet(tag, name)), &mut endpoint, &clock),
            ConnectionProgress::Advanced { frames: 1 }
        );
        let ticket = endpoint.loads.last.expect("login started");
        assert_eq!(
            link.poll(id, &mut endpoint, &clock),
            ConnectionProgress::Advanced { frames: 1 }
        );
        let success_out = link.receive(id, 8, 1 << 20);
        assert_eq!(success_out.len(), 1);
        assert_eq!(
            decode_server(&success_out[0]),
            ServerPacket::LoginSuccess(LoginSuccess::new(player(tag), WORLD_SEED as u64))
        );
        assert_eq!(
            link.acknowledge(id, 1, &mut endpoint),
            ConnectionProgress::Advanced { frames: 1 }
        );
        let session = endpoint.committed_session(ticket);
        assert_eq!(
            endpoint.authority.session(session).unwrap().phase,
            SessionPhase::Active
        );
        sessions.push(session.get());
        actives.push((id, session));
    }
    assert_eq!(sessions, vec![1, 2], "session numbering starts at one");

    let packets = inventory_packets();
    let (ada_id, ada) = actives[0];
    let (bea_id, bea) = actives[1];
    for (family, owner, packet) in &packets[..INVENTORY_FIRST_TICK_PACKETS] {
        let (id, _) = if *owner == 1 {
            (ada_id, ada)
        } else {
            (bea_id, bea)
        };
        assert_eq!(
            link.send(id, frame(packet), &mut endpoint, &clock),
            ConnectionProgress::Advanced { frames: 1 },
            "the memory adapter admits the {family} family"
        );
    }

    // Ten sequenced commands drain in the first tick with nothing stale.
    let first = endpoint.authority.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(first.tick, 0);
    assert_eq!(tick_counters(&first), (10, 0, 0));
    let first_events = event_order(&first);
    endpoint.authority.publish(first).unwrap();

    for (family, owner, packet) in &packets[INVENTORY_FIRST_TICK_PACKETS..] {
        let (id, _) = if *owner == 1 {
            (ada_id, ada)
        } else {
            (bea_id, bea)
        };
        assert_eq!(
            link.send(id, frame(packet), &mut endpoint, &clock),
            ConnectionProgress::Advanced { frames: 1 },
            "the memory adapter admits the {family} family"
        );
    }
    // Ada resubmits her already-applied CollectWater sequence, which sits
    // below her applied watermark, so the second tick reports exactly one
    // stale refusal beside nine fresh commands.
    let (_, _, stale) = &packets[3];
    assert_eq!(
        link.send(ada_id, frame(stale), &mut endpoint, &clock),
        ConnectionProgress::Advanced { frames: 1 },
        "the stale resubmit still enters the memory adapter"
    );
    assert_eq!(endpoint.submits.len(), 22);
    let second = endpoint.authority.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(second.tick, 1);
    assert_eq!(tick_counters(&second), (10, 0, 1));
    let second_events = event_order(&second);
    endpoint.authority.publish(second).unwrap();

    let mut drained = Vec::new();
    for (_, session) in &actives {
        drained.push(MemoryTransport::drain_session(&mut endpoint, *session, 64, 4 << 20).unwrap());
    }
    let facts = vec![
        endpoint.authority.session(ada).unwrap(),
        endpoint.authority.session(bea).unwrap(),
    ];

    for (id, session) in actives {
        link.close(id, CloseReason::PeerGone, &mut endpoint);
        assert_eq!(
            endpoint.authority.session(session).unwrap().phase,
            SessionPhase::Retired
        );
    }
    assert_eq!(endpoint.closes, vec![ada, bea]);

    (
        Transcript {
            sessions,
            submits: endpoint.submits,
            ticks: vec![0, 1],
            commands: vec![10, 10],
            carried: vec![0, 0],
            stale: vec![0, 1],
            events: vec![first_events, second_events],
            drained,
        },
        facts,
    )
}

/// Runs the same command inventory over the TCP adapter with the same packet
/// split and tick structure, so the transcript stays byte-comparable with
/// the memory run.
fn run_inventory_tcp() -> (Transcript, Vec<SessionFacts>) {
    let mut harness = TcpHarness::new();

    let (ada_id, mut ada_client, ada) = tcp_login(&mut harness, 1, "Ada");
    let (bea_id, mut bea_client, bea) = tcp_login(&mut harness, 2, "Bea");
    assert_eq!(ada.get(), 1, "first login takes the first number");
    assert_eq!(bea.get(), 2, "numbering continues across logins");

    let packets = inventory_packets();
    for (family, owner, packet) in &packets[..INVENTORY_FIRST_TICK_PACKETS] {
        let (id, client) = if *owner == 1 {
            (ada_id, &mut ada_client)
        } else {
            (bea_id, &mut bea_client)
        };
        let before = harness.endpoint.submits.len();
        client.send(&frame(packet));
        spin_frames(&mut harness, id, 1);
        assert_eq!(
            harness.endpoint.submits.len(),
            before + 1,
            "the TCP adapter admits the {family} family"
        );
    }

    let first = harness
        .endpoint
        .authority
        .advance_tick(TickBudget::full())
        .unwrap();
    assert_eq!(first.tick, 0);
    assert_eq!(tick_counters(&first), (10, 0, 0));
    let first_events = event_order(&first);
    harness.endpoint.authority.publish(first).unwrap();

    for (family, owner, packet) in &packets[INVENTORY_FIRST_TICK_PACKETS..] {
        let (id, client) = if *owner == 1 {
            (ada_id, &mut ada_client)
        } else {
            (bea_id, &mut bea_client)
        };
        let before = harness.endpoint.submits.len();
        client.send(&frame(packet));
        spin_frames(&mut harness, id, 1);
        assert_eq!(
            harness.endpoint.submits.len(),
            before + 1,
            "the TCP adapter admits the {family} family"
        );
    }
    let (_, _, stale) = &packets[3];
    ada_client.send(&frame(stale));
    spin_frames(&mut harness, ada_id, 1);
    assert_eq!(harness.endpoint.submits.len(), 22);
    let second = harness
        .endpoint
        .authority
        .advance_tick(TickBudget::full())
        .unwrap();
    assert_eq!(second.tick, 1);
    assert_eq!(tick_counters(&second), (10, 0, 1));
    let second_events = event_order(&second);
    harness.endpoint.authority.publish(second).unwrap();

    let mut drained = Vec::new();
    for session in [ada, bea] {
        drained.push(harness.endpoint.take_outbox(session, 64, 4 << 20).unwrap());
    }
    let facts = vec![
        harness.endpoint.authority.session(ada).unwrap(),
        harness.endpoint.authority.session(bea).unwrap(),
    ];

    harness
        .server
        .close(ada_id, CloseReason::PeerGone, &mut harness.endpoint);
    harness
        .server
        .close(bea_id, CloseReason::PeerGone, &mut harness.endpoint);
    assert_eq!(
        harness.endpoint.authority.session(ada).unwrap().phase,
        SessionPhase::Retired
    );
    assert_eq!(
        harness.endpoint.authority.session(bea).unwrap().phase,
        SessionPhase::Retired
    );
    assert_eq!(harness.endpoint.closes, vec![ada, bea]);

    (
        Transcript {
            sessions: vec![ada.get(), bea.get()],
            submits: harness.endpoint.submits,
            ticks: vec![0, 1],
            commands: vec![10, 10],
            carried: vec![0, 0],
            stale: vec![0, 1],
            events: vec![first_events, second_events],
            drained,
        },
        facts,
    )
}

/// Replays the same command inventory against a third endpoint with no
/// transport at all: sessions rise through the same login seams, packets
/// enter through the same conversion the adapters run, and the replay keeps
/// only logical facts — per-tick ordered events and session facts.
fn run_inventory_direct() -> (Vec<Vec<String>>, Vec<SessionFacts>) {
    let mut endpoint = RealEndpoint::new();
    let clock = StepClock::new();

    let mut sessions = Vec::new();
    for (tag, name) in [(1u8, "Ada"), (2u8, "Bea")] {
        let start = LoginStart::new(player(tag), name, 8).unwrap();
        let inbound = LoginStart::decode_inbound(&start.encode().unwrap()).unwrap();
        let admitted = admit_login(inbound).unwrap();
        let deadline = Deadline::after(clock.monotonic(), CLIENT_TIMEOUT).unwrap();
        let ticket = endpoint
            .begin_login(admitted, TransportKind::Memory, deadline)
            .unwrap();
        let session = match endpoint.poll_login(ticket) {
            LoginPoll::Ready { session, .. } => session,
            other => panic!("the immediate load resolves on the first poll: {other:?}"),
        };
        assert_eq!(endpoint.commit_login(ticket).unwrap(), session);
        sessions.push(session);
    }

    let packets = inventory_packets();
    let (ada, bea) = (sessions[0], sessions[1]);
    submit_inventory_slice(
        &mut endpoint,
        &packets[..INVENTORY_FIRST_TICK_PACKETS],
        ada,
        bea,
    );
    let first = endpoint.authority.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(first.tick, 0);
    assert_eq!(tick_counters(&first), (10, 0, 0));
    let first_events = event_order(&first);
    endpoint.authority.publish(first).unwrap();

    submit_inventory_slice(
        &mut endpoint,
        &packets[INVENTORY_FIRST_TICK_PACKETS..],
        ada,
        bea,
    );
    let (_, _, stale) = &packets[3];
    let intent = PlayIntent::try_from(stale.clone()).unwrap();
    endpoint.submit(ada, intent).unwrap();
    let second = endpoint.authority.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(second.tick, 1);
    assert_eq!(tick_counters(&second), (10, 0, 1));
    let second_events = event_order(&second);
    endpoint.authority.publish(second).unwrap();

    let facts = vec![
        endpoint.authority.session(ada).unwrap(),
        endpoint.authority.session(bea).unwrap(),
    ];
    (vec![first_events, second_events], facts)
}

/// The complete client-command inventory — one packet per
/// `command.protocol.client.*` row of the capability inventory — flows
/// through both real adapters with identical ordered outputs and identical
/// authority state, and a transport-free replay of the same transcript
/// produces the same ordered domain events and the same logical session
/// facts.
#[test]
fn inventory_command_transcripts_match_across_adapters() {
    let (memory, memory_facts) = run_inventory_memory();
    let (tcp, tcp_facts) = run_inventory_tcp();
    let (direct_events, direct_facts) = run_inventory_direct();

    assert_eq!(
        memory, tcp,
        "adapters serve the command inventory identically"
    );
    assert_eq!(
        tcp_facts, memory_facts,
        "both adapters leave the same session facts"
    );
    assert_eq!(
        direct_events, memory.events,
        "the direct replay produces the same ordered domain events"
    );
    assert_eq!(
        direct_facts, memory_facts,
        "the direct replay leaves the same logical session facts"
    );

    assert_eq!(memory_facts[0].phase, SessionPhase::Active);
    assert_eq!(
        memory_facts[0].last_applied_sequence, 10,
        "Ada applied her highest sequence"
    );
    assert_eq!(
        memory_facts[0].next_arrival, 11,
        "the stale resubmit still consumed an arrival slot"
    );
    assert_eq!(memory_facts[1].phase, SessionPhase::Active);
    assert_eq!(memory_facts[1].last_applied_sequence, 9);
    assert_eq!(memory_facts[1].next_arrival, 9);
    assert_eq!(
        memory
            .submits
            .iter()
            .filter(|receipt| matches!(receipt, SubmissionReceipt::ControlAccepted))
            .count(),
        2,
        "chat and keep alive are the only control receipts"
    );
}

/// One-shot scratch world directory for the real disk owner, removed on
/// drop so a failed case never leaks a world directory behind it.
struct ScratchRoot(PathBuf);

impl ScratchRoot {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mornlea-parity-disk-{}-{}-{tag}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&path).expect("scratch world directory creates");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for ScratchRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A TCP disconnect drops the peer while a save is still pending (selected
/// but uncommitted); the real disk completion settles after the close. The
/// selected snapshot then commits with a real `DiskStore` write and sync on
/// the real exclusive store, the retired session's authority acks the real
/// completion, and a reopened store loads the chunk back with its revisions
/// intact. This proves post-close settlement of a pending save; it does not
/// claim concurrent background-write overlap with the disconnect itself.
///
/// Fixture boundary: the staged chunk snapshot is a synthetic minimal
/// fixture; the completion, sync, and reload are real `DiskStore` I/O on a
/// one-shot temp directory.
#[test]
fn tcp_disconnect_pending_save_completes_through_real_disk_store() {
    use mornlea_domain::{ChunkPos, Dimension};
    use mornlea_server::contracts::{
        ChunkKey, DiskBackend, LoadedValue, OwnedSnapshot, SaveBudget, SaveKey, SaveMode,
        SaveRequest, SaveTicket, SaveUrgency, SaveValue,
    };
    use mornlea_server::store::disk::{DiskOptions, DiskStore};
    use mornlea_storage::{
        ChestSlot, Chunk, ContainerSnapshot, DropSlot, FurnaceSlot, METADATA_CURRENT_VERSION,
        Metadata, MetadataChunkPos, StorageKind,
    };

    let options = |seed: i64| DiskOptions {
        create: Metadata {
            format_version: METADATA_CURRENT_VERSION,
            seed,
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
        region_handle_cap: 1,
    };

    // The setup of the in-flight case: one login, one play submit, one chunk
    // save staged through the real dirty lane and selected, then the peer
    // drops while the save is in flight.
    let mut harness = TcpHarness::new();
    let (id, mut client, session) = tcp_login(&mut harness, 1, "Ada");
    client.send(&tcp_play_frame(5));
    spin_frames(&mut harness, id, 1);
    assert_eq!(harness.endpoint.submits.len(), 1);

    let key = SaveKey::Chunk(ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    });
    let snapshot = OwnedSnapshot::try_new(
        key.clone(),
        9,
        1024,
        SaveUrgency::Autosave,
        SaveValue::Chunk(mornlea_storage::ChunkSave {
            key: mornlea_storage::ChunkKey {
                dimension: 0,
                x: 0,
                z: 0,
            },
            revision: 9,
            chunk: Chunk {
                sections: vec![
                    ContainerSnapshot {
                        kind: StorageKind::Single,
                        bits: 0,
                        single: 2,
                        palette: Vec::new(),
                        packed: Vec::new(),
                    };
                    24
                ],
                drops: vec![DropSlot::default(); 32],
                furnaces: vec![FurnaceSlot::default(); 32],
                chests: vec![ChestSlot::default(); 16],
            },
        }),
    )
    .unwrap();
    harness.endpoint.authority.remember_dirty(snapshot).unwrap();
    let selected = harness.endpoint.authority.select(
        SaveMode::All,
        SaveBudget {
            chunks: 8,
            estimated_bytes: 4 << 20,
        },
    );
    assert_eq!(selected.len(), 1, "one chunk save is pending");
    assert_eq!(harness.endpoint.authority.save_stats().in_flight, 1);

    harness
        .server
        .close(id, CloseReason::PeerGone, &mut harness.endpoint);
    assert_eq!(
        harness.endpoint.authority.session(session).unwrap().phase,
        SessionPhase::Retired,
        "disconnect retires the session"
    );
    assert_eq!(
        harness.endpoint.authority.save_stats().in_flight,
        1,
        "the pending save survives the disconnect"
    );

    // The real disk leg: the exclusive owner commits the selected snapshot,
    // syncs, and the authority acks the real completion after the close.
    let root = ScratchRoot::new("tcp-pending-save");
    let mut disk = DiskStore::open(root.path(), options(WORLD_SEED)).unwrap();
    let completion = disk.write(
        SaveTicket::try_from_raw(1).unwrap(),
        SaveRequest {
            snapshots: selected,
        },
    );
    assert!(
        completion.error.is_none(),
        "the real commit reports no error"
    );
    disk.sync().unwrap();
    let report = harness.endpoint.authority.apply_completion(completion);
    assert_eq!(report.acked, 1, "the real completion acks the pending save");
    assert!(report.retry.is_empty());
    assert_eq!(harness.endpoint.authority.save_stats().in_flight, 0);

    // Durability: the owner closes, a fresh owner reopens the same
    // directory, and the chunk loads back with its logical revisions.
    disk.close().unwrap();
    let mut reopened = DiskStore::open(root.path(), options(99)).unwrap();
    match reopened.load(key).expect("the chunk loads after reopen") {
        LoadedValue::Chunk(recovered) => {
            assert_eq!(recovered.revision, 9);
            assert_eq!(recovered.persisted_revision, 9);
        }
        other => panic!("chunk key loaded another family: {other:?}"),
    }
    reopened.close().unwrap();
    let _ = client;
}

// ---------------------------------------------------------------------------
// Publication projection parity: the per-tick publication families proven
// through the real Memory and TCP adapters. Both adapters serve one shared
// driving surface, so every fixture staging, packet submission, and tick runs
// through the identical code path on both sides and the captured publications
// must be exactly equal.
// ---------------------------------------------------------------------------

use mornlea_domain::{
    self, BlockChange, BlockPos, ChatBody, ChatEvent, ChatEventParts, ChunkPos, CommandText,
    CompanionDespawn, CompanionId, CompanionName, CompanionSpawn, CompanionSpawnParts,
    CompanionSpeaker, CompanionState, CompanionStateParts, CompanionStates, ContainerKind,
    Dimension, DisplayName, DropId, Event, EventRecipient, FiniteVec3, ForgetChunks, HostileId,
    HostileKind, HostileSpawn, HostileSpawnParts, HostileSpawnRecord, HostileSpawnRecordParts,
    HostileState, HostileStateParts, HostileStateRecord, HostileStateRecordParts, HotbarSlot,
    InventoryState, InventoryStateParts, ItemDrop, ItemDropParts, ItemDropUpserts, ItemStack,
    LookAngles, MotionState, MotionStateParts, PassiveDespawn, PassiveDespawnParts,
    PassiveDespawnReason, PassiveDespawnRecord, PassiveId, PassiveSpawn, PassiveSpawnParts,
    PassiveSpawnRecord, PassiveSpawnRecordParts, PassiveState, PassiveStateParts,
    PassiveStateRecord, PassiveStateRecordParts, ProjectileId, ProjectileKind, ProjectileSpawn,
    ProjectileSpawnParts, ProjectileSpawnRecord, ProjectileSpawnRecordParts, ProjectileState,
    ProjectileStateParts, ProjectileStateRecord, ProjectileStateRecordParts, RemotePlayerSpawn,
    RemotePlayerSpawnParts, RemotePlayerState, RemotePlayerStateParts, RemotePlayerStates,
    SurvivalState, SurvivalStateParts, TaskState, Weather,
};
use mornlea_protocol::{CONTAINER_KIND_CHEST, STACK_VIEW_CONTAINER};
use mornlea_server::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, AgentPlan,
    AgentRequestId, ChunkKey, CompanionAction, DropRecord, EnvironmentState, PlanStep,
    ProjectileRecord, RuleEffect, RuleTunables, RunId, SnapshotId,
};
use mornlea_server::core::companion_chat::CompanionChatPhase;
use mornlea_server::core::world::ReadyChunk;
use mornlea_server::state::TickContext;
use mornlea_storage::{
    ChestSlot, Chunk, CompanionBody, ContainerSnapshot, FurnaceSlot, HostileMob, Inventory,
    ItemStack as StorageStack, PassiveMob, PlayerId as SavePlayerId, PlayerLocation, StorageKind,
    StoredCompanionTask, StoredPlayer,
};

const GRASS_BLOCK: u16 = 4;
const DIRT_BLOCK: u16 = 3;
const CHEST_BLOCK: u16 = 11;
const FURNACE_BLOCK: u16 = 9;
const ITEM_DIRT: u16 = 2;
const ITEM_STONE: u16 = 1;
const ITEM_STONE_HOE: u16 = 30;
const ITEM_STICK: u16 = 37;
const ITEM_RAW_IRON: u16 = 6;
const ITEM_COAL: u16 = 5;
const DROP_LIFETIME_MARGIN: u32 = 5_997;

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn companion_id(tag: u8) -> CompanionId {
    CompanionId::try_from_bytes(uuid(tag)).unwrap()
}

/// The deterministic published companion name: `companion-` plus the
/// lowercase hex of the identity's first eight bytes.
fn derived_name(id: CompanionId) -> String {
    let mut text = String::from("companion-");
    for byte in &id.bytes()[..8] {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

fn chunk_key(x: i32, z: i32) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(x, z),
    }
}

fn air_chunk() -> Chunk {
    Chunk {
        sections: vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: vec![],
                packed: vec![],
            };
            24
        ],
        drops: vec![Default::default(); 32],
        furnaces: vec![FurnaceSlot::default(); 32],
        chests: vec![ChestSlot::default(); 16],
    }
}

/// Writes one block cell into a compact chunk, converting the touched section
/// to direct storage the way the container fixtures do.
fn set_cell(chunk: &mut Chunk, pos: BlockPos, block: u16) {
    let index = mornlea_domain::chunk_block_index(pos) as usize;
    let section = &mut chunk.sections[index / 4096];
    if section.kind == StorageKind::Single {
        *section = ContainerSnapshot {
            kind: StorageKind::Direct,
            bits: 15,
            single: 0,
            palette: vec![],
            packed: vec![0; 1024],
        };
    }
    section.packed[(index % 4096) / 4] |= u64::from(block) << ((index % 4) * 15);
}

/// A walkable chunk: one grass layer at y 64 across the whole column.
fn ground_chunk() -> Chunk {
    let mut chunk = air_chunk();
    for x in 0..16 {
        for z in 0..16 {
            set_cell(&mut chunk, BlockPos::new(x, 64, z), GRASS_BLOCK);
        }
    }
    chunk
}

/// Installs one active chest slot at the given cell and returns its reference.
fn chest_in_chunk(
    chunk: &mut Chunk,
    pos: BlockPos,
    items: [StorageStack; 27],
) -> mornlea_domain::ContainerRef {
    set_cell(chunk, pos, CHEST_BLOCK);
    let index = mornlea_domain::chunk_block_index(pos) as u32;
    chunk.chests[0] = ChestSlot {
        generation: 1,
        active: true,
        block_index: index,
        items,
    };
    mornlea_domain::ContainerRef::try_new(
        ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
        ContainerKind::Chest,
        0,
        1,
    )
    .unwrap()
}

/// Installs one active furnace slot at the given cell and returns its
/// reference.
fn furnace_in_chunk(
    chunk: &mut Chunk,
    pos: BlockPos,
    input: StorageStack,
    fuel: StorageStack,
    output: StorageStack,
) -> mornlea_domain::ContainerRef {
    set_cell(chunk, pos, FURNACE_BLOCK);
    let index = mornlea_domain::chunk_block_index(pos) as u32;
    chunk.furnaces[1] = FurnaceSlot {
        generation: 1,
        active: true,
        block_index: index,
        input,
        fuel,
        output,
        burn_ticks: 0,
        progress_ticks: 0,
    };
    mornlea_domain::ContainerRef::try_new(
        ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
        ContainerKind::Furnace,
        1,
        1,
    )
    .unwrap()
}

fn day_environment(world_time: u64) -> EnvironmentState {
    EnvironmentState {
        seed: WORLD_SEED,
        next_tick: 0,
        world_time,
        day_phase_offset: 0,
        season_offset: 0,
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty: 0,
        tunables: RuleTunables::source_defaults(),
    }
}

fn survival() -> SurvivalState {
    SurvivalState::try_new(SurvivalStateParts {
        health: 20,
        oxygen: 300,
        hunger: 20,
        saturation_zero: false,
        armor_points: 0,
    })
    .unwrap()
}

fn look(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).unwrap()
}

fn companion_actor(id: CompanionId, position: [f32; 3], yaw: f32) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Companion(id),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        }),
        look(yaw, 0.0),
        survival(),
        ActorBody::Companion(CompanionBody {
            id: SavePlayerId::from_bytes(id.bytes()),
            dimension: 0,
            position,
            yaw,
            pitch: 0.0,
            inventory: Inventory::default(),
        }),
    )
    .unwrap()
}

fn hostile_actor(id: u64, position: [f32; 3], kind: u8) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Hostile(HostileId::try_new(id).unwrap()),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        }),
        look(0.0, 0.0),
        survival(),
        ActorBody::Hostile(HostileMob {
            id,
            dimension: 0,
            position,
            velocity: [0.0; 3],
            on_ground: true,
            yaw: 0.0,
            health: 20,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 20,
            has_target: false,
            player_id: SavePlayerId::from_bytes([0; 16]),
            next_repath_ticks: 0,
            distant_ticks: 0,
            kind,
        }),
    )
    .unwrap()
}

fn passive_actor(id: u64, position: [f32; 3]) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Passive(PassiveId::try_new(id).unwrap()),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        }),
        look(0.0, 0.0),
        survival(),
        ActorBody::Passive(PassiveMob {
            id,
            dimension: 0,
            position,
            velocity: [0.0; 3],
            on_ground: true,
            yaw: 0.0,
            health: 20,
        }),
    )
    .unwrap()
}

/// The neutral passive runtime the combat settlement row requires: an
/// inactive graze lane at home beside the spawn column.
fn passive_runtime(id: PassiveId) -> ActorRuntime {
    ActorRuntime {
        key: ActorKey::Passive(id),
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 0,
        peak_y: 65.0,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Passive {
            home: BlockPos::new(8, 65, 8),
            flee_ticks: 0,
            flee_from: None,
            graze_ticks: 0,
            graze_at: None,
            fresh: false,
        },
    }
}

fn drop_record(slot: u8, age: u32, position: [f32; 3]) -> DropRecord {
    DropRecord {
        id: DropId::try_new(0, ChunkPos::new(0, 0), slot, 1).unwrap(),
        position: FiniteVec3::try_new(position).unwrap(),
        stack: StorageStack {
            item: ITEM_COAL,
            count: 2,
            durability: 0,
        },
        pickup_delay: 5,
        age,
    }
}

fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack::try_new(item, count, 0).unwrap()
}

fn storage(item: u16, count: u8) -> StorageStack {
    StorageStack {
        item,
        count,
        durability: 0,
    }
}

fn storage_array<const N: usize>(filled: &[(usize, u16, u8)]) -> [StorageStack; N] {
    let mut slots = [StorageStack::default(); N];
    for (index, item, count) in filled {
        slots[*index] = storage(*item, *count);
    }
    slots
}

fn item_array<const N: usize>(filled: &[(usize, u16, u8)]) -> [ItemStack; N] {
    let mut slots = [ItemStack::EMPTY; N];
    for (index, item, count) in filled {
        slots[*index] = stack(*item, *count);
    }
    slots
}

/// One stored player the load port hands the login, pinning the exact spawn
/// pose and inventory the publication cases assert against.
fn stored_player(
    tag: u8,
    name: &str,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
    customize: impl FnOnce(&mut StoredPlayer),
) -> StoredPlayer {
    let mut player = StoredPlayer {
        player_id: SavePlayerId::from_bytes(uuid(tag)),
        revision: 1,
        display_name: name.to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position,
        },
        yaw,
        pitch,
        safe: None,
        inventory: Inventory::default(),
        health: 20,
        hunger: 20,
        saturation_milli: 5_000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor: [StorageStack::default(); 4],
        needs_rewrite: false,
    };
    customize(&mut player);
    player
}

/// One tick's events addressed to one session, as exact domain values.
fn session_events(publication: &TickPublication, session: SessionKey) -> Vec<Event> {
    publication
        .events
        .iter()
        .filter(|event| event.recipient() == EventRecipient::Session(session.get()))
        .map(|event| event.event().clone())
        .collect()
}

/// Decodes one drained frame as a Play-state server packet.
fn decode_play(frame: &[u8]) -> ServerPacket {
    let parsed = read_frame_ref(frame).unwrap();
    ProtocolCodec::new()
        .unwrap()
        .decode_server(State::Play, parsed.packet_id, parsed.payload)
        .unwrap()
}

/// One admitted player through either real adapter: its transport connection
/// and the session the authority assigned.
#[derive(Clone)]
struct Peer {
    conn: ConnectionId,
    session: SessionKey,
}

/// The shared driving surface behind both real transports: staged stored
/// logins, client packets through the real transport ingress, one real
/// authority tick per published output, between-tick fixture staging on the
/// committed residents, and per-session outbox drains. Every publication
/// parity case runs the identical scenario through this surface twice.
trait ParityAdapter {
    /// The real endpoint the adapter serves.
    fn endpoint(&mut self) -> &mut RealEndpoint;

    /// Drives one login through the real handshake and returns its handles.
    fn login_with_view(&mut self, tag: u8, name: &str, view: u8) -> Peer;

    /// Drives one login with the legacy radius-two fixture view distance.
    fn login(&mut self, tag: u8, name: &str) -> Peer {
        self.login_with_view(tag, name, 8)
    }

    /// Submits one client packet through the real transport ingress.
    fn send_packet(&mut self, conn: ConnectionId, packet: &ClientPacket);

    /// Drops the peer's connection, retiring its session.
    fn close_peer(&mut self, conn: ConnectionId);

    /// Stages one stored player for the named tag's next login.
    fn stage_login(&mut self, tag: u8, stored: StoredPlayer) {
        self.endpoint().loads.stage_player(player(tag), stored);
    }

    /// Stages fixtures between ticks through the between-tick restage seam,
    /// seeded from the committed residents.
    fn stage(&mut self, staged: Box<dyn FnOnce(&mut TickContext<'_>)>) {
        let authority = &mut self.endpoint().authority;
        let mut context = TickContext::restage(authority, TickBudget::full());
        staged(&mut context);
        let residents = context.resident_snapshot();
        drop(context);
        authority.commit_residents(residents);
    }

    /// One real tick. `advance_tick` itself publishes the tick's frames
    /// into the per-session outboxes; re-publishing would duplicate every
    /// frame, so the default only captures the publication both adapters
    /// must agree on.
    fn tick(&mut self) -> TickPublication {
        self.endpoint()
            .authority
            .advance_tick(TickBudget::full())
            .unwrap()
    }

    /// Drains one session's published frames under the outbox budgets.
    fn drain(&mut self, session: SessionKey) -> Vec<Vec<u8>> {
        self.endpoint().take_outbox(session, 512, 8 << 20).unwrap()
    }
}

/// The walker's foot chunk column inside its own dimension.
fn foot_chunk(adapter: &mut dyn ParityAdapter, session: SessionKey) -> ChunkPos {
    let residents = adapter.endpoint().authority.residents();
    let actor = residents
        .actors
        .iter()
        .find(|actor| actor.key == ActorKey::Player(session))
        .expect("the player actor stays resident");
    let position = actor.motion.position().get();
    ChunkPos::new(
        (position[0].floor() as i32) >> 4,
        (position[2].floor() as i32) >> 4,
    )
}

/// Submits one eastbound sprint through the real ingress, then ticks until
/// the walker's foot chunk reaches `target_x`, publishing and draining every
/// tick so the walk stays inside the outbox bounds. Returns every walk
/// publication in order; the last one carries the interest transition.
fn walk_east(
    adapter: &mut dyn ParityAdapter,
    peers: &[Peer],
    walker: &Peer,
    sequence: u64,
    target_x: i32,
    max_ticks: usize,
) -> Vec<TickPublication> {
    adapter.send_packet(
        walker.conn,
        &ClientPacket::PlayerInput(
            mornlea_protocol::PlayerInput::new(
                sequence,
                0,
                1,
                false,
                -std::f32::consts::FRAC_PI_2,
                0.0,
                false,
                false,
                true,
                false,
            )
            .unwrap(),
        ),
    );
    let mut publications = Vec::new();
    for _ in 0..max_ticks {
        let publication = adapter.tick();
        for peer in peers {
            adapter.drain(peer.session);
        }
        let reached = foot_chunk(adapter, walker.session).x() >= target_x;
        publications.push(publication);
        if reached {
            return publications;
        }
    }
    panic!("the eastbound walk never reached chunk x {target_x} within {max_ticks} ticks");
}

/// The memory adapter: the real in-process transport link in front of the
/// same real endpoint.
struct MemoryParity {
    link: MemoryTransport,
    endpoint: RealEndpoint,
    clock: StepClock,
}

impl MemoryParity {
    fn new() -> Self {
        Self {
            link: MemoryTransport::new(),
            endpoint: RealEndpoint::new(),
            clock: StepClock::new(),
        }
    }
}

impl ParityAdapter for MemoryParity {
    fn endpoint(&mut self) -> &mut RealEndpoint {
        &mut self.endpoint
    }

    fn login_with_view(&mut self, tag: u8, name: &str, view: u8) -> Peer {
        let id = self.link.connect(self.clock.monotonic()).unwrap();
        assert_eq!(
            self.link
                .send(id, frame(&hello_packet()), &mut self.endpoint, &self.clock),
            ConnectionProgress::Advanced { frames: 1 }
        );
        let hello_out = self.link.receive(id, 8, 1 << 20);
        assert_eq!(hello_out.len(), 1);
        assert_eq!(
            self.link.acknowledge(id, 1, &mut self.endpoint),
            ConnectionProgress::Advanced { frames: 1 }
        );
        assert_eq!(
            self.link.send(
                id,
                frame(&login_packet_with_view(tag, name, view)),
                &mut self.endpoint,
                &self.clock
            ),
            ConnectionProgress::Advanced { frames: 1 }
        );
        let ticket = self.endpoint.loads.last.expect("login started");
        assert_eq!(
            self.link.poll(id, &mut self.endpoint, &self.clock),
            ConnectionProgress::Advanced { frames: 1 }
        );
        let success_out = self.link.receive(id, 8, 1 << 20);
        assert_eq!(success_out.len(), 1);
        assert_eq!(
            decode_server(&success_out[0]),
            ServerPacket::LoginSuccess(LoginSuccess::new(player(tag), WORLD_SEED as u64))
        );
        assert_eq!(
            self.link.acknowledge(id, 1, &mut self.endpoint),
            ConnectionProgress::Advanced { frames: 1 }
        );
        Peer {
            conn: id,
            session: self.endpoint.committed_session(ticket),
        }
    }

    fn send_packet(&mut self, conn: ConnectionId, packet: &ClientPacket) {
        assert_eq!(
            self.link
                .send(conn, frame(packet), &mut self.endpoint, &self.clock),
            ConnectionProgress::Advanced { frames: 1 }
        );
    }

    fn close_peer(&mut self, conn: ConnectionId) {
        self.link
            .close(conn, CloseReason::PeerGone, &mut self.endpoint);
    }
}

/// The TCP adapter: the real loopback listener in front of the same real
/// endpoint, holding each peer's client socket for wire reads.
struct TcpParity {
    harness: TcpHarness,
    clients: BTreeMap<u64, ClientConn>,
}

impl TcpParity {
    fn new() -> Self {
        Self {
            harness: TcpHarness::new(),
            clients: BTreeMap::new(),
        }
    }

    /// Forwards one session's published frames onto the live socket and reads
    /// them back through the client's frame reader.
    fn socket_frames(&mut self, peer: &Peer, count: usize) -> Vec<Vec<u8>> {
        let forwarded = self
            .harness
            .server
            .forward_outbox(
                peer.conn,
                peer.session,
                &mut self.harness.endpoint,
                64,
                1 << 20,
            )
            .unwrap();
        assert_eq!(forwarded.len(), count, "the socket receives every frame");
        let client = self.clients.get_mut(&peer.conn.get()).unwrap();
        (0..count).map(|_| client.next_frame()).collect()
    }
}

impl ParityAdapter for TcpParity {
    fn endpoint(&mut self) -> &mut RealEndpoint {
        &mut self.harness.endpoint
    }

    fn login_with_view(&mut self, tag: u8, name: &str, view: u8) -> Peer {
        let mut client = ClientConn::connect(self.harness.addr());
        let id = accept_next(&mut self.harness);
        client.send(&hello_frame());
        spin_frames(&mut self.harness, id, 1);
        let (sent, _) = self
            .harness
            .server
            .flush_out(id, &mut self.harness.endpoint);
        assert_eq!(sent, 1, "hello answer flushes exactly one frame");
        assert_eq!(
            client.next_frame(),
            control_frame(&ServerPacket::ServerHello(
                ServerHello::new(protocol()).unwrap()
            ))
        );
        client.send(&login_start_frame_with_view(tag, name, view));
        spin_frames(&mut self.harness, id, 1);
        let ticket = self.harness.endpoint.loads.last.expect("login started");
        match self
            .harness
            .server
            .poll(id, &mut self.harness.endpoint, &self.harness.clock)
        {
            ConnectionProgress::Advanced { frames } => assert_eq!(frames, 1),
            other => panic!("ready load queues the success frame, got {other:?}"),
        }
        let (sent, _) = self
            .harness
            .server
            .flush_out(id, &mut self.harness.endpoint);
        assert_eq!(sent, 1, "success flushes exactly once");
        assert_eq!(
            client.next_frame(),
            control_frame(&ServerPacket::LoginSuccess(LoginSuccess::new(
                player(tag),
                WORLD_SEED as u64
            )))
        );
        self.clients.insert(id.get(), client);
        Peer {
            conn: id,
            session: self.harness.endpoint.committed_session(ticket),
        }
    }

    fn send_packet(&mut self, conn: ConnectionId, packet: &ClientPacket) {
        let bytes = frame(packet);
        self.clients
            .get_mut(&conn.get())
            .expect("the peer socket stays owned")
            .send(&bytes);
        spin_frames(&mut self.harness, conn, 1);
    }

    fn close_peer(&mut self, conn: ConnectionId) {
        self.harness
            .server
            .close(conn, CloseReason::PeerGone, &mut self.harness.endpoint);
    }
}

/// Seeds the shared world: the day environment and the standing ground chunk
/// under the spawn column.
fn seed_day_world(adapter: &mut dyn ParityAdapter) {
    adapter.stage(Box::new(|context| {
        context
            .stage(RuleEffect::Environment(day_environment(1_000)))
            .unwrap();
        context.preload_ready_chunk(
            ReadyChunk::try_new(chunk_key(0, 0), 1, 1, ground_chunk()).unwrap(),
        );
    }));
}

/// One decoded publication packet's family name, for ordered-wire
/// assertions over the frozen family order.
fn family(packet: &ServerPacket) -> &'static str {
    match packet {
        ServerPacket::ServerHello(_) => "server-hello",
        ServerPacket::HandshakeReject(_) => "handshake-reject",
        ServerPacket::LoginSuccess(_) => "login-success",
        ServerPacket::LoginReject(_) => "login-reject",
        ServerPacket::ChunkSnapshot(_) => "chunk-snapshot",
        ServerPacket::BlockChanges(_) => "block-changes",
        ServerPacket::ForgetChunks(_) => "forget-chunks",
        ServerPacket::PlayerState(_) => "player-state",
        ServerPacket::CommandRejected(_) => "command-rejected",
        ServerPacket::KeepAlive(_) => "keep-alive",
        ServerPacket::Disconnect(_) => "disconnect",
        ServerPacket::RemotePlayerSpawn(_) => "remote-spawn",
        ServerPacket::RemotePlayerDespawn(_) => "remote-despawn",
        ServerPacket::RemotePlayerStates(_) => "remote-states",
        ServerPacket::InventoryState(_) => "inventory-state",
        ServerPacket::ItemDropUpserts(_) => "drop-upserts",
        ServerPacket::ItemDropRemoves(_) => "drop-removes",
        ServerPacket::FurnaceState(_) => "furnace-state",
        ServerPacket::ContainerClosed(_) => "container-closed",
        ServerPacket::ChestState(_) => "chest-state",
        ServerPacket::ChatEvent(_) => "chat",
        ServerPacket::CompanionSpawn(_) => "companion-spawn",
        ServerPacket::CompanionStates(_) => "companion-states",
        ServerPacket::CompanionDespawn(_) => "companion-despawn",
        ServerPacket::PlaceBlockSucceeded(_) => "place-block-succeeded",
        ServerPacket::CraftingState(_) => "crafting-state",
        ServerPacket::HostileSpawn(_) => "hostile-spawn",
        ServerPacket::HostileState(_) => "hostile-state",
        ServerPacket::HostileDespawn(_) => "hostile-despawn",
        ServerPacket::CombatHit(_) => "combat-hit",
        ServerPacket::PassiveSpawn(_) => "passive-spawn",
        ServerPacket::PassiveState(_) => "passive-state",
        ServerPacket::PassiveDespawn(_) => "passive-despawn",
        ServerPacket::ProjectileSpawn(_) => "projectile-spawn",
        ServerPacket::ProjectileState(_) => "projectile-state",
        ServerPacket::ProjectileDespawn(_) => "projectile-despawn",
    }
}

/// Builds Ada's chat event with the exact published identity.
fn ada_chat(event_id: u64, body: ChatBody) -> Event {
    Event::Chat(
        ChatEvent::try_new(ChatEventParts {
            event_id,
            player_id: player(1),
            player_name: DisplayName::try_from_canonical("Ada".to_owned()).unwrap(),
            body,
        })
        .unwrap(),
    )
}

/// The chat parity scenario: two logged-in players and one staged companion
/// exchange malformed, unknown, and accepted chat in one tick, then one more
/// accepted chat on the next tick. Returns both published ticks plus the
/// frames each session drained after the first tick.
struct ChatCapture {
    ticks: Vec<TickPublication>,
    sender_frames: Vec<Vec<u8>>,
    peer_frames: Vec<Vec<u8>>,
}

fn chat_scenario(adapter: &mut dyn ParityAdapter) -> ChatCapture {
    seed_day_world(adapter);
    let ada = adapter.login(1, "Ada");
    let bea = adapter.login(2, "Bea");
    let id = companion_id(9);
    let name = derived_name(id);
    // Fixture migration: the derived companion name is explicitly registered
    // before the first tick. There is no production auto-registration.
    adapter
        .endpoint()
        .authority
        .configure_companion_chat(&[(id, CompanionName::try_from_canonical(name.clone()).unwrap())])
        .unwrap();
    adapter.stage(Box::new(move |context| {
        context
            .stage(RuleEffect::Actor(companion_actor(
                id,
                [8.5, 65.0, 8.5],
                0.0,
            )))
            .unwrap();
        context.preload_drop(drop_record(3, DROP_LIFETIME_MARGIN, [0.5, 65.5, 2.5]));
    }));
    let accepted_name = CompanionName::try_from_canonical(name).unwrap();

    let chat =
        |text: String| ClientPacket::ChatCommand(mornlea_protocol::ChatCommand::new(text).unwrap());
    adapter.send_packet(ada.conn, &chat("hello".to_owned()));
    adapter.send_packet(ada.conn, &chat("@nobody dig".to_owned()));
    adapter.send_packet(
        ada.conn,
        &chat(format!("@{} mine stone", accepted_name.as_str())),
    );
    let tick_a = adapter.tick();
    let sender_frames = adapter.drain(ada.session);
    let peer_frames = adapter.drain(bea.session);

    adapter.send_packet(ada.conn, &chat(format!("@{} dig", accepted_name.as_str())));
    let tick_b = adapter.tick();
    let _ = adapter.drain(ada.session);
    let _ = adapter.drain(bea.session);

    // The exact published bodies, addressed exactly.
    assert_eq!(
        tick_counters(&tick_a),
        (0, 0, 0),
        "chat enqueues no commands"
    );
    let sender_events = session_events(&tick_a, ada.session);
    assert_eq!(
        sender_events
            .iter()
            .filter(|event| matches!(event, Event::Chat(_)))
            .count(),
        2,
        "the sender observes both rejects; the accepted chat is broadcast"
    );
    let malformed = ada_chat(1, ChatBody::InvalidFormat);
    let unknown = ada_chat(
        2,
        ChatBody::UnknownCompanion {
            name: CompanionName::try_from_canonical("nobody".to_owned()).unwrap(),
        },
    );
    let accepted = ada_chat(
        3,
        ChatBody::Accepted {
            companion: CompanionSpeaker::new(id, accepted_name.clone()),
            command: CommandText::try_from_canonical("mine stone".to_owned()).unwrap(),
        },
    );
    assert!(
        sender_events.contains(&malformed),
        "the malformed chat rejects with the exact event"
    );
    assert!(
        sender_events.contains(&unknown),
        "the unknown companion rejects with the exact event"
    );
    let chats: Vec<&mornlea_domain::ChatEvent> = tick_a
        .events
        .iter()
        .filter_map(|event| match event.event() {
            Event::Chat(chat_event) => Some(chat_event),
            _ => None,
        })
        .collect();
    assert_eq!(chats.len(), 3);
    for pair in chats.windows(2) {
        assert!(
            pair[0].event_id() < pair[1].event_id(),
            "event ids increase"
        );
    }
    let broadcast = tick_a
        .events
        .iter()
        .find(|event| event.event() == &accepted)
        .expect("the accepted chat broadcasts");
    assert_eq!(broadcast.recipient(), EventRecipient::Broadcast);

    // The peer's drained frames carry the broadcast but neither reject.
    let reject_ids: Vec<u64> = chats.iter().map(|chat| chat.event_id()).collect();
    assert_eq!(reject_ids, vec![1, 2, 3]);
    let decoded: Vec<ServerPacket> = peer_frames.iter().map(|f| decode_play(f)).collect();
    assert!(
        decoded.iter().any(|packet| matches!(packet,
            ServerPacket::ChatEvent(event) if event.event_id == 3)),
        "the peer receives the broadcast chat"
    );
    assert!(
        decoded.iter().all(|packet| !matches!(packet,
            ServerPacket::ChatEvent(event) if event.event_id == 1 || event.event_id == 2)),
        "the peer never receives the sender-only rejects"
    );

    // The chat family sits after the mob and drop families and before the
    // record-state and private observation families.
    let chat_index = tick_a
        .events
        .iter()
        .position(|event| matches!(event.event(), Event::Chat(_)))
        .unwrap();
    let drop_index = tick_a
        .events
        .iter()
        .position(|event| matches!(event.event(), Event::ItemDropUpserts(_)))
        .expect("the staged drop publishes before chat");
    let player_index = tick_a
        .events
        .iter()
        .position(|event| matches!(event.event(), Event::PlayerState(_)))
        .unwrap();
    assert!(drop_index < chat_index && chat_index < player_index);

    // The second tick continues the id sequence with one more accepted chat.
    assert_eq!(tick_counters(&tick_b), (0, 0, 0));
    let continued = ada_chat(
        4,
        ChatBody::Accepted {
            companion: CompanionSpeaker::new(id, accepted_name),
            command: CommandText::try_from_canonical("dig".to_owned()).unwrap(),
        },
    );
    let tick_b_chats: Vec<&Event> = tick_b
        .events
        .iter()
        .map(|event| event.event())
        .filter(|event| matches!(event, Event::Chat(_)))
        .collect();
    assert_eq!(tick_b_chats, vec![&continued]);

    ChatCapture {
        ticks: vec![tick_a, tick_b],
        sender_frames,
        peer_frames,
    }
}

/// Chat addressing through both real adapters: malformed and unknown
/// addressing reject to the sender only, a valid command broadcasts to both
/// sessions, ids stay strictly increasing across ticks, chat never enqueues a
/// sequenced command, and the family order keeps chat after the drop family
/// and before the record-state families.
#[test]
fn chat_publications_match_across_adapters() {
    let mut memory = MemoryParity::new();
    let memory = chat_scenario(&mut memory);
    let mut tcp = TcpParity::new();
    let tcp = chat_scenario(&mut tcp);
    assert_eq!(
        memory.ticks, tcp.ticks,
        "chat publications are identical across the real adapters"
    );
    assert_eq!(
        memory.sender_frames, tcp.sender_frames,
        "the sender's drained frames match byte for byte"
    );
    assert_eq!(
        memory.peer_frames, tcp.peer_frames,
        "the peer's drained frames match byte for byte"
    );
}

/// The configured chat companion for the contract matrix: real identity
/// tag 9 with the name `U+963F U+6728`.
fn amu_pair() -> (CompanionId, CompanionName) {
    (
        companion_id(9),
        CompanionName::try_from_canonical("阿木".to_owned()).unwrap(),
    )
}

/// A neutral companion runtime with a provider-owned attempt and no task.
fn contract_runtime(id: CompanionId) -> ActorRuntime {
    ActorRuntime {
        key: ActorKey::Companion(id),
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 300,
        peak_y: 65.0,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Companion {
            generation: 0,
            attempt: 3,
            task: StoredCompanionTask::default(),
            mining_target: None,
        },
    }
}

/// A checked terminal-follow plan for Ada's player identity.
fn contract_follow_plan() -> AgentPlan {
    AgentPlan::try_new(
        "跟随我".to_owned(),
        vec![PlanStep::Follow {
            player_id: player(1),
        }],
    )
    .unwrap()
}

/// A checked finite plan with no terminal follow.
fn contract_finite_plan() -> AgentPlan {
    AgentPlan::try_new(
        "前进".to_owned(),
        vec![PlanStep::GoTo { x: 1, y: 65, z: 1 }],
    )
    .unwrap()
}

/// One companion action envelope naming an explicit task generation.
fn contract_envelope(
    id: CompanionId,
    generation: u64,
    tag: u8,
    action: CompanionAction,
) -> CompanionActionEnvelope {
    CompanionActionEnvelope::try_new(
        id,
        0,
        AgentRequestId::try_from_bytes(uuid(tag)).unwrap(),
        RunId::try_from_bytes(uuid(tag + 1)).unwrap(),
        SnapshotId::try_from_bytes(uuid(tag + 2)).unwrap(),
        generation,
        1,
        [0u8; 32],
        action,
    )
    .unwrap()
}

/// Every broadcast chat event in one publication, in publication order.
fn broadcast_chats(publication: &TickPublication) -> Vec<ChatEvent> {
    publication
        .events
        .iter()
        .filter(|event| event.recipient() == EventRecipient::Broadcast)
        .filter_map(|event| match event.event() {
            Event::Chat(chat) => Some(chat.clone()),
            _ => None,
        })
        .collect()
}

/// Every chat event addressed to one session, in publication order.
fn session_chats(publication: &TickPublication, session: SessionKey) -> Vec<ChatEvent> {
    session_events(publication, session)
        .into_iter()
        .filter_map(|event| match event {
            Event::Chat(chat) => Some(chat),
            _ => None,
        })
        .collect()
}

/// Configured `U+963F U+6728` full lifecycle through one real adapter: idle exact
/// stop rejects sender-only, normal admission broadcasts and queues with the
/// captured Ada issuer, the planning take is once-only, a planning stop
/// preserves, the install seam starts with the original issuer, sixteen
/// pendings fill before the seventeenth refuses, a peer exact stop bypasses
/// the full FIFO with the original issuer and command while the head
/// promotes same-tick, and the stale generation refuses. Every phase ticks
/// the real authority and drains real frames; the capture accumulates all
/// phases for cross-adapter comparison.
fn chat_contract_matrix_scenario(adapter: &mut dyn ParityAdapter) -> ChatCapture {
    let (amu_id, amu_name) = amu_pair();
    adapter
        .endpoint()
        .authority
        .configure_companion_chat(&[(amu_id, amu_name.clone())])
        .unwrap();
    seed_day_world(adapter);
    let ada = adapter.login(1, "Ada");
    let bea = adapter.login(2, "Bea");
    let speaker = CompanionSpeaker::new(amu_id, amu_name.clone());
    let chat =
        |text: String| ClientPacket::ChatCommand(mornlea_protocol::ChatCommand::new(text).unwrap());
    let mut ticks = Vec::new();
    let mut sender_frames: Vec<Vec<u8>> = Vec::new();
    let mut peer_frames: Vec<Vec<u8>> = Vec::new();
    let collect = |adapter: &mut dyn ParityAdapter,
                   ticks: &mut Vec<TickPublication>,
                   sender_frames: &mut Vec<Vec<u8>>,
                   peer_frames: &mut Vec<Vec<u8>>,
                   ada: &Peer,
                   bea: &Peer| {
        ticks.push(adapter.tick());
        sender_frames.extend(adapter.drain(ada.session));
        peer_frames.extend(adapter.drain(bea.session));
    };
    adapter.stage(Box::new(move |context| {
        context
            .stage(RuleEffect::Actor(companion_actor(
                amu_id,
                [8.5, 65.0, 8.5],
                0.0,
            )))
            .unwrap();
    }));

    // Phase 0: idle exact stop rejects sender-only with no queue effect.
    adapter.send_packet(ada.conn, &chat("@阿木 停止".to_owned()));
    collect(
        &mut *adapter,
        &mut ticks,
        &mut sender_frames,
        &mut peer_frames,
        &ada,
        &bea,
    );
    let tick = ticks.last().expect("phase tick");
    assert_eq!(tick_counters(tick), (0, 0, 0));
    let rejects = session_chats(tick, ada.session);
    assert_eq!(rejects.len(), 1);
    assert_eq!(
        rejects[0].body(),
        &ChatBody::NotFollowing {
            companion: CompanionSpeaker::new(amu_id, amu_name.clone()),
            command: CommandText::try_from_canonical("停止".to_owned()).unwrap(),
        }
    );
    assert!(session_chats(tick, bea.session).is_empty());
    assert!(
        adapter
            .endpoint()
            .authority
            .companion_chat_queue(amu_id)
            .unwrap()
            .current
            .is_none()
    );

    // Phase 1: normal admission broadcasts, queues, and captures Ada.
    adapter.send_packet(ada.conn, &chat("@阿木 mine stone".to_owned()));
    collect(
        &mut *adapter,
        &mut ticks,
        &mut sender_frames,
        &mut peer_frames,
        &ada,
        &bea,
    );
    let tick = ticks.last().expect("phase tick");
    let accepted = broadcast_chats(tick);
    assert_eq!(accepted.len(), 1);
    assert_eq!(
        accepted[0].body(),
        &ChatBody::Accepted {
            companion: speaker.clone(),
            command: CommandText::try_from_canonical("mine stone".to_owned()).unwrap(),
        }
    );
    let view = adapter
        .endpoint()
        .authority
        .companion_chat_queue(amu_id)
        .unwrap();
    let current = view.current.as_ref().expect("queued current");
    assert_eq!(current.generation, 1);
    assert_eq!(current.phase, CompanionChatPhase::Queued);
    assert_eq!(current.issuer.player_name.as_str(), "Ada");
    assert_eq!(current.issuer.player_id, player(1));
    let planned = adapter
        .endpoint()
        .authority
        .take_companion_chat_planning(amu_id)
        .unwrap()
        .expect("planning receipt");
    assert_eq!(planned.generation, 1);
    assert_eq!(planned.issuer.player_name.as_str(), "Ada");
    assert!(
        adapter
            .endpoint()
            .authority
            .take_companion_chat_planning(amu_id)
            .unwrap()
            .is_none()
    );

    // Phase 2: a planning stop preserves the queue for the other session.
    adapter.send_packet(bea.conn, &chat("@阿木 停止".to_owned()));
    collect(
        &mut *adapter,
        &mut ticks,
        &mut sender_frames,
        &mut peer_frames,
        &ada,
        &bea,
    );
    let tick = ticks.last().expect("phase tick");
    let rejects = session_chats(tick, bea.session);
    assert_eq!(rejects.len(), 1);
    assert!(matches!(rejects[0].body(), ChatBody::NotFollowing { .. }));
    assert!(session_chats(tick, ada.session).is_empty());
    let view = adapter
        .endpoint()
        .authority
        .companion_chat_queue(amu_id)
        .unwrap();
    assert_eq!(
        view.current.as_ref().expect("planning kept").phase,
        CompanionChatPhase::Planning
    );

    // Install through the real seam, then flush the started fact.
    adapter.stage(Box::new(move |context| {
        context
            .stage(RuleEffect::Runtime(contract_runtime(amu_id)))
            .unwrap();
    }));
    assert!(
        adapter
            .endpoint()
            .authority
            .install_companion_chat_plan(amu_id, 1, contract_follow_plan())
            .unwrap()
    );
    collect(
        &mut *adapter,
        &mut ticks,
        &mut sender_frames,
        &mut peer_frames,
        &ada,
        &bea,
    );
    let tick = ticks.last().expect("phase tick");
    let started = broadcast_chats(tick)
        .into_iter()
        .find(|event| {
            matches!(
                event.body(),
                ChatBody::Task {
                    state: TaskState::Started,
                    ..
                }
            )
        })
        .expect("started broadcast");
    assert_eq!(
        started.body(),
        &ChatBody::Task {
            companion: speaker.clone(),
            command: CommandText::try_from_canonical("mine stone".to_owned()).unwrap(),
            state: TaskState::Started,
        }
    );
    assert_eq!(started.player_name().as_str(), "Ada");

    // Phase 4: sixteen pendings fill beside the running task.
    for n in 0..16 {
        adapter.send_packet(ada.conn, &chat(format!("@阿木 task-{}", n)));
    }
    collect(
        &mut *adapter,
        &mut ticks,
        &mut sender_frames,
        &mut peer_frames,
        &ada,
        &bea,
    );
    let tick = ticks.last().expect("phase tick");
    assert_eq!(broadcast_chats(tick).len(), 16);
    let view = adapter
        .endpoint()
        .authority
        .companion_chat_queue(amu_id)
        .unwrap();
    assert_eq!(view.pending.len(), 16);

    // Phase 5: the seventeenth refuses while Bea's exact stop bypasses the
    // full FIFO with the original Ada issuer and command.
    adapter.send_packet(ada.conn, &chat("@阿木 task-overflow".to_owned()));
    adapter.send_packet(bea.conn, &chat("@阿木 停止".to_owned()));
    collect(
        &mut *adapter,
        &mut ticks,
        &mut sender_frames,
        &mut peer_frames,
        &ada,
        &bea,
    );
    let tick = ticks.last().expect("phase tick");
    let rejects = session_chats(tick, ada.session);
    assert_eq!(rejects.len(), 1);
    assert!(matches!(rejects[0].body(), ChatBody::QueueFull { .. }));
    let stopped = broadcast_chats(tick)
        .into_iter()
        .find(|event| {
            matches!(
                event.body(),
                ChatBody::Task {
                    state: TaskState::Stopped,
                    ..
                }
            )
        })
        .expect("stopped broadcast");
    assert_eq!(
        stopped.body(),
        &ChatBody::Task {
            companion: speaker.clone(),
            command: CommandText::try_from_canonical("mine stone".to_owned()).unwrap(),
            state: TaskState::Stopped,
        }
    );
    assert_eq!(stopped.player_name().as_str(), "Ada");
    let view = adapter
        .endpoint()
        .authority
        .companion_chat_queue(amu_id)
        .unwrap();
    let current = view.current.as_ref().expect("promoted head");
    assert_eq!(current.generation, 2);
    assert_eq!(current.command.as_str(), "task-0");
    assert_eq!(view.pending.len(), 15);

    // Phase 6: the delayed stale generation refuses at the fence.
    assert_eq!(
        adapter
            .endpoint()
            .authority
            .submit_companion(contract_envelope(
                amu_id,
                1,
                50,
                CompanionAction::MineRelease,
            )),
        Err(mornlea_server::contracts::ServerError::InvalidInput {
            field: "companion_generation",
        })
    );

    ChatCapture {
        ticks,
        sender_frames,
        peer_frames,
    }
}

/// Ordinary `U+505C U+6B62 U+79FB U+52A8`/`stop` phrases admit while an exact stop against a
/// running finite plan rejects sender-only with the queue preserved,
/// proven through both real adapters.
fn chat_contract_phrases_finite_scenario(adapter: &mut dyn ParityAdapter) -> ChatCapture {
    let (amu_id, amu_name) = amu_pair();
    adapter
        .endpoint()
        .authority
        .configure_companion_chat(&[(amu_id, amu_name.clone())])
        .unwrap();
    seed_day_world(adapter);
    let ada = adapter.login(1, "Ada");
    let bea = adapter.login(2, "Bea");
    let speaker = CompanionSpeaker::new(amu_id, amu_name.clone());
    let chat =
        |text: String| ClientPacket::ChatCommand(mornlea_protocol::ChatCommand::new(text).unwrap());
    let mut ticks = Vec::new();
    let mut sender_frames: Vec<Vec<u8>> = Vec::new();
    let mut peer_frames: Vec<Vec<u8>> = Vec::new();
    adapter.stage(Box::new(move |context| {
        context
            .stage(RuleEffect::Actor(companion_actor(
                amu_id,
                [8.5, 65.0, 8.5],
                0.0,
            )))
            .unwrap();
        context
            .stage(RuleEffect::Runtime(contract_runtime(amu_id)))
            .unwrap();
    }));

    // Ordinary phrases admit as normal commands.
    adapter.send_packet(ada.conn, &chat("@阿木 停止移动".to_owned()));
    adapter.send_packet(ada.conn, &chat("@阿木 stop".to_owned()));
    ticks.push(adapter.tick());
    sender_frames.extend(adapter.drain(ada.session));
    peer_frames.extend(adapter.drain(bea.session));
    let tick = ticks.last().expect("phase tick");
    let accepted = broadcast_chats(tick);
    assert_eq!(accepted.len(), 2);
    assert_eq!(
        accepted[0].body(),
        &ChatBody::Accepted {
            companion: speaker.clone(),
            command: CommandText::try_from_canonical("停止移动".to_owned()).unwrap(),
        }
    );
    assert_eq!(
        accepted[1].body(),
        &ChatBody::Accepted {
            companion: speaker.clone(),
            command: CommandText::try_from_canonical("stop".to_owned()).unwrap(),
        }
    );

    // A running finite plan is not following: the exact stop rejects
    // sender-only with the task and FIFO preserved.
    adapter
        .endpoint()
        .authority
        .take_companion_chat_planning(amu_id)
        .unwrap()
        .expect("planning receipt");
    assert!(
        adapter
            .endpoint()
            .authority
            .install_companion_chat_plan(amu_id, 1, contract_finite_plan())
            .unwrap()
    );
    ticks.push(adapter.tick());
    sender_frames.extend(adapter.drain(ada.session));
    peer_frames.extend(adapter.drain(bea.session));
    adapter.send_packet(ada.conn, &chat("@阿木 停止".to_owned()));
    ticks.push(adapter.tick());
    sender_frames.extend(adapter.drain(ada.session));
    peer_frames.extend(adapter.drain(bea.session));
    let tick = ticks.last().expect("phase tick");
    let rejects = session_chats(tick, ada.session);
    assert_eq!(rejects.len(), 1);
    assert_eq!(
        rejects[0].body(),
        &ChatBody::NotFollowing {
            companion: speaker.clone(),
            command: CommandText::try_from_canonical("停止".to_owned()).unwrap(),
        }
    );
    assert!(session_chats(tick, bea.session).is_empty());
    let view = adapter
        .endpoint()
        .authority
        .companion_chat_queue(amu_id)
        .unwrap();
    let current = view.current.as_ref().expect("running kept");
    assert_eq!(current.phase, CompanionChatPhase::Running);
    assert_eq!(current.generation, 1);

    ChatCapture {
        ticks,
        sender_frames,
        peer_frames,
    }
}

/// The configured `U+963F U+6728` contract matrix is identical across the real
/// adapters: whole ordered transcripts plus exact drained frame bytes.
#[test]
fn chat_contract_matrix_across_adapters() {
    let mut memory = MemoryParity::new();
    let memory_capture = chat_contract_matrix_scenario(&mut memory);
    let mut tcp = TcpParity::new();
    let tcp_capture = chat_contract_matrix_scenario(&mut tcp);
    assert_eq!(
        memory_capture.ticks, tcp_capture.ticks,
        "matrix publications are identical across the real adapters"
    );
    assert_eq!(
        memory_capture.sender_frames, tcp_capture.sender_frames,
        "matrix sender frames match byte for byte"
    );
    assert_eq!(
        memory_capture.peer_frames, tcp_capture.peer_frames,
        "matrix peer frames match byte for byte"
    );
}

/// Ordinary phrases plus the finite-plan stop are identical across the real
/// adapters: whole ordered transcripts plus exact drained frame bytes.
#[test]
fn chat_contract_phrases_finite_across_adapters() {
    let mut memory = MemoryParity::new();
    let memory_capture = chat_contract_phrases_finite_scenario(&mut memory);
    let mut tcp = TcpParity::new();
    let tcp_capture = chat_contract_phrases_finite_scenario(&mut tcp);
    assert_eq!(
        memory_capture.ticks, tcp_capture.ticks,
        "phrase publications are identical across the real adapters"
    );
    assert_eq!(
        memory_capture.sender_frames, tcp_capture.sender_frames,
        "phrase sender frames match byte for byte"
    );
    assert_eq!(
        memory_capture.peer_frames, tcp_capture.peer_frames,
        "phrase peer frames match byte for byte"
    );
}

/// The remote-player lifecycle scenario: Ada ticks alone, Bea logs in nearby,
/// both exchange the exact spawn then state batches, and Bea's close despawns
/// her on Ada's next tick while the retired Bea receives nothing.
fn remote_lifecycle_scenario(adapter: &mut dyn ParityAdapter) -> Vec<TickPublication> {
    seed_day_world(adapter);
    adapter.stage_login(
        1,
        stored_player(1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    let ada = adapter.login(1, "Ada");

    // Ada alone publishes no remote family at all.
    let solo = adapter.tick();
    assert!(
        !solo.events.iter().any(|event| matches!(
            event.event(),
            Event::RemotePlayerSpawn(_)
                | Event::RemotePlayerStates(_)
                | Event::RemotePlayerDespawn(_)
        )),
        "a lone session exchanges no remote events"
    );
    let _ = adapter.drain(ada.session);

    // Bea logs in nearby: the spawn tick carries the exact spawn record and
    // no state batch.
    adapter.stage_login(
        2,
        stored_player(2, "Bea", [4.5, 65.0, 4.5], 0.75, -0.25, |_| {}),
    );
    let bea = adapter.login(2, "Bea");
    let spawn_tick = adapter.tick();
    let expected_spawn = RemotePlayerSpawn::new(RemotePlayerSpawnParts {
        player_id: player(2),
        display_name: DisplayName::try_from_canonical("Bea".to_owned()).unwrap(),
        server_tick: 1,
        dimension: Dimension::OVERWORLD,
        position: FiniteVec3::try_new([4.5, 65.0, 4.5]).unwrap(),
        look: look(0.75, -0.25),
    });
    let ada_events = session_events(&spawn_tick, ada.session);
    assert_eq!(
        ada_events
            .iter()
            .filter(|event| matches!(event, Event::RemotePlayerSpawn(_)))
            .count(),
        1
    );
    assert!(
        ada_events.iter().any(
            |event| matches!(event, Event::RemotePlayerSpawn(spawn) if *spawn == expected_spawn)
        ),
        "Ada sees Bea's exact spawn record"
    );
    assert!(
        !ada_events
            .iter()
            .any(|event| matches!(event, Event::RemotePlayerStates(_))),
        "the spawn tick publishes no remote state batch"
    );
    let bea_events = session_events(&spawn_tick, bea.session);
    assert!(
        bea_events
            .iter()
            .any(|event| matches!(event, Event::RemotePlayerSpawn(spawn)
                if spawn.player_id() == player(1) && spawn.server_tick() == 1)),
        "Bea sees Ada spawn on the same tick"
    );
    for peer in [&ada, &bea] {
        let _ = adapter.drain(peer.session);
    }

    // The next tick carries one exact persisting state batch and no respawn.
    let state_tick = adapter.tick();
    let expected_states = RemotePlayerStates::try_new(mornlea_domain::RemotePlayerStatesParts {
        server_tick: 2,
        states: vec![RemotePlayerState::new(RemotePlayerStateParts {
            player_id: player(2),
            dimension: Dimension::OVERWORLD,
            position: FiniteVec3::try_new([4.5, 65.0, 4.5]).unwrap(),
            look: look(0.75, -0.25),
            reset: false,
        })]
        .into_boxed_slice(),
    })
    .unwrap();
    let ada_events = session_events(&state_tick, ada.session);
    assert_eq!(
        ada_events
            .iter()
            .filter(|event| matches!(event, Event::RemotePlayerStates(_)))
            .count(),
        1
    );
    assert!(
        ada_events.iter().any(
            |event| matches!(event, Event::RemotePlayerStates(batch) if *batch == expected_states)
        ),
        "the persisting batch carries Bea's exact static pose"
    );
    for peer in [&ada, &bea] {
        let _ = adapter.drain(peer.session);
    }

    // Bea drops: the next tick despawns her on Ada's side, ordered before the
    // same tick's new snapshot and the private observation, and the retired
    // session receives nothing at all.
    adapter.close_peer(bea.conn);
    adapter.stage(Box::new(|context| {
        context.preload_ready_chunk(
            ReadyChunk::try_new(chunk_key(1, 0), 1, 1, ground_chunk()).unwrap(),
        );
    }));
    let despawn_tick = adapter.tick();
    let ada_events = session_events(&despawn_tick, ada.session);
    let despawn = ada_events
        .iter()
        .position(|event| matches!(event, Event::RemotePlayerDespawn(despawn) if despawn.player_id() == player(2)))
        .expect("Ada learns Bea left");
    let snapshot = ada_events
        .iter()
        .position(|event| matches!(event, Event::ChunkSnapshot(_)))
        .expect("the same tick carries a new snapshot");
    assert!(
        despawn < snapshot,
        "the despawn precedes the same tick's snapshot"
    );
    assert!(
        ada_events
            .iter()
            .any(|event| matches!(event, Event::PlayerState(_))),
        "the private observation closes the tick"
    );
    assert!(
        !ada_events
            .iter()
            .any(|event| matches!(event, Event::RemotePlayerStates(_))),
        "the despawn tick publishes no remote state batch"
    );
    assert!(
        !despawn_tick
            .events
            .iter()
            .any(|event| event.recipient() == EventRecipient::Session(bea.session.get())),
        "the retired session receives nothing"
    );
    let _ = adapter.drain(ada.session);

    // One tick later the departure never repeats: the despawn family stays
    // silent for the departed remote on every adapter.
    let after_tick = adapter.tick();
    let _ = adapter.drain(ada.session);
    assert!(
        !session_events(&after_tick, ada.session)
            .iter()
            .any(|event| matches!(
                event,
                Event::RemotePlayerDespawn(despawn) if despawn.player_id() == player(2)
            )),
        "the departed remote despawns exactly once"
    );

    vec![solo, spawn_tick, state_tick, despawn_tick, after_tick]
}

/// The remote-player spawn, state, and despawn publications through both real
/// adapters, with the exact spawn records, the despawn-before-snapshot family
/// order, and identical publications on every tick.
#[test]
fn remote_player_lifecycle_publications_match_across_adapters() {
    let mut memory = MemoryParity::new();
    let memory = remote_lifecycle_scenario(&mut memory);
    let mut tcp = TcpParity::new();
    let tcp = remote_lifecycle_scenario(&mut tcp);
    assert_eq!(
        memory, tcp,
        "the remote lifecycle publishes identically across the real adapters"
    );
}

/// The companion lifecycle scenario: one staged companion near Ada publishes
/// its derived-name spawn, a static state batch, and a despawn once Ada's
/// real eastbound walk carries her interest square past the companion's
/// column.
fn companion_lifecycle_scenario(adapter: &mut dyn ParityAdapter) -> Vec<TickPublication> {
    seed_day_world(adapter);
    adapter.stage_login(
        1,
        stored_player(1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    let ada = adapter.login(1, "Ada");
    let id = companion_id(9);
    let name = derived_name(id);
    adapter.stage(Box::new(move |context| {
        context
            .stage(RuleEffect::Actor(companion_actor(
                id,
                [8.5, 65.0, 8.5],
                1.25,
            )))
            .unwrap();
    }));

    let spawn_tick = adapter.tick();
    let events = session_events(&spawn_tick, ada.session);
    let expected = CompanionSpawn::try_new(CompanionSpawnParts {
        id,
        name: CompanionName::try_from_canonical(name.clone()).unwrap(),
        server_tick: 0,
        dimension: Dimension::OVERWORLD,
        position: FiniteVec3::try_new([8.5, 65.0, 8.5]).unwrap(),
        look: look(1.25, 0.0),
    })
    .unwrap();
    assert_eq!(name, "companion-0900000000004000");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::CompanionSpawn(_)))
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::CompanionSpawn(spawn) if *spawn == expected)),
        "the spawn carries the exact derived name {name}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::CompanionStates(_)))
    );
    let _ = adapter.drain(ada.session);

    let state_tick = adapter.tick();
    let events = session_events(&state_tick, ada.session);
    let expected_states = CompanionStates::try_new(mornlea_domain::CompanionStatesParts {
        server_tick: 1,
        states: vec![
            CompanionState::try_new(CompanionStateParts {
                id,
                dimension: Dimension::OVERWORLD,
                position: FiniteVec3::try_new([8.5, 65.0, 8.5]).unwrap(),
                look: look(1.25, 0.0),
                reset: false,
            })
            .unwrap(),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    assert!(
        events.iter().any(
            |event| matches!(event, Event::CompanionStates(batch) if *batch == expected_states)
        )
    );
    let _ = adapter.drain(ada.session);

    // Walk east through staged ground so the companion's column leaves the
    // interest square; the transition tick publishes the despawn.
    adapter.stage(Box::new(|context| {
        for x in 1..=4 {
            context.preload_ready_chunk(
                ReadyChunk::try_new(chunk_key(x, 0), 1, 1, ground_chunk()).unwrap(),
            );
        }
    }));
    let walk = walk_east(adapter, std::slice::from_ref(&ada), &ada, 1, 3, 700);
    let transition = walk.last().unwrap();
    let events = session_events(transition, ada.session);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::CompanionDespawn(despawn) if *despawn == CompanionDespawn::new(id)))
            .count(),
        1,
        "the interest exit despawns the companion exactly once"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::CompanionStates(_))),
        "the despawn tick carries no companion state batch"
    );

    // One tick later the departure never repeats: the interest exit despawns
    // the companion exactly once on every adapter.
    let after_tick = adapter.tick();
    let _ = adapter.drain(ada.session);
    assert!(
        !session_events(&after_tick, ada.session)
            .iter()
            .any(|event| matches!(event, Event::CompanionDespawn(despawn) if *despawn == CompanionDespawn::new(id))),
        "the departed companion despawns exactly once"
    );

    let mut ticks = vec![spawn_tick, state_tick];
    ticks.extend(walk);
    ticks.push(after_tick);
    ticks
}

/// The companion spawn, state, and interest-exit despawn publications through
/// both real adapters, with the exact derived name and identical
/// publications on every tick of the walk.
#[test]
fn companion_lifecycle_publications_match_across_adapters() {
    let mut memory = MemoryParity::new();
    let memory = companion_lifecycle_scenario(&mut memory);
    let mut tcp = TcpParity::new();
    let tcp = companion_lifecycle_scenario(&mut tcp);
    assert_eq!(
        memory, tcp,
        "the companion lifecycle publishes identically across the real adapters"
    );
}

/// One real placement command through the adapter with Ada's preloaded dirt.
fn place_dirt(adapter: &mut dyn ParityAdapter, peer: &Peer, sequence: u64) {
    adapter.send_packet(
        peer.conn,
        &ClientPacket::PlaceBlock(
            mornlea_protocol::PlaceBlock::new(
                sequence,
                std::f32::consts::PI,
                -std::f32::consts::FRAC_PI_4,
                0,
            )
            .unwrap(),
        ),
    );
}

/// The chunk publication scenario: the first Ready contact publishes the
/// exact snapshot, real placements publish exact contiguous deltas, a late
/// subscriber two revisions behind gets a snapshot instead of a delta, and a
/// real eastbound walk forgets the exact sorted exited columns.
fn chunk_publication_scenario(adapter: &mut dyn ParityAdapter) -> Vec<TickPublication> {
    seed_day_world(adapter);
    adapter.stage_login(
        1,
        stored_player(1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |player| {
            player.inventory.hotbar.slots[0] = storage(ITEM_DIRT, 3);
        }),
    );
    let ada = adapter.login(1, "Ada");

    // First contact: the exact staged column.
    let tick_a = adapter.tick();
    let events = session_events(&tick_a, ada.session);
    let snapshot = events
        .iter()
        .find_map(|event| match event {
            Event::ChunkSnapshot(snapshot) => Some(snapshot.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(snapshot.dimension(), Dimension::OVERWORLD);
    assert_eq!(snapshot.chunk(), ChunkPos::new(0, 0));
    assert_eq!(snapshot.revision(), 1);
    for (x, z) in [(0, 0), (7, 3), (15, 15)] {
        let index = mornlea_domain::chunk_block_index(BlockPos::new(x, 64, z)) as usize;
        assert_eq!(
            snapshot.sections()[8].block_at(index % 4096),
            Some(GRASS_BLOCK)
        );
    }
    assert_eq!(snapshot.sections()[0].block_at(0), Some(0));
    let _ = adapter.drain(ada.session);

    // One real placement becomes one exact contiguous delta.
    place_dirt(adapter, &ada, 1);
    let tick_b = adapter.tick();
    let events = session_events(&tick_b, ada.session);
    let expected = mornlea_domain::BlockChanges::try_new(mornlea_domain::BlockChangesParts {
        dimension: Dimension::OVERWORLD,
        chunk: ChunkPos::new(0, 0),
        base_revision: 1,
        new_revision: 2,
        changes: vec![BlockChange::try_new(BlockPos::new(0, 65, 2), DIRT_BLOCK).unwrap()]
            .into_boxed_slice(),
    })
    .unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::BlockChanges(batch) if *batch == expected)),
        "the first placement publishes the exact delta"
    );
    let _ = adapter.drain(ada.session);

    // A second placement, then Ben logs in two revisions late: his first
    // contact is a snapshot, not a delta.
    place_dirt(adapter, &ada, 2);
    adapter.stage_login(
        2,
        stored_player(2, "Ben", [1.5, 65.0, 1.5], 0.0, 0.0, |_| {}),
    );
    let ben = adapter.login(2, "Ben");
    let tick_c = adapter.tick();
    let ben_events = session_events(&tick_c, ben.session);
    let late = ben_events
        .iter()
        .find_map(|event| match event {
            Event::ChunkSnapshot(snapshot) => Some(snapshot.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(late.chunk(), ChunkPos::new(0, 0));
    assert_eq!(late.revision(), 3, "the late subscriber sees a snapshot");
    assert!(
        !ben_events
            .iter()
            .any(|event| matches!(event, Event::BlockChanges(_)))
    );
    let ada_events = session_events(&tick_c, ada.session);
    let expected = mornlea_domain::BlockChanges::try_new(mornlea_domain::BlockChangesParts {
        dimension: Dimension::OVERWORLD,
        chunk: ChunkPos::new(0, 0),
        base_revision: 2,
        new_revision: 3,
        changes: vec![BlockChange::try_new(BlockPos::new(0, 65, 1), DIRT_BLOCK).unwrap()]
            .into_boxed_slice(),
    })
    .unwrap();
    assert!(
        ada_events
            .iter()
            .any(|event| matches!(event, Event::BlockChanges(batch) if *batch == expected)),
        "the contiguous subscriber keeps receiving deltas"
    );
    for peer in [&ada, &ben] {
        let _ = adapter.drain(peer.session);
    }

    // Both observers see one more contiguous delta after the late snapshot.
    place_dirt(adapter, &ada, 3);
    let tick_d = adapter.tick();
    for peer in [&ada, &ben] {
        let events = session_events(&tick_d, peer.session);
        let delta = events
            .iter()
            .find_map(|event| match event {
                Event::BlockChanges(batch) => Some(batch.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(delta.base_revision(), 3);
        assert_eq!(delta.new_revision(), 4);
        assert_eq!(
            delta.changes(),
            &[BlockChange::try_new(BlockPos::new(0, 66, 1), DIRT_BLOCK).unwrap()]
        );
        let _ = adapter.drain(peer.session);
    }

    // A real eastbound walk: the first interest transition forgets exactly
    // the exited x -2 column, addressed to the walker alone.
    adapter.stage(Box::new(|context| {
        for x in 1..=2 {
            context.preload_ready_chunk(
                ReadyChunk::try_new(chunk_key(x, 0), 1, 1, ground_chunk()).unwrap(),
            );
        }
    }));
    let walk = walk_east(adapter, &[ada.clone(), ben.clone()], &ada, 4, 1, 700);
    let transition = walk.last().expect("the walk published at least one tick");
    let ada_events = session_events(transition, ada.session);
    let mut exited = Vec::new();
    for z in -2..=2 {
        exited.push(ChunkPos::new(-2, z));
    }
    let expected = ForgetChunks::try_new(mornlea_domain::ForgetChunksParts {
        dimension: Dimension::OVERWORLD,
        chunks: exited.into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        ada_events
            .iter()
            .filter(|event| matches!(event, Event::ForgetChunks(batch) if *batch == expected))
            .count(),
        1,
        "the first interest exit forgets the exact sorted column list"
    );
    assert!(
        !session_events(transition, ben.session)
            .iter()
            .any(|event| matches!(event, Event::ForgetChunks(_))),
        "the forget batch belongs to the walker alone"
    );

    let mut ticks = vec![tick_a, tick_b, tick_c, tick_d];
    ticks.extend(walk);
    ticks
}

/// Chunk interest through both real adapters: the exact first snapshot, exact
/// contiguous deltas from real placements, a snapshot for a late subscriber,
/// and the exact forget batch from a real walk, with identical publications
/// on every tick.
#[test]
fn chunk_snapshot_block_changes_forget_match_across_adapters() {
    let mut memory = MemoryParity::new();
    let memory = chunk_publication_scenario(&mut memory);
    let mut tcp = TcpParity::new();
    let tcp = chunk_publication_scenario(&mut tcp);
    assert_eq!(
        memory, tcp,
        "chunk publications are identical across the real adapters"
    );
}

/// The provider resync scenario: a wanted Ready column re-sends its full
/// current snapshot regardless of `HaveRevision`, before the ordinary
/// first-send snapshots of the same tick, while out-of-interest and unready
/// requests publish nothing yet still consume their sequences.
fn resync_scenario(adapter: &mut dyn ParityAdapter) -> Vec<TickPublication> {
    seed_day_world(adapter);
    adapter.stage_login(
        1,
        stored_player(1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    let ada = adapter.login(1, "Ada");
    let first = adapter.tick();
    assert_eq!(
        session_events(&first, ada.session)
            .iter()
            .filter(|event| matches!(event, Event::ChunkSnapshot(_)))
            .count(),
        1,
        "the first contact snapshots the seeded column"
    );
    let _ = adapter.drain(ada.session);

    // One newly ready column beside the resync target, so the same tick
    // carries one resync answer and one first send.
    adapter.stage(Box::new(|context| {
        context.preload_ready_chunk(
            ReadyChunk::try_new(chunk_key(1, 0), 1, 1, ground_chunk()).unwrap(),
        );
    }));
    let resync = |adapter: &mut dyn ParityAdapter,
                  conn: ConnectionId,
                  sequence: u64,
                  x: i32,
                  z: i32,
                  have: u64| {
        adapter.send_packet(
            conn,
            &ClientPacket::RequestChunkResync(RequestChunkResync::new(
                sequence,
                Dimension::OVERWORLD,
                x,
                z,
                have,
            )),
        );
    };
    resync(adapter, ada.conn, 1, 0, 0, 999);
    resync(adapter, ada.conn, 2, 9, 9, 1);
    resync(adapter, ada.conn, 3, 2, 0, 1);
    let tick_b = adapter.tick();
    assert_eq!(
        tick_counters(&tick_b),
        (3, 0, 0),
        "every resync consumes its command slot"
    );
    assert_eq!(
        adapter
            .endpoint()
            .authority
            .session(ada.session)
            .unwrap()
            .last_applied_sequence,
        3,
        "every resync advances the watermark"
    );
    let events = session_events(&tick_b, ada.session);
    let snapshots: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::ChunkSnapshot(snapshot) => Some(snapshot.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(snapshots.len(), 2, "only the wanted ready columns answer");
    assert_eq!(snapshots[0].chunk(), ChunkPos::new(0, 0));
    assert_eq!(snapshots[0].revision(), 1);
    assert_eq!(snapshots[1].chunk(), ChunkPos::new(1, 0));
    let _ = adapter.drain(ada.session);

    // A second resync with a different HaveRevision replies the identical
    // payload; `HaveRevision` is ignored.
    resync(adapter, ada.conn, 4, 0, 0, 1);
    let tick_c = adapter.tick();
    assert_eq!(
        tick_counters(&tick_c),
        (1, 0, 0),
        "the silently dropped requests still consume their command slots"
    );
    assert_eq!(
        adapter
            .endpoint()
            .authority
            .session(ada.session)
            .unwrap()
            .last_applied_sequence,
        4
    );
    let events = session_events(&tick_c, ada.session);
    let snapshots: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::ChunkSnapshot(snapshot) => Some(snapshot.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].chunk(), ChunkPos::new(0, 0));
    assert_eq!(snapshots[0].revision(), 1);
    let first_answer = session_events(&tick_b, ada.session)
        .into_iter()
        .find_map(|event| match event {
            Event::ChunkSnapshot(snapshot) if snapshot.chunk() == ChunkPos::new(0, 0) => {
                Some(snapshot)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        snapshots[0], first_answer,
        "the resync payload ignores HaveRevision"
    );
    let _ = adapter.drain(ada.session);

    vec![first, tick_b, tick_c]
}

/// The provider resync lane through both real adapters: same-tick full
/// snapshots at the current revision, `HaveRevision` ignored, silent drops
/// for out-of-interest and unready columns, and advancing watermarks.
#[test]
fn request_chunk_resync_provider_matches_across_adapters() {
    let mut memory = MemoryParity::new();
    let memory = resync_scenario(&mut memory);
    let mut tcp = TcpParity::new();
    let tcp = resync_scenario(&mut tcp);
    assert_eq!(
        memory, tcp,
        "the resync provider publishes identically across the real adapters"
    );
}

/// The hostile lifecycle scenario under a staged night environment: one
/// staged nightwalker publishes its exact spawn, a later motion tick its
/// exact persisting state, and a staged lethal outcome publishes only the
/// despawn. The wandering body derives from the post-tick committed record,
/// asserted identically through both adapters.
fn hostile_lifecycle_scenario(adapter: &mut dyn ParityAdapter) -> Vec<TickPublication> {
    adapter.stage(Box::new(|context| {
        context
            .stage(RuleEffect::Environment(day_environment(14_000)))
            .unwrap();
        context.preload_ready_chunk(
            ReadyChunk::try_new(chunk_key(0, 0), 1, 1, ground_chunk()).unwrap(),
        );
    }));
    adapter.stage_login(
        1,
        stored_player(1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    let ada = adapter.login(1, "Ada");
    let id = HostileId::try_new(31).unwrap();
    adapter.stage(Box::new(move |context| {
        context
            .stage(RuleEffect::Actor(hostile_actor(31, [8.5, 65.0, 8.5], 0)))
            .unwrap();
    }));

    let spawn_tick = adapter.tick();
    let events = session_events(&spawn_tick, ada.session);
    let expected = HostileSpawn::try_new(HostileSpawnParts {
        server_tick: 0,
        spawns: vec![
            HostileSpawnRecord::try_new(HostileSpawnRecordParts {
                id,
                dimension: Dimension::OVERWORLD,
                position: FiniteVec3::try_new([8.5, 65.0, 8.5]).unwrap(),
                yaw: 0.0,
                health: 20,
                kind: HostileKind::Nightwalker,
            })
            .unwrap(),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::HostileSpawn(batch) if *batch == expected)),
        "the staged nightwalker publishes its exact spawn record"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::HostileState(_)))
    );
    let _ = adapter.drain(ada.session);

    // One motion tick: the persisting batch carries the committed body.
    let state_tick = adapter.tick();
    let committed = {
        let residents = adapter.endpoint().authority.residents();
        residents
            .actors
            .iter()
            .find(|actor| actor.key == ActorKey::Hostile(id))
            .cloned()
            .unwrap()
    };
    let expected = HostileState::try_new(HostileStateParts {
        server_tick: 1,
        states: vec![
            HostileStateRecord::try_new(HostileStateRecordParts {
                id,
                position: committed.motion.position(),
                velocity: committed.motion.velocity(),
                yaw: committed.look.yaw(),
                health: committed.survival.health(),
                kind: HostileKind::Nightwalker,
            })
            .unwrap(),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    let events = session_events(&state_tick, ada.session);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::HostileState(_)))
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::HostileState(batch) if *batch == expected)),
        "the persisting batch carries the exact committed body"
    );
    let _ = adapter.drain(ada.session);

    // The staged lethal outcome: only the despawn remains.
    adapter.stage(Box::new(move |context| {
        let mut actor = context
            .read()
            .actor(ActorKey::Hostile(id))
            .cloned()
            .unwrap();
        actor.lifecycle = ActorLifecycle::Dead;
        context.stage(RuleEffect::Actor(actor)).unwrap();
    }));
    let despawn_tick = adapter.tick();
    let events = session_events(&despawn_tick, ada.session);
    let expected = mornlea_domain::HostileDespawn::try_new(mornlea_domain::HostileDespawnParts {
        server_tick: 2,
        ids: vec![id].into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::HostileDespawn(_)))
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::HostileDespawn(batch) if *batch == expected)),
        "the death publishes the exact despawn batch"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::HostileState(_))),
        "the dead hostile is absent from the state family"
    );
    let _ = adapter.drain(ada.session);

    // One tick later the departure never repeats on either adapter.
    let after_tick = adapter.tick();
    let _ = adapter.drain(ada.session);
    assert!(
        !session_events(&after_tick, ada.session)
            .iter()
            .any(|event| matches!(
                event,
                Event::HostileDespawn(batch) if batch.ids().contains(&id)
            )),
        "the dead hostile despawns exactly once"
    );

    vec![spawn_tick, state_tick, despawn_tick, after_tick]
}

/// The hostile spawn, state, and death-despawn publications through both real
/// adapters under the night environment, with identical publications on every
/// tick.
#[test]
fn hostile_lifecycle_publications_match_across_adapters() {
    let mut memory = MemoryParity::new();
    let memory = hostile_lifecycle_scenario(&mut memory);
    let mut tcp = TcpParity::new();
    let tcp = hostile_lifecycle_scenario(&mut tcp);
    assert_eq!(
        memory, tcp,
        "the hostile lifecycle publishes identically across the real adapters"
    );
}

/// The passive lifecycle scenario: one staged cow publishes its spawn, its
/// persisting state, and the died despawn once its death settles. Settled body
/// values derive from the post-tick committed record, asserted identically
/// through both adapters.
fn passive_lifecycle_scenario(adapter: &mut dyn ParityAdapter) -> Vec<TickPublication> {
    seed_day_world(adapter);
    adapter.stage_login(
        1,
        stored_player(1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    let ada = adapter.login(1, "Ada");
    let id = PassiveId::try_new(7).unwrap();
    adapter.stage(Box::new(move |context| {
        context
            .stage(RuleEffect::Actor(passive_actor(7, [8.5, 65.0, 8.5])))
            .unwrap();
        context
            .stage(RuleEffect::Runtime(passive_runtime(id)))
            .unwrap();
    }));

    let spawn_tick = adapter.tick();
    let committed = {
        let residents = adapter.endpoint().authority.residents();
        residents
            .actors
            .iter()
            .find(|actor| actor.key == ActorKey::Passive(id))
            .cloned()
            .unwrap()
    };
    let events = session_events(&spawn_tick, ada.session);
    let expected = PassiveSpawn::try_new(PassiveSpawnParts {
        server_tick: 0,
        spawns: vec![
            PassiveSpawnRecord::try_new(PassiveSpawnRecordParts {
                id,
                dimension: Dimension::OVERWORLD,
                position: committed.motion.position(),
                yaw: committed.look.yaw(),
                health: committed.survival.health(),
            })
            .unwrap(),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::PassiveSpawn(batch) if *batch == expected)),
        "the staged cow publishes its exact spawn record"
    );
    // The drained spawn-tick bytes carry the spawn packet and no despawn.
    let frames = adapter.drain(ada.session);
    let decoded: Vec<ServerPacket> = frames.iter().map(|frame| decode_play(frame)).collect();
    assert!(
        decoded
            .iter()
            .any(|packet| matches!(packet, ServerPacket::PassiveSpawn(_))),
        "the drained spawn frames carry the spawn packet"
    );
    assert!(
        !decoded
            .iter()
            .any(|packet| matches!(packet, ServerPacket::PassiveDespawn(_))),
        "no despawn leaves before the removal"
    );

    let state_tick = adapter.tick();
    let committed = {
        let residents = adapter.endpoint().authority.residents();
        residents
            .actors
            .iter()
            .find(|actor| actor.key == ActorKey::Passive(id))
            .cloned()
            .unwrap()
    };
    let expected = PassiveState::try_new(PassiveStateParts {
        server_tick: 1,
        states: vec![
            PassiveStateRecord::try_new(PassiveStateRecordParts {
                id,
                position: committed.motion.position(),
                velocity: committed.motion.velocity(),
                yaw: committed.look.yaw(),
                health: committed.survival.health(),
                grazing: false,
            })
            .unwrap(),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    let events = session_events(&state_tick, ada.session);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::PassiveState(_)))
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::PassiveState(batch) if *batch == expected)),
        "the persisting batch carries the exact settled body"
    );
    // The drained state-tick bytes carry the state packet and no despawn.
    let frames = adapter.drain(ada.session);
    let decoded: Vec<ServerPacket> = frames.iter().map(|frame| decode_play(frame)).collect();
    assert!(
        decoded
            .iter()
            .any(|packet| matches!(packet, ServerPacket::PassiveState(_))),
        "the drained state frames carry the state packet"
    );
    assert!(
        !decoded
            .iter()
            .any(|packet| matches!(packet, ServerPacket::PassiveDespawn(_))),
        "no despawn leaves before the removal"
    );

    adapter.stage(Box::new(move |context| {
        let mut actor = context
            .read()
            .actor(ActorKey::Passive(id))
            .cloned()
            .unwrap();
        actor.lifecycle = ActorLifecycle::Dead;
        context.stage(RuleEffect::Actor(actor)).unwrap();
    }));
    let despawn_tick = adapter.tick();
    let events = session_events(&despawn_tick, ada.session);
    let expected = PassiveDespawn::try_new(PassiveDespawnParts {
        server_tick: 2,
        despawns: vec![PassiveDespawnRecord::new(id, PassiveDespawnReason::Died)]
            .into_boxed_slice(),
    })
    .unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::PassiveDespawn(batch) if *batch == expected)),
        "the death settlement publishes the died despawn"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::PassiveState(_)))
    );
    // The drained despawn-tick bytes carry exactly the died wire batch.
    let frames = adapter.drain(ada.session);
    let decoded: Vec<ServerPacket> = frames.iter().map(|frame| decode_play(frame)).collect();
    let despawns: Vec<&mornlea_protocol::PassiveDespawn> = decoded
        .iter()
        .filter_map(|packet| match packet {
            ServerPacket::PassiveDespawn(batch) => Some(batch),
            _ => None,
        })
        .collect();
    assert_eq!(
        despawns.len(),
        1,
        "the death publishes exactly one despawn packet"
    );
    assert_eq!(despawns[0].server_tick, 2);
    assert_eq!(
        despawns[0].despawns,
        vec![mornlea_protocol::PassiveDespawnRecord {
            id,
            reason: mornlea_protocol::PASSIVE_DESPAWN_DIED,
        }],
        "the wire batch carries the died reason"
    );

    // One tick later the departure never repeats on either adapter.
    let after_tick = adapter.tick();
    let frames = adapter.drain(ada.session);
    let decoded: Vec<ServerPacket> = frames.iter().map(|frame| decode_play(frame)).collect();
    assert!(
        !session_events(&after_tick, ada.session)
            .iter()
            .any(|event| matches!(
                event,
                Event::PassiveDespawn(batch)
                    if batch.despawns().iter().any(|record| record.id() == id)
            )),
        "the died passive despawns exactly once"
    );
    assert!(
        !decoded
            .iter()
            .any(|packet| matches!(packet, ServerPacket::PassiveDespawn(_))),
        "the follow-up drain carries no despawn packet"
    );

    vec![spawn_tick, state_tick, despawn_tick, after_tick]
}

/// The passive spawn, state, and died-despawn publications through both
/// real adapters, with identical publications on every tick.
#[test]
fn passive_lifecycle_publications_match_across_adapters() {
    let mut memory = MemoryParity::new();
    let memory = passive_lifecycle_scenario(&mut memory);
    let mut tcp = TcpParity::new();
    let tcp = passive_lifecycle_scenario(&mut tcp);
    assert_eq!(
        memory, tcp,
        "the passive lifecycle publishes identically across the real adapters"
    );
}

/// The passive removal-reason scenario: two observing peers share interest
/// in two cows while a third peer never subscribes to their chunk. One cow
/// dies while the first observer exits interest (death wins, Died); the
/// second cow only exits that observer's interest (Vanished). The staying
/// observer sees only the death, the far peer sees no passive frames and
/// stays active, and the next tick repeats nothing. Returns every published
/// tick plus every drained frame per session in login order.
fn passive_removal_reason_scenario(
    adapter: &mut dyn ParityAdapter,
) -> (Vec<TickPublication>, Vec<Vec<Vec<u8>>>) {
    seed_day_world(adapter);
    adapter.stage_login(
        1,
        stored_player(1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    adapter.stage_login(
        2,
        stored_player(2, "Bea", [4.5, 65.0, 4.5], 0.0, 0.0, |_| {}),
    );
    adapter.stage_login(
        3,
        stored_player(3, "Cleo", [400.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    let ada = adapter.login(1, "Ada");
    let bea = adapter.login(2, "Bea");
    let cleo = adapter.login(3, "Cleo");
    let dead = PassiveId::try_new(7).unwrap();
    let live = PassiveId::try_new(11).unwrap();
    adapter.stage(Box::new(move |context| {
        context
            .stage(RuleEffect::Actor(passive_actor(7, [8.5, 65.0, 8.5])))
            .unwrap();
        context
            .stage(RuleEffect::Runtime(passive_runtime(dead)))
            .unwrap();
        context
            .stage(RuleEffect::Actor(passive_actor(11, [9.5, 65.0, 9.5])))
            .unwrap();
        context
            .stage(RuleEffect::Runtime(passive_runtime(live)))
            .unwrap();
    }));

    // Decodes one session's drained bytes and keeps the passive despawn
    // packets in publication order.
    let despawn_packets = |drained: &[Vec<u8>]| -> Vec<mornlea_protocol::PassiveDespawn> {
        drained
            .iter()
            .map(|frame| decode_play(frame))
            .filter_map(|packet| match packet {
                ServerPacket::PassiveDespawn(batch) => Some(batch),
                _ => None,
            })
            .collect()
    };

    let spawn_tick = adapter.tick();
    for peer in [&ada, &bea] {
        assert!(
            session_events(&spawn_tick, peer.session)
                .iter()
                .any(|event| matches!(event, Event::PassiveSpawn(_))),
            "both observers see the shared spawn batch"
        );
    }
    assert!(
        !session_events(&spawn_tick, cleo.session)
            .iter()
            .any(|event| matches!(
                event,
                Event::PassiveSpawn(_) | Event::PassiveState(_) | Event::PassiveDespawn(_)
            )),
        "the far peer never subscribes to the cows' chunk"
    );
    let mut frames = vec![
        adapter.drain(ada.session),
        adapter.drain(bea.session),
        adapter.drain(cleo.session),
    ];
    let cleo_spawn: Vec<ServerPacket> = frames[2].iter().map(|frame| decode_play(frame)).collect();
    assert!(
        !cleo_spawn.iter().any(|packet| matches!(
            packet,
            ServerPacket::PassiveSpawn(_)
                | ServerPacket::PassiveState(_)
                | ServerPacket::PassiveDespawn(_)
        )),
        "no passive frame leaks to the non-observer"
    );

    // One cow dies while Ada walks out of interest: death wins that
    // simultaneous exit, the living cow's exit stays vanished, and Bea —
    // who stays — sees only the death.
    let ada_session = ada.session;
    adapter.stage(Box::new(move |context| {
        let mut gone = context
            .read()
            .actor(ActorKey::Passive(dead))
            .cloned()
            .unwrap();
        gone.lifecycle = ActorLifecycle::Dead;
        context.stage(RuleEffect::Actor(gone)).unwrap();
        let mut walker = context
            .read()
            .actor(ActorKey::Player(ada_session))
            .cloned()
            .unwrap();
        walker.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([500.5, 65.0, 4.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        context.stage(RuleEffect::Actor(walker)).unwrap();
    }));
    let despawn_tick = adapter.tick();
    let ada_expected = PassiveDespawn::try_new(PassiveDespawnParts {
        server_tick: 1,
        despawns: vec![
            PassiveDespawnRecord::new(dead, PassiveDespawnReason::Died),
            PassiveDespawnRecord::new(live, PassiveDespawnReason::Vanished),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    assert!(
        session_events(&despawn_tick, ada.session)
            .iter()
            .any(|event| matches!(event, Event::PassiveDespawn(batch) if *batch == ada_expected)),
        "death beats Ada's simultaneous view exit; the live exit stays vanished"
    );
    let bea_expected = PassiveDespawn::try_new(PassiveDespawnParts {
        server_tick: 1,
        despawns: vec![PassiveDespawnRecord::new(dead, PassiveDespawnReason::Died)]
            .into_boxed_slice(),
    })
    .unwrap();
    assert!(
        session_events(&despawn_tick, bea.session)
            .iter()
            .any(|event| matches!(event, Event::PassiveDespawn(batch) if *batch == bea_expected)),
        "the staying observer sees only the death"
    );
    assert!(
        !session_events(&despawn_tick, cleo.session)
            .iter()
            .any(|event| matches!(event, Event::PassiveDespawn(_))),
        "only sessions that previously observed them see the despawn"
    );
    frames[0].extend(adapter.drain(ada.session));
    frames[1].extend(adapter.drain(bea.session));
    frames[2].extend(adapter.drain(cleo.session));
    // The drained bytes carry the exact died/vanished wire batches in
    // publication order.
    let ada_wire = mornlea_protocol::PassiveDespawn::new(
        1,
        vec![
            mornlea_protocol::PassiveDespawnRecord {
                id: dead,
                reason: mornlea_protocol::PASSIVE_DESPAWN_DIED,
            },
            mornlea_protocol::PassiveDespawnRecord {
                id: live,
                reason: mornlea_protocol::PASSIVE_DESPAWN_VANISHED,
            },
        ],
    )
    .unwrap();
    assert_eq!(
        despawn_packets(&frames[0]),
        vec![ada_wire],
        "Ada's drained bytes carry the exact died/vanished batch in order"
    );
    let bea_wire = mornlea_protocol::PassiveDespawn::new(
        1,
        vec![mornlea_protocol::PassiveDespawnRecord {
            id: dead,
            reason: mornlea_protocol::PASSIVE_DESPAWN_DIED,
        }],
    )
    .unwrap();
    assert_eq!(
        despawn_packets(&frames[1]),
        vec![bea_wire],
        "Bea's drained bytes carry exactly the died batch"
    );
    let cleo_packets: Vec<ServerPacket> =
        frames[2].iter().map(|frame| decode_play(frame)).collect();
    assert!(
        !cleo_packets.iter().any(|packet| matches!(
            packet,
            ServerPacket::PassiveSpawn(_)
                | ServerPacket::PassiveState(_)
                | ServerPacket::PassiveDespawn(_)
        )),
        "no passive frame leaks to the non-observer"
    );
    assert_eq!(
        adapter
            .endpoint()
            .authority
            .session(cleo.session)
            .unwrap()
            .phase,
        SessionPhase::Active,
        "the unaffected peer remains active"
    );

    // The subsequent tick repeats no departure on any session.
    let after_tick = adapter.tick();
    for peer in [&ada, &bea] {
        assert!(
            !session_events(&after_tick, peer.session)
                .iter()
                .any(|event| matches!(event, Event::PassiveDespawn(_))),
            "the departure never repeats"
        );
    }
    frames[0].extend(adapter.drain(ada.session));
    frames[1].extend(adapter.drain(bea.session));
    frames[2].extend(adapter.drain(cleo.session));
    assert!(
        despawn_packets(&frames[0]).len() == 1 && despawn_packets(&frames[1]).len() == 1,
        "the follow-up tick publishes no second despawn packet"
    );

    (vec![spawn_tick, despawn_tick, after_tick], frames)
}

/// The passive removal reasons publish identically through both real
/// adapters: exact died/vanished records in publication order, identical
/// drained bytes, no leak to the non-observer, and no repetition.
#[test]
fn passive_removal_reasons_match_across_adapters() {
    let mut memory = MemoryParity::new();
    let memory = passive_removal_reason_scenario(&mut memory);
    let mut tcp = TcpParity::new();
    let tcp = passive_removal_reason_scenario(&mut tcp);
    assert_eq!(
        memory.0, tcp.0,
        "the removal publications match across the real adapters"
    );
    assert_eq!(
        memory.1, tcp.1,
        "the drained bytes match across the real adapters"
    );
}

/// The projectile lifecycle scenario: one staged shard publishes its spawn,
/// its flight state each later tick, and its age-expiry despawn. Flight
/// values derive from the post-tick committed record, asserted identically
/// through both adapters.
fn projectile_lifecycle_scenario(adapter: &mut dyn ParityAdapter) -> Vec<TickPublication> {
    seed_day_world(adapter);
    adapter.stage_login(
        1,
        stored_player(1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    let ada = adapter.login(1, "Ada");
    let id = ProjectileId::try_new(5).unwrap();
    adapter.stage(Box::new(move |context| {
        context
            .stage(RuleEffect::Projectile {
                before: None,
                after: Some(ProjectileRecord {
                    id,
                    owner: ActorKey::Player(ada.session),
                    dimension: Dimension::OVERWORLD,
                    position: FiniteVec3::try_new([4.5, 100.0, 4.5]).unwrap(),
                    velocity: FiniteVec3::try_new([0.0, 0.0, 0.0]).unwrap(),
                    kind: ProjectileKind::Shard,
                    damage: 3,
                    age: 97,
                }),
            })
            .unwrap();
    }));

    let spawn_tick = adapter.tick();
    let committed = {
        let residents = adapter.endpoint().authority.residents();
        residents
            .projectiles
            .iter()
            .find(|record| record.id == id)
            .cloned()
            .unwrap()
    };
    let events = session_events(&spawn_tick, ada.session);
    let expected = ProjectileSpawn::try_new(ProjectileSpawnParts {
        server_tick: 0,
        spawns: vec![ProjectileSpawnRecord::new(ProjectileSpawnRecordParts {
            id,
            kind: ProjectileKind::Shard,
            dimension: Dimension::OVERWORLD,
            position: committed.position,
            velocity: committed.velocity,
        })]
        .into_boxed_slice(),
    })
    .unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ProjectileSpawn(batch) if *batch == expected)),
        "the staged shard publishes its exact spawn record"
    );
    let _ = adapter.drain(ada.session);

    let state_tick = adapter.tick();
    let committed = {
        let residents = adapter.endpoint().authority.residents();
        residents
            .projectiles
            .iter()
            .find(|record| record.id == id)
            .cloned()
            .unwrap()
    };
    let expected = ProjectileState::try_new(ProjectileStateParts {
        server_tick: 1,
        states: vec![ProjectileStateRecord::new(ProjectileStateRecordParts {
            id,
            position: committed.position,
        })]
        .into_boxed_slice(),
    })
    .unwrap();
    let events = session_events(&state_tick, ada.session);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::ProjectileState(_)))
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ProjectileState(batch) if *batch == expected)),
        "the flight tick publishes the exact flight body"
    );
    let _ = adapter.drain(ada.session);

    // The age ceiling removes the projectile: the despawn publishes and the
    // state family falls silent.
    let settle_tick = adapter.tick();
    let _ = adapter.drain(ada.session);
    let despawn_tick = adapter.tick();
    let events = session_events(&despawn_tick, ada.session);
    let expected =
        mornlea_domain::ProjectileDespawn::try_new(mornlea_domain::ProjectileDespawnParts {
            server_tick: 3,
            ids: vec![id].into_boxed_slice(),
        })
        .unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ProjectileDespawn(batch) if *batch == expected)),
        "the age expiry publishes the exact despawn batch"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::ProjectileState(_)))
    );
    let _ = adapter.drain(ada.session);

    // One tick later the departure never repeats on either adapter.
    let after_tick = adapter.tick();
    let _ = adapter.drain(ada.session);
    assert!(
        !session_events(&after_tick, ada.session)
            .iter()
            .any(|event| matches!(
                event,
                Event::ProjectileDespawn(batch) if batch.ids().contains(&id)
            )),
        "the expired projectile despawns exactly once"
    );

    vec![
        spawn_tick,
        state_tick,
        settle_tick,
        despawn_tick,
        after_tick,
    ]
}

/// The projectile spawn, flight-state, and expiry-despawn publications
/// through both real adapters, with identical publications on every tick.
#[test]
fn projectile_lifecycle_publications_match_across_adapters() {
    let mut memory = MemoryParity::new();
    let memory = projectile_lifecycle_scenario(&mut memory);
    let mut tcp = TcpParity::new();
    let tcp = projectile_lifecycle_scenario(&mut tcp);
    assert_eq!(
        memory, tcp,
        "the projectile lifecycle publishes identically across the real adapters"
    );
}

/// The pickup scene: one fresh staged drop publishes its exact upsert, a
/// real southbound walk onto it removes it through the pickup, and the next
/// tick republishes nothing for the departed identity.
fn drop_pickup_scene(adapter: &mut dyn ParityAdapter) -> Vec<TickPublication> {
    seed_day_world(adapter);
    adapter.stage_login(
        1,
        stored_player(1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    let ada = adapter.login(1, "Ada");
    adapter.stage(Box::new(|context| {
        context.preload_drop(drop_record(2, 40, [1.5, 65.5, 2.5]));
    }));

    let upsert_tick = adapter.tick();
    let events = session_events(&upsert_tick, ada.session);
    let expected = ItemDropUpserts::try_new(mornlea_domain::ItemDropUpsertsParts {
        server_tick: 0,
        drops: vec![
            ItemDrop::try_new(ItemDropParts {
                id: DropId::try_new(0, ChunkPos::new(0, 0), 2, 1).unwrap(),
                block_index: mornlea_domain::chunk_block_index(BlockPos::new(1, 65, 2)),
                stack: stack(ITEM_COAL, 2),
            })
            .unwrap(),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ItemDropUpserts(batch) if *batch == expected)),
        "the fresh drop publishes its exact upsert"
    );
    let _ = adapter.drain(ada.session);

    // A real walk south onto the drop removes it through the pickup.
    adapter.send_packet(
        ada.conn,
        &ClientPacket::PlayerInput(
            mornlea_protocol::PlayerInput::new(
                1,
                0,
                1,
                false,
                std::f32::consts::PI,
                0.0,
                false,
                false,
                false,
                false,
            )
            .unwrap(),
        ),
    );
    let mut ticks = vec![upsert_tick];
    let mut pickup_tick = None;
    for _ in 0..40 {
        let publication = adapter.tick();
        let _ = adapter.drain(ada.session);
        let picked = session_events(&publication, ada.session)
            .into_iter()
            .any(|event| matches!(event, Event::ItemDropRemoves(_)));
        ticks.push(publication.clone());
        if picked {
            pickup_tick = Some(publication);
            break;
        }
    }
    let pickup_tick = pickup_tick.expect("the walk reaches the drop within the window");
    let departed = DropId::try_new(0, ChunkPos::new(0, 0), 2, 1).unwrap();
    let expected = mornlea_domain::ItemDropRemoves::try_new(mornlea_domain::ItemDropRemovesParts {
        server_tick: pickup_tick.tick,
        ids: vec![departed].into_boxed_slice(),
    })
    .unwrap();
    let pickup_events = session_events(&pickup_tick, ada.session);
    let removes: Vec<&Event> = pickup_events
        .iter()
        .filter(|event| matches!(event, Event::ItemDropRemoves(_)))
        .collect();
    assert_eq!(
        removes.len(),
        1,
        "the pickup tick publishes one removes batch"
    );
    assert!(
        matches!(removes[0], Event::ItemDropRemoves(batch) if *batch == expected),
        "the pickup removes exactly the walked-onto drop"
    );

    // One tick later the departure never repeats: neither drop family
    // mentions the picked-up identity again on either adapter.
    let after_tick = adapter.tick();
    let _ = adapter.drain(ada.session);
    let after_events = session_events(&after_tick, ada.session);
    assert!(
        !after_events
            .iter()
            .any(|event| matches!(event, Event::ItemDropRemoves(_))),
        "the picked-up drop is never removed twice"
    );
    assert!(
        !after_events.iter().any(|event| matches!(
            event,
            Event::ItemDropUpserts(batch)
                if batch.drops().iter().any(|drop| drop.id() == departed)
        )),
        "the picked-up drop never re-upserts"
    );
    ticks.push(after_tick);
    ticks
}

/// The expiry scene: one aged staged drop publishes its exact upsert, the
/// lifetime ceiling removes it, and the next tick republishes nothing for the
/// departed identity.
fn drop_expiry_scene(adapter: &mut dyn ParityAdapter) -> Vec<TickPublication> {
    seed_day_world(adapter);
    adapter.stage_login(
        1,
        stored_player(1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    let ada = adapter.login(1, "Ada");
    adapter.stage(Box::new(|context| {
        context.preload_drop(drop_record(3, DROP_LIFETIME_MARGIN, [3.5, 65.5, 3.5]));
    }));

    let upsert_tick = adapter.tick();
    let events = session_events(&upsert_tick, ada.session);
    let expected = ItemDropUpserts::try_new(mornlea_domain::ItemDropUpsertsParts {
        server_tick: 0,
        drops: vec![
            ItemDrop::try_new(ItemDropParts {
                id: DropId::try_new(0, ChunkPos::new(0, 0), 3, 1).unwrap(),
                block_index: mornlea_domain::chunk_block_index(BlockPos::new(3, 65, 3)),
                stack: stack(ITEM_COAL, 2),
            })
            .unwrap(),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ItemDropUpserts(batch) if *batch == expected)),
        "the aged drop publishes its exact upsert"
    );
    let _ = adapter.drain(ada.session);

    let mut ticks = vec![upsert_tick];
    let mut expiry_tick = None;
    for _ in 0..6 {
        let publication = adapter.tick();
        let _ = adapter.drain(ada.session);
        let expired = session_events(&publication, ada.session)
            .into_iter()
            .any(|event| matches!(event, Event::ItemDropRemoves(_)));
        ticks.push(publication.clone());
        if expired {
            expiry_tick = Some(publication);
            break;
        }
    }
    let expiry_tick = expiry_tick.expect("the aged drop expires within the window");
    let departed = DropId::try_new(0, ChunkPos::new(0, 0), 3, 1).unwrap();
    let expected = mornlea_domain::ItemDropRemoves::try_new(mornlea_domain::ItemDropRemovesParts {
        server_tick: expiry_tick.tick,
        ids: vec![departed].into_boxed_slice(),
    })
    .unwrap();
    let expiry_events = session_events(&expiry_tick, ada.session);
    let removes: Vec<&Event> = expiry_events
        .iter()
        .filter(|event| matches!(event, Event::ItemDropRemoves(_)))
        .collect();
    assert_eq!(
        removes.len(),
        1,
        "the expiry tick publishes one removes batch"
    );
    assert!(
        matches!(removes[0], Event::ItemDropRemoves(batch) if *batch == expected),
        "the expiry removes exactly the aged drop"
    );

    // One tick later the departure never repeats: neither drop family
    // mentions the expired identity again on either adapter.
    let after_tick = adapter.tick();
    let _ = adapter.drain(ada.session);
    let after_events = session_events(&after_tick, ada.session);
    assert!(
        !after_events
            .iter()
            .any(|event| matches!(event, Event::ItemDropRemoves(_))),
        "the expired drop is never removed twice"
    );
    assert!(
        !after_events.iter().any(|event| matches!(
            event,
            Event::ItemDropUpserts(batch)
                if batch.drops().iter().any(|drop| drop.id() == departed)
        )),
        "the expired drop never re-upserts"
    );
    ticks.push(after_tick);
    ticks
}

/// The item-drop upsert and removal publications through both real adapters:
/// the exact staged upsert, a real-input walk pickup, and an aged expiry,
/// each removal observed on its first publication, with identical
/// publications on every tick.
#[test]
fn item_drop_publications_match_across_adapters() {
    let mut memory = MemoryParity::new();
    let memory_pickup = drop_pickup_scene(&mut memory);
    let mut memory = MemoryParity::new();
    let memory_expiry = drop_expiry_scene(&mut memory);
    let mut tcp = TcpParity::new();
    let tcp_pickup = drop_pickup_scene(&mut tcp);
    let mut tcp = TcpParity::new();
    let tcp_expiry = drop_expiry_scene(&mut tcp);
    assert_eq!(
        memory_pickup, tcp_pickup,
        "the pickup publications are identical across the real adapters"
    );
    assert_eq!(
        memory_expiry, tcp_expiry,
        "the expiry publications are identical across the real adapters"
    );
}

/// The wire form of one domain container reference.
fn wire_container(
    chunk_x: i32,
    chunk_z: i32,
    kind: u8,
    slot: u8,
) -> mornlea_protocol::ContainerRef {
    mornlea_protocol::ContainerRef {
        dimension: 0,
        chunk_x,
        chunk_z,
        kind,
        slot,
        generation: 1,
    }
}

/// The record-state scenario: real commands publish the exact owner-only
/// inventory, chest, crafting, and furnace states, a close publishes the
/// container-closed notice, and one refused command publishes no record
/// event at all.
fn record_state_scenario(adapter: &mut dyn ParityAdapter) -> Vec<TickPublication> {
    // The world: standing ground with a furnace behind Ada, and a chest
    // chunk in front of her.
    adapter.stage(Box::new(|context| {
        context
            .stage(RuleEffect::Environment(day_environment(1_000)))
            .unwrap();
        let mut front = ground_chunk();
        chest_in_chunk(
            &mut front,
            BlockPos::new(0, 66, -1),
            [StorageStack::default(); 27],
        );
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, -1), 1, 1, front).unwrap());
        let mut home = ground_chunk();
        furnace_in_chunk(
            &mut home,
            BlockPos::new(0, 66, 1),
            storage(ITEM_RAW_IRON, 2),
            storage(ITEM_COAL, 2),
            StorageStack::default(),
        );
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, 0), 1, 1, home).unwrap());
    }));
    adapter.stage_login(
        1,
        stored_player(1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |player| {
            player.inventory.backpack[0] = storage(ITEM_DIRT, 5);
        }),
    );
    let ada = adapter.login(1, "Ada");
    adapter.stage_login(
        2,
        stored_player(2, "Bea", [4.5, 65.0, 4.5], 0.0, 0.0, |_| {}),
    );
    let bea = adapter.login(2, "Bea");
    let tick_zero = adapter.tick();
    for peer in [&ada, &bea] {
        let _ = adapter.drain(peer.session);
    }

    let chest_domain =
        mornlea_domain::ContainerRef::try_new(ChunkPos::new(0, -1), ContainerKind::Chest, 0, 1)
            .unwrap();
    let chest_wire = wire_container(0, -1, CONTAINER_KIND_CHEST, 0);
    let furnace_domain =
        mornlea_domain::ContainerRef::try_new(ChunkPos::new(0, 0), ContainerKind::Furnace, 1, 1)
            .unwrap();

    // A hotbar selection publishes the exact owner-only inventory; the
    // refused drop from the lease-less peer publishes no record event.
    adapter.send_packet(
        ada.conn,
        &ClientPacket::SelectHotbar(mornlea_protocol::SelectHotbar::new(1, 2).unwrap()),
    );
    adapter.send_packet(
        bea.conn,
        &ClientPacket::DropStack(
            mornlea_protocol::DropStack::new(1, chest_wire, STACK_VIEW_CONTAINER, 36).unwrap(),
        ),
    );
    let tick_one = adapter.tick();
    assert_eq!(
        tick_counters(&tick_one),
        (2, 0, 0),
        "the selection and the refused drop both consume their commands"
    );
    let expected_inventory = InventoryState::new(InventoryStateParts {
        selected: HotbarSlot::new(2).unwrap(),
        hotbar: item_array(&[]),
        backpack: item_array(&[(0, ITEM_DIRT, 5)]),
    });
    let ada_events = session_events(&tick_one, ada.session);
    assert_eq!(
        ada_events
            .iter()
            .filter(|event| matches!(event, Event::InventoryState(_)))
            .count(),
        1
    );
    assert!(
        ada_events.iter().any(
            |event| matches!(event, Event::InventoryState(state) if *state == expected_inventory)
        ),
        "the selection publishes the exact inventory to the owner"
    );
    let bea_events = session_events(&tick_one, bea.session);
    assert!(
        !bea_events.iter().any(|event| matches!(
            event,
            Event::InventoryState(_)
                | Event::ChestState(_)
                | Event::FurnaceState(_)
                | Event::CraftingState(_)
                | Event::ContainerClosed(_)
        )),
        "the refused command publishes no record event"
    );
    for peer in [&ada, &bea] {
        let _ = adapter.drain(peer.session);
    }

    // Opening the chest publishes its exact (empty) contents.
    adapter.send_packet(
        ada.conn,
        &ClientPacket::OpenContainer(mornlea_protocol::OpenContainer::new(2, 0.0, 0.0).unwrap()),
    );
    let tick_two = adapter.tick();
    let expected_chest = mornlea_domain::ChestState::try_new(mornlea_domain::ChestStateParts {
        container: chest_domain,
        items: item_array(&[]),
    })
    .unwrap();
    assert!(
        session_events(&tick_two, ada.session)
            .iter()
            .any(|event| matches!(event, Event::ChestState(state) if *state == expected_chest))
    );
    let _ = adapter.drain(ada.session);
    let _ = adapter.drain(bea.session);

    // One partial move publishes the moved chest contents.
    adapter.send_packet(
        ada.conn,
        &ClientPacket::MoveStackPartial(
            MoveStackPartial::new(3, chest_wire, STACK_VIEW_CONTAINER, 9, 36, true).unwrap(),
        ),
    );
    let tick_three = adapter.tick();
    let expected_chest = mornlea_domain::ChestState::try_new(mornlea_domain::ChestStateParts {
        container: chest_domain,
        items: item_array(&[(0, ITEM_DIRT, 1)]),
    })
    .unwrap();
    assert!(
        session_events(&tick_three, ada.session)
            .iter()
            .any(|event| matches!(event, Event::ChestState(state) if *state == expected_chest)),
        "the partial move publishes the moved chest contents"
    );
    let _ = adapter.drain(ada.session);
    let _ = adapter.drain(bea.session);

    // Closing publishes the exact released reference.
    adapter.send_packet(
        ada.conn,
        &ClientPacket::CloseContainer(mornlea_protocol::CloseContainer::new(4)),
    );
    let tick_four = adapter.tick();
    assert!(
        session_events(&tick_four, ada.session)
            .iter()
            .any(|event| matches!(event, Event::ContainerClosed(closed)
                if *closed == mornlea_domain::ContainerClosed::new(chest_domain)))
    );
    let _ = adapter.drain(ada.session);
    let _ = adapter.drain(bea.session);

    // The staged personal stone-hoe grid publishes with its exact matched
    // output; the atomic output take empties it.
    adapter.stage(Box::new(move |context| {
        let actor = ActorKey::Player(ada.session);
        let mut record = context.read().inventory(actor).cloned().unwrap();
        record.crafting = storage_array(&[
            (0, ITEM_STONE, 1),
            (1, ITEM_STICK, 1),
            (2, ITEM_STONE, 1),
            (3, ITEM_STICK, 1),
        ]);
        context.preload_inventory(actor, record);
    }));
    let tick_five = adapter.tick();
    let expected_crafting =
        mornlea_domain::CraftingState::try_new(mornlea_domain::CraftingStateParts {
            size: mornlea_domain::CraftingSize::Personal,
            slots: item_array(&[
                (0, ITEM_STONE, 1),
                (1, ITEM_STICK, 1),
                (2, ITEM_STONE, 1),
                (3, ITEM_STICK, 1),
            ]),
            output: ItemStack::try_new(ITEM_STONE_HOE, 1, 131).unwrap(),
        })
        .unwrap();
    assert!(
        session_events(&tick_five, ada.session).iter().any(
            |event| matches!(event, Event::CraftingState(state) if *state == expected_crafting)
        ),
        "the staged grid publishes the exact stone-hoe output"
    );
    let _ = adapter.drain(ada.session);
    let _ = adapter.drain(bea.session);

    adapter.send_packet(
        ada.conn,
        &ClientPacket::TakeCraftingOutput(mornlea_protocol::TakeCraftingOutput::new(5).unwrap()),
    );
    let tick_six = adapter.tick();
    let expected_crafting =
        mornlea_domain::CraftingState::try_new(mornlea_domain::CraftingStateParts {
            size: mornlea_domain::CraftingSize::Personal,
            slots: item_array(&[]),
            output: ItemStack::EMPTY,
        })
        .unwrap();
    assert!(
        session_events(&tick_six, ada.session).iter().any(
            |event| matches!(event, Event::CraftingState(state) if *state == expected_crafting)
        ),
        "the atomic take empties the published grid"
    );
    let _ = adapter.drain(ada.session);
    let _ = adapter.drain(bea.session);

    // Opening the furnace publishes its burning body exactly: ignition ran
    // on tick zero and every later tick advanced both timers.
    adapter.send_packet(
        ada.conn,
        &ClientPacket::OpenContainer(
            mornlea_protocol::OpenContainer::new(6, std::f32::consts::PI, 0.0).unwrap(),
        ),
    );
    let tick_seven = adapter.tick();
    let expected_furnace =
        mornlea_domain::FurnaceState::try_new(mornlea_domain::FurnaceStateParts {
            container: furnace_domain,
            input: stack(ITEM_RAW_IRON, 2),
            fuel: stack(ITEM_COAL, 1),
            output: ItemStack::EMPTY,
            progress_ticks: 8,
            burn_ticks: 1_600 - 8,
        })
        .unwrap();
    assert!(
        session_events(&tick_seven, ada.session)
            .iter()
            .any(|event| matches!(event, Event::FurnaceState(state) if *state == expected_furnace)),
        "the furnace publishes its exact ignited body"
    );
    let _ = adapter.drain(ada.session);
    let _ = adapter.drain(bea.session);

    vec![
        tick_zero, tick_one, tick_two, tick_three, tick_four, tick_five, tick_six, tick_seven,
    ]
}

/// The owner-only record states through both real adapters: exact inventory,
/// chest, crafting, and furnace publications from real commands, the exact
/// container-closed notice, a refused command publishing nothing, and
/// identical publications on every tick.
#[test]
fn record_state_publications_match_across_adapters() {
    let mut memory = MemoryParity::new();
    let memory = record_state_scenario(&mut memory);
    let mut tcp = TcpParity::new();
    let tcp = record_state_scenario(&mut tcp);
    assert_eq!(
        memory, tcp,
        "the record-state publications are identical across the real adapters"
    );
}

/// One combined scene: two sessions, one ready chunk, one companion, and one
/// staged drop, published on the first tick. Returns the publication with
/// both peers; the outboxes stay undrained for the caller's wire capture.
fn combined_scene(adapter: &mut dyn ParityAdapter) -> (TickPublication, Peer, Peer) {
    seed_day_world(adapter);
    adapter.stage_login(
        1,
        stored_player(1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    let ada = adapter.login(1, "Ada");
    adapter.stage_login(
        2,
        stored_player(2, "Bea", [4.5, 65.0, 4.5], 0.0, 0.0, |_| {}),
    );
    let bea = adapter.login(2, "Bea");
    let id = companion_id(9);
    adapter.stage(Box::new(move |context| {
        context
            .stage(RuleEffect::Actor(companion_actor(
                id,
                [8.5, 65.0, 8.5],
                0.0,
            )))
            .unwrap();
        context.preload_drop(drop_record(3, 40, [0.5, 65.5, 2.5]));
    }));
    (adapter.tick(), ada, bea)
}

/// The frozen family order one session observes on the wire for the combined
/// scene's first tick.
const COMBINED_FIRST_TICK_ORDER: [&str; 7] = [
    "chunk-snapshot",
    "companion-spawn",
    "remote-spawn",
    "drop-upserts",
    "inventory-state",
    "crafting-state",
    "player-state",
];

/// Asserts one session's captured frames decode to exactly that session's
/// publication order under the frozen family order.
fn assert_wire_order(label: &str, tick: &TickPublication, peer: &Peer, frames: &[Vec<u8>]) {
    let events = session_events(tick, peer.session);
    assert_eq!(
        frames.len(),
        events.len(),
        "{label} receives one frame per session event"
    );
    let decoded: Vec<ServerPacket> = frames.iter().map(|f| decode_play(f)).collect();
    let families: Vec<&str> = decoded.iter().map(|packet| family(packet)).collect();
    assert_eq!(
        families, COMBINED_FIRST_TICK_ORDER,
        "{label} observes the frozen family order on the wire"
    );
    for (packet, event) in decoded.iter().zip(&events) {
        assert_eq!(
            packet,
            &ServerPacket::try_from(event.clone()).unwrap(),
            "{label}'s frames carry the exact publication packets in order"
        );
    }
}

/// One combined scene's first tick through both real adapters: each session's
/// drained frames decode to exactly that session's publication order under
/// the frozen family order, the memory frames come from the per-session
/// outbox drain, the TCP frames come through the client's live socket reader,
/// and the two frame sets are byte-for-byte identical.
#[test]
fn publication_wire_delivery_reaches_each_session_in_order() {
    let mut memory = MemoryParity::new();
    let (memory_tick, memory_ada, memory_bea) = combined_scene(&mut memory);
    let memory_ada_frames =
        MemoryTransport::drain_session(&mut memory.endpoint, memory_ada.session, 16, 1 << 20)
            .unwrap();
    let memory_bea_frames =
        MemoryTransport::drain_session(&mut memory.endpoint, memory_bea.session, 16, 1 << 20)
            .unwrap();

    let mut tcp = TcpParity::new();
    let (tcp_tick, tcp_ada, tcp_bea) = combined_scene(&mut tcp);
    let tcp_ada_frames =
        tcp.socket_frames(&tcp_ada, session_events(&tcp_tick, tcp_ada.session).len());
    let tcp_bea_frames =
        tcp.socket_frames(&tcp_bea, session_events(&tcp_tick, tcp_bea.session).len());

    assert_wire_order("memory Ada", &memory_tick, &memory_ada, &memory_ada_frames);
    assert_wire_order("memory Bea", &memory_tick, &memory_bea, &memory_bea_frames);
    assert_wire_order("tcp Ada", &tcp_tick, &tcp_ada, &tcp_ada_frames);
    assert_wire_order("tcp Bea", &tcp_tick, &tcp_bea, &tcp_bea_frames);
    assert_eq!(
        memory_tick, tcp_tick,
        "the combined scene publishes identically across the real adapters"
    );
    assert_eq!(
        memory_ada_frames, tcp_ada_frames,
        "Ada's memory and socket frames are byte-for-byte identical"
    );
    assert_eq!(
        memory_bea_frames, tcp_bea_frames,
        "Bea's memory and socket frames are byte-for-byte identical"
    );
}

/// Replaces the legacy radius-two fixture authority with a freshly
/// constructed true-default authority (view bound 33) before any login.
fn replace_with_default_authority(adapter: &mut dyn ParityAdapter) {
    adapter.endpoint().authority = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        WORLD_SEED,
    )
    .unwrap();
    assert_eq!(
        adapter.endpoint().authority.limits().view_radius(),
        33,
        "the wanted fixtures run under the true default bound"
    );
}

fn snapshot_keys(events: &[Event]) -> Vec<ChunkPos> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ChunkSnapshot(snapshot) => Some(snapshot.chunk()),
            _ => None,
        })
        .collect()
}

fn forget_keys(events: &[Event]) -> Vec<ChunkPos> {
    let mut keys = Vec::new();
    for event in events {
        if let Event::ForgetChunks(batch) = event {
            keys.extend(batch.chunks().iter().copied());
        }
    }
    keys
}

fn snapshot_revisions(events: &[Event]) -> Vec<(ChunkPos, u64)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ChunkSnapshot(snapshot) => Some((snapshot.chunk(), snapshot.revision())),
            _ => None,
        })
        .collect()
}

/// Decodes every drained frame as Play-state packets and checks the frames
/// carry exactly the session's publication events in order.
fn assert_frames_match_events(
    label: &str,
    tick: &TickPublication,
    peer: &Peer,
    frames: &[Vec<u8>],
) {
    let events = session_events(tick, peer.session);
    assert_eq!(
        frames.len(),
        events.len(),
        "{label} drains one frame per session event"
    );
    let decoded: Vec<ServerPacket> = frames.iter().map(|frame| decode_play(frame)).collect();
    for (packet, event) in decoded.iter().zip(&events) {
        assert_eq!(
            packet,
            &ServerPacket::try_from(event.clone()).unwrap(),
            "{label}'s frames carry the exact publication packets in order"
        );
    }
}

/// The wanted subscription scenario through a real adapter: true-default
/// authority, narrow (declared 2) and wide (declared 8) co-located sessions
/// through the real handshake, Ready (3, 0)/(4, 0) first contact, a late join,
/// and a staged chunk-0 to chunk-1 move with its sorted forget. Returns the
/// tick publications and the complete ordered drain transcript bytes.
fn wanted_subscription_scenario(
    adapter: &mut dyn ParityAdapter,
) -> (Vec<TickPublication>, Vec<Vec<u8>>) {
    replace_with_default_authority(adapter);
    seed_day_world(adapter);
    adapter.stage_login(
        1,
        stored_player(1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    adapter.stage_login(
        2,
        stored_player(2, "Bea", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    let ada = adapter.login_with_view(1, "Ada", 2);
    let bea = adapter.login_with_view(2, "Bea", 8);
    adapter.stage(Box::new(|context| {
        for (x, z) in [(3, 0), (4, 0)] {
            context.preload_ready_chunk(
                ReadyChunk::try_new(chunk_key(x, z), 1, 1, ground_chunk()).unwrap(),
            );
        }
    }));
    let tick_a = adapter.tick();
    let ada_events = session_events(&tick_a, ada.session);
    let bea_events = session_events(&tick_a, bea.session);
    assert!(snapshot_keys(&ada_events).contains(&ChunkPos::new(3, 0)));
    assert!(
        !snapshot_keys(&ada_events).contains(&ChunkPos::new(4, 0)),
        "the narrow session never mirrors (4, 0)"
    );
    assert!(snapshot_keys(&bea_events).contains(&ChunkPos::new(3, 0)));
    assert!(
        snapshot_keys(&bea_events).contains(&ChunkPos::new(4, 0)),
        "the wide session publishes the (4, 0) boundary"
    );
    for event in ada_events.iter().chain(&bea_events) {
        if let Event::ChunkSnapshot(snapshot) = event {
            assert_eq!(snapshot.dimension(), Dimension::OVERWORLD);
            assert_eq!(snapshot.revision(), 1);
        }
    }
    let ada_frames_a = adapter.drain(ada.session);
    let bea_frames_a = adapter.drain(bea.session);
    assert_frames_match_events("narrow first contact", &tick_a, &ada, &ada_frames_a);
    assert_frames_match_events("wide first contact", &tick_a, &bea, &bea_frames_a);

    adapter.stage_login(
        3,
        stored_player(3, "Cleo", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    let cleo = adapter.login_with_view(3, "Cleo", 8);
    let tick_b = adapter.tick();
    assert_eq!(
        snapshot_keys(&session_events(&tick_b, cleo.session)),
        vec![
            ChunkPos::new(0, 0),
            ChunkPos::new(3, 0),
            ChunkPos::new(4, 0),
        ],
        "the late joiner snapshots the shared Ready columns"
    );
    assert!(
        snapshot_keys(&session_events(&tick_b, ada.session)).is_empty(),
        "the existing narrow subscriber sees no duplicate"
    );
    assert!(
        snapshot_keys(&session_events(&tick_b, bea.session)).is_empty(),
        "the existing wide subscriber sees no duplicate"
    );
    let ada_frames_b = adapter.drain(ada.session);
    let bea_frames_b = adapter.drain(bea.session);
    let cleo_frames_b = adapter.drain(cleo.session);
    assert_frames_match_events("narrow steady", &tick_b, &ada, &ada_frames_b);
    assert_frames_match_events("wide steady", &tick_b, &bea, &bea_frames_b);
    assert_frames_match_events("late join", &tick_b, &cleo, &cleo_frames_b);

    let walker = ada.session;
    adapter.stage(Box::new(move |context| {
        let mut actor = context
            .read()
            .actor(ActorKey::Player(walker))
            .cloned()
            .unwrap();
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([16.5, 65.0, 0.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        context.stage(RuleEffect::Actor(actor)).unwrap();
    }));
    let tick_c = adapter.tick();
    let ada_move_events = session_events(&tick_c, ada.session);
    let mut expected: Vec<ChunkPos> = (-3..=3).map(|z| ChunkPos::new(-3, z)).collect();
    expected.sort();
    let forgets = forget_keys(&ada_move_events);
    assert_eq!(
        forgets, expected,
        "the move forgets the exact sorted column"
    );
    assert_eq!(
        snapshot_keys(&ada_move_events),
        vec![ChunkPos::new(4, 0)],
        "the move first-sends only the new boundary"
    );
    assert!(
        snapshot_keys(&session_events(&tick_c, bea.session)).is_empty(),
        "the forget belongs to the walker alone"
    );
    let ada_frames_c = adapter.drain(ada.session);
    let bea_frames_c = adapter.drain(bea.session);
    let cleo_frames_c = adapter.drain(cleo.session);
    assert_frames_match_events("narrow move", &tick_c, &ada, &ada_frames_c);
    assert_frames_match_events("wide steady after move", &tick_c, &bea, &bea_frames_c);
    assert_frames_match_events("late steady after move", &tick_c, &cleo, &cleo_frames_c);

    let mut transcript = Vec::new();
    for frames in [
        ada_frames_a,
        bea_frames_a,
        ada_frames_b,
        bea_frames_b,
        cleo_frames_b,
        ada_frames_c,
        bea_frames_c,
        cleo_frames_c,
    ] {
        transcript.extend(frames);
    }
    (vec![tick_a, tick_b, tick_c], transcript)
}

/// Wanted subscriptions through both real adapters: per-session first
/// contact, late-join isolation, and the staged move/forget transition agree
/// exactly, down to the ordered transcript bytes.
#[test]
fn wanted_subscription_publications_match_across_adapters() {
    let mut memory = MemoryParity::new();
    let (memory_ticks, memory_bytes) = wanted_subscription_scenario(&mut memory);
    let mut tcp = TcpParity::new();
    let (tcp_ticks, tcp_bytes) = wanted_subscription_scenario(&mut tcp);
    assert_eq!(
        memory_ticks, tcp_ticks,
        "wanted subscriptions publish identically across the real adapters"
    );
    assert_eq!(
        memory_bytes, tcp_bytes,
        "wanted subscription transcript bytes are identical across adapters"
    );
}

/// The wanted resync scenario through a real adapter: Ready (3, 0)/(4, 0),
/// a new (9, 0) first send, nonwanted Ready (10, 0), and unready (6, 0);
/// real RequestChunkResync ingress with watermark and stale handling; then
/// (6, 0) becomes Ready and first-sends once. Returns the publications and
/// the complete ordered drain transcript bytes.
fn wanted_resync_scenario(adapter: &mut dyn ParityAdapter) -> (Vec<TickPublication>, Vec<Vec<u8>>) {
    replace_with_default_authority(adapter);
    seed_day_world(adapter);
    adapter.stage_login(
        1,
        stored_player(1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    adapter.stage_login(
        2,
        stored_player(2, "Bo", [0.5, 65.0, 0.5], 0.0, 0.0, |_| {}),
    );
    let ada = adapter.login_with_view(1, "Ada", 8);
    let bo = adapter.login_with_view(2, "Bo", 2);
    let first = adapter.tick();
    assert_eq!(
        snapshot_keys(&session_events(&first, ada.session)),
        vec![ChunkPos::new(0, 0)],
    );
    let ada_frames_first = adapter.drain(ada.session);
    let bo_frames_first = adapter.drain(bo.session);
    assert_frames_match_events("wide first contact", &first, &ada, &ada_frames_first);
    assert_frames_match_events("narrow first contact", &first, &bo, &bo_frames_first);

    adapter.stage(Box::new(|context| {
        for (x, z) in [(3, 0), (4, 0), (9, 0), (10, 0)] {
            context.preload_ready_chunk(
                ReadyChunk::try_new(chunk_key(x, z), 1, 1, ground_chunk()).unwrap(),
            );
        }
    }));
    let resync = |adapter: &mut dyn ParityAdapter,
                  conn: ConnectionId,
                  sequence: u64,
                  x: i32,
                  z: i32,
                  have: u64| {
        adapter.send_packet(
            conn,
            &ClientPacket::RequestChunkResync(RequestChunkResync::new(
                sequence,
                Dimension::OVERWORLD,
                x,
                z,
                have,
            )),
        );
    };
    resync(adapter, ada.conn, 1, 4, 0, 999);
    resync(adapter, ada.conn, 2, 10, 0, 1);
    resync(adapter, ada.conn, 3, 6, 0, 1);
    resync(adapter, bo.conn, 1, 4, 0, 1);
    let tick_b = adapter.tick();
    assert_eq!(tick_counters(&tick_b), (4, 0, 0));
    assert_eq!(
        adapter
            .endpoint()
            .authority
            .session(ada.session)
            .unwrap()
            .last_applied_sequence,
        3
    );
    assert_eq!(
        adapter
            .endpoint()
            .authority
            .session(bo.session)
            .unwrap()
            .last_applied_sequence,
        1
    );
    let ada_events = session_events(&tick_b, ada.session);
    assert_eq!(
        snapshot_keys(&ada_events),
        vec![
            ChunkPos::new(4, 0),
            ChunkPos::new(3, 0),
            ChunkPos::new(9, 0),
        ],
        "the wanted Ready resync answers before ordinary first sends"
    );
    assert_eq!(
        snapshot_revisions(&ada_events),
        vec![
            (ChunkPos::new(4, 0), 1),
            (ChunkPos::new(3, 0), 1),
            (ChunkPos::new(9, 0), 1),
        ],
    );
    assert!(
        !snapshot_keys(&ada_events).contains(&ChunkPos::new(10, 0)),
        "nonwanted Ready (10, 0) stays silent"
    );
    assert!(
        !snapshot_keys(&ada_events).contains(&ChunkPos::new(6, 0)),
        "unready (6, 0) stays silent"
    );
    assert!(
        snapshot_keys(&session_events(&tick_b, bo.session)).is_empty()
            || !snapshot_keys(&session_events(&tick_b, bo.session)).contains(&ChunkPos::new(4, 0)),
        "the narrow session never mirrors (4, 0)"
    );
    let ada_frames_b = adapter.drain(ada.session);
    let bo_frames_b = adapter.drain(bo.session);
    assert_frames_match_events("wide resync", &tick_b, &ada, &ada_frames_b);
    assert_frames_match_events("narrow resync silence", &tick_b, &bo, &bo_frames_b);

    resync(adapter, ada.conn, 3, 0, 0, 1);
    let tick_c = adapter.tick();
    assert_eq!(tick_counters(&tick_c), (1, 0, 1));
    assert_eq!(
        adapter
            .endpoint()
            .authority
            .session(ada.session)
            .unwrap()
            .last_applied_sequence,
        3,
        "the repeated sequence stays stale"
    );
    assert!(snapshot_keys(&session_events(&tick_c, ada.session)).is_empty());
    let ada_frames_c = adapter.drain(ada.session);
    let bo_frames_c = adapter.drain(bo.session);
    assert_frames_match_events("wide stale resync", &tick_c, &ada, &ada_frames_c);
    assert_frames_match_events("narrow stale steady", &tick_c, &bo, &bo_frames_c);

    adapter.stage(Box::new(|context| {
        context.preload_ready_chunk(
            ReadyChunk::try_new(chunk_key(6, 0), 1, 1, ground_chunk()).unwrap(),
        );
    }));
    resync(adapter, ada.conn, 4, 6, 0, 1);
    let tick_d = adapter.tick();
    assert_eq!(
        snapshot_keys(&session_events(&tick_d, ada.session)),
        vec![ChunkPos::new(6, 0)],
        "the newly Ready column first-sends once"
    );
    let ada_frames_d = adapter.drain(ada.session);
    let bo_frames_d = adapter.drain(bo.session);
    assert_frames_match_events("wide newly ready", &tick_d, &ada, &ada_frames_d);
    assert_frames_match_events("narrow newly ready silence", &tick_d, &bo, &bo_frames_d);

    let mut transcript = Vec::new();
    for frames in [
        ada_frames_first,
        bo_frames_first,
        ada_frames_b,
        bo_frames_b,
        ada_frames_c,
        bo_frames_c,
        ada_frames_d,
        bo_frames_d,
    ] {
        transcript.extend(frames);
    }
    (vec![first, tick_b, tick_c, tick_d], transcript)
}

/// Wanted resyncs through both real adapters: identical publications,
/// identical watermarks, and identical ordered transcript bytes.
#[test]
fn wanted_resync_publications_match_across_adapters() {
    let mut memory = MemoryParity::new();
    let (memory_ticks, memory_bytes) = wanted_resync_scenario(&mut memory);
    let mut tcp = TcpParity::new();
    let (tcp_ticks, tcp_bytes) = wanted_resync_scenario(&mut tcp);
    assert_eq!(
        memory_ticks, tcp_ticks,
        "wanted resyncs publish identically across the real adapters"
    );
    assert_eq!(
        memory_bytes, tcp_bytes,
        "wanted resync transcript bytes are identical across adapters"
    );
}
