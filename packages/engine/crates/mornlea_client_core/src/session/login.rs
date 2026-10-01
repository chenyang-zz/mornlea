//! The login/session-observation provider: the first real `ClientEndpoint`
//! implementation inside the session seam.
//!
//! This provider owns the state-checked hello/login exchange before play, the
//! accepted monotonic deadlines, one terminal publication and one transport
//! release per session, and the real lifecycle Open/terminal records a pending
//! epoch issues. It is the sole place the session phase advances during
//! connection: `Connecting` issues the current-version hello, a validated
//! server hello moves the session to `Handshaking` and sends the login start,
//! and only a complete login success admits the session to `Play`. A frame
//! that does not belong to the current phase, that is structurally truncated,
//! that carries a foreign protocol version or whose declared body exceeds the
//! accepted frame cap is rejected in place: no transition, no terminal and no
//! release, exactly as a rejected frame is never a failed step.
//!
//! The deadlines come from the injected configuration. The accepted session
//! policy they carry is the landed transport binding's `HandshakeTimeout` of
//! five seconds and `LoginTimeout` of ten seconds
//! (`packages/shared/network/login.go`), the same values the replay harness
//! configuration builds its `ClientConfig` with, so a deployment may tighten
//! them but the tests pin the accepted policy.
//!
//! Ownership boundaries: the confirmed mirror's revision is advanced only by
//! the mirror provider, so this provider publishes session and lifecycle
//! records at revision zero and stages admitted play packets as pending
//! accepted observations for that owner to consume; the serial frame
//! assembler and the reset/local-close lifecycle conformance belong to their
//! own later nodes, so `reset` here only issues a fresh pending epoch and
//! `close` only observes terminal-once.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mornlea_domain::{Identities, PlayerId};
use mornlea_protocol::{
    ClientHello, LoginStart, ProtocolCodec, ProtocolError, ServerPacket, State, read_frame_ref,
    write_frame,
};

use crate::contracts::{
    ClientConfig, ClientEndpoint, ClientError, ClientIdentity, ClientLimits, ClientWorkBudget,
    CloseReason, ConfirmedRevision, Connector, ConnectorRegistry, Endpoint, FAMILY_LIFECYCLE,
    FAMILY_SESSION, FamilyKey, FamilyOperation, InputReceipt, MonotonicClock, ObservationKey,
    RecordHeader, SessionEpoch, SessionPhase, StepReport, TransportPoll, TransportTicket,
};
use crate::input::{InputAdmissionState, InputBatch, InputTranslator};
use crate::presentation::frame::{
    FamilyFrame, FamilyRecords, LifecycleRecord, LifecycleTransition, PresentationFrame,
    SessionRecord,
};
use crate::presentation::{
    AcceptedObservation, BoundedText, LifecycleProjectionState, ResourceKey, TextKind,
};
use crate::session::{ConfirmedMirror, ConfirmedMirrorParts};

/// The login/session state machine over the injected substitution ports.
///
/// One `LoginSession` owns at most one live epoch. Every mutation runs through
/// the checked paths of the contract landing: epochs are issued from a checked
/// nonzero counter, the transport is acquired through `try_connect` and
/// released exactly once, and every publication is a validator-checked frame.
pub struct LoginSession {
    limits: ClientLimits,
    hello_timeout: Duration,
    login_timeout: Duration,
    clock: Arc<dyn MonotonicClock>,
    connectors: Arc<ConnectorRegistry>,
    epoch_counter: u64,
    session: Option<LiveSession>,
}

/// The state of one live epoch.
struct LiveSession {
    epoch: SessionEpoch,
    phase: SessionPhase,
    endpoint: Endpoint,
    identity: ClientIdentity,
    connector: Arc<dyn Connector>,
    ticket: TransportTicket,
    hello_deadline: Instant,
    login_deadline: Option<Instant>,
    /// The complete login start frame, encoded once at connect and sent only
    /// after a validated server hello admits the handshake.
    login_frame: Vec<u8>,
    /// Complete outbound frames retained after a `Capacity` or `Io` send
    /// refusal; the head is retried before any new poll.
    outbound: VecDeque<Vec<u8>>,
    codec: ProtocolCodec,
    mirror: ConfirmedMirror,
    admission: InputAdmissionState,
    pending_inbound: Vec<AcceptedObservation>,
    inbound_limit: usize,
    limits: ClientLimits,
    lifecycle: LifecycleProjectionState,
    player_id: Option<PlayerId>,
    terminal: Option<CloseReason>,
    released: bool,
    visible: Option<Arc<PresentationFrame>>,
    frame_index: u64,
    /// Rejected inbound frames by class source; the diagnostics projection
    /// consumes this counter when its owner lands.
    rejected_inbound: u64,
}

