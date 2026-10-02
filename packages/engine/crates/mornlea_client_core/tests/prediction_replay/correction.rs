//! The correction replay cases: the epoch/sequence prediction journal,
//! acknowledge-once corrections, rejection removal and the authoritative
//! replay through the frozen F1 numerical kernels.
//!
//! Every row drives the same assertions through one `UnderTest` adapter, so
//! the committed table can run against the real provider. The deliberately
//! wrong replay double — a journal that acknowledges twice, keeps rejected
//! intent, applies foreign-epoch corrections and replays from the predicted
//! state instead of the latest confirmed one — is the only replay behavior
//! that exists before the provider lands, so the behavioral red runs against
//! it and the wrong behavior stays executable in the pinned artifact at the
//! bottom.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, ObservationKey, SessionEpoch,
};
use mornlea_client_core::presentation::CorrectionReason;
use mornlea_domain::{
    BlockPos, Dimension, FiniteVec3, HeldActions, LookAngles, MiningState, MiningStateParts,
    MotionState, MotionStateParts, Movement, PlayerControl, PlayerControlParts, PlayerState,
    PlayerStateParts, Season, SurvivalState, SurvivalStateParts, Weather, WorldState,
    WorldStateParts,
};
use mornlea_engine::native::contracts::{
    Aabb, CollisionCell, CollisionGrid, PhysicsControls, PhysicsRequest, PhysicsState,
    PhysicsTuning, SweepBounds,
};
use mornlea_engine::native::physics::step_physics;

/// The frozen F1 collision epsilon both accepted kernels share — the Go
/// `physics.CollisionEpsilon` and the Rust engine collision `EPSILON` — reused
/// as the tolerance of every numeric comparison here. No worker-selected
/// epsilon exists in this file.
const F1_COLLISION_EPSILON: f64 = 1e-5;

/// The frozen F1 default tuning, transcribed from the accepted Go
/// `DefaultTunables` snapshot the predictor and the authority step with.
const TUNING: PhysicsTuning = PhysicsTuning {
    fixed_delta_seconds: 0.05,
    step_height: 0.6,
    walk_speed: 4.3,
    ground_acceleration: 40.0,
    ground_deceleration: 50.0,
    air_acceleration: 8.0,
    jump_speed: 8.4,
    gravity: 32.0,
    terminal_fall_speed: 78.4,
    fluid_gravity: 6.4,
    fluid_sink_speed: 3.0,
    fluid_ascend_speed: 4.0,
    fluid_horizontal_drag: 0.8,
    sprint_speed_multiplier: 1.3,
    sneak_speed_multiplier: 0.3,
};

// The walking fixture grid: every cell loaded, one full-cube floor layer at
// the grid's lowest y (one block below the walking plane), air above it and
// an optional two-block-high wall along world cell column x=1. Cells are laid
// out in the native physics wrapper's order: y-major, then x, then z.
const WALK_ORIGIN: [i32; 3] = [-4, -1, -8];
const WALK_DIMS: [u32; 3] = [9, 6, 17];

/// The transcript fixture grid, the same shape raised so its walking plane
/// sits at y=10 exactly where the Go transcript case plays.
const TRANSCRIPT_ORIGIN: [i32; 3] = [-4, 9, -8];
const TRANSCRIPT_DIMS: [u32; 3] = [9, 3, 17];

