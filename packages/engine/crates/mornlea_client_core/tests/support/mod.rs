//! Deterministic test support for the four registered contract targets.
//!
//! This module owns the deterministic clock and transport doubles, the
//! replay harness and the deterministic session consumer double. The double
//! uses real checked constructors, the real `InputTranslator` admission path
//! and the real frame validator; its pending storage, mirror staging and
//! publication behavior stand in for the later provider owners and are
//! replaced by them node by node. It also carries the deliberately wrong
//! mode used for the behavioral reds: a non-atomic, mixed-frame consumer the
//! real validator must reject.

use std::collections::VecDeque;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mornlea_client_core::ClientIdentity;
use mornlea_client_core::contracts::{
    ClientConfig, ClientError, ClientLimits, ClientWorkBudget, ConfirmedRevision, Connector,
    ConnectorRegistry, Endpoint, FAMILY_SESSION, FAMILY_TERRAIN, FamilyKey, InputReceipt,
    MonotonicClock, ObservationKey, PreparationPort, RecordHeader, SessionEpoch, SessionPhase,
    StepReport, TransportLaunch, TransportPoll, TransportTicket,
};
use mornlea_client_core::input::{InputAdmissionState, InputBatch, InputTranslator};
use mornlea_client_core::preparation::{
    InvalidationReport, PreparationJob, PreparationResult, PreparationTicket, RejectedPreparation,
};
use mornlea_client_core::presentation::frame::{
    FamilyFrame, FamilyRecords, LifecycleRecord, LifecycleTransition, PresentationFrame,
    SessionRecord, TerrainRecord,
};
use mornlea_client_core::presentation::{AcceptedObservation, ResourceKey};
use mornlea_client_core::presentation::{InputReceiptState, InputRecord};
use mornlea_client_core::session::{ConfirmedMirror, ConfirmedMirrorParts};
use mornlea_protocol::{ProtocolCodec, ServerPacket, State, read_frame_ref};

/// Which consumer behavior the harness double runs: the checked contract
/// mode, or the deliberately wrong mode used to demonstrate the behavioral
/// reds (mixed-frame publication and non-atomic admission).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DoubleMode {
    Contract,
    Broken,
}

/// Deterministic monotonic clock. `now` advances only when the test advances
/// it, so deadline behavior is exact.
#[derive(Default)]
pub struct DeterministicClock {
    elapsed: Mutex<Duration>,
}

impl DeterministicClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn advance(&self, by: Duration) {
        let mut elapsed = self.elapsed.lock().expect("clock mutex");
        *elapsed += by;
    }
}

impl MonotonicClock for DeterministicClock {
    fn now(&self) -> Instant {
        Instant::now() + *self.elapsed.lock().expect("clock mutex")
    }
}

/// The scripted send policy of the memory connector double.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendPolicy {
    AlwaysOk,
    CapacityFailures(u32),
    IoFailures(u32),
}

struct MemoryInner {
    ticket: Option<TransportTicket>,
    launch: TransportLaunch,
    incoming: VecDeque<Vec<u8>>,
    sent: Vec<Vec<u8>>,
    policy: SendPolicy,
    closed: bool,
    releases: u32,
}

/// The deterministic memory transport double: scripted incoming frames,
/// observable sent frames and an injectable send failure policy.
pub struct MemoryConnectorDouble {
    inner: Mutex<MemoryInner>,
}

impl MemoryConnectorDouble {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(MemoryInner {
                ticket: None,
                launch: TransportLaunch::new(),
                incoming: VecDeque::new(),
                sent: Vec::new(),
                policy: SendPolicy::AlwaysOk,
                closed: false,
                releases: 0,
            }),
        }
    }

    pub fn feed_frame(&self, frame: Vec<u8>) {
        self.inner
            .lock()
            .expect("connector mutex")
            .incoming
            .push_back(frame);
    }

    pub fn set_policy(&self, policy: SendPolicy) {
        self.inner.lock().expect("connector mutex").policy = policy;
    }

    pub fn sent(&self) -> Vec<Vec<u8>> {
        self.inner.lock().expect("connector mutex").sent.clone()
    }

    pub fn releases(&self) -> u32 {
        self.inner.lock().expect("connector mutex").releases
    }
}

impl Default for MemoryConnectorDouble {
    fn default() -> Self {
        Self::new()
    }
}

impl Connector for MemoryConnectorDouble {
    fn try_connect(
        &self,
        _endpoint: &Endpoint,
        _identity: &ClientIdentity,
    ) -> Result<TransportTicket, ClientError> {
        let mut inner = self.inner.lock().expect("connector mutex");
        let generation = inner.launch.advance()?;
        let ticket = TransportTicket::try_new(NonZeroU64::new(1).expect("one"), generation)?;
        inner.ticket = Some(ticket);
        Ok(ticket)
    }

    fn poll(&self, ticket: TransportTicket) -> TransportPoll {
        let mut inner = self.inner.lock().expect("connector mutex");
        if inner.closed {
            return TransportPoll::Closed(ClientError::Disconnected);
        }
        if inner.ticket != Some(ticket) {
            return TransportPoll::Closed(ClientError::InvalidState);
        }
        match inner.incoming.pop_front() {
            Some(frame) => TransportPoll::Frame(frame),
            None => TransportPoll::Pending,
        }
    }

    fn try_send(&self, ticket: TransportTicket, frame: &[u8]) -> Result<(), ClientError> {
        let mut inner = self.inner.lock().expect("connector mutex");
        if inner.closed || inner.ticket != Some(ticket) {
            return Err(ClientError::InvalidState);
        }
        match inner.policy {
            SendPolicy::AlwaysOk => {}
            SendPolicy::CapacityFailures(remaining) => {
                if remaining > 0 {
                    inner.policy = SendPolicy::CapacityFailures(remaining - 1);
                    return Err(ClientError::Capacity);
                }
            }
            SendPolicy::IoFailures(remaining) => {
                if remaining > 0 {
                    inner.policy = SendPolicy::IoFailures(remaining - 1);
                    return Err(ClientError::Io);
                }
            }
        }
        inner.sent.push(frame.to_vec());
        Ok(())
    }