impl LoginSession {
    /// Builds the session state machine from a checked configuration. The
    /// clock, connector registry and deadline policy are exactly the injected
    /// ones; no default deadline exists here.
    pub fn new(config: ClientConfig) -> Result<Self, ClientError> {
        Ok(Self {
            limits: *config.limits(),
            hello_timeout: config.hello_timeout(),
            login_timeout: config.login_timeout(),
            clock: Arc::clone(config.clock()),
            connectors: Arc::clone(config.connectors()),
            epoch_counter: 0,
            session: None,
        })
    }

    /// Builds one live epoch from the checked pieces. Every fallible piece is
    /// computed before the returned session exists, so a refusal never
    /// strands a half-built session; the caller releases the freshly
    /// acquired transport when this refuses.
    fn build_live_session(
        &self,
        endpoint: Endpoint,
        identity: ClientIdentity,
        connector: Arc<dyn Connector>,
        ticket: TransportTicket,
        epoch: SessionEpoch,
    ) -> Result<LiveSession, ClientError> {
        let login_frame = login_frame_bytes(&identity)?;
        let codec = ProtocolCodec::new().map_err(|_| ClientError::Internal)?;
        let mirror = ConfirmedMirror::try_new(ConfirmedMirrorParts::pending(epoch))?;
        let admission = InputAdmissionState::try_new(epoch, self.limits)?;
        let lifecycle = LifecycleProjectionState::try_new(epoch.get())?;
        let now = self.clock.now();
        Ok(LiveSession {
            epoch,
            phase: SessionPhase::Connecting,
            endpoint,
            identity,
            connector,
            ticket,
            hello_deadline: now + self.hello_timeout,
            login_deadline: None,
            login_frame,
            outbound: VecDeque::new(),
            codec,
            mirror,
            admission,
            pending_inbound: Vec::new(),
            inbound_limit: self.limits.inbound_observations(),
            limits: self.limits,
            lifecycle,
            player_id: None,
            terminal: None,
            released: false,
            visible: None,
            frame_index: 0,
            rejected_inbound: 0,
        })
    }

