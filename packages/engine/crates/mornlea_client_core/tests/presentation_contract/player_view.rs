//! Player-view projection contract tests.
//!
//! The table pins the accepted player-view semantics against the real
//! prediction replay owner and the landed mirror provider: the confirmed pose
//! is always the authority's own pose, the predicted pose rides in its
//! explicitly attributed optional field and is never relabeled confirmed —
//! not while steps are pending, not after a correction replays them, and not
//! after a rejection removes one — and the retained correction publishes its
//! exact reason and last input sequence for every reason variant. The look
//! ray is only ever emitted inside the checked `FiniteRay` domain, whose
//! typed constructor rejects a zero, NaN or over-range reach and a NaN look
//! before any record exists. An epoch reset clears the projection owner —
//! nothing is projectable until a fresh authority lands, and the fresh
//! epoch's first authority publishes the authoritative-reset reason — and a
//! player state or observation from another epoch rejects the whole
//! projection with no partial output. Mining progress is the persisted
//! source `MiningState` of the latest confirmed authority the projection
//! state carries: it survives a full observation-queue drain unchanged — no
//! idle flap — always pairs with the confirmed base's own pose rather than a
//! newer retained observation's, and only an authoritative confirmation
//! replaces it. Elapsed frame revisions never advance it, because the
//! provider reconstructs nothing from local time, and no source tick is
//! invented for the owner-persisted summary the record publishes.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::{PlayerProjectionState, PredictionReplay, StepEnvironment};
use mornlea_client_core::presentation::family_player_view::project_player_view;
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioProjectionState, CorrectionReason, DiagnosticProjectionState,
    ErrorClassCounters, FiniteRay, LifecycleProjectionState, MovementIntent, Pose,
    ProducerIdentity, ProjectionView, PublicationConsumption, QueueCounters,
};
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    BlockPos, ChunkPos, ContainerClosed, ContainerKind, ContainerRef, Dimension, Event, FiniteVec3,
    HeldActions, LookAngles, MiningState, MiningStateParts, MotionState, MotionStateParts,
    Movement, PlayerControl, PlayerControlParts, PlayerState, PlayerStateParts, Season,
    SurvivalState, SurvivalStateParts, Weather, WorldState, WorldStateParts,
};
use mornlea_engine::native::contracts::{Aabb, CollisionCell, CollisionGrid};
use mornlea_protocol::ServerPacket;

/// The epoch every fixture mirror, replay owner and observation key carries.
const EPOCH: u64 = 7;

// The walking fixture grid, the same shape the accepted prediction replay
// cases use: one full-cube floor layer at the grid's lowest y (one block
// below the walking plane) and air above it. Cells are laid out in the
// native physics wrapper's order: y-major, then x, then z.
const WALK_ORIGIN: [i32; 3] = [-4, -1, -8];
const WALK_DIMS: [u32; 3] = [9, 6, 17];

fn fixture_cells(dims: [u32; 3]) -> Vec<CollisionCell> {
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

fn walking_grid(cells: &[CollisionCell]) -> CollisionGrid<'_> {
    CollisionGrid::try_new(WALK_ORIGIN, WALK_DIMS, cells).expect("walking fixture grid")
}

fn epoch(value: u64) -> SessionEpoch {
    SessionEpoch::try_new(value).expect("nonzero epoch")
}

fn limits() -> ClientLimits {
    ClientLimits::try_new().expect("frozen limits")
}

fn look(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).expect("finite fixture look")
}

/// One neutral forward-walking control under the given yaw, the same shape
/// the accepted prediction replay cases step with.
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

/// One overworld authority record at the caller's tick, sequence, motion,
/// look, readiness and mining state, the shape the accepted replay cases
/// use.
#[allow(clippy::too_many_arguments)]
fn authority(
    tick: u64,
    last_sequence: u64,
    position: [f32; 3],
    look: LookAngles,
    ready: bool,
    mining: MiningState,
) -> PlayerState {
    PlayerState::new(PlayerStateParts {
        server_tick: tick,
        last_input_sequence: last_sequence,
        dimension: Dimension::OVERWORLD,
        motion: MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("finite fixture position"),
            velocity: FiniteVec3::try_new([0.0, 0.0, 0.0]).expect("finite fixture velocity"),
            on_ground: true,
        }),
        look,
        ready,
        reset: false,
        mining,
        survival: survival(),
        world: world(),
    })
}