    fn close(&self, ticket: TransportTicket) -> Result<(), ClientError> {
        let mut inner = self.inner.lock().expect("connector mutex");
        let _ = ticket;
        inner.closed = true;
        inner.releases += 1;
        Ok(())
    }
}

/// Builds a registry with one memory connector under id 1.
pub fn registry_with_memory(connector: Arc<MemoryConnectorDouble>) -> Arc<ConnectorRegistry> {
    let mut registry = ConnectorRegistry::new();
    registry
        .register(NonZeroU64::new(1).expect("one"), connector)
        .expect("fresh registry");
    Arc::new(registry)
}

/// Encodes one server packet into a complete prefix-inclusive v45 frame. The
/// concrete packet encoder is the family's own gate; this helper only sizes
/// and frames the payload, so no second encoder exists.
pub fn frame_server_packet(packet: &ServerPacket) -> Vec<u8> {
    let payload = encode_server_payload(packet);
    mornlea_protocol::write_frame(packet.key().id, &payload).expect("fixture frame fits")
}

fn encode_server_payload(packet: &ServerPacket) -> Vec<u8> {
    use mornlea_protocol::MAX_SMALL_PAYLOAD_BYTES;
    let mut buffer = vec![0u8; MAX_SMALL_PAYLOAD_BYTES + 16];
    let written = match packet {
        ServerPacket::ServerHello(record) => record.encode_into(&mut buffer),
        ServerPacket::HandshakeReject(record) => record.encode_into(&mut buffer),
        ServerPacket::LoginSuccess(record) => record.encode_into(&mut buffer),
        ServerPacket::LoginReject(record) => record.encode_into(&mut buffer),
        ServerPacket::BlockChanges(record) => record.encode_into(&mut buffer),
        ServerPacket::ForgetChunks(record) => record.encode_into(&mut buffer),
        ServerPacket::PlayerState(record) => record.encode_into(&mut buffer),
        ServerPacket::CommandRejected(record) => record.encode_into(&mut buffer),
        ServerPacket::KeepAlive(record) => record.encode_into(&mut buffer),
        ServerPacket::Disconnect(record) => record.encode_into(&mut buffer),
        ServerPacket::RemotePlayerSpawn(record) => record.encode_into(&mut buffer),
        ServerPacket::RemotePlayerDespawn(record) => record.encode_into(&mut buffer),
        ServerPacket::RemotePlayerStates(record) => record.encode_into(&mut buffer),
        ServerPacket::InventoryState(record) => record.encode_into(&mut buffer),
        ServerPacket::ItemDropUpserts(record) => record.encode_into(&mut buffer),
        ServerPacket::ItemDropRemoves(record) => record.encode_into(&mut buffer),
        ServerPacket::FurnaceState(record) => record.encode_into(&mut buffer),
        ServerPacket::ContainerClosed(record) => record.encode_into(&mut buffer),
        ServerPacket::ChestState(record) => record.encode_into(&mut buffer),
        ServerPacket::ChatEvent(record) => record.encode_into(&mut buffer),
        ServerPacket::CompanionSpawn(record) => record.encode_into(&mut buffer),
        ServerPacket::CompanionStates(record) => record.encode_into(&mut buffer),
        ServerPacket::CompanionDespawn(record) => record.encode_into(&mut buffer),
        ServerPacket::PlaceBlockSucceeded(record) => record.encode_into(&mut buffer),
        ServerPacket::CraftingState(record) => record.encode_into(&mut buffer),
        ServerPacket::HostileSpawn(record) => record.encode_into(&mut buffer),
        ServerPacket::HostileState(record) => record.encode_into(&mut buffer),
        ServerPacket::HostileDespawn(record) => record.encode_into(&mut buffer),
        ServerPacket::CombatHit(record) => record.encode_into(&mut buffer),
        ServerPacket::PassiveSpawn(record) => record.encode_into(&mut buffer),
        ServerPacket::PassiveState(record) => record.encode_into(&mut buffer),
        ServerPacket::PassiveDespawn(record) => record.encode_into(&mut buffer),
        ServerPacket::ProjectileSpawn(record) => record.encode_into(&mut buffer),
        ServerPacket::ProjectileState(record) => record.encode_into(&mut buffer),
        ServerPacket::ProjectileDespawn(record) => record.encode_into(&mut buffer),
        ServerPacket::ChunkSnapshot(_) => {
            panic!("chunk snapshots route through the codec's owned decompressor")
        }
    }
    .expect("fixture packet encodes");
    buffer.truncate(written);
    buffer
}

/// The deterministic session consumer double.
///
/// In `Contract` mode it is atomic and publishes only validator-checked
/// frames: observation application runs on staged copies, and the visible
/// Arc, frame index, pending queues and revision only change when the whole
/// candidate validates. In `Broken` mode it is the deliberately wrong double:
/// it publishes mixed-revision frames without validation and advances the
/// sequence even when a batch is rejected. Provider tests replace this double
/// with the real provider.
pub struct ContractDouble {
    mode: DoubleMode,
    limits: ClientLimits,
    codec: ProtocolCodec,
    connector: Option<Arc<MemoryConnectorDouble>>,
    ticket: Option<TransportTicket>,
    epoch_counter: u64,
    epoch: Option<SessionEpoch>,
    phase: SessionPhase,
    revision: u64,
    frame_index: u64,
    visible: Option<Arc<PresentationFrame>>,
    admission: Option<InputAdmissionState>,
    mirror: Option<ConfirmedMirror>,
    pending: Vec<AcceptedObservation>,
    receive_buffer: Vec<u8>,
    session_phase_published: SessionPhase,
    lifecycle_open_pending: bool,
    pending_removals: Vec<(
        mornlea_domain::Dimension,
        mornlea_domain::ChunkPos,
        Option<u64>,
    )>,
    pending_input: Vec<(Option<u64>, mornlea_client_core::input::ClientIntentKind)>,
    terminal: Option<mornlea_client_core::contracts::CloseReason>,
    frame_cap_override: Option<usize>,
    broken_sequence: u64,
    pending_dedup: mornlea_client_core::presentation::AudioDedupDelta,
    committed_dedup: Vec<mornlea_client_core::presentation::AudioDedupKey>,
}

