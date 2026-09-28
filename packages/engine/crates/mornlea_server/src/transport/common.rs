//! Shared transport admission for the Memory and TCP adapters.
//!
//! This module owns the one connection path both adapters call: frame
//! decoding, hello validation, the shared prelogin reservation bounded by
//! [`HandshakeLimits`], login admission through [`TransportAuthority`],
//! semantic conversion into `PlayIntent`, S1 submit, and the outbound
//! control-packet encoder. The adapters duplicate none of it; they move bytes
//! in and out of this core.
//!
//! The connection state machine is `Hello -> Login -> Loading -> Handoff ->
//! Play -> Closed`. The transition into Play happens only when the adapter
//! acknowledges the actual send of the queued `LoginSuccess` frame through
//! [`ConnectionCore::ack_sent`]; a queued success alone is never play. A
//! cancellation that arrives before that acknowledgment retires the prepared
//! session through `cancel_login`, while a committed handoff wins a later
//! cancellation by closing through the ordinary session lane.
//!
//! Ownership note: the seam plan declares `TransportAuthority` and the
//! prelogin `HandshakeLimits` as shared transport declarations, and the
//! contract node landed neither in `core/contracts.rs` (its ledger records
//! that `HandshakeLimits` was deliberately not invented there). This module,
//! as the single owner of the shared transport path, declares both here with
//! the plan's exact signatures so the later adapters consume one seam.
//!
//! The mirrored values come from the Go sources: the prelogin ceiling from
//! `hostPreLoginCapacity` in `packages/server/server/host.go`, the two
//! deadlines from `HandshakeTimeout`/`LoginTimeout` in
//! `packages/shared/network/login.go`, the reject codes and messages from
//! the Go login driver, and the receive cap from the seam plan's
//! two-maximal-frame budget.

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use mornlea_domain::Identities;
use mornlea_protocol::{
    AdmittedLogin, ClientPacket, HandshakeReject, LOGIN_ALREADY_ONLINE, LOGIN_INTERNAL_ERROR,
    LOGIN_INVALID_IDENTITY, LOGIN_PLAYER_DATA_CORRUPT, LOGIN_PROTOCOL_VIOLATION, LOGIN_SERVER_FULL,
    LOGIN_STORE_UNAVAILABLE, LoginReject, MAX_FRAME_BYTES, PlayIntent, ProtocolCodec,
    ProtocolError, ServerHello, ServerPacket, State, admit_login, decode_client, read_frame_ref,
    validate_hello, write_frame,
};

use crate::contracts::{
    Clock, CloseReason, ConnectionId, ConnectionProgress, Deadline, LoginPoll, LoginTicket,
    Operation, PublicationPort, Resource, ServerEndpoint, ServerError, SessionKey, TransportKind,
};

/// Ceiling of simultaneous prelogin reservations, mirroring the Go host's
/// `hostPreLoginCapacity` pin. The sixteenth reservation succeeds and the
/// seventeenth is refused with `Capacity`.
pub const MAX_PENDING_LOGINS: usize = 16;
/// Hello negotiation deadline, mirroring the Go `HandshakeTimeout`.
pub const HELLO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Login deadline including the player load and the success handoff,
/// mirroring the Go `LoginTimeout`.
pub const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Total admitted receive storage per connection: one retained maximal frame
/// plus one bounded incoming chunk of the same size. A frame carries at most
/// a 2 MiB body and a 5-byte worst-case length prefix, so the budget is
/// `2 * (2 MiB + 5)` bytes.
pub const RECEIVE_BUFFER_CAP: usize = 2 * (MAX_FRAME_BYTES as usize + 5);
/// Frame ceiling one bounded poll processes before retaining the suffix.
const POLL_FRAME_BUDGET: usize = 64;
/// Byte ceiling one bounded poll processes over frame envelopes before
/// retaining the suffix. A single larger valid frame is processed alone.
const POLL_BYTE_BUDGET: usize = 1 << 20;

/// Checked prelogin reservation limits for one connection core.
///
/// The constructor ceilings are the frozen source values; a larger value is
/// refused before the core stores anything, exactly like the checked limit
/// records in the contract module.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandshakeLimits {
    pending: usize,
    hello_timeout: std::time::Duration,
    login_timeout: std::time::Duration,
}