fn fixture_cells(dims: [u32; 3], wall: bool) -> Vec<CollisionCell> {
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
        for x in 0..dims[0] {
            for z in 0..dims[2] {
                let _ = z;
                let floor = y == 0;
                let wall_cell = wall && x == 5 && (y == 1 || y == 2);
                let mut boxes = [empty; 8];
                let used = if floor || wall_cell {
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

fn transcript_grid(cells: &[CollisionCell]) -> CollisionGrid<'_> {
    CollisionGrid::try_new(TRANSCRIPT_ORIGIN, TRANSCRIPT_DIMS, cells).expect("transcript grid")
}

/// One fixed step through the frozen F1 kernel. The sweep envelope only
/// validates displacement — it never clamps a value — so the deliberately
/// wide fixture envelope leaves every stepped number identical to the exact
/// caller-side envelope while keeping this helper independent of the provider.
fn kernel_step(
    state: PhysicsState,
    control: &PlayerControl,
    grid: &CollisionGrid<'_>,
) -> (PhysicsState, [bool; 3], bool, bool) {
    let look = control.look();
    let yaw = f64::from(look.yaw());
    let controls = PhysicsControls {
        move_x: control.movement().move_x,
        move_z: control.movement().move_z,
        jump: control.movement().jump,
        yaw_sin: yaw.sin() as f32,
        yaw_cos: yaw.cos() as f32,
        body_in_fluid: false,
        sprinting: control.actions().sprinting,
        sneaking: control.actions().sneaking,
    };
    let request = PhysicsRequest {
        state,
        controls,
        tuning: TUNING,
        sweep: SweepBounds {
            minimum: [-1000.0; 3],
            maximum: [1000.0; 3],
        },
        grid: *grid,
    };
    let result = step_physics(&request).expect("fixture step inside the frozen kernel");
    (
        result.state,
        result.clipped,
        result.used_step,
        result.hit_unknown,
    )
}

/// Folds controls through the frozen kernel in the given order: the oracle
/// every replay parity assertion compares against.
fn fold(start: PhysicsState, controls: &[PlayerControl], grid: &CollisionGrid<'_>) -> PhysicsState {
    let mut state = start;
    for control in controls {
        state = kernel_step(state, control, grid).0;
    }
    state
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

/// One neutral walking control under the given yaw.
fn control(move_x: i8, move_z: i8, yaw: f32) -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x,
            move_z,
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

/// One walking control with every held bit explicit, for the rows that need
/// the jump branch of the frozen kernel.
fn held_control(move_x: i8, move_z: i8, jump: bool) -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x,
            move_z,
            jump,
        },
        look: look(0.0, 0.0),
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

/// One ready overworld authority record: the shape the Go transcript fixture
/// `predictionReadyState` carries, at the caller's coordinates and mining
/// state.
fn authority_with_mining(
    tick: u64,
    last_sequence: u64,
    position: [f32; 3],
    velocity: [f32; 3],
    on_ground: bool,
    look: LookAngles,
    mining: MiningState,
) -> PlayerState {
    PlayerState::new(PlayerStateParts {
        server_tick: tick,
        last_input_sequence: last_sequence,
        dimension: Dimension::OVERWORLD,
        motion: MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("finite fixture position"),
            velocity: FiniteVec3::try_new(velocity).expect("finite fixture velocity"),
            on_ground,
        }),
        look,
        ready: true,
        reset: false,
        mining,
        survival: survival(),
        world: world(),
    })
}

fn authority(
    tick: u64,
    last_sequence: u64,
    position: [f32; 3],
    velocity: [f32; 3],
    on_ground: bool,
    look: LookAngles,
) -> PlayerState {
    authority_with_mining(
        tick,
        last_sequence,
        position,
        velocity,
        on_ground,
        look,
        MiningState::Idle,
    )
}

/// One checked active swing against the given target block, so persistence
/// rows can distinguish two different authoritative swings.
fn active_mining_at(target: BlockPos) -> MiningState {
    MiningState::try_new(MiningStateParts {
        active: true,
        target,
        progress: 4,
        required: 10,
        harvestable: false,
    })
    .expect("checked active mining fixture")
}

fn active_mining() -> MiningState {
    active_mining_at(BlockPos::new(2, 0, 3))
}

fn key(epoch: SessionEpoch, revision: u64) -> ObservationKey {
    ObservationKey::try_new(epoch, ConfirmedRevision::new(revision), 0)
        .expect("fixture observation key")
}

/// Maps a correction reason onto a comparable tag.
fn reason_tag(reason: CorrectionReason) -> u8 {
    match reason {
        CorrectionReason::AuthoritativeReset => 0,
        CorrectionReason::AcknowledgedReplay => 1,
        CorrectionReason::RejectedInput => 2,
    }
}

/// The comparable outcome of one predicted fixed step.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StepOutcome {
    sequence: u64,
    position: [f64; 3],
    velocity: [f32; 3],
    on_ground: bool,
    clipped: [bool; 3],
    used_step: bool,
    hit_unknown: bool,
}

/// The comparable counts of one authoritative correction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CorrectionCounts {
    acknowledged: usize,
    replayed: usize,
    reset: bool,
}

/// The comparable projection snapshot: everything the player projection
/// carries, in a form both drivers report so unchanged-state assertions are
/// bit-for-bit.
#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    confirmed_position: Option<[f64; 3]>,
    predicted_position: Option<[f64; 3]>,
    predicted_on_ground: Option<bool>,
    journal: Vec<u64>,
    correction: Option<(u64, u8)>,
    look_ray_origin: Option<[f64; 3]>,
    look_reach: Option<f32>,
    movement_control_present: bool,
    source_mining: Option<MiningState>,
    projection_mining: Option<MiningState>,
}

impl Snapshot {
    /// The nothing-attributable snapshot an owner reports before its first
    /// confirmed authority.
    fn empty() -> Self {
        Self {
            confirmed_position: None,
            predicted_position: None,
            predicted_on_ground: None,
            journal: Vec::new(),
            correction: None,
            look_ray_origin: None,
            look_reach: None,
            movement_control_present: false,
            source_mining: None,
            projection_mining: None,
        }
    }
}

/// The deliberately wrong replay double: the journal keeps rejected intent,
/// a duplicate acknowledgement re-processes the correction, a foreign-epoch
/// correction is applied, out-of-range axes are silently clamped instead of
/// rejected, no journal or frame bound exists, and replay continues from the
/// current predicted state instead of rebasing on the latest confirmed one.
/// Every step still runs through the frozen kernel, so its stepping is exact
/// and only the journal and replay bookkeeping is wrong.
struct WrongReplayDouble {
    journal: Vec<(u64, PlayerControl)>,
    confirmed: Option<PhysicsState>,
    predicted: PhysicsState,
    correction: Option<(u64, u8)>,
    mining: Option<MiningState>,
}

impl WrongReplayDouble {
    fn new() -> Self {
        Self {
            journal: Vec::new(),
            confirmed: None,
            predicted: PhysicsState {
                position: [0.0; 3],
                velocity: [0.0; 3],
                on_ground: false,
            },
            correction: None,
            mining: None,
        }
    }

    fn predict(
        &mut self,
        sequence: u64,
        control: PlayerControl,
        grid: &CollisionGrid<'_>,
    ) -> StepOutcome {
        // Wrong: silent clamp instead of a typed range rejection, and neither
        // the journal bound nor the per-frame step bound is checked.
        let clamped = PlayerControl::new(PlayerControlParts {
            movement: Movement {
                move_x: control.movement().move_x.clamp(-1, 1),
                move_z: control.movement().move_z.clamp(-1, 1),
                jump: control.movement().jump,
            },
            look: control.look(),
            actions: control.actions(),
        });
        let (state, clipped, used_step, hit_unknown) = kernel_step(self.predicted, &clamped, grid);
        self.predicted = state;
        self.journal.push((sequence, clamped));
        StepOutcome {
            sequence,
            position: state.position.map(f64::from),
            velocity: state.velocity,
            on_ground: state.on_ground,
            clipped,
            used_step,
            hit_unknown,
        }
    }