impl ContractDouble {
    pub fn new(mode: DoubleMode, limits: ClientLimits) -> Self {
        Self {
            mode,
            limits,
            codec: ProtocolCodec::new().expect("codec scratch"),
            connector: None,
            ticket: None,
            epoch_counter: 0,
            epoch: None,
            phase: SessionPhase::Disconnected,
            revision: 0,
            frame_index: 0,
            visible: None,
            admission: None,
            mirror: None,
            pending: Vec::new(),
            receive_buffer: Vec::new(),
            session_phase_published: SessionPhase::Connecting,
            lifecycle_open_pending: false,
            pending_removals: Vec::new(),
            pending_input: Vec::new(),
            terminal: None,
            frame_cap_override: None,
            broken_sequence: 1,
            pending_dedup: mornlea_client_core::presentation::AudioDedupDelta::try_new(
                Vec::new(),
                Vec::new(),
            )
            .expect("empty delta"),
            committed_dedup: Vec::new(),
        }
    }

    /// The registered memory connector for release-count inspection.
    pub fn with_connector(mut self, connector: Arc<MemoryConnectorDouble>) -> Self {
        self.connector = Some(connector);
        self
    }

    /// The checked pending-connection constructor: a nonzero epoch at
    /// revision zero with a Connecting session record, one lifecycle Open
    /// record and no world state.
    pub fn connect(
        &mut self,
        _endpoint: mornlea_client_core::contracts::Endpoint,
        identity: ClientIdentity,
    ) -> Result<SessionEpoch, ClientError> {
        if let Some(connector) = &self.connector {
            let endpoint = mornlea_client_core::contracts::Endpoint::Memory {
                connector_id: NonZeroU64::new(1).expect("one"),
            };
            self.ticket = Some(connector.try_connect(&endpoint, &identity)?);
        }
        self.epoch_counter += 1;
        let epoch = SessionEpoch::try_new(self.epoch_counter)?;
        self.phase = SessionPhase::Connecting;
        self.revision = 0;
        self.admission = Some(InputAdmissionState::try_new(epoch, self.limits)?);
        self.mirror = Some(ConfirmedMirror::try_new(ConfirmedMirrorParts::pending(
            epoch,
        ))?);
        self.session_phase_published = SessionPhase::Connecting;
        self.lifecycle_open_pending = true;
        self.epoch = Some(epoch);
        // The pending frame publishes at index zero: revision zero carries
        // only session, lifecycle, diagnostics and local input records.
        let mirror = self.mirror.clone().expect("connected");
        let candidate =
            self.build_candidate(epoch, ConfirmedRevision::new(0), &mirror, &[], 0, false)?;
        candidate.validate(&self.limits)?;
        self.visible = Some(Arc::new(candidate));
        Ok(epoch)
    }

    /// The checked admitted-session transition the double's input cases use.
    /// The real login path is the login provider's owner.
    pub fn admit(&mut self) -> Result<(), ClientError> {
        let epoch = self.epoch.expect("connected");
        self.phase = SessionPhase::Admitted;
        self.session_phase_published = SessionPhase::Admitted;
        self.mirror = Some(ConfirmedMirror::try_new(ConfirmedMirrorParts {
            epoch,
            revision: ConfirmedRevision::new(self.revision),
            phase: SessionPhase::Admitted,
            world: None,
            actors: None,
            inventory: None,
            world_ui: None,
        })?);
        Ok(())
    }

    /// Feeds arbitrary receive bytes; complete frames are staged as accepted
    /// observations, fragmented input is retained until it completes.
    pub fn receive_bytes(&mut self, bytes: &[u8]) -> Result<usize, ClientError> {
        self.receive_buffer.extend_from_slice(bytes);
        let mut consumed = 0usize;
        let epoch = self.epoch.expect("connected");
        let state = match self.phase {
            SessionPhase::Connecting => State::Handshake,
            _ => State::Play,
        };
        loop {
            let frame = match read_frame_ref(&self.receive_buffer) {
                Ok(frame) => frame,
                Err(_) => break,
            };
            let packet = self
                .codec
                .decode_server(state, frame.packet_id, frame.payload)
                .map_err(|_| ClientError::InvalidInput)?;
            let consumed_bytes = frame.consumed;
            self.receive_buffer.drain(..consumed_bytes);
            self.stage_observation(epoch, packet)?;
            consumed += consumed_bytes;
        }
        Ok(consumed)
    }

    /// Stages one decoded packet as a complete accepted observation with its
    /// actual source key and optional tick.
    pub fn stage_observation(
        &mut self,
        epoch: SessionEpoch,
        packet: ServerPacket,
    ) -> Result<(), ClientError> {
        let revision = ConfirmedRevision::new(self.revision + 1);
        let ordinal = self.pending.len() as u32;
        let key = ObservationKey::try_new(epoch, revision, ordinal)?;
        let source_tick = packet_source_tick(&packet);
        self.pending.push(AcceptedObservation::try_new(
            key,
            source_tick,
            packet,
            Vec::new(),
        )?);
        Ok(())
    }