/// The transcript active swing: target (2,0,3), four of ten ticks done. The
/// exact value the mining row compares the projected record against.
fn active_mining() -> MiningState {
    MiningState::try_new(MiningStateParts {
        active: true,
        target: BlockPos::new(2, 0, 3),
        progress: 4,
        required: 10,
        harvestable: false,
    })
    .expect("checked active mining fixture")
}

/// One filler observation of a non-player topic, used to advance the frame
/// revision past the player observation without contributing player facts.
fn closed_event(generation: u32) -> Event {
    Event::ContainerClosed(ContainerClosed::new(
        ContainerRef::try_new(ChunkPos::new(-3, 7), ContainerKind::Chest, 5, generation)
            .expect("checked container reference"),
    ))
}

fn packet(event: Event) -> ServerPacket {
    ServerPacket::try_from(event).expect("publication converts to its packet")
}

/// An admitted real mirror provider with no observations yet.
fn admitted_mirror(value: u64) -> MirrorProvider {
    let mut provider =
        MirrorProvider::new(epoch(value), limits()).expect("admitted mirror provider");
    provider.admit().expect("admitted session");
    provider
}

/// Commits one publication through the real mirror provider and returns the
/// confirmed revision it issued beside the actual source key it retained.
fn commit(
    provider: &mut MirrorProvider,
    value: u64,
    event: Event,
) -> (ConfirmedRevision, ObservationKey) {
    let next = provider.mirror().revision().get() + 1;
    let key =
        ObservationKey::try_new(epoch(value), ConfirmedRevision::new(next), 0).expect("staged key");
    let staged = AcceptedObservation::try_new(key, None, packet(event), Vec::new())
        .expect("staged observation");
    let revision = provider.commit(&staged).expect("committed observation");
    (revision, key)
}

/// The projection-view state owners a fixture keeps alive beside the
/// borrowed mirror, observation queue and player projection state.
struct ViewFixture {
    input: InputProjectionState,
    audio: AudioProjectionState,
    lifecycle: LifecycleProjectionState,
    diagnostics: DiagnosticProjectionState,
    limits: ClientLimits,
}

impl ViewFixture {
    fn new() -> Self {
        Self {
            input: InputProjectionState::try_new(1).expect("input projection state"),
            audio: AudioProjectionState::try_new().expect("audio projection state"),
            lifecycle: LifecycleProjectionState::try_new(1).expect("lifecycle state"),
            diagnostics: DiagnosticProjectionState::try_new(
                ProducerIdentity::try_new([0u8; 20], [0u8; 32]).expect("producer identity"),
                QueueCounters::default(),
                ErrorClassCounters::default(),
            )
            .expect("diagnostics projection state"),
            limits: limits(),
        }
    }

    /// Builds one immutable projection view over the supplied player state,
    /// mirror and observation queue, at the given coherent candidate revision.
    #[allow(clippy::too_many_arguments)]
    fn view<'a>(
        &'a self,
        player: &'a PlayerProjectionState,
        mirror: &'a mornlea_client_core::session::ConfirmedMirror,
        observations: &'a [AcceptedObservation],
        value: u64,
        revision: ConfirmedRevision,
    ) -> ProjectionView<'a> {
        ProjectionView::try_new(
            mirror,
            observations,
            &self.input,
            player,
            &self.audio,
            &self.lifecycle,
            &self.diagnostics,
            epoch(value),
            revision,
            5,
            &self.limits,
        )
        .expect("projection view")
    }

    /// The candidate view of one provider's committed state: the next
    /// revision the coherent frame would carry.
    fn view_of<'a>(
        &'a self,
        player: &'a PlayerProjectionState,
        provider: &'a MirrorProvider,
        value: u64,
    ) -> ProjectionView<'a> {
        let revision = ConfirmedRevision::new(provider.mirror().revision().get() + 1);
        self.view(
            player,
            provider.mirror(),
            provider.observations(),
            value,
            revision,
        )
    }
}

