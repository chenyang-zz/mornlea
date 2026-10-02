//! Serial whole-frame assembly and the atomic publication transaction.
//!
//! The table drives the real assembly seam over every accepted real family
//! provider: the real mirror provider and its committed observation queue,
//! the real admission path with its pending input metadata and native local
//! cue events, the real prediction replay owner, the real terrain publisher
//! over the two real preparation owners, the real per-kind actor, inventory,
//! world, audio and diagnostics projections behind their serial family
//! assemblers, and the real login provider's lifecycle Open/terminal records.
//! One successful publication is one atomic transaction: the candidate
//! validates whole, the reservation checks every byte, count and dequeued
//! source key before any owner mutates, and the commit swaps the owner
//! cursors and the visible Arc exactly once. A failed validation,
//! preallocation or over-cap frame consumes no event, no removal, no
//! cancellation and no dedup key — the queues stay retry-owned and a valid
//! retry commits once. The old Arc stays readable across every failure and
//! after every successful swap.

use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use crate::preparation_port::fixture_params;
use crate::support::{
    DeterministicClock, MemoryConnectorDouble, frame_server_packet, registry_with_memory,
};
use mornlea_client_core::contracts::{
    ClientConfig, ClientError, ClientIdentity, ClientWorkBudget, ConfirmedRevision, FAMILY_ACTORS,
    FAMILY_AUDIO_CUES, FAMILY_DIAGNOSTICS, FAMILY_INPUT, FAMILY_INVENTORY_UI, FAMILY_LIFECYCLE,
    FAMILY_PLAYER_VIEW, FAMILY_SESSION, FAMILY_TERRAIN, FAMILY_WORLD_UI, FamilyKey,
    FamilyOperation, ObservationKey, RecordHeader, SessionEpoch, SessionPhase,
};
use mornlea_client_core::input::{
    ClientIntent, ClientIntentKind, InputAction, InputAdmissionState, InputBatch,
    InputProjectionState, InputTranslator,
};
use mornlea_client_core::prediction::{PlayerProjectionState, PredictionReplay, StepEnvironment};
use mornlea_client_core::preparation::lod::LodSelection;
use mornlea_client_core::preparation::{
    LodConfig, LodStep, OwnedMeshView, PreparationJob, PreparationPayload, PreparationQueue,
    PreparedResourceKey, SectionKey, TerrainKey,
};
use mornlea_client_core::presentation::actors::{
    companion::project_companion, drop::project_drop, hostile::project_hostile,
    passive::project_passive, projectile::project_projectile, remote_player::project_remote_player,
};
use mornlea_client_core::presentation::assembly::{
    assemble_frame, commit_publication, prepare_publication, publication_owners,
};
use mornlea_client_core::presentation::family_actors::assemble_actors;
use mornlea_client_core::presentation::family_audio::project_audio;
use mornlea_client_core::presentation::family_diagnostics::project_diagnostics;
use mornlea_client_core::presentation::family_inventory_ui::assemble_inventory_ui;
use mornlea_client_core::presentation::family_player_view::project_player_view;
use mornlea_client_core::presentation::family_terrain::TerrainPublisher;
use mornlea_client_core::presentation::family_world_ui::assemble_world_ui;
use mornlea_client_core::presentation::frame::{
    ActorRecord, AudioCueRecord, FamilyFrame, FamilyRecords, InputReceiptState as Receipt,
    InputRecord, LifecycleRecord, LifecycleTransition, PresentationFrame, SessionRecord,
    TerrainRecord, WorldUiRecord,
};
use mornlea_client_core::presentation::inventory_ui::{
    container::project_container, crafting::project_crafting, furnace::project_furnace,
    inventory::project_inventory,
};
use mornlea_client_core::presentation::world_ui::{
    chat::project_chat, environment::project_environment, prompt::project_prompt,
    survival::project_survival, task::project_task,
};
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioDedupDelta, AudioDedupKey, AudioProjectionState, CueId,
    DiagnosticProjectionState, ErrorClassCounters, InputAdmissionOwner, LifecycleProjectionState,
    MovementIntent, Pose, ProducerIdentity, ProjectionView, PublicationConsumption,
    PublicationOwners, QueueCounters, ResourceKey,
};
use mornlea_client_core::session::login::LoginSession;
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_client_core::session::{
    ActorConfirmed, ConfirmedMirrorParts, InventoryConfirmed, WorldConfirmed, WorldUiConfirmed,
};
use mornlea_client_core::{ClientEndpoint, CueProvenance};
use mornlea_domain::{
    ChatBody, ChatEvent, ChatEventParts, ChunkPos, CombatHit, CombatTarget, ContainerKind,
    ContainerRef, Dimension, DisplayName, Event, FiniteVec3, HeldActions, HostileId, HostileKind,
    HostileSpawn, HostileSpawnParts, HostileSpawnRecord, HostileSpawnRecordParts, LookAngles,
    MiningState, MotionState, MotionStateParts, Movement, PlayerControl, PlayerControlParts,
    PlayerId, PlayerState, PlayerStateParts, RemotePlayerDespawn, RemotePlayerSpawn,
    RemotePlayerSpawnParts, Season, SurvivalState, SurvivalStateParts, Weather, WorldState,
    WorldStateParts,
};
use mornlea_engine::native::contracts::mesh::{MeshModel, MeshRegistry, MeshRegistryEntry};
use mornlea_engine::native::contracts::world::WorldgenParams;
use mornlea_protocol::{
    ClientHello, LOGIN_SERVER_FULL, LoginReject, LoginStart, ServerPacket, write_frame,
};

/// The epoch every fixture mirror, admission owner and observation key
/// carries unless the case names another one.
const EPOCH: u64 = 11;

/// The confirmed content revision the fixture world holds for chunk (0, 0);
/// the terrain fixture's near key carries the same identity.
const CHUNK_REVISION: u64 = 5;

// --- checked-value helpers ---

fn epoch(value: u64) -> SessionEpoch {
    SessionEpoch::try_new(value).expect("nonzero epoch")
}

fn limits() -> mornlea_client_core::contracts::ClientLimits {
    mornlea_client_core::contracts::ClientLimits::try_new().expect("frozen limits")
}

/// The frozen limits with only the family-record and frame-byte caps
/// replaced, the accepted tighter-configuration proof technique.
fn limits_with(
    family_records: usize,
    frame_bytes: usize,
) -> mornlea_client_core::contracts::ClientLimits {
    let frozen = limits();
    mornlea_client_core::contracts::ClientLimits::try_new_with(
        frozen.queued_input_events(),
        frozen.inbound_observations(),
        frozen.inbound_bytes(),
        frozen.outbound_commands(),
        frozen.outbound_bytes(),
        frozen.prediction_journal(),
        frozen.message_work(),
        frozen.mesh_work(),
        frozen.preparation_results(),
        frozen.preparation_bytes(),
        family_records,
        frame_bytes,
    )
    .expect("cap at or below the frozen ceiling")
}

fn look(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).expect("finite fixture look")
}

fn player(byte: u8) -> PlayerId {
    PlayerId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, byte])
        .expect("uuid v4")
}

fn packet(event: Event) -> ServerPacket {
    ServerPacket::try_from(event).expect("publication converts to its packet")
}

fn chat(event_id: u64) -> Event {
    Event::Chat(
        ChatEvent::try_new(ChatEventParts {
            event_id,
            player_id: player(3),
            player_name: DisplayName::try_from_canonical("pilot".to_string())
                .expect("canonical fixture name"),
            body: ChatBody::InvalidFormat,
        })
        .expect("chat event"),
    )
}

fn combat(server_tick: u64) -> Event {
    Event::CombatHit(CombatHit::try_new(server_tick, 3, CombatTarget::Hostile).expect("combat hit"))
}

/// One overworld authority record at the caller's tick, acknowledged
/// sequence and position, the shape the accepted replay cases step with.
#[allow(clippy::too_many_arguments)]
fn authority(tick: u64, last_sequence: u64, position: [f32; 3], ready: bool) -> PlayerState {
    PlayerState::new(PlayerStateParts {
        server_tick: tick,
        last_input_sequence: last_sequence,
        dimension: Dimension::OVERWORLD,
        motion: MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("finite fixture position"),
            velocity: FiniteVec3::try_new([0.0, 0.0, 0.0]).expect("finite fixture velocity"),
            on_ground: true,
        }),
        look: look(0.2, -0.1),
        ready,
        reset: false,
        mining: MiningState::Idle,
        survival: survival(),
        world: world(),
    })
}

fn survival() -> SurvivalState {
    SurvivalState::try_new(SurvivalStateParts {
        health: 20,
        oxygen: 300,
        hunger: 20,
        saturation_zero: false,
        armor_points: 0,
    })
    .expect("checked survival fixture")
}

fn world() -> WorldState {
    WorldState::try_new(WorldStateParts {
        day_phase_offset: 0,
        world_time_ticks: 0,
        weather: Weather::Clear,
        season: Season::Spring,
        season_progress: 0,
        temperature: 0,
    })
    .expect("checked world fixture")
}

fn remote_spawn(id: PlayerId, server_tick: u64) -> Event {
    Event::RemotePlayerSpawn(RemotePlayerSpawn::new(RemotePlayerSpawnParts {
        player_id: id,
        display_name: DisplayName::try_from_canonical("peer".to_string())
            .expect("canonical fixture name"),
        server_tick,
        dimension: Dimension::OVERWORLD,
        position: FiniteVec3::try_new([1.0, 0.0, 1.0]).expect("finite fixture position"),
        look: look(0.0, 0.0),
    }))
}