impl HandshakeLimits {
    pub fn try_new(
        pending: usize,
        hello_timeout: std::time::Duration,
        login_timeout: std::time::Duration,
    ) -> Result<Self, ServerError> {
        if pending == 0 || pending > MAX_PENDING_LOGINS {
            return Err(ServerError::Capacity {
                resource: Resource::PendingLogins,
                limit: MAX_PENDING_LOGINS,
                observed: pending,
            });
        }
        if hello_timeout.is_zero() || hello_timeout > HELLO_TIMEOUT {
            return Err(ServerError::InvalidInput {
                field: "hello_timeout",
            });
        }
        if login_timeout.is_zero() || login_timeout > LOGIN_TIMEOUT {
            return Err(ServerError::InvalidInput {
                field: "login_timeout",
            });
        }
        Ok(Self {
            pending,
            hello_timeout,
            login_timeout,
        })
    }

    /// The frozen source configuration: 16 pending reservations, a 5 s hello
    /// deadline, and a 10 s login deadline.
    pub fn source() -> Self {
        Self {
            pending: MAX_PENDING_LOGINS,
            hello_timeout: HELLO_TIMEOUT,
            login_timeout: LOGIN_TIMEOUT,
        }
    }

    pub fn pending(self) -> usize {
        self.pending
    }

    pub fn hello_timeout(self) -> std::time::Duration {
        self.hello_timeout
    }

    pub fn login_timeout(self) -> std::time::Duration {
        self.login_timeout
    }
}

/// The authority seam one connection core drives. It bundles the frozen
/// endpoint and publication ports with the login lifecycle both adapters
/// share: `begin_login` rechecks capacity and identity through the same S1
/// admission, prepares the session, and starts a bounded player load;
/// `poll_login` reports the load outcome; `commit_login` activates exactly
/// once on the acknowledged success handoff; `cancel_login` retires a
/// prepared session whose handoff was never acknowledged. `world_seed` is
/// immutable until close and feeds the existing `LoginSuccess` payload.
pub trait TransportAuthority: ServerEndpoint + PublicationPort {
    fn begin_login(
        &mut self,
        login: AdmittedLogin,
        kind: TransportKind,
        deadline: Deadline,
    ) -> Result<LoginTicket, ServerError>;
    fn poll_login(&mut self, ticket: LoginTicket) -> LoginPoll;
    fn commit_login(&mut self, ticket: LoginTicket) -> Result<SessionKey, ServerError>;
    fn cancel_login(&mut self, ticket: LoginTicket);
    fn world_seed(&self) -> i64;
}

/// Close diagnostics paired by the state machine: the observable reason and
/// the retained internal class.
type Close = (CloseReason, Option<ServerError>);

/// Connection phase. The login deadline covers `Loading` and `Handoff`:
/// player loading and the success handoff both run inside the same 10 s
/// budget, and its expiry cancels whichever stage is in flight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// Awaiting the peer's `ClientHello` under the hello deadline.
    Hello { deadline: Deadline },
    /// Hello negotiated; awaiting `LoginStart` under the login deadline.
    Login { deadline: Deadline },
    /// Login admitted; the player load is in flight.
    Loading {
        ticket: LoginTicket,
        deadline: Deadline,
    },
    /// Load ready and `LoginSuccess` queued; awaiting the send acknowledgment.
    Handoff {
        ticket: LoginTicket,
        deadline: Deadline,
    },
    /// Acknowledged handoff committed; play traffic is admissible.
    Play { session: SessionKey },
    /// Terminal. The final view replays on every later call.
    Closed,
}

/// One queued outbound frame. The absolute index of the `LoginSuccess`
/// frame, recorded when it is queued, is what the send-acknowledgment ledger
/// compares against, so the frame itself carries only its bytes.
struct OutboundFrame {
    frame: Vec<u8>,
}

