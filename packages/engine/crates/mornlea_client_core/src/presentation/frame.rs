//! The owned C2 semantic records, the checked family frame representation
//! and the frame validator.
//!
//! Every family record carries one shared checked header; every `Option` is a
//! tagged value, never a magic zero; and `validated_size` counts the entire
//! owned payload once per family and once per frame. The validator belongs to
//! this contract landing: it rejects a family count above the frozen ceiling
//! and a total above the frozen frame cap, mixed epoch or revision, duplicate
//! family keys, incompatible layout versions and revision-zero frames that
//! carry world records, before one immutable `Arc` is swapped. The real
//! assembler and the atomic swap are a later serial provider.

use mornlea_domain::{ChatBody, MiningState, PlayerId};

use crate::contracts::{
    ClientError, ClientLimits, CloseReason, ConfirmedRevision, FAMILY_ACTORS, FAMILY_AUDIO_CUES,
    FAMILY_DIAGNOSTICS, FAMILY_INPUT, FAMILY_INVENTORY_UI, FAMILY_LIFECYCLE, FAMILY_PLAYER_VIEW,
    FAMILY_SESSION, FAMILY_TERRAIN, FAMILY_WORLD_UI, FamilyKey, RecordHeader, SessionEpoch,
    SessionPhase,
};
use crate::input::{ClientIntentKind, ContainerToken};
use crate::preparation::{LightSummary, PreparedResourceKey, TerrainKey, TerrainVisibility};
use crate::presentation::{
    BoundedText, Correction, CueId, CueProvenance, ErrorClassCounters, FinitePositive, FiniteRay,
    FiniteUnit, InventoryUiView, MovementIntent, Pose, ProducerIdentity, QueueCounters,
    ResourceKey, UiOutcome, WorldUiView,
};

/// The `session@1` record: protocol hello/login/disconnect meaning with a
/// monotonic phase and exactly one terminal record per epoch.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionRecord {
    header: RecordHeader,
    phase: SessionPhase,
    player_id: Option<PlayerId>,
    terminal: Option<CloseReason>,
}

impl SessionRecord {
    pub fn try_new(
        header: RecordHeader,
        phase: SessionPhase,
        player_id: Option<PlayerId>,
        terminal: Option<CloseReason>,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            header,
            phase,
            player_id,
            terminal,
        })
    }

    pub fn header(&self) -> &RecordHeader {
        &self.header
    }

    pub fn phase(&self) -> &SessionPhase {
        &self.phase
    }

    pub fn player_id(&self) -> Option<PlayerId> {
        self.player_id
    }

    pub fn terminal(&self) -> Option<&CloseReason> {
        self.terminal.as_ref()
    }
}

/// The receipt state of one locally admitted or acknowledged input record.
/// Chat is never labeled confirmed merely because similar chat text arrives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputReceiptState {
    Queued,
    Rejected { class: ClientError },
    Confirmed { server_tick: Option<u64> },
}

/// The `input@1` record: local semantic ingress and any correlatable server
/// acknowledgment. No device scan code and no fabricated confirmation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputRecord {
    header: RecordHeader,
    local_sequence: Option<u64>,
    intent_kind: ClientIntentKind,
    receipt: InputReceiptState,
}

impl InputRecord {
    pub fn try_new(
        header: RecordHeader,
        local_sequence: Option<u64>,
        intent_kind: ClientIntentKind,
        receipt: InputReceiptState,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            header,
            local_sequence,
            intent_kind,
            receipt,
        })
    }

    pub fn header(&self) -> &RecordHeader {
        &self.header
    }

    pub fn local_sequence(&self) -> Option<u64> {
        self.local_sequence
    }

    pub fn intent_kind(&self) -> ClientIntentKind {
        self.intent_kind
    }

    pub fn receipt(&self) -> &InputReceiptState {
        &self.receipt
    }
}

/// The terrain material class of one published section or tile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerrainMaterial {
    Opaque,
    Cutout,
    Water,
}

/// The `terrain@1` record: chunk snapshot, change or forget plus preparation,
/// with no GPU handle and an ordered remove before same-key reuse.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainRecord {
    header: RecordHeader,
    dimension: mornlea_domain::Dimension,
    key: TerrainKey,
    content_revision: u64,
    generation: u64,
    material: TerrainMaterial,
    visibility: TerrainVisibility,
    light: LightSummary,
    resource: Option<PreparedResourceKey>,
}