/// The first row of the table: while unconfirmed steps are pending the
/// record carries the authority's own confirmed pose beside the explicitly
/// attributed predicted pose — the two are distinct values in distinct
/// fields, so an unconfirmed pose is never labeled confirmed — together with
/// the checked ray that follows the effective pose, the movement intent of
/// the pending control, the beginning correction and the coherent rebased
/// header.
#[test]
fn confirmed_pose_stays_authoritative_while_prediction_is_explicit() {
    let cells = fixture_cells(WALK_DIMS);
    let grid = walking_grid(&cells);
    let environment = StepEnvironment::new(grid, false);
    let mut provider = admitted_mirror(EPOCH);
    let mut replay = PredictionReplay::try_new(epoch(EPOCH), limits()).expect("replay owner");

    let begin = authority(
        1,
        0,
        [0.5, 0.0, 0.5],
        look(0.2, -0.1),
        true,
        MiningState::Idle,
    );
    let (_, key) = commit(&mut provider, EPOCH, Event::PlayerState(begin));
    let adopted = replay
        .apply_confirmed(&key, &begin, &environment)
        .expect("the first authority begins prediction");
    assert!(
        adopted.reset(),
        "the first adoption is an authoritative reset"
    );

    replay.begin_frame();
    let control = forward(0.4);
    let witness = replay
        .predict_step(1, control, &environment)
        .expect("journaled prediction step");
    let player = replay.projection().expect("attributable projection state");

    let fixture = ViewFixture::new();
    let view = fixture.view_of(&player, &provider, EPOCH);
    let records =
        project_player_view(&view).expect("an attributable player state projects one record");
    assert_eq!(records.len(), 1);
    let record = &records[0];

    // The confirmed pose is the authority's own pose, bit-for-bit.
    assert_eq!(
        record.confirmed_pose().position(),
        begin.motion().position().get().map(f64::from),
        "the confirmed pose is the authority's exact pose"
    );
    assert_eq!(record.confirmed_pose().yaw(), f64::from(0.2f32));
    assert_eq!(record.confirmed_pose().pitch(), f64::from(-0.1f32));

    // The predicted pose is the stepped witness in its own attributed field.
    assert_eq!(record.predicted_pose(), Some(witness.pose()));
    let predicted = record
        .predicted_pose()
        .expect("the pending step is attributed");
    assert_ne!(
        predicted.position(),
        record.confirmed_pose().position(),
        "an unconfirmed pose is a distinct value, never relabeled confirmed"
    );

    // The movement intent is the pending control and the stepped ground bit.
    assert_eq!(record.movement().control(), Some(&control));
    assert_eq!(record.movement().on_ground(), witness.on_ground());

    // The ray follows the effective — here predicted — pose with the
    // accepted interaction reach.
    let ray = record.look_ray().expect("the projection carries its ray");
    assert_eq!(ray.origin(), predicted.position());
    assert_eq!(ray.look(), control.look());
    assert_eq!(ray.reach(), FiniteRay::MAX_REACH);

    // The beginning correction and the rebased coherent header.
    let correction = record
        .correction()
        .expect("the adoption published a correction");
    assert_eq!(correction.reason(), CorrectionReason::AuthoritativeReset);
    assert_eq!(correction.last_input_sequence(), 0);
    assert_eq!(record.mining(), &MiningState::Idle);
    assert_eq!(record.header().operation(), FamilyOperation::Upsert);
    assert_eq!(record.header().epoch().get(), EPOCH);
    assert_eq!(record.header().revision(), view.frame_revision());
    assert_eq!(
        record.header().source_tick(),
        None,
        "the owner-persisted summary carries no source tick and none is invented"
    );
}

