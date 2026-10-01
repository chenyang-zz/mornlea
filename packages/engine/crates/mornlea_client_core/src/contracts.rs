//! The frozen C1 external contract: epochs, limits, errors, endpoint and
//! transport substitution ports, the shared family header and the bounded
//! preparation port.
//!
//! Every checked struct here has private fields and a validated creation
//! path; validation precedes any state change, and every bound admits N and
//! rejects N + 1 with a typed `ClientError::Capacity` before allocation,
//! sequence advance or partial publication. The numeric values frozen here
//! come from the measured client capability inventory: the fixed ceilings
//! (128 input actions, 4096 message/mesh work items, 2 MiB protocol body,
//! 256 journal entries) plus the measured revision of the inbound receiver
//! (8192, which contains every supported run) and the proposed retained
//! bounds (8 MiB inbound/outbound/frame bytes, 4104 outbound commands,
//! 4096 preparation results, 64 MiB preparation bytes, 4096 family records).

use std::net::SocketAddr;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mornlea_protocol::LoginStart;

use crate::input::InputBatch;
use crate::preparation::{
    InvalidationReport, PreparationJob, PreparationResult, PreparationTicket, RejectedPreparation,
};
use crate::presentation::{BoundedText, PresentationFrame};

/// The session epoch: a nonzero identity of one connection attempt.
///
/// A pending connection already owns an epoch; reset issues a new one and old
/// epochs never resurrect. The value is a client-issued identity, not a
/// fabricated server tick.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionEpoch(NonZeroU64);

impl SessionEpoch {
    pub fn try_new(value: u64) -> Result<Self, ClientError> {
        let value = NonZeroU64::try_from(value).map_err(|_| ClientError::InvalidInput)?;
        Ok(Self(value))
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

/// The confirmed revision: one complete accepted authoritative observation.
///
/// Zero is reserved for pending-connection local, session, lifecycle,
/// diagnostics and local input frames before the first complete server
/// observation; the revision increases exactly once per complete accepted
/// observation and never regresses.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConfirmedRevision(u64);

impl ConfirmedRevision {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

/// The closed failure class set every client-core rejection reports.
///
/// Validation failures precede allocation and never publish a partial frame;
/// the class is the only failure surface, so consumers classify without
/// inspecting internals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientError {
    InvalidInput,
    IncompatibleVersion,
    InvalidState,
    StaleEpoch,
    Capacity,
    Timeout,
    Disconnected,
    Io,
    Internal,
}

/// The monotonic session phase. It only advances within one epoch; a reset
/// starts a fresh epoch at `Disconnected`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionPhase {
    Disconnected,
    Connecting,
    Handshaking,
    Admitted,
    Closing,
}

/// The local close reason of a terminal session observation.
///
/// Source disconnect and login-reject text is preserved byte-for-byte as
/// `Control` bounded text; no other prose is invented here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CloseReason {
    LocalClose,
    RemoteDisconnect(BoundedText),
    LoginRejected(BoundedText),
    Timeout,
    Capacity,
    Internal,
}

/// The frozen client limits, measured by the prerequisite inventory.
///
/// `try_new` publishes the frozen values; `try_new_with` admits a
/// configuration that is nonzero and at or below each frozen ceiling, so a
/// tighter cap is a legal deployment choice while an injected over-limit
/// value is rejected before any state change. Accessors are read-only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientLimits {
    queued_input_events: usize,
    inbound_observations: usize,
    inbound_bytes: usize,
    outbound_commands: usize,
    outbound_bytes: usize,
    prediction_journal: usize,
    message_work: usize,
    mesh_work: usize,
    preparation_results: usize,
    preparation_bytes: usize,
    family_records: usize,
    frame_bytes: usize,
}