    /// Drains the staged observations into one committed mirror revision and
    /// one validated visible frame. Observation application runs on staged
    /// copies, so a validation failure leaves every owner, the pending
    /// observations, the frame index and the old Arc untouched. In `Broken`
    /// mode the candidate carries mixed revision headers and skips
    /// validation.
    pub fn step(&mut self, work: ClientWorkBudget) -> Result<StepReport, ClientError> {
        let epoch = self.epoch.expect("connected");
        let mut processed = 0u16;
        let mut staged_mirror = self.mirror.clone().expect("connected");
        let mut staged_removals = self.pending_removals.clone();
        let mut staged_revision = self.revision;
        while let Some(observation) = self.pending.get(processed as usize).cloned() {
            if processed >= work.messages() {
                break;
            }
            if self.mode == DoubleMode::Contract {
                apply_observation(
                    &mut staged_mirror,
                    &observation,
                    ConfirmedRevision::new(staged_revision + 1),
                    &mut staged_removals,
                )?;
            }
            staged_revision += 1;
            processed += 1;
        }
        let candidate_revision = ConfirmedRevision::new(staged_revision);
        let candidate = self.build_candidate(
            epoch,
            candidate_revision,
            &staged_mirror,
            &staged_removals,
            self.frame_index + 1,
            self.mode == DoubleMode::Broken,
        )?;
        if self.mode == DoubleMode::Contract {
            let effective = self.effective_limits()?;
            candidate.validate(&effective)?;
        }
        // The single atomic commit: staged state becomes visible exactly once
        // and every proposal the candidate carried is consumed exactly once,
        // including the proposed audio dedup delta.
        self.mirror = Some(staged_mirror);
        self.revision = staged_revision;
        self.pending.drain(..processed as usize);
        self.pending_removals.clear();
        self.pending_input.clear();
        self.lifecycle_open_pending = false;
        self.commit_dedup_proposal();
        self.visible = Some(Arc::new(candidate));
        self.frame_index += 1;
        Ok(StepReport::try_new(
            epoch,
            candidate_revision,
            self.frame_index,
            processed,
            0,
            self.admission.as_ref().map_or(0, |a| a.outbound_records()),
            self.pending.len(),
            0,
            self.terminal.clone(),
        )?)
    }

    /// The visible immutable frame, or an invalid-state error before the
    /// first publication.
    pub fn snapshot(&self, epoch: SessionEpoch) -> Result<Arc<PresentationFrame>, ClientError> {
        if self.epoch != Some(epoch) {
            return Err(ClientError::StaleEpoch);
        }
        self.visible.clone().ok_or(ClientError::InvalidState)
    }

    /// The real whole-batch admission path: read-only validation, then the
    /// single-owner atomic commit into the admission owner. In `Broken` mode
    /// a rejected batch still advances the sequence witness.
    pub fn submit_input(
        &mut self,
        _epoch: SessionEpoch,
        batch: InputBatch,
    ) -> Result<InputReceipt, ClientError> {
        let mirror = self.mirror.clone().expect("connected");
        let demanded = batch
            .actions()
            .iter()
            .filter(|action| {
                !matches!(
                    action.intent,
                    mornlea_client_core::input::ClientIntent::Chat(_)
                )
            })
            .count() as u64;
        let admitted =
            InputTranslator::validate_batch(&batch, &mirror, &self.limits).and_then(|validated| {
                let sequenced = u64::from(validated.sequenced_count());
                let admission = self.admission.as_mut().expect("connected");
                // The frozen two-argument commit: the validated batch carries
                // its validate-time mirror snapshot and the admission owner
                // carries its frozen limits.
                InputTranslator::commit(validated, admission).map(|receipt| (receipt, sequenced))
            });
        match admitted {
            Ok((receipt, sequenced)) => {
                if self.mode == DoubleMode::Contract {
                    self.queue_input_records(&batch, &receipt)?;
                }
                self.broken_sequence += sequenced;
                Ok(receipt)
            }
            Err(error) => {
                if self.mode == DoubleMode::Broken {
                    // The deliberate non-atomic wrong behavior: the sequence
                    // advances even though the batch was rejected.
                    self.broken_sequence += demanded.max(1);
                }
                Err(error)
            }
        }
    }

    /// Resets to a fresh epoch; every epoch-scoped owner is cleared and old
    /// work cannot cross.
    pub fn reset(&mut self, epoch: SessionEpoch) -> Result<SessionEpoch, ClientError> {
        if self.epoch != Some(epoch) {
            return Err(ClientError::StaleEpoch);
        }
        let next = SessionEpoch::try_new(self.epoch_counter + 1)?;
        self.epoch_counter += 1;
        self.phase = SessionPhase::Connecting;
        self.revision = 0;
        self.admission = Some(InputAdmissionState::try_new(next, self.limits)?);
        self.mirror = Some(ConfirmedMirror::try_new(ConfirmedMirrorParts::pending(
            next,
        ))?);
        self.pending.clear();
        self.receive_buffer.clear();
        self.pending_removals.clear();
        self.pending_input.clear();
        self.session_phase_published = SessionPhase::Connecting;
        self.lifecycle_open_pending = true;
        self.epoch = Some(next);
        Ok(next)
    }

    /// Closes the session and releases the transport exactly once.
    pub fn close(&mut self) -> Result<(), ClientError> {
        self.phase = SessionPhase::Closing;
        self.terminal = Some(mornlea_client_core::contracts::CloseReason::LocalClose);
        if let Some(connector) = &self.connector {
            if let Some(ticket) = self.ticket {
                connector.close(ticket)?;
            }
        }
        Ok(())
    }

    // --- read-only inspection used by the named cases ---