/// One connection's owned state: the retained inbound bytes, the queued
/// outbound frames, the send-acknowledgment ledger, and the phase.
struct Connection {
    kind: TransportKind,
    phase: Phase,
    codec: ProtocolCodec,
    inbound: Vec<u8>,
    outbound: VecDeque<OutboundFrame>,
    queued_total: usize,
    taken_total: usize,
    acked_total: usize,
    /// Absolute queue index of the `LoginSuccess` frame.
    success_index: Option<usize>,
    reservation: bool,
    closed: Option<ConnectionProgress>,
}

impl Connection {
    fn new(kind: TransportKind, deadline: Deadline, codec: ProtocolCodec) -> Self {
        Self {
            kind,
            phase: Phase::Hello { deadline },
            codec,
            inbound: Vec::new(),
            outbound: VecDeque::new(),
            queued_total: 0,
            taken_total: 0,
            acked_total: 0,
            success_index: None,
            reservation: true,
            closed: None,
        }
    }

    fn deadline(&self) -> Option<Deadline> {
        match self.phase {
            Phase::Hello { deadline }
            | Phase::Login { deadline }
            | Phase::Loading { deadline, .. }
            | Phase::Handoff { deadline, .. } => Some(deadline),
            Phase::Play { .. } | Phase::Closed => None,
        }
    }

    /// Queues one encoded control packet and returns its absolute index.
    fn queue_packet(&mut self, packet: &ServerPacket) -> Result<usize, ServerError> {
        let frame = encode_frame(&mut self.codec, packet)?;
        self.outbound.push_back(OutboundFrame { frame });
        self.queued_total += 1;
        Ok(self.queued_total - 1)
    }
}

/// The shared connection core. One instance serves one transport endpoint
/// (Memory or TCP); it owns the prelogin reservation pool and every
/// connection's state machine. All time flows in through the injected clock.
pub struct ConnectionCore {
    limits: HandshakeLimits,
    connections: BTreeMap<u64, Connection>,
    next_id: u64,
    pending: usize,
}

impl ConnectionCore {
    pub fn new(limits: HandshakeLimits) -> Self {
        Self {
            limits,
            connections: BTreeMap::new(),
            next_id: 0,
            pending: 0,
        }
    }

    /// Reserves one prelogin slot and opens a connection in the hello phase.
    /// The sixteenth simultaneous reservation succeeds; the seventeenth is
    /// refused with `Capacity` before any state is stored. A reservation is
    /// not an active S1 session.
    pub fn open(&mut self, kind: TransportKind, now: Instant) -> Result<ConnectionId, ServerError> {
        if self.pending >= self.limits.pending || self.next_id == u64::MAX {
            return Err(ServerError::Capacity {
                resource: Resource::PendingLogins,
                limit: self.limits.pending,
                observed: self.pending + 1,
            });
        }
        let deadline = Deadline::after(now, self.limits.hello_timeout)?;
        let codec =
            ProtocolCodec::new().map_err(|_| ServerError::InvalidInput { field: "packet" })?;
        self.next_id += 1;
        let id = ConnectionId::try_from_raw(self.next_id)?;
        self.connections
            .insert(id.get(), Connection::new(kind, deadline, codec));
        self.pending += 1;
        Ok(id)
    }

    /// Offers one owned incoming chunk. A chunk that would push the retained
    /// unconsumed bytes past [`RECEIVE_BUFFER_CAP`] is refused before any of
    /// it is appended and the connection closes `Capacity` silently;
    /// otherwise the bytes are appended and one bounded poll runs. `terminal`
    /// marks end-of-stream evidence: a still-truncated tail then closes the
    /// connection silently.
    pub fn ingest(
        &mut self,
        id: ConnectionId,
        bytes: Vec<u8>,
        terminal: bool,
        endpoint: &mut dyn TransportAuthority,
        clock: &dyn Clock,
    ) -> ConnectionProgress {
        if let Some(progress) = self.replay_or_expire(id, endpoint, clock) {
            return progress;
        }
        let cap_refused = self.connections.get(&id.get()).is_some_and(|conn| {
            conn.inbound.len().saturating_add(bytes.len()) > RECEIVE_BUFFER_CAP
        });
        if cap_refused {
            return self.shut(id, CloseReason::Capacity, None, endpoint);
        }
        let Some(conn) = self.connections.get_mut(&id.get()) else {
            return unknown_connection();
        };
        conn.inbound.extend_from_slice(&bytes);
        let progress = self.drive(id, endpoint, clock);
        // End-of-stream evidence: no further bytes can arrive, so an
        // unconsumed tail can never complete. The connection ends without a
        // wire answer, like the Go driver's EOF path.
        if terminal
            && self
                .connections
                .get(&id.get())
                .is_some_and(|conn| conn.closed.is_none())
        {
            return self.shut(id, CloseReason::PeerGone, None, endpoint);
        }
        progress
    }