    /// Publishes one validator-checked frame at `next_index`: the current
    /// session record plus every still-pending lifecycle record, which the
    /// publication consumes exactly once.
    fn publish(&mut self, next_index: u64) -> Result<(), ClientError> {
        let limits = self.limits;
        let session = self.session.as_mut().ok_or(ClientError::InvalidState)?;
        let revision = session.mirror.revision();
        let header = RecordHeader::try_new(session.epoch, revision, None, FamilyOperation::Upsert)?;
        let session_record = SessionRecord::try_new(
            header,
            session.phase,
            session.player_id,
            session.terminal.clone(),
        )?;
        let mut families = vec![FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_SESSION)?,
            FamilyRecords::Session(vec![session_record]),
        )?];
        if !session.lifecycle.pending().is_empty() {
            families.push(FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_LIFECYCLE)?,
                FamilyRecords::Lifecycle(session.lifecycle.pending().to_vec()),
            )?);
        }
        let frame = PresentationFrame::try_new(session.epoch, revision, next_index, families)?;
        frame.validate(&limits)?;
        session.visible = Some(Arc::new(frame));
        session.frame_index = next_index;
        let generation = session.lifecycle.generation();
        session.lifecycle = LifecycleProjectionState::try_new(generation)?.with_pending(Vec::new());
        Ok(())
    }

    /// Terminates the session exactly once: the phase closes, the terminal
    /// session reason is set, the ordered terminal lifecycle pair is staged,
    /// the transport is released once and one terminal frame is published.
    /// A second call cannot occur through the public surface because a
    /// terminal session rejects further stepping.
    fn terminate(&mut self, reason: CloseReason) -> Result<(), ClientError> {
        {
            let session = self.session.as_mut().ok_or(ClientError::InvalidState)?;
            session.phase = SessionPhase::Closing;
            session.terminal = Some(reason);
            let generation = session.lifecycle.generation();
            let revision = session.mirror.revision();
            let header =
                RecordHeader::try_new(session.epoch, revision, None, FamilyOperation::Upsert)?;
            // The ordered terminal pair: `Close` ends the epoch and
            // `Invalidate` releases the actually core-owned resources in
            // consumer-before-provider order. No feature or bridge handle
            // identity exists in this core, so none can be listed.
            let close = LifecycleRecord::try_new(
                header,
                LifecycleTransition::Close,
                generation,
                Vec::new(),
            )?;
            let invalidate = LifecycleRecord::try_new(
                header,
                LifecycleTransition::Invalidate,
                generation,
                core_resources(),
            )?;
            let mut pending = session.lifecycle.pending().to_vec();
            pending.push(close);
            pending.push(invalidate);
            session.lifecycle =
                LifecycleProjectionState::try_new(generation)?.with_pending(pending);
            if !session.released {
                session.connector.close(session.ticket)?;
                session.released = true;
            }
        }
        let next = self.session.as_ref().expect("live session").frame_index + 1;
        self.publish(next)
    }
}

impl ClientEndpoint for LoginSession {
    fn connect(
        &mut self,
        endpoint: Endpoint,
        identity: ClientIdentity,
    ) -> Result<SessionEpoch, ClientError> {
        if self.session.is_some() {
            return Err(ClientError::InvalidState);
        }
        // Memory connector ids resolve only through the injected registry;
        // the TCP connector provider has not landed, so a TCP endpoint is an
        // invalid target here rather than an invented transport.
        let connector = match &endpoint {
            Endpoint::Memory { connector_id } => self
                .connectors
                .resolve(*connector_id)
                .ok_or(ClientError::InvalidInput)?,
            Endpoint::Tcp(_) => return Err(ClientError::InvalidInput),
        };
        // The hello frame is encoded before the transport is acquired, so an
        // encoding refusal can never leak a ticket.
        let hello = hello_frame_bytes()?;
        let ticket = connector.try_connect(&endpoint, &identity)?;
        self.epoch_counter = self
            .epoch_counter
            .checked_add(1)
            .ok_or(ClientError::Capacity)?;
        let epoch = SessionEpoch::try_new(self.epoch_counter)?;
        // Everything fallible after the ticket acquisition is guarded: a
        // refused construction releases the freshly acquired transport, so an
        // error leaves no leaked ticket behind.
        let mut session = match self.build_live_session(
            endpoint,
            identity,
            Arc::clone(&connector),
            ticket,
            epoch,
        ) {
            Ok(session) => session,
            Err(error) => {
                let _ = connector.close(ticket);
                return Err(error);
            }
        };
        // The current-version hello goes out immediately. A `Capacity` or
        // `Io` refusal retains the complete frame for the next step's retry;
        // nothing partial is ever sent.
        match session.connector.try_send(session.ticket, &hello) {
            Ok(()) => {}
            Err(ClientError::Capacity) | Err(ClientError::Io) => {
                session.outbound.push_back(hello);
            }
            Err(error) => {
                let _ = session.connector.close(session.ticket);
                return Err(error);
            }
        }
        // The pending epoch opens exactly one lifecycle record at revision
        // zero under the checked local core generation.
        let header = RecordHeader::try_new(
            epoch,
            ConfirmedRevision::new(0),
            None,
            FamilyOperation::Upsert,
        )?;
        let open = LifecycleRecord::try_new(
            header,
            LifecycleTransition::Open,
            epoch.get(),
            core_resources(),
        )?;
        session.lifecycle =
            LifecycleProjectionState::try_new(epoch.get())?.with_pending(vec![open]);
        self.session = Some(session);
        // The pending frame publishes at index zero: revision zero carries
        // only session and lifecycle records. A refused publication takes the
        // half-built session back and releases its transport.
        if let Err(error) = self.publish(0) {
            let session = self.session.take().expect("just connected");
            let _ = session.connector.close(session.ticket);
            return Err(error);
        }
        Ok(epoch)
    }