    pub fn epoch(&self) -> Option<SessionEpoch> {
        self.epoch
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    pub fn mirror(&self) -> Option<&ConfirmedMirror> {
        self.mirror.as_ref()
    }

    pub fn admission(&self) -> Option<&InputAdmissionState> {
        self.admission.as_ref()
    }

    pub fn pending_observations(&self) -> usize {
        self.pending.len()
    }

    pub fn limits(&self) -> &ClientLimits {
        &self.limits
    }

    pub fn mode(&self) -> DoubleMode {
        self.mode
    }

    pub fn pending_removals(
        &self,
    ) -> &[(
        mornlea_domain::Dimension,
        mornlea_domain::ChunkPos,
        Option<u64>,
    )] {
        &self.pending_removals
    }

    /// The sequence witness the input cases inspect: the real admission
    /// owner's next sequence in contract mode, the broken double's own
    /// counter in broken mode.
    pub fn sequence_witness(&self) -> u64 {
        match self.mode {
            DoubleMode::Contract => self.admission.as_ref().map_or(
                1,
                mornlea_client_core::input::InputAdmissionState::next_sequence,
            ),
            DoubleMode::Broken => self.broken_sequence,
        }
    }

    /// The size the next publication would carry: the candidate built from
    /// the current staged state.
    pub fn candidate_size_hint(&self) -> usize {
        let epoch = self.epoch.expect("connected");
        let mirror = self.mirror.clone().expect("connected");
        let candidate = self
            .build_candidate(
                epoch,
                ConfirmedRevision::new(self.revision + 1),
                &mirror,
                &self.pending_removals,
                self.frame_index + 1,
                self.mode == DoubleMode::Broken,
            )
            .expect("candidate");
        candidate.validated_size().expect("checked size")
    }

    /// Test-only frame cap override: the next publication validates against
    /// a copy of the frozen limits whose frame byte cap is `cap`.
    pub fn set_frame_cap_for_test(&mut self, cap: usize) {
        self.frame_cap_override = Some(cap);
    }

    /// Stages one proposed audio dedup delta. A failed publication retains it
    /// unchanged; a successful publication commits its insertions exactly
    /// once and consumes the proposal.
    pub fn stage_dedup_proposal(
        &mut self,
        delta: mornlea_client_core::presentation::AudioDedupDelta,
    ) {
        self.pending_dedup = delta;
    }

    /// The staged, not yet committed, dedup proposal.
    pub fn pending_dedup_proposal(&self) -> &mornlea_client_core::presentation::AudioDedupDelta {
        &self.pending_dedup
    }

    /// The committed epoch-scoped dedup keys.
    pub fn committed_dedup(&self) -> &[mornlea_client_core::presentation::AudioDedupKey] {
        &self.committed_dedup
    }

    fn commit_dedup_proposal(&mut self) {
        let delta = std::mem::replace(
            &mut self.pending_dedup,
            mornlea_client_core::presentation::AudioDedupDelta::try_new(Vec::new(), Vec::new())
                .expect("empty delta"),
        );
        for key in delta.cancellations() {
            self.committed_dedup.retain(|committed| committed != key);
        }
        self.committed_dedup
            .extend(delta.insertions().iter().copied());
    }

    /// Stages one confirmed container view at the current revision, the
    /// checked stand-in for a confirmed reopened view.
    pub fn stage_inventory_for_test(
        &mut self,
        inventory: mornlea_client_core::session::InventoryConfirmed,
    ) {
        let epoch = self.epoch.expect("connected");
        let mirror = self.mirror.clone().expect("connected");
        self.mirror = ConfirmedMirror::try_new(ConfirmedMirrorParts {
            epoch,
            revision: ConfirmedRevision::new(self.revision),
            phase: *mirror.phase(),
            world: Some(mirror.world().clone()),
            actors: Some(mirror.actors().clone()),
            inventory: Some(inventory),
            world_ui: Some(mirror.world_ui().clone()),
        })
        .ok();
    }

    // --- internals ---

    fn effective_limits(&self) -> Result<ClientLimits, ClientError> {
        match self.frame_cap_override {
            Some(cap) => {
                let limits = self.limits;
                ClientLimits::try_new_with(
                    limits.queued_input_events(),
                    limits.inbound_observations(),
                    limits.inbound_bytes(),
                    limits.outbound_commands(),
                    limits.outbound_bytes(),
                    limits.prediction_journal(),
                    limits.message_work(),
                    limits.mesh_work(),
                    limits.preparation_results(),
                    limits.preparation_bytes(),
                    limits.family_records(),
                    cap,
                )
            }
            None => Ok(self.limits),
        }
    }

    fn queue_input_records(
        &mut self,
        batch: &InputBatch,
        receipt: &InputReceipt,
    ) -> Result<(), ClientError> {
        let InputReceipt::Queued { first_sequence, .. } = receipt else {
            return Ok(());
        };
        let mut sequence = first_sequence.unwrap_or(0);
        for action in batch.actions() {
            let sequenced = !matches!(
                action.intent,
                mornlea_client_core::input::ClientIntent::Chat(_)
            );
            let local_sequence = if sequenced {
                let value = sequence;
                sequence += 1;
                Some(value)
            } else {
                None
            };
            self.pending_input
                .push((local_sequence, action.intent.kind()));
        }
        Ok(())
    }

    #[allow(clippy::needless_pass_by_ref)]
    fn build_candidate(
        &self,
        epoch: SessionEpoch,
        revision: ConfirmedRevision,
        _mirror: &ConfirmedMirror,
        removals: &[(
            mornlea_domain::Dimension,
            mornlea_domain::ChunkPos,
            Option<u64>,
        )],
        next_index: u64,
        mixed: bool,
    ) -> Result<PresentationFrame, ClientError> {
        use mornlea_client_core::contracts::FamilyOperation;
        let upsert = FamilyOperation::Upsert;
        let mut families = Vec::new();

        // The session record is rebased onto the coherent candidate revision.
        let header = RecordHeader::try_new(epoch, revision, None, upsert)?;
        families.push(FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_SESSION)?,
            FamilyRecords::Session(vec![SessionRecord::try_new(
                header,
                self.session_phase_published,
                None,
                None,
            )?]),
        )?);

