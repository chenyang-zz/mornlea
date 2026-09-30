//! Loopback TCP adapter cases over the shared transport admission core.
//!
//! These cases drive the same connection core the Memory adapter uses, but
//! through real loopback sockets: a nonblocking server side owned by the
//! adapter and blocking client sockets with generous read timeouts. Protocol
//! time comes only from the injected step clock; bounded wall-clock waits
//! allow kernel delivery without advancing protocol deadlines. The expected values
//! (prelogin ceiling 16, hello 5 s, login 10 s, the reject vocabulary, the
//! 512-frame slow-receiver retirement, and the exact session numbering) are
//! the frozen rows the shared core pins, mirrored here through the socket
//! layer. The transcript case pins the session IDs and event order the
//! sibling adapter must reproduce for the same inputs.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use mornlea_domain::{
    CommandRejection, CompanionId, Event, EventRecipient, Identities, PlayerId, RejectReason,
    RoutedEvent,
};
use mornlea_protocol::{
    CloseContainer, LoginStart, LoginSuccess, ProtocolCodec, ProtocolError, ServerHello,
    ServerPacket, encode_uvarint, read_frame_ref, write_frame,
};
use mornlea_server::contracts::{
    ActorPersistence, AgentHandle, AgentPoll, AgentRequest, AgentRequestId, Clock, CloseReason,
    CompanionActionEnvelope, CompanionReceipt, ConnectionId, ConnectionProgress, Deadline,
    FinalReducer, FlushReport, FrozenLease, LoadPoll, LoginPoll, LoginTicket, McpLifecycle,
    MemoryFinalizationReport, MemoryFinalizer, NamespaceId, Operation, PlanningSnapshot,
    PlayerLoadPort, PublicationPort, SaveAuthority, SaveBudget, SaveKey, SavePoll, SaveRequest,
    SaveScheduleReport, SaveStats, SaveTicket, ServerEndpoint, ServerError, ServerLimits,
    ServerPhase, SessionKey, SessionPhase, ShutdownFailure, ShutdownReport, SnapshotId,
    SnapshotPort, SnapshotRegistration, StoreHandle, SubmissionReceipt, SubmitSaveError,
    TickBudget, TickCounters, TickPublication, TransportKind, WorkerLifecycle,
};
use mornlea_server::state::AuthorityState;
use mornlea_server::transport::common::TransportAuthority;
use mornlea_server::transport::tcp::TcpTransport;
use mornlea_storage::StoredPlayer;

fn protocol() -> u32 {
    Identities::current().protocol
}
const WORLD_SEED: i64 = 7;
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Injected monotonic clock. Protocol time advances only explicitly.
struct StepClock {
    now: RefCell<Instant>,
    unix_ms: i64,
}

impl StepClock {
    fn new(base: Instant) -> Self {
        Self {
            now: RefCell::new(base),
            unix_ms: 1_000,
        }
    }

    fn advance(&self, by: Duration) {
        *self.now.borrow_mut() += by;
    }
}

impl Clock for StepClock {
    fn monotonic(&self) -> Instant {
        *self.now.borrow()
    }

    fn unix_ms(&self) -> i64 {
        self.unix_ms
    }
}

/// One scripted load outcome per started ticket.
enum LoadOutcome {
    Pending,
    Loaded(Box<Option<StoredPlayer>>),
}

/// Executing load double: every started ticket resolves to the next scripted
/// outcome, or to a canonical new player when the script is empty.
#[derive(Default)]
struct ScriptedLoad {
    next_ticket: u64,
    script: VecDeque<LoadOutcome>,
    entries: BTreeMap<u64, LoadOutcome>,
    started: usize,
    cancelled: Vec<u64>,
    last: Option<LoginTicket>,
}

impl ScriptedLoad {
    fn push(&mut self, outcome: LoadOutcome) {
        self.script.push_back(outcome);
    }
}

impl PlayerLoadPort for ScriptedLoad {
    fn start(
        &mut self,
        _player: PlayerId,
        _deadline: Deadline,
    ) -> Result<LoginTicket, ServerError> {
        self.next_ticket += 1;
        let ticket = LoginTicket::try_from_raw(self.next_ticket).expect("load tickets are nonzero");
        let outcome = self
            .script
            .pop_front()
            .unwrap_or(LoadOutcome::Loaded(Box::new(None)));
        self.entries.insert(ticket.get(), outcome);
        self.started += 1;
        self.last = Some(ticket);
        Ok(ticket)
    }