    /// Drives one connection without new bytes: it advances an in-flight
    /// login by one `poll_login`, otherwise re-runs one bounded poll over
    /// retained input, and reports deadline expiry. Play-phase liveness
    /// probing stays with the adapter; this poll never blocks.
    pub fn poll(
        &mut self,
        id: ConnectionId,
        endpoint: &mut dyn TransportAuthority,
        clock: &dyn Clock,
    ) -> ConnectionProgress {
        if let Some(progress) = self.replay_or_expire(id, endpoint, clock) {
            return progress;
        }
        let loading = self
            .connections
            .get(&id.get())
            .is_some_and(|conn| matches!(conn.phase, Phase::Loading { .. }));
        if !loading {
            return self.drive(id, endpoint, clock);
        }
        let (ticket, deadline) = match &self.connections[&id.get()].phase {
            Phase::Loading { ticket, deadline } => (*ticket, *deadline),
            _ => return unknown_connection(),
        };
        match endpoint.poll_login(ticket) {
            LoginPoll::Pending => ConnectionProgress::AwaitMore,
            LoginPoll::Ready {
                session: _,
                success,
            } => match self.queue_control(id, &success) {
                Ok(index) => {
                    let conn = self
                        .connections
                        .get_mut(&id.get())
                        .expect("connection exists");
                    conn.success_index = Some(index);
                    conn.phase = Phase::Handoff { ticket, deadline };
                    ConnectionProgress::Advanced { frames: 1 }
                }
                Err(close) => self.shut_with(id, close, endpoint),
            },
            LoginPoll::Failed { error, reject } => {
                // The endpoint already cancelled the load and retired the
                // prepared slot before reporting the failure, so the close
                // records only: cancelling again would double-retire. The
                // best-effort reject still reaches the peer first.
                let queued = (|| {
                    let record = LoginReject::new(reject, reject_message(reject))
                        .map_err(|_| (CloseReason::PeerGone, Some(reject_error(reject))))?;
                    self.queue_control(id, &ServerPacket::LoginReject(record))
                        .map(|_| ())
                })();
                let close = queued.err().unwrap_or((CloseReason::PeerGone, Some(error)));
                self.record_close(id, close.0, close.1)
            }
        }
    }

    /// Marks `frames` queued frames as actually sent by the adapter. When the
    /// acknowledgment covers the queued `LoginSuccess` frame, the handoff
    /// commits exactly once and the connection enters Play; a commit failure
    /// closes the connection with the typed error.
    pub fn ack_sent(
        &mut self,
        id: ConnectionId,
        frames: usize,
        endpoint: &mut dyn TransportAuthority,
    ) -> ConnectionProgress {
        if let Some(closed) = self.replay(id) {
            return closed;
        }
        let Some(conn) = self.connections.get_mut(&id.get()) else {
            return unknown_connection();
        };
        let markable = conn.taken_total.saturating_sub(conn.acked_total);
        let marked = frames.min(markable);
        conn.acked_total += marked;
        let success_acked = conn
            .success_index
            .is_some_and(|index| index < conn.acked_total);
        let handoff = match conn.phase {
            Phase::Handoff {
                ticket,
                deadline: _,
            } => Some(ticket),
            _ => None,
        };
        let Some(ticket) = handoff.filter(|_| success_acked) else {
            return if marked > 0 {
                ConnectionProgress::Advanced { frames: marked }
            } else {
                ConnectionProgress::AwaitMore
            };
        };
        match endpoint.commit_login(ticket) {
            Ok(session) => {
                let conn = self
                    .connections
                    .get_mut(&id.get())
                    .expect("connection exists");
                conn.phase = Phase::Play { session };
                // A committed connection no longer holds a prelogin slot; it
                // holds an active session slot instead.
                self.release_reservation(id);
                ConnectionProgress::Advanced {
                    frames: marked.max(1),
                }
            }
            Err(error) => self.shut(id, CloseReason::PeerGone, Some(error), endpoint),
        }
    }