fn remote_despawn(id: PlayerId) -> Event {
    Event::RemotePlayerDespawn(RemotePlayerDespawn::new(id))
}

fn hostile_batch(server_tick: u64, ids: &[u64]) -> Event {
    let spawns = ids
        .iter()
        .map(|id| {
            HostileSpawnRecord::try_new(HostileSpawnRecordParts {
                id: HostileId::try_new(*id).expect("hostile id"),
                dimension: Dimension::OVERWORLD,
                position: FiniteVec3::try_new([2.0, 0.0, 2.0]).expect("finite fixture position"),
                yaw: 0.0,
                health: 10,
                kind: HostileKind::Nightwalker,
            })
            .expect("hostile spawn record")
        })
        .collect::<Vec<_>>();
    Event::HostileSpawn(
        HostileSpawn::try_new(HostileSpawnParts {
            server_tick,
            spawns: spawns.into_boxed_slice(),
        })
        .expect("hostile spawn batch"),
    )
}

fn chest() -> Event {
    Event::ChestState(
        mornlea_domain::ChestState::try_new(mornlea_domain::ChestStateParts {
            container: ContainerRef::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, 0, 1)
                .expect("chest reference"),
            items: [mornlea_domain::ItemStack::EMPTY; 27],
        })
        .expect("chest state"),
    )
}

// --- the walking prediction fixture (shared shape with the audio table) ---

const WALK_ORIGIN: [i32; 3] = [-4, -1, -8];
const WALK_DIMS: [u32; 3] = [9, 6, 17];

fn fixture_cells(dims: [u32; 3]) -> Vec<mornlea_engine::native::contracts::CollisionCell> {
    use mornlea_engine::native::contracts::{Aabb, CollisionCell};
    let cube = Aabb {
        minimum: [0.0; 3],
        maximum: [1.0, 1.0, 1.0],
    };
    let empty = Aabb {
        minimum: [0.0; 3],
        maximum: [0.0; 3],
    };
    let mut cells = Vec::new();
    for y in 0..dims[1] {
        for _x in 0..dims[0] {
            for _z in 0..dims[2] {
                let floor = y == 0;
                let mut boxes = [empty; 8];
                let used = if floor {
                    boxes[0] = cube;
                    1u8
                } else {
                    0u8
                };
                cells.push(CollisionCell::try_new(true, boxes, used).expect("checked cell"));
            }
        }
    }
    cells
}

fn walking_grid(
    cells: &[mornlea_engine::native::contracts::CollisionCell],
) -> mornlea_engine::native::contracts::CollisionGrid<'_> {
    mornlea_engine::native::contracts::CollisionGrid::try_new(WALK_ORIGIN, WALK_DIMS, cells)
        .expect("walking fixture grid")
}

fn forward(yaw: f32) -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: 0,
            move_z: 1,
            jump: false,
        },
        look: look(yaw, 0.0),
        actions: HeldActions {
            primary: false,
            eating: false,
            sprinting: false,
            sneaking: false,
        },
    })
}

/// One directly constructed checked player state with an empty journal for
/// the cases whose subject is not the prediction owner.
fn idle_player(value: u64, revision: ConfirmedRevision) -> PlayerProjectionState {
    PlayerProjectionState::try_new(
        epoch(value),
        revision,
        Pose::try_new([0.5, 0.0, 0.5], 0.2, -0.1).expect("finite fixture pose"),
        None,
        Vec::new(),
        None,
        None,
        MovementIntent::try_new(None, true).expect("checked movement intent"),
    )
    .expect("checked player projection state")
}

// --- the frame fixture: every real projection owner beside the owners ---

/// The controller-side fixture: the real mirror provider with its committed
/// observation queue, the real admission owner's projection state, the audio,
/// lifecycle and diagnostics state owners and the frozen limits. The
/// observation queue is the controller-owned copy of the provider's
/// committed queue — the same entries the publication cursors drain.
struct FrameFixture {
    epoch: SessionEpoch,
    mirror: MirrorProvider,
    observations: Vec<AcceptedObservation>,
    input_owner: InputAdmissionOwner,
    audio: AudioProjectionState,
    lifecycle: LifecycleProjectionState,
    diagnostics: DiagnosticProjectionState,
    limits: mornlea_client_core::contracts::ClientLimits,
}

fn frame_fixture(value: u64, with_chunk: bool) -> FrameFixture {
    let epoch = epoch(value);
    let limits = limits();
    let world = if with_chunk {
        Some(
            WorldConfirmed::try_new(BTreeMap::new())
                .expect("world")
                .with_chunk(Dimension::OVERWORLD, ChunkPos::new(0, 0), CHUNK_REVISION),
        )
    } else {
        None
    };
    let mirror = MirrorProvider::from_parts(
        ConfirmedMirrorParts {
            epoch,
            revision: ConfirmedRevision::new(1),
            phase: SessionPhase::Admitted,
            world,
            actors: Some(ActorConfirmed::try_new().expect("actor store")),
            inventory: Some(InventoryConfirmed::try_new().expect("inventory store")),
            world_ui: Some(WorldUiConfirmed::try_new().expect("world ui store")),
        },
        limits,
    )
    .expect("admitted mirror provider");
    FrameFixture {
        epoch,
        mirror,
        observations: Vec::new(),
        input_owner: InputAdmissionOwner::try_new(
            InputProjectionState::try_new(1).expect("input projection state"),
        )
        .expect("input owner"),
        audio: AudioProjectionState::try_new().expect("audio state"),
        lifecycle: LifecycleProjectionState::try_new(epoch.get()).expect("lifecycle state"),
        diagnostics: DiagnosticProjectionState::try_new(
            ProducerIdentity::try_new([7; 20], [9; 32]).expect("producer identity"),
            QueueCounters::default(),
            ErrorClassCounters::default(),
        )
        .expect("diagnostics state"),
        limits,
    }
}

impl FrameFixture {
    /// Commits one event through the real mirror provider and appends the
    /// provider's enriched entry to the controller-owned queue the
    /// publication cursors drain.
    fn commit_observation(&mut self, event: Event) -> ObservationKey {
        let next = self.mirror.mirror().revision().get() + 1;
        let key =
            ObservationKey::try_new(self.epoch, ConfirmedRevision::new(next), 0).expect("key");
        let staged = AcceptedObservation::try_new(key, None, packet(event), Vec::new())
            .expect("staged observation");
        self.mirror.commit(&staged).expect("committed observation");
        // The controller-owned queue takes the provider's enriched entry;
        // the publication cursors drain this queue alone.
        let enriched = self
            .mirror
            .observations()
            .last()
            .expect("committed entry")
            .clone();
        self.observations.push(enriched);
        key
    }

    /// The immutable projection view at the coherent candidate identity.
    fn view<'a>(
        &'a self,
        player_state: &'a PlayerProjectionState,
        revision: ConfirmedRevision,
        index: u64,
    ) -> ProjectionView<'a> {
        ProjectionView::try_new(
            self.mirror.mirror(),
            &self.observations,
            self.input_owner.projection(),
            player_state,
            &self.audio,
            &self.lifecycle,
            &self.diagnostics,
            self.epoch,
            revision,
            index,
            &self.limits,
        )
        .expect("projection view")
    }

    /// The candidate revision the next frame would carry.
    fn revision(&self) -> ConfirmedRevision {
        self.mirror.mirror().revision()
    }
}

/// Drives one real whole-batch admission and republishes the admission
/// owner's exposed projection state into the publication owner.
fn admit(fixture: &mut FrameFixture, actions: Vec<InputAction>) {
    let epoch = fixture.epoch;
    let mirror = fixture.mirror.mirror().clone();
    let mut admission = InputAdmissionState::try_new(epoch, limits()).expect("admission owner");
    let batch = InputBatch::try_new(epoch, actions).expect("batch");
    let validated =
        InputTranslator::validate_batch(&batch, &mirror, &fixture.limits).expect("validated");
    let receipt = InputTranslator::commit(validated, &mut admission).expect("admitted batch");
    // The controller wiring records the receipt's sequenced metadata into
    // the publication owner's projection state; the admission commit itself
    // stages only the native local cue events.
    let mut projection = admission.projection().clone();
    if let mornlea_client_core::contracts::InputReceipt::Queued { first_sequence, .. } = receipt {
        let mut sequence = first_sequence.unwrap_or(0);
        for action in batch.actions() {
            if !matches!(action.intent, ClientIntent::Chat(_)) {
                projection.record_admitted(sequence, action.intent.kind());
                sequence += 1;
            }
        }
    }
    fixture.input_owner =
        InputAdmissionOwner::try_new(projection).expect("publication input owner");
}

fn collect_water() -> InputAction {
    InputAction {
        intent: ClientIntent::CollectWater(look(0.0, 0.0)),
        container: None,
        crafting: None,
    }
}

fn select_hotbar() -> InputAction {
    InputAction {
        intent: ClientIntent::SelectHotbar(mornlea_domain::HotbarSlot::new(0).expect("slot")),
        container: None,
        crafting: None,
    }
}

// --- the controller-role family builder ---

/// One built candidate beside the audio proposal its publication commits.
struct CandidateBuild {
    frame: PresentationFrame,
    audio: AudioDedupDelta,
}