impl TerrainRecord {
    /// Publishes a terrain record whose visibility tag must match the key
    /// tag: a near section is `Near`, a far tile is `Far`.
    pub fn try_new(
        header: RecordHeader,
        dimension: mornlea_domain::Dimension,
        key: TerrainKey,
        content_revision: u64,
        generation: u64,
        material: TerrainMaterial,
        visibility: TerrainVisibility,
        light: LightSummary,
        resource: Option<PreparedResourceKey>,
    ) -> Result<Self, ClientError> {
        let matches = matches!(
            (&key, &visibility),
            (TerrainKey::Section(_), TerrainVisibility::Near)
                | (TerrainKey::LodTile(_), TerrainVisibility::Far)
        );
        if !matches {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            header,
            dimension,
            key,
            content_revision,
            generation,
            material,
            visibility,
            light,
            resource,
        })
    }

    pub fn header(&self) -> &RecordHeader {
        &self.header
    }

    pub fn dimension(&self) -> mornlea_domain::Dimension {
        self.dimension
    }

    pub fn key(&self) -> &TerrainKey {
        &self.key
    }

    pub fn content_revision(&self) -> u64 {
        self.content_revision
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn material(&self) -> &TerrainMaterial {
        &self.material
    }

    pub fn visibility(&self) -> &TerrainVisibility {
        &self.visibility
    }

    pub fn light(&self) -> &LightSummary {
        &self.light
    }

    pub fn resource(&self) -> Option<&PreparedResourceKey> {
        self.resource.as_ref()
    }
}

/// The closed actor kind set, in contract variant order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ActorKind {
    RemotePlayer,
    Drop,
    Hostile,
    Passive,
    Projectile,
    Companion,
}

/// The closed actor identity union. The tag must match `ActorRecord.kind`
/// and every source identity is preserved without numeric conversion.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ActorId {
    RemotePlayer(PlayerId),
    Drop(mornlea_domain::DropId),
    Hostile(mornlea_domain::HostileId),
    Passive(mornlea_domain::PassiveId),
    Projectile(mornlea_domain::ProjectileId),
    Companion(mornlea_domain::CompanionId),
}

impl ActorId {
    /// The kind this identity names, used to reject a kind/identity mismatch.
    pub fn kind(&self) -> ActorKind {
        match self {
            ActorId::RemotePlayer(_) => ActorKind::RemotePlayer,
            ActorId::Drop(_) => ActorKind::Drop,
            ActorId::Hostile(_) => ActorKind::Hostile,
            ActorId::Passive(_) => ActorKind::Passive,
            ActorId::Projectile(_) => ActorKind::Projectile,
            ActorId::Companion(_) => ActorKind::Companion,
        }
    }
}

/// The actor dimension. Typed F1 actor dimensions are `Known`; the accepted
/// drop identity deliberately carries an open raw dimension that is never
/// silently coerced into a known world.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ActorDimension {
    Known(mornlea_domain::Dimension),
    DropRaw(i32),
}

/// The kind-tagged actor detail union. The detail tag must match the record
/// kind; `None` is legal only for a packet with no additional actor fields.
#[derive(Clone, Debug, PartialEq)]
pub enum ActorDetail {
    RemotePlayer {
        display_name: Option<BoundedText>,
        reset: Option<bool>,
    },
    Drop {
        block_index: u32,
        stack: mornlea_domain::ItemStack,
    },
    Hostile {
        archetype: u8,
        health: u8,
    },
    Passive {
        health: Option<u8>,
        grazing: Option<u8>,
        despawn_reason: Option<mornlea_domain::PassiveDespawnReason>,
    },
    Projectile {
        archetype: Option<u8>,
    },
    Companion {
        name: Option<BoundedText>,
        reset: Option<bool>,
    },
}

impl ActorDetail {
    /// The kind this detail variant belongs to.
    pub fn kind(&self) -> ActorKind {
        match self {
            ActorDetail::RemotePlayer { .. } => ActorKind::RemotePlayer,
            ActorDetail::Drop { .. } => ActorKind::Drop,
            ActorDetail::Hostile { .. } => ActorKind::Hostile,
            ActorDetail::Passive { .. } => ActorKind::Passive,
            ActorDetail::Projectile { .. } => ActorKind::Projectile,
            ActorDetail::Companion { .. } => ActorKind::Companion,
        }
    }
}

/// The `actors@1` record: one actor observation with kind and dimension as
/// part of its identity. A late packet cannot resurrect a despawn.
#[derive(Clone, Debug, PartialEq)]
pub struct ActorRecord {
    header: RecordHeader,
    kind: ActorKind,
    id: ActorId,
    dimension: ActorDimension,
    position: Option<[f64; 3]>,
    yaw: Option<f64>,
    pitch: Option<f64>,
    velocity: Option<[f64; 3]>,
    detail: Option<ActorDetail>,
}