    fn poll(&mut self, ticket: LoginTicket) -> LoadPoll {
        match self.entries.get(&ticket.get()) {
            Some(LoadOutcome::Pending) | None => LoadPoll::Pending,
            Some(LoadOutcome::Loaded(stored)) => LoadPoll::Loaded((**stored).clone()),
        }
    }

    fn cancel(&mut self, ticket: LoginTicket) -> Result<(), ServerError> {
        self.entries.remove(&ticket.get());
        self.cancelled.push(ticket.get());
        Ok(())
    }
}

/// Login bookkeeping the double keeps beside the authority state.
struct TicketRecord {
    session: SessionKey,
    player: PlayerId,
    committed: bool,
}

/// Executing transport-authority double: the frozen session, publication, and
/// endpoint seams over one real authority state, plus the scripted load port.
struct TcpTestEndpoint {
    authority: AuthorityState,
    loads: ScriptedLoad,
    tickets: BTreeMap<u64, TicketRecord>,
    prepares: usize,
    installs: usize,
    activates: usize,
    commits: Vec<u64>,
    cancels: Vec<u64>,
    closes: Vec<SessionKey>,
    submits: Vec<SubmissionReceipt>,
}

impl TcpTestEndpoint {
    fn new() -> Self {
        let limits = ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap();
        Self {
            authority: AuthorityState::try_new(limits, WORLD_SEED).unwrap(),
            loads: ScriptedLoad::default(),
            tickets: BTreeMap::new(),
            prepares: 0,
            installs: 0,
            activates: 0,
            commits: Vec::new(),
            cancels: Vec::new(),
            closes: Vec::new(),
            submits: Vec::new(),
        }
    }

    fn session_phase(&self, ticket: LoginTicket) -> SessionPhase {
        self.authority
            .session(self.tickets[&ticket.get()].session)
            .map(|facts| facts.phase)
            .expect("ticket session stays registered after retire")
    }

    fn session_of(&self, ticket: LoginTicket) -> SessionKey {
        self.tickets[&ticket.get()].session
    }

    fn last_ticket(&self) -> LoginTicket {
        self.loads.last.expect("login started")
    }
}

impl ServerEndpoint for TcpTestEndpoint {
    fn admit(
        &mut self,
        login: mornlea_protocol::AdmittedLogin,
        transport: TransportKind,
    ) -> Result<SessionKey, ServerError> {
        self.authority.admit(login, transport)
    }

    fn submit(
        &mut self,
        session: SessionKey,
        intent: mornlea_protocol::PlayIntent,
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

    fn shutdown(&mut self, deadline: Deadline) -> Result<ShutdownReport, ShutdownFailure> {
        let mut reducer = TickReducer;
        let mut store = IdleStore;
        let mut agent = IdleAgent;
        let mut snapshots = IdleSnapshots;
        let mut workers = IdleWorkers;
        let mut persistence = IdlePersistence;
        let mut mcp = IdleMcp;
        let mut memory = IdleMemory;
        let clock = ShutdownClock {
            instant: deadline.instant(),
        };
        let mut io = mornlea_server::state::ShutdownIo {
            reducer: &mut reducer,
            store: &mut store,
            agent: &mut agent,
            snapshots: &mut snapshots,
            clock: &clock,
            workers: &mut workers,
            persistence: &mut persistence,
            mcp: &mut mcp,
            memory: &mut memory,
        };
        self.authority.drive_shutdown(deadline, &mut io)
    }
}

impl PublicationPort for TcpTestEndpoint {
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

impl TransportAuthority for TcpTestEndpoint {
    fn begin_login(
        &mut self,
        login: mornlea_protocol::AdmittedLogin,
        kind: TransportKind,
        deadline: Deadline,
    ) -> Result<LoginTicket, ServerError> {
        let session = self.authority.prepare(login.clone(), kind)?;
        self.prepares += 1;
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
                self.installs += 1;
                LoginPoll::Ready {
                    session: record.session,
                    success: ServerPacket::LoginSuccess(LoginSuccess::new(
                        record.player,
                        self.world_seed() as u64,
                    )),
                }
            }
            LoadPoll::Failed(error) => {
                self.loads.cancel(ticket).unwrap();
                let record = &self.tickets[&ticket.get()];
                self.authority
                    .retire(record.session, CloseReason::PeerGone)
                    .unwrap();
                let reject = match error {
                    ServerError::Io {
                        kind: std::io::ErrorKind::InvalidData,
                        ..
                    } => mornlea_protocol::LOGIN_PLAYER_DATA_CORRUPT,
                    _ => mornlea_protocol::LOGIN_STORE_UNAVAILABLE,
                };
                LoginPoll::Failed { error, reject }
            }
        }
    }