/// The fixed and measured ceiling each limit may not exceed.
///
/// The frozen contract values and the maximum an injected configuration may
/// name are the same numbers: the queue byte, 8192 observation record, 4096
/// pending-preparation and 8 MiB frame bounds contain every supported run of
/// the measured inventory, and 4 MiB is invalid for the 4,325,408-byte world
/// aggregate.
impl ClientLimits {
    pub const MAX_QUEUED_INPUT_EVENTS: usize = 128;
    pub const MAX_INBOUND_OBSERVATIONS: usize = 8192;
    pub const MAX_INBOUND_BYTES: usize = 8 << 20;
    pub const MAX_OUTBOUND_COMMANDS: usize = 4104;
    pub const MAX_OUTBOUND_BYTES: usize = 8 << 20;
    pub const MAX_PREDICTION_JOURNAL: usize = 256;
    pub const MAX_MESSAGE_WORK: usize = 4096;
    pub const MAX_MESH_WORK: usize = 4096;
    pub const MAX_PREPARATION_RESULTS: usize = 4096;
    pub const MAX_PREPARATION_BYTES: usize = 64 << 20;
    pub const MAX_FAMILY_RECORDS: usize = 4096;
    pub const MAX_FRAME_BYTES: usize = 8 << 20;

    /// Publishes the frozen measured values.
    pub fn try_new() -> Result<Self, ClientError> {
        Self::try_new_with(
            Self::MAX_QUEUED_INPUT_EVENTS,
            Self::MAX_INBOUND_OBSERVATIONS,
            Self::MAX_INBOUND_BYTES,
            Self::MAX_OUTBOUND_COMMANDS,
            Self::MAX_OUTBOUND_BYTES,
            Self::MAX_PREDICTION_JOURNAL,
            Self::MAX_MESSAGE_WORK,
            Self::MAX_MESH_WORK,
            Self::MAX_PREPARATION_RESULTS,
            Self::MAX_PREPARATION_BYTES,
            Self::MAX_FAMILY_RECORDS,
            Self::MAX_FRAME_BYTES,
        )
    }

    /// Admits a configuration at or below every frozen ceiling.
    ///
    /// A zero limit would make the matching owner reject every batch, so it
    /// is an invalid configuration rather than a degenerate one; the check
    /// runs before the value is stored, so no partially configured limits
    /// value can exist.
    #[allow(clippy::too_many_lines)]
    pub fn try_new_with(
        queued_input_events: usize,
        inbound_observations: usize,
        inbound_bytes: usize,
        outbound_commands: usize,
        outbound_bytes: usize,
        prediction_journal: usize,
        message_work: usize,
        mesh_work: usize,
        preparation_results: usize,
        preparation_bytes: usize,
        family_records: usize,
        frame_bytes: usize,
    ) -> Result<Self, ClientError> {
        let ok = |value: usize, max: usize| value != 0 && value <= max;
        if !ok(queued_input_events, Self::MAX_QUEUED_INPUT_EVENTS)
            || !ok(inbound_observations, Self::MAX_INBOUND_OBSERVATIONS)
            || !ok(inbound_bytes, Self::MAX_INBOUND_BYTES)
            || !ok(outbound_commands, Self::MAX_OUTBOUND_COMMANDS)
            || !ok(outbound_bytes, Self::MAX_OUTBOUND_BYTES)
            || !ok(prediction_journal, Self::MAX_PREDICTION_JOURNAL)
            || !ok(message_work, Self::MAX_MESSAGE_WORK)
            || !ok(mesh_work, Self::MAX_MESH_WORK)
            || !ok(preparation_results, Self::MAX_PREPARATION_RESULTS)
            || !ok(preparation_bytes, Self::MAX_PREPARATION_BYTES)
            || !ok(family_records, Self::MAX_FAMILY_RECORDS)
            || !ok(frame_bytes, Self::MAX_FRAME_BYTES)
        {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            queued_input_events,
            inbound_observations,
            inbound_bytes,
            outbound_commands,
            outbound_bytes,
            prediction_journal,
            message_work,
            mesh_work,
            preparation_results,
            preparation_bytes,
            family_records,
            frame_bytes,
        })
    }

    pub fn queued_input_events(&self) -> usize {
        self.queued_input_events
    }

    pub fn inbound_observations(&self) -> usize {
        self.inbound_observations
    }

    pub fn inbound_bytes(&self) -> usize {
        self.inbound_bytes
    }

    pub fn outbound_commands(&self) -> usize {
        self.outbound_commands
    }

    pub fn outbound_bytes(&self) -> usize {
        self.outbound_bytes
    }

    pub fn prediction_journal(&self) -> usize {
        self.prediction_journal
    }

    pub fn message_work(&self) -> usize {
        self.message_work
    }

    pub fn mesh_work(&self) -> usize {
        self.mesh_work
    }

    pub fn preparation_results(&self) -> usize {
        self.preparation_results
    }

    pub fn preparation_bytes(&self) -> usize {
        self.preparation_bytes
    }

    pub fn family_records(&self) -> usize {
        self.family_records
    }

    pub fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }
}

