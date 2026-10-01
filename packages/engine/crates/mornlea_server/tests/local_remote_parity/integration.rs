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
use std::time::{Duration, Instant};

use mornlea_domain::{Identities, PlayerId};
use mornlea_protocol::{
    AdmittedLogin, ClientHello, ClientPacket, CloseContainer, LoginStart, LoginSuccess, PlayIntent,
    ProtocolCodec, ServerHello, ServerPacket, State, encode_uvarint, read_frame_ref, write_frame,
};
use mornlea_server::contracts::{
    Clock, CloseReason, CompanionActionEnvelope, CompanionReceipt, ConnectionId,
    ConnectionProgress, Deadline, LoadPoll, LoginPoll, LoginTicket, PlayerLoadPort,
    PublicationPort, ServerEndpoint, ServerError, ServerLimits, SessionKey, SessionPhase,
    ShutdownFailure, SubmissionReceipt, TickBudget, TickPublication, TransportKind,
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