/// Builds one complete candidate frame from the real providers at the
/// coherent candidate identity: the caller-supplied real session and
/// lifecycle records, the real terrain publisher's output and every projected
/// family the view currently carries. The audio proposal is returned beside
/// the frame because its dedup delta belongs to the publication transaction,
/// not to the frame.
fn build_candidate(
    fixture: &FrameFixture,
    player_state: &PlayerProjectionState,
    revision: ConfirmedRevision,
    index: u64,
    terrain: Vec<TerrainRecord>,
    lifecycle: Vec<LifecycleRecord>,
    session: Vec<SessionRecord>,
) -> Result<CandidateBuild, ClientError> {
    let view = fixture.view(player_state, revision, index);
    let upsert = FamilyOperation::Upsert;
    let mut families: Vec<FamilyFrame> = Vec::new();

    if !session.is_empty() {
        families.push(FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_SESSION)?,
            FamilyRecords::Session(session),
        )?);
    }

    // The input family: the pending input metadata in the frozen
    // admitted-first-then-rejected order, the records the publication
    // consumes with its input cursor.
    let mut input_records = Vec::new();
    for (sequence, kind) in view.input().admitted() {
        input_records.push(InputRecord::try_new(
            RecordHeader::try_new(view.frame_epoch(), revision, None, upsert)?,
            Some(*sequence),
            *kind,
            Receipt::Queued,
        )?);
    }
    for (sequence, class) in view.input().rejected() {
        input_records.push(InputRecord::try_new(
            RecordHeader::try_new(view.frame_epoch(), revision, None, upsert)?,
            Some(*sequence),
            // A rejection metadata entry carries no action kind; the neutral
            // sequenced tag names the journal entry it refuses.
            ClientIntentKind::PlayerInput,
            Receipt::Rejected { class: *class },
        )?);
    }
    if !input_records.is_empty() {
        families.push(FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_INPUT)?,
            FamilyRecords::Input(input_records),
        )?);
    }

    if !terrain.is_empty() {
        families.push(FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_TERRAIN)?,
            FamilyRecords::Terrain(terrain),
        )?);
    }

    let actor_parts = [
        project_remote_player(&view)?,
        project_hostile(&view)?,
        project_passive(&view)?,
        project_projectile(&view)?,
        project_companion(&view)?,
        project_drop(&view)?,
    ];
    let actor_records = assemble_actors(view.frame_epoch(), revision, actor_parts, view.limits())?;
    if !actor_records.is_empty() {
        families.push(FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_ACTORS)?,
            FamilyRecords::Actors(actor_records),
        )?);
    }

    let player_records = project_player_view(&view)?;
    if !player_records.is_empty() {
        families.push(FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_PLAYER_VIEW)?,
            FamilyRecords::PlayerView(player_records),
        )?);
    }

    let inventory_records = assemble_inventory_ui(
        view.frame_epoch(),
        revision,
        [
            project_inventory(&view)?,
            project_container(&view)?,
            project_crafting(&view)?,
            project_furnace(&view)?,
        ],
        view.limits(),
    )?;
    if !inventory_records.is_empty() {
        families.push(FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_INVENTORY_UI)?,
            FamilyRecords::InventoryUi(inventory_records),
        )?);
    }

    let world_records = assemble_world_ui(
        view.frame_epoch(),
        revision,
        [
            project_environment(&view)?,
            project_survival(&view)?,
            project_chat(&view)?,
            project_task(&view)?,
            project_prompt(&view)?,
        ],
        view.limits(),
    )?;
    if !world_records.is_empty() {
        families.push(FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_WORLD_UI)?,
            FamilyRecords::WorldUi(world_records),
        )?);
    }

    let audio = project_audio(&view)?;
    if !audio.records().is_empty() {
        families.push(FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_AUDIO_CUES)?,
            FamilyRecords::AudioCues(audio.records().to_vec()),
        )?);
    }

    if !lifecycle.is_empty() {
        families.push(FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_LIFECYCLE)?,
            FamilyRecords::Lifecycle(lifecycle),
        )?);
    }

    let diagnostic_records = project_diagnostics(&view)?;
    if !diagnostic_records.is_empty() {
        families.push(FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_DIAGNOSTICS)?,
            FamilyRecords::Diagnostics(diagnostic_records),
        )?);
    }

    let frame = PresentationFrame::try_new(view.frame_epoch(), revision, index, families)?;
    Ok(CandidateBuild {
        frame,
        audio: AudioDedupDelta::try_new(
            audio.proposed_dedup().insertions().to_vec(),
            audio.proposed_dedup().cancellations().to_vec(),
        )?,
    })
}

/// The session record the controller rebases onto the candidate identity
/// from the mirror's real session phase.
fn session_record(
    fixture: &FrameFixture,
    revision: ConfirmedRevision,
) -> Result<SessionRecord, ClientError> {
    Ok(SessionRecord::try_new(
        RecordHeader::try_new(fixture.epoch, revision, None, FamilyOperation::Upsert)?,
        *fixture.mirror.mirror().phase(),
        Some(player(3)),
        None,
    )?)
}

/// The bootstrap visible frame: the checked pending-connection publication
/// at index zero the controller holds before the first transaction.
fn bootstrap(value: u64) -> Arc<PresentationFrame> {
    let epoch = epoch(value);
    let header = RecordHeader::try_new(
        epoch,
        ConfirmedRevision::new(0),
        None,
        FamilyOperation::Upsert,
    )
    .expect("bootstrap header");
    let frame = PresentationFrame::try_new(
        epoch,
        ConfirmedRevision::new(0),
        0,
        vec![
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_SESSION).expect("session key"),
                FamilyRecords::Session(vec![
                    SessionRecord::try_new(header, SessionPhase::Connecting, None, None)
                        .expect("bootstrap session record"),
                ]),
            )
            .expect("bootstrap family"),
        ],
    )
    .expect("bootstrap frame");
    frame.validate(&limits()).expect("bootstrap validates");
    Arc::new(frame)
}

/// Scopes the owners bundle over the fixture's real state owners.
fn owners<'a>(
    fixture: &'a mut FrameFixture,
    visible: &'a mut Arc<PresentationFrame>,
) -> PublicationOwners<'a> {
    publication_owners(
        visible,
        &mut fixture.observations,
        &mut fixture.input_owner,
        &mut fixture.audio,
        &mut fixture.lifecycle,
    )
    .expect("owners bundle")
}

/// The dedup keys of one audio family in frame order.
fn audio_keys(frame: &PresentationFrame) -> Vec<AudioDedupKey> {
    let mut keys = Vec::new();
    for family in frame.families() {
        if let FamilyRecords::AudioCues(records) = family.records() {
            for record in records {
                keys.push(match record.provenance() {
                    CueProvenance::Confirmed {
                        observation,
                        authoritative_event_id,
                    } => AudioDedupKey::Confirmed {
                        epoch: record.header().epoch(),
                        observation: *observation,
                        authoritative_event_id: *authoritative_event_id,
                        cue: record.cue_id(),
                    },
                    CueProvenance::Predicted { input_sequence } => AudioDedupKey::Predicted {
                        epoch: record.header().epoch(),
                        input_sequence: *input_sequence,
                        cue: record.cue_id(),
                    },
                    CueProvenance::Local {
                        local_event_sequence,
                    } => AudioDedupKey::Local {
                        epoch: record.header().epoch(),
                        local_event_sequence: *local_event_sequence,
                        cue: record.cue_id(),
                    },
                });
            }
        }
    }
    keys
}

/// The record count of one logical family in a frame.
fn family_count(frame: &PresentationFrame, name: &'static str) -> usize {
    frame
        .families()
        .iter()
        .find(|family| family.key().logical_name == name)
        .map_or(0, |family| family.records().record_count())
}

// --- the real terrain publisher fixture ---

struct TerrainFixture {
    publisher: TerrainPublisher,
    queue: PreparationQueue,
    selection: LodSelection,
    params: Arc<WorldgenParams>,
    registry: Arc<MeshRegistry>,
    job_id: u64,
}

fn terrain_fixture(
    value: u64,
    limits: mornlea_client_core::contracts::ClientLimits,
) -> TerrainFixture {
    // Far selection disabled: the fixture publishes near sections alone.
    let config = LodConfig::try_new(false, 2, 3, LodStep::Four, 4096).expect("checked lod config");
    let entries = [MeshRegistryEntry {
        id: 1,
        opaque: true,
        emission: 0,
        material: [1; 6],
        fluid_height: 0,
        light_attenuation: 0,
        block_top_raw: 0,
        model: MeshModel::Default,
    }];
    TerrainFixture {
        publisher: TerrainPublisher::try_new().expect("terrain publisher"),
        queue: PreparationQueue::new(limits),
        selection: LodSelection::try_new(epoch(value), Dimension::OVERWORLD, config, 7)
            .expect("checked selection"),
        params: fixture_params(),
        registry: MeshRegistry::try_new(&entries, &[0], 0, 5)
            .expect("checked registry")
            .into(),
        job_id: 0,
    }
}

/// Admits one real near section job and publishes it through the real
/// terrain publisher over the real preparation owners.
fn publish_near_terrain(
    terrain: &mut TerrainFixture,
    view: &ProjectionView<'_>,
) -> Result<Vec<TerrainRecord>, ClientError> {
    terrain.job_id += 1;
    let key = PreparedResourceKey::try_new(
        view.frame_epoch(),
        Dimension::OVERWORLD,
        TerrainKey::Section(SectionKey::try_new(ChunkPos::new(0, 0), 0).expect("section")),
        1,
        CHUNK_REVISION,
        terrain.job_id,
    )
    .expect("checked near key");
    let payload = OwnedMeshView::try_new(
        Box::new([1u16; 110592]),
        [false; 9],
        Box::new([[0i16; 256]; 9]),
        mornlea_protocol::MIN_Y,
        Arc::clone(&terrain.registry),
    )
    .expect("checked mesh view");
    let job = PreparationJob::try_new(key, PreparationPayload::Near(payload)).expect("near job");
    terrain
        .publisher
        .admit_near(&mut terrain.queue, job)
        .expect("near admission");
    terrain.publisher.publish(
        view,
        &mut terrain.queue,
        &mut terrain.selection,
        ChunkPos::new(0, 0),
        &terrain.params,
        ClientWorkBudget::try_new(0, 4096).expect("budget"),
    )
}