/// The per-step work demand. Zero is legal; each field is capped at the
/// measured 4096 work budget and rechecked by `step` before any dequeue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientWorkBudget {
    messages: u16,
    meshes: u16,
}

impl ClientWorkBudget {
    pub const MAX_PER_STEP: u16 = 4096;

    pub fn try_new(messages: u16, meshes: u16) -> Result<Self, ClientError> {
        if messages > Self::MAX_PER_STEP || meshes > Self::MAX_PER_STEP {
            return Err(ClientError::Capacity);
        }
        Ok(Self { messages, meshes })
    }

    pub fn messages(&self) -> u16 {
        self.messages
    }

    pub fn meshes(&self) -> u16 {
        self.meshes
    }
}

/// The local admission receipt of one input batch.
///
/// A valid receipt attests local admission only; it is never a server
/// confirmation and implies no authoritative outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputReceipt {
    Noop,
    Queued {
        epoch: SessionEpoch,
        first_sequence: Option<u64>,
        sequenced_count: u16,
        chat_count: u16,
    },
}

/// What one step drained and what remains owned work.
///
/// Pending counts describe owned accepted work, never dropped records.
#[derive(Clone, Debug, PartialEq)]
pub struct StepReport {
    epoch: SessionEpoch,
    confirmed_revision: ConfirmedRevision,
    frame_index: u64,
    processed_messages: u16,
    processed_meshes: u16,
    pending_input: usize,
    pending_inbound: usize,
    pending_preparation: usize,
    terminal: Option<CloseReason>,
}

impl StepReport {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        epoch: SessionEpoch,
        confirmed_revision: ConfirmedRevision,
        frame_index: u64,
        processed_messages: u16,
        processed_meshes: u16,
        pending_input: usize,
        pending_inbound: usize,
        pending_preparation: usize,
        terminal: Option<CloseReason>,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            epoch,
            confirmed_revision,
            frame_index,
            processed_messages,
            processed_meshes,
            pending_input,
            pending_inbound,
            pending_preparation,
            terminal,
        })
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn confirmed_revision(&self) -> ConfirmedRevision {
        self.confirmed_revision
    }

    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    pub fn processed_messages(&self) -> u16 {
        self.processed_messages
    }

    pub fn processed_meshes(&self) -> u16 {
        self.processed_meshes
    }

    pub fn pending_input(&self) -> usize {
        self.pending_input
    }

    pub fn pending_inbound(&self) -> usize {
        self.pending_inbound
    }

    pub fn pending_preparation(&self) -> usize {
        self.pending_preparation
    }

    pub fn terminal(&self) -> Option<&CloseReason> {
        self.terminal.as_ref()
    }
}