/// The acknowledged-replay row: a newer authority that confirms the first of
/// two pending steps publishes the acknowledged-replay reason naming the last
/// input sequence it covered, adopts the authority's pose as the confirmed
/// one, and keeps the replayed remaining step explicitly attributed.
#[test]
fn acknowledged_correction_publishes_reason_and_sequence() {
    let cells = fixture_cells(WALK_DIMS);
    let grid = walking_grid(&cells);
    let environment = StepEnvironment::new(grid, false);
    let mut provider = admitted_mirror(EPOCH);
    let mut replay = PredictionReplay::try_new(epoch(EPOCH), limits()).expect("replay owner");

    let begin = authority(
        1,
        0,
        [0.5, 0.0, 0.5],
        look(0.2, -0.1),
        true,
        MiningState::Idle,
    );
    let (_, key) = commit(&mut provider, EPOCH, Event::PlayerState(begin));
    replay
        .apply_confirmed(&key, &begin, &environment)
        .expect("begins");
    replay.begin_frame();
    let control = forward(0.4);
    replay
        .predict_step(1, control, &environment)
        .expect("first journaled step");
    replay
        .predict_step(2, control, &environment)
        .expect("second journaled step");

    let corrected = authority(
        2,
        1,
        [0.75, 0.0, 0.25],
        look(0.2, -0.1),
        true,
        MiningState::Idle,
    );
    let (_, corrected_key) = commit(&mut provider, EPOCH, Event::PlayerState(corrected));
    let outcome = replay
        .apply_confirmed(&corrected_key, &corrected, &environment)
        .expect("the correction admits");
    assert_eq!(outcome.acknowledged(), 1);
    assert_eq!(outcome.replayed(), 1);

    let player = replay.projection().expect("attributable projection state");
    let fixture = ViewFixture::new();
    let records =
        project_player_view(&fixture.view_of(&player, &provider, EPOCH)).expect("projects");
    assert_eq!(records.len(), 1);
    let record = &records[0];

    let correction = record.correction().expect("the correction is retained");
    assert_eq!(correction.reason(), CorrectionReason::AcknowledgedReplay);
    assert_eq!(
        correction.last_input_sequence(),
        1,
        "the correction names the last input sequence it covered"
    );
    assert_eq!(
        record.confirmed_pose().position(),
        corrected.motion().position().get().map(f64::from),
        "the adopted authority pose is the confirmed one"
    );
    // The replayed remaining step stays attributed and distinct from the
    // confirmed pose — the correction relabels nothing.
    let predicted = record
        .predicted_pose()
        .expect("the replayed step is still attributed");
    assert_eq!(record.predicted_pose(), player.predicted_pose());
    assert_ne!(predicted.position(), record.confirmed_pose().position());
    assert_eq!(
        record.header().source_tick(),
        None,
        "the owner-persisted summary carries no source tick and none is invented"
    );
}

/// The rejected-input row: removing one rejected input's pending prediction
/// publishes the rejected-input reason naming exactly that sequence, and the
/// replayed remainder stays explicitly attributed.
#[test]
fn rejected_input_publishes_rejected_reason_and_sequence() {
    let cells = fixture_cells(WALK_DIMS);
    let grid = walking_grid(&cells);
    let environment = StepEnvironment::new(grid, false);
    let mut provider = admitted_mirror(EPOCH);
    let mut replay = PredictionReplay::try_new(epoch(EPOCH), limits()).expect("replay owner");

    let begin = authority(
        1,
        0,
        [0.5, 0.0, 0.5],
        look(0.0, 0.0),
        true,
        MiningState::Idle,
    );
    let (_, key) = commit(&mut provider, EPOCH, Event::PlayerState(begin));
    replay
        .apply_confirmed(&key, &begin, &environment)
        .expect("begins");
    replay.begin_frame();
    let control = forward(0.0);
    replay
        .predict_step(1, control, &environment)
        .expect("first journaled step");
    replay
        .predict_step(2, control, &environment)
        .expect("second journaled step");

    let removed = replay
        .reject(1, &environment)
        .expect("the rejection removes its prediction")
        .expect("the removed sequence is published");
    assert_eq!(removed.sequence(), 1);

    let player = replay.projection().expect("attributable projection state");
    let fixture = ViewFixture::new();
    let records =
        project_player_view(&fixture.view_of(&player, &provider, EPOCH)).expect("projects");
    assert_eq!(records.len(), 1);
    let record = &records[0];

    let correction = record
        .correction()
        .expect("the rejection published a correction");
    assert_eq!(correction.reason(), CorrectionReason::RejectedInput);
    assert_eq!(correction.last_input_sequence(), 1);
    let predicted = record
        .predicted_pose()
        .expect("the replayed remainder stays attributed");
    assert_ne!(predicted.position(), record.confirmed_pose().position());
}

