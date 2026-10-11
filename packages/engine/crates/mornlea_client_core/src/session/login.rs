//! The sole live session composes the accepted transport, mirror and publication owners.
//! Wire packets use the shared protocol codec; observations retire only at full-frame commit.

use std::collections::VecDeque;
use std::num::NonZeroU64;

#[path = "publication.rs"]
mod publication;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mornlea_domain::{BlockPos, ChunkPos, Dimension, Event, Identities, PlayerId};
use mornlea_protocol::{
    ClientHello, ClientPacket, KeepAliveReply, LoginStart, ProtocolCodec, ProtocolError,
    ServerPacket, State, encode_client_into, read_frame_ref, write_frame,
};

use crate::contracts::{
    ClientConfig, ClientEndpoint, ClientError, ClientIdentity, ClientLimits, ClientWorkBudget,
    CloseReason, ConfirmedRevision, Connector, ConnectorRegistry, Endpoint, FAMILY_SESSION,
    FamilyKey, FamilyOperation, InputReceipt, MonotonicClock, ObservationKey, RecordHeader,
    SessionEpoch, SessionPhase, StepReport, TransportPoll, TransportTicket,
};
use crate::input::{ClientIntentKind, InputAdmissionState, InputBatch, InputTranslator};
use crate::prediction::{PlayerProjectionState, PredictionReplay};
use crate::preparation::{LodConfig, LodStep, PreparationQueue};
use crate::preparation::{lod::LodSelection, near::NearScheduler};
use crate::presentation::AudioProjectionState;
use crate::presentation::assembly::{commit_publication, prepare_publication};
use crate::presentation::family_terrain::TerrainPublisher;
use crate::presentation::frame::{
    FamilyFrame, FamilyRecords, LifecycleRecord, PresentationFrame, SessionRecord,
};
use crate::presentation::{AcceptedObservation, BoundedText, LifecycleProjectionState, TextKind};
use crate::presentation::{
    MovementIntent, Pose, ProjectionView, PublicationConsumption, PublicationOwners,
};
use crate::session::io::TcpConnector;
use crate::session::lifecycle::{
    LifecycleInvalidation, LifecycleOwner, close_projection, open_projection, reset_projection,
};
use crate::session::mirror::MirrorProvider;
use mornlea_engine::native::contracts::{MeshRegistry, WorldgenParams};

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
    tcp: Arc<dyn Connector>,
    owner: LifecycleOwner,
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
    mirror: MirrorProvider,
    prediction: PredictionReplay,
    player_source: Option<(u64, Dimension, bool)>,
    effects_cursor: usize,
    world_seed: Option<u64>,
    admission: InputAdmissionState,
    /// Correlated journal facts remain owned until their whole publication commits.
    confirmations: Vec<(u64, ClientIntentKind, Option<u64>)>,
    limits: ClientLimits,
    lifecycle: LifecycleProjectionState,
    player_id: Option<PlayerId>,
    terminal: Option<CloseReason>,
    pending_terminal: Option<CloseReason>,
    audio: AudioProjectionState,
    preparation: PreparationQueue,
    near: NearScheduler,
    terrain: TerrainPublisher,
    registry: Arc<MeshRegistry>,
    far: Option<(LodSelection, Arc<WorldgenParams>)>,
    terrain_generation: u64,
    visible: Option<Arc<PresentationFrame>>,
    frame_index: u64,
    queue_high_water: crate::presentation::QueueCounters,
    /// Invalid wire observations remain visible in epoch diagnostics after consumption.
    rejected_inbound: u64,
}

impl LiveSession {
    fn select_terrain_dimension(&mut self, dimension: Dimension) -> Result<(), ClientError> {
        if self
            .far
            .as_ref()
            .is_some_and(|(selection, _)| selection.dimension() == dimension)
        {
            return Ok(());
        }
        let generation = self
            .terrain_generation
            .checked_add(1)
            .ok_or(ClientError::Capacity)?;
        let config = LodConfig::try_new(
            true,
            self.identity.login().view_distance,
            LodConfig::DEFAULT_FAR_MULTIPLIER,
            LodStep::try_new(4)?,
            4 << 20,
        )?;
        let params = crate::runtime_data::worldgen_params(
            self.world_seed.ok_or(ClientError::InvalidState)? as i64,
            dimension,
            true,
        )?;
        let selection = LodSelection::try_new(self.epoch, dimension, config, generation)?;
        self.far = Some((selection, params));
        self.terrain_generation = generation;
        Ok(())
    }

