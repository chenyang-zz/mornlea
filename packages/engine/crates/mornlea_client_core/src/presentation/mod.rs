//! The C2 presentation seam: checked supporting values, the projection
//! context, source-order envelopes and the frozen publication ownership.
//!
//! This module declares the checked values every family record reuses, the
//! `ProjectionView` immutable context with its exact borrowed inputs, the
//! private `OrderedRecord`/`ProjectionOrder`/`StableRecordKey` source-order
//! envelopes, the state-owner private types their owning nodes replace, and
//! the publication ownership bundle the serial assembler consumes. The
//! per-kind `project_*` and serial `assemble_*` functions named by the
//! contract are defined inside their owning provider files with exactly the
//! frozen signatures recorded in the contract landing; nothing here
//! implements them early.

pub mod frame;
pub mod geometry;

pub mod assembly;
pub mod family_actors;
pub mod family_audio;
pub mod family_diagnostics;
pub mod family_inventory_ui;
pub mod family_player_view;
pub mod family_terrain;
pub mod family_world_ui;

// The split families are declared inline so each per-topic provider file is
// registered once by the contract landing and no provider ever edits a root,
// export or registration.
pub mod actors {
    pub mod companion;
    pub mod drop;
    pub mod hostile;
    pub mod passive;
    pub mod projectile;
    pub mod remote_player;
}

pub mod inventory_ui {
    pub mod container;
    pub mod crafting;
    pub mod furnace;
    pub mod inventory;
}

pub mod world_ui {
    pub mod chat;
    pub mod environment;
    pub mod prompt;
    pub mod survival;
    pub mod task;
}

// The record and frame shapes are declared in `frame` and re-exported here so
// the presentation seam has one surface.
pub use frame::{
    ActorDetail, ActorDimension, ActorId, ActorKind, AudioCueCategory, AudioCueRecord,
    DiagnosticRecord, FRAME_LAYOUT_MAJOR, FRAME_LAYOUT_MINOR, FamilyFrame, FamilyRecords,
    InputReceiptState, InputRecord, InventoryUiRecord, LifecycleRecord, LifecycleTransition,
    PlayerViewRecord, PresentationFrame, SessionRecord, TerrainMaterial, TerrainRecord,
    WorldUiRecord,
};

use std::sync::Arc;

use mornlea_domain::{
    BlockPos, ChatEvent, CommandText, InventoryState, SurvivalState, TaskState, WorldState,
};

use crate::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, ObservationKey, SessionEpoch,
};
use crate::input::InputProjectionState;
use crate::session::ConfirmedMirror;

/// Bounded display text with an explicit kind. `Name` admits at most 32
/// scalars and 128 bytes; `Command` 1024 bytes; `Speech` 256 bytes; `Target`
/// 64 bytes, where an empty target means a cleared local target. `Control`
/// admits the exact protocol control-message set: any valid UTF-8 up to 256
/// bytes and 256 scalars, including empty, padded and control-containing
/// values. Other kinds keep the accepted domain rules for trimming, control
/// and emptiness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextKind {
    Name,
    Command,
    Speech,
    Target,
    Control,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedText {
    bytes: String,
    kind: TextKind,
}

impl BoundedText {
    pub const NAME_MAX_SCALARS: usize = 32;
    pub const NAME_MAX_BYTES: usize = 128;
    pub const COMMAND_MAX_BYTES: usize = 1024;
    pub const SPEECH_MAX_BYTES: usize = 256;
    pub const TARGET_MAX_BYTES: usize = 64;
    pub const CONTROL_MAX_BYTES: usize = 256;
    pub const CONTROL_MAX_SCALARS: usize = 256;

    pub fn try_new(bytes: String, kind: TextKind) -> Result<Self, ClientError> {
        let scalars = bytes.chars().count();
        let ok = match kind {
            TextKind::Name => {
                (1..=Self::NAME_MAX_SCALARS).contains(&scalars)
                    && bytes.len() <= Self::NAME_MAX_BYTES
                    && !bytes.is_empty()
            }
            TextKind::Command => !bytes.is_empty() && bytes.len() <= Self::COMMAND_MAX_BYTES,
            TextKind::Speech => !bytes.is_empty() && bytes.len() <= Self::SPEECH_MAX_BYTES,
            // An empty target is the cleared local target, not a violation.
            TextKind::Target => bytes.len() <= Self::TARGET_MAX_BYTES,
            TextKind::Control => {
                scalars <= Self::CONTROL_MAX_SCALARS && bytes.len() <= Self::CONTROL_MAX_BYTES
            }
        };
        if !ok {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self { bytes, kind })
    }