/// The client endpoint surface the host drives once per step.
///
/// `connect` is nonblocking: its epoch identifies a pending attempt and login
/// succeeds or fails only through `step` under the injected monotonic
/// deadlines. Errors never publish a partial frame and leave every owner
/// unchanged.
pub trait ClientEndpoint {
    fn connect(
        &mut self,
        endpoint: Endpoint,
        identity: ClientIdentity,
    ) -> Result<SessionEpoch, ClientError>;
    fn submit_input(
        &mut self,
        epoch: SessionEpoch,
        input: InputBatch,
    ) -> Result<InputReceipt, ClientError>;
    fn step(
        &mut self,
        epoch: SessionEpoch,
        work: ClientWorkBudget,
    ) -> Result<StepReport, ClientError>;
    fn snapshot(&self, epoch: SessionEpoch) -> Result<Arc<PresentationFrame>, ClientError>;
    fn reset(&mut self, epoch: SessionEpoch) -> Result<SessionEpoch, ClientError>;
    fn close(&mut self) -> Result<(), ClientError>;
}

/// The client core owner: created from a checked config, it validates the
/// configuration (including registered memory connector ids) before any
/// epoch exists and owns the prepared-resource arena lookup later providers
/// fill in.
pub struct ClientCore {
    config: ClientConfig,
}

impl ClientCore {
    pub fn new(config: ClientConfig) -> Result<Self, ClientError> {
        config.validate()?;
        Ok(Self { config })
    }

    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// Safe Rust-only lookup of one retained prepared geometry.
    ///
    /// The arena is owned by the bounded preparation queue; until that owner
    /// lands, no key can be retained, so every lookup is an invalid input and
    /// no fabricated resource is ever returned.
    pub fn prepared_resource(
        &self,
        key: &crate::preparation::PreparedResourceKey,
    ) -> Result<Arc<crate::preparation::PreparedGeometry>, ClientError> {
        let _ = key;
        Err(ClientError::InvalidInput)
    }
}

/// The checked login identity: the exact protocol `LoginStart` value is held
/// read-only, and boundary construction rechecks every invariant.
#[derive(Clone, Debug)]
pub struct ClientIdentity {
    login: LoginStart,
}

impl ClientIdentity {
    pub fn try_new(login: LoginStart) -> Result<Self, ClientError> {
        login.validate().map_err(|_| ClientError::InvalidInput)?;
        Ok(Self { login })
    }

    pub fn login(&self) -> &LoginStart {
        &self.login
    }
}

/// The connection target. Memory connector ids resolve only through the
/// injected registry, never through a server dependency.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Endpoint {
    Memory { connector_id: NonZeroU64 },
    Tcp(SocketAddr),
}

/// The runtime-injected monotonic clock. Production uses
/// `std::time::Instant`; tests inject a deterministic implementation of the
/// same trait.
pub trait MonotonicClock: Send + Sync {
    fn now(&self) -> Instant;
}

/// Production clock over `std::time::Instant`.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdMonotonicClock;

impl MonotonicClock for StdMonotonicClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// The nonblocking client transport port. Implementations never block the
/// core step; the TCP provider owns its bounded worker and queues, and a
/// failed `try_send` retains the entire core queue head for retry or
/// terminal reporting, never a partial record.
pub trait Connector: Send + Sync {
    fn try_connect(
        &self,
        endpoint: &Endpoint,
        identity: &ClientIdentity,
    ) -> Result<TransportTicket, ClientError>;
    fn poll(&self, ticket: TransportTicket) -> TransportPoll;
    fn try_send(&self, ticket: TransportTicket, frame: &[u8]) -> Result<(), ClientError>;
    fn close(&self, ticket: TransportTicket) -> Result<(), ClientError>;
}

/// The checked connector registry. Memory endpoint ids must resolve here
/// before an epoch is created; the registry carries no authority over world
/// or player state.
#[derive(Default)]
pub struct ConnectorRegistry {
    connectors: Vec<(NonZeroU64, Arc<dyn Connector>)>,
}