    fn player_projection(&self) -> Result<PlayerProjectionState, ClientError> {
        if self.player_source.is_some() {
            return self.prediction.projection();
        }
        // The pending context carries no player fact into an emitted family.
        PlayerProjectionState::try_new(
            self.epoch,
            self.mirror.mirror().revision(),
            Pose::try_new([0.0; 3], 0.0, 0.0)?,
            None,
            Vec::new(),
            None,
            None,
            MovementIntent::try_new(None, false)?,
        )
    }

    fn check_input_capacity(&self, count: usize) -> Result<(), ClientError> {
        let metadata = self.admission.projection();
        let held = metadata.admitted().len() + metadata.rejected().len() + self.confirmations.len();
        if count > self.limits.family_records().saturating_sub(held) {
            return Err(ClientError::Capacity);
        }
        Ok(())
    }

    fn reserve_confirmations(&mut self, count: usize) -> Result<(), ClientError> {
        self.check_input_capacity(count)?;
        self.confirmations
            .try_reserve(count)
            .map_err(|_| ClientError::Capacity)
    }

    fn advance_sources(&mut self) -> Result<(), ClientError> {
        self.prediction.begin_frame();
        while self.effects_cursor < self.mirror.observations().len() {
            let observation = &self.mirror.observations()[self.effects_cursor];
            let observation_key = *observation.key();
            let event = Event::try_from(observation.packet().clone())
                .map_err(|_| ClientError::InvalidInput)?;
            match event {
                Event::PlayerState(state)
                    if state.server_tick() > self.player_source.map_or(0, |source| source.0) =>
                {
                    if state.last_input_sequence() >= self.admission.next_sequence() {
                        return Err(ClientError::InvalidInput);
                    }
                    let confirmed: Vec<_> = self
                        .admission
                        .pending_acknowledgments(state.last_input_sequence())
                        .collect();
                    self.reserve_confirmations(confirmed.len())?;
                    self.prediction.apply_world(
                        &observation_key,
                        &state,
                        self.mirror.mirror().world(),
                    )?;
                    self.admission.acknowledge(state.last_input_sequence())?;
                    self.confirmations.extend(
                        confirmed
                            .into_iter()
                            .map(|(sequence, kind)| (sequence, kind, Some(state.server_tick()))),
                    );
                    self.player_source =
                        Some((state.server_tick(), state.dimension(), state.ready()));
                }
                Event::CommandRejected(rejection)
                    if self.admission.holds_sequence(rejection.sequence()) =>
                {
                    self.check_input_capacity(1)?;
                    let dimension = self
                        .player_source
                        .map_or(Dimension::OVERWORLD, |source| source.1);
                    let mut prediction = self.prediction.clone();
                    prediction.reject_world(
                        rejection.sequence(),
                        self.mirror.mirror().world(),
                        dimension,
                    )?;
                    self.admission
                        .reject(rejection.sequence(), ClientError::InvalidInput)?;
                    self.prediction = prediction;
                }
                Event::PlaceBlockSucceeded(success) => {
                    let sequence = success.sequence();
                    if self
                        .admission
                        .pending_acknowledgments(sequence)
                        .any(|(held, kind)| {
                            held == sequence && kind == ClientIntentKind::PlaceBlock
                        })
                    {
                        self.reserve_confirmations(1)?;
                        if let Some(kind) = self.admission.confirm_placement(sequence) {
                            self.confirmations.push((sequence, kind, None));
                        }
                    }
                }
                _ => {}
            }
            self.effects_cursor += 1;
        }
        if let Some((_, dimension, true)) = self.player_source {
            let mut last = None;
            for _ in 0..5 {
                let Some((sequence, control)) = self.admission.pending_control() else {
                    break;
                };
                let witness = self.prediction.predict_world(
                    sequence,
                    control,
                    self.mirror.mirror().world(),
                    dimension,
                )?;
                self.admission.mark_predicted(sequence)?;
                last = Some(witness);
            }
            if let Some(witness) = last {
                let position = witness.pose().position();
                let foot = BlockPos::new(
                    position[0].floor() as i32,
                    position[1].floor() as i32,
                    position[2].floor() as i32,
                );
                self.prediction
                    .observe_snow_step(self.mirror.mirror().world().block_at(dimension, foot))?;
            }
            self.prediction.refresh_world_targets(
                self.mirror.mirror().world(),
                dimension,
                self.mirror.mirror().revision(),
            )?;
        }
        Ok(())
    }
}