    pub fn kind(&self) -> &TextKind {
        &self.kind
    }

    pub fn as_str(&self) -> &str {
        &self.bytes
    }

    pub(crate) fn owned_bytes(&self) -> usize {
        1 + self.bytes.len()
    }
}

/// A finite world-space pose: finite block positions and finite radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    position: [f64; 3],
    yaw: f64,
    pitch: f64,
}

impl Pose {
    pub fn try_new(position: [f64; 3], yaw: f64, pitch: f64) -> Result<Self, ClientError> {
        if !position.iter().all(|value| value.is_finite()) || !yaw.is_finite() || !pitch.is_finite()
        {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            position,
            yaw,
            pitch,
        })
    }

    pub fn position(&self) -> [f64; 3] {
        self.position
    }

    pub fn yaw(&self) -> f64 {
        self.yaw
    }

    pub fn pitch(&self) -> f64 {
        self.pitch
    }
}

/// A finite look ray from a finite origin through accepted look angles with
/// a positive finite reach. The reach is bounded by the accepted authority
/// interaction distance of 6 blocks, the value the pilot's target and input
/// ray casts share.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FiniteRay {
    origin: [f64; 3],
    look: mornlea_domain::LookAngles,
    reach: f32,
}

impl FiniteRay {
    /// The accepted authority interaction distance.
    pub const MAX_REACH: f32 = 6.0;

    pub fn try_new(
        origin: [f64; 3],
        look: mornlea_domain::LookAngles,
        reach: f32,
    ) -> Result<Self, ClientError> {
        if !origin.iter().all(|value| value.is_finite())
            || !reach.is_finite()
            || reach <= 0.0
            || reach > Self::MAX_REACH
        {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            origin,
            look,
            reach,
        })
    }

    pub fn origin(&self) -> [f64; 3] {
        self.origin
    }

    pub fn look(&self) -> mornlea_domain::LookAngles {
        self.look
    }

    pub fn reach(&self) -> f32 {
        self.reach
    }
}

/// The local movement intent beside one confirmed pose.
#[derive(Clone, Debug, PartialEq)]
pub struct MovementIntent {
    control: Option<mornlea_domain::PlayerControl>,
    on_ground: bool,
}

impl MovementIntent {
    pub fn try_new(
        control: Option<mornlea_domain::PlayerControl>,
        on_ground: bool,
    ) -> Result<Self, ClientError> {
        Ok(Self { control, on_ground })
    }

    pub fn control(&self) -> Option<&mornlea_domain::PlayerControl> {
        self.control.as_ref()
    }

    pub fn on_ground(&self) -> bool {
        self.on_ground
    }
}

/// Why one authoritative correction replayed the journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorrectionReason {
    AuthoritativeReset,
    AcknowledgedReplay,
    RejectedInput,
}

/// One correction: the last input sequence it covers and its reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Correction {
    last_input_sequence: u64,
    reason: CorrectionReason,
}

impl Correction {
    pub fn try_new(
        last_input_sequence: u64,
        reason: CorrectionReason,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            last_input_sequence,
            reason,
        })
    }

    pub fn last_input_sequence(&self) -> u64 {
        self.last_input_sequence
    }

    pub fn reason(&self) -> CorrectionReason {
        self.reason
    }
}

/// The outcome of one inventory or placement interaction, with its actual
/// sequence. Command rejection and placement success are distinct outcomes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiOutcome {
    Rejected {
        sequence: u64,
        reason: mornlea_domain::RejectReason,
    },
    PlacementAccepted {
        sequence: u64,
    },
}

/// One task observation: the accepted observation key, the companion it was
/// addressed to, the restated command and the lifecycle state. There is no
/// task id, no generation, no progress percentage and no pending/running
/// wire state, so none can be displayed.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskView {
    observation: ObservationKey,
    companion: mornlea_domain::CompanionSpeaker,
    command: CommandText,
    state: TaskState,
}