/// The exhausted-journal row: once an authority acknowledges every pending
/// step nothing is attributable — no predicted pose is published at all, let
/// alone relabeled confirmed — and the ray and movement fall back onto the
/// confirmed authority's own pose, look and ground bit.
#[test]
fn exhausted_journal_publishes_no_prediction_and_never_relabels() {
    let cells = fixture_cells(WALK_DIMS);
    let grid = walking_grid(&cells);
    let environment = StepEnvironment::new(grid, false);
    let mut provider = admitted_mirror(EPOCH);
    let mut replay = PredictionReplay::try_new(epoch(EPOCH), limits()).expect("replay owner");

    let begin = authority(
        1,
        0,
        [0.5, 0.0, 0.5],
        look(0.2, -0.1),
        true,
        MiningState::Idle,
    );
    let (_, key) = commit(&mut provider, EPOCH, Event::PlayerState(begin));
    replay
        .apply_confirmed(&key, &begin, &environment)
        .expect("begins");
    replay.begin_frame();
    let control = forward(0.4);
    replay
        .predict_step(1, control, &environment)
        .expect("first journaled step");
    replay
        .predict_step(2, control, &environment)
        .expect("second journaled step");

    let corrected = authority(
        2,
        2,
        [0.75, 0.0, 0.25],
        look(0.1, 0.0),
        true,
        MiningState::Idle,
    );
    let (_, corrected_key) = commit(&mut provider, EPOCH, Event::PlayerState(corrected));
    let outcome = replay
        .apply_confirmed(&corrected_key, &corrected, &environment)
        .expect("the acknowledgement admits");
    assert_eq!(outcome.acknowledged(), 2);
    assert_eq!(outcome.replayed(), 0);

    let player = replay
        .projection()
        .expect("the confirmed base still projects");
    let fixture = ViewFixture::new();
    let records =
        project_player_view(&fixture.view_of(&player, &provider, EPOCH)).expect("projects");
    assert_eq!(records.len(), 1);
    let record = &records[0];

    assert_eq!(
        record.predicted_pose(),
        None,
        "an exhausted journal attributes no predicted pose"
    );
    assert_eq!(
        record.confirmed_pose().position(),
        corrected.motion().position().get().map(f64::from)
    );
    let correction = record
        .correction()
        .expect("the acknowledgement is retained");
    assert_eq!(correction.reason(), CorrectionReason::AcknowledgedReplay);
    assert_eq!(correction.last_input_sequence(), 2);
    // With nothing pending, the ray and movement follow the confirmed
    // authority itself: its position, its look and its ground bit.
    let ray = record.look_ray().expect("the projection carries its ray");
    assert_eq!(
        ray.origin(),
        corrected.motion().position().get().map(f64::from)
    );
    assert_eq!(ray.look(), corrected.look());
    assert_eq!(record.movement().control(), None);
    assert_eq!(
        record.movement().on_ground(),
        corrected.motion().on_ground()
    );
}

/// The ray row: every emitted ray lives inside the checked `FiniteRay`
/// domain — the typed constructor rejects a zero reach, a NaN origin, a
/// NaN look and an over-range reach with a typed error before any record
/// exists — and the projected record's ray round-trips through that same
/// constructor unchanged.
#[test]
fn look_ray_is_always_inside_the_checked_finite_domain() {
    // The typed gate itself: zero, NaN and ranged values reject, the
    // accepted interaction distance admits.
    assert_eq!(
        FiniteRay::try_new([0.0; 3], look(0.0, 0.0), 0.0),
        Err(ClientError::InvalidInput),
        "a zero reach describes no ray"
    );
    assert_eq!(
        FiniteRay::try_new([0.0, f64::NAN, 0.0], look(0.0, 0.0), 6.0),
        Err(ClientError::InvalidInput),
        "a NaN origin is not finite"
    );
    assert!(
        LookAngles::try_new(f32::NAN, 0.0).is_err(),
        "a NaN look rejects before a ray can exist"
    );
    assert_eq!(
        FiniteRay::try_new([0.0; 3], look(0.0, 0.0), FiniteRay::MAX_REACH + 0.1),
        Err(ClientError::InvalidInput),
        "a reach beyond the accepted interaction distance rejects"
    );
    assert!(FiniteRay::try_new([0.0, 64.0, 0.0], look(0.0, 0.0), 6.0).is_ok());

    let cells = fixture_cells(WALK_DIMS);
    let grid = walking_grid(&cells);
    let environment = StepEnvironment::new(grid, false);
    let mut provider = admitted_mirror(EPOCH);
    let mut replay = PredictionReplay::try_new(epoch(EPOCH), limits()).expect("replay owner");
    let begin = authority(
        1,
        0,
        [0.5, 0.0, 0.5],
        look(0.2, -0.1),
        true,
        MiningState::Idle,
    );
    let (_, key) = commit(&mut provider, EPOCH, Event::PlayerState(begin));
    replay
        .apply_confirmed(&key, &begin, &environment)
        .expect("begins");
    replay.begin_frame();
    replay
        .predict_step(1, forward(0.4), &environment)
        .expect("journaled prediction step");

    let player = replay.projection().expect("attributable projection state");
    let fixture = ViewFixture::new();
    let records =
        project_player_view(&fixture.view_of(&player, &provider, EPOCH)).expect("projects");
    assert_eq!(records.len(), 1);
    let ray = records[0]
        .look_ray()
        .expect("the projection carries its ray");
    assert!(ray.origin().iter().all(|value| value.is_finite()));
    assert!(ray.reach().is_finite() && ray.reach() > 0.0 && ray.reach() <= FiniteRay::MAX_REACH);
    // The emitted ray round-trips through the typed constructor: the record
    // only ever carries a value that gate itself would admit.
    assert_eq!(
        FiniteRay::try_new(ray.origin(), ray.look(), ray.reach()),
        Ok(*ray)
    );
}