// --- the real login lifecycle fixture ---

fn hello_frame() -> Vec<u8> {
    let hello =
        ClientHello::new(mornlea_domain::Identities::current().protocol).expect("current hello");
    write_frame(
        ClientHello::PACKET_ID,
        &hello.encode().expect("hello payload"),
    )
    .expect("hello frame")
}

fn login_reject_frame() -> Vec<u8> {
    frame_server_packet(&ServerPacket::LoginReject(
        LoginReject::new(LOGIN_SERVER_FULL, "server full".to_string()).expect("login reject"),
    ))
}

/// Drives the real login provider through connect and a login rejection on
/// the deterministic memory transport, returning the epoch, the real Open
/// family from the connect publication and the real terminal session and
/// lifecycle families from the terminal publication.
fn login_lifecycle() -> Result<
    (
        SessionEpoch,
        Vec<LifecycleRecord>,
        Vec<SessionRecord>,
        Vec<LifecycleRecord>,
    ),
    ClientError,
> {
    let connector = Arc::new(MemoryConnectorDouble::new());
    let clock = Arc::new(DeterministicClock::new());
    let config = ClientConfig::try_new(
        limits(),
        Duration::from_secs(5),
        Duration::from_secs(10),
        clock,
        registry_with_memory(connector.clone()),
    )?;
    let identity = ClientIdentity::try_new(LoginStart::new(player(3), "frame", 8).expect("login"))?;
    let mut session = LoginSession::new(config)?;
    let epoch = session.connect(
        mornlea_client_core::contracts::Endpoint::Memory {
            connector_id: NonZeroU64::new(1).expect("one"),
        },
        identity,
    )?;

    // The pending epoch publishes exactly one Open record at revision zero.
    let open_frame = session.snapshot(epoch)?;
    let mut open = Vec::new();
    for family in open_frame.families() {
        if let FamilyRecords::Lifecycle(records) = family.records() {
            open = records.clone();
        }
    }

    // A login rejection terminates once and publishes the ordered terminal
    // pair beside the terminal session record.
    connector.feed_frame(hello_frame());
    session.step(epoch, ClientWorkBudget::try_new(16, 0).expect("budget"))?;
    connector.feed_frame(login_reject_frame());
    session.step(epoch, ClientWorkBudget::try_new(16, 0).expect("budget"))?;
    let terminal_frame = session.snapshot(epoch)?;
    let mut terminal_session = Vec::new();
    let mut terminal_lifecycle = Vec::new();
    for family in terminal_frame.families() {
        match family.records() {
            FamilyRecords::Session(records) => terminal_session = records.clone(),
            FamilyRecords::Lifecycle(records) => terminal_lifecycle = records.clone(),
            _ => {}
        }
    }
    Ok((epoch, open, terminal_session, terminal_lifecycle))
}

/// Rebases one real lifecycle record onto the coherent candidate identity,
/// the frozen child-header rule for records the provider published at
/// another revision.
fn rebase_lifecycle(
    record: &LifecycleRecord,
    epoch: SessionEpoch,
    revision: ConfirmedRevision,
) -> Result<LifecycleRecord, ClientError> {
    LifecycleRecord::try_new(
        RecordHeader::try_new(epoch, revision, None, FamilyOperation::Upsert)?,
        record.transition(),
        record.generation(),
        record.resource_order().to_vec(),
    )
}

// --- the table ---

/// `frame::complete_frame_publishes_every_family_atomically`: one candidate
/// carries all ten families — real terrain, real lifecycle Open, real
/// admission metadata, real local cue, real confirmed and predicted cues,
/// real actors, real player view, real chest and chat records and the real
/// diagnostics record — and one transaction swaps the visible Arc exactly
/// once while every consumed queue drains by exactly its cursor.
#[test]
fn complete_frame_publishes_every_family_atomically() {
    let mut fixture = frame_fixture(EPOCH, true);
    let cells = fixture_cells(WALK_DIMS);
    let grid = walking_grid(&cells);
    let environment = StepEnvironment::new(grid, false);
    let mut replay = PredictionReplay::try_new(epoch(EPOCH), limits()).expect("replay owner");

    // Real observations through the real mirror provider: the prediction
    // authority, a chat acknowledgment, a chest view and a remote player.
    let authority_key =
        fixture.commit_observation(Event::PlayerState(authority(1, 0, [0.5, 0.0, 0.5], true)));
    replay
        .apply_confirmed(
            &authority_key,
            &authority(1, 0, [0.5, 0.0, 0.5], true),
            &environment,
        )
        .expect("the authority begins prediction");
    let chat_key = fixture.commit_observation(chat(99));
    fixture.commit_observation(chest());
    fixture.commit_observation(remote_spawn(player(4), 7));
    replay.begin_frame();
    replay
        .predict_step(1, forward(0.4), &environment)
        .expect("journaled prediction step");

    // Real admission: one local-cue action beside one plain sequenced action.
    admit(&mut fixture, vec![collect_water(), select_hotbar()]);

    // Real lifecycle Open record from the real login provider, rebased onto
    // the candidate revision per the frozen child-header rule and staged
    // into the owners' pending lifecycle state for the publication to
    // consume.
    let (_, open, _, _) = login_lifecycle().expect("real lifecycle records");
    assert_eq!(open.len(), 1, "the provider opens exactly once");
    assert_eq!(open[0].transition(), LifecycleTransition::Open);
    let open_rebased =
        rebase_lifecycle(&open[0], fixture.epoch, fixture.revision()).expect("rebased open record");
    fixture.lifecycle = LifecycleProjectionState::try_new(fixture.epoch.get())
        .expect("lifecycle state")
        .with_pending(vec![open_rebased.clone()]);

    // Real terrain through the real preparation owners.
    let player_state = replay.projection().expect("attributable player state");
    let revision = fixture.revision();
    let mut terrain = terrain_fixture(EPOCH, limits());
    let view = fixture.view(&player_state, revision, 1);
    let terrain_records = publish_near_terrain(&mut terrain, &view).expect("terrain publishes");
    assert_eq!(terrain_records.len(), 1, "one real near section record");

    let build = build_candidate(
        &fixture,
        &player_state,
        revision,
        1,
        terrain_records,
        vec![open_rebased],
        vec![session_record(&fixture, revision).expect("session record")],
    )
    .expect("candidate builds");
    let frame = &build.frame;
    assert_eq!(frame.families().len(), 10, "every family is present");
    for name in [
        FAMILY_SESSION,
        FAMILY_INPUT,
        FAMILY_TERRAIN,
        FAMILY_ACTORS,
        FAMILY_PLAYER_VIEW,
        FAMILY_INVENTORY_UI,
        FAMILY_WORLD_UI,
        FAMILY_AUDIO_CUES,
        FAMILY_LIFECYCLE,
        FAMILY_DIAGNOSTICS,
    ] {
        assert!(
            frame
                .families()
                .iter()
                .any(|family| family.key().logical_name == name
                    && family.records().record_count() > 0),
            "family {name} is present and nonempty"
        );
    }
    assert_eq!(family_count(frame, FAMILY_AUDIO_CUES), 3);
    assert_eq!(family_count(frame, FAMILY_INPUT), 2);

    // The assembled prepared Arc: complete, next-indexed, current untouched.
    let current = bootstrap(EPOCH);
    let prepared =
        assemble_frame(&current, build.frame.clone(), &limits()).expect("the candidate prepares");
    assert_eq!(prepared.frame_index(), 1, "the next index");
    assert_eq!(current.frame_index(), 0, "the current frame never changed");

    // The reservation checks everything and stages the input-projection
    // replacement; the owners stay untouched, because prepare is a checked,
    // mutation-free step over immutable borrows.
    let mut visible = current;
    let old = Arc::clone(&visible);
    let consume = PublicationConsumption::try_new(4, 2, 1, 1).expect("cursors");
    let mut fixture_ref = fixture;
    let reservation = {
        let bundle = owners(&mut fixture_ref, &mut visible);
        let reservation = prepare_publication(
            &bundle,
            build.frame.clone(),
            build.audio.clone(),
            consume,
            &limits(),
        )
        .expect("the reservation checks");
        // The staged replacement carries the survivors, the drained prefixes
        // and the advanced hint, ready for the commit's single move.
        assert_eq!(reservation.staged_input().sequence_hint(), 3);
        assert!(reservation.staged_input().admitted().is_empty());
        assert!(reservation.staged_input().rejected().is_empty());
        assert!(reservation.staged_input().pending_local_cues().is_empty());
        reservation
    };
    assert_eq!(
        fixture_ref.input_owner.projection().admitted().len(),
        2,
        "prepare stages the replacement; it never mutates the owner"
    );
    assert_eq!(
        fixture_ref
            .input_owner
            .projection()
            .pending_local_cues()
            .len(),
        1,
        "prepare stages the replacement; it never mutates the owner"
    );
    assert_eq!(
        fixture_ref.observations.len(),
        4,
        "prepare consumes nothing"
    );
    assert_eq!(visible.frame_index(), 0, "prepare never touches the Arc");
    // The commit is the single staged swap plus the one visible Arc swap.
    let committed = {
        let mut bundle = owners(&mut fixture_ref, &mut visible);
        commit_publication(&mut bundle, reservation)
    };
    assert!(
        Arc::ptr_eq(&committed, &visible),
        "the commit returns the Arc it swapped in"
    );
    assert_eq!(visible.frame_index(), 1, "the index advanced exactly once");
    assert_eq!(
        visible.confirmed_revision(),
        revision,
        "the coherent candidate revision"
    );

    // Every consumed queue drained by exactly its cursor.
    assert!(
        fixture_ref.observations.is_empty(),
        "four observations drained"
    );
    let projection = fixture_ref.input_owner.projection();
    assert!(
        projection.admitted().is_empty() && projection.rejected().is_empty(),
        "the published input metadata is consumed"
    );
    assert!(
        projection.pending_local_cues().is_empty(),
        "the published local cue is consumed"
    );
    assert_eq!(
        projection.sequence_hint(),
        3,
        "the hint advances past the attested sequences"
    );
    assert!(
        fixture_ref.lifecycle.pending().is_empty(),
        "the published lifecycle record is consumed"
    );

    // The audio state committed exactly the proposed insertions.
    let chat_cue = AudioDedupKey::Confirmed {
        epoch: epoch(EPOCH),
        observation: chat_key,
        authoritative_event_id: Some(99),
        cue: CueId::try_new(0).expect("ui click cue"),
    };
    let step_cue = AudioDedupKey::Predicted {
        epoch: epoch(EPOCH),
        input_sequence: 1,
        cue: CueId::try_new(6).expect("snow step cue"),
    };
    let local_cue = AudioDedupKey::Local {
        epoch: epoch(EPOCH),
        local_event_sequence: 1,
        cue: CueId::try_new(4).expect("water splash cue"),
    };
    assert_eq!(
        fixture_ref.audio.committed(),
        &[chat_cue, step_cue, local_cue],
        "the confirmed, predicted and local keys committed in source order"
    );
    assert!(
        fixture_ref.audio.pending_cancellations().is_empty(),
        "no cancellation existed to stage"
    );

    // The old Arc stays readable after the swap.
    assert_eq!(old.frame_index(), 0, "the old frame is unchanged");
    assert_eq!(old.families().len(), 1, "the old frame content is intact");
    assert_eq!(audio_keys(&visible), vec![chat_cue, step_cue, local_cue]);
}