fn queue_control(session: &mut LiveSession, frame: Vec<u8>) -> Result<(), ClientError> {
    let count = session.outbound.len() + session.admission.outbound_records() + 1;
    let bytes = session
        .outbound
        .iter()
        .try_fold(session.admission.outbound_bytes(), |held, value| {
            held.checked_add(value.len())
        })
        .and_then(|held| held.checked_add(frame.len()))
        .ok_or(ClientError::Capacity)?;
    if count > session.limits.outbound_commands() || bytes > session.limits.outbound_bytes() {
        return Err(ClientError::Capacity);
    }
    session.outbound.push_back(frame);
    publication::observe_queues(session);
    Ok(())
}

impl LoginSession {
    /// Resolves only the live owning epoch; reset cannot resurrect an old geometry.
    pub fn prepared_resource(
        &self,
        key: &crate::preparation::PreparedResourceKey,
    ) -> Result<Arc<crate::preparation::PreparedGeometry>, ClientError> {
        let session = self.session.as_ref().ok_or(ClientError::InvalidInput)?;
        if key.epoch() != session.epoch {
            return Err(ClientError::StaleEpoch);
        }
        session.preparation.prepared_resource(key)
    }
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
            tcp: Arc::new(TcpConnector::new(
                NonZeroU64::MAX,
                *config.limits(),
                config.hello_timeout(),
            )?),
            owner: LifecycleOwner::try_new(*config.limits())?,
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
        let mirror = MirrorProvider::new(epoch, self.limits)?;
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
            prediction: PredictionReplay::try_new(epoch, self.limits)?,
            player_source: None,
            effects_cursor: 0,
            world_seed: None,
            admission,
            confirmations: Vec::new(),
            limits: self.limits,
            lifecycle,
            player_id: None,
            terminal: None,
            pending_terminal: None,
            audio: AudioProjectionState::try_new()?,
            preparation: PreparationQueue::new(self.limits),
            near: NearScheduler::new(epoch, self.limits),
            terrain: TerrainPublisher::try_new()?,
            registry: crate::runtime_data::mesh_registry()?,
            far: None,
            terrain_generation: 0,
            visible: None,
            frame_index: 0,
            queue_high_water: crate::presentation::QueueCounters::default(),
            rejected_inbound: 0,
        })
    }

    /// Commits all families together before retiring source cursors or terrain references.
    fn publish(&mut self, next_index: u64, work: ClientWorkBudget) -> Result<u16, ClientError> {
        let session = self.session.as_mut().ok_or(ClientError::InvalidState)?;
        publication::observe_queues(session);
        if let Err(error) = session.advance_sources() {
            // Consumed authority cannot disappear when its retained effect cannot fit.
            session.pending_terminal = Some(if error == ClientError::Capacity {
                CloseReason::Capacity
            } else {
                CloseReason::Internal
            });
            return Err(error);
        }
        let player = session.player_projection()?;
        let ready = session.player_source.is_some_and(|(_, _, ready)| ready);
        if let Some((_, dimension, _)) = session.player_source
            && (ready || session.far.is_some())
        {
            session.select_terrain_dimension(dimension)?;
        }
        let near_jobs = if ready {
            session.near.dispatch(
                session.mirror.mirror().world(),
                session.player_source.expect("authority").1,
                &session.registry,
                &mut session.terrain,
                &mut session.preparation,
                work,
            )?
        } else {
            0
        };
        publication::observe_queues(session);
        let diagnostics = publication::diagnostics(session)?;
        let view = ProjectionView::try_new(
            session.mirror.mirror(),
            session.mirror.observations(),
            session.admission.projection(),
            &player,
            &session.audio,
            &session.lifecycle,
            &diagnostics,
            session.epoch,
            session.mirror.mirror().revision(),
            next_index,
            &session.limits,
        )?;
        let proposal = if let Some((selection, params)) = session.far.as_mut() {
            let position = player
                .predicted_pose()
                .unwrap_or(player.confirmed_pose())
                .position();
            let center = ChunkPos::new(
                (position[0].floor() as i32).div_euclid(16),
                (position[2].floor() as i32).div_euclid(16),
            );
            Some(session.terrain.prepare_with_dispatch_limit(
                &view,
                &mut session.preparation,
                selection,
                center,
                params,
                work,
                if ready {
                    work.meshes() - u16::try_from(near_jobs).map_err(|_| ClientError::Internal)?
                } else {
                    0
                },
            )?)
        } else {
            None
        };
        let processed_meshes = proposal
            .as_ref()
            .map_or(0, |proposal| proposal.processed_meshes());
        let built = publication::complete(
            &view,
            session.phase,
            session.player_id,
            session.terminal.clone(),
            proposal
                .as_ref()
                .map_or_else(Vec::new, |proposal| proposal.records().to_vec()),
            session.lifecycle.pending().to_vec(),
            session.player_source.is_some(),
            &session.confirmations,
        )?;
        let consumption = PublicationConsumption::try_new(
            session.mirror.observations().len(),
            session.admission.projection().admitted().len()
                + session.admission.projection().rejected().len(),
            session.admission.pending_local_cues().len(),
            session.lifecycle.pending().len(),
        )?;
        let snow_count = player.predicted_snow_cues().len();
        let visible = session.visible.as_mut().ok_or(ClientError::InvalidState)?;
        let mut owners = PublicationOwners::try_new(
            visible,
            session.mirror.publication_observations_mut(),
            session.admission.publication_input(),
            &mut session.audio,
            &mut session.lifecycle,
        )?;
        let reservation = prepare_publication(
            &owners,
            built.frame,
            built.audio,
            consumption,
            &session.limits,
        )?;
        let frame = commit_publication(&mut owners, reservation);
        session.frame_index = frame.frame_index();
        session.effects_cursor = 0;
        session.confirmations.clear();
        if let Some(proposal) = proposal {
            session.terrain.commit(&mut session.preparation, proposal);
        }
        session
            .prediction
            .consume_snow_cues(snow_count)
            .expect("prevalidated cue prefix");
        publication::observe_queues(session);
        Ok(processed_meshes)
    }

    /// Prepares terminal output before the fallible release. A refused
    /// release retains the reason for a later retry without freezing a live frame.
    fn terminate(&mut self, reason: CloseReason) -> Result<(), ClientError> {
        let session = self.session.as_mut().ok_or(ClientError::InvalidState)?;
        session.pending_terminal = Some(reason.clone());
        let next = session
            .frame_index
            .checked_add(1)
            .ok_or(ClientError::Capacity)?;
        let mut records = session.lifecycle.pending().to_vec();
        records.extend(close_projection(
            session.epoch,
            session.mirror.mirror().revision(),
        )?);
        let frame = prepare_frame(
            session,
            SessionPhase::Closing,
            Some(reason.clone()),
            records,
            next,
            &self.limits,
        )?;
        let owners = LifecycleInvalidation::try_new(
            session.mirror.publication_observations_mut(),
            &mut session.admission,
            &mut session.audio,
            &mut session.lifecycle,
        )?;
        self.owner.terminate(
            session.epoch,
            reason.clone(),
            session.connector.as_ref(),
            owners,
            &mut session.preparation,
        )?;
        session.outbound.clear();
        session.confirmations.clear();
        session.phase = SessionPhase::Closing;
        session.terminal = Some(reason);
        session.pending_terminal = None;
        install_frame(session, frame);
        Ok(())
    }
}