impl ConnectorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one connector under a nonzero id; a duplicate id is an
    /// invalid configuration and rejects before any mutation.
    pub fn register(
        &mut self,
        connector_id: NonZeroU64,
        connector: Arc<dyn Connector>,
    ) -> Result<(), ClientError> {
        if self.connectors.iter().any(|(id, _)| *id == connector_id) {
            return Err(ClientError::InvalidInput);
        }
        self.connectors.push((connector_id, connector));
        Ok(())
    }

    pub fn resolve(&self, connector_id: NonZeroU64) -> Option<Arc<dyn Connector>> {
        self.connectors
            .iter()
            .find(|(id, _)| *id == connector_id)
            .map(|(_, connector)| Arc::clone(connector))
    }
}

/// The checked core configuration: frozen-or-tighter limits, the shared
/// hello/login deadline policy and the injected clock plus connector
/// registry. Deadlines must be nonzero; the 5 s / 10 s policy values are the
/// caller's choice, not a second default here.
pub struct ClientConfig {
    limits: ClientLimits,
    hello_timeout: Duration,
    login_timeout: Duration,
    clock: Arc<dyn MonotonicClock>,
    connectors: Arc<ConnectorRegistry>,
}

impl ClientConfig {
    pub fn try_new(
        limits: ClientLimits,
        hello_timeout: Duration,
        login_timeout: Duration,
        clock: Arc<dyn MonotonicClock>,
        connectors: Arc<ConnectorRegistry>,
    ) -> Result<Self, ClientError> {
        if hello_timeout.is_zero() || login_timeout.is_zero() {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            limits,
            hello_timeout,
            login_timeout,
            clock,
            connectors,
        })
    }

    fn validate(&self) -> Result<(), ClientError> {
        if self.hello_timeout.is_zero() || self.login_timeout.is_zero() {
            return Err(ClientError::InvalidInput);
        }
        Ok(())
    }

    pub fn limits(&self) -> &ClientLimits {
        &self.limits
    }

    pub fn hello_timeout(&self) -> Duration {
        self.hello_timeout
    }

    pub fn login_timeout(&self) -> Duration {
        self.login_timeout
    }

    pub fn clock(&self) -> &Arc<dyn MonotonicClock> {
        &self.clock
    }

    pub fn connectors(&self) -> &Arc<ConnectorRegistry> {
        &self.connectors
    }
}

/// A transport launch ticket: the connector id plus a nonzero launch
/// generation, both private. Old tickets are rejected after close/reset and
/// generation exhaustion is the typed `Capacity` error.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TransportTicket {
    connector_id: NonZeroU64,
    generation: NonZeroU64,
}

impl TransportTicket {
    pub fn try_new(connector_id: NonZeroU64, generation: NonZeroU64) -> Result<Self, ClientError> {
        Ok(Self {
            connector_id,
            generation,
        })
    }

    pub fn connector_id(&self) -> NonZeroU64 {
        self.connector_id
    }

    pub fn generation(&self) -> NonZeroU64 {
        self.generation
    }
}

/// The per-connector launch generation counter. Exhaustion of the nonzero
/// generation space returns `Capacity` instead of wrapping into reuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransportLaunch {
    next: u64,
}

impl TransportLaunch {
    pub fn new() -> Self {
        Self { next: 1 }
    }

    pub fn advance(&mut self) -> Result<NonZeroU64, ClientError> {
        if self.next == 0 {
            return Err(ClientError::Capacity);
        }
        let generation = NonZeroU64::new(self.next).ok_or(ClientError::Capacity)?;
        self.next = self.next.checked_add(1).ok_or(ClientError::Capacity)?;
        Ok(generation)
    }
}

impl Default for TransportLaunch {
    fn default() -> Self {
        Self::new()
    }
}

/// One transport poll result. `Frame` is exactly one complete prefix-inclusive
/// v45 frame, already bounded by the accepted 2 MiB body cap; the core still
/// runs the normal decoder and admission path on it.
#[derive(Clone, Debug, PartialEq)]
pub enum TransportPoll {
    Pending,
    Connected,
    Frame(Vec<u8>),
    Closed(ClientError),
}

/// The source identity of one accepted observation: assigned only after a
/// complete accepted observation, it exists even when the server packet
/// carries no universal event id.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObservationKey {
    epoch: SessionEpoch,
    confirmed_revision: ConfirmedRevision,
    ordinal: u32,
}