impl ActorRecord {
    /// Publishes an actor record after checking that the identity, kind and
    /// dimension tags agree, that every value is finite, and that the raw
    /// drop dimension is used by drops alone.
    pub fn try_new(
        header: RecordHeader,
        kind: ActorKind,
        id: ActorId,
        dimension: ActorDimension,
        position: Option<[f64; 3]>,
        yaw: Option<f64>,
        pitch: Option<f64>,
        velocity: Option<[f64; 3]>,
        detail: Option<ActorDetail>,
    ) -> Result<Self, ClientError> {
        if id.kind() != kind {
            return Err(ClientError::InvalidInput);
        }
        if let Some(detail) = &detail {
            if detail.kind() != kind {
                return Err(ClientError::InvalidInput);
            }
        }
        match (kind, dimension) {
            (ActorKind::Drop, ActorDimension::DropRaw(_))
            | (ActorKind::Drop, ActorDimension::Known(_)) => {}
            (_, ActorDimension::DropRaw(_)) => return Err(ClientError::InvalidInput),
            _ => {}
        }
        let finite = |values: Option<[f64; 3]>| {
            values.is_none_or(|values| values.iter().all(|value| value.is_finite()))
        };
        if !finite(position) || !finite(velocity) {
            return Err(ClientError::InvalidInput);
        }
        if yaw.is_some_and(|value| !value.is_finite())
            || pitch.is_some_and(|value| !value.is_finite())
        {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            header,
            kind,
            id,
            dimension,
            position,
            yaw,
            pitch,
            velocity,
            detail,
        })
    }

    pub fn header(&self) -> &RecordHeader {
        &self.header
    }

    pub fn kind(&self) -> ActorKind {
        self.kind
    }

    pub fn id(&self) -> &ActorId {
        &self.id
    }

    pub fn dimension(&self) -> &ActorDimension {
        &self.dimension
    }

    pub fn position(&self) -> Option<[f64; 3]> {
        self.position
    }

    pub fn yaw(&self) -> Option<f64> {
        self.yaw
    }

    pub fn pitch(&self) -> Option<f64> {
        self.pitch
    }

    pub fn velocity(&self) -> Option<[f64; 3]> {
        self.velocity
    }

    pub fn detail(&self) -> Option<&ActorDetail> {
        self.detail.as_ref()
    }
}

/// The `player-view@1` record. The predicted pose is always explicitly
/// attributed and a correction cannot relabel it confirmed.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerViewRecord {
    header: RecordHeader,
    confirmed_pose: Pose,
    predicted_pose: Option<Pose>,
    look_ray: Option<FiniteRay>,
    movement: MovementIntent,
    correction: Option<Correction>,
    mining: MiningState,
}

impl PlayerViewRecord {
    pub fn try_new(
        header: RecordHeader,
        confirmed_pose: Pose,
        predicted_pose: Option<Pose>,
        look_ray: Option<FiniteRay>,
        movement: MovementIntent,
        correction: Option<Correction>,
        mining: MiningState,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            header,
            confirmed_pose,
            predicted_pose,
            look_ray,
            movement,
            correction,
            mining,
        })
    }

    pub fn header(&self) -> &RecordHeader {
        &self.header
    }

    pub fn confirmed_pose(&self) -> &Pose {
        &self.confirmed_pose
    }

    pub fn predicted_pose(&self) -> Option<&Pose> {
        self.predicted_pose.as_ref()
    }

    pub fn look_ray(&self) -> Option<&FiniteRay> {
        self.look_ray.as_ref()
    }

    pub fn movement(&self) -> &MovementIntent {
        &self.movement
    }

    pub fn correction(&self) -> Option<&Correction> {
        self.correction.as_ref()
    }

    pub fn mining(&self) -> &MiningState {
        &self.mining
    }
}

/// The `inventory-ui@1` record: the closed checked view payloads replace any
/// loose optional-field combination. Token/revision ownership and item
/// conservation stay in server authority.
#[derive(Clone, Debug, PartialEq)]
pub struct InventoryUiRecord {
    header: RecordHeader,
    view: InventoryUiView,
    token: Option<ContainerToken>,
    outcome: Option<UiOutcome>,
}

impl InventoryUiRecord {
    pub fn try_new(
        header: RecordHeader,
        view: InventoryUiView,
        token: Option<ContainerToken>,
        outcome: Option<UiOutcome>,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            header,
            view,
            token,
            outcome,
        })
    }

    pub fn header(&self) -> &RecordHeader {
        &self.header
    }

    pub fn view(&self) -> &InventoryUiView {
        &self.view
    }

    pub fn token(&self) -> Option<&ContainerToken> {
        self.token.as_ref()
    }

    pub fn outcome(&self) -> Option<&UiOutcome> {
        self.outcome.as_ref()
    }
}