/// The absent-values row: a player state whose ray is absent publishes no
/// invented ray, a directly constructed state's default authority carries
/// the idle mining, and the header publishes no invented source tick for the
/// owner-persisted summary.
#[test]
fn absent_ray_and_default_authority_publish_no_invented_values() {
    let mut provider = admitted_mirror(EPOCH);
    let _ = commit(&mut provider, EPOCH, closed_event(9));

    let player = PlayerProjectionState::try_new(
        epoch(EPOCH),
        ConfirmedRevision::new(1),
        Pose::try_new([0.0, 64.0, 0.0], 0.0, 0.0).expect("finite pose"),
        None,
        Vec::new(),
        None,
        None,
        MovementIntent::try_new(None, true).expect("movement intent"),
    )
    .expect("checked player projection state");

    let fixture = ViewFixture::new();
    let records =
        project_player_view(&fixture.view_of(&player, &provider, EPOCH)).expect("projects");
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(
        record.look_ray(),
        None,
        "no ray is invented for a state that carries none"
    );
    assert_eq!(
        record.mining(),
        &MiningState::Idle,
        "a directly constructed state carries the idle default authority"
    );
    assert_eq!(
        record.header().source_tick(),
        None,
        "no source tick is invented for the owner-persisted summary"
    );
    assert_eq!(record.predicted_pose(), None);
}

/// The not-ready row: a not-ready authority leaves no correction and no
/// attributable prediction, and the record publishes exactly that — no
/// fabricated correction and no predicted pose.
#[test]
fn not_ready_authority_publishes_no_correction_and_no_prediction() {
    let cells = fixture_cells(WALK_DIMS);
    let grid = walking_grid(&cells);
    let environment = StepEnvironment::new(grid, false);
    let mut provider = admitted_mirror(EPOCH);
    let mut replay = PredictionReplay::try_new(epoch(EPOCH), limits()).expect("replay owner");

    let not_ready = authority(
        1,
        0,
        [0.5, 0.0, 0.5],
        look(0.0, 0.0),
        false,
        MiningState::Idle,
    );
    let (_, key) = commit(&mut provider, EPOCH, Event::PlayerState(not_ready));
    replay
        .apply_confirmed(&key, &not_ready, &environment)
        .expect("the not-ready observation admits");

    let player = replay
        .projection()
        .expect("the not-ready base still projects");
    let fixture = ViewFixture::new();
    let records =
        project_player_view(&fixture.view_of(&player, &provider, EPOCH)).expect("projects");
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(
        record.correction(),
        None,
        "a not-ready authority publishes no correction"
    );
    assert_eq!(record.predicted_pose(), None);
    assert_eq!(
        record.confirmed_pose().position(),
        not_ready.motion().position().get().map(f64::from)
    );
}

/// The stale-epoch row: a player projection state or a retained observation
/// from another epoch rejects the whole projection with `StaleEpoch` and no
/// partial output — an old epoch's prediction never enters a fresh frame,
/// and a foreign observation never enters this one.
#[test]
fn stale_epoch_inputs_reject_whole_projection_without_partial_output() {
    let cells = fixture_cells(WALK_DIMS);
    let grid = walking_grid(&cells);
    let environment = StepEnvironment::new(grid, false);
    let mut provider = admitted_mirror(EPOCH);
    let mut replay = PredictionReplay::try_new(epoch(EPOCH), limits()).expect("replay owner");
    let begin = authority(
        1,
        0,
        [0.5, 0.0, 0.5],
        look(0.0, 0.0),
        true,
        MiningState::Idle,
    );
    let (_, key) = commit(&mut provider, EPOCH, Event::PlayerState(begin));
    replay
        .apply_confirmed(&key, &begin, &environment)
        .expect("begins");
    let player = replay.projection().expect("attributable projection state");

    let fixture = ViewFixture::new();
    // A player state from the old epoch never enters the fresh epoch's frame.
    let fresh = EPOCH + 1;
    assert_eq!(
        project_player_view(&fixture.view_of(&player, &provider, fresh)),
        Err(ClientError::StaleEpoch),
        "an old-epoch player state rejects without partial output"
    );

    // A retained observation from another epoch rejects the same way, even
    // beside the valid same-epoch player state and its own valid queue.
    let stale = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch(fresh), ConfirmedRevision::new(2), 0).expect("key"),
        None,
        packet(closed_event(9)),
        Vec::new(),
    )
    .expect("staged in another epoch");
    let queue: Vec<AcceptedObservation> = provider.observations().to_vec();
    let extended_queue = [queue, vec![stale]].concat();
    assert_eq!(
        project_player_view(&fixture.view(
            &player,
            provider.mirror(),
            &extended_queue,
            EPOCH,
            ConfirmedRevision::new(2),
        )),
        Err(ClientError::StaleEpoch),
        "a foreign-epoch observation rejects without partial output"
    );

    // The same-epoch state and queue still project after the rejections.
    let records =
        project_player_view(&fixture.view_of(&player, &provider, EPOCH)).expect("still projects");
    assert_eq!(records.len(), 1);
}

