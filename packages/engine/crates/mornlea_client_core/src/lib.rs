//! The standalone semantic client core: checked C1 session/input contracts,
//! C2 family records, the frame validator and the substitution ports.
//!
//! This crate is the compile-ready contract landing of the client-core
//! migration. It owns the frozen public boundary (`ClientCore`,
//! `ClientEndpoint`, `InputTranslator`, `PreparationPort`), the checked
//! supporting values, the `PresentationFrame` validator, and the checked
//! drop-geometry helper. It consumes validated `mornlea_protocol`
//! observations and emits owned immutable publications; it never imports the
//! server implementation, a Godot host, Python, or GPU code, and it adds no
//! wire decoder of its own: inbound bytes always go through
//! `mornlea_protocol::ProtocolCodec::decode_server`.
//!
//! Session, mirror, transport, prediction and family projection logic live in
//! the per-node provider files beneath `session/` and `presentation/`; this
//! root only declares modules and re-exports the frozen boundary.

#![deny(unsafe_code)]

pub mod contracts;
pub mod input;
pub mod prediction;
pub mod preparation;
pub mod presentation;
pub mod session;

pub use contracts::{
    ClientConfig, ClientCore, ClientEndpoint, ClientError, ClientIdentity, ClientLimits,
    ClientWorkBudget, CloseReason, ConfirmedRevision, Connector, ConnectorRegistry, Endpoint,
    FamilyKey, FamilyOperation, InputReceipt, MonotonicClock, ObservationKey, PreparationPort,
    RecordHeader, SessionEpoch, SessionPhase, StdMonotonicClock, StepReport, TransportLaunch,
    TransportPoll, TransportTicket,
};
pub use input::{
    ClientIntent, ClientIntentKind, ContainerOperation, ContainerToken, CraftingViewToken,
    InputAction, InputAdmissionState, InputBatch, InputTranslator, LocalCueSource,
    LocalViewValidity, ValidatedInputBatch,
};
pub use prediction::{JournalEntry, PlayerProjectionState};
pub use preparation::{
    InvalidationReport, LightSummary, LodConfig, LodStep, OwnedLodRequest, OwnedMeshView,
    PreparationJob, PreparationPayload, PreparationResult, PreparationTicket, PreparedGeometry,
    PreparedResourceKey, RejectedPreparation, SectionKey, TerrainKey, TerrainVisibility, TilePos,
};
pub use presentation::geometry::drop_position;
pub use presentation::{
    AcceptedObservation, ActorDetail, ActorDimension, ActorId, ActorKind, AudioCueCategory,
    AudioCueRecord, AudioDedupDelta, AudioDedupKey, AudioProjection, AudioProjectionState,
    BoundedText, Correction, CorrectionReason, CueId, CueProvenance, DiagnosticProjectionState,
    DiagnosticRecord, EnvironmentView, ErrorClassCounters, FRAME_LAYOUT_MAJOR, FRAME_LAYOUT_MINOR,
    FamilyFrame, FamilyRecords, FinitePositive, FiniteRay, FiniteUnit, InputAdmissionOwner,
    InputReceiptState, InputRecord, InventoryTopic, InventoryUiRecord, InventoryUiView,
    LifecycleProjectionState, LifecycleRecord, LifecycleTransition, MovementIntent, OrderedRecord,
    PlayerViewRecord, PresentationFrame, ProjectionOrder, ProjectionView, PromptView,
    PublicationConsumption, PublicationOwners, PublicationReservation, QueueCounters,
    ResolvedActor, ResourceKey, SessionRecord, StableRecordKey, SurvivalView, TaskView,
    TerrainMaterial, TerrainRecord, TextKind, UiOutcome, WorldTopic, WorldUiRecord, WorldUiView,
};
pub use session::{
    ActorConfirmed, ConfirmedMirror, ConfirmedMirrorParts, InventoryConfirmed, WorldConfirmed,
    WorldUiConfirmed,
};