/// `frame::multi_record_and_reuse_flow_through`: multi-record packets and
/// several lifecycle records sharing one stable key flow through assembly —
/// a spawn, its despawn and its reuse spawn are distinct records of one
/// identity in retained source order, and the publication consumes every
/// carrying observation.
#[test]
fn multi_record_and_reuse_flow_through() {
    let mut fixture = frame_fixture(EPOCH, false);
    let peer = player(4);
    fixture.commit_observation(remote_spawn(peer, 1));
    fixture.commit_observation(hostile_batch(2, &[1, 2]));
    fixture.commit_observation(remote_despawn(peer));
    fixture.commit_observation(remote_spawn(peer, 4));

    let player_state = idle_player(EPOCH, fixture.revision());
    let revision = fixture.revision();
    let build = build_candidate(
        &fixture,
        &player_state,
        revision,
        1,
        Vec::new(),
        Vec::new(),
        vec![session_record(&fixture, revision).expect("session record")],
    )
    .expect("candidate builds");
    assert_eq!(family_count(&build.frame, FAMILY_ACTORS), 5);

    let mut visible = bootstrap(EPOCH);
    let mut fixture_ref = fixture;
    let committed = {
        let mut bundle = owners(&mut fixture_ref, &mut visible);
        let reservation = prepare_publication(
            &bundle,
            build.frame,
            build.audio,
            PublicationConsumption::try_new(4, 0, 0, 0).expect("cursors"),
            &limits(),
        )
        .expect("the reuse candidate reserves");
        commit_publication(&mut bundle, reservation)
    };
    let mut actor_records: Vec<&ActorRecord> = Vec::new();
    for family in committed.families() {
        if let FamilyRecords::Actors(records) = family.records() {
            actor_records = records.iter().collect();
        }
    }
    assert_eq!(actor_records.len(), 5, "spawn, two spawns, despawn, reuse");
    assert_eq!(
        actor_records[0].id(),
        &mornlea_client_core::presentation::frame::ActorId::RemotePlayer(peer),
        "the first spawn comes first in retained order"
    );
    assert!(
        matches!(
            actor_records[3].header().operation(),
            FamilyOperation::Remove
        ),
        "the despawn keeps its remove tag in order"
    );
    assert!(
        matches!(
            actor_records[4].header().operation(),
            FamilyOperation::Upsert
        ),
        "the reuse spawn follows its removal"
    );
    assert!(
        fixture_ref.observations.is_empty(),
        "every carrying observation is consumed once"
    );
}

/// `frame::interleaved_equal_and_tickless_source_order`: equal and absent
/// source ticks never reconstruct order — the assembled families keep the
/// retained observation order (revision, then packet ordinal) while the
/// audio provenance keeps actual observation order across equal and tickless
/// sources.
#[test]
fn interleaved_equal_and_tickless_source_order() {
    let mut fixture = frame_fixture(EPOCH, false);
    // Spawn and hostile batch share one server tick; the chest and chat
    // observations carry none.
    fixture.commit_observation(remote_spawn(player(4), 5));
    fixture.commit_observation(hostile_batch(5, &[1, 2]));
    fixture.commit_observation(chest());
    fixture.commit_observation(combat(5));
    fixture.commit_observation(chat(99));

    let player_state = idle_player(EPOCH, fixture.revision());
    let revision = fixture.revision();
    let build = build_candidate(
        &fixture,
        &player_state,
        revision,
        1,
        Vec::new(),
        Vec::new(),
        vec![session_record(&fixture, revision).expect("session record")],
    )
    .expect("candidate builds");

    // The actor family: the spawn precedes both hostile records even though
    // the ticks are equal, and the batch keeps its packet ordinal order.
    let mut visible = bootstrap(EPOCH);
    let mut fixture_ref = fixture;
    let committed = {
        let mut bundle = owners(&mut fixture_ref, &mut visible);
        let reservation = prepare_publication(
            &bundle,
            build.frame,
            build.audio,
            PublicationConsumption::try_new(5, 0, 0, 0).expect("cursors"),
            &limits(),
        )
        .expect("the equal-tick candidate reserves");
        commit_publication(&mut bundle, reservation)
    };
    let mut actor_records: Vec<&ActorRecord> = Vec::new();
    let mut audio_records: Vec<&AudioCueRecord> = Vec::new();
    for family in committed.families() {
        match family.records() {
            FamilyRecords::Actors(records) => actor_records = records.iter().collect(),
            FamilyRecords::AudioCues(records) => audio_records = records.iter().collect(),
            _ => {}
        }
    }
    assert_eq!(actor_records.len(), 3, "one spawn beside two batch records");
    assert_eq!(
        actor_records[0].id(),
        &mornlea_client_core::presentation::frame::ActorId::RemotePlayer(player(4)),
        "retained observation order, never tick order"
    );
    assert_eq!(
        actor_records[1].id(),
        &mornlea_client_core::presentation::frame::ActorId::Hostile(
            HostileId::try_new(1).expect("hostile id")
        ),
        "the batch's first record keeps its packet ordinal"
    );
    assert_eq!(
        actor_records[2].id(),
        &mornlea_client_core::presentation::frame::ActorId::Hostile(
            HostileId::try_new(2).expect("hostile id")
        ),
        "the batch's second record follows it"
    );
    // The audio family: the confirmed cues keep actual observation order
    // across the equal tick and the tickless sources.
    assert_eq!(audio_records.len(), 2);
    assert_eq!(audio_records[0].header().source_tick(), Some(5));
    assert_eq!(audio_records[1].header().source_tick(), None);
    assert_eq!(
        audio_records[1].cue_id(),
        CueId::try_new(0).expect("ui click cue"),
        "the chat acknowledgment follows the combat hit in observation order"
    );
}