/// The `world-ui@1` record: one accepted revision per complete source
/// observation, with bounded display text.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldUiRecord {
    header: RecordHeader,
    view: WorldUiView,
}

impl WorldUiRecord {
    pub fn try_new(header: RecordHeader, view: WorldUiView) -> Result<Self, ClientError> {
        Ok(Self { header, view })
    }

    pub fn header(&self) -> &RecordHeader {
        &self.header
    }

    pub fn view(&self) -> &WorldUiView {
        &self.view
    }
}

/// The audio cue category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioCueCategory {
    Ui,
    Footstep,
    World,
    Combat,
    Ambient,
}

/// The `audio-cues@1` record: semantic cue only, no device or stream.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioCueRecord {
    header: RecordHeader,
    cue_id: CueId,
    provenance: CueProvenance,
    category: AudioCueCategory,
    position: Option<[f64; 3]>,
    gain: FiniteUnit,
    pitch: FinitePositive,
}

impl AudioCueRecord {
    pub fn try_new(
        header: RecordHeader,
        cue_id: CueId,
        provenance: CueProvenance,
        category: AudioCueCategory,
        position: Option<[f64; 3]>,
        gain: FiniteUnit,
        pitch: FinitePositive,
    ) -> Result<Self, ClientError> {
        if position.is_some_and(|values| values.iter().any(|value| !value.is_finite())) {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            header,
            cue_id,
            provenance,
            category,
            position,
            gain,
            pitch,
        })
    }

    pub fn header(&self) -> &RecordHeader {
        &self.header
    }

    pub fn cue_id(&self) -> CueId {
        self.cue_id
    }

    pub fn provenance(&self) -> &CueProvenance {
        &self.provenance
    }

    pub fn category(&self) -> &AudioCueCategory {
        &self.category
    }

    pub fn position(&self) -> Option<[f64; 3]> {
        self.position
    }

    pub fn gain(&self) -> FiniteUnit {
        self.gain
    }

    pub fn pitch(&self) -> FinitePositive {
        self.pitch
    }
}

/// The lifecycle transition of one record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleTransition {
    Open,
    Reset,
    Close,
    Invalidate,
}

/// The `lifecycle@1` record: epoch open/close/reset, feature activation
/// generation and the resource invalidation order. Reset invalidates input,
/// preparation and handles before the next epoch and releases consumers
/// before providers.
#[derive(Clone, Debug, PartialEq)]
pub struct LifecycleRecord {
    header: RecordHeader,
    transition: LifecycleTransition,
    generation: u64,
    resource_order: Vec<ResourceKey>,
}

impl LifecycleRecord {
    pub fn try_new(
        header: RecordHeader,
        transition: LifecycleTransition,
        generation: u64,
        resource_order: Vec<ResourceKey>,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            header,
            transition,
            generation,
            resource_order,
        })
    }

    pub fn header(&self) -> &RecordHeader {
        &self.header
    }

    pub fn transition(&self) -> LifecycleTransition {
        self.transition
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn resource_order(&self) -> &[ResourceKey] {
        &self.resource_order
    }
}

/// The `diagnostics@1` record: bounded owner-maintained counters and
/// identity correlation only. Raw packet and command payloads are excluded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiagnosticRecord {
    header: RecordHeader,
    producer: ProducerIdentity,
    frame_index: u64,
    queue_high_water: QueueCounters,
    rejected: ErrorClassCounters,
}

impl DiagnosticRecord {
    pub fn try_new(
        header: RecordHeader,
        producer: ProducerIdentity,
        frame_index: u64,
        queue_high_water: QueueCounters,
        rejected: ErrorClassCounters,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            header,
            producer,
            frame_index,
            queue_high_water,
            rejected,
        })
    }

    pub fn header(&self) -> &RecordHeader {
        &self.header
    }

    pub fn producer(&self) -> &ProducerIdentity {
        &self.producer
    }

    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    pub fn queue_high_water(&self) -> &QueueCounters {
        &self.queue_high_water
    }

    pub fn rejected(&self) -> &ErrorClassCounters {
        &self.rejected
    }
}

/// The checked per-frame family entry: the logical key and the closed record
/// vector. The key/variant pairing is enforced by the constructor.
#[derive(Clone, Debug, PartialEq)]
pub struct FamilyFrame {
    key: FamilyKey,
    records: FamilyRecords,
}