    /// Hands the adapter up to `max_frames` queued frames and up to
    /// `max_bytes` of frame envelope, always delivering a nonempty queue's
    /// first frame even when it alone exceeds the byte budget. Ownership of
    /// the returned buffers transfers to the caller; the acknowledgment
    /// ledger keeps the send order.
    pub fn take_frames(
        &mut self,
        id: ConnectionId,
        max_frames: usize,
        max_bytes: usize,
    ) -> Vec<Vec<u8>> {
        let Some(conn) = self.connections.get_mut(&id.get()) else {
            return Vec::new();
        };
        let mut taken = Vec::new();
        let mut bytes = 0usize;
        while taken.len() < max_frames {
            let Some(next) = conn.outbound.front() else {
                break;
            };
            let next_bytes = bytes.saturating_add(next.frame.len());
            if !taken.is_empty() && next_bytes > max_bytes {
                break;
            }
            bytes = next_bytes;
            taken.push(conn.outbound.pop_front().expect("front frame exists").frame);
        }
        conn.taken_total += taken.len();
        taken
    }

    /// Closes one connection. Queued frames stay drainable so a closing
    /// rejection can still reach the peer. A login whose handoff was never
    /// acknowledged is cancelled (the prepared session retires); a committed
    /// connection closes through the ordinary session lane, so a late
    /// cancellation never un-commits a handoff.
    pub fn close(
        &mut self,
        id: ConnectionId,
        reason: CloseReason,
        endpoint: &mut dyn TransportAuthority,
    ) {
        self.shut(id, reason, None, endpoint);
    }

    /// Retained unconsumed inbound bytes for one connection.
    pub fn retained_len(&self, id: ConnectionId) -> Option<usize> {
        self.connections
            .get(&id.get())
            .map(|conn| conn.inbound.len())
    }

    /// Replays a stored close, or applies a passed deadline as a timeout
    /// close before the caller does any work.
    fn replay_or_expire(
        &mut self,
        id: ConnectionId,
        endpoint: &mut dyn TransportAuthority,
        clock: &dyn Clock,
    ) -> Option<ConnectionProgress> {
        if let Some(closed) = self.replay(id) {
            return Some(closed);
        }
        let now = clock.monotonic();
        let expired = self
            .connections
            .get(&id.get())
            .and_then(|conn| conn.deadline())
            .is_some_and(|deadline| deadline.expired(now));
        if expired {
            // Pre-play deadline expiry is silent on the wire; the typed
            // timeout stays in the diagnostic class.
            let timeout = ServerError::Timeout {
                operation: Operation::Transport,
            };
            Some(self.shut(id, CloseReason::PeerGone, Some(timeout), endpoint))
        } else {
            None
        }
    }

    fn replay(&self, id: ConnectionId) -> Option<ConnectionProgress> {
        self.connections.get(&id.get()).and_then(|conn| conn.closed)
    }

    /// Closes a connection through the authority: cancel an unacknowledged
    /// login, close an active session, release the reservation exactly once,
    /// and record the replayable terminal view.
    fn shut(
        &mut self,
        id: ConnectionId,
        reason: CloseReason,
        class: Option<ServerError>,
        endpoint: &mut dyn TransportAuthority,
    ) -> ConnectionProgress {
        let cancel = {
            let Some(conn) = self.connections.get_mut(&id.get()) else {
                return unknown_connection();
            };
            if let Some(closed) = conn.closed {
                return closed;
            }
            match conn.phase {
                Phase::Loading { ticket, .. } | Phase::Handoff { ticket, .. } => Some(ticket),
                Phase::Play { session } => {
                    let _ = endpoint.close_session(session, reason);
                    None
                }
                Phase::Hello { .. } | Phase::Login { .. } | Phase::Closed => None,
            }
        };
        if let Some(ticket) = cancel {
            // The handoff was never acknowledged, so the login is cancelled
            // and the prepared session retires. A committed handoff never
            // reaches this branch: commit moved the phase to Play first.
            endpoint.cancel_login(ticket);
        }
        self.record_close(id, reason, class)
    }

