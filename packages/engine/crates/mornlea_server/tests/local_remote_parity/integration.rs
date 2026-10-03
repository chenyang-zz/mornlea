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
/// stored record, so the next poll loads a canonical new player.
#[derive(Default)]
struct ImmediateLoad {
    next_ticket: u64,
    live: BTreeMap<u64, ()>,
    last: Option<LoginTicket>,
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
        let limits = ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap();
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

fn login_packet(tag: u8, name: &str) -> ClientPacket {
    let start = LoginStart::new(player(tag), name, 8).unwrap();
    ClientPacket::LoginStart(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap())
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

fn login_start_frame(tag: u8, name: &str) -> Vec<u8> {
    let start = LoginStart::new(player(tag), name, 8).unwrap();
    write_frame(0, &start.encode().unwrap()).unwrap()
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