/// The closed family record vectors in logical family order.
#[derive(Clone, Debug, PartialEq)]
pub enum FamilyRecords {
    Session(Vec<SessionRecord>),
    Input(Vec<InputRecord>),
    Terrain(Vec<TerrainRecord>),
    Actors(Vec<ActorRecord>),
    PlayerView(Vec<PlayerViewRecord>),
    InventoryUi(Vec<InventoryUiRecord>),
    WorldUi(Vec<WorldUiRecord>),
    AudioCues(Vec<AudioCueRecord>),
    Lifecycle(Vec<LifecycleRecord>),
    Diagnostics(Vec<DiagnosticRecord>),
}

impl FamilyRecords {
    /// The logical family name this variant publishes under.
    pub fn key(&self) -> FamilyKey {
        let name = match self {
            FamilyRecords::Session(_) => FAMILY_SESSION,
            FamilyRecords::Input(_) => FAMILY_INPUT,
            FamilyRecords::Terrain(_) => FAMILY_TERRAIN,
            FamilyRecords::Actors(_) => FAMILY_ACTORS,
            FamilyRecords::PlayerView(_) => FAMILY_PLAYER_VIEW,
            FamilyRecords::InventoryUi(_) => FAMILY_INVENTORY_UI,
            FamilyRecords::WorldUi(_) => FAMILY_WORLD_UI,
            FamilyRecords::AudioCues(_) => FAMILY_AUDIO_CUES,
            FamilyRecords::Lifecycle(_) => FAMILY_LIFECYCLE,
            FamilyRecords::Diagnostics(_) => FAMILY_DIAGNOSTICS,
        };
        FamilyKey::try_new(name).expect("contract family names are known")
    }

    pub fn record_count(&self) -> usize {
        match self {
            FamilyRecords::Session(records) => records.len(),
            FamilyRecords::Input(records) => records.len(),
            FamilyRecords::Terrain(records) => records.len(),
            FamilyRecords::Actors(records) => records.len(),
            FamilyRecords::PlayerView(records) => records.len(),
            FamilyRecords::InventoryUi(records) => records.len(),
            FamilyRecords::WorldUi(records) => records.len(),
            FamilyRecords::AudioCues(records) => records.len(),
            FamilyRecords::Lifecycle(records) => records.len(),
            FamilyRecords::Diagnostics(records) => records.len(),
        }
    }

    /// Whether this family may carry records at confirmed revision zero:
    /// only session, lifecycle, diagnostics and local input records may.
    fn allowed_at_revision_zero(&self) -> bool {
        matches!(
            self,
            FamilyRecords::Session(_)
                | FamilyRecords::Input(_)
                | FamilyRecords::Lifecycle(_)
                | FamilyRecords::Diagnostics(_)
        )
    }

    /// The family's entire owned payload in bytes.
    pub(crate) fn owned_bytes(&self) -> usize {
        let record_bytes = match self {
            FamilyRecords::Session(records) => {
                records.iter().map(session_record_bytes).sum::<usize>()
            }
            FamilyRecords::Input(records) => records.iter().map(input_record_bytes).sum::<usize>(),
            FamilyRecords::Terrain(records) => {
                records.iter().map(terrain_record_bytes).sum::<usize>()
            }
            FamilyRecords::Actors(records) => records.iter().map(actor_record_bytes).sum::<usize>(),
            FamilyRecords::PlayerView(records) => {
                records.iter().map(player_view_record_bytes).sum::<usize>()
            }
            FamilyRecords::InventoryUi(records) => {
                records.iter().map(inventory_ui_record_bytes).sum::<usize>()
            }
            FamilyRecords::WorldUi(records) => {
                records.iter().map(world_ui_record_bytes).sum::<usize>()
            }
            FamilyRecords::AudioCues(records) => {
                records.iter().map(audio_cue_record_bytes).sum::<usize>()
            }
            FamilyRecords::Lifecycle(records) => {
                records.iter().map(lifecycle_record_bytes).sum::<usize>()
            }
            FamilyRecords::Diagnostics(records) => {
                records.iter().map(diagnostic_record_bytes).sum::<usize>()
            }
        };
        // One vector length field plus the key tag beside the record payload.
        record_bytes.saturating_add(8).saturating_add(1)
    }
}

impl FamilyFrame {
    /// Publishes a family entry, rejecting a key that disagrees with the
    /// record vector it is paired with.
    pub fn try_new(key: FamilyKey, records: FamilyRecords) -> Result<Self, ClientError> {
        if records.key() != key {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self { key, records })
    }

    pub fn key(&self) -> &FamilyKey {
        &self.key
    }

    pub fn records(&self) -> &FamilyRecords {
        &self.records
    }
}