    fn submit_input(
        &mut self,
        epoch: SessionEpoch,
        input: InputBatch,
    ) -> Result<InputReceipt, ClientError> {
        let session = self.session.as_mut().ok_or(ClientError::InvalidState)?;
        if session.epoch != epoch {
            return Err(ClientError::StaleEpoch);
        }
        if session.terminal.is_some() {
            return Err(ClientError::Disconnected);
        }
        // The landed admission path is state-checked: a mirror that has not
        // been admitted by a complete login rejects with `InvalidState`
        // before any sequence, queue or journal changes.
        let validated = InputTranslator::validate_batch(&input, &session.mirror, &session.limits)?;
        InputTranslator::commit(validated, &mut session.admission)
    }

    fn step(
        &mut self,
        epoch: SessionEpoch,
        work: ClientWorkBudget,
    ) -> Result<StepReport, ClientError> {
        let (terminal, processed) = {
            let session = self.session.as_mut().ok_or(ClientError::InvalidState)?;
            if session.epoch != epoch {
                return Err(ClientError::StaleEpoch);
            }
            if session.terminal.is_some() {
                // One terminal publication per session: the terminal step
                // already published it and released the transport.
                return Err(ClientError::Disconnected);
            }
            // The monotonic deadlines are checked at step entry, before any
            // frame is processed: a hello or login answer that only arrives
            // after its deadline cannot extend the phase, exactly as a
            // blocking read under the same deadline would fail instead.
            // Each deadline covers its own phase from the instant that phase
            // began, and an admitted session has none.
            let now = self.clock.now();
            let mut terminal = match session.phase {
                SessionPhase::Connecting if now >= session.hello_deadline => {
                    Some(CloseReason::Timeout)
                }
                SessionPhase::Handshaking
                    if session
                        .login_deadline
                        .is_some_and(|deadline| now >= deadline) =>
                {
                    Some(CloseReason::Timeout)
                }
                _ => None,
            };
            let mut processed = 0u16;
            if terminal.is_none() {
                // Retry the retained outbound head first; a still-refused
                // head stays queued for the next step.
                while let Some(frame) = session.outbound.front().cloned() {
                    match session.connector.try_send(session.ticket, &frame) {
                        Ok(()) => {
                            session.outbound.pop_front();
                        }
                        Err(ClientError::Capacity) | Err(ClientError::Io) => break,
                        Err(error) => return Err(error),
                    }
                }
                while processed < work.messages() {
                    match session.connector.poll(session.ticket) {
                        // Connection-established and idle are both "no frame
                        // now": the bounded loop stops instead of spinning on
                        // a connector that never produces a frame.
                        TransportPoll::Connected | TransportPoll::Pending => break,
                        TransportPoll::Closed(error) => {
                            terminal = Some(transport_close_reason(error)?);
                            break;
                        }
                        TransportPoll::Frame(bytes) => {
                            processed += 1;
                            let now = self.clock.now();
                            handle_frame(session, &bytes, now, self.login_timeout, &mut terminal)?;
                            if terminal.is_some() {
                                break;
                            }
                        }
                    }
                }
            }
            (terminal, processed)
        };
        match terminal {
            Some(reason) => self.terminate(reason)?,
            None => {
                let next = self.session.as_ref().expect("live session").frame_index + 1;
                self.publish(next)?;
            }
        }
        let session = self.session.as_ref().expect("live session");
        StepReport::try_new(
            session.epoch,
            session.mirror.revision(),
            session.frame_index,
            processed,
            0,
            session.admission.outbound_records(),
            session.pending_inbound.len(),
            0,
            session.terminal.clone(),
        )
    }

    fn snapshot(&self, epoch: SessionEpoch) -> Result<Arc<PresentationFrame>, ClientError> {
        let session = self.session.as_ref().ok_or(ClientError::InvalidState)?;
        if session.epoch != epoch {
            return Err(ClientError::StaleEpoch);
        }
        session.visible.clone().ok_or(ClientError::InvalidState)
    }