impl ClientEndpoint for LoginSession {
    fn connect(
        &mut self,
        endpoint: Endpoint,
        identity: ClientIdentity,
    ) -> Result<SessionEpoch, ClientError> {
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.terminal.is_none())
        {
            return Err(ClientError::InvalidState);
        }
        let connector = match &endpoint {
            Endpoint::Memory { connector_id } => self
                .connectors
                .resolve(*connector_id)
                .ok_or(ClientError::InvalidInput)?,
            Endpoint::Tcp(_) => Arc::clone(&self.tcp),
        };
        // The hello frame is encoded before the transport is acquired, so an
        // encoding refusal can never leak a ticket.
        let epoch = self.owner.next_epoch()?;
        let hello = hello_frame_bytes()?;
        let ticket = connector.try_connect(&endpoint, &identity)?;
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
                if let Err(error) = queue_control(&mut session, hello) {
                    let _ = connector.close(ticket);
                    return Err(error);
                }
            }
            Err(error) => {
                let _ = session.connector.close(session.ticket);
                return Err(error);
            }
        }
        let frame = match prepare_frame(
            &session,
            session.phase,
            None,
            vec![open_projection(epoch)?],
            0,
            &self.limits,
        ) {
            Ok(frame) => frame,
            Err(error) => {
                let _ = connector.close(ticket);
                return Err(error);
            }
        };
        if let Err(error) = self
            .owner
            .open(ticket, &mut session.admission, &mut session.lifecycle)
        {
            let _ = connector.close(ticket);
            return Err(error);
        }
        install_frame(&mut session, frame);
        self.session = Some(session);
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
        let validated =
            InputTranslator::validate_batch(&input, session.mirror.mirror(), &session.limits)?;
        session.check_input_capacity(usize::from(validated.sequenced_count()))?;
        let control_count = session.outbound.len();
        let control_bytes: usize = session.outbound.iter().map(Vec::len).sum();
        let receipt = InputTranslator::commit_with_reserved(
            validated,
            &mut session.admission,
            control_count,
            control_bytes,
        )?;
        session
            .prediction
            .admit_through(session.admission.next_sequence() - 1)?;
        publication::observe_queues(session);
        Ok(receipt)
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
            if usize::from(work.messages()) > session.limits.message_work()
                || usize::from(work.meshes()) > session.limits.mesh_work()
            {
                return Err(ClientError::Capacity);
            }
            // The monotonic deadlines are checked at step entry, before any
            // frame is processed: a hello or login answer that only arrives
            // after its deadline cannot extend the phase, exactly as a
            // blocking read under the same deadline would fail instead.
            // Each deadline covers its own phase from the instant that phase
            // began, and an admitted session has none.
            let now = self.clock.now();
            let mut terminal = session
                .pending_terminal
                .clone()
                .or_else(|| match session.phase {
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
                });
            let mut processed = 0u16;
            if terminal.is_none() {
                // Retry the retained outbound head first; a still-refused
                // head stays queued for the next step.
                for _ in 0..work.messages() {
                    let control = session.outbound.front();
                    let frame = match control {
                        Some(frame) => frame.as_slice(),
                        None if session.phase == SessionPhase::Admitted => {
                            match session.admission.outbound_head_frame() {
                                Some(frame) => frame,
                                None => break,
                            }
                        }
                        None => break,
                    };
                    match session.connector.try_send(session.ticket, frame) {
                        Ok(()) => {
                            if control.is_some() {
                                session.outbound.pop_front();
                            } else {
                                session.admission.sent_head()?;
                            }
                        }
                        Err(ClientError::Capacity) | Err(ClientError::Io) => break,
                        Err(error) => {
                            terminal = Some(transport_close_reason(error)?);
                            break;
                        }
                    }
                }
                while terminal.is_none() && processed < work.messages() {
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
        let mut processed_meshes = 0;
        match terminal {
            Some(reason) => self.terminate(reason)?,
            None => {
                let next = self
                    .session
                    .as_ref()
                    .expect("live session")
                    .frame_index
                    .checked_add(1)
                    .ok_or(ClientError::Capacity)?;
                match self.publish(next, work) {
                    Ok(processed) => processed_meshes = processed,
                    Err(error) => {
                        let reason = self
                            .session
                            .as_ref()
                            .and_then(|live| live.pending_terminal.clone());
                        if let Some(reason) = reason {
                            self.terminate(reason)?;
                        } else {
                            return Err(error);
                        }
                    }
                }
            }
        }
        let session = self.session.as_ref().expect("live session");
        StepReport::try_new(
            session.epoch,
            session.mirror.mirror().revision(),
            session.frame_index,
            processed,
            processed_meshes,
            session.admission.outbound_records(),
            session.mirror.observations().len(),
            session.preparation.pending_jobs(),
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
        let session = self.session.as_mut().ok_or(ClientError::InvalidState)?;
        if session.epoch != epoch {
            return Err(ClientError::StaleEpoch);
        }
        if session.terminal.is_some() {
            return Err(ClientError::Disconnected);
        }
        self.owner.next_epoch()?;
        let next = session
            .frame_index
            .checked_add(1)
            .ok_or(ClientError::Capacity)?;
        let mut records = session.lifecycle.pending().to_vec();
        records.extend(reset_projection(epoch, session.mirror.mirror().revision())?);
        // The fixed facade returns the fresh epoch; the dying publication is
        // validated and consumed internally before a new generation opens.
        let dying = prepare_frame(session, session.phase, None, records, next, &self.limits)?;
        let owners = LifecycleInvalidation::try_new(
            session.mirror.publication_observations_mut(),
            &mut session.admission,
            &mut session.audio,
            &mut session.lifecycle,
        )?;
        self.owner.reset(
            epoch,
            session.connector.as_ref(),
            owners,
            &mut session.preparation,
        )?;
        session.outbound.clear();
        install_frame(session, dying);
        let endpoint = session.endpoint;
        let identity = session.identity.clone();
        self.session = None;
        self.connect(endpoint, identity)
    }

    fn close(&mut self) -> Result<(), ClientError> {
        let Some(session) = self.session.as_ref() else {
            return Ok(());
        };
        if session.terminal.is_some() {
            return Ok(());
        }
        self.terminate(
            session
                .pending_terminal
                .clone()
                .unwrap_or(CloseReason::LocalClose),
        )
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
            session.rejected_inbound = session.rejected_inbound.saturating_add(1);
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
            session.rejected_inbound = session.rejected_inbound.saturating_add(1);
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
                    if let Err(error) = queue_control(session, session.login_frame.clone()) {
                        if error == ClientError::Capacity {
                            *terminal = Some(CloseReason::Capacity);
                        } else {
                            return Err(error);
                        }
                    }
                }
                Err(error) => {
                    *terminal = Some(transport_close_reason(error)?);
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
                session.rejected_inbound = session.rejected_inbound.saturating_add(1);
                return Ok(());
            }
            session.phase = SessionPhase::Admitted;
            session.player_id = Some(success.player_id);
            session.mirror.admit()?;
            session.world_seed = Some(success.world_seed);
        }
        (SessionPhase::Handshaking, ServerPacket::LoginReject(reject)) => {
            *terminal = Some(CloseReason::LoginRejected(control_text(&reject.message)?));
        }
        (SessionPhase::Admitted, ServerPacket::Disconnect(disconnect)) => {
            *terminal = Some(CloseReason::RemoteDisconnect(control_text(
                &disconnect.message,
            )?));
        }
        (SessionPhase::Admitted, ServerPacket::KeepAlive(alive)) => {
            let reply = ClientPacket::KeepAliveReply(
                KeepAliveReply::new(alive.token).map_err(encode_failed)?,
            );
            let mut payload = [0; 8];
            let written = encode_client_into(&reply, &mut payload).map_err(encode_failed)?;
            let frame = write_frame(reply.key().id, &payload[..written]).map_err(encode_failed)?;
            if let Err(error) = queue_control(session, frame) {
                if error == ClientError::Capacity {
                    *terminal = Some(CloseReason::Capacity);
                } else {
                    return Err(error);
                }
            }
        }
        (SessionPhase::Admitted, packet) => match stage_observation(session, packet) {
            Ok(()) => {}
            // A complete authoritative packet already left the transport;
            // capacity ends this epoch explicitly rather than losing an update.
            Err(ClientError::Capacity) => *terminal = Some(CloseReason::Capacity),
            Err(ClientError::InvalidInput) => {
                session.rejected_inbound = session.rejected_inbound.saturating_add(1)
            }
            Err(error) => return Err(error),
        },
        _ => {
            session.rejected_inbound = session.rejected_inbound.saturating_add(1);
        }
    }
    Ok(())
}