/// The owned immutable presentation frame: the frozen layout version, the
/// frame identity and the checked family entries.
#[derive(Clone, Debug, PartialEq)]
pub struct PresentationFrame {
    layout_major: u16,
    layout_minor: u16,
    session_epoch: SessionEpoch,
    confirmed_revision: ConfirmedRevision,
    frame_index: u64,
    families: Vec<FamilyFrame>,
}

/// The frozen frame layout version this contract publishes.
pub const FRAME_LAYOUT_MAJOR: u16 = 1;
pub const FRAME_LAYOUT_MINOR: u16 = 0;

impl PresentationFrame {
    /// Publishes an immutable candidate frame. The layout version and the
    /// family coherence are validated separately by [`Self::validate`], so a
    /// candidate that would fail publication can be built and rejected
    /// without ever becoming visible.
    pub fn try_new(
        session_epoch: SessionEpoch,
        confirmed_revision: ConfirmedRevision,
        frame_index: u64,
        families: Vec<FamilyFrame>,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            layout_major: FRAME_LAYOUT_MAJOR,
            layout_minor: FRAME_LAYOUT_MINOR,
            session_epoch,
            confirmed_revision,
            frame_index,
            families,
        })
    }

    pub fn layout_major(&self) -> u16 {
        self.layout_major
    }

    pub fn layout_minor(&self) -> u16 {
        self.layout_minor
    }

    pub fn session_epoch(&self) -> SessionEpoch {
        self.session_epoch
    }

    pub fn confirmed_revision(&self) -> ConfirmedRevision {
        self.confirmed_revision
    }

    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    pub fn families(&self) -> &[FamilyFrame] {
        &self.families
    }

    /// The frame's entire owned payload: the fixed scalar fields, every
    /// family's key and record vectors, and every record's header, tags,
    /// text bytes, vector elements and opaque keys. Checked throughout.
    pub fn validated_size(&self) -> Result<usize, ClientError> {
        let mut total: usize = 2 + 2 + 8 + 8 + 8 + 8;
        for family in &self.families {
            // Logical-name byte length, major/minor and the records vector.
            total = total
                .checked_add(family.key.logical_name.len())
                .and_then(|value| value.checked_add(2 + 2))
                .ok_or(ClientError::Capacity)?;
            total = total
                .checked_add(family.records.owned_bytes())
                .ok_or(ClientError::Capacity)?;
        }
        Ok(total)
    }

    /// The frame validator. A frame publishes only when every family child
    /// passes epoch/revision coherence, the per-family count and whole-frame
    /// byte caps, the required-family revision-zero restriction and the
    /// layout compatibility rule. Any failure is typed and leaves the
    /// previously visible frame untouched.
    pub fn validate(&self, limits: &ClientLimits) -> Result<(), ClientError> {
        if self.layout_major != FRAME_LAYOUT_MAJOR {
            return Err(ClientError::IncompatibleVersion);
        }
        if self.layout_minor != FRAME_LAYOUT_MINOR {
            return Err(ClientError::IncompatibleVersion);
        }
        let mut seen: Vec<&'static str> = Vec::new();
        for family in &self.families {
            if seen.contains(&family.key.logical_name) {
                return Err(ClientError::InvalidInput);
            }
            seen.push(family.key.logical_name);
            if family.records.record_count() > limits.family_records() {
                return Err(ClientError::Capacity);
            }
            if self.confirmed_revision.get() == 0
                && family.records.record_count() > 0
                && !family.records.allowed_at_revision_zero()
            {
                return Err(ClientError::InvalidInput);
            }
            check_family_headers(family, self.session_epoch, self.confirmed_revision)?;
        }
        let size = self.validated_size()?;
        if size > limits.frame_bytes() {
            return Err(ClientError::Capacity);
        }
        Ok(())
    }
}

/// Header coherence: every record header must carry the frame's epoch and
/// revision, because child headers are rebased onto the coherent candidate.
fn check_family_headers(
    family: &FamilyFrame,
    epoch: SessionEpoch,
    revision: ConfirmedRevision,
) -> Result<(), ClientError> {
    let check = |header: &RecordHeader| -> Result<(), ClientError> {
        if header.epoch() != epoch {
            return Err(ClientError::InvalidInput);
        }
        if header.revision() != revision {
            return Err(ClientError::InvalidInput);
        }
        Ok(())
    };
    match &family.records {
        FamilyRecords::Session(records) => records.iter().try_for_each(|r| check(r.header())),
        FamilyRecords::Input(records) => records.iter().try_for_each(|r| check(r.header())),
        FamilyRecords::Terrain(records) => records.iter().try_for_each(|r| check(r.header())),
        FamilyRecords::Actors(records) => records.iter().try_for_each(|r| check(r.header())),
        FamilyRecords::PlayerView(records) => records.iter().try_for_each(|r| check(r.header())),
        FamilyRecords::InventoryUi(records) => records.iter().try_for_each(|r| check(r.header())),
        FamilyRecords::WorldUi(records) => records.iter().try_for_each(|r| check(r.header())),
        FamilyRecords::AudioCues(records) => records.iter().try_for_each(|r| check(r.header())),
        FamilyRecords::Lifecycle(records) => records.iter().try_for_each(|r| check(r.header())),
        FamilyRecords::Diagnostics(records) => records.iter().try_for_each(|r| check(r.header())),
    }
}