    fn shut_with(
        &mut self,
        id: ConnectionId,
        close: Close,
        endpoint: &mut dyn TransportAuthority,
    ) -> ConnectionProgress {
        self.shut(id, close.0, close.1, endpoint)
    }

    /// Records the terminal view without any authority interaction.
    fn record_close(
        &mut self,
        id: ConnectionId,
        reason: CloseReason,
        class: Option<ServerError>,
    ) -> ConnectionProgress {
        let progress = ConnectionProgress::Closed { reason, class };
        if let Some(conn) = self.connections.get_mut(&id.get()) {
            conn.phase = Phase::Closed;
        } else {
            return unknown_connection();
        }
        self.release_reservation(id);
        let conn = self
            .connections
            .get_mut(&id.get())
            .expect("connection exists");
        conn.closed = Some(progress);
        progress
    }

    fn release_reservation(&mut self, id: ConnectionId) {
        if let Some(conn) = self.connections.get_mut(&id.get())
            && conn.reservation
        {
            conn.reservation = false;
            self.pending = self.pending.saturating_sub(1);
        }
    }

    /// Queues one control packet on a connection, reporting queue failures in
    /// the close vocabulary the state machine consumes.
    fn queue_control(&mut self, id: ConnectionId, packet: &ServerPacket) -> Result<usize, Close> {
        self.connections
            .get_mut(&id.get())
            .expect("connection exists")
            .queue_packet(packet)
            .map_err(|error| (CloseReason::PeerGone, Some(error)))
    }

    /// Runs one bounded poll over the retained inbound bytes: at most
    /// [`POLL_FRAME_BUDGET`] frames and [`POLL_BYTE_BUDGET`] bytes of frame
    /// envelope per call, with a single larger valid frame processed alone,
    /// and the unconsumed suffix retained for the next poll.
    fn drive(
        &mut self,
        id: ConnectionId,
        endpoint: &mut dyn TransportAuthority,
        clock: &dyn Clock,
    ) -> ConnectionProgress {
        let mut frames = 0usize;
        let mut bytes = 0usize;
        loop {
            let parsed = {
                let Some(conn) = self.connections.get(&id.get()) else {
                    return unknown_connection();
                };
                if conn.closed.is_some() {
                    break;
                }
                read_frame_ref(&conn.inbound)
            };
            let frame = match parsed {
                Ok(frame) => frame,
                // An incomplete frame waits for more bytes; the caller's
                // terminal evidence decides whether that wait ever ends.
                Err(ProtocolError::Truncated) => break,
                // Every other framing refusal is a malformed frame: the
                // connection closes silently before any payload is used.
                Err(_) => {
                    self.shut(
                        id,
                        CloseReason::InvalidPlay,
                        Some(ServerError::InvalidInput { field: "frame" }),
                        endpoint,
                    );
                    break;
                }
            };
            if frames > 0
                && (frames + 1 > POLL_FRAME_BUDGET
                    || bytes.saturating_add(frame.consumed) > POLL_BYTE_BUDGET)
            {
                break;
            }
            let (packet_id, payload) = (frame.packet_id, frame.payload.to_vec());
            let consumed = frame.consumed;
            {
                let Some(conn) = self.connections.get_mut(&id.get()) else {
                    return unknown_connection();
                };
                conn.inbound.drain(..consumed);
            }
            frames += 1;
            bytes += consumed;
            if let Err(close) =
                self.process_frame(id, packet_id, payload, endpoint, clock.monotonic())
            {
                self.shut_with(id, close, endpoint);
                break;
            }
        }
        let Some(conn) = self.connections.get(&id.get()) else {
            return unknown_connection();
        };
        if let Some(closed) = conn.closed {
            return closed;
        }
        if frames > 0 {
            ConnectionProgress::Advanced { frames }
        } else {
            ConnectionProgress::AwaitMore
        }
    }