        if !self.pending_input.is_empty() {
            let records = self
                .pending_input
                .iter()
                .map(|(sequence, kind)| {
                    InputRecord::try_new(header, *sequence, *kind, InputReceiptState::Queued)
                })
                .collect::<Result<Vec<_>, _>>()?;
            families.push(FamilyFrame::try_new(
                FamilyKey::try_new(mornlea_client_core::contracts::FAMILY_INPUT)?,
                FamilyRecords::Input(records),
            )?);
        }
        if !removals.is_empty() {
            let records = removals
                .iter()
                .map(|(dimension, chunk, tick)| {
                    let header =
                        RecordHeader::try_new(epoch, revision, *tick, FamilyOperation::Remove)?;
                    TerrainRecord::try_new(
                        header,
                        *dimension,
                        mornlea_client_core::preparation::TerrainKey::Section(
                            mornlea_client_core::preparation::SectionKey::try_new(*chunk, 0)?,
                        ),
                        0,
                        0,
                        mornlea_client_core::presentation::frame::TerrainMaterial::Opaque,
                        mornlea_client_core::preparation::TerrainVisibility::Near,
                        mornlea_client_core::preparation::LightSummary::try_new(15, 0)?,
                        None,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            families.push(FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_TERRAIN)?,
                FamilyRecords::Terrain(records),
            )?);
        }
        if self.lifecycle_open_pending {
            families.push(FamilyFrame::try_new(
                FamilyKey::try_new(mornlea_client_core::contracts::FAMILY_LIFECYCLE)?,
                FamilyRecords::Lifecycle(vec![LifecycleRecord::try_new(
                    header,
                    LifecycleTransition::Open,
                    1,
                    vec![
                        ResourceKey::InputJournal,
                        ResourceKey::PreparationQueue,
                        ResourceKey::PresentationFrames,
                    ],
                )?]),
            )?);
        }
        if mixed {
            // The deliberate mixed-frame wrong behavior: the session
            // family's records carry a revision ahead of the frame parent.
            let mixed_header = RecordHeader::try_new(
                epoch,
                ConfirmedRevision::new(revision.get() + 1),
                None,
                upsert,
            )?;
            families[0] = FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_SESSION)?,
                FamilyRecords::Session(vec![SessionRecord::try_new(
                    mixed_header,
                    self.session_phase_published,
                    None,
                    None,
                )?]),
            )?;
        }
        PresentationFrame::try_new(epoch, revision, next_index, families)
    }
}

/// The optional server tick a packet carries. Valid packets without a tick —
/// including `ForgetChunks` — keep `None`; the mirror assigns the revision
/// and transport order without inventing one.
fn packet_source_tick(packet: &ServerPacket) -> Option<u64> {
    let event = mornlea_domain::Event::try_from(packet.clone()).ok()?;
    match event {
        mornlea_domain::Event::RemotePlayerSpawn(event) => Some(event.server_tick()),
        mornlea_domain::Event::RemotePlayerStates(event) => Some(event.server_tick()),
        mornlea_domain::Event::HostileSpawn(event) => Some(event.server_tick()),
        mornlea_domain::Event::HostileState(event) => Some(event.server_tick()),
        mornlea_domain::Event::HostileDespawn(event) => Some(event.server_tick()),
        mornlea_domain::Event::PassiveSpawn(event) => Some(event.server_tick()),
        mornlea_domain::Event::PassiveState(event) => Some(event.server_tick()),
        mornlea_domain::Event::PassiveDespawn(event) => Some(event.server_tick()),
        mornlea_domain::Event::ProjectileSpawn(event) => Some(event.server_tick()),
        mornlea_domain::Event::ProjectileState(event) => Some(event.server_tick()),
        mornlea_domain::Event::CompanionSpawn(event) => Some(event.server_tick()),
        mornlea_domain::Event::CompanionStates(event) => Some(event.server_tick()),
        mornlea_domain::Event::ItemDropUpserts(event) => Some(event.server_tick()),
        _ => None,
    }
}