/// `frame::candidate_defects_reject_typed`: one wrong epoch, one mixed
/// revision, one duplicate family, one invalid number, one wrong next index
/// and one missing required source key each reject typed, and every failed
/// preparation leaves the owners exactly as they were.
#[test]
fn candidate_defects_reject_typed() {
    let mut fixture = frame_fixture(EPOCH, false);
    fixture.commit_observation(chat(99));
    admit(&mut fixture, vec![collect_water()]);

    let player_state = idle_player(EPOCH, fixture.revision());
    let revision = fixture.revision();
    let build = build_candidate(
        &fixture,
        &player_state,
        revision,
        1,
        Vec::new(),
        Vec::new(),
        vec![session_record(&fixture, revision).expect("session record")],
    )
    .expect("candidate builds");

    let mut visible = bootstrap(EPOCH);
    let old = Arc::clone(&visible);

    // Wrong epoch: a record header of another epoch inside the frame.
    let wrong_epoch_header =
        RecordHeader::try_new(epoch(EPOCH + 1), revision, None, FamilyOperation::Upsert)
            .expect("header");
    let wrong_epoch = PresentationFrame::try_new(
        fixture.epoch,
        revision,
        1,
        vec![
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_SESSION).expect("key"),
                FamilyRecords::Session(vec![
                    SessionRecord::try_new(wrong_epoch_header, SessionPhase::Admitted, None, None)
                        .expect("session record"),
                ]),
            )
            .expect("family"),
        ],
    )
    .expect("candidate frame");
    {
        let bundle = owners(&mut fixture, &mut visible);
        assert_eq!(
            prepare_publication(
                &bundle,
                wrong_epoch,
                AudioDedupDelta::try_new(Vec::new(), Vec::new()).expect("empty delta"),
                PublicationConsumption::default(),
                &limits(),
            )
            .err(),
            Some(ClientError::InvalidInput),
            "a wrong-epoch record header rejects the whole candidate"
        );
    }

    // Mixed revision: a record header ahead of the frame parent.
    let mixed_header = RecordHeader::try_new(
        fixture.epoch,
        ConfirmedRevision::new(revision.get() + 1),
        None,
        FamilyOperation::Upsert,
    )
    .expect("header");
    let mixed = PresentationFrame::try_new(
        fixture.epoch,
        revision,
        1,
        vec![
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_SESSION).expect("key"),
                FamilyRecords::Session(vec![
                    SessionRecord::try_new(mixed_header, SessionPhase::Admitted, None, None)
                        .expect("session record"),
                ]),
            )
            .expect("family"),
        ],
    )
    .expect("candidate frame");
    {
        let bundle = owners(&mut fixture, &mut visible);
        assert_eq!(
            prepare_publication(
                &bundle,
                mixed,
                AudioDedupDelta::try_new(Vec::new(), Vec::new()).expect("empty delta"),
                PublicationConsumption::default(),
                &limits(),
            )
            .err(),
            Some(ClientError::InvalidInput),
            "a mixed-revision record header rejects the whole candidate"
        );
    }

    // Duplicate family: the same logical family twice in one frame.
    let duplicate = PresentationFrame::try_new(
        fixture.epoch,
        revision,
        1,
        vec![
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_SESSION).expect("key"),
                FamilyRecords::Session(vec![
                    session_record(&fixture, revision).expect("session record"),
                ]),
            )
            .expect("family"),
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_SESSION).expect("key"),
                FamilyRecords::Session(vec![
                    session_record(&fixture, revision).expect("session record"),
                ]),
            )
            .expect("duplicate family"),
        ],
    )
    .expect("candidate frame");
    {
        let bundle = owners(&mut fixture, &mut visible);
        assert_eq!(
            prepare_publication(
                &bundle,
                duplicate,
                AudioDedupDelta::try_new(Vec::new(), Vec::new()).expect("empty delta"),
                PublicationConsumption::default(),
                &limits(),
            )
            .err(),
            Some(ClientError::InvalidInput),
            "a duplicated family rejects the whole candidate"
        );
    }

    // Wrong next index: the candidate must be exactly current + 1.
    let wrong_index =
        PresentationFrame::try_new(fixture.epoch, revision, 2, build.frame.families().to_vec())
            .expect("candidate frame");
    assert_eq!(
        assemble_frame(&visible, wrong_index.clone(), &limits()),
        Err(ClientError::InvalidInput),
        "assemble refuses a candidate that skips the next index"
    );
    {
        let bundle = owners(&mut fixture, &mut visible);
        assert_eq!(
            prepare_publication(
                &bundle,
                wrong_index,
                AudioDedupDelta::try_new(Vec::new(), Vec::new()).expect("empty delta"),
                PublicationConsumption::default(),
                &limits(),
            )
            .err(),
            Some(ClientError::InvalidInput),
            "prepare refuses a candidate that skips the next index"
        );
    }

    // Invalid number: the family record cap plus one rejects typed.
    let over_count = PresentationFrame::try_new(
        fixture.epoch,
        revision,
        1,
        vec![
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_SESSION).expect("key"),
                FamilyRecords::Session(vec![
                    session_record(&fixture, revision).expect("session record"),
                    session_record(&fixture, revision).expect("session record"),
                ]),
            )
            .expect("family"),
        ],
    )
    .expect("candidate frame");
    {
        let bundle = owners(&mut fixture, &mut visible);
        assert_eq!(
            prepare_publication(
                &bundle,
                over_count,
                AudioDedupDelta::try_new(Vec::new(), Vec::new()).expect("empty delta"),
                PublicationConsumption::default(),
                &limits_with(
                    1,
                    mornlea_client_core::contracts::ClientLimits::MAX_FRAME_BYTES
                ),
            )
            .err(),
            Some(ClientError::Capacity),
            "the family record cap plus one rejects"
        );
    }

    // Missing required source keys: the audio record attests an observation
    // the cursors do not dequeue, and the input record attests metadata the
    // cursors do not consume; an over-long cursor is not a prefix at all.
    {
        let audio_only = PresentationFrame::try_new(
            fixture.epoch,
            revision,
            1,
            vec![
                FamilyFrame::try_new(
                    FamilyKey::try_new(FAMILY_AUDIO_CUES).expect("key"),
                    FamilyRecords::AudioCues(
                        project_audio(&fixture.view(&player_state, revision, 1))
                            .expect("audio projection")
                            .records()
                            .to_vec(),
                    ),
                )
                .expect("family"),
            ],
        )
        .expect("candidate frame");
        let bundle = owners(&mut fixture, &mut visible);
        assert_eq!(
            prepare_publication(
                &bundle,
                audio_only,
                AudioDedupDelta::try_new(Vec::new(), Vec::new()).expect("empty delta"),
                PublicationConsumption::default(),
                &limits(),
            )
            .err(),
            Some(ClientError::InvalidInput),
            "a confirmed cue without its dequeued observation rejects"
        );
    }
    {
        let input_only = PresentationFrame::try_new(
            fixture.epoch,
            revision,
            1,
            vec![
                FamilyFrame::try_new(
                    FamilyKey::try_new(FAMILY_INPUT).expect("key"),
                    FamilyRecords::Input(vec![
                        InputRecord::try_new(
                            RecordHeader::try_new(
                                fixture.epoch,
                                revision,
                                None,
                                FamilyOperation::Upsert,
                            )
                            .expect("header"),
                            Some(1),
                            ClientIntentKind::CollectWater,
                            Receipt::Queued,
                        )
                        .expect("input record"),
                    ]),
                )
                .expect("family"),
            ],
        )
        .expect("candidate frame");
        let bundle = owners(&mut fixture, &mut visible);
        assert_eq!(
            prepare_publication(
                &bundle,
                input_only,
                AudioDedupDelta::try_new(Vec::new(), Vec::new()).expect("empty delta"),
                PublicationConsumption::default(),
                &limits(),
            )
            .err(),
            Some(ClientError::InvalidInput),
            "a queued input record without its dequeued metadata rejects"
        );
        let over_long = PublicationConsumption::try_new(9, 0, 0, 0).expect("cursors");
        assert_eq!(
            prepare_publication(
                &bundle,
                build.frame.clone(),
                build.audio.clone(),
                over_long,
                &limits(),
            )
            .err(),
            Some(ClientError::InvalidInput),
            "a cursor past the queue end is not a legal prefix"
        );
    }

    // Every failed preparation left the owners exactly as they were.
    assert_eq!(fixture.observations.len(), 1, "no observation consumed");
    let projection = fixture.input_owner.projection();
    assert_eq!(projection.admitted().len(), 1, "no metadata consumed");
    assert_eq!(projection.pending_local_cues().len(), 1, "no cue consumed");
    assert!(fixture.audio.committed().is_empty(), "no key committed");
    assert!(
        fixture.audio.pending_cancellations().is_empty(),
        "no cancellation staged"
    );
    assert_eq!(visible.frame_index(), 0, "the index never moved");
    assert_eq!(
        Arc::ptr_eq(&old, &visible) && old.families().len() == 1,
        true,
        "the old Arc is still the visible one and fully readable"
    );

    // The unchanged candidate still reserves and commits once.
    let committed = {
        let mut bundle = owners(&mut fixture, &mut visible);
        let reservation = prepare_publication(
            &bundle,
            build.frame,
            build.audio,
            PublicationConsumption::try_new(1, 1, 1, 0).expect("cursors"),
            &limits(),
        )
        .expect("the valid retry reserves");
        commit_publication(&mut bundle, reservation)
    };
    assert_eq!(committed.frame_index(), 1, "the retry commits exactly once");
    assert!(fixture.observations.is_empty());
    assert!(fixture.input_owner.projection().admitted().is_empty());
    assert!(
        fixture
            .input_owner
            .projection()
            .pending_local_cues()
            .is_empty()
    );
}

/// `frame::caps_plus_one_preserve_the_visible_frame`: the measured
/// 4,325,408-byte world aggregate publishes under the frozen cap; one byte
/// less configured capacity and the family-record cap plus one each reject
/// with the visible frame, the index and every queue preserved.
#[test]
fn caps_plus_one_preserve_the_visible_frame() {
    // The exact measured aggregate: one world-ui family of 4096 task records.
    let aggregate_epoch = epoch(21);
    let aggregate =
        aggregate_world_frame(aggregate_epoch, 4096, Some(4_325_408)).expect("aggregate frame");
    assert_eq!(
        aggregate.validated_size().expect("checked size"),
        4_325_408,
        "the exact measured aggregate"
    );
    let mut fixture = frame_fixture(21, false);
    let mut visible = bootstrap(21);
    let committed = {
        let mut bundle = owners(&mut fixture, &mut visible);
        let reservation = prepare_publication(
            &bundle,
            aggregate.clone(),
            AudioDedupDelta::try_new(Vec::new(), Vec::new()).expect("empty delta"),
            PublicationConsumption::default(),
            &limits(),
        )
        .expect("the aggregate fits the frozen cap");
        commit_publication(&mut bundle, reservation)
    };
    assert_eq!(committed.frame_index(), 1, "the aggregate publishes");

    // One byte less configured capacity rejects whole, preserving everything.
    let starved_limits = limits_with(
        mornlea_client_core::contracts::ClientLimits::MAX_FAMILY_RECORDS,
        4_325_407,
    );
    let mut fixture = frame_fixture(21, false);
    let mut visible = bootstrap(21);
    {
        let bundle = owners(&mut fixture, &mut visible);
        assert_eq!(
            prepare_publication(
                &bundle,
                aggregate,
                AudioDedupDelta::try_new(Vec::new(), Vec::new()).expect("empty delta"),
                PublicationConsumption::default(),
                &starved_limits,
            )
            .err(),
            Some(ClientError::Capacity),
            "the aggregate cannot fit one byte less"
        );
    }
    assert_eq!(visible.frame_index(), 0, "the index is preserved");
    assert_eq!(visible.families().len(), 1, "the old frame is preserved");

    // The family-record cap plus one rejects before publication.
    let over = aggregate_world_frame(epoch(22), 4097, None).expect("over-count frame");
    let mut fixture = frame_fixture(22, false);
    let mut visible = bootstrap(22);
    {
        let bundle = owners(&mut fixture, &mut visible);
        assert_eq!(
            prepare_publication(
                &bundle,
                over,
                AudioDedupDelta::try_new(Vec::new(), Vec::new()).expect("empty delta"),
                PublicationConsumption::default(),
                &limits(),
            )
            .err(),
            Some(ClientError::Capacity),
            "the frozen family-record cap plus one rejects"
        );
    }
    assert_eq!(visible.frame_index(), 0, "the index is preserved");
}

