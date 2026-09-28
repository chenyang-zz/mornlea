//! Common transport admission cases shared by the Memory and TCP adapters.
//!
//! These cases drive the shared connection core over the frozen ports: a real
//! `AuthorityState` behind the session and publication seams, an executing
//! scripted `PlayerLoadPort` double, and an injected monotonic clock. The
//! expected values (prelogin ceiling 16, hello 5 s, login 10 s, the reject
//! vocabulary, and the control-packet payloads) mirror the Go sources
//! `packages/shared/network/login.go`, `packages/server/server/host.go`, and
//! `packages/server/server/host_login.go`, plus the Go login tests in
//! `packages/shared/network` and `packages/server/server`.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use mornlea_domain::{CompanionId, Identities, PlayerId};
use mornlea_protocol::{
    CloseContainer, HANDSHAKE_VERSION_MISMATCH, LOGIN_PLAYER_DATA_CORRUPT, LOGIN_STORE_UNAVAILABLE,
    LoginStart, LoginSuccess, MAX_FRAME_BYTES, ProtocolCodec, ServerHello, ServerPacket, State,
    encode_uvarint, read_frame_ref, write_frame,
};
use mornlea_server::contracts::{
    ActorPersistence, AgentHandle, AgentPoll, AgentRequest, AgentRequestId, Clock, CloseReason,
    CompanionActionEnvelope, CompanionReceipt, ConnectionId, ConnectionProgress, Deadline,
    FinalReducer, FlushReport, FrozenLease, LoadPoll, LoginPoll, LoginTicket, McpLifecycle,
    MemoryFinalizationReport, MemoryFinalizer, NamespaceId, Operation, PlanningSnapshot,
    PlayerLoadPort, PublicationPort, Resource, SaveAuthority, SaveBudget, SaveKey, SavePoll,
    SaveRequest, SaveScheduleReport, SaveStats, SaveTicket, ServerEndpoint, ServerError,
    ServerLimits, ServerPhase, SessionKey, SessionPhase, ShutdownFailure, ShutdownReport,
    SnapshotId, SnapshotPort, SnapshotRegistration, StoreHandle, SubmissionReceipt,
    SubmitSaveError, TickBudget, TickPublication, TransportKind, WorkerLifecycle,
};
use mornlea_server::state::AuthorityState;
use mornlea_server::transport::common::{
    ConnectionCore, HandshakeLimits, MAX_PENDING_LOGINS, RECEIVE_BUFFER_CAP, TransportAuthority,
};
use mornlea_storage::StoredPlayer;

fn protocol() -> u32 {
    Identities::current().protocol
}
const WORLD_SEED: i64 = 7;

/// Injected monotonic clock. No case sleeps; time advances only explicitly.
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

/// One scripted load outcome per started ticket. The loaded record is boxed
/// to keep the script queue small beside the pending and failed markers.
enum LoadOutcome {
    Pending,
    Loaded(Box<Option<StoredPlayer>>),
    Failed(ServerError),
}