    fn confirm(&mut self, state: &PlayerState, grid: &CollisionGrid<'_>) -> CorrectionCounts {
        // Wrong: no epoch gate and no stale-tick guard, so a duplicate
        // acknowledgement re-processes and a foreign-epoch correction applies.
        let last = state.last_input_sequence();
        let reset = self.confirmed.is_none() || state.reset();
        let acknowledged = if reset {
            0
        } else {
            self.journal.iter().filter(|(s, _)| *s <= last).count()
        };
        if reset {
            self.journal.clear();
        } else {
            self.journal.retain(|(sequence, _)| *sequence > last);
        }
        let motion = state.motion();
        let confirmed = PhysicsState {
            position: motion.position().get(),
            velocity: motion.velocity().get(),
            on_ground: motion.on_ground(),
        };
        self.confirmed = Some(confirmed);
        self.mining = Some(state.mining());
        if reset {
            // The one correct half: a fresh adoption begins at the authority,
            // exactly like the accepted predictor's begin.
            self.predicted = confirmed;
        }
        // Wrong: every later replay continues from the current predicted
        // state instead of rebasing on the confirmed one.
        for (_, control) in &self.journal {
            self.predicted = kernel_step(self.predicted, control, grid).0;
        }
        let replayed = self.journal.len();
        self.correction = Some((last, if reset { 0 } else { 1 }));
        CorrectionCounts {
            acknowledged,
            replayed,
            reset,
        }
    }

    fn reject(&mut self, _sequence: u64) -> Option<u64> {
        // Wrong: the rejected intent survives the journal and no cue
        // attribution is returned.
        None
    }

    fn reset(&mut self) {
        self.journal.clear();
        self.confirmed = None;
        self.predicted = PhysicsState {
            position: [0.0; 3],
            velocity: [0.0; 3],
            on_ground: false,
        };
        self.correction = None;
        self.mining = None;
    }

    fn journal(&self) -> Vec<u64> {
        self.journal.iter().map(|(sequence, _)| *sequence).collect()
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            confirmed_position: self.confirmed.map(|state| state.position.map(f64::from)),
            predicted_position: if self.journal.is_empty() {
                None
            } else {
                Some(self.predicted.position.map(f64::from))
            },
            predicted_on_ground: if self.journal.is_empty() {
                None
            } else {
                Some(self.predicted.on_ground)
            },
            journal: self.journal(),
            correction: self.correction,
            look_ray_origin: None,
            look_reach: None,
            movement_control_present: !self.journal.is_empty(),
            source_mining: self.mining,
            projection_mining: self.mining,
        }
    }
}

/// One adapter surface the table drives: the real provider, or the
/// deliberately wrong double whose journal the behavioral red ran against and
/// whose wrong behavior the pinned artifact keeps executable.
enum Driver {
    Double,
    Provider,
}

const DRIVER: Driver = Driver::Provider;

struct UnderTest {
    double: WrongReplayDouble,
    provider: Option<mornlea_client_core::prediction::PredictionReplay>,
}

impl UnderTest {
    fn new(epoch: SessionEpoch, limits: ClientLimits) -> Self {
        Self {
            double: WrongReplayDouble::new(),
            provider: Some(
                mornlea_client_core::prediction::PredictionReplay::try_new(epoch, limits)
                    .expect("replay owner"),
            ),
        }
    }

    fn begin_frame(&mut self) {
        match DRIVER {
            Driver::Double => {}
            Driver::Provider => self
                .provider
                .as_mut()
                .expect("provider under test")
                .begin_frame(),
        }
    }

    fn predict(
        &mut self,
        sequence: u64,
        control: PlayerControl,
        grid: &CollisionGrid<'_>,
    ) -> Result<StepOutcome, ClientError> {
        match DRIVER {
            Driver::Double => Ok(self.double.predict(sequence, control, grid)),
            Driver::Provider => {
                let environment =
                    mornlea_client_core::prediction::StepEnvironment::new(*grid, false);
                self.provider
                    .as_mut()
                    .expect("provider under test")
                    .predict_step(sequence, control, &environment)
                    .map(|witness| StepOutcome {
                        sequence: witness.sequence(),
                        position: witness.pose().position(),
                        velocity: witness.velocity(),
                        on_ground: witness.on_ground(),
                        clipped: witness.clipped(),
                        used_step: witness.used_step(),
                        hit_unknown: witness.hit_unknown(),
                    })
            }
        }
    }

    fn confirm(
        &mut self,
        key: &ObservationKey,
        state: &PlayerState,
        grid: &CollisionGrid<'_>,
    ) -> Result<CorrectionCounts, ClientError> {
        match DRIVER {
            Driver::Double => Ok(self.double.confirm(state, grid)),
            Driver::Provider => {
                let environment =
                    mornlea_client_core::prediction::StepEnvironment::new(*grid, false);
                self.provider
                    .as_mut()
                    .expect("provider under test")
                    .apply_confirmed(key, state, &environment)
                    .map(|witness| CorrectionCounts {
                        acknowledged: witness.acknowledged(),
                        replayed: witness.replayed(),
                        reset: witness.reset(),
                    })
            }
        }
    }

    fn reject(
        &mut self,
        sequence: u64,
        grid: &CollisionGrid<'_>,
    ) -> Result<Option<u64>, ClientError> {
        match DRIVER {
            Driver::Double => Ok(self.double.reject(sequence)),
            Driver::Provider => {
                let environment =
                    mornlea_client_core::prediction::StepEnvironment::new(*grid, false);
                self.provider
                    .as_mut()
                    .expect("provider under test")
                    .reject(sequence, &environment)
                    .map(|removed| removed.map(|rejected| rejected.sequence()))
            }
        }
    }

    fn reset(&mut self, epoch: SessionEpoch) -> Result<(), ClientError> {
        match DRIVER {
            Driver::Double => {
                self.double.reset();
                Ok(())
            }
            Driver::Provider => self
                .provider
                .as_mut()
                .expect("provider under test")
                .reset_epoch(epoch),
        }
    }