    /// Advances the state machine by exactly one decoded frame.
    fn process_frame(
        &mut self,
        id: ConnectionId,
        packet_id: u32,
        payload: Vec<u8>,
        endpoint: &mut dyn TransportAuthority,
        now: Instant,
    ) -> Result<(), Close> {
        let (kind, phase) = {
            let Some(conn) = self.connections.get(&id.get()) else {
                return Err((CloseReason::InvalidPlay, None));
            };
            (conn.kind, conn.phase)
        };
        match phase {
            Phase::Hello { .. } => {
                let packet = decode_client(State::Handshake, packet_id, &payload)
                    .map_err(|_| frame_refused())?;
                let ClientPacket::ClientHello(hello) = packet else {
                    return Err(frame_refused());
                };
                match validate_hello(hello) {
                    Ok(()) => {
                        let server = ServerPacket::ServerHello(
                            ServerHello::new(Identities::current().protocol)
                                .map_err(|_| packet_refused())?,
                        );
                        self.queue_control(id, &server)?;
                        let deadline = Deadline::after(now, self.limits.login_timeout)
                            .map_err(|error| (CloseReason::PeerGone, Some(error)))?;
                        let conn = self
                            .connections
                            .get_mut(&id.get())
                            .expect("connection exists");
                        conn.phase = Phase::Login { deadline };
                        Ok(())
                    }
                    Err(mornlea_protocol::HandshakeRejection::VersionMismatch {
                        server_version,
                    }) => {
                        // The negotiated mismatch answer is delivered before
                        // the connection ends; no session work happens.
                        let reject = HandshakeReject::new(
                            server_version,
                            mornlea_protocol::HANDSHAKE_VERSION_MISMATCH,
                            "协议版本不匹配",
                        )
                        .map_err(|_| packet_refused())?;
                        self.queue_control(id, &ServerPacket::HandshakeReject(reject))?;
                        Err((CloseReason::PeerGone, None))
                    }
                }
            }
            Phase::Login { deadline } => {
                let packet = decode_client(State::Login, packet_id, &payload)
                    .map_err(|_| frame_refused())?;
                let ClientPacket::LoginStart(start) = packet else {
                    return Err(frame_refused());
                };
                let admitted = match admit_login(start) {
                    Ok(admitted) => admitted,
                    Err(mornlea_protocol::LoginAdmissionError::InvalidIdentity) => {
                        return self.queue_login_reject(id, LOGIN_INVALID_IDENTITY);
                    }
                    Err(mornlea_protocol::LoginAdmissionError::ProtocolViolation) => {
                        return self.queue_login_reject(id, LOGIN_PROTOCOL_VIOLATION);
                    }
                };
                match endpoint.begin_login(admitted, kind, deadline) {
                    Ok(ticket) => {
                        let conn = self
                            .connections
                            .get_mut(&id.get())
                            .expect("connection exists");
                        conn.phase = Phase::Loading { ticket, deadline };
                        Ok(())
                    }
                    Err(error) => self.queue_login_reject(id, begin_reject_code(&error)),
                }
            }
            // The session is prepared but not active, so any further frame is
            // unauthenticated play and the login is cancelled by the close.
            Phase::Loading { .. } | Phase::Handoff { .. } => Err(frame_refused()),
            Phase::Play { session } => {
                let packet =
                    decode_client(State::Play, packet_id, &payload).map_err(|_| frame_refused())?;
                let intent = PlayIntent::try_from(packet).map_err(|_| frame_refused())?;
                match endpoint.submit(session, intent) {
                    Ok(_) => Ok(()),
                    Err(error) => Err((submit_close_reason(&error), Some(error))),
                }
            }
            Phase::Closed => Ok(()),
        }
    }

    /// Queues the best-effort login rejection for one of the shared failure
    /// gates and reports the follow-on close.
    fn queue_login_reject(&mut self, id: ConnectionId, code: u8) -> Result<(), Close> {
        let record = LoginReject::new(code, reject_message(code)).map_err(|_| packet_refused())?;
        self.queue_control(id, &ServerPacket::LoginReject(record))?;
        Err((CloseReason::PeerGone, Some(reject_error(code))))
    }
}