/// Executing `PlayerLoadPort` double: every started ticket resolves to the
/// next scripted outcome, or to a canonical new player when the script is
/// empty.
#[derive(Default)]
struct ScriptedLoad {
    next_ticket: u64,
    script: VecDeque<LoadOutcome>,
    entries: std::collections::BTreeMap<u64, LoadOutcome>,
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
        let ticket = LoginTicket::try_from_raw(self.next_ticket)
            .expect("load tickets are nonzero by construction");
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
            Some(LoadOutcome::Failed(error)) => LoadPoll::Failed(*error),
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
/// endpoint seams over one real `AuthorityState`, plus the scripted load port.
struct AdmissionDouble {
    authority: AuthorityState,
    loads: ScriptedLoad,
    tickets: std::collections::BTreeMap<u64, TicketRecord>,
    prepares: usize,
    installs: usize,
    activates: usize,
    commits: Vec<u64>,
    cancels: Vec<u64>,
    closes: Vec<SessionKey>,
    submits: Vec<SubmissionReceipt>,
}

impl AdmissionDouble {
    fn new() -> Self {
        let limits = ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap();
        Self {
            authority: AuthorityState::try_new(limits, WORLD_SEED).unwrap(),
            loads: ScriptedLoad::default(),
            tickets: std::collections::BTreeMap::new(),
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

    fn ticket_session(&self, ticket: LoginTicket) -> SessionKey {
        self.tickets[&ticket.get()].session
    }
}

impl ServerEndpoint for AdmissionDouble {
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

impl PublicationPort for AdmissionDouble {
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

impl TransportAuthority for AdmissionDouble {
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
                // The endpoint cancels the load and retires the prepared slot
                // before the typed failure reaches the transport. A corrupt
                // payload answers with the corrupt code; every other load
                // failure answers with the store-unavailable code, mirroring
                // the Go host's load-error mapping.
                self.loads.cancel(ticket).unwrap();
                let record = &self.tickets[&ticket.get()];
                self.authority
                    .retire(record.session, CloseReason::PeerGone)
                    .unwrap();
                let reject = match error {
                    ServerError::Io {
                        kind: std::io::ErrorKind::InvalidData,
                        ..
                    } => LOGIN_PLAYER_DATA_CORRUPT,
                    _ => LOGIN_STORE_UNAVAILABLE,
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
            // The retire must succeed: a cancel reaches a prepared session
            // exactly once, so a duplicate retire fails the test here instead
            // of being swallowed. This is the regression guard for the
            // failed-load double-retire.
            self.authority
                .retire(record.session, CloseReason::PeerGone)
                .unwrap();
            self.cancels.push(ticket.get());
        }
        // A committed handoff wins: a late cancel after commit retires
        // nothing through the login lane.
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

fn new_core() -> ConnectionCore {
    ConnectionCore::new(HandshakeLimits::source())
}

fn new_endpoint() -> AdmissionDouble {
    AdmissionDouble::new()
}

fn new_clock() -> StepClock {
    StepClock::new(Instant::now())
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

/// One login-start frame whose identity bytes are structurally decodable but
/// not a UUIDv4, so the raw inbound record survives to admission.
fn raw_identity_login_frame() -> Vec<u8> {
    let mut payload = vec![0u8; 16];
    payload.extend_from_slice(&encode_uvarint(3));
    payload.extend_from_slice(b"Ada");
    payload.push(8);
    write_frame(0, &payload).unwrap()
}

/// One sequenced play frame (CloseContainer, C/Play/10).
fn play_frame(sequence: u64) -> Vec<u8> {
    let record = CloseContainer::new(sequence);
    write_frame(CloseContainer::PACKET_ID, &record.encode().unwrap()).unwrap()
}

/// Splits one complete frame into its packet id and payload.
fn split_frame(frame: &[u8]) -> (u32, Vec<u8>) {
    let parsed = read_frame_ref(frame).unwrap();
    (parsed.packet_id, parsed.payload.to_vec())
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

fn decode_server_payload(state: State, id: u32, payload: &[u8]) -> ServerPacket {
    ProtocolCodec::new()
        .unwrap()
        .decode_server(state, id, payload)
        .unwrap()
}

fn expect_advanced(progress: ConnectionProgress, frames: usize) {
    assert_eq!(progress, ConnectionProgress::Advanced { frames });
}

fn expect_await(progress: ConnectionProgress) {
    assert_eq!(progress, ConnectionProgress::AwaitMore);
}

fn expect_closed(progress: ConnectionProgress) -> (CloseReason, Option<ServerError>) {
    match progress {
        ConnectionProgress::Closed { reason, class } => (reason, class),
        other => panic!("expected a closed connection, got {other:?}"),
    }
}

/// Drives one connection from `open` through the queued-but-unacknowledged
/// success handoff and returns its login ticket.
fn drive_to_handoff(
    core: &mut ConnectionCore,
    endpoint: &mut AdmissionDouble,
    clock: &StepClock,
    kind: TransportKind,
    tag: u8,
    name: &str,
) -> (ConnectionId, LoginTicket) {
    let id = core.open(kind, clock.monotonic()).unwrap();
    expect_advanced(
        core.ingest(id, hello_frame(protocol()), false, endpoint, clock),
        1,
    );
    let frames = core.take_frames(id, 8, 1 << 20);
    assert_eq!(frames.len(), 1);
    core.ack_sent(id, 1, endpoint);
    expect_advanced(
        core.ingest(id, login_start_frame(tag, name), false, endpoint, clock),
        1,
    );
    let ticket = endpoint.loads.last.expect("login started");
    expect_advanced(core.poll(id, endpoint, clock), 1);
    (id, ticket)
}

/// Drives one connection through the acknowledged success handoff. The
/// connection is in Play when this returns.
fn drive_to_play(
    core: &mut ConnectionCore,
    endpoint: &mut AdmissionDouble,
    clock: &StepClock,
    kind: TransportKind,
    tag: u8,
    name: &str,
) -> (ConnectionId, LoginTicket) {
    let (id, ticket) = drive_to_handoff(core, endpoint, clock, kind, tag, name);
    let frames = core.take_frames(id, 8, 1 << 20);
    assert_eq!(frames.len(), 1);
    expect_advanced(core.ack_sent(id, 1, endpoint), 1);
    (id, ticket)
}

#[test]
fn valid_handshake_round_trip() {
    for kind in [TransportKind::Memory, TransportKind::Tcp] {
        let mut core = new_core();
        let mut endpoint = new_endpoint();
        let clock = new_clock();
        let (id, ticket) = drive_to_play(&mut core, &mut endpoint, &clock, kind, 1, "Ada");

        // The negotiated ServerHello answered with this side's version and
        // was byte-equal to an independently encoded control frame (asserted
        // inside the login handoff case below for the success record; here
        // the state pins the whole admission).
        assert!(core.take_frames(id, 8, 1 << 20).is_empty());

        let record = &endpoint.tickets[&ticket.get()];
        assert_eq!(endpoint.prepares, 1);
        assert_eq!(endpoint.installs, 1);
        assert_eq!(endpoint.activates, 1);
        assert_eq!(endpoint.commits, vec![ticket.get()]);
        assert_eq!(
            endpoint.authority.session(record.session).unwrap().phase,
            SessionPhase::Active
        );
        assert_eq!(endpoint.world_seed(), WORLD_SEED);

        // Semantic conversion reaches the S1 admission path with the earliest
        // eligible tick and the session's first arrival index.
        expect_advanced(
            core.ingest(id, play_frame(5), false, &mut endpoint, &clock),
            1,
        );
        assert_eq!(
            endpoint.submits,
            vec![SubmissionReceipt::QueuedForTick {
                tick: 0,
                arrival_index: 0
            }]
        );

        // The independently encoded ServerHello is byte-equal to the frame
        // the connection queued during negotiation.
        let expected = ServerPacket::ServerHello(ServerHello::new(protocol()).unwrap());
        let mut second = new_core();
        let mut endpoint = new_endpoint();
        let clock = new_clock();
        let id = second.open(kind, clock.monotonic()).unwrap();
        expect_advanced(
            second.ingest(id, hello_frame(protocol()), false, &mut endpoint, &clock),
            1,
        );
        let frames = second.take_frames(id, 8, 1 << 20);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], control_frame(&expected));
    }
}

#[test]
fn sixteenth_seventeenth_reservation() {
    assert_eq!(MAX_PENDING_LOGINS, 16);
    let limits = HandshakeLimits::source();
    assert_eq!(limits.pending(), 16);
    assert_eq!(limits.hello_timeout(), Duration::from_secs(5));
    assert_eq!(limits.login_timeout(), Duration::from_secs(10));

    let mut core = new_core();
    let mut endpoint = new_endpoint();
    let clock = new_clock();
    let mut ids = Vec::new();
    for _ in 0..16 {
        ids.push(core.open(TransportKind::Tcp, clock.monotonic()).unwrap());
    }
    // A reservation is not an active S1 session: nothing reached the session
    // plane.
    assert_eq!(endpoint.prepares, 0);

    let seventeenth = core.open(TransportKind::Tcp, clock.monotonic());
    assert_eq!(
        seventeenth.unwrap_err(),
        ServerError::Capacity {
            resource: Resource::PendingLogins,
            limit: 16,
            observed: 17,
        }
    );

    // A released reservation permits the next: closing one connection frees
    // its pending slot.
    core.close(ids[0], CloseReason::PeerGone, &mut endpoint);
    assert!(
        core.open(TransportKind::Tcp, clock.monotonic()).is_ok(),
        "a closed reservation frees its pending slot"
    );

    // Timeout expiry also releases exactly once: the hello deadline passes,
    // the reservation expires with the connection, and a new reservation is
    // admitted.
    clock.advance(Duration::from_secs(5));
    expect_closed(core.poll(ids[1], &mut endpoint, &clock));
    assert!(
        core.open(TransportKind::Memory, clock.monotonic()).is_ok(),
        "an expired reservation frees its pending slot"
    );
    assert_eq!(endpoint.prepares, 0);
}

#[test]
fn wrong_version_truncated_expired_refused() {
    // Wrong version: the negotiated mismatch answer is delivered, then the
    // connection closes before any session work.
    let mut core = new_core();
    let mut endpoint = new_endpoint();
    let clock = new_clock();
    let id = core.open(TransportKind::Memory, clock.monotonic()).unwrap();
    expect_closed(core.ingest(
        id,
        hello_frame(protocol() - 1),
        false,
        &mut endpoint,
        &clock,
    ));
    let reject_frames = core.take_frames(id, 8, 1 << 20);
    assert_eq!(reject_frames.len(), 1);
    let (packet_id, payload) = split_frame(&reject_frames[0]);
    assert_eq!(packet_id, 1);
    let ServerPacket::HandshakeReject(record) =
        decode_server_payload(State::Handshake, 1, &payload)
    else {
        panic!("expected a handshake reject");
    };
    assert_eq!(record.server_protocol_version, protocol());
    assert_eq!(record.code, HANDSHAKE_VERSION_MISMATCH);
    assert_eq!(record.message, "协议版本不匹配");
    assert_eq!(endpoint.prepares, 0);

    // Truncated frame without terminal evidence waits; terminal evidence
    // closes silently.
    let id = core.open(TransportKind::Memory, clock.monotonic()).unwrap();
    let mut truncated = hello_frame(protocol());
    truncated.truncate(truncated.len() - 1);
    let retained = truncated.len();
    expect_await(core.ingest(id, truncated, false, &mut endpoint, &clock));
    assert_eq!(core.retained_len(id), Some(retained));
    let (reason, class) = expect_closed(core.ingest(id, Vec::new(), true, &mut endpoint, &clock));
    assert_eq!(reason, CloseReason::PeerGone);
    assert_eq!(class, None);

    // Expired hello: the five-second deadline fails the reservation before
    // any session work, and a later call replays the same close.
    let id = core.open(TransportKind::Memory, clock.monotonic()).unwrap();
    clock.advance(Duration::from_secs(5));
    let (reason, class) = expect_closed(core.poll(id, &mut endpoint, &clock));
    assert_eq!(
        class,
        Some(ServerError::Timeout {
            operation: Operation::Transport
        })
    );
    let _ = reason;
    expect_closed(core.poll(id, &mut endpoint, &clock));

    // Expired login: the ten-second deadline covers the load and the
    // handoff; expiry cancels the ticket and retires the prepared slot
    // before activation.
    let mut core = new_core();
    let mut endpoint = new_endpoint();
    let clock = new_clock();
    let id = core.open(TransportKind::Tcp, clock.monotonic()).unwrap();
    expect_advanced(
        core.ingest(id, hello_frame(protocol()), false, &mut endpoint, &clock),
        1,
    );
    let _ = core.take_frames(id, 8, 1 << 20);
    core.ack_sent(id, 1, &mut endpoint);
    endpoint.loads.push(LoadOutcome::Pending);
    expect_advanced(
        core.ingest(
            id,
            login_start_frame(2, "Bea"),
            false,
            &mut endpoint,
            &clock,
        ),
        1,
    );
    let ticket = endpoint.loads.last.expect("login started");
    clock.advance(Duration::from_secs(10));
    expect_closed(core.poll(id, &mut endpoint, &clock));
    assert_eq!(endpoint.cancels, vec![ticket.get()]);
    assert_eq!(endpoint.session_phase(ticket), SessionPhase::Retired);
    assert_eq!(endpoint.activates, 0);

    // Refused: a frame declaring an over-ceiling body closes silently before
    // any payload is buffered.
    let id = core.open(TransportKind::Memory, clock.monotonic()).unwrap();
    let oversized = encode_uvarint(MAX_FRAME_BYTES + 1);
    let refused_bytes = oversized.len();
    let (reason, _) = expect_closed(core.ingest(id, oversized, false, &mut endpoint, &clock));
    assert_eq!(reason, CloseReason::InvalidPlay);
    // The refused prefix stays retained on the closed connection; no payload
    // beyond it was ever buffered and no answer is queued.
    assert_eq!(core.retained_len(id), Some(refused_bytes));
    assert!(core.take_frames(id, 8, 1 << 20).is_empty());

    // A structurally valid login start with a non-UUIDv4 identity answers
    // with the invalid-identity reject code and never reaches admission.
    let prepares_before = endpoint.prepares;
    let id = core.open(TransportKind::Memory, clock.monotonic()).unwrap();
    expect_advanced(
        core.ingest(id, hello_frame(protocol()), false, &mut endpoint, &clock),
        1,
    );
    let _ = core.take_frames(id, 8, 1 << 20);
    expect_closed(core.ingest(id, raw_identity_login_frame(), false, &mut endpoint, &clock));
    let reject_frames = core.take_frames(id, 8, 1 << 20);
    assert_eq!(reject_frames.len(), 1);
    let (packet_id, payload) = split_frame(&reject_frames[0]);
    assert_eq!(packet_id, 1);
    let ServerPacket::LoginReject(record) = decode_server_payload(State::Login, 1, &payload) else {
        panic!("expected a login reject");
    };
    assert_eq!(record.code, mornlea_protocol::LOGIN_INVALID_IDENTITY);
    assert_eq!(endpoint.prepares, prepares_before);

    // A failed load answers with the store-unavailable reject code, and the
    // core must not cancel the ticket a second time: the endpoint's failed
    // outcome already cancelled the load and retired the prepared slot, so a
    // core-side cancel would double-retire it.
    let cancels_before = endpoint.cancels.len();
    let id = core.open(TransportKind::Memory, clock.monotonic()).unwrap();
    expect_advanced(
        core.ingest(id, hello_frame(protocol()), false, &mut endpoint, &clock),
        1,
    );
    let _ = core.take_frames(id, 8, 1 << 20);
    core.ack_sent(id, 1, &mut endpoint);
    endpoint.loads.push(LoadOutcome::Failed(ServerError::Io {
        operation: Operation::Load,
        kind: std::io::ErrorKind::TimedOut,
    }));
    expect_advanced(
        core.ingest(
            id,
            login_start_frame(3, "Cara"),
            false,
            &mut endpoint,
            &clock,
        ),
        1,
    );
    expect_closed(core.poll(id, &mut endpoint, &clock));
    assert_eq!(
        endpoint.cancels.len(),
        cancels_before,
        "a failed load is already cancelled by the endpoint; a second core-side cancel double-retires"
    );
    let reject_frames = core.take_frames(id, 8, 1 << 20);
    assert_eq!(reject_frames.len(), 1);
    let (_, payload) = split_frame(&reject_frames[0]);
    let ServerPacket::LoginReject(record) = decode_server_payload(State::Login, 1, &payload) else {
        panic!("expected a login reject");
    };
    assert_eq!(record.code, LOGIN_STORE_UNAVAILABLE);
    assert_eq!(record.message, "玩家数据暂不可用");
}

#[test]
fn unauthenticated_play_wrong_phase() {
    // Unauthenticated play: a play frame before login admission closes the
    // connection before any S1 submit.
    let mut core = new_core();
    let mut endpoint = new_endpoint();
    let clock = new_clock();
    let id = core.open(TransportKind::Memory, clock.monotonic()).unwrap();
    let (reason, _) = expect_closed(core.ingest(id, play_frame(1), false, &mut endpoint, &clock));
    assert_eq!(reason, CloseReason::InvalidPlay);
    assert_eq!(endpoint.prepares, 0);
    assert_eq!(endpoint.submits.len(), 0);

    // Wrong phase: a play frame between hello and login start is equally
    // unauthenticated.
    let id = core.open(TransportKind::Memory, clock.monotonic()).unwrap();
    expect_advanced(
        core.ingest(id, hello_frame(protocol()), false, &mut endpoint, &clock),
        1,
    );
    let _ = core.take_frames(id, 8, 1 << 20);
    expect_closed(core.ingest(id, play_frame(1), false, &mut endpoint, &clock));
    assert_eq!(endpoint.submits.len(), 0);

    // Wrong phase after the login started: the ticket exists but the session
    // is not active, so play is still refused and the prepared slot is
    // retired by the cancel path.
    let id = core.open(TransportKind::Memory, clock.monotonic()).unwrap();
    expect_advanced(
        core.ingest(id, hello_frame(protocol()), false, &mut endpoint, &clock),
        1,
    );
    let _ = core.take_frames(id, 8, 1 << 20);
    core.ack_sent(id, 1, &mut endpoint);
    expect_advanced(
        core.ingest(
            id,
            login_start_frame(3, "Cara"),
            false,
            &mut endpoint,
            &clock,
        ),
        1,
    );
    let ticket = endpoint.loads.last.expect("login started");
    expect_closed(core.ingest(id, play_frame(1), false, &mut endpoint, &clock));
    assert_eq!(endpoint.submits.len(), 0);
    assert_eq!(endpoint.cancels, vec![ticket.get()]);
    assert_eq!(endpoint.session_phase(ticket), SessionPhase::Retired);

    // A second hello after negotiation is a wrong-phase record and closes
    // silently without further session work.
    let id = core.open(TransportKind::Memory, clock.monotonic()).unwrap();
    expect_advanced(
        core.ingest(id, hello_frame(protocol()), false, &mut endpoint, &clock),
        1,
    );
    let _ = core.take_frames(id, 8, 1 << 20);
    expect_closed(core.ingest(id, hello_frame(protocol()), false, &mut endpoint, &clock));
    assert_eq!(endpoint.prepares, 1);
    assert_eq!(endpoint.installs, 0);
}

#[test]
fn coalesced_then_ingest_cap() {
    assert_eq!(RECEIVE_BUFFER_CAP, 4_194_314);

    // Coalesced ingress: hello and login start arrive in one chunk and both
    // frames process in one bounded poll; the outbound queue keeps order.
    let mut core = new_core();
    let mut endpoint = new_endpoint();
    let clock = new_clock();
    let id = core.open(TransportKind::Memory, clock.monotonic()).unwrap();
    let mut coalesced = hello_frame(protocol());
    coalesced.extend_from_slice(&login_start_frame(1, "Ada"));
    expect_advanced(core.ingest(id, coalesced, false, &mut endpoint, &clock), 2);
    assert_eq!(core.retained_len(id), Some(0));
    expect_advanced(core.poll(id, &mut endpoint, &clock), 1);
    let frames = core.take_frames(id, 8, 1 << 20);
    assert_eq!(frames.len(), 2);
    let (hello_id, hello_payload) = split_frame(&frames[0]);
    assert_eq!(hello_id, 0);
    assert_eq!(
        decode_server_payload(State::Handshake, 0, &hello_payload),
        ServerPacket::ServerHello(ServerHello::new(protocol()).unwrap())
    );
    let (success_id, success_payload) = split_frame(&frames[1]);
    assert_eq!(success_id, 0);
    assert!(matches!(
        decode_server_payload(State::Login, 0, &success_payload),
        ServerPacket::LoginSuccess(_)
    ));

    // The frame budget: one poll processes at most 64 frames and retains the
    // suffix for the next poll.
    let (id, _ticket) = drive_to_play(
        &mut core,
        &mut endpoint,
        &clock,
        TransportKind::Tcp,
        2,
        "Bea",
    );
    let mut batch = Vec::new();
    for sequence in 0..65 {
        batch.extend_from_slice(&play_frame(sequence));
    }
    expect_advanced(core.ingest(id, batch, false, &mut endpoint, &clock), 64);
    let last = play_frame(64);
    assert_eq!(core.retained_len(id), Some(last.len()));
    expect_advanced(core.poll(id, &mut endpoint, &clock), 1);
    assert_eq!(core.retained_len(id), Some(0));
    assert_eq!(endpoint.submits.len(), 65);

    // Ingest cap: retained bytes plus an incoming chunk beyond two maximal
    // frames are refused before any byte is appended, closing Capacity
    // silently.
    let id = core.open(TransportKind::Memory, clock.monotonic()).unwrap();
    let mut declared = encode_uvarint(MAX_FRAME_BYTES);
    declared.extend_from_slice(&vec![0u8; 100 - declared.len()]);
    let retained = declared.len();
    expect_await(core.ingest(id, declared, false, &mut endpoint, &clock));
    assert_eq!(core.retained_len(id), Some(retained));
    let (reason, class) = expect_closed(core.ingest(
        id,
        vec![7u8; RECEIVE_BUFFER_CAP - retained + 1],
        false,
        &mut endpoint,
        &clock,
    ));
    assert_eq!(reason, CloseReason::Capacity);
    assert_eq!(class, None);
    assert_eq!(
        core.retained_len(id),
        Some(retained),
        "the refused chunk is not appended"
    );

    // The boundary itself is legal: a chunk that brings the retained bytes to
    // exactly the cap is appended rather than refused.
    let id = core.open(TransportKind::Memory, clock.monotonic()).unwrap();
    let mut declared = encode_uvarint(MAX_FRAME_BYTES);
    declared.extend_from_slice(&vec![0u8; 100 - declared.len()]);
    let retained = declared.len();
    expect_await(core.ingest(id, declared, false, &mut endpoint, &clock));
    let appended = core.ingest(
        id,
        vec![7u8; RECEIVE_BUFFER_CAP - retained],
        false,
        &mut endpoint,
        &clock,
    );
    assert_ne!(
        appended,
        ConnectionProgress::Closed {
            reason: CloseReason::Capacity,
            class: None,
        },
        "an exactly-at-cap chunk is admitted"
    );
}

#[test]
fn login_success_send_ack_before_play() {
    let mut core = new_core();
    let mut endpoint = new_endpoint();
    let clock = new_clock();
    let (id, _early) = drive_to_handoff(
        &mut core,
        &mut endpoint,
        &clock,
        TransportKind::Memory,
        1,
        "Ada",
    );

    // The queued success is byte-equal to the independently encoded record.
    let frames = core.take_frames(id, 8, 1 << 20);
    assert_eq!(frames.len(), 1);
    let expected = ServerPacket::LoginSuccess(LoginSuccess::new(player(1), WORLD_SEED as u64));
    assert_eq!(frames[0], control_frame(&expected));

    // Queued success alone is not play: before the send acknowledgment every
    // play frame is unauthenticated and no submit reaches S1.
    let (reason, _) = expect_closed(core.ingest(id, play_frame(1), false, &mut endpoint, &clock));
    assert_eq!(reason, CloseReason::InvalidPlay);
    assert_eq!(endpoint.submits.len(), 0);
    assert_eq!(endpoint.activates, 0);

    // The acknowledged handoff commits exactly once and admits play.
    let (id, ticket) = drive_to_play(
        &mut core,
        &mut endpoint,
        &clock,
        TransportKind::Memory,
        2,
        "Bea",
    );
    assert_eq!(endpoint.commits, vec![ticket.get()]);
    assert_eq!(endpoint.session_phase(ticket), SessionPhase::Active);
    expect_advanced(
        core.ingest(id, play_frame(1), false, &mut endpoint, &clock),
        1,
    );
    assert_eq!(endpoint.submits.len(), 1);
}

#[test]
fn cancel_before_vs_after_handoff() {
    // Before the acknowledged handoff: cancellation retires the prepared
    // session, and the ticket can no longer activate.
    let mut core = new_core();
    let mut endpoint = new_endpoint();
    let clock = new_clock();
    let (id, early) = drive_to_handoff(
        &mut core,
        &mut endpoint,
        &clock,
        TransportKind::Memory,
        1,
        "Ada",
    );
    core.close(id, CloseReason::PeerGone, &mut endpoint);
    assert_eq!(endpoint.cancels, vec![early.get()]);
    assert_eq!(endpoint.session_phase(early), SessionPhase::Retired);
    assert_eq!(endpoint.activates, 0);
    // A late acknowledgment on the closed connection cannot commit, and the
    // endpoint itself refuses to activate a retired preparation.
    expect_closed(core.ack_sent(id, 1, &mut endpoint));
    assert_eq!(endpoint.commits.len(), 0);
    assert!(endpoint.commit_login(early).is_err());
    assert_eq!(endpoint.activates, 0);

    // The retired slot frees its player identity: a new login for the same
    // player reserves the released slot.
    let (id, relogin) = drive_to_handoff(
        &mut core,
        &mut endpoint,
        &clock,
        TransportKind::Memory,
        1,
        "Ada",
    );
    assert_eq!(endpoint.session_phase(relogin), SessionPhase::Prepared);
    core.close(id, CloseReason::PeerGone, &mut endpoint);

    // After the acknowledged handoff: the committed handoff wins the later
    // cancellation. The session activated, and the late close takes the
    // session lane rather than the login lane.
    let (id, late) = drive_to_play(
        &mut core,
        &mut endpoint,
        &clock,
        TransportKind::Tcp,
        2,
        "Bea",
    );
    assert_eq!(endpoint.commits, vec![late.get()]);
    assert_eq!(endpoint.session_phase(late), SessionPhase::Active);
    core.close(id, CloseReason::PeerGone, &mut endpoint);
    assert_eq!(
        endpoint.cancels,
        vec![early.get(), relogin.get()],
        "only unacknowledged handoffs cancel; the committed one closes"
    );
    assert_eq!(endpoint.closes, vec![endpoint.ticket_session(late)]);
    assert_eq!(endpoint.session_phase(late), SessionPhase::Retired);
}