/// Fixed-size helpers: one place owns the byte accounting for the record
/// payloads, keeping `validated_size` checked and deterministic.

fn owned_bytes_of_header(header: &RecordHeader) -> usize {
    header.owned_bytes()
}

fn session_record_bytes(record: &SessionRecord) -> usize {
    let terminal = record
        .terminal()
        .map(|reason| match reason {
            CloseReason::LocalClose
            | CloseReason::Timeout
            | CloseReason::Capacity
            | CloseReason::Internal => 1,
            CloseReason::RemoteDisconnect(text) | CloseReason::LoginRejected(text) => {
                1 + 1 + text.as_str().len()
            }
        })
        .unwrap_or(0);
    owned_bytes_of_header(record.header()) + 1 + record.player_id().map_or(0, |_| 1 + 16) + terminal
}

fn input_record_bytes(record: &InputRecord) -> usize {
    owned_bytes_of_header(record.header())
        + 1
        + record.local_sequence().map_or(0, |_| 8)
        + 1
        + match record.receipt() {
            InputReceiptState::Queued => 1,
            InputReceiptState::Rejected { .. } => 1 + 1,
            InputReceiptState::Confirmed { server_tick } => 1 + 1 + server_tick.map_or(0, |_| 8),
        }
}

fn terrain_key_bytes(key: &TerrainKey) -> usize {
    1 + match key {
        TerrainKey::Section(_) => 8 + 1,
        TerrainKey::LodTile(_) => 4 + 4,
    }
}

fn terrain_record_bytes(record: &TerrainRecord) -> usize {
    owned_bytes_of_header(record.header())
        + 1
        + terrain_key_bytes(record.key())
        + 8
        + 8
        + 1
        + 1
        + 2
        + record.resource().map_or(0, |_| {
            1 + 8 + 1 + terrain_key_bytes(record.key()) + 8 + 8 + 8
        })
}

fn actor_identity_bytes(id: &ActorId) -> usize {
    1 + match id {
        ActorId::RemotePlayer(_) | ActorId::Companion(_) => 16,
        ActorId::Hostile(_) | ActorId::Passive(_) | ActorId::Projectile(_) => 8,
        ActorId::Drop(_) => 4 + 8 + 1 + 4,
    }
}

fn actor_record_bytes(record: &ActorRecord) -> usize {
    owned_bytes_of_header(record.header())
        + 1
        + actor_identity_bytes(record.id())
        + 1
        + match record.dimension() {
            ActorDimension::Known(_) => 1,
            ActorDimension::DropRaw(_) => 4,
        }
        + record.position().map_or(0, |_| 1 + 24)
        + record.yaw().map_or(0, |_| 1 + 8)
        + record.pitch().map_or(0, |_| 1 + 8)
        + record.velocity().map_or(0, |_| 1 + 24)
        + record
            .detail()
            .map(|detail| {
                1 + match detail {
                    ActorDetail::RemotePlayer {
                        display_name,
                        reset,
                    } => {
                        display_name
                            .as_ref()
                            .map_or(0, |text| 1 + text.as_str().len())
                            + reset.map_or(0, |_| 1)
                    }
                    ActorDetail::Drop { .. } => 4 + 1 + 2 + 1 + 2,
                    ActorDetail::Hostile { .. } => 2,
                    ActorDetail::Passive { .. } => 3,
                    ActorDetail::Projectile { .. } => 1,
                    ActorDetail::Companion { name, reset } => {
                        name.as_ref().map_or(0, |text| 1 + text.as_str().len())
                            + reset.map_or(0, |_| 1)
                    }
                }
            })
            .unwrap_or(0)
}