/// `frame::failed_publication_consumes_nothing_and_retry_commits_once`: the
/// failed publication consumes no event, no input metadata, no local cue and
/// no cancellation — the dedup proposal and every pending queue stay
/// retry-owned — and the retry commits once, applying the cancellation to the
/// committed keys and staging it as the pending predicted cancellation the
/// next projection proposes even after the derived source is consumed.
#[test]
fn failed_publication_consumes_nothing_and_retry_commits_once() {
    let cells = fixture_cells(WALK_DIMS);
    let grid = walking_grid(&cells);
    let environment = StepEnvironment::new(grid, false);
    let mut fixture = frame_fixture(EPOCH, false);
    let mut replay = PredictionReplay::try_new(epoch(EPOCH), limits()).expect("replay owner");

    // The prediction begins and one step is journaled; its publication
    // commits the predicted cue key.
    let authority_key =
        fixture.commit_observation(Event::PlayerState(authority(1, 0, [0.5, 0.0, 0.5], true)));
    replay
        .apply_confirmed(
            &authority_key,
            &authority(1, 0, [0.5, 0.0, 0.5], true),
            &environment,
        )
        .expect("the authority begins prediction");
    replay.begin_frame();
    replay
        .predict_step(1, forward(0.4), &environment)
        .expect("journaled prediction step");
    let player_state = replay.projection().expect("attributable state");
    let revision = fixture.revision();
    let first = build_candidate(
        &fixture,
        &player_state,
        revision,
        1,
        Vec::new(),
        Vec::new(),
        vec![session_record(&fixture, revision).expect("session record")],
    )
    .expect("candidate builds");
    let step_key = AudioDedupKey::Predicted {
        epoch: epoch(EPOCH),
        input_sequence: 1,
        cue: CueId::try_new(6).expect("snow step cue"),
    };
    assert_eq!(first.audio.insertions(), &[step_key]);

    let mut visible = bootstrap(EPOCH);
    {
        let mut bundle = owners(&mut fixture, &mut visible);
        let reservation = prepare_publication(
            &bundle,
            first.frame,
            first.audio,
            PublicationConsumption::try_new(1, 0, 0, 0).expect("cursors"),
            &limits(),
        )
        .expect("the first publication reserves");
        commit_publication(&mut bundle, reservation);
    }
    assert_eq!(fixture.audio.committed(), &[step_key]);
    assert_eq!(visible.frame_index(), 1);

    // The authority refuses the input: the replay owner removes the pending
    // prediction and the wiring records the refused sequence.
    let rejected = replay.reject(1, &environment).expect("the rejection lands");
    assert!(rejected.is_some(), "the refused step is attributed");
    let projection = fixture.input_owner.projection();
    let mut with_rejection =
        InputProjectionState::try_new(projection.sequence_hint()).expect("input projection state");
    for (sequence, kind) in projection.admitted() {
        with_rejection.record_admitted(*sequence, *kind);
    }
    for (sequence, class) in projection.rejected() {
        with_rejection.record_rejected(*sequence, *class);
    }
    with_rejection.record_rejected(1, ClientError::InvalidInput);
    fixture.input_owner = InputAdmissionOwner::try_new(with_rejection).expect("input owner");

    // An unconsumed event stays queued behind the failure row.
    let chat_observation_key = fixture.commit_observation(chat(99));
    let chat_key = AudioDedupKey::Confirmed {
        epoch: epoch(EPOCH),
        observation: chat_observation_key,
        authoritative_event_id: Some(99),
        cue: CueId::try_new(0).expect("ui click cue"),
    };

    // The retry-owned candidate: the rejection record beside the derived
    // cancellation of the committed step key.
    let player_state = replay.projection().expect("attributable state");
    let revision = fixture.revision();
    let second = build_candidate(
        &fixture,
        &player_state,
        revision,
        2,
        Vec::new(),
        Vec::new(),
        vec![session_record(&fixture, revision).expect("session record")],
    )
    .expect("candidate builds");
    assert_eq!(
        second.audio.insertions(),
        &[chat_key],
        "the retained event's cue is still proposed"
    );
    assert_eq!(
        second.audio.cancellations(),
        &[step_key],
        "the rejection derives exactly the refused key"
    );
    assert_eq!(family_count(&second.frame, FAMILY_INPUT), 1);

    // The publication fails on a configured frame cap one byte short.
    let tight = limits_with(
        mornlea_client_core::contracts::ClientLimits::MAX_FAMILY_RECORDS,
        second.frame.validated_size().expect("checked size") - 1,
    );
    {
        let bundle = owners(&mut fixture, &mut visible);
        assert_eq!(
            prepare_publication(
                &bundle,
                second.frame.clone(),
                second.audio.clone(),
                PublicationConsumption::try_new(1, 1, 0, 0).expect("cursors"),
                &tight,
            )
            .err(),
            Some(ClientError::Capacity),
            "the configured cap plus one rejects the publication"
        );
    }

    // Nothing was consumed: the event, the metadata, the committed key, the
    // visible frame and the index all stay exactly as they were.
    assert_eq!(fixture.observations.len(), 1, "no event consumed");
    assert_eq!(
        fixture.input_owner.projection().rejected().len(),
        1,
        "the refusal metadata stays pending"
    );
    assert_eq!(
        fixture.audio.committed(),
        &[step_key],
        "the committed key stays uncancelled"
    );
    assert!(
        fixture.audio.pending_cancellations().is_empty(),
        "no cancellation staged by a failed publication"
    );
    assert_eq!(visible.frame_index(), 1, "the index never moved");

    // The valid retry reserves and commits once.
    {
        let mut bundle = owners(&mut fixture, &mut visible);
        let reservation = prepare_publication(
            &bundle,
            second.frame,
            second.audio,
            PublicationConsumption::try_new(1, 1, 0, 0).expect("cursors"),
            &limits(),
        )
        .expect("the retry reserves");
        commit_publication(&mut bundle, reservation);
    }
    assert_eq!(visible.frame_index(), 2, "the retry commits once");
    assert!(
        fixture.input_owner.projection().rejected().is_empty(),
        "the published rejection metadata is consumed"
    );
    assert_eq!(
        fixture.input_owner.projection().sequence_hint(),
        2,
        "the staged swap advanced the hint past the attested sequence"
    );
    assert_eq!(
        fixture.observations.len(),
        0,
        "the published event is consumed exactly once"
    );
    assert_eq!(
        fixture.audio.committed(),
        &[chat_key],
        "the refused key is cancelled and the retained event's key commits once"
    );
    assert_eq!(
        fixture.audio.pending_cancellations(),
        &[step_key],
        "the applied cancellation stages as the pending predicted cancellation"
    );

    // The staged half is drivable end to end: after a fresh authority
    // replaces the rejection correction, the derived half is silent and the
    // staged cancellation alone is still proposed.
    let next_key =
        fixture.commit_observation(Event::PlayerState(authority(2, 1, [0.6, 0.0, 0.6], true)));
    replay
        .apply_confirmed(
            &next_key,
            &authority(2, 1, [0.6, 0.0, 0.6], true),
            &environment,
        )
        .expect("the fresh authority acknowledges");
    let player_state = replay.projection().expect("attributable state");
    let view = fixture.view(&player_state, fixture.revision(), 3);
    let projection = project_audio(&view).expect("valid projection");
    assert!(
        projection.records().is_empty(),
        "the refused step never sounds again"
    );
    assert!(
        projection.proposed_dedup().insertions().is_empty(),
        "nothing new is proposed"
    );
    assert_eq!(
        projection.proposed_dedup().cancellations(),
        &[step_key],
        "the staged cancellation survives the consumed derived source"
    );
}