impl TaskView {
    pub fn try_new(
        observation: ObservationKey,
        companion: mornlea_domain::CompanionSpeaker,
        command: CommandText,
        state: TaskState,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            observation,
            companion,
            command,
            state,
        })
    }

    pub fn observation(&self) -> &ObservationKey {
        &self.observation
    }

    pub fn companion(&self) -> &mornlea_domain::CompanionSpeaker {
        &self.companion
    }

    pub fn command(&self) -> &CommandText {
        &self.command
    }

    pub fn state(&self) -> &TaskState {
        &self.state
    }
}

/// The mirror-derived presentation of the current checked ray target and its
/// registered display name. It is a derived local value, explicitly not an
/// authoritative prompt packet.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptView {
    target: BlockPos,
    label: BoundedText,
}

impl PromptView {
    pub fn try_new(target: BlockPos, label: BoundedText) -> Result<Self, ClientError> {
        Ok(Self { target, label })
    }

    pub fn target(&self) -> BlockPos {
        self.target
    }

    pub fn label(&self) -> &BoundedText {
        &self.label
    }
}

/// The closed inventory UI view set. These variants wrap the accepted checked
/// domain payloads; no recipe result or equipped-armor field exists here.
#[derive(Clone, Debug, PartialEq)]
pub enum InventoryUiView {
    Inventory(InventoryState),
    Crafting(mornlea_domain::CraftingState),
    Furnace(mornlea_domain::FurnaceState),
    Chest(mornlea_domain::ChestState),
    Closed(mornlea_domain::ContainerRef),
    Rejected(mornlea_domain::CommandRejection),
}

/// The closed world UI view set.
pub type EnvironmentView = WorldState;
pub type SurvivalView = SurvivalState;

#[derive(Clone, Debug, PartialEq)]
pub enum WorldUiView {
    Environment(EnvironmentView),
    Survival(SurvivalView),
    Chat(ChatEvent),
    Task(TaskView),
    Prompt(Option<PromptView>),
}

/// A registered audio cue identity. Only the cue ids the measured inventory
/// registered through its audio source are admitted: the seven pilot cues
/// (0..=6: UI click, mining complete, eating complete, damage, water splash,
/// combat hit, snow step).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct CueId(u16);

impl CueId {
    /// The highest registered cue id of the measured inventory source.
    pub const MAX_REGISTERED: u16 = 6;

    pub fn try_new(value: u16) -> Result<Self, ClientError> {
        if value > Self::MAX_REGISTERED {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

/// A finite gain in the closed 0..=1 range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FiniteUnit(f32);

impl FiniteUnit {
    pub fn try_new(value: f32) -> Result<Self, ClientError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> f32 {
        self.0
    }
}

/// A finite strictly positive scalar, for playback pitch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FinitePositive(f32);

impl FinitePositive {
    pub fn try_new(value: f32) -> Result<Self, ClientError> {
        if !value.is_finite() || value <= 0.0 {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> f32 {
        self.0
    }
}

/// The closed resource key set of one lifecycle record's invalidation order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceKey {
    InputJournal,
    PreparationQueue,
    PresentationFrames,
    BridgeHandles,
    Feature(u64),
}

/// Reports one increment, saturating at `u64::MAX`. A saturated counter is
/// explicitly reported as incomplete evidence by the diagnostics contract,
/// never silent wrap.
pub(crate) fn bump(field: &mut u64) {
    *field = field.saturating_add(1);
}

/// The closed cue provenance. Confirmed cues exist even without a universal
/// event id, predicted cues carry their input sequence, and local cues carry
/// a native local event sequence and never claim server confirmation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CueProvenance {
    Confirmed {
        observation: ObservationKey,
        authoritative_event_id: Option<u64>,
    },
    Predicted {
        input_sequence: u64,
    },
    Local {
        local_event_sequence: u64,
    },
}

/// Bounded owner-maintained queue counters with saturating reporting.
/// Saturation is explicitly reported as incomplete evidence, never silent
/// wrap: a saturated counter is `u64::MAX`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueueCounters {
    pub(crate) input: u64,
    pub(crate) inbound_records: u64,
    pub(crate) inbound_bytes: u64,
    pub(crate) outbound_records: u64,
    pub(crate) outbound_bytes: u64,
    pub(crate) journal: u64,
    pub(crate) preparation: u64,
}

impl QueueCounters {
    pub fn try_new(
        input: u64,
        inbound_records: u64,
        inbound_bytes: u64,
        outbound_records: u64,
        outbound_bytes: u64,
        journal: u64,
        preparation: u64,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            input,
            inbound_records,
            inbound_bytes,
            outbound_records,
            outbound_bytes,
            journal,
            preparation,
        })
    }
}