/// Renders one typed server packet into its complete length-prefixed wire
/// frame: the packet's registry key names the frame's packet id and the
/// codec writes the payload, matching the Go `WriteFrame` composition.
fn encode_frame(codec: &mut ProtocolCodec, packet: &ServerPacket) -> Result<Vec<u8>, ServerError> {
    let mut buffer = vec![0u8; 64];
    let payload = loop {
        match codec.encode_server_into(packet, &mut buffer) {
            Ok(written) => break buffer[..written].to_vec(),
            Err(ProtocolError::OutputTooSmall { needed, .. }) if needed > buffer.len() => {
                buffer.resize(needed, 0);
            }
            Err(_) => return Err(ServerError::InvalidInput { field: "packet" }),
        }
    };
    write_frame(packet.key().id, &payload)
        .map_err(|_| ServerError::InvalidInput { field: "packet" })
}

/// A malformed or wrong-phase frame closes silently; the class keeps the
/// frame diagnostic.
fn frame_refused() -> Close {
    (
        CloseReason::InvalidPlay,
        Some(ServerError::InvalidInput { field: "frame" }),
    )
}

/// An unencodable control packet is an internal transport failure.
fn packet_refused() -> Close {
    (
        CloseReason::PeerGone,
        Some(ServerError::InvalidInput { field: "packet" }),
    )
}

fn submit_close_reason(error: &ServerError) -> CloseReason {
    match error {
        ServerError::StaleSession { .. } => CloseReason::PeerGone,
        ServerError::InvalidState { .. } => CloseReason::Shutdown,
        _ => CloseReason::InvalidPlay,
    }
}

/// Maps one `begin_login` refusal onto the frozen reject vocabulary. A full
/// player plane answers `LOGIN_SERVER_FULL`, a duplicate live identity
/// answers `LOGIN_ALREADY_ONLINE`, and every other refusal is the internal
/// error, mirroring the Go host's reservation answers.
fn begin_reject_code(error: &ServerError) -> u8 {
    match error {
        ServerError::Capacity {
            resource: Resource::Players,
            ..
        } => LOGIN_SERVER_FULL,
        ServerError::InvalidInput { field: "player_id" } => LOGIN_ALREADY_ONLINE,
        _ => LOGIN_INTERNAL_ERROR,
    }
}

/// The diagnostic class carried beside a queued login rejection, so the
/// transport matrix retains the internal failure for every wire answer.
fn reject_error(code: u8) -> ServerError {
    ServerError::InvalidInput {
        field: match code {
            LOGIN_SERVER_FULL => "login_full",
            LOGIN_INVALID_IDENTITY => "login_identity",
            LOGIN_PLAYER_DATA_CORRUPT => "login_corrupt",
            LOGIN_STORE_UNAVAILABLE => "login_store",
            LOGIN_PROTOCOL_VIOLATION => "login_view",
            LOGIN_ALREADY_ONLINE => "login_duplicate",
            _ => "login_internal",
        },
    }
}

/// The Go login driver's exact reject messages, keyed by the frozen code.
fn reject_message(code: u8) -> &'static str {
    match code {
        LOGIN_SERVER_FULL => "服务器已满",
        LOGIN_INVALID_IDENTITY => "玩家 ID 或昵称非法",
        LOGIN_PLAYER_DATA_CORRUPT => "玩家数据已损坏",
        LOGIN_STORE_UNAVAILABLE => "玩家数据暂不可用",
        LOGIN_PROTOCOL_VIOLATION => "视距非法",
        LOGIN_INTERNAL_ERROR => "服务端无法建立会话",
        LOGIN_ALREADY_ONLINE => "玩家已在线",
        _ => "服务端无法建立会话",
    }
}

fn unknown_connection() -> ConnectionProgress {
    ConnectionProgress::Closed {
        reason: CloseReason::InvalidPlay,
        class: Some(ServerError::Internal {
            invariant: "connection",
        }),
    }
}