    fn reset(&mut self, epoch: SessionEpoch) -> Result<SessionEpoch, ClientError> {
        // Minimal fresh-epoch behavior: the old transport is released once
        // and the same checked connect path issues the next pending epoch.
        // Reset-time invalidation and local-close conformance belong to the
        // later lifecycle node and are not accepted here.
        let (endpoint, identity) = {
            let session = self.session.as_mut().ok_or(ClientError::InvalidState)?;
            if session.epoch != epoch {
                return Err(ClientError::StaleEpoch);
            }
            if session.terminal.is_some() {
                return Err(ClientError::Disconnected);
            }
            if !session.released {
                session.connector.close(session.ticket)?;
                session.released = true;
            }
            (session.endpoint, session.identity.clone())
        };
        self.session = None;
        self.connect(endpoint, identity)
    }

    fn close(&mut self) -> Result<(), ClientError> {
        {
            let session = self.session.as_mut().ok_or(ClientError::InvalidState)?;
            if session.terminal.is_some() {
                return Err(ClientError::Disconnected);
            }
        }
        self.terminate(CloseReason::LocalClose)
    }
}

/// Handles one complete inbound frame against the current phase.
///
/// A frame that cannot be parsed under the landed framer (including a
/// declared body over the accepted frame cap) or decoded under the phase's
/// protocol state is rejected in place: counted, dropped and leaving every
/// owner unchanged. Only a validated packet may transition the phase, and the
/// login success additionally requires the confirmed identity to match the
/// request before admission.
fn handle_frame(
    session: &mut LiveSession,
    bytes: &[u8],
    now: Instant,
    login_timeout: Duration,
    terminal: &mut Option<CloseReason>,
) -> Result<(), ClientError> {
    let frame = match read_frame_ref(bytes) {
        Ok(frame) => frame,
        Err(_) => {
            session.rejected_inbound += 1;
            return Ok(());
        }
    };
    let state = decode_state(session.phase);
    let packet = match session
        .codec
        .decode_server(state, frame.packet_id, frame.payload)
    {
        Ok(packet) => packet,
        Err(_) => {
            session.rejected_inbound += 1;
            return Ok(());
        }
    };
    match (session.phase, packet) {
        // The decoder already refused any version but the current one, so a
        // decoded hello is the accepted one by construction.
        (SessionPhase::Connecting, ServerPacket::ServerHello(_)) => {
            session.phase = SessionPhase::Handshaking;
            session.login_deadline = Some(now + login_timeout);
            match session
                .connector
                .try_send(session.ticket, &session.login_frame)
            {
                Ok(()) => {}
                Err(ClientError::Capacity) | Err(ClientError::Io) => {
                    session.outbound.push_back(session.login_frame.clone());
                }
                Err(error) => {
                    // The refused frame stays owned for the next step's
                    // retry or terminal reporting; it is never dropped.
                    session.outbound.push_back(session.login_frame.clone());
                    return Err(error);
                }
            }
        }
        (SessionPhase::Connecting, ServerPacket::HandshakeReject(reject)) => {
            *terminal = Some(CloseReason::LoginRejected(control_text(&reject.message)?));
        }
        (SessionPhase::Handshaking, ServerPacket::LoginSuccess(success)) => {
            if success.player_id != session.identity.login().player_id {
                // A confirmed identity that is not the requested one never
                // admits the session; the rejection leaves the handshake
                // waiting for the real answer.
                session.rejected_inbound += 1;
                return Ok(());
            }
            session.phase = SessionPhase::Admitted;
            session.player_id = Some(success.player_id);
            session.mirror = ConfirmedMirror::try_new(ConfirmedMirrorParts {
                epoch: session.epoch,
                revision: session.mirror.revision(),
                phase: SessionPhase::Admitted,
                world: Some(session.mirror.world().clone()),
                actors: Some(session.mirror.actors().clone()),
                inventory: Some(session.mirror.inventory().clone()),
                world_ui: Some(session.mirror.world_ui().clone()),
            })?;
        }
        (SessionPhase::Handshaking, ServerPacket::LoginReject(reject)) => {
            *terminal = Some(CloseReason::LoginRejected(control_text(&reject.message)?));
        }
        (SessionPhase::Admitted, ServerPacket::Disconnect(disconnect)) => {
            *terminal = Some(CloseReason::RemoteDisconnect(control_text(
                &disconnect.message,
            )?));
        }
        (SessionPhase::Admitted, packet) => stage_observation(session, packet)?,
        _ => {
            session.rejected_inbound += 1;
        }
    }
    Ok(())
}