/// The reset row: an epoch reset clears the projection owner — nothing is
/// projectable until a fresh authority lands — and the fresh epoch's first
/// authority publishes the authoritative-reset correction with no predicted
/// pose.
#[test]
fn epoch_reset_clears_projection_and_republishes_authoritative_reset() {
    let cells = fixture_cells(WALK_DIMS);
    let grid = walking_grid(&cells);
    let environment = StepEnvironment::new(grid, false);
    let mut provider = admitted_mirror(EPOCH);
    let mut replay = PredictionReplay::try_new(epoch(EPOCH), limits()).expect("replay owner");
    let begin = authority(
        1,
        0,
        [0.5, 0.0, 0.5],
        look(0.2, -0.1),
        true,
        MiningState::Idle,
    );
    let (_, key) = commit(&mut provider, EPOCH, Event::PlayerState(begin));
    replay
        .apply_confirmed(&key, &begin, &environment)
        .expect("begins");
    replay.begin_frame();
    replay
        .predict_step(1, forward(0.4), &environment)
        .expect("journaled prediction step");
    assert!(
        replay
            .projection()
            .expect("attributable before reset")
            .predicted_pose()
            .is_some()
    );

    // The reset clears every epoch-scoped owner of the projection.
    let fresh_epoch = EPOCH + 1;
    replay.reset_epoch(epoch(fresh_epoch)).expect("reset");
    assert!(
        matches!(replay.projection(), Err(ClientError::InvalidState)),
        "after the reset nothing is projectable — the view is cleared"
    );

    // The fresh epoch's first authority publishes the authoritative reset.
    let mut fresh_provider = admitted_mirror(fresh_epoch);
    let fresh_begin = authority(
        5,
        0,
        [1.5, 0.0, 1.5],
        look(0.0, 0.0),
        true,
        MiningState::Idle,
    );
    let (_, fresh_key) = commit(
        &mut fresh_provider,
        fresh_epoch,
        Event::PlayerState(fresh_begin),
    );
    let adopted = replay
        .apply_confirmed(&fresh_key, &fresh_begin, &environment)
        .expect("the fresh authority begins prediction");
    assert!(adopted.reset());

    let player = replay.projection().expect("the fresh base projects");
    assert_eq!(player.epoch(), epoch(fresh_epoch));
    let fixture = ViewFixture::new();
    let records = project_player_view(&fixture.view_of(&player, &fresh_provider, fresh_epoch))
        .expect("projects");
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(
        record.predicted_pose(),
        None,
        "the reset cleared every pending prediction"
    );
    let correction = record
        .correction()
        .expect("the fresh adoption published a correction");
    assert_eq!(correction.reason(), CorrectionReason::AuthoritativeReset);
    assert_eq!(correction.last_input_sequence(), 0);
    assert_eq!(
        record.confirmed_pose().position(),
        fresh_begin.motion().position().get().map(f64::from)
    );
}