    fn commit_login(&mut self, ticket: LoginTicket) -> Result<SessionKey, ServerError> {
        let record = self.tickets.get_mut(&ticket.get()).expect("known ticket");
        self.authority.activate(record.session)?;
        record.committed = true;
        self.activates += 1;
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

struct TickReducer;

impl FinalReducer for TickReducer {
    fn reduce_final(&mut self, authority: &mut AuthorityState) -> Result<u64, ServerError> {
        Ok(authority.next_tick())
    }
}

struct ShutdownClock {
    instant: Instant,
}

impl Clock for ShutdownClock {
    fn monotonic(&self) -> Instant {
        self.instant
    }

    fn unix_ms(&self) -> i64 {
        0
    }
}

/// Minimal store double: shutdown is not part of these cases, so every port
/// answers with an immediate success and no durable work.
struct IdleStore;

impl StoreHandle for IdleStore {
    fn submit(&mut self, request: SaveRequest) -> Result<SaveTicket, SubmitSaveError> {
        match SaveTicket::try_from_raw(1) {
            Ok(ticket) => Ok(ticket),
            Err(error) => Err(SubmitSaveError { error, request }),
        }
    }

    fn poll(&mut self, _ticket: SaveTicket) -> SavePoll {
        SavePoll::Pending
    }

    fn poll_tick(
        &mut self,
        _tick: u64,
        _budget: SaveBudget,
        _authority: &mut dyn SaveAuthority,
    ) -> Result<SaveScheduleReport, ServerError> {
        Ok(SaveScheduleReport {
            urgent: 0,
            autosave: 0,
            retry: 0,
            stats: SaveStats::default(),
            backpressured: false,
        })
    }

    fn cancel_pending(
        &mut self,
    ) -> Result<Vec<mornlea_server::contracts::OwnedSnapshot>, ServerError> {
        Ok(Vec::new())
    }

    fn flush(
        &mut self,
        _deadline: Deadline,
        _authority: &mut dyn SaveAuthority,
        _clock: &dyn Clock,
    ) -> Result<FlushReport, ServerError> {
        Ok(FlushReport::default())
    }

    fn sync(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }

    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}

struct IdleAgent;

impl AgentHandle for IdleAgent {
    fn submit(&mut self, _request: AgentRequest) -> Result<AgentRequestId, ServerError> {
        let mut bytes = [0u8; 16];
        bytes[0] = 1;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        AgentRequestId::try_from_bytes(bytes)
    }

    fn poll(&mut self, _id: AgentRequestId) -> AgentPoll {
        AgentPoll::Pending
    }

    fn cancel(&mut self, _id: AgentRequestId, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }

    fn freeze(&mut self, _clock: &dyn Clock) -> Option<FrozenLease> {
        None
    }

    fn release(&mut self, _lease: &FrozenLease, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }

    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}

struct IdleSnapshots;

impl SnapshotPort for IdleSnapshots {
    fn register(
        &mut self,
        _namespace: NamespaceId,
        _companion: CompanionId,
        _generation: u64,
        _snapshot: PlanningSnapshot,
        _deadline: Deadline,
    ) -> Result<SnapshotRegistration, ServerError> {
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closed,
        })
    }

    fn complete(&mut self, _id: SnapshotId) -> Result<(), ServerError> {
        Ok(())
    }

    fn cancel(&mut self, _id: SnapshotId) -> Result<(), ServerError> {
        Ok(())
    }

    fn close(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
}

struct IdleWorkers;

impl WorkerLifecycle for IdleWorkers {
    fn stop_new(&mut self) -> Result<(), ServerError> {
        Ok(())
    }

    fn cancel(&mut self) -> Result<(), ServerError> {
        Ok(())
    }

    fn wait(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }

    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}

struct IdlePersistence;

impl ActorPersistence for IdlePersistence {
    fn flush(&mut self, _family: SaveKey, _deadline: Deadline) -> Result<FlushReport, ServerError> {
        Ok(FlushReport::default())
    }
}

struct IdleMcp;

impl McpLifecycle for IdleMcp {
    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}

struct IdleMemory;

impl MemoryFinalizer for IdleMemory {
    fn begin_attempt(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }

    fn drain(&mut self, _deadline: Deadline) -> Result<MemoryFinalizationReport, ServerError> {
        Ok(MemoryFinalizationReport::default())
    }
}

fn player(tag: u8) -> PlayerId {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    PlayerId::try_from_bytes(bytes).unwrap()
}

/// One hello frame carrying the given protocol version.
fn hello_frame(version: u32) -> Vec<u8> {
    write_frame(0, &encode_uvarint(version)).unwrap()
}

/// One login-start frame for a canonical player.
fn login_start_frame(tag: u8, name: &str) -> Vec<u8> {
    let start = LoginStart::new(player(tag), name, 8).unwrap();
    write_frame(0, &start.encode().unwrap()).unwrap()
}

/// One sequenced play frame (CloseContainer).
fn play_frame(sequence: u64) -> Vec<u8> {
    let record = CloseContainer::new(sequence);
    write_frame(CloseContainer::PACKET_ID, &record.encode().unwrap()).unwrap()
}

/// Independently encodes one server control frame for byte comparison.
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

/// Packet identity of the rejection envelope, from the serving loop's own
/// publication record: the outbox drain carries payloads alone, so the
/// loop supplies the identity the adapter frames with.
fn rejection_packet_id() -> u32 {
    ServerPacket::try_from(Event::CommandRejected(CommandRejection::new(
        0,
        RejectReason::InvalidRay,
    )))
    .expect("rejection converts")
    .key()
    .id
}

/// Blocking test client over loopback. Reads carry a generous timeout so a
/// stuck server fails the case instead of hanging the suite; the timeout
/// never drives protocol behavior.
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
    /// Panics on timeout: the server owed a frame here.
    fn next_frame(&mut self) -> Vec<u8> {
        self.next_frame_opt().expect("server owed one frame here")
    }

    /// Reads one envelope, or returns `None` on orderly end of stream.
    fn next_frame_opt(&mut self) -> Option<Vec<u8>> {
        loop {
            match read_frame_ref(&self.buf) {
                Ok(frame) => {
                    let envelope = self.buf[..frame.consumed].to_vec();
                    self.buf.drain(..frame.consumed);
                    return Some(envelope);
                }
                Err(ProtocolError::Truncated) => {
                    let mut chunk = [0u8; 4096];
                    match self.stream.read(&mut chunk) {
                        Ok(0) => {
                            assert!(
                                self.buf.is_empty(),
                                "stream ended mid-frame: peer broke framing"
                            );
                            return None;
                        }
                        Ok(read) => self.buf.extend_from_slice(&chunk[..read]),
                        Err(error) => panic!("loopback read failed: {error:?}"),
                    }
                }
                Err(error) => panic!("server sent a malformed frame: {error:?}"),
            }
        }
    }
}

/// One case harness: a loopback adapter, the executing endpoint double, and
/// the injected clock.
struct Harness {
    server: TcpTransport,
    endpoint: TcpTestEndpoint,
    clock: StepClock,
}

impl Harness {
    fn new() -> Self {
        Self {
            server: TcpTransport::bind_loopback().expect("loopback bind succeeds"),
            endpoint: TcpTestEndpoint::new(),
            clock: StepClock::new(Instant::now()),
        }
    }

    fn addr(&self) -> SocketAddr {
        self.server.local_addr().expect("listener has an address")
    }
}

/// Gives kernel delivery a scheduling turn without changing protocol time.
fn delivery_turn(deadline: Instant) {
    std::thread::sleep(
        deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(1)),
    );
}