fn player_view_record_bytes(record: &PlayerViewRecord) -> usize {
    owned_bytes_of_header(record.header())
        + 24
        + 8
        + 8
        + record.predicted_pose().map_or(0, |_| 40)
        + record.look_ray().map_or(0, |_| 1 + 24 + 8 + 4)
        + 1
        + record.movement().control().map_or(0, |_| 1 + 6 + 8 + 4)
        + 1
        + record.correction().map_or(0, |_| 1 + 8 + 1)
        + match record.mining() {
            MiningState::Idle => 1,
            MiningState::Active(_) => 1 + 1 + 12 + 2 + 2 + 1,
        }
}

fn stack_bytes() -> usize {
    2 + 1 + 2
}

fn inventory_ui_record_bytes(record: &InventoryUiRecord) -> usize {
    owned_bytes_of_header(record.header())
        + 1
        + match record.view() {
            InventoryUiView::Inventory(state) => {
                1 + (state.hotbar().len() + state.backpack().len()) * stack_bytes()
            }
            InventoryUiView::Crafting(state) => {
                1 + state.slots().len() * stack_bytes() + stack_bytes() + 1
            }
            InventoryUiView::Furnace(_state) => 1 + stack_bytes() * 3 + 1 + 2,
            InventoryUiView::Chest(state) => 1 + state.items().len() * stack_bytes(),
            InventoryUiView::Closed(_) => 1 + 1 + 8 + 1 + 4,
            InventoryUiView::Rejected(_) => 1 + 8 + 1,
        }
        + record.token().map_or(0, |_| 1 + 8 + 1 + 8 + 1 + 4 + 8)
        + record.outcome().map_or(0, |outcome| match outcome {
            UiOutcome::Rejected { .. } => 1 + 1 + 8 + 1,
            UiOutcome::PlacementAccepted { .. } => 1 + 1 + 8,
        })
}

fn world_ui_record_bytes(record: &WorldUiRecord) -> usize {
    owned_bytes_of_header(record.header())
        + 1
        + match record.view() {
            crate::presentation::WorldUiView::Environment(_) => 1 + 2 + 8 + 1 + 1 + 1 + 1,
            crate::presentation::WorldUiView::Survival(_) => 1 + 1 + 2 + 1 + 1 + 1,
            crate::presentation::WorldUiView::Chat(event) => {
                1 + 8 + 16 + event.player_name().as_str().len() + chat_body_bytes(event.body())
            }
            crate::presentation::WorldUiView::Task(task) => {
                1 + 8 + 8 + 4 + 16 + task.command().as_str().len() + 1
            }
            crate::presentation::WorldUiView::Prompt(prompt) => prompt
                .as_ref()
                .map_or(1, |view| 1 + 1 + 12 + 1 + view.label().as_str().len()),
        }
}

fn chat_body_bytes(body: &ChatBody) -> usize {
    // The event value itself is owned: id, player identity, name and the
    // closed body union with its restated or speech text.
    1 + match body {
        ChatBody::Accepted { command, .. } | ChatBody::QueueFull { command, .. } => {
            16 + 1 + command.as_str().len()
        }
        ChatBody::InvalidFormat => 0,
        ChatBody::UnknownCompanion { name } => 1 + name.as_str().len(),
        ChatBody::NotFollowing { command, .. } => 16 + 1 + command.as_str().len(),
        ChatBody::Task { command, .. } => 16 + 1 + command.as_str().len(),
        ChatBody::Speech { text, .. } => 16 + 1 + text.as_str().len(),
    }
}

fn audio_cue_record_bytes(record: &AudioCueRecord) -> usize {
    owned_bytes_of_header(record.header())
        + 2
        + match record.provenance() {
            CueProvenance::Confirmed {
                authoritative_event_id,
                ..
            } => 1 + 8 + 8 + 4 + 1 + authoritative_event_id.map_or(0, |_| 8),
            CueProvenance::Predicted { .. } => 1 + 8,
            CueProvenance::Local { .. } => 1 + 8,
        }
        + 1
        + record.position().map_or(0, |_| 1 + 24)
        + 4
        + 4
}

fn lifecycle_record_bytes(record: &LifecycleRecord) -> usize {
    owned_bytes_of_header(record.header())
        + 1
        + 8
        + 8
        + record
            .resource_order()
            .iter()
            .map(|key| 1 + resource_key_bytes(key))
            .sum::<usize>()
}

fn resource_key_bytes(key: &ResourceKey) -> usize {
    match key {
        ResourceKey::InputJournal
        | ResourceKey::PreparationQueue
        | ResourceKey::PresentationFrames
        | ResourceKey::BridgeHandles => 1,
        ResourceKey::Feature(_) => 8,
    }
}

fn diagnostic_record_bytes(record: &DiagnosticRecord) -> usize {
    owned_bytes_of_header(record.header()) + 20 + 32 + 8 + 7 * 8 + 6 * 8
}