/// The mining row: the projected mining state is the persisted source value
/// of the latest confirmed authority the projection state carries — active
/// swing preserved field-for-field, never advanced by the frame revisions
/// that elapsed after it, never dropped to idle by a full observation-queue
/// drain, always paired with the confirmed base's own pose rather than a
/// newer retained observation's, and replaced exactly when a newer
/// authoritative confirmation carries the idle swing.
#[test]
fn mining_is_the_persisted_source_state_of_the_confirmed_base() {
    let cells = fixture_cells(WALK_DIMS);
    let grid = walking_grid(&cells);
    let environment = StepEnvironment::new(grid, false);
    let mut provider = admitted_mirror(EPOCH);
    let mut replay = PredictionReplay::try_new(epoch(EPOCH), limits()).expect("replay owner");

    let swing = active_mining();
    let begin = authority(1, 0, [0.5, 0.0, 0.5], look(0.2, -0.1), true, swing);
    let (_, key) = commit(&mut provider, EPOCH, Event::PlayerState(begin));
    replay
        .apply_confirmed(&key, &begin, &environment)
        .expect("begins");
    replay.begin_frame();
    replay
        .predict_step(1, forward(0.4), &environment)
        .expect("journaled prediction step");
    let player = replay.projection().expect("attributable projection state");
    assert_eq!(player.mining(), &swing);

    let fixture = ViewFixture::new();
    let records =
        project_player_view(&fixture.view_of(&player, &provider, EPOCH)).expect("projects");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].mining(), &swing);
    let active = match records[0].mining() {
        MiningState::Active(active) => *active,
        other => panic!("the authoritative swing is active: {other:?}"),
    };
    assert_eq!(active.target(), BlockPos::new(2, 0, 3));
    assert_eq!(active.progress(), 4);
    assert_eq!(active.required(), 10);
    assert!(!active.harvestable());

    // Two frame revisions elapse after the player observation: the swing is
    // still the exact source value, never advanced by elapsed revisions or
    // reconstructed from local time.
    let _ = commit(&mut provider, EPOCH, closed_event(9));
    let _ = commit(&mut provider, EPOCH, closed_event(10));
    let records =
        project_player_view(&fixture.view_of(&player, &provider, EPOCH)).expect("projects");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].mining(),
        &swing,
        "elapsed frame revisions never advance authoritative mining progress"
    );

    // The post-consumption persistence row: a FULL observation-queue drain —
    // the publication cursor consuming every retained observation, including
    // the player observation that carried the swing — leaves the active
    // swing and its paired confirmed pose exactly as they were. The persisted
    // confirmed base, not a retained observation, owns the mining, so no
    // idle flap can exist.
    let drained = PublicationConsumption::try_new(provider.observations().len(), 0, 0, 0)
        .expect("full drain cursor");
    provider.consume(&drained).expect("full drain consumes");
    assert!(
        provider.observations().is_empty(),
        "the queue is fully drained"
    );
    let records =
        project_player_view(&fixture.view_of(&player, &provider, EPOCH)).expect("projects");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].mining(),
        &swing,
        "a fully drained queue never flips an active swing to idle"
    );
    assert_eq!(
        records[0].confirmed_pose().position(),
        begin.motion().position().get().map(f64::from),
        "the mining stays paired with the confirmed base's own pose"
    );

    // The pairing discipline: a NEWER retained player observation carrying
    // the idle swing — committed but not yet confirmed onto the projection
    // owner — changes nothing. The published mining pairs with the confirmed
    // base's pose, never a newer retained observation's value.
    let stopped = authority(
        2,
        1,
        [0.75, 0.0, 0.25],
        look(0.2, -0.1),
        true,
        MiningState::Idle,
    );
    let _ = commit(&mut provider, EPOCH, Event::PlayerState(stopped));
    let records =
        project_player_view(&fixture.view_of(&player, &provider, EPOCH)).expect("projects");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].mining(),
        &swing,
        "a newer retained observation alone never replaces the persisted swing"
    );
    assert_eq!(
        records[0].confirmed_pose().position(),
        begin.motion().position().get().map(f64::from),
        "the record never pairs the base's mining with a newer observation's pose"
    );

    // Only an authoritative confirmation replaces the persisted swing: once
    // the newer observation is confirmed onto the projection owner, the
    // mining and the confirmed pose move together.
    let observations = provider.observations().to_vec();
    let stopped_key = *observations
        .last()
        .expect("the newer player observation is retained")
        .key();
    replay
        .apply_confirmed(&stopped_key, &stopped, &environment)
        .expect("the newer authority admits");
    let player = replay.projection().expect("attributable projection state");
    let records =
        project_player_view(&fixture.view_of(&player, &provider, EPOCH)).expect("projects");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].mining(),
        &MiningState::Idle,
        "the newer authoritative confirmation ended the swing exactly"
    );
    assert_eq!(
        records[0].confirmed_pose().position(),
        stopped.motion().position().get().map(f64::from),
        "the replaced mining pairs with the newly confirmed pose"
    );
}