/// Applies one complete accepted observation to a staged confirmed mirror.
/// The staging here is the deterministic stand-in for the mirror provider:
/// it keeps container, crafting, actor, chunk and chat facts the named cases
/// inspect, and invents nothing else.
fn apply_observation(
    mirror: &mut ConfirmedMirror,
    observation: &AcceptedObservation,
    revision: ConfirmedRevision,
    removals: &mut Vec<(
        mornlea_domain::Dimension,
        mornlea_domain::ChunkPos,
        Option<u64>,
    )>,
) -> Result<(), ClientError> {
    use mornlea_domain::Event;

    let event =
        Event::try_from(observation.packet().clone()).map_err(|_| ClientError::InvalidInput)?;
    let inventory = mirror.inventory().clone();
    let actors = mirror.actors().clone();
    let mut world = mirror.world().clone();
    let world_ui = mirror.world_ui().clone();
    match event {
        Event::ChestState(state) => {
            let inventory = inventory.with_container_view(state.container(), revision);
            *mirror = ConfirmedMirror::try_new(ConfirmedMirrorParts {
                epoch: mirror.epoch(),
                revision,
                phase: *mirror.phase(),
                world: Some(world),
                actors: Some(actors),
                inventory: Some(inventory),
                world_ui: Some(world_ui),
            })?;
        }
        Event::ContainerClosed(closed) => {
            let mut inventory = inventory;
            inventory.retire_container(&closed.container());
            *mirror = ConfirmedMirror::try_new(ConfirmedMirrorParts {
                epoch: mirror.epoch(),
                revision,
                phase: *mirror.phase(),
                world: Some(world),
                actors: Some(actors),
                inventory: Some(inventory),
                world_ui: Some(world_ui),
            })?;
        }
        Event::CraftingState(state) => {
            let inventory = inventory.with_crafting_view(state.size(), revision);
            *mirror = ConfirmedMirror::try_new(ConfirmedMirrorParts {
                epoch: mirror.epoch(),
                revision,
                phase: *mirror.phase(),
                world: Some(world),
                actors: Some(actors),
                inventory: Some(inventory),
                world_ui: Some(world_ui),
            })?;
        }
        Event::RemotePlayerSpawn(spawn) => {
            let actors = actors.with_remote_player(spawn.player_id(), spawn.dimension());
            *mirror = ConfirmedMirror::try_new(ConfirmedMirrorParts {
                epoch: mirror.epoch(),
                revision,
                phase: *mirror.phase(),
                world: Some(world),
                actors: Some(actors),
                inventory: Some(inventory),
                world_ui: Some(world_ui),
            })?;
        }
        Event::HostileSpawn(spawn) => {
            let mut staged = actors;
            for record in spawn.spawns() {
                staged = staged.with_hostile(record.id(), record.dimension());
            }
            *mirror = ConfirmedMirror::try_new(ConfirmedMirrorParts {
                epoch: mirror.epoch(),
                revision,
                phase: *mirror.phase(),
                world: Some(world),
                actors: Some(staged),
                inventory: Some(inventory),
                world_ui: Some(world_ui),
            })?;
        }
        Event::Chat(event) => {
            let world_ui = world_ui.with_chat(event);
            *mirror = ConfirmedMirror::try_new(ConfirmedMirrorParts {
                epoch: mirror.epoch(),
                revision,
                phase: *mirror.phase(),
                world: Some(world),
                actors: Some(actors),
                inventory: Some(inventory),
                world_ui: Some(world_ui),
            })?;
        }
        Event::ForgetChunks(forget) => {
            for chunk in forget.chunks() {
                world.remove_chunk(forget.dimension(), *chunk);
                removals.push((forget.dimension(), *chunk, observation.source_tick()));
            }
            *mirror = ConfirmedMirror::try_new(ConfirmedMirrorParts {
                epoch: mirror.epoch(),
                revision,
                phase: *mirror.phase(),
                world: Some(world),
                actors: Some(actors),
                inventory: Some(inventory),
                world_ui: Some(world_ui),
            })?;
        }
        _ => {}
    }
    Ok(())
}

/// The deterministic delayed-completion preparation double.
///
/// A submitted job is admitted with the same charge accounting as the port
/// double, but it completes only when the harness advances it: `advance`
/// moves up to `count` FIFO pending jobs into the completed results queue, so
/// a test scripts the exact completion timing with no sleeps and no
/// scheduling dependence. `poll_ready` transfers one FIFO completed result
/// and debits its queue ownership; `invalidate` releases stale work exactly
/// once.
pub struct PreparationFaucet {
    limits: ClientLimits,
    pending: std::collections::VecDeque<(PreparationTicket, PreparationJob, usize)>,
    completed: std::collections::VecDeque<(PreparationResult, usize)>,
    next_ticket: u64,
    tickets_issued: u64,
    charge: usize,
    seen_registries: Vec<usize>,
}

impl PreparationFaucet {
    pub fn new(limits: ClientLimits) -> Self {
        Self {
            limits,
            pending: std::collections::VecDeque::new(),
            completed: std::collections::VecDeque::new(),
            next_ticket: 1,
            tickets_issued: 0,
            charge: 0,
            seen_registries: Vec::new(),
        }
    }

    pub fn pending_preparations(&self) -> usize {
        self.pending.len()
    }

    pub fn completed_preparations(&self) -> usize {
        self.completed.len()
    }

    pub fn tickets_issued(&self) -> u64 {
        self.tickets_issued
    }

    pub fn charge(&self) -> usize {
        self.charge
    }

    fn job_charge(&mut self, job: &PreparationJob) -> usize {
        match job.payload() {
            mornlea_client_core::preparation::PreparationPayload::Near(near) => {
                let identity = Arc::as_ptr(near.registry()) as *const () as usize;
                let shared = if self.seen_registries.contains(&identity) {
                    0
                } else {
                    self.seen_registries.push(identity);
                    near.registry().entries().len()
                        * std::mem::size_of::<
                            mornlea_engine::native::contracts::mesh::MeshRegistryEntry,
                        >()
                        + near.registry().visibility().len() * 8
                        + 4
                };
                near.owned_bytes() + shared
            }
            mornlea_client_core::preparation::PreparationPayload::Far(far) => {
                far.owned_bytes()
                    + std::mem::size_of::<mornlea_engine::native::contracts::world::WorldgenParams>(
                    )
            }
        }
    }

    /// The delayed-completion operation: moves up to `count` FIFO pending
    /// jobs into the completed results queue. Until this runs, an admitted
    /// job has no result at all.
    pub fn advance(&mut self, count: usize) -> usize {
        let mut advanced = 0usize;
        while advanced < count {
            let Some((ticket, job, charge)) = self.pending.pop_front() else {
                break;
            };
            let key = *job.key();
            let geometry = match job.payload() {
                mornlea_client_core::preparation::PreparationPayload::Near(_) => {
                    mornlea_client_core::preparation::PreparedGeometry::Near(Vec::new())
                }
                mornlea_client_core::preparation::PreparationPayload::Far(_) => {
                    mornlea_client_core::preparation::PreparedGeometry::Far(Vec::new())
                }
            };
            let outcome =
                mornlea_client_core::preparation::PreparedGeometry::try_new(geometry).map(Arc::new);
            let result =
                mornlea_client_core::preparation::PreparationResult::try_new(ticket, key, outcome)
                    .expect("checked result");
            self.completed.push_back((result, charge));
            advanced += 1;
        }
        advanced
    }
}