/// Stages one decoded play packet as a pending accepted observation for the
/// mirror provider to consume. The observation key carries the next expected
/// mirror revision and the staged ordinal; source ticks and actor identity
/// resolution belong to that provider, so staging here is structural.
fn stage_observation(session: &mut LiveSession, packet: ServerPacket) -> Result<(), ClientError> {
    if session.pending_inbound.len() + 1 > session.inbound_limit {
        session.rejected_inbound += 1;
        return Ok(());
    }
    let revision = ConfirmedRevision::new(
        session
            .mirror
            .revision()
            .get()
            .checked_add(1)
            .ok_or(ClientError::Capacity)?,
    );
    let ordinal =
        u32::try_from(session.pending_inbound.len()).map_err(|_| ClientError::Capacity)?;
    let key = ObservationKey::try_new(session.epoch, revision, ordinal)?;
    session
        .pending_inbound
        .push(AcceptedObservation::try_new(key, None, packet, Vec::new())?);
    Ok(())
}

/// The protocol state the current session phase decodes under: handshake
/// negotiation, login admission, then steady-state play.
fn decode_state(phase: SessionPhase) -> State {
    match phase {
        SessionPhase::Connecting => State::Handshake,
        SessionPhase::Handshaking => State::Login,
        SessionPhase::Admitted | SessionPhase::Closing | SessionPhase::Disconnected => State::Play,
    }
}

/// Maps one transport-reported close to the local terminal reason. The
/// transport carries no disconnect text, so a source disconnect keeps an
/// empty control value rather than invented prose.
fn transport_close_reason(error: ClientError) -> Result<CloseReason, ClientError> {
    match error {
        ClientError::Disconnected => Ok(CloseReason::RemoteDisconnect(control_text("")?)),
        ClientError::Timeout => Ok(CloseReason::Timeout),
        ClientError::Capacity => Ok(CloseReason::Capacity),
        _ => Ok(CloseReason::Internal),
    }
}

/// Wraps server control text byte-for-byte as bounded control text. The wire
/// control bound of 256 bytes and 256 runes is exactly the `Control`
/// admission, so a decoded message always fits.
fn control_text(message: &str) -> Result<BoundedText, ClientError> {
    BoundedText::try_new(message.to_string(), TextKind::Control)
}

/// The resource keys the client core actually owns, in release order:
/// consumers before providers. The Python feature and native bridge handle
/// owners live outside this core, so their identities are never listed.
fn core_resources() -> Vec<ResourceKey> {
    vec![
        ResourceKey::InputJournal,
        ResourceKey::PreparationQueue,
        ResourceKey::PresentationFrames,
    ]
}

/// The client's current-version hello frame through the landed encoder.
fn hello_frame_bytes() -> Result<Vec<u8>, ClientError> {
    let hello = ClientHello::new(Identities::current().protocol).map_err(encode_failed)?;
    let payload = hello.encode().map_err(encode_failed)?;
    write_frame(ClientHello::PACKET_ID, &payload).map_err(encode_failed)
}

/// The client's login start frame for one checked identity.
fn login_frame_bytes(identity: &ClientIdentity) -> Result<Vec<u8>, ClientError> {
    let payload = identity.login().encode().map_err(encode_failed)?;
    write_frame(LoginStart::PACKET_ID, &payload).map_err(encode_failed)
}

/// An already-validated record the landed encoder refuses is an internal
/// fault, never a wire condition to report as input.
fn encode_failed(_: ProtocolError) -> ClientError {
    ClientError::Internal
}