/// Bounded rejection counters by error class, saturating the same way.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ErrorClassCounters {
    pub(crate) invalid: u64,
    pub(crate) capacity: u64,
    pub(crate) stale: u64,
    pub(crate) disconnected: u64,
    pub(crate) io: u64,
    pub(crate) internal: u64,
}

impl ErrorClassCounters {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        invalid: u64,
        capacity: u64,
        stale: u64,
        disconnected: u64,
        io: u64,
        internal: u64,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            invalid,
            capacity,
            stale,
            disconnected,
            io,
            internal,
        })
    }

    pub(crate) fn record(&mut self, class: ClientError) {
        let field = match class {
            ClientError::InvalidInput | ClientError::IncompatibleVersion => &mut self.invalid,
            ClientError::Capacity => &mut self.capacity,
            ClientError::StaleEpoch | ClientError::InvalidState | ClientError::Timeout => {
                &mut self.stale
            }
            ClientError::Disconnected => &mut self.disconnected,
            ClientError::Io => &mut self.io,
            ClientError::Internal => &mut self.internal,
        };
        bump(field);
    }
}

/// The checked producer identity of one diagnostics record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProducerIdentity {
    source_sha: [u8; 20],
    contract_sha: [u8; 32],
}

impl ProducerIdentity {
    pub fn try_new(source_sha: [u8; 20], contract_sha: [u8; 32]) -> Result<Self, ClientError> {
        Ok(Self {
            source_sha,
            contract_sha,
        })
    }

    pub fn source_sha(&self) -> [u8; 20] {
        self.source_sha
    }

    pub fn contract_sha(&self) -> [u8; 32] {
        self.contract_sha
    }
}

/// One accepted observation staged by the session owner: its source key, the
/// optional source tick, the typed packet and the actor identities the
/// mirror already resolved for it. The pending storage of these values counts
/// toward the measured family and frame-owned byte and count accounting.
#[derive(Clone, Debug, PartialEq)]
pub struct AcceptedObservation {
    key: ObservationKey,
    source_tick: Option<u64>,
    packet: mornlea_protocol::ServerPacket,
    resolved: Vec<ResolvedActor>,
}

/// One prevalidated actor identity resolution: the confirmed live identity a
/// dimensionless state or despawn packet was matched against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedActor {
    kind: frame::ActorKind,
    id: ActorId,
    dimension: mornlea_domain::Dimension,
}

impl ResolvedActor {
    pub fn try_new(
        kind: frame::ActorKind,
        id: ActorId,
        dimension: mornlea_domain::Dimension,
    ) -> Result<Self, ClientError> {
        if id.kind() != kind {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            kind,
            id,
            dimension,
        })
    }

    pub fn kind(&self) -> frame::ActorKind {
        self.kind
    }

    pub fn id(&self) -> &ActorId {
        &self.id
    }

    pub fn dimension(&self) -> mornlea_domain::Dimension {
        self.dimension
    }
}

impl AcceptedObservation {
    pub fn try_new(
        key: ObservationKey,
        source_tick: Option<u64>,
        packet: mornlea_protocol::ServerPacket,
        resolved: Vec<ResolvedActor>,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            key,
            source_tick,
            packet,
            resolved,
        })
    }

    pub fn key(&self) -> &ObservationKey {
        &self.key
    }

    pub fn source_tick(&self) -> Option<u64> {
        self.source_tick
    }

    pub fn packet(&self) -> &mornlea_protocol::ServerPacket {
        &self.packet
    }

    pub fn resolved(&self) -> &[ResolvedActor] {
        &self.resolved
    }
}

/// The private source-order envelope. Confirmed entries sort by actual
/// observation revision and ordinal, then packet record ordinal, then stable
/// key; local or derived entries sort after confirmed entries at their
/// sampled revision, then by local sequence, record ordinal and stable key.
/// Equal or absent source ticks never substitute for this order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum ProjectionOrder {
    Confirmed {
        observation: ObservationKey,
        record_ordinal: u32,
    },
    AfterConfirmed {
        epoch: SessionEpoch,
        revision: ConfirmedRevision,
        local_sequence: u64,
        record_ordinal: u32,
    },
}