impl PreparationPort for PreparationFaucet {
    fn try_submit(
        &mut self,
        job: PreparationJob,
    ) -> Result<PreparationTicket, RejectedPreparation> {
        let held = self.pending.len() + self.completed.len();
        if held + 1 > self.limits.preparation_results() {
            return Err(
                RejectedPreparation::try_new(ClientError::Capacity, job).expect("rejection")
            );
        }
        let extra = self.job_charge(&job);
        let new_charge = self.charge.saturating_add(extra);
        if new_charge > self.limits.preparation_bytes() {
            return Err(
                RejectedPreparation::try_new(ClientError::Capacity, job).expect("rejection")
            );
        }
        let ticket = PreparationTicket::try_new(self.next_ticket).expect("nonzero ticket");
        self.next_ticket += 1;
        self.tickets_issued += 1;
        self.charge = new_charge;
        self.pending.push_back((ticket, job, extra));
        Ok(ticket)
    }

    fn poll_ready(&mut self) -> Option<PreparationResult> {
        let (result, _charge) = self.completed.pop_front()?;
        Some(result)
    }

    fn invalidate(&mut self, epoch: SessionEpoch) -> InvalidationReport {
        let mut jobs_cancelled = 0u32;
        let mut results_stale = 0u32;
        let mut bytes_released = 0u64;
        let remaining = self.pending.len();
        for _ in 0..remaining {
            let (ticket, job, charge) = self.pending.pop_front().expect("queued job");
            if job.key().epoch() == epoch {
                jobs_cancelled += 1;
                bytes_released += charge as u64;
            } else {
                self.pending.push_back((ticket, job, charge));
            }
        }
        let remaining = self.completed.len();
        for _ in 0..remaining {
            let (result, charge) = self.completed.pop_front().expect("held result");
            if result.key().epoch() == epoch {
                results_stale += 1;
                bytes_released += charge as u64;
            } else {
                self.completed.push_back((result, charge));
            }
        }
        self.charge -= usize::try_from(bytes_released)
            .unwrap_or(self.charge)
            .min(self.charge);
        InvalidationReport::try_new(jobs_cancelled, results_stale, bytes_released)
            .expect("checked report")
    }
}

/// Harness combining the deterministic clock, transport and consumer double
/// with the checked configuration. Fixture-driven operations follow the
/// contract: connect/submit/step/snapshot/reset/close, clock advance,
/// complete/fragmented receive, send Capacity/Io and read-only inspection.
pub struct ReplayHarness {
    pub clock: Arc<DeterministicClock>,
    pub connector: Arc<MemoryConnectorDouble>,
    pub double: ContractDouble,
    pub preparations: PreparationFaucet,
    limits: ClientLimits,
}

impl ReplayHarness {
    pub fn new(mode: DoubleMode) -> Result<Self, ClientError> {
        Self::with_limits(mode, ClientLimits::try_new()?)
    }

    pub fn with_limits(mode: DoubleMode, limits: ClientLimits) -> Result<Self, ClientError> {
        let connector = Arc::new(MemoryConnectorDouble::new());
        Ok(Self {
            clock: Arc::new(DeterministicClock::new()),
            connector: Arc::clone(&connector),
            double: ContractDouble::new(mode, limits).with_connector(connector),
            preparations: PreparationFaucet::new(limits),
            limits,
        })
    }

    pub fn config(&self) -> Result<ClientConfig, ClientError> {
        ClientConfig::try_new(
            self.limits,
            Duration::from_secs(5),
            Duration::from_secs(10),
            self.clock.clone(),
            registry_with_memory(Arc::clone(&self.connector)),
        )
    }

    pub fn connect(&mut self, identity: ClientIdentity) -> Result<SessionEpoch, ClientError> {
        let endpoint = mornlea_client_core::contracts::Endpoint::Memory {
            connector_id: NonZeroU64::new(1).expect("one"),
        };
        self.double.connect(endpoint, identity)
    }

    pub fn submit_input(
        &mut self,
        epoch: SessionEpoch,
        batch: InputBatch,
    ) -> Result<InputReceipt, ClientError> {
        self.double.submit_input(epoch, batch)
    }

    pub fn step(
        &mut self,
        _epoch: SessionEpoch,
        work: ClientWorkBudget,
    ) -> Result<StepReport, ClientError> {
        self.double.step(work)
    }

    pub fn snapshot(&self, epoch: SessionEpoch) -> Result<Arc<PresentationFrame>, ClientError> {
        self.double.snapshot(epoch)
    }

    /// The checked close of one live epoch; a stale epoch is rejected.
    pub fn double_close_checked(&mut self, epoch: SessionEpoch) -> Result<(), ClientError> {
        if self.double.epoch() != Some(epoch) {
            return Err(ClientError::StaleEpoch);
        }
        self.double.close()
    }

    /// Direct transport polling for ticket contract checks.
    pub fn connector_poll(&self, ticket: TransportTicket) -> TransportPoll {
        self.connector.poll(ticket)
    }

    /// Admits one preparation job into the delayed-completion facility.
    pub fn submit_preparation(
        &mut self,
        job: PreparationJob,
    ) -> Result<PreparationTicket, RejectedPreparation> {
        self.preparations.try_submit(job)
    }

    /// The delayed preparation completion operation: advances up to `count`
    /// FIFO admitted jobs into completed results. A job completes only when
    /// this runs, so completion timing is scripted and deterministic.
    pub fn advance_preparations(&mut self, count: usize) -> usize {
        self.preparations.advance(count)
    }

    /// Transfers one FIFO completed preparation result.
    pub fn poll_preparation(&mut self) -> Option<PreparationResult> {
        self.preparations.poll_ready()
    }

    /// Releases stale preparation work by epoch, exactly once.
    pub fn invalidate_preparations(&mut self, epoch: SessionEpoch) -> InvalidationReport {
        self.preparations.invalidate(epoch)
    }

    /// Read-only preparation inspection: pending jobs not yet advanced.
    pub fn pending_preparations(&self) -> usize {
        self.preparations.pending_preparations()
    }

    /// Read-only preparation inspection: completed results not yet polled.
    pub fn completed_preparations(&self) -> usize {
        self.preparations.completed_preparations()
    }
}