/// `frame::assemble_prepares_immutable_arc_and_empty_families_publish`:
/// `assemble_frame` returns a prepared immutable Arc without touching the
/// current frame, the prepared Arc stays valid across a later failed
/// preparation, an empty family entry is legal, and a repeated local
/// publication at the same confirmed revision advances only the index.
#[test]
fn assemble_prepares_immutable_arc_and_empty_families_publish() {
    let mut fixture = frame_fixture(EPOCH, false);
    admit(&mut fixture, vec![collect_water()]);

    let player_state = idle_player(EPOCH, fixture.revision());
    let revision = fixture.revision();
    let build = build_candidate(
        &fixture,
        &player_state,
        revision,
        1,
        Vec::new(),
        Vec::new(),
        vec![session_record(&fixture, revision).expect("session record")],
    )
    .expect("candidate builds");

    let current = bootstrap(EPOCH);
    let prepared =
        assemble_frame(&current, build.frame.clone(), &limits()).expect("the candidate prepares");
    assert_eq!(prepared.frame_index(), 1);
    assert_eq!(current.frame_index(), 0, "the current frame is untouched");
    assert!(
        prepared.validate(&limits()).is_ok(),
        "the prepared Arc is a complete valid frame"
    );

    // A later failed preparation cannot invalidate the prepared Arc.
    let tight = limits_with(
        mornlea_client_core::contracts::ClientLimits::MAX_FAMILY_RECORDS,
        build.frame.validated_size().expect("checked size") - 1,
    );
    {
        let mut visible = Arc::clone(&current);
        let bundle = owners(&mut fixture, &mut visible);
        assert_eq!(
            prepare_publication(
                &bundle,
                build.frame.clone(),
                build.audio.clone(),
                PublicationConsumption::default(),
                &tight,
            )
            .err(),
            Some(ClientError::Capacity),
            "the later preparation fails typed"
        );
    }
    assert!(
        prepared.validate(&limits()).is_ok(),
        "the prepared Arc stays safe after the later failure"
    );

    // The first publication commits the local cue and its metadata.
    let mut visible = Arc::clone(&current);
    {
        let mut bundle = owners(&mut fixture, &mut visible);
        let reservation = prepare_publication(
            &bundle,
            build.frame,
            build.audio,
            PublicationConsumption::try_new(0, 1, 1, 0).expect("cursors"),
            &limits(),
        )
        .expect("the first local publication reserves");
        commit_publication(&mut bundle, reservation);
    }
    let first_frame = Arc::clone(&visible);
    assert_eq!(first_frame.frame_index(), 1);
    assert_eq!(first_frame.confirmed_revision(), revision);

    // The repeated local publication carries the same confirmed revision, an
    // explicitly empty input family and a fresh diagnostics record; only the
    // frame index advances.
    let empty_family_frame = PresentationFrame::try_new(
        fixture.epoch,
        revision,
        2,
        vec![
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_SESSION).expect("key"),
                FamilyRecords::Session(vec![
                    session_record(&fixture, revision).expect("session record"),
                ]),
            )
            .expect("family"),
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_INPUT).expect("key"),
                FamilyRecords::Input(Vec::new()),
            )
            .expect("empty family"),
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_DIAGNOSTICS).expect("key"),
                FamilyRecords::Diagnostics(
                    project_diagnostics(&fixture.view(&player_state, revision, 2))
                        .expect("diagnostics projection"),
                ),
            )
            .expect("family"),
        ],
    )
    .expect("candidate frame");
    let second_prepared =
        assemble_frame(&visible, empty_family_frame, &limits()).expect("the empty family is legal");
    {
        let mut bundle = owners(&mut fixture, &mut visible);
        let reservation = prepare_publication(
            &bundle,
            second_prepared.as_ref().clone(),
            AudioDedupDelta::try_new(Vec::new(), Vec::new()).expect("empty delta"),
            PublicationConsumption::default(),
            &limits(),
        )
        .expect("the repeated local publication reserves");
        commit_publication(&mut bundle, reservation);
    }
    assert_eq!(visible.frame_index(), 2, "only the index advanced");
    assert_eq!(
        visible.confirmed_revision(),
        revision,
        "the confirmed revision is unchanged"
    );
    assert_eq!(
        first_frame.frame_index(),
        1,
        "the previous frame stays readable"
    );
    assert_eq!(
        first_frame.confirmed_revision(),
        visible.confirmed_revision(),
        "both local publications carry the same confirmed revision"
    );
}

/// `frame::real_lifecycle_terminal_pair_publishes`: the real login
/// provider's ordered terminal Close/Invalidate pair beside its terminal
/// session record publishes as one atomic transaction at revision zero, with
/// the actual core-owned resource order and the checked core generation.
#[test]
fn real_lifecycle_terminal_pair_publishes() {
    let (lifecycle_epoch, open, terminal_session, terminal_lifecycle) =
        login_lifecycle().expect("the real login provider");
    assert_eq!(open.len(), 1, "one Open record");
    assert_eq!(open[0].transition(), LifecycleTransition::Open);
    assert_eq!(
        open[0].generation(),
        lifecycle_epoch.get(),
        "the checked core generation"
    );
    assert_eq!(
        open[0].resource_order(),
        [
            ResourceKey::InputJournal,
            ResourceKey::PreparationQueue,
            ResourceKey::PresentationFrames
        ],
        "the actual core-owned resource order"
    );
    assert_eq!(terminal_session.len(), 1);
    assert_eq!(*terminal_session[0].phase(), SessionPhase::Closing);
    assert_eq!(terminal_lifecycle.len(), 2, "one ordered terminal pair");
    assert_eq!(
        terminal_lifecycle[0].transition(),
        LifecycleTransition::Close
    );
    assert_eq!(
        terminal_lifecycle[1].transition(),
        LifecycleTransition::Invalidate
    );

    // The real terminal families publish as one transaction at revision
    // zero, verbatim from the provider's terminal publication.
    let candidate = PresentationFrame::try_new(
        lifecycle_epoch,
        ConfirmedRevision::new(0),
        1,
        vec![
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_SESSION).expect("key"),
                FamilyRecords::Session(terminal_session),
            )
            .expect("session family"),
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_LIFECYCLE).expect("key"),
                FamilyRecords::Lifecycle(terminal_lifecycle),
            )
            .expect("lifecycle family"),
        ],
    )
    .expect("candidate frame");

    let mut fixture = frame_fixture(lifecycle_epoch.get(), false);
    let mut visible = bootstrap(lifecycle_epoch.get());
    let committed = {
        let mut bundle = owners(&mut fixture, &mut visible);
        let reservation = prepare_publication(
            &bundle,
            candidate,
            AudioDedupDelta::try_new(Vec::new(), Vec::new()).expect("empty delta"),
            PublicationConsumption::default(),
            &limits(),
        )
        .expect("the terminal candidate reserves");
        commit_publication(&mut bundle, reservation)
    };
    assert_eq!(
        committed.frame_index(),
        1,
        "one atomic terminal publication"
    );
    assert_eq!(committed.confirmed_revision(), ConfirmedRevision::new(0));
    let mut lifecycle_records: Vec<&LifecycleRecord> = Vec::new();
    for family in committed.families() {
        if let FamilyRecords::Lifecycle(records) = family.records() {
            lifecycle_records = records.iter().collect();
        }
    }
    assert_eq!(lifecycle_records.len(), 2);
    assert_eq!(
        lifecycle_records[0].transition(),
        LifecycleTransition::Close
    );
    assert_eq!(
        lifecycle_records[1].resource_order(),
        [
            ResourceKey::InputJournal,
            ResourceKey::PreparationQueue,
            ResourceKey::PresentationFrames
        ],
        "the terminal Invalidate names the real core-owned order"
    );
}

/// Builds one world-ui task frame of `count` records sized to exactly
/// `target` owned bytes when given, the accepted aggregate proof technique:
/// a uniform text length absorbs the deficit record by record.
fn aggregate_world_frame(
    value: SessionEpoch,
    count: usize,
    target: Option<usize>,
) -> Result<PresentationFrame, ClientError> {
    use mornlea_domain::{CommandText, CompanionId, CompanionName, CompanionSpeaker, TaskState};
    let revision = ConfirmedRevision::new(1);
    let companion = CompanionSpeaker::new(
        CompanionId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 30])
            .expect("uuid"),
        CompanionName::try_from_canonical("spec".to_string()).expect("name"),
    );
    let observation = ObservationKey::try_new(value, revision, 0).expect("key");

    let mut lengths = vec![512usize; count];
    let frame_of = |lengths: &[usize]| -> Result<PresentationFrame, ClientError> {
        let header = RecordHeader::try_new(value, revision, Some(1), FamilyOperation::Upsert)?;
        let mut records = Vec::with_capacity(count);
        for length in lengths {
            let command = CommandText::try_from_canonical("a".repeat(*length))
                .map_err(|_| ClientError::InvalidInput)?;
            let view = mornlea_client_core::presentation::TaskView::try_new(
                observation,
                companion.clone(),
                command,
                TaskState::Progress,
            )
            .map_err(|_| ClientError::InvalidInput)?;
            records.push(WorldUiRecord::try_new(
                header,
                mornlea_client_core::presentation::WorldUiView::Task(view),
            )?);
        }
        PresentationFrame::try_new(
            value,
            revision,
            1,
            vec![FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_WORLD_UI)?,
                FamilyRecords::WorldUi(records),
            )?],
        )
    };
    let Some(target) = target else {
        return frame_of(&lengths);
    };
    let mut frame = frame_of(&lengths)?;
    let mut size = frame.validated_size()?;
    if size > target {
        return Err(ClientError::InvalidInput);
    }
    let mut deficit = target - size;
    for length in lengths.iter_mut() {
        let take = deficit.min(1024 - *length);
        *length += take;
        deficit -= take;
        if deficit == 0 {
            break;
        }
    }
    if deficit != 0 {
        return Err(ClientError::InvalidInput);
    }
    frame = frame_of(&lengths)?;
    size = frame.validated_size()?;
    assert_eq!(size, target, "the aggregate construction is exact");
    Ok(frame)
}