/// The closed typed stable identity derived from the record itself: actor
/// kind plus dimension plus the actual actor id; terrain dimension plus the
/// terrain key; or the inventory/world topic tag plus its actual container
/// or event identity where present. Singleton topic ties keep the existing
/// union variant order, which derived `Ord` preserves.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StableRecordKey {
    Actor {
        kind: frame::ActorKind,
        dimension: ActorDimension,
        id: ActorId,
    },
    Terrain {
        dimension: mornlea_domain::Dimension,
        key: crate::preparation::TerrainKey,
    },
    Inventory {
        topic: InventoryTopic,
        identity: Option<mornlea_domain::ContainerRef>,
    },
    World {
        topic: WorldTopic,
        identity: Option<ObservationKey>,
    },
}

/// The inventory UI topic tags in variant order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum InventoryTopic {
    Inventory,
    Crafting,
    Furnace,
    Chest,
    Closed,
    Rejected,
}

/// The world UI topic tags in variant order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum WorldTopic {
    Environment,
    Survival,
    Chat,
    Task,
    Prompt,
}

/// One record wrapped with the private order data the serial family
/// assemblies need. The envelope is stripped only when the unchanged record
/// vector is emitted; no external record field or tag changes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrderedRecord<R> {
    order: ProjectionOrder,
    stable_key: StableRecordKey,
    record: R,
}

impl<R> OrderedRecord<R> {
    pub fn try_new(
        order: ProjectionOrder,
        stable_key: StableRecordKey,
        record: R,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            order,
            stable_key,
            record,
        })
    }

    pub fn order(&self) -> &ProjectionOrder {
        &self.order
    }

    pub fn stable_key(&self) -> &StableRecordKey {
        &self.stable_key
    }

    pub fn record(&self) -> &R {
        &self.record
    }

    pub fn into_record(self) -> R {
        self.record
    }

    /// The source order used by the serial assemblies' deterministic merge.
    pub(crate) fn sort_key(&self) -> &ProjectionOrder {
        &self.order
    }
}

/// The checked audio dedup key families, per provenance class.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AudioDedupKey {
    Confirmed {
        epoch: SessionEpoch,
        observation: ObservationKey,
        authoritative_event_id: Option<u64>,
        cue: CueId,
    },
    Predicted {
        epoch: SessionEpoch,
        input_sequence: u64,
        cue: CueId,
    },
    Local {
        epoch: SessionEpoch,
        local_event_sequence: u64,
        cue: CueId,
    },
}

/// The proposed audio dedup delta of one publication: the keys to commit and
/// the pending predicted keys a rejected input or consumed correction
/// cancels. It is a checked proposal, never a direct mutation of the
/// committed audio state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioDedupDelta {
    insertions: Vec<AudioDedupKey>,
    cancellations: Vec<AudioDedupKey>,
}

impl AudioDedupDelta {
    pub fn try_new(
        insertions: Vec<AudioDedupKey>,
        cancellations: Vec<AudioDedupKey>,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            insertions,
            cancellations,
        })
    }

    pub fn insertions(&self) -> &[AudioDedupKey] {
        &self.insertions
    }

    pub fn cancellations(&self) -> &[AudioDedupKey] {
        &self.cancellations
    }

    pub(crate) fn extend(&mut self, other: AudioDedupDelta) {
        self.insertions.extend(other.insertions);
        self.cancellations.extend(other.cancellations);
    }
}

/// The audio projection proposal: the checked records plus the dedup delta
/// that a successful publication would commit. It is not a mutation.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioProjection {
    records: Vec<frame::AudioCueRecord>,
    proposed_dedup: AudioDedupDelta,
}

impl AudioProjection {
    pub fn try_new(
        records: Vec<frame::AudioCueRecord>,
        proposed_dedup: AudioDedupDelta,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            records,
            proposed_dedup,
        })
    }

    pub fn records(&self) -> &[frame::AudioCueRecord] {
        &self.records
    }

    pub fn proposed_dedup(&self) -> &AudioDedupDelta {
        &self.proposed_dedup
    }
}

/// The audio state owner's immutable projection input: committed epoch-scoped
/// dedup keys and pending predicted cancellations. No device handle exists
/// here; device absence does not alter record generation.
#[derive(Clone, Debug, Default)]
pub struct AudioProjectionState {
    committed: Vec<AudioDedupKey>,
    pending_cancellations: Vec<AudioDedupKey>,
}