/// Accepts the next pending loopback connection within a delivery deadline.
fn accept_next(harness: &mut Harness) -> ConnectionId {
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

fn advanced_frames(progress: ConnectionProgress) -> usize {
    match progress {
        ConnectionProgress::Advanced { frames } => frames,
        ConnectionProgress::AwaitMore => 0,
        other => panic!("expected progress, got closed: {other:?}"),
    }
}

/// Waits for kernel delivery, independently of injected protocol deadlines.
fn frames_until(
    harness: &mut Harness,
    id: ConnectionId,
    want: usize,
    deadline: Instant,
) -> Option<usize> {
    let mut total = 0;
    while Instant::now() < deadline {
        match harness
            .server
            .pump_in(id, &mut harness.endpoint, &harness.clock)
        {
            ConnectionProgress::Advanced { frames } => {
                total += frames;
                if total >= want {
                    return Some(total);
                }
            }
            ConnectionProgress::AwaitMore => delivery_turn(deadline),
            closed => panic!("connection closed while waiting for frames: {closed:?}"),
        }
    }
    None
}

fn spin_frames(harness: &mut Harness, id: ConnectionId, want: usize) -> usize {
    frames_until(harness, id, want, Instant::now() + DELIVERY_TIMEOUT)
        .unwrap_or_else(|| panic!("delivery timed out waiting for {want} frames"))
}

#[test]
fn absent_delivery_expires_without_advancing_protocol_clock() {
    let mut harness = Harness::new();
    let _client = ClientConn::connect(harness.addr());
    let id = accept_next(&mut harness);
    let protocol_time = harness.clock.monotonic();
    let began = Instant::now();
    assert_eq!(
        frames_until(&mut harness, id, 1, began + Duration::from_millis(20)),
        None
    );
    assert!(began.elapsed() >= Duration::from_millis(20));
    assert!(began.elapsed() < Duration::from_secs(1));
    assert_eq!(harness.clock.monotonic(), protocol_time);
    assert_eq!(harness.server.retained_sockets(), 1);
}

/// Spins until the retained inbound bytes reach exactly `want`, proving a
/// partial frame arrived before it could complete.
fn spin_retained(harness: &mut Harness, id: ConnectionId, want: usize) {
    let deadline = Instant::now() + DELIVERY_TIMEOUT;
    while Instant::now() < deadline {
        let _ = harness
            .server
            .pump_in(id, &mut harness.endpoint, &harness.clock);
        if harness.server.retained_len(id) == Some(want) {
            return;
        }
        delivery_turn(deadline);
    }
    panic!("ingress spin exhausted waiting for {want} retained bytes");
}

fn expect_closed(progress: ConnectionProgress) -> (CloseReason, Option<ServerError>) {
    match progress {
        ConnectionProgress::Closed { reason, class } => (reason, class),
        other => panic!("expected a closed connection, got {other:?}"),
    }
}

/// Drives one loopback client from connect through the acknowledged success
/// handoff, checking every wire byte against an independent encoding.
fn login(
    harness: &mut Harness,
    tag: u8,
    name: &str,
) -> (ConnectionId, LoginTicket, ClientConn, SessionKey) {
    let mut client = ClientConn::connect(harness.addr());
    let id = accept_next(harness);
    client.send(&hello_frame(protocol()));
    assert_eq!(
        spin_frames(&mut *harness, id, 1),
        1,
        "hello processes exactly one frame"
    );
    let (sent, _) = harness.server.flush_out(id, &mut harness.endpoint);
    assert_eq!(sent, 1, "hello answer flushes exactly one frame");
    let expected_hello = ServerPacket::ServerHello(ServerHello::new(protocol()).unwrap());
    assert_eq!(client.next_frame(), control_frame(&expected_hello));
    assert!(
        client.buf.is_empty(),
        "no second frame hides behind the hello answer"
    );

    client.send(&login_start_frame(tag, name));
    assert_eq!(
        spin_frames(&mut *harness, id, 1),
        1,
        "login start processes exactly one frame"
    );
    let ticket = harness.endpoint.last_ticket();
    assert_eq!(
        advanced_frames(
            harness
                .server
                .poll(id, &mut harness.endpoint, &harness.clock)
        ),
        1,
        "ready load queues the success frame"
    );
    let (sent, _) = harness.server.flush_out(id, &mut harness.endpoint);
    assert_eq!(sent, 1, "success flushes exactly once");
    let expected_success =
        ServerPacket::LoginSuccess(LoginSuccess::new(player(tag), WORLD_SEED as u64));
    assert_eq!(client.next_frame(), control_frame(&expected_success));
    assert!(
        client.buf.is_empty(),
        "no second frame hides behind the success frame"
    );
    let session = harness.endpoint.session_of(ticket);
    (id, ticket, client, session)
}

fn spin_closed(harness: &mut Harness, id: ConnectionId) -> ConnectionProgress {
    let deadline = Instant::now() + DELIVERY_TIMEOUT;
    while Instant::now() < deadline {
        let progress = harness
            .server
            .pump_in(id, &mut harness.endpoint, &harness.clock);
        if matches!(progress, ConnectionProgress::Closed { .. }) {
            return progress;
        }
        delivery_turn(deadline);
    }
    panic!("ingress did not observe terminal evidence");
}

#[test]
fn terminal_eof_releases_socket() {
    let mut harness = Harness::new();
    let mut client = ClientConn::connect(harness.addr());
    let id = accept_next(&mut harness);
    let mut partial = hello_frame(protocol());
    partial.pop();
    client.send(&partial);
    spin_retained(&mut harness, id, partial.len());
    client.stream.shutdown(std::net::Shutdown::Write).unwrap();
    let progress = spin_closed(&mut harness, id);
    assert_eq!(harness.server.retained_sockets(), 0);
    assert_eq!(harness.server.retained_len(id), Some(0));
    assert_eq!(
        harness
            .server
            .poll(id, &mut harness.endpoint, &harness.clock),
        progress
    );
    assert!(client.next_frame_opt().is_none());
}

#[test]
fn terminal_rejection_flushes_then_releases_socket() {
    let mut harness = Harness::new();
    let mut client = ClientConn::connect(harness.addr());
    let id = accept_next(&mut harness);
    client.send(&hello_frame(protocol() - 1));
    let progress = spin_closed(&mut harness, id);
    assert_eq!(harness.server.retained_sockets(), 0);
    let expected = mornlea_protocol::HandshakeReject::new(
        protocol(),
        mornlea_protocol::HANDSHAKE_VERSION_MISMATCH,
        "协议版本不匹配",
    )
    .unwrap();
    assert_eq!(
        client.next_frame(),
        control_frame(&ServerPacket::HandshakeReject(expected))
    );
    assert!(client.next_frame_opt().is_none());
    assert_eq!(
        harness
            .server
            .poll(id, &mut harness.endpoint, &harness.clock),
        progress
    );
    assert_eq!(harness.endpoint.prepares, 0);
}

#[test]
fn terminal_poll_expiry_releases_socket() {
    let mut harness = Harness::new();
    let mut client = ClientConn::connect(harness.addr());
    let id = accept_next(&mut harness);
    assert_eq!(harness.server.retained_sockets(), 1);
    harness.clock.advance(Duration::from_secs(5));
    let progress = harness
        .server
        .poll(id, &mut harness.endpoint, &harness.clock);
    expect_closed(progress);
    assert_eq!(harness.server.retained_sockets(), 0);
    assert!(client.next_frame_opt().is_none());
    assert_eq!(
        harness
            .server
            .poll(id, &mut harness.endpoint, &harness.clock),
        progress
    );
}

#[test]
fn fragmented_frame_reassembled() {
    let mut harness = Harness::new();
    let mut client = ClientConn::connect(harness.addr());
    let id = accept_next(&mut harness);

    // One hello frame split across two socket writes assembles exactly once.
    // The first half arrives and is retained without completing; only the
    // second half completes the single frame, and the peer observes a
    // single byte-exact hello answer.
    let hello = hello_frame(protocol());
    let split = hello.len() / 2;
    client.send(&hello[..split]);
    spin_retained(&mut harness, id, split);
    client.send(&hello[split..]);
    assert_eq!(
        spin_frames(&mut harness, id, 1),
        1,
        "split frame assembles exactly once"
    );

    let (sent, _) = harness.server.flush_out(id, &mut harness.endpoint);
    assert_eq!(sent, 1, "one hello answer flushes");
    let expected = ServerPacket::ServerHello(ServerHello::new(protocol()).unwrap());
    assert_eq!(client.next_frame(), control_frame(&expected));
    assert!(client.buf.is_empty());

    // The reassembled connection stays fully usable: login completes on it.
    client.send(&login_start_frame(1, "Ada"));
    assert_eq!(spin_frames(&mut harness, id, 1), 1);
    assert_eq!(
        advanced_frames(
            harness
                .server
                .poll(id, &mut harness.endpoint, &harness.clock)
        ),
        1
    );
    let (sent, _) = harness.server.flush_out(id, &mut harness.endpoint);
    assert_eq!(sent, 1);
    let success = ServerPacket::LoginSuccess(LoginSuccess::new(player(1), WORLD_SEED as u64));
    assert_eq!(client.next_frame(), control_frame(&success));
}

#[test]
fn hello_login_timeouts() {
    // Hello expiry: five seconds without a hello closes the reservation
    // silently, replays the same close, and frees the slot for the next peer.
    let mut harness = Harness::new();
    let waiting = ClientConn::connect(harness.addr());
    let idle = accept_next(&mut harness);
    harness.clock.advance(Duration::from_secs(5));
    let (reason, class) = expect_closed(harness.server.poll(
        idle,
        &mut harness.endpoint,
        &harness.clock,
    ));
    assert_eq!(reason, CloseReason::PeerGone);
    assert_eq!(
        class,
        Some(ServerError::Timeout {
            operation: Operation::Transport
        })
    );
    expect_closed(
        harness
            .server
            .poll(idle, &mut harness.endpoint, &harness.clock),
    );
    assert_eq!(
        harness.endpoint.prepares, 0,
        "no session work precedes hello"
    );
    drop(waiting);

    // The freed reservation admits a full login on a fresh connection.
    let (id, _ticket, mut client, _session) = login(&mut harness, 1, "Ada");
    client.send(&play_frame(5));
    assert_eq!(spin_frames(&mut harness, id, 1), 1);
    assert_eq!(
        harness.endpoint.submits,
        vec![SubmissionReceipt::QueuedForTick {
            tick: 0,
            arrival_index: 0
        }]
    );

    // Login expiry: ten seconds with the player load still pending cancels
    // the ticket and retires the prepared slot before activation.
    let mut harness = Harness::new();
    let mut client = ClientConn::connect(harness.addr());
    let id = accept_next(&mut harness);
    client.send(&hello_frame(protocol()));
    assert_eq!(spin_frames(&mut harness, id, 1), 1);
    let (sent, _) = harness.server.flush_out(id, &mut harness.endpoint);
    assert_eq!(sent, 1);
    assert_eq!(
        client.next_frame(),
        control_frame(&ServerPacket::ServerHello(
            ServerHello::new(protocol()).unwrap()
        ))
    );
    harness.endpoint.loads.push(LoadOutcome::Pending);
    client.send(&login_start_frame(2, "Bea"));
    assert_eq!(spin_frames(&mut harness, id, 1), 1);
    let ticket = harness.endpoint.last_ticket();
    harness.clock.advance(Duration::from_secs(10));
    expect_closed(
        harness
            .server
            .poll(id, &mut harness.endpoint, &harness.clock),
    );
    assert_eq!(harness.endpoint.cancels, vec![ticket.get()]);
    assert_eq!(
        harness.endpoint.session_phase(ticket),
        SessionPhase::Retired
    );
    assert_eq!(harness.endpoint.activates, 0);
}

#[test]
fn peer_reset_recovers() {
    let mut harness = Harness::new();
    let mut client = ClientConn::connect(harness.addr());
    let id = accept_next(&mut harness);

    // Drive to the queued-but-unacknowledged success handoff, then drop the
    // peer without reading: the unread hello answer makes the close abrupt,
    // and the server must observe it on read. The success frame is never
    // flushed, so no acknowledgment can commit the handoff first.
    client.send(&hello_frame(protocol()));
    assert_eq!(spin_frames(&mut harness, id, 1), 1);
    let (sent, _) = harness.server.flush_out(id, &mut harness.endpoint);
    assert_eq!(sent, 1);
    client.send(&login_start_frame(1, "Ada"));
    assert_eq!(spin_frames(&mut harness, id, 1), 1);
    let ticket = harness.endpoint.last_ticket();
    assert_eq!(
        advanced_frames(
            harness
                .server
                .poll(id, &mut harness.endpoint, &harness.clock)
        ),
        1
    );
    drop(client);

    let (reason, _class) = expect_closed(spin_closed(&mut harness, id));
    assert_eq!(reason, CloseReason::PeerGone);

    // No half-open state survives: the unacknowledged login is cancelled, its
    // prepared session is retired, and nothing activated.
    assert_eq!(harness.endpoint.cancels, vec![ticket.get()]);
    assert_eq!(
        harness.endpoint.session_phase(ticket),
        SessionPhase::Retired
    );
    assert_eq!(harness.endpoint.activates, 0);

    // Recovery: a fresh peer logs in on the freed reservation, session
    // numbering continues without reuse, and play flows.
    let (id, _ticket, mut client, session) = login(&mut harness, 1, "Ada");
    assert_eq!(
        session.get(),
        2,
        "the retired slot frees its identity but never its number"
    );
    client.send(&play_frame(5));
    assert_eq!(spin_frames(&mut harness, id, 1), 1);
    assert_eq!(
        harness.endpoint.submits,
        vec![SubmissionReceipt::QueuedForTick {
            tick: 0,
            arrival_index: 0
        }]
    );
}

#[test]
fn slow_receiver_isolated() {
    let mut harness = Harness::new();
    // The slow peer completes login, then stops reading. The healthy peer
    // reads everything it is owed.
    let (slow_id, _slow_ticket, mut slow_client, slow_session) = login(&mut harness, 1, "Ada");
    let (healthy_id, _healthy_ticket, mut healthy_client, healthy_session) =
        login(&mut harness, 2, "Bea");

    // Saturating the slow receiver's outbox retires only that receiver. The
    // per-session frame bound is 512, so the 513th event overflows it.
    let mut events = Vec::with_capacity(513);
    for sequence in 0..513u64 {
        events.push(RoutedEvent::new(
            EventRecipient::Session(slow_session.get()),
            Event::CommandRejected(CommandRejection::new(sequence, RejectReason::InvalidRay)),
        ));
    }
    harness
        .endpoint
        .authority
        .publish(TickPublication {
            tick: 0,
            events,
            control: Vec::new(),
            counters: TickCounters::default(),
        })
        .expect("publication fits the frozen encode surface");
    assert_eq!(
        harness
            .endpoint
            .authority
            .session(slow_session)
            .expect("slow session stays registered")
            .phase,
        SessionPhase::Retired,
        "only the saturated receiver retires"
    );
    assert_eq!(
        harness
            .endpoint
            .authority
            .session(healthy_session)
            .expect("healthy session stays registered")
            .phase,
        SessionPhase::Active,
        "publishing never blocks the healthy peer"
    );

    // The held frames stay drainable and reach the slow socket in order.
    // The drain carries payloads alone; the packet identity comes from the
    // serving loop's own publication record.
    let reject_id = rejection_packet_id();
    let forwarded = harness
        .server
        .forward_outbox(
            slow_id,
            slow_session,
            &mut harness.endpoint,
            &vec![reject_id; 512],
            1024,
            1 << 20,
        )
        .expect("outbox drain writes to the live socket");
    assert_eq!(forwarded.len(), 512, "the overflowing frame was dropped");
    // Each send call has a fixed work budget; the serving loop drains the
    // accepted queue over bounded calls before reaping the slow receiver.
    for _ in 0..8 {
        harness.server.flush_out(slow_id, &mut harness.endpoint);
    }

    // Reaping closes the slow connection with the slow-receiver reason while
    // the healthy connection keeps serving.
    let (reason, _class) = expect_closed(
        harness
            .server
            .close_slow_receiver(slow_id, &mut harness.endpoint),
    );
    assert_eq!(reason, CloseReason::SlowReceiver);
    assert!(
        harness.server.pending_control_frames(slow_id).is_empty(),
        "no disconnect frame is appended on the slow-receiver close"
    );

    // End to end: the slow client observes exactly the 512 held frames, then
    // orderly end of stream.
    let mut drained = 0usize;
    while slow_client.next_frame_opt().is_some() {
        drained += 1;
    }
    assert_eq!(drained, 512);

    // The healthy peer is unaffected: play submits and its own outbox drains.
    healthy_client.send(&play_frame(7));
    assert_eq!(spin_frames(&mut harness, healthy_id, 1), 1);
    assert_eq!(
        harness.endpoint.submits,
        vec![SubmissionReceipt::QueuedForTick {
            tick: 0,
            arrival_index: 0
        }]
    );
    harness
        .endpoint
        .authority
        .publish(TickPublication {
            tick: 1,
            events: vec![RoutedEvent::new(
                EventRecipient::Session(healthy_session.get()),
                Event::CommandRejected(CommandRejection::new(9, RejectReason::NoTarget)),
            )],
            control: Vec::new(),
            counters: TickCounters::default(),
        })
        .unwrap();
    let forwarded = harness
        .server
        .forward_outbox(
            healthy_id,
            healthy_session,
            &mut harness.endpoint,
            &[reject_id],
            8,
            1 << 20,
        )
        .unwrap();
    assert_eq!(forwarded.len(), 1);
    assert_eq!(healthy_client.next_frame(), forwarded[0]);
}

#[test]
fn transcript_matches_memory() {
    // One transcript both adapters must serve identically: login, play,
    // close for two players in sequence. Session IDs and event order come
    // from the shared core and authority alone, so the same inputs must
    // produce the same transcript over either transport.
    let mut harness = Harness::new();

    let (ada_id, _ada_ticket, mut ada_client, ada_session) = login(&mut harness, 1, "Ada");
    assert_eq!(ada_session.get(), 1, "first login takes the first number");
    ada_client.send(&play_frame(5));
    assert_eq!(spin_frames(&mut harness, ada_id, 1), 1);
    ada_client.send(&play_frame(6));
    assert_eq!(spin_frames(&mut harness, ada_id, 1), 1);
    harness
        .server
        .close(ada_id, CloseReason::PeerGone, &mut harness.endpoint);
    assert!(
        ada_client.next_frame_opt().is_none(),
        "close ends the peer stream"
    );

    let (bea_id, _bea_ticket, mut bea_client, bea_session) = login(&mut harness, 2, "Bea");
    assert_eq!(bea_session.get(), 2, "numbering continues across logins");
    bea_client.send(&play_frame(1));
    assert_eq!(spin_frames(&mut harness, bea_id, 1), 1);
    harness
        .server
        .close(bea_id, CloseReason::PeerGone, &mut harness.endpoint);
    assert!(bea_client.next_frame_opt().is_none());

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
        ],
        "arrival order follows the input transcript on both transports"
    );
    assert_eq!(
        harness.endpoint.closes,
        vec![ada_session, bea_session],
        "close order follows the transcript"
    );
    assert_eq!(
        harness
            .endpoint
            .authority
            .session(ada_session)
            .expect("closed session stays registered")
            .phase,
        SessionPhase::Retired
    );
    assert_eq!(
        harness
            .endpoint
            .authority
            .session(bea_session)
            .expect("closed session stays registered")
            .phase,
        SessionPhase::Retired
    );
}