    fn journal(&self) -> Vec<u64> {
        match DRIVER {
            Driver::Double => self.double.journal(),
            Driver::Provider => self
                .provider
                .as_ref()
                .and_then(|owner| owner.projection().ok())
                .map(|projection| {
                    projection
                        .journal()
                        .iter()
                        .map(|entry| entry.sequence())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn snapshot(&self) -> Snapshot {
        match DRIVER {
            Driver::Double => self.double.snapshot(),
            Driver::Provider => {
                let owner = self.provider.as_ref().expect("provider under test");
                let Ok(projection) = owner.projection() else {
                    // No confirmed base yet: nothing is attributable at all,
                    // which is itself the comparable state.
                    return Snapshot::empty();
                };
                Snapshot {
                    confirmed_position: Some(projection.confirmed_pose().position()),
                    predicted_position: projection.predicted_pose().map(|pose| pose.position()),
                    predicted_on_ground: projection
                        .predicted_pose()
                        .map(|_| projection.movement().on_ground()),
                    journal: projection
                        .journal()
                        .iter()
                        .map(|entry| entry.sequence())
                        .collect(),
                    correction: projection.correction().map(|correction| {
                        (
                            correction.last_input_sequence(),
                            reason_tag(correction.reason()),
                        )
                    }),
                    look_ray_origin: projection.look_ray().map(|ray| ray.origin()),
                    look_reach: projection.look_ray().map(|ray| ray.reach()),
                    movement_control_present: projection.movement().control().is_some(),
                    source_mining: owner.source_mining(),
                    projection_mining: Some(*projection.mining()),
                }
            }
        }
    }
}

/// Asserts two kernel positions agree bit-for-bit — both widen the same f32
/// bits to f64 — the parity form every replay row compares with.
#[track_caller]
fn assert_same_position(actual: [f64; 3], expected: [f32; 3]) {
    assert_eq!(
        actual,
        expected.map(f64::from),
        "the replay must match the frozen kernel fold exactly"
    );
}

#[track_caller]
fn near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < F1_COLLISION_EPSILON,
        "value {actual} is not within the frozen F1 tolerance of {expected}"
    );
}

/// The Go transcript case (`TestPredictReconcileParityReplaysUnconfirmedInput`
/// in `packages/client/runtime`): the authority begins at (0.5, 10, 0.5),
/// three fixed steps of forward movement under yaw 0.4 are journaled, then a
/// newer authoritative state at (0.75, 10, 0.25) acknowledges only the first.
/// The remaining two steps must replay in sequence order from the confirmed
/// state — never continuing from the pre-correction prediction — through the
/// accepted numerical kernel.
#[test]
fn go_transcript_reconcile_replays_two_unconfirmed_steps() {
    let cells = fixture_cells(TRANSCRIPT_DIMS, false);
    let grid = transcript_grid(&cells);
    let epoch_one = epoch(1);
    let mut test = UnderTest::new(epoch_one, limits());

    let begin = test
        .confirm(
            &key(epoch_one, 1),
            &authority(
                1,
                0,
                [0.5, 10.0, 0.5],
                [0.0, 0.0, 0.0],
                true,
                look(0.2, -0.1),
            ),
            &grid,
        )
        .expect("the first authority begins prediction");
    assert_eq!(
        begin,
        CorrectionCounts {
            acknowledged: 0,
            replayed: 0,
            reset: true,
        },
        "the first adoption is an authoritative reset"
    );

    let forward = control(0, 1, 0.4);
    for sequence in 1..=3u64 {
        test.predict(sequence, forward, &grid)
            .expect("journaled prediction step");
    }
    assert_eq!(test.journal(), vec![1, 2, 3]);

    let corrected = authority(
        2,
        1,
        [0.75, 10.0, 0.25],
        [0.0, 0.0, 0.0],
        true,
        look(0.2, -0.1),
    );
    let outcome = test
        .confirm(&key(epoch_one, 2), &corrected, &grid)
        .expect("the correction admits");
    assert_eq!(
        outcome,
        CorrectionCounts {
            acknowledged: 1,
            replayed: 2,
            reset: false,
        }
    );
    assert_eq!(
        test.journal(),
        vec![2, 3],
        "only the first step is confirmed"
    );

    let snapshot = test.snapshot();
    let replayed = snapshot
        .predicted_position
        .expect("the replay attributes a predicted pose");
    let direct = fold(
        PhysicsState {
            position: [0.75, 10.0, 0.25],
            velocity: [0.0, 0.0, 0.0],
            on_ground: true,
        },
        &[forward, forward],
        &grid,
    );
    assert_same_position(replayed, direct.position);
    // Two ground accelerations from the authoritative zero velocity reach
    // twice the per-step acceleration budget of 2 m/s.
    let horizontal = f64::from(
        (direct.velocity[0] * direct.velocity[0] + direct.velocity[2] * direct.velocity[2]).sqrt(),
    );
    near(horizontal, 4.0);
}

/// The correction row of the binding table: four controls with the second
/// acknowledged and an authoritative correction in between; the remaining
/// entries replay from the latest confirmed state in sequence order. The
/// axis-split controls make a wrong base or a swapped order produce a
/// different position than the kernel fold.
#[test]
fn correction_replays_remaining_from_confirmed_in_sequence_order() {
    let cells = fixture_cells(WALK_DIMS, false);
    let grid = walking_grid(&cells);
    let epoch_one = epoch(1);
    let mut test = UnderTest::new(epoch_one, limits());
    test.confirm(
        &key(epoch_one, 1),
        &authority(1, 0, [0.5, 0.0, 0.5], [0.0, 0.0, 0.0], true, look(0.0, 0.0)),
        &grid,
    )
    .expect("begins");

    let north = control(0, 1, 0.0);
    let east = control(1, 0, 0.0);
    for (sequence, step) in [(1u64, north), (2, east), (3, north), (4, east)] {
        test.predict(sequence, step, &grid).expect("journaled step");
    }

    let corrected = authority(
        2,
        2,
        [0.75, 0.0, 0.25],
        [0.0, 0.0, 0.0],
        true,
        look(0.0, 0.0),
    );
    let outcome = test
        .confirm(&key(epoch_one, 2), &corrected, &grid)
        .expect("correction admits");
    assert_eq!(
        outcome,
        CorrectionCounts {
            acknowledged: 2,
            replayed: 2,
            reset: false,
        }
    );
    assert_eq!(test.journal(), vec![3, 4]);

    let snapshot = test.snapshot();
    let replayed = snapshot
        .predicted_position
        .expect("attributed predicted pose after replay");
    let direct = fold(
        PhysicsState {
            position: [0.75, 0.0, 0.25],
            velocity: [0.0, 0.0, 0.0],
            on_ground: true,
        },
        &[north, east],
        &grid,
    );
    assert_same_position(replayed, direct.position);
    // Both replayed axes move, so neither entry can be missing, and the
    // replay starts from the correction, not from the pre-correction
    // prediction further north.
    assert!(replayed[0] > 0.75, "the second replayed entry moves east");
    assert!(replayed[2] < 0.25, "the first replayed entry moves north");
    assert!(replayed[2] > 0.0, "the replay rebased on the correction");
    assert_eq!(
        snapshot.correction,
        Some((2, reason_tag(CorrectionReason::AcknowledgedReplay)))
    );
}

/// Acknowledgement is idempotent-once: re-delivering the same authoritative
/// observation (the same key and server tick) is a no-op that leaves every
/// projection field bit-for-bit unchanged.
#[test]
fn duplicate_acknowledgement_leaves_state_unchanged() {
    let cells = fixture_cells(WALK_DIMS, false);
    let grid = walking_grid(&cells);
    let epoch_one = epoch(1);
    let mut test = UnderTest::new(epoch_one, limits());
    test.confirm(
        &key(epoch_one, 1),
        &authority(1, 0, [0.5, 0.0, 0.5], [0.0, 0.0, 0.0], true, look(0.0, 0.0)),
        &grid,
    )
    .expect("begins");
    for (sequence, step) in [(1u64, control(0, 1, 0.0)), (2, control(1, 0, 0.0))] {
        test.predict(sequence, step, &grid).expect("journaled step");
    }
    let corrected = authority(2, 1, [0.6, 0.0, 0.4], [0.0, 0.0, 0.0], true, look(0.0, 0.0));
    test.confirm(&key(epoch_one, 2), &corrected, &grid)
        .expect("first acknowledgement");
    let before = test.snapshot();

    let duplicate = test
        .confirm(&key(epoch_one, 2), &corrected, &grid)
        .expect("the duplicate is admitted as a no-op");
    assert_eq!(
        duplicate,
        CorrectionCounts {
            acknowledged: 0,
            replayed: 0,
            reset: false,
        },
        "nothing is acknowledged or replayed twice"
    );
    assert_eq!(
        test.snapshot(),
        before,
        "duplicate ack leaves state unchanged"
    );
}

/// A rejected input sequence removes its pending prediction and its cue
/// attribution, then replays the remaining entries from the latest confirmed
/// state. Re-rejecting the same sequence has nothing left to remove.
#[test]
fn rejection_removes_pending_prediction_and_cue_attribution() {
    let cells = fixture_cells(WALK_DIMS, false);
    let grid = walking_grid(&cells);
    let epoch_one = epoch(1);
    let mut test = UnderTest::new(epoch_one, limits());
    test.confirm(
        &key(epoch_one, 1),
        &authority(1, 0, [0.5, 0.0, 0.5], [0.0, 0.0, 0.0], true, look(0.0, 0.0)),
        &grid,
    )
    .expect("begins");
    let north = control(0, 1, 0.0);
    let east = control(1, 0, 0.0);
    for (sequence, step) in [(1u64, north), (2, east), (3, north)] {
        test.predict(sequence, step, &grid).expect("journaled step");
    }

    let removed = test.reject(2, &grid).expect("rejection admits");
    assert_eq!(
        removed,
        Some(2),
        "the cue-attribution key of the removed intent"
    );
    assert_eq!(test.journal(), vec![1, 3]);

    let snapshot = test.snapshot();
    let replayed = snapshot
        .predicted_position
        .expect("attributed predicted pose after the rejection replay");
    let direct = fold(
        PhysicsState {
            position: [0.5, 0.0, 0.5],
            velocity: [0.0, 0.0, 0.0],
            on_ground: true,
        },
        &[north, north],
        &grid,
    );
    assert_same_position(replayed, direct.position);
    assert_eq!(
        snapshot.correction,
        Some((2, reason_tag(CorrectionReason::RejectedInput)))
    );

    let before = test.snapshot();
    assert_eq!(test.reject(2, &grid).expect("re-rejection admits"), None);
    assert_eq!(test.snapshot(), before, "re-rejection changes nothing");
}

/// The collision row: a two-block wall clips prediction through the frozen
/// collision kernel, and the correction replay routes through the same
/// kernel, so the clipped result — not the free walk — is what replay renders.
#[test]
fn collision_clips_prediction_and_replay_through_kernel() {
    let cells = fixture_cells(WALK_DIMS, true);
    let grid = walking_grid(&cells);
    let epoch_one = epoch(1);
    let mut test = UnderTest::new(epoch_one, limits());
    test.confirm(
        &key(epoch_one, 1),
        &authority(1, 0, [0.5, 0.0, 0.5], [0.0, 0.0, 0.0], true, look(0.0, 0.0)),
        &grid,
    )
    .expect("begins");

    let east = control(1, 0, 0.0);
    let first = test.predict(1, east, &grid).expect("first step");
    near(first.position[0] - 0.5, 0.1);
    assert!(!first.clipped[0], "the first step does not reach the wall");
    let second = test.predict(2, east, &grid).expect("second step");
    assert!(second.clipped[0], "the second step reaches the wall");
    assert_eq!(
        second.velocity[0], 0.0,
        "a clipped axis zeroes its velocity"
    );
    assert!(
        second.position[0] <= 0.7,
        "the wall stops the predicted body at its face"
    );
    let north = control(0, 1, 0.0);
    test.predict(3, north, &grid).expect("third step");

    // The correction pulls the confirmed base back from the wall, so the
    // replay of the remaining north step must leave the corrected east
    // coordinate alone: a replay from the wall-clipped prediction instead of
    // the confirmed base would carry the clipped position forward.
    let corrected = authority(2, 2, [0.2, 0.0, 0.5], [0.0, 0.0, 0.0], true, look(0.0, 0.0));
    let outcome = test
        .confirm(&key(epoch_one, 2), &corrected, &grid)
        .expect("correction admits");
    assert_eq!(
        outcome,
        CorrectionCounts {
            acknowledged: 2,
            replayed: 1,
            reset: false,
        }
    );
    let replayed = test
        .snapshot()
        .predicted_position
        .expect("attributed predicted pose");
    let direct = fold(
        PhysicsState {
            position: [0.2, 0.0, 0.5],
            velocity: [0.0, 0.0, 0.0],
            on_ground: true,
        },
        &[north],
        &grid,
    );
    assert_same_position(replayed, direct.position);
    near(replayed[0], 0.2);
    near(replayed[2], 0.4);
}

/// Reset opens a new epoch whose sequence space restarts, and a correction
/// from the old epoch — even one with a newer server tick — can never affect
/// the new epoch's state.
#[test]
fn reset_and_old_epoch_correction_cannot_affect_new_epoch() {
    let cells = fixture_cells(WALK_DIMS, false);
    let grid = walking_grid(&cells);
    let epoch_one = epoch(1);
    let epoch_two = epoch(2);
    let mut test = UnderTest::new(epoch_one, limits());
    test.confirm(
        &key(epoch_one, 1),
        &authority(1, 0, [0.5, 0.0, 0.5], [0.0, 0.0, 0.0], true, look(0.0, 0.0)),
        &grid,
    )
    .expect("begins");
    test.predict(1, control(0, 1, 0.0), &grid)
        .expect("journaled step in the first epoch");

    test.reset(epoch_two).expect("reset opens the second epoch");
    assert!(test.journal().is_empty(), "the journal is epoch-scoped");
    assert_eq!(
        test.snapshot().correction,
        None,
        "no correction crosses the reset"
    );

    let stale = authority(
        99,
        0,
        [9.0, 0.0, 9.0],
        [0.0, 0.0, 0.0],
        true,
        look(0.0, 0.0),
    );
    let outcome = test.confirm(&key(epoch_one, 2), &stale, &grid);
    assert_eq!(
        outcome,
        Err(ClientError::StaleEpoch),
        "an old-epoch correction rejects"
    );
    assert!(
        test.snapshot().confirmed_position.is_none(),
        "the old-epoch correction changed nothing"
    );

    // The new epoch's sequence space restarted at one.
    test.confirm(
        &key(epoch_two, 1),
        &authority(
            100,
            0,
            [0.5, 0.0, 0.5],
            [0.0, 0.0, 0.0],
            true,
            look(0.0, 0.0),
        ),
        &grid,
    )
    .expect("the second epoch begins from its own authority");
    test.predict(1, control(0, 1, 0.0), &grid)
        .expect("the restarted sequence space journals again");
    assert_eq!(test.journal(), vec![1]);
}

/// The journal bound admits exactly the frozen 256 pending entries and
/// rejects the next one with a typed capacity error before any mutation; a
/// correction that acknowledges the journal drains it and admission resumes.
#[test]
fn journal_bound_admits_256_rejects_257_typed() {
    let cells = fixture_cells(WALK_DIMS, false);
    let grid = walking_grid(&cells);
    let epoch_one = epoch(1);
    let mut test = UnderTest::new(epoch_one, limits());
    test.confirm(
        &key(epoch_one, 1),
        &authority(1, 0, [0.5, 0.0, 0.5], [0.0, 0.0, 0.0], true, look(0.0, 0.0)),
        &grid,
    )
    .expect("begins");
    // A neutral control keeps the body on its block, so only the bound is
    // under test here, never fixture geometry.
    let neutral = control(0, 0, 0.0);

    let mut sequence = 0u64;
    for frame in 0..52 {
        test.begin_frame();
        let steps = if frame < 51 { 5 } else { 1 };
        for _ in 0..steps {
            sequence += 1;
            test.predict(sequence, neutral, &grid)
                .expect("journal entry under the frozen ceiling");
        }
    }
    assert_eq!(
        test.journal().len(),
        limits().prediction_journal(),
        "exactly the frozen ceiling is admitted"
    );

    let before = test.snapshot();
    test.begin_frame();
    sequence += 1;
    assert_eq!(
        test.predict(sequence, neutral, &grid),
        Err(ClientError::Capacity),
        "the ceiling-plus-one step rejects typed"
    );
    assert_eq!(test.snapshot(), before, "the refused step mutated nothing");

    let drained = authority(
        2,
        256,
        [0.5, 0.0, 0.5],
        [0.0, 0.0, 0.0],
        true,
        look(0.0, 0.0),
    );
    test.confirm(&key(epoch_one, 2), &drained, &grid)
        .expect("the acknowledgement drains the journal");
    assert!(test.journal().is_empty());
    test.begin_frame();
    sequence += 1;
    test.predict(sequence, neutral, &grid)
        .expect("admission resumes after the drain");
}

/// The per-frame step budget admits five fixed steps and rejects the sixth
/// with a typed capacity error; the next frame's budget admits again.
#[test]
fn frame_bound_admits_five_steps_rejects_sixth_typed() {
    let cells = fixture_cells(WALK_DIMS, false);
    let grid = walking_grid(&cells);
    let epoch_one = epoch(1);
    let mut test = UnderTest::new(epoch_one, limits());
    test.confirm(
        &key(epoch_one, 1),
        &authority(1, 0, [0.5, 0.0, 0.5], [0.0, 0.0, 0.0], true, look(0.0, 0.0)),
        &grid,
    )
    .expect("begins");
    let jumping = held_control(0, 1, true);
    for sequence in 1..=5u64 {
        test.predict(sequence, jumping, &grid)
            .expect("step inside the per-frame budget");
    }
    let before = test.snapshot();
    assert_eq!(
        test.predict(6, jumping, &grid),
        Err(ClientError::Capacity),
        "the sixth step of one frame rejects"
    );
    assert_eq!(test.snapshot(), before, "the refused step mutated nothing");

    test.begin_frame();
    test.predict(6, jumping, &grid)
        .expect("the next frame's budget admits again");
}

/// The typed guards: out-of-range movement axes reject instead of clamping,
/// prediction before the first authority rejects, an acknowledgement of
/// never-issued input rejects, and a stale server tick is a no-op.
#[test]
fn ranged_controls_and_typed_acknowledgements_reject_before_mutation() {
    let cells = fixture_cells(WALK_DIMS, false);
    let grid = walking_grid(&cells);
    let epoch_one = epoch(1);

    let mut fresh = UnderTest::new(epoch_one, limits());
    assert_eq!(
        fresh.predict(1, control(0, 1, 0.0), &grid),
        Err(ClientError::InvalidState),
        "no prediction exists before the first authority"
    );

    let mut test = UnderTest::new(epoch_one, limits());
    test.confirm(
        &key(epoch_one, 1),
        &authority(1, 0, [0.5, 0.0, 0.5], [0.0, 0.0, 0.0], true, look(0.0, 0.0)),
        &grid,
    )
    .expect("begins");
    for (offset, axis) in [-1i8, 0, 1].into_iter().enumerate() {
        test.predict(
            u64::try_from(offset + 1).expect("sequence index"),
            control(axis, axis, 0.0),
            &grid,
        )
        .expect("every unit axis admits");
    }
    let before = test.snapshot();
    assert_eq!(
        test.predict(9, control(2, 0, 0.0), &grid),
        Err(ClientError::InvalidInput),
        "an out-of-range axis rejects"
    );
    assert_eq!(
        test.predict(9, control(0, -2, 0.0), &grid),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        test.snapshot(),
        before,
        "the refused controls mutated nothing"
    );

    let unsent = authority(
        2,
        99,
        [0.5, 0.0, 0.5],
        [0.0, 0.0, 0.0],
        true,
        look(0.0, 0.0),
    );
    assert_eq!(
        test.confirm(&key(epoch_one, 2), &unsent, &grid),
        Err(ClientError::InvalidInput),
        "an authority may not acknowledge unissued input"
    );
    assert_eq!(test.snapshot(), before);

    // A stale tick is a no-op, not an error: the delivered observation is
    // old news, never a rollback.
    let stale = authority(1, 0, [7.0, 0.0, 7.0], [0.0, 0.0, 0.0], true, look(0.0, 0.0));
    let stale_outcome = test.confirm(&key(epoch_one, 2), &stale, &grid);
    assert_eq!(stale_outcome.map(|_| ()), Ok(()));
    assert_eq!(test.snapshot(), before, "the stale tick changed nothing");
}

/// The projection the player-view family consumes carries the checked
/// confirmed and explicitly attributed predicted pose, the retained
/// correction, the finite look ray, the movement intent and the source
/// mining state of the authority.
#[test]
fn projection_carries_poses_correction_ray_movement_and_source_mining() {
    let cells = fixture_cells(WALK_DIMS, false);
    let grid = walking_grid(&cells);
    let epoch_one = epoch(1);
    let mut test = UnderTest::new(epoch_one, limits());
    let beginning = authority(
        1,
        0,
        [0.5, 0.0, 0.5],
        [0.0, 0.0, 0.0],
        true,
        look(0.1, -0.2),
    );
    test.confirm(&key(epoch_one, 1), &beginning, &grid)
        .expect("begins");
    let north = control(0, 1, 0.3);
    test.predict(1, north, &grid).expect("journaled step");
    test.predict(2, north, &grid)
        .expect("second journaled step, still unconfirmed");
    // The correction carries an active authoritative swing, so the source
    // mining state the projection reports is the latest authority's.
    let corrected = authority_with_mining(
        2,
        1,
        [0.6, 0.0, 0.4],
        [0.0, 0.0, 0.0],
        true,
        look(0.3, -0.25),
        active_mining(),
    );
    test.confirm(&key(epoch_one, 2), &corrected, &grid)
        .expect("correction");

    let snapshot = test.snapshot();
    let confirmed = snapshot.confirmed_position.expect("checked confirmed pose");
    assert_eq!(confirmed, [0.6f32, 0.0, 0.4f32].map(f64::from));
    let predicted = snapshot
        .predicted_position
        .expect("attributed predicted pose");
    let direct = fold(
        PhysicsState {
            position: [0.6, 0.0, 0.4],
            velocity: [0.0, 0.0, 0.0],
            on_ground: true,
        },
        &[north],
        &grid,
    );
    assert_same_position(predicted, direct.position);
    assert_eq!(
        snapshot.correction,
        Some((1, reason_tag(CorrectionReason::AcknowledgedReplay)))
    );
    let origin = snapshot.look_ray_origin.expect("finite look ray");
    assert_eq!(origin, predicted, "the ray starts at the attributed pose");
    assert_eq!(
        snapshot.look_reach,
        Some(6.0),
        "the authority interaction reach"
    );
    assert!(
        snapshot.movement_control_present,
        "the movement intent carries the replayed control"
    );
    assert_eq!(
        snapshot.source_mining,
        Some(active_mining()),
        "the source mining state stays the authoritative one"
    );
    assert_eq!(
        snapshot.projection_mining,
        Some(active_mining()),
        "the projection state itself carries the persisted source mining"
    );
    assert_eq!(
        snapshot.projection_mining, snapshot.source_mining,
        "the projection input role and the owner report one value"
    );
    assert_eq!(
        snapshot.predicted_on_ground,
        Some(true),
        "grounded replay stays grounded"
    );
}

/// The source mining seam the player projection reads: the projection
/// state's mining equals the confirmed base's mining, persists through every
/// non-authoritative operation — prediction steps, a rejection replay and a
/// duplicate delivery, none of which can flap a mid-swing idle — and is
/// replaced only by a newer authoritative confirmation. A reset drops it with
/// the confirmed base it rode in on.
#[test]
fn source_mining_persists_across_replay_and_updates_only_on_authority() {
    let cells = fixture_cells(WALK_DIMS, false);
    let grid = walking_grid(&cells);
    let epoch_one = epoch(1);
    let mut test = UnderTest::new(epoch_one, limits());

    let first_swing = active_mining();
    let beginning = authority_with_mining(
        1,
        0,
        [0.5, 0.0, 0.5],
        [0.0, 0.0, 0.0],
        true,
        look(0.0, 0.0),
        first_swing,
    );
    test.confirm(&key(epoch_one, 1), &beginning, &grid)
        .expect("begins");
    let north = control(0, 1, 0.0);
    test.predict(1, north, &grid).expect("journaled step");
    test.predict(2, north, &grid).expect("journaled step");
    assert_eq!(
        test.snapshot().projection_mining,
        Some(first_swing),
        "predicted steps never touch the source mining"
    );

    test.reject(1, &grid).expect("rejection replays");
    assert_eq!(
        test.snapshot().projection_mining,
        Some(first_swing),
        "a rejection is not an authority: the swing survives the replay"
    );
    test.confirm(&key(epoch_one, 1), &beginning, &grid)
        .expect("duplicate delivery is a no-op");
    assert_eq!(
        test.snapshot().projection_mining,
        Some(first_swing),
        "a duplicate acknowledgement never rewrites the source mining"
    );

    let second_swing = active_mining_at(BlockPos::new(5, 0, 7));
    let advanced = authority_with_mining(
        2,
        1,
        [0.5, 0.0, 0.4],
        [0.0, 0.0, 0.0],
        true,
        look(0.0, 0.0),
        second_swing,
    );
    test.confirm(&key(epoch_one, 2), &advanced, &grid)
        .expect("a newer authority replaces the swing");
    assert_eq!(
        test.snapshot().projection_mining,
        Some(second_swing),
        "the correction's own mining is the new source, persisted through its replay"
    );

    let idle = authority(3, 2, [0.5, 0.0, 0.3], [0.0, 0.0, 0.0], true, look(0.0, 0.0));
    test.confirm(&key(epoch_one, 3), &idle, &grid)
        .expect("an idle authority clears the swing");
    assert_eq!(
        test.snapshot().projection_mining,
        Some(MiningState::Idle),
        "the latest authority's idle mining is the source"
    );

    test.reset(epoch(2)).expect("reset drops the base");
    assert_eq!(
        test.snapshot(),
        Snapshot::empty(),
        "the mining leaves with the confirmed base it rode in on"
    );
}

/// The executable wrong-behavior artifact: the deliberately wrong double
/// really does acknowledge twice, keep rejected intent, apply foreign-epoch
/// corrections, silently clamp out-of-range axes and replay from the
/// predicted state — exactly what the assertions above refuse.
#[test]
fn wrong_double_replay_wrongness_is_executable() {
    let cells = fixture_cells(WALK_DIMS, false);
    let grid = walking_grid(&cells);
    let mut wrong = WrongReplayDouble::new();
    let begin = authority(1, 0, [0.5, 0.0, 0.5], [0.0, 0.0, 0.0], true, look(0.0, 0.0));
    wrong.confirm(&begin, &grid);
    let north = control(0, 1, 0.0);
    for sequence in 1..=3u64 {
        wrong.predict(sequence, north, &grid);
    }
    let corrected = authority(2, 1, [0.6, 0.0, 0.4], [0.0, 0.0, 0.0], true, look(0.0, 0.0));
    wrong.confirm(&corrected, &grid);
    let after_correction = wrong.predicted.position;
    wrong.confirm(&corrected, &grid);
    assert_ne!(
        wrong.predicted.position, after_correction,
        "the duplicate acknowledgement mutates the wrong double"
    );
    wrong.reject(2);
    assert_eq!(
        wrong.journal(),
        vec![2, 3],
        "the rejected intent survives the wrong double's journal"
    );
    wrong.reset();
    let clamped = wrong.predict(1, control(2, 0, 0.0), &grid);
    assert!(
        clamped.position[0] > 0.0,
        "the wrong double silently clamps an out-of-range axis instead of rejecting it"
    );
    let foreign = authority(
        99,
        0,
        [9.0, 0.0, 9.0],
        [0.0, 0.0, 0.0],
        true,
        look(0.0, 0.0),
    );
    wrong.confirm(&foreign, &grid);
    assert_eq!(
        wrong.confirmed.map(|state| state.position),
        Some([9.0, 0.0, 9.0]),
        "the wrong double applies a foreign-epoch correction"
    );
}