impl AudioProjectionState {
    pub fn try_new() -> Result<Self, ClientError> {
        Ok(Self::default())
    }

    pub fn committed(&self) -> &[AudioDedupKey] {
        &self.committed
    }

    pub fn pending_cancellations(&self) -> &[AudioDedupKey] {
        &self.pending_cancellations
    }

    /// Checked constructor surface the owner uses to publish a committed
    /// dedup state; publication itself stays with the serial assembler.
    pub fn with_committed(mut self, committed: Vec<AudioDedupKey>) -> Self {
        self.committed = committed;
        self
    }
}

/// The lifecycle state owner's projection input: bounded pending transition
/// records and the checked core-owned generation and resource order. No
/// feature or native handle identity is fabricated here.
#[derive(Clone, Debug, Default)]
pub struct LifecycleProjectionState {
    pending: Vec<frame::LifecycleRecord>,
    generation: u64,
}

impl LifecycleProjectionState {
    pub fn try_new(generation: u64) -> Result<Self, ClientError> {
        Ok(Self {
            pending: Vec::new(),
            generation,
        })
    }

    pub fn pending(&self) -> &[frame::LifecycleRecord] {
        &self.pending
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn with_pending(mut self, pending: Vec<frame::LifecycleRecord>) -> Self {
        self.pending = pending;
        self
    }
}

/// The diagnostics state owner's projection input: the checked producer
/// identity and an immutable counter snapshot from the input, I/O,
/// preparation, publication and lifetime owners.
#[derive(Clone, Debug)]
pub struct DiagnosticProjectionState {
    producer: ProducerIdentity,
    queues: QueueCounters,
    rejected: ErrorClassCounters,
}

impl DiagnosticProjectionState {
    pub fn try_new(
        producer: ProducerIdentity,
        queues: QueueCounters,
        rejected: ErrorClassCounters,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            producer,
            queues,
            rejected,
        })
    }

    pub fn producer(&self) -> &ProducerIdentity {
        &self.producer
    }

    pub fn queues(&self) -> &QueueCounters {
        &self.queues
    }

    pub fn rejected(&self) -> &ErrorClassCounters {
        &self.rejected
    }
}

/// The one immutable projection context every private projection function
/// borrows. It grants read-only access to the owner-maintained inputs and
/// never owns a second authority; the frame identity and limits are checked
/// controller-owned values that projections cannot change.
#[derive(Clone, Copy)]
pub struct ProjectionView<'a> {
    mirror: &'a ConfirmedMirror,
    observations: &'a [AcceptedObservation],
    input: &'a InputProjectionState,
    player: &'a crate::prediction::PlayerProjectionState,
    audio: &'a AudioProjectionState,
    lifecycle: &'a LifecycleProjectionState,
    diagnostics: &'a DiagnosticProjectionState,
    frame_epoch: SessionEpoch,
    frame_revision: ConfirmedRevision,
    frame_index: u64,
    limits: &'a ClientLimits,
}

impl<'a> ProjectionView<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        mirror: &'a ConfirmedMirror,
        observations: &'a [AcceptedObservation],
        input: &'a InputProjectionState,
        player: &'a crate::prediction::PlayerProjectionState,
        audio: &'a AudioProjectionState,
        lifecycle: &'a LifecycleProjectionState,
        diagnostics: &'a DiagnosticProjectionState,
        frame_epoch: SessionEpoch,
        frame_revision: ConfirmedRevision,
        frame_index: u64,
        limits: &'a ClientLimits,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            mirror,
            observations,
            input,
            player,
            audio,
            lifecycle,
            diagnostics,
            frame_epoch,
            frame_revision,
            frame_index,
            limits,
        })
    }

    pub fn mirror(&self) -> &ConfirmedMirror {
        self.mirror
    }

    pub fn observations(&self) -> &[AcceptedObservation] {
        self.observations
    }

    pub fn input(&self) -> &InputProjectionState {
        self.input
    }

    pub fn player(&self) -> &crate::prediction::PlayerProjectionState {
        self.player
    }

    pub fn audio(&self) -> &AudioProjectionState {
        self.audio
    }

    pub fn lifecycle(&self) -> &LifecycleProjectionState {
        self.lifecycle
    }

    pub fn diagnostics(&self) -> &DiagnosticProjectionState {
        self.diagnostics
    }

    pub fn frame_epoch(&self) -> SessionEpoch {
        self.frame_epoch
    }

    pub fn frame_revision(&self) -> ConfirmedRevision {
        self.frame_revision
    }

    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    pub fn limits(&self) -> &ClientLimits {
        self.limits
    }
}