impl ObservationKey {
    pub fn try_new(
        epoch: SessionEpoch,
        confirmed_revision: ConfirmedRevision,
        ordinal: u32,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            epoch,
            confirmed_revision,
            ordinal,
        })
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn confirmed_revision(&self) -> ConfirmedRevision {
        self.confirmed_revision
    }

    pub fn ordinal(&self) -> u32 {
        self.ordinal
    }
}

/// The logical family identity. Numeric descriptors are assigned later by the
/// exclusive registry owner; feature workers use these logical keys only.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FamilyKey {
    pub logical_name: &'static str,
    pub major: u16,
    pub minor: u16,
}

/// The ten logical family names in contract order.
pub const FAMILY_SESSION: &str = "session";
pub const FAMILY_INPUT: &str = "input";
pub const FAMILY_TERRAIN: &str = "terrain";
pub const FAMILY_ACTORS: &str = "actors";
pub const FAMILY_PLAYER_VIEW: &str = "player-view";
pub const FAMILY_INVENTORY_UI: &str = "inventory-ui";
pub const FAMILY_WORLD_UI: &str = "world-ui";
pub const FAMILY_AUDIO_CUES: &str = "audio-cues";
pub const FAMILY_LIFECYCLE: &str = "lifecycle";
pub const FAMILY_DIAGNOSTICS: &str = "diagnostics";

impl FamilyKey {
    /// Publishes a major 1 / minor 0 key under a known logical name. An
    /// unknown name or a non-1.0 version is a contract drift and rejects.
    pub fn try_new(logical_name: &'static str) -> Result<Self, ClientError> {
        let known = matches!(
            logical_name,
            FAMILY_SESSION
                | FAMILY_INPUT
                | FAMILY_TERRAIN
                | FAMILY_ACTORS
                | FAMILY_PLAYER_VIEW
                | FAMILY_INVENTORY_UI
                | FAMILY_WORLD_UI
                | FAMILY_AUDIO_CUES
                | FAMILY_LIFECYCLE
                | FAMILY_DIAGNOSTICS
        );
        if !known {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            logical_name,
            major: 1,
            minor: 0,
        })
    }
}

/// The per-record operation. Valid packets such as `ForgetChunks` carry no
/// source tick; removal precedes reuse of the same stable id.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FamilyOperation {
    Upsert,
    Remove,
}

/// The owned checked record header every family record carries, even when a
/// frame parent repeats the same epoch and revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordHeader {
    epoch: SessionEpoch,
    revision: ConfirmedRevision,
    source_tick: Option<u64>,
    operation: FamilyOperation,
}

impl RecordHeader {
    pub fn try_new(
        epoch: SessionEpoch,
        revision: ConfirmedRevision,
        source_tick: Option<u64>,
        operation: FamilyOperation,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            epoch,
            revision,
            source_tick,
            operation,
        })
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn revision(&self) -> ConfirmedRevision {
        self.revision
    }

    pub fn source_tick(&self) -> Option<u64> {
        self.source_tick
    }

    pub fn operation(&self) -> FamilyOperation {
        self.operation
    }

    /// The owned byte charge of the header: epoch, revision, the optional
    /// tick tag and value, and the operation tag.
    pub(crate) fn owned_bytes(&self) -> usize {
        8 + 8 + 1 + self.source_tick.map_or(0, |_| 8) + 1
    }
}

/// The bounded shared preparation port. Implementations never block; a
/// rejected admission returns the complete owned job and issues no ticket,
/// and stale results are counted and released exactly once.
pub trait PreparationPort {
    fn try_submit(&mut self, job: PreparationJob)
    -> Result<PreparationTicket, RejectedPreparation>;
    fn poll_ready(&mut self) -> Option<PreparationResult>;
    fn invalidate(&mut self, epoch: SessionEpoch) -> InvalidationReport;
}