/// Stages one decoded play packet as a pending accepted observation for the
/// mirror provider to consume. The observation key carries the next expected
/// mirror revision and the staged ordinal; source ticks and actor identity
/// resolution belong to that provider, so staging here is structural.
fn stage_observation(session: &mut LiveSession, packet: ServerPacket) -> Result<(), ClientError> {
    // Reject unissued source identities before they enter the retry-owned queue.
    match &packet {
        ServerPacket::PlayerState(state)
            if state.server_tick > session.player_source.map_or(0, |source| source.0)
                && state.last_input_sequence >= session.admission.next_sequence() =>
        {
            return Err(ClientError::InvalidInput);
        }
        ServerPacket::CommandRejected(rejection)
            if rejection.sequence >= session.admission.next_sequence() =>
        {
            return Err(ClientError::InvalidInput);
        }
        _ => {}
    }
    let revision = ConfirmedRevision::new(
        session
            .mirror
            .mirror()
            .revision()
            .get()
            .checked_add(1)
            .ok_or(ClientError::Capacity)?,
    );
    let ordinal =
        u32::try_from(session.mirror.observations().len()).map_err(|_| ClientError::Capacity)?;
    let key = ObservationKey::try_new(session.epoch, revision, ordinal)?;
    session.mirror.commit(&AcceptedObservation::try_new(
        key,
        None,
        packet,
        Vec::new(),
    )?)?;
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

/// Validates owned publication before transport release or state replacement.
fn prepare_frame(
    session: &LiveSession,
    phase: SessionPhase,
    terminal: Option<CloseReason>,
    records: Vec<LifecycleRecord>,
    next_index: u64,
    limits: &ClientLimits,
) -> Result<Arc<PresentationFrame>, ClientError> {
    let revision = session.mirror.mirror().revision();
    let header = RecordHeader::try_new(session.epoch, revision, None, FamilyOperation::Upsert)?;
    let record = SessionRecord::try_new(header, phase, session.player_id, terminal)?;
    let mut families = vec![FamilyFrame::try_new(
        FamilyKey::try_new(FAMILY_SESSION)?,
        FamilyRecords::Session(vec![record]),
    )?];
    for records in [
        FamilyRecords::Input(vec![]),
        FamilyRecords::Terrain(vec![]),
        FamilyRecords::Actors(vec![]),
        FamilyRecords::PlayerView(vec![]),
        FamilyRecords::InventoryUi(vec![]),
        FamilyRecords::WorldUi(vec![]),
        FamilyRecords::AudioCues(vec![]),
        FamilyRecords::Lifecycle(records),
        FamilyRecords::Diagnostics(vec![publication::diagnostic_record(session, next_index)?]),
    ] {
        families.push(FamilyFrame::try_new(records.key(), records)?);
    }
    let frame = PresentationFrame::try_new(session.epoch, revision, next_index, families)?;
    frame.validate(limits)?;
    Ok(Arc::new(frame))
}

/// Installs an already-validated publication and consumes its transitions.
fn install_frame(session: &mut LiveSession, frame: Arc<PresentationFrame>) {
    session.frame_index = frame.frame_index();
    session.visible = Some(frame);
    session.lifecycle =
        LifecycleProjectionState::try_new(session.epoch.get()).expect("checked generation");
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