/// The controller's exclusive borrowed publication ownership bundle: the
/// visible Arc, the staged observation queue, the pending input and local
/// cue state, the committed audio dedup and cancellation state, and the
/// pending lifecycle state. It grants no authority over the confirmed
/// mirror.
pub struct PublicationOwners<'a> {
    visible: &'a mut Arc<PresentationFrame>,
    observations: &'a mut Vec<AcceptedObservation>,
    input: &'a mut InputAdmissionOwner,
    audio: &'a mut AudioProjectionState,
    lifecycle: &'a mut LifecycleProjectionState,
}

/// The pending input publication owner: the input projection state beside the
/// admission owner's pending local cue-source metadata that a successful
/// publication consumes.
pub struct InputAdmissionOwner {
    projection: InputProjectionState,
}

impl InputAdmissionOwner {
    pub fn try_new(projection: InputProjectionState) -> Result<Self, ClientError> {
        Ok(Self { projection })
    }

    pub fn projection(&self) -> &InputProjectionState {
        &self.projection
    }
}

/// The checked consumption cursors of one publication: prefix counts for the
/// pending observation, input, local cue and lifecycle queues, never
/// arbitrary removal indices.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicationConsumption {
    observations: usize,
    input_records: usize,
    local_cues: usize,
    lifecycle_records: usize,
}

impl PublicationConsumption {
    pub fn try_new(
        observations: usize,
        input_records: usize,
        local_cues: usize,
        lifecycle_records: usize,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            observations,
            input_records,
            local_cues,
            lifecycle_records,
        })
    }

    pub fn observations(&self) -> usize {
        self.observations
    }

    pub fn input_records(&self) -> usize {
        self.input_records
    }

    pub fn local_cues(&self) -> usize {
        self.local_cues
    }

    pub fn lifecycle_records(&self) -> usize {
        self.lifecycle_records
    }
}

/// The checked publication reservation: the validated new Arc, the
/// preallocated proposed audio dedup state and the consumption cursors. All
/// bytes and counts are checked and staging state allocated before any owner
/// mutates; the final commit contains only infallible swaps.
pub struct PublicationReservation {
    frame: Arc<PresentationFrame>,
    audio: AudioDedupDelta,
    consume: PublicationConsumption,
}

impl PublicationReservation {
    pub fn try_new(
        frame: Arc<PresentationFrame>,
        audio: AudioDedupDelta,
        consume: PublicationConsumption,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            frame,
            audio,
            consume,
        })
    }

    pub fn frame(&self) -> &Arc<PresentationFrame> {
        &self.frame
    }

    pub fn audio(&self) -> &AudioDedupDelta {
        &self.audio
    }

    pub fn consume(&self) -> &PublicationConsumption {
        &self.consume
    }
}

/// The frozen serial assembly merge: entries sort by their retained
/// `ProjectionOrder` (confirmed before local at the same sampled revision,
/// observation revision/ordinal, then record ordinal) with the stable key as
/// the final tiebreak, so topic vectors interleave by actual source order,
/// never by parts order or equal source ticks. A duplicate stable record key
/// or an over-limit total rejects. This is the checked double of the private
/// assembly contract; each family assembler implements the same rule in its
/// owning file.
pub fn merge_ordered<R: Clone>(
    parts: Vec<Vec<OrderedRecord<R>>>,
    limit: usize,
) -> Result<Vec<OrderedRecord<R>>, ClientError> {
    let mut merged: Vec<OrderedRecord<R>> = parts.into_iter().flatten().collect();
    if merged.len() > limit {
        return Err(ClientError::Capacity);
    }
    let mut seen: Vec<StableRecordKey> = Vec::new();
    for entry in &merged {
        let identity = *entry.stable_key();
        if seen.contains(&identity) {
            return Err(ClientError::InvalidInput);
        }
        seen.push(identity);
    }
    merged.sort_by(|left, right| {
        left.order()
            .cmp(right.order())
            .then_with(|| left.stable_key().cmp(right.stable_key()))
    });
    Ok(merged)
}
