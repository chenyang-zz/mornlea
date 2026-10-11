//! The prediction and correction seam's state owner and replay provider.
//!
//! The player prediction state is the immutable projection input the
//! `player-view` projection borrows: the checked confirmed pose, the
//! explicitly attributed predicted pose, the last correction, the finite
//! look ray, the movement intent and the accepted source mining state. It
//! carries no elapsed local time, because authoritative mining progress is
//! never reconstructed from it.
//!
//! `PredictionReplay` owns the replay half: an epoch-scoped journal of
//! pending predicted steps keyed by the admission owner's issued sequence,
//! acknowledge-once authoritative corrections, rejection removal, and the
//! replay of the remaining entries in sequence order from the latest
//! confirmed state. Every step — prediction and replay alike — runs through
//! the frozen F1 numerical kernel (`mornlea_engine`'s native physics step,
//! the accepted port of the Go integrator), never a locally re-derived
//! integrator: the sweep envelope supplied to the kernel is the caller half
//! of that frozen ABI, mirrored operation-for-operation from the accepted
//! rule. The world facts the ABI needs beside the control — the borrowed
//! collision grid and the body-in-fluid bit — come from the borrowed
//! confirmed world or the legacy caller-supplied grid. World views rebuild
//! bounded grids at each actual step and ray position, without owning a
//! second block mirror. Each step and replay walks the accepted engine
//! raycast kernel along the confirmed and predicted poses' look rays,
//! recording the first target cell each reaches, stamped with the confirmed base's revision; no grid is read
//! outside step time and no hit is ever invented.

use crate::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, ObservationKey, SessionEpoch,
};
use crate::presentation::{Correction, CorrectionReason, FiniteRay, MovementIntent, Pose};
use crate::session::WorldConfirmed;

mod world;
use mornlea_domain::{
    BlockPos, Dimension, LookAngles, MiningState, PlayerControl, PlayerControlParts, PlayerState,
};
use mornlea_engine::native::contracts::raycast::{Ray, RayCursor, RaycastOp};
use mornlea_engine::native::contracts::{
    CollisionGrid, KernelError, PhysicsControls, PhysicsRequest, PhysicsState, PhysicsTuning,
    SweepBounds,
};
use mornlea_engine::native::physics::step_physics;
use mornlea_engine::native::raycast::NativeRaycast;
use world::BorrowedEnvironment;

/// One accepted prediction journal observation, keyed by epoch and local
/// sequence. A rejected input removes its pending entry; an acknowledged one
/// confirms exactly once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JournalEntry {
    epoch: SessionEpoch,
    sequence: u64,
    acknowledged: bool,
}

impl JournalEntry {
    pub fn try_new(
        epoch: SessionEpoch,
        sequence: u64,
        acknowledged: bool,
    ) -> Result<Self, ClientError> {
        if sequence == 0 {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            epoch,
            sequence,
            acknowledged,
        })
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn acknowledged(&self) -> bool {
        self.acknowledged
    }
}

/// The checked player prediction state: the confirmed pose beside its
/// explicitly attributed predicted pose, the correction that caused the last
/// replay, the finite look ray, the movement intent and the source mining
/// state. The predicted pose is always tagged; a correction never relabels
/// it confirmed. The mining is the persisted source `MiningState` of the
/// latest confirmed authority: it survives observation-queue consumption and
/// every correction replay unchanged, and only an authoritative confirmation
/// — never a prediction, rejection or duplicate delivery — replaces it.
/// Beside each pose rides the recorded ray target of that pose's own look
/// ray, derived at step time against the grid the step borrows: the
/// confirmed target is confirmed data, the predicted target is explicitly
/// unconfirmed data whose attribution belongs to the consumer.
#[derive(Clone, Debug)]
pub struct PlayerProjectionState {
    epoch: SessionEpoch,
    revision: ConfirmedRevision,
    confirmed_pose: Pose,
    predicted_pose: Option<Pose>,
    journal: Vec<JournalEntry>,
    correction: Option<Correction>,
    look_ray: Option<FiniteRay>,
    movement: MovementIntent,
    mining: MiningState,
    confirmed_target: Option<RayTarget>,
    predicted_target: Option<RayTarget>,
    predicted_snow_cues: Vec<u64>,
    world_dimension: Option<Dimension>,
}

impl PlayerProjectionState {
    pub fn try_new(
        epoch: SessionEpoch,
        revision: ConfirmedRevision,
        confirmed_pose: Pose,
        predicted_pose: Option<Pose>,
        journal: Vec<JournalEntry>,
        correction: Option<Correction>,
        look_ray: Option<FiniteRay>,
        movement: MovementIntent,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            epoch,
            revision,
            confirmed_pose,
            predicted_pose,
            journal,
            correction,
            look_ray,
            movement,
            // A directly constructed state carries the idle authority; the
            // projection owner publishes the persisted confirmed-base value
            // through `with_mining`, so no caller of the frozen constructor
            // changes behavior.
            mining: MiningState::Idle,
            confirmed_target: None,
            predicted_target: None,
            predicted_snow_cues: Vec::new(),
            world_dimension: None,
        })
    }

    /// Publishes the persisted source mining state alongside the rest of the
    /// projection. The replay owner sets it from its confirmed base; the
    /// value is a checked authority record, so there is nothing further to
    /// validate here.
    pub fn with_mining(mut self, mining: MiningState) -> Self {
        self.mining = mining;
        self
    }

    /// Names the confirmed world's actual dimension for derived block readers.
    pub fn with_world_dimension(mut self, dimension: Dimension) -> Self {
        self.world_dimension = Some(dimension);
        self
    }

    pub fn world_dimension(&self) -> Option<Dimension> {
        self.world_dimension
    }

    /// Publishes the recorded target of the confirmed pose's look ray. The
    /// replay owner derives it at step time against the grid the step
    /// borrows; a directly constructed state carries no hit.
    pub fn with_confirmed_target(mut self, target: Option<RayTarget>) -> Self {
        self.confirmed_target = target;
        self
    }

    /// Publishes the recorded target of the predicted pose's look ray. It is
    /// data only — the attribution of an unconfirmed target is the
    /// consumer's, never a claim of confirmation.
    pub fn with_predicted_target(mut self, target: Option<RayTarget>) -> Self {
        self.predicted_target = target;
        self
    }

    /// Attaches only cue facts attributed to still-pending prediction sources.
    pub fn with_predicted_snow_cues(mut self, cues: Vec<u64>) -> Result<Self, ClientError> {
        if cues.len() > ClientLimits::MAX_PREDICTION_JOURNAL {
            return Err(ClientError::Capacity);
        }
        for (index, sequence) in cues.iter().enumerate() {
            if *sequence == 0
                || cues[..index].contains(sequence)
                || !self.journal.iter().any(|entry| {
                    entry.epoch == self.epoch && entry.sequence == *sequence && !entry.acknowledged
                })
            {
                return Err(ClientError::InvalidInput);
            }
        }
        self.predicted_snow_cues = cues;
        Ok(self)
    }

    pub fn predicted_snow_cues(&self) -> &[u64] {
        &self.predicted_snow_cues
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn revision(&self) -> ConfirmedRevision {
        self.revision
    }

    pub fn confirmed_pose(&self) -> &Pose {
        &self.confirmed_pose
    }

    pub fn predicted_pose(&self) -> Option<&Pose> {
        self.predicted_pose.as_ref()
    }

    pub fn journal(&self) -> &[JournalEntry] {
        &self.journal
    }

    pub fn correction(&self) -> Option<&Correction> {
        self.correction.as_ref()
    }

    pub fn look_ray(&self) -> Option<&FiniteRay> {
        self.look_ray.as_ref()
    }

    pub fn movement(&self) -> &MovementIntent {
        &self.movement
    }

    /// The persisted source mining state of the latest confirmed authority.
    pub fn mining(&self) -> &MiningState {
        &self.mining
    }

    /// The recorded target of the confirmed pose's look ray, when the
    /// step-borrowed grid put a target cell on it.
    pub fn confirmed_target(&self) -> Option<&RayTarget> {
        self.confirmed_target.as_ref()
    }

    /// The recorded target of the predicted pose's own look ray, when the
    /// step-borrowed grid put a target cell on it. Unconfirmed data.
    pub fn predicted_target(&self) -> Option<&RayTarget> {
        self.predicted_target.as_ref()
    }
}

/// The frozen per-frame fixed-step ceiling: one rendered frame may consume at
/// most five predicted steps, the accepted Go predictor's frozen
/// `maxPredictionSteps`. The ceiling-plus-one step rejects with a typed
/// `Capacity` before any journal or state mutation.
pub const MAX_STEPS_PER_FRAME: u16 = 5;

/// The frozen F1 default tuning snapshot, transcribed from the accepted Go
/// `DefaultTunables` values the authoritative server and the accepted
/// predictor step with. No second tuning surface exists on this owner: every
/// prediction and replay step runs with exactly these values.
pub const F1_DEFAULT_TUNING: PhysicsTuning = PhysicsTuning {
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

/// The confirmed-hunger floor below which sprinting never boosts the target
/// speed. The accepted predictor gates its sprint bit by the mirrored hunger
/// with the same threshold as the authority, so a hungry client predicting a
/// sprint and the authority walking it can never diverge step after step.
const HUNGER_SPRINT_FLOOR: u8 = 6;

/// The per-call world facts the frozen F1 step ABI requires beside the
/// control: the borrowed collision grid around the body and the body-in-fluid
/// bit. Legacy grid callers supply these facts directly; the private world
/// adapter derives them from the confirmed mirror at each actual position.
#[derive(Clone, Copy)]
pub struct StepEnvironment<'a> {
    grid: CollisionGrid<'a>,
    body_in_fluid: bool,
}

impl<'a> StepEnvironment<'a> {
    pub fn new(grid: CollisionGrid<'a>, body_in_fluid: bool) -> Self {
        Self {
            grid,
            body_in_fluid,
        }
    }

    pub fn grid(&self) -> &CollisionGrid<'a> {
        &self.grid
    }

    pub fn body_in_fluid(&self) -> bool {
        self.body_in_fluid
    }
}

/// The outcome of one predicted fixed step: the sequence it attests, the
/// attributed predicted pose, the kernel state's velocity and ground bit, and
/// the frozen kernel's clip/step/unknown flags.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepWitness {
    sequence: u64,
    pose: Pose,
    velocity: [f32; 3],
    on_ground: bool,
    clipped: [bool; 3],
    used_step: bool,
    hit_unknown: bool,
}

impl StepWitness {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn pose(&self) -> &Pose {
        &self.pose
    }

    pub fn velocity(&self) -> [f32; 3] {
        self.velocity
    }

    pub fn on_ground(&self) -> bool {
        self.on_ground
    }

    pub fn clipped(&self) -> [bool; 3] {
        self.clipped
    }

    pub fn used_step(&self) -> bool {
        self.used_step
    }

    pub fn hit_unknown(&self) -> bool {
        self.hit_unknown
    }
}

/// The counts of one authoritative correction: how many pending predictions
/// the acknowledgement confirmed, how many remaining entries the replay
/// re-applied, and whether the correction was an authoritative reset that
/// cleared the journal instead of acknowledging it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CorrectionWitness {
    acknowledged: usize,
    replayed: usize,
    reset: bool,
}

impl CorrectionWitness {
    pub fn try_new(acknowledged: usize, replayed: usize, reset: bool) -> Result<Self, ClientError> {
        Ok(Self {
            acknowledged,
            replayed,
            reset,
        })
    }

    pub fn acknowledged(&self) -> usize {
        self.acknowledged
    }

    pub fn replayed(&self) -> usize {
        self.replayed
    }

    pub fn reset(&self) -> bool {
        self.reset
    }
}

/// The attribution key of one removed pending prediction: the epoch and input
/// sequence the audio family's predicted-cue dedup keys carry, so a rejected
/// input cancels its cue attribution exactly once. No cue id or device
/// concept exists on this owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RejectedPrediction {
    epoch: SessionEpoch,
    sequence: u64,
}

impl RejectedPrediction {
    pub fn try_new(epoch: SessionEpoch, sequence: u64) -> Result<Self, ClientError> {
        if sequence == 0 {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self { epoch, sequence })
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}

/// One journaled pending prediction: the sequence the admission owner issued
/// for the step beside the effective control actually stepped — the sprint
/// bit already gated by the confirmed hunger, exactly the input the accepted
/// predictor journals after gating — so replay reproduces the original step
/// bit-for-bit.
#[derive(Clone)]
struct PendingStep {
    sequence: u64,
    control: PlayerControl,
}

/// The latest confirmed authoritative base a replay starts from: the kernel
/// state, the authority's look, mining and hunger, the confirmed revision of
/// the observation that carried it, and whether the authority was ready.
#[derive(Clone)]
struct ConfirmedBase {
    revision: ConfirmedRevision,
    dimension: Dimension,
    state: PhysicsState,
    look: LookAngles,
    mining: MiningState,
    hunger: u8,
    ready: bool,
}

/// One recorded ray-target hit: the world cell the accepted raycast kernel
/// reached and the confirmed revision of the base whose borrowed grid the
/// hit was observed against. The position is a checked domain block
/// position; no face, distance or block identity is invented here, because
/// the projection seam consumes only the target cell and its source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayTarget {
    position: BlockPos,
    source_revision: ConfirmedRevision,
}

impl RayTarget {
    pub fn try_new(
        position: BlockPos,
        source_revision: ConfirmedRevision,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            position,
            source_revision,
        })
    }

    pub fn position(&self) -> BlockPos {
        self.position
    }

    pub fn source_revision(&self) -> ConfirmedRevision {
        self.source_revision
    }
}

/// Matches the presentation stride rule without contributing gameplay state.
#[derive(Clone, Copy, Default)]
struct SnowStride {
    position: Option<[f32; 3]>,
    travel: f32,
}

impl SnowStride {
    fn observe(&mut self, state: PhysicsState, foot_block: Option<u16>) -> bool {
        let previous = self.position.replace(state.position);
        let Some(previous) = previous else {
            return false;
        };
        let dx = state.position[0] - previous[0];
        let dz = state.position[2] - previous[2];
        let horizontal = f64::from(dx * dx + dz * dz).sqrt() as f32;
        if horizontal > 8.0 || !state.on_ground {
            return false;
        }
        self.travel += horizontal;
        if self.travel < 0.8 {
            return false;
        }
        self.travel = 0.0;
        foot_block.is_some_and(|block| (85..=88).contains(&block))
    }
}

/// The prediction journal and authoritative correction replay owner.
///
/// The journal is this owner's replay state by epoch and sequence — the
/// admission owner's accepted-input journal stays read-only to it and the
/// caller wires each admitted sequence to `predict_step`. Acknowledgement is
/// idempotent-once: a re-delivered observation with the same server tick is
/// a no-op, and a stale tick never rolls anything back. A rejection removes
/// its pending prediction and publishes the removed attribution key. Every
/// mutation of the journal — acknowledgement, rejection, reset — rebuilds
/// the attributed predicted pose by replaying the remaining entries in
/// sequence order from the latest confirmed state through the frozen kernel.
/// A correction from another epoch rejects with `StaleEpoch` before any
/// state changes, so an old epoch can never affect a new one.
#[derive(Clone)]
pub struct PredictionReplay {
    epoch: SessionEpoch,
    limits: ClientLimits,
    confirmed: Option<ConfirmedBase>,
    predicted: Option<PhysicsState>,
    pending: Vec<PendingStep>,
    issued_high_water: u64,
    admitted_high_water: u64,
    last_acknowledged: u64,
    last_server_tick: u64,
    correction: Option<Correction>,
    frame_steps: u16,
    confirmed_target: Option<RayTarget>,
    predicted_target: Option<RayTarget>,
    snow_stride: SnowStride,
    snow_cues: Vec<u64>,
    snow_sampled: bool,
}

impl PredictionReplay {
    /// Publishes the empty owner for one epoch under the frozen limits. The
    /// sequence space starts empty: nothing is issued or acknowledged until
    /// the first authority begins prediction.
    pub fn try_new(epoch: SessionEpoch, limits: ClientLimits) -> Result<Self, ClientError> {
        Ok(Self {
            epoch,
            limits,
            confirmed: None,
            predicted: None,
            pending: Vec::new(),
            issued_high_water: 0,
            admitted_high_water: 0,
            last_acknowledged: 0,
            last_server_tick: 0,
            correction: None,
            frame_steps: 0,
            confirmed_target: None,
            predicted_target: None,
            snow_stride: SnowStride::default(),
            snow_cues: Vec::new(),
            snow_sampled: false,
        })
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    /// The highest input sequence this epoch has acknowledged. Everything at
    /// or below it left the journal through confirmation, not rejection.
    pub fn last_acknowledged_sequence(&self) -> u64 {
        self.last_acknowledged
    }

    /// The source mining state of the latest confirmed authority. It is a
    /// purely authoritative value this owner mirrors and never reconstructs
    /// from elapsed local time.
    pub fn source_mining(&self) -> Option<MiningState> {
        self.confirmed.as_ref().map(|base| base.mining)
    }

    /// The recorded target of the confirmed pose's look ray at the latest
    /// step-time derivation. Unlike the projection field, this reads even
    /// while no confirmed base exists, so an emptied owner reports its
    /// emptiness directly.
    pub fn confirmed_target(&self) -> Option<RayTarget> {
        self.confirmed_target
    }

    /// The recorded target of the predicted pose's own look ray at the
    /// latest step-time derivation, readable the same way.
    pub fn predicted_target(&self) -> Option<RayTarget> {
        self.predicted_target
    }

    /// The immutable projection input for the `player-view` family: the
    /// checked confirmed pose beside the explicitly attributed predicted
    /// pose, the retained correction, the finite look ray, the movement
    /// intent and the journal of still-pending predictions. Before the first
    /// confirmed authority nothing can be projected.
    pub fn projection(&self) -> Result<PlayerProjectionState, ClientError> {
        let base = self.confirmed.as_ref().ok_or(ClientError::InvalidState)?;
        let confirmed_pose = Pose::try_new(
            base.state.position.map(f64::from),
            f64::from(base.look.yaw()),
            f64::from(base.look.pitch()),
        )?;
        let last_control = self.pending.last().map(|step| step.control);
        // The predicted pose exists only while unconfirmed steps are
        // attributed; a correction that acknowledges everything relabels
        // nothing and leaves the confirmed pose alone.
        let predicted_pose = match (self.predicted, last_control) {
            (Some(state), Some(control)) => Some(Pose::try_new(
                state.position.map(f64::from),
                f64::from(control.look().yaw()),
                f64::from(control.look().pitch()),
            )?),
            _ => None,
        };
        let journal = self
            .pending
            .iter()
            .map(|step| JournalEntry::try_new(self.epoch, step.sequence, false))
            .collect::<Result<Vec<_>, _>>()?;
        let position = self
            .predicted
            .map(|state| state.position)
            .unwrap_or(base.state.position)
            .map(f64::from);
        let look = last_control
            .map(|control| control.look())
            .unwrap_or(base.look);
        let look_ray = FiniteRay::try_new(position, look, FiniteRay::MAX_REACH)?;
        let on_ground = self
            .predicted
            .map(|state| state.on_ground)
            .unwrap_or(base.state.on_ground);
        let movement = MovementIntent::try_new(last_control, on_ground)?;
        PlayerProjectionState::try_new(
            self.epoch,
            base.revision,
            confirmed_pose,
            predicted_pose,
            journal,
            self.correction,
            Some(look_ray),
            movement,
        )?
        // The persisted source mining of the confirmed base rides beside the
        // poses so the projection input role carries it; the owner-level
        // accessor reports the same value.
        .with_mining(base.mining)
        .with_world_dimension(base.dimension)
        // The step-time recorded ray targets ride beside their poses. The
        // re-derivation happened inside the step or replay that borrowed the
        // grid; reporting here is pure, so a repeated projection is equal.
        .with_confirmed_target(self.confirmed_target)
        .with_predicted_target(self.predicted_target)
        .with_predicted_snow_cues(self.snow_cues.clone())
    }

    /// Opens the next rendered frame's step budget. The per-frame ceiling
    /// exists so one long frame cannot journal an unbounded burst of steps;
    /// the ceiling-plus-one step rejects typed until this runs.
    pub fn begin_frame(&mut self) {
        self.frame_steps = 0;
        self.snow_sampled = false;
    }

    /// Samples snow feedback after the final physics step of a rendered frame.
    pub fn observe_snow_step(&mut self, foot_block: Option<u16>) -> Result<(), ClientError> {
        if self.snow_sampled {
            return Ok(());
        }
        let Some(base) = self.confirmed.as_ref().filter(|base| base.ready) else {
            self.snow_sampled = true;
            return Ok(());
        };
        let mut stride = self.snow_stride;
        let sounds = stride.observe(self.predicted.unwrap_or(base.state), foot_block);
        let sequence = self.pending.last().map(|step| step.sequence);
        let cue = sequence.filter(|sequence| sounds && !self.snow_cues.contains(sequence));
        if cue.is_some()
            && (self.snow_cues.len() >= self.limits.queued_input_events()
                || self.snow_cues.len() >= self.limits.prediction_journal())
        {
            return Err(ClientError::Capacity);
        }
        self.snow_stride = stride;
        self.snow_sampled = true;
        if let Some(sequence) = cue {
            self.snow_cues.push(sequence);
        }
        Ok(())
    }

    /// Consumes only an admitted FIFO prefix after successful publication.
    pub fn consume_snow_cues(&mut self, count: usize) -> Result<(), ClientError> {
        if count > self.snow_cues.len() {
            return Err(ClientError::InvalidInput);
        }
        self.snow_cues.drain(..count);
        Ok(())
    }

    /// Journals and steps one predicted fixed step under the sequence the
    /// admission owner issued for it.
    ///
    /// Every bound and range rejects before any mutation: the movement axes
    /// must be the frozen kernel's `-1..=1` domain, the sequence must be
    /// nonzero and strictly newer than anything issued this epoch, the frame
    /// budget and the frozen journal ceiling must admit one more step, and a
    /// ready confirmed authority must exist to step from. The step itself is
    /// the frozen F1 kernel's; the sprint bit is gated by the confirmed
    /// hunger before journaling so the replayed input is exactly the input
    /// that was stepped.
    pub fn predict_step(
        &mut self,
        sequence: u64,
        control: PlayerControl,
        environment: &StepEnvironment<'_>,
    ) -> Result<StepWitness, ClientError> {
        let mut candidate = self.clone();
        candidate.admit_through(candidate.admitted_high_water.max(sequence))?;
        let witness = candidate.predict_candidate(
            sequence,
            control,
            BorrowedEnvironment::Grid(*environment),
        )?;
        *self = candidate;
        Ok(witness)
    }

    /// Tracks real input admission independently of movement; zero and equal
    /// marks are valid, while a decrease leaves admission unchanged.
    pub(crate) fn admit_through(&mut self, sequence: u64) -> Result<(), ClientError> {
        if sequence < self.admitted_high_water {
            return Err(ClientError::InvalidInput);
        }
        self.admitted_high_water = sequence;
        Ok(())
    }

    /// Stages world reads and numerical results before advancing prediction ownership.
    pub(crate) fn predict_world(
        &mut self,
        sequence: u64,
        control: PlayerControl,
        world: &WorldConfirmed,
        dimension: Dimension,
    ) -> Result<StepWitness, ClientError> {
        let mut candidate = self.clone();
        let witness = candidate.predict_candidate(
            sequence,
            control,
            BorrowedEnvironment::World(world, dimension),
        )?;
        *self = candidate;
        Ok(witness)
    }

    fn predict_candidate(
        &mut self,
        sequence: u64,
        control: PlayerControl,
        environment: BorrowedEnvironment<'_>,
    ) -> Result<StepWitness, ClientError> {
        let base = self
            .confirmed
            .as_ref()
            .filter(|base| base.ready)
            .ok_or(ClientError::InvalidState)?;
        let movement = control.movement();
        if !(-1..=1).contains(&movement.move_x) || !(-1..=1).contains(&movement.move_z) {
            return Err(ClientError::InvalidInput);
        }
        if sequence == 0
            || sequence <= self.issued_high_water
            || sequence <= self.last_acknowledged
            || sequence > self.admitted_high_water
        {
            return Err(ClientError::InvalidInput);
        }
        if self.frame_steps >= MAX_STEPS_PER_FRAME {
            return Err(ClientError::Capacity);
        }
        if self.pending.len() + 1 > self.limits.prediction_journal() {
            return Err(ClientError::Capacity);
        }
        let effective = effective_control(control, base.hunger);
        let current = self.predicted.unwrap_or(base.state);
        let (state, clipped, used_step, hit_unknown) = environment.step(current, &effective)?;
        let look = effective.look();
        let pose = Pose::try_new(
            state.position.map(f64::from),
            f64::from(look.yaw()),
            f64::from(look.pitch()),
        )?;
        let witness = StepWitness {
            sequence,
            pose,
            velocity: state.velocity,
            on_ground: state.on_ground,
            clipped,
            used_step,
            hit_unknown,
        };
        self.pending.push(PendingStep {
            sequence,
            control: effective,
        });
        self.issued_high_water = sequence;
        self.frame_steps += 1;
        self.predicted = Some(state);
        // Both targets must validate before this staged owner replaces live state.
        self.refresh_ray_targets(environment, None)?;
        Ok(witness)
    }

    /// Applies one complete authoritative player observation and replays the
    /// still-unconfirmed journal from it.
    ///
    /// A foreign epoch rejects with `StaleEpoch` before anything changes. An
    /// observation at or below the latest seen server tick is a no-op: the
    /// acknowledgement already happened exactly once and a stale tick never
    /// rolls state back. An acknowledgement naming input this epoch never
    /// issued rejects as malformed. A reset observation — or the first ready
    /// one — clears the journal and re-begins at the authority instead of
    /// acknowledging; a not-ready one clears every prediction and leaves
    /// nothing attributable until the next ready observation. Otherwise the
    /// acknowledged prefix leaves the journal, the confirmed base is adopted,
    /// and every remaining entry replays in sequence order from that base
    /// through the frozen kernel.
    pub fn apply_confirmed(
        &mut self,
        key: &ObservationKey,
        state: &PlayerState,
        environment: &StepEnvironment<'_>,
    ) -> Result<CorrectionWitness, ClientError> {
        // Replay and ray validation must succeed before replacing the live owner.
        let mut candidate = self.clone();
        let witness = candidate.apply_confirmed_candidate(
            key,
            state,
            BorrowedEnvironment::Grid(*environment),
        )?;
        *self = candidate;
        Ok(witness)
    }

    /// Corrections read the authority's actual dimension, including Depths.
    pub(crate) fn apply_world(
        &mut self,
        key: &ObservationKey,
        state: &PlayerState,
        world: &WorldConfirmed,
    ) -> Result<CorrectionWitness, ClientError> {
        let mut candidate = self.clone();
        let witness = candidate.apply_confirmed_candidate(
            key,
            state,
            BorrowedEnvironment::World(world, state.dimension()),
        )?;
        *self = candidate;
        Ok(witness)
    }

    /// World changes recast targets at the current mirror revision without
    /// adopting a new authoritative player pose or correction base.
    pub(crate) fn refresh_world_targets(
        &mut self,
        world: &WorldConfirmed,
        dimension: Dimension,
        revision: ConfirmedRevision,
    ) -> Result<(), ClientError> {
        if self
            .confirmed
            .as_ref()
            .is_some_and(|base| revision < base.revision)
        {
            return Err(ClientError::InvalidInput);
        }
        let mut candidate = self.clone();
        candidate
            .refresh_ray_targets(BorrowedEnvironment::World(world, dimension), Some(revision))?;
        *self = candidate;
        Ok(())
    }

    fn apply_confirmed_candidate(
        &mut self,
        key: &ObservationKey,
        state: &PlayerState,
        environment: BorrowedEnvironment<'_>,
    ) -> Result<CorrectionWitness, ClientError> {
        if key.epoch() != self.epoch {
            return Err(ClientError::StaleEpoch);
        }
        if state.server_tick() <= self.last_server_tick {
            // The duplicate-acknowledgement no-op: the same observation is
            // old news, never a second acknowledgement or a rollback.
            return CorrectionWitness::try_new(0, 0, false);
        }
        if matches!(environment, BorrowedEnvironment::Grid(_))
            && state.dimension() != Dimension::OVERWORLD
        {
            return Err(ClientError::InvalidInput);
        }
        let last = state.last_input_sequence();
        if last > self.admitted_high_water {
            // The authority may not confirm input this epoch never issued.
            return Err(ClientError::InvalidInput);
        }
        let motion = state.motion();
        let confirmed_state = PhysicsState {
            position: motion.position().get(),
            velocity: motion.velocity().get(),
            on_ground: motion.on_ground(),
        };
        let base = ConfirmedBase {
            revision: key.confirmed_revision(),
            dimension: state.dimension(),
            state: confirmed_state,
            look: state.look(),
            mining: state.mining(),
            hunger: state.survival().hunger(),
            ready: state.ready(),
        };
        self.last_server_tick = state.server_tick();
        if !state.ready() {
            // A not-ready authority clears every prediction and publishes no
            // correction: nothing is attributable until readiness returns.
            self.confirmed = Some(base);
            self.pending.clear();
            self.predicted = None;
            self.correction = None;
            self.clear_snow();
            self.issued_high_water = last;
            self.last_acknowledged = self.last_acknowledged.max(last);
            self.refresh_ray_targets(environment, None)?;
            return CorrectionWitness::try_new(0, 0, false);
        }
        let reset = self
            .confirmed
            .as_ref()
            .is_none_or(|previous| !previous.ready)
            || state.reset();
        let acknowledged = if reset {
            0
        } else {
            self.pending
                .iter()
                .filter(|step| step.sequence <= last)
                .count()
        };
        if reset {
            // A reset restarts the epoch's sequence accounting at the
            // authority's own acknowledgement floor, exactly as the accepted
            // predictor's begin does.
            self.pending.clear();
            self.clear_snow();
            self.issued_high_water = last;
            self.last_acknowledged = last;
        } else {
            self.pending.retain(|step| step.sequence > last);
            self.last_acknowledged = self.last_acknowledged.max(last);
        }
        self.retain_snow_sources();
        self.confirmed = Some(base);
        self.replay(environment)?;
        self.correction = Some(Correction::try_new(
            last,
            if reset {
                CorrectionReason::AuthoritativeReset
            } else {
                CorrectionReason::AcknowledgedReplay
            },
        )?);
        CorrectionWitness::try_new(acknowledged, self.pending.len(), reset)
    }

    /// Removes one rejected input's pending prediction, publishes its cue
    /// attribution key and replays the remaining entries from the latest
    /// confirmed state in sequence order. A sequence with no pending
    /// prediction has nothing to remove and changes nothing.
    pub fn reject(
        &mut self,
        sequence: u64,
        environment: &StepEnvironment<'_>,
    ) -> Result<Option<RejectedPrediction>, ClientError> {
        // Replay and ray validation must succeed before replacing the live owner.
        let mut candidate = self.clone();
        let witness =
            candidate.reject_candidate(sequence, BorrowedEnvironment::Grid(*environment))?;
        *self = candidate;
        Ok(witness)
    }

    /// Rejection replays only remaining movement against the confirmed world.
    pub(crate) fn reject_world(
        &mut self,
        sequence: u64,
        world: &WorldConfirmed,
        dimension: Dimension,
    ) -> Result<Option<RejectedPrediction>, ClientError> {
        let mut candidate = self.clone();
        let witness =
            candidate.reject_candidate(sequence, BorrowedEnvironment::World(world, dimension))?;
        *self = candidate;
        Ok(witness)
    }

    fn reject_candidate(
        &mut self,
        sequence: u64,
        environment: BorrowedEnvironment<'_>,
    ) -> Result<Option<RejectedPrediction>, ClientError> {
        if self.pending.iter().all(|step| step.sequence != sequence) {
            return Ok(None);
        }
        self.pending.retain(|step| step.sequence != sequence);
        self.retain_snow_sources();
        self.replay(environment)?;
        self.correction = Some(Correction::try_new(
            sequence,
            CorrectionReason::RejectedInput,
        )?);
        RejectedPrediction::try_new(self.epoch, sequence).map(Some)
    }

    /// Opens a new epoch: every epoch-scoped owner — journal, sequence
    /// accounting, confirmed base, predicted pose, correction, tick gate and
    /// both recorded ray targets — is cleared, so neither an old correction
    /// nor an old sequence can affect the new epoch.
    pub fn reset_epoch(&mut self, epoch: SessionEpoch) -> Result<(), ClientError> {
        self.epoch = epoch;
        self.confirmed = None;
        self.predicted = None;
        self.pending.clear();
        self.issued_high_water = 0;
        self.admitted_high_water = 0;
        self.last_acknowledged = 0;
        self.last_server_tick = 0;
        self.correction = None;
        self.frame_steps = 0;
        self.confirmed_target = None;
        self.predicted_target = None;
        self.clear_snow();
        Ok(())
    }

    fn clear_snow(&mut self) {
        self.snow_stride = SnowStride::default();
        self.snow_cues.clear();
        self.snow_sampled = false;
    }

    fn retain_snow_sources(&mut self) {
        self.snow_cues
            .retain(|sequence| self.pending.iter().any(|step| step.sequence == *sequence));
    }

    /// Rebuilds the attributed predicted state: the frozen kernel folds the
    /// remaining pending entries in sequence order starting from the latest
    /// confirmed base — never from the pre-correction prediction. With no
    /// pending entries nothing is attributable and the confirmed base is the
    /// whole projection.
    fn replay(&mut self, environment: BorrowedEnvironment<'_>) -> Result<(), ClientError> {
        let Some(base) = self.confirmed.as_ref() else {
            return Ok(());
        };
        let mut state = base.state;
        for step in &self.pending {
            state = environment.step(state, &step.control)?.0;
        }
        self.predicted = (!self.pending.is_empty()).then_some(state);
        self.refresh_ray_targets(environment, None)?;
        Ok(())
    }

    /// Re-derives both recorded ray targets against the grid the current
    /// step or replay borrowed: the confirmed pose's look ray and the
    /// predicted pose's own look ray, each capped by the landed interaction
    /// reach, each hit stamped with the confirmed base's revision. No hit
    /// within the borrowed view records `None`; the walk stops at the first
    /// cell the grid does not cover or does not load, because nothing beyond
    /// that view's data may be claimed. The confirmed target is confirmed
    /// data; the predicted target is data only.
    fn refresh_ray_targets(
        &mut self,
        environment: BorrowedEnvironment<'_>,
        revision: Option<ConfirmedRevision>,
    ) -> Result<(), ClientError> {
        let Some(base) = self.confirmed.as_ref() else {
            self.confirmed_target = None;
            self.predicted_target = None;
            return Ok(());
        };
        let revision = revision.unwrap_or(base.revision);
        self.confirmed_target = environment.target(base.state.position, base.look, revision)?;
        self.predicted_target = match (self.predicted, self.pending.last()) {
            (Some(state), Some(step)) => {
                environment.target(state.position, step.control.look(), revision)?
            }
            _ => None,
        };
        Ok(())
    }
}

/// Gates the sprint bit by the confirmed hunger and sneak precedence before
/// the control is stepped and journaled, mirroring the accepted predictor's
/// intake shaping: below the hunger floor no sprint boost is predicted, and
/// a sneaking step never sprints.
fn effective_control(control: PlayerControl, hunger: u8) -> PlayerControl {
    let mut actions = control.actions();
    if hunger < HUNGER_SPRINT_FLOOR {
        actions.sprinting = false;
    }
    if actions.sneaking {
        actions.sprinting = false;
    }
    PlayerControl::new(PlayerControlParts {
        movement: control.movement(),
        look: control.look(),
        actions,
    })
}

/// One fixed step through the frozen F1 kernel: the native physics step whose
/// integration and collision resolution are the accepted port of the Go
/// implementation. The sweep envelope is the caller half of that frozen ABI,
/// computed by the mirrored rule below; the kernel itself validates the
/// integrated displacement against it and rejects any nonfinite derived
/// state, so no local integrator or second epsilon exists on this path.
fn kernel_step(
    state: PhysicsState,
    control: &PlayerControl,
    environment: &StepEnvironment<'_>,
) -> Result<(PhysicsState, [bool; 3], bool, bool), ClientError> {
    let controls = physics_controls(control, environment.body_in_fluid());
    let request = PhysicsRequest {
        state,
        controls,
        tuning: F1_DEFAULT_TUNING,
        sweep: sweep_bounds(state, &controls),
        grid: *environment.grid(),
    };
    let result = step_physics(&request).map_err(|error| match error {
        KernelError::InvalidInput => ClientError::InvalidInput,
        _ => ClientError::Internal,
    })?;
    Ok((
        result.state,
        result.clipped,
        result.used_step,
        result.hit_unknown,
    ))
}

fn physics_controls(control: &PlayerControl, body_in_fluid: bool) -> PhysicsControls {
    let look = control.look();
    let yaw = f64::from(look.yaw());
    PhysicsControls {
        move_x: control.movement().move_x,
        move_z: control.movement().move_z,
        jump: control.movement().jump,
        yaw_sin: yaw.sin() as f32,
        yaw_cos: yaw.cos() as f32,
        body_in_fluid,
        sprinting: control.actions().sprinting,
        sneaking: control.actions().sneaking,
    }
}

/// The convex displacement envelope the frozen step ABI requires the caller
/// to supply, mirrored operation-for-operation from the accepted rule the Go
/// caller computes and the kernel checks against: the horizontal hull over
/// the current and accelerated velocity under the sneak/sprint-gated target,
/// and the vertical hull of the fluid-ascend, jump or gravity branch.
fn sweep_bounds(state: PhysicsState, controls: &PhysicsControls) -> SweepBounds {
    let tuning = &F1_DEFAULT_TUNING;
    let dt = tuning.fixed_delta_seconds;
    let sneaking_grounded = controls.sneaking && state.on_ground && !controls.body_in_fluid;
    let effective_walk = if sneaking_grounded {
        tuning.walk_speed * tuning.sneak_speed_multiplier
    } else if controls.sprinting
        && controls.move_z > 0
        && state.on_ground
        && !controls.body_in_fluid
    {
        tuning.walk_speed * tuning.sprint_speed_multiplier
    } else {
        tuning.walk_speed
    };
    let target = movement_target(
        controls.move_x,
        controls.move_z,
        effective_walk,
        controls.yaw_sin,
        controls.yaw_cos,
    );
    let mut horizontal = [state.velocity[0], 0.0, state.velocity[2]];
    if state.on_ground {
        if vec3_len(target) == 0.0 {
            horizontal = move_toward(horizontal, [0.0; 3], tuning.ground_deceleration * dt);
        } else {
            horizontal = move_toward(horizontal, target, tuning.ground_acceleration * dt);
        }
    } else {
        horizontal = move_toward(horizontal, target, tuning.air_acceleration * dt);
        let length = vec3_len(horizontal);
        if length > tuning.walk_speed {
            let inverse = 1.0 / length;
            horizontal = [
                horizontal[0] * inverse * tuning.walk_speed,
                horizontal[1] * inverse * tuning.walk_speed,
                horizontal[2] * inverse * tuning.walk_speed,
            ];
        }
    }
    if controls.body_in_fluid {
        let drag = tuning.fluid_horizontal_drag;
        horizontal = [
            horizontal[0] * drag,
            horizontal[1] * drag,
            horizontal[2] * drag,
        ];
    }
    let mut minimum = [0.0f32; 3];
    let mut maximum = [0.0f32; 3];
    minimum[0] = min3(0.0, state.velocity[0], horizontal[0]) * dt;
    maximum[0] = max3(0.0, state.velocity[0], horizontal[0]) * dt;
    minimum[2] = min3(0.0, state.velocity[2], horizontal[2]) * dt;
    maximum[2] = max3(0.0, state.velocity[2], horizontal[2]) * dt;
    let ascending = controls.body_in_fluid && controls.jump;
    if ascending {
        let rise = tuning.fluid_ascend_speed * dt;
        minimum[1] = 0.0f32.min(rise);
        maximum[1] = 0.0f32.max(rise);
    } else if state.on_ground && controls.jump {
        // The jump branch bounds only the rise; the fall side stays at zero.
        maximum[1] = tuning.jump_speed * dt;
    } else {
        let (gravity, terminal) = if controls.body_in_fluid {
            (tuning.fluid_gravity, tuning.fluid_sink_speed)
        } else {
            (tuning.gravity, tuning.terminal_fall_speed)
        };
        let fallen = state.velocity[1] - gravity * dt;
        let bound = if fallen >= -terminal {
            fallen
        } else {
            -terminal
        };
        minimum[1] = min3(0.0, state.velocity[1], bound) * dt;
        maximum[1] = max3(0.0, state.velocity[1], bound) * dt;
    }
    SweepBounds { minimum, maximum }
}

/// The fused vector length of the frozen kernel: the squared components
/// accumulate through one fused multiply-add chain and take one wide sqrt.
fn vec3_len(v: [f32; 3]) -> f32 {
    let sum = v[2].mul_add(v[2], v[0].mul_add(v[0], v[1] * v[1]));
    ((sum as f64).sqrt()) as f32
}

/// Moves one vector toward a target by at most the given delta: at or inside
/// the delta the target itself is reached, never overshot.
fn move_toward(current: [f32; 3], target: [f32; 3], maximum_delta: f32) -> [f32; 3] {
    let delta = [
        target[0] - current[0],
        target[1] - current[1],
        target[2] - current[2],
    ];
    let length = vec3_len(delta);
    if length <= maximum_delta {
        return target;
    }
    let scale = maximum_delta / length;
    [
        current[0] + delta[0] * scale,
        current[1] + delta[1] * scale,
        current[2] + delta[2] * scale,
    ]
}

/// The yaw-relative walking target: the normalized intent of the raw axes
/// scaled by the effective walk speed, zero when the axes cancel.
fn movement_target(
    move_x: i8,
    move_z: i8,
    walk_speed: f32,
    yaw_sin: f32,
    yaw_cos: f32,
) -> [f32; 3] {
    let forward = [-yaw_sin, 0.0, -yaw_cos];
    let right = [yaw_cos, 0.0, -yaw_sin];
    let intent = [
        right[0] * move_x as f32 + forward[0] * move_z as f32,
        right[1] * move_x as f32 + forward[1] * move_z as f32,
        right[2] * move_x as f32 + forward[2] * move_z as f32,
    ];
    if vec3_len(intent) == 0.0 {
        return [0.0; 3];
    }
    let inverse = 1.0 / vec3_len(intent);
    [
        intent[0] * inverse * walk_speed,
        intent[1] * inverse * walk_speed,
        intent[2] * inverse * walk_speed,
    ]
}

fn min3(first: f32, second: f32, third: f32) -> f32 {
    first.min(second).min(third)
}

fn max3(first: f32, second: f32, third: f32) -> f32 {
    first.max(second).max(third)
}

/// Walks the accepted engine raycast kernel (`NativeRaycast`, the landed
/// `RaycastOp` implementation whose DDA the authority's own target walks
/// drive) along one pose's look ray against the step-borrowed grid, and
/// records the first target cell it reaches.
///
/// The walk mirrors the accepted target-walk shape: a checked cursor over
/// the normalized look direction, batch-by-batch kernel batches, every
/// traversed cell classified against the only world data the step borrowed —
/// a covered, loaded cell carrying at least one collision box is a target
/// cell, a covered, loaded empty cell passes over, and the first cell the
/// grid does not cover — outside its extent or not loaded — ends the walk
/// with no hit, because nothing beyond the borrowed view may be claimed. The
/// reach is the landed authority interaction
/// distance, so the recorded hit is a hit of exactly the ray the projection
/// publishes. The bounded DDA crosses at most `3 x reach + 2` cells inside
/// one 64-record kernel batch, so the walk is one bounded kernel call.
fn ray_target(
    origin: [f32; 3],
    look: LookAngles,
    source_revision: ConfirmedRevision,
    grid: &CollisionGrid<'_>,
) -> Result<Option<RayTarget>, ClientError> {
    let direction = look_direction(look.yaw(), look.pitch());
    let Some(direction) = normalized_direction(direction) else {
        // No normalizable direction means no ray to walk; recording no hit
        // invents nothing.
        return Ok(None);
    };
    let mut cursor = RayCursor::try_new(Ray {
        origin,
        direction,
        maximum: FiniteRay::MAX_REACH,
    })
    .map_err(|_| ClientError::Internal)?;
    loop {
        let batch = NativeRaycast
            .next_batch(&mut cursor)
            .map_err(|_| ClientError::Internal)?;
        for record in batch.records() {
            match grid_target_cell(grid, record.cell) {
                CellView::Uncovered => return Ok(None),
                CellView::Passes => {}
                CellView::Target => {
                    return RayTarget::try_new(
                        BlockPos::new(record.cell[0], record.cell[1], record.cell[2]),
                        source_revision,
                    )
                    .map(Some);
                }
            }
        }
        if batch.is_done() {
            return Ok(None);
        }
    }
}

/// The classification of one traversed cell against the borrowed grid.
enum CellView {
    /// Inside the grid's cover, loaded, and carrying at least one collision
    /// box: a target cell.
    Target,
    /// Inside the cover, loaded, and empty of boxes: the walk passes over
    /// it.
    Passes,
    /// Outside the grid's cover or unloaded: nothing may be claimed past it.
    Uncovered,
}

/// Classifies one world cell against the borrowed collision grid, mirroring
/// the native physics wrapper's cell order (y-major, then x, then z) with the
/// same widened relative arithmetic, so the ray walk and the physics solver
/// read one grid. An unloaded in-cover cell is a knowledge boundary, not an
/// empty one: the collision core treats it as unknown and the accepted
/// target walk terminates at the first unobserved cell, so the ray walk ends
/// there too instead of claiming a solid cell beyond data the view does not
/// carry.
fn grid_target_cell(grid: &CollisionGrid<'_>, cell: [i32; 3]) -> CellView {
    let origin = grid.origin();
    let dims = grid.dimensions();
    let relative = [
        i64::from(cell[0]) - i64::from(origin[0]),
        i64::from(cell[1]) - i64::from(origin[1]),
        i64::from(cell[2]) - i64::from(origin[2]),
    ];
    if (0..3).any(|axis| relative[axis] < 0 || relative[axis] >= i64::from(dims[axis])) {
        return CellView::Uncovered;
    }
    let index = ((relative[1] as usize * dims[0] as usize) + relative[0] as usize)
        * dims[2] as usize
        + relative[2] as usize;
    let Some(observed) = grid.cells().get(index) else {
        return CellView::Uncovered;
    };
    if !observed.loaded() {
        return CellView::Uncovered;
    }
    if observed.used() > 0 {
        CellView::Target
    } else {
        CellView::Passes
    }
}

/// The look-ray direction of checked angles, mirrored from the accepted
/// interaction source the authority's own target walk uses: the f64
/// trigonometry keeps its float32 casts between the trigonometry and the
/// multiplication.
fn look_direction(yaw: f32, pitch: f32) -> [f32; 3] {
    let cos_pitch = f64::from(pitch).cos() as f32;
    [
        -(f64::from(yaw).sin() as f32) * cos_pitch,
        f64::from(pitch).sin() as f32,
        -(f64::from(yaw).cos() as f32) * cos_pitch,
    ]
}

/// Normalizes a direction without overflowing a float32 squared sum, the
/// accepted f64-hypot form: a tiny or invalid direction has no unit ray.
fn normalized_direction(direction: [f32; 3]) -> Option<[f32; 3]> {
    if direction.iter().any(|component| !component.is_finite()) {
        return None;
    }
    let length = f64::from(direction[0])
        .hypot(f64::from(direction[1]))
        .hypot(f64::from(direction[2]));
    if length < 1e-6 {
        return None;
    }
    let inverse = (1.0 / length) as f32;
    Some(direction.map(|component| component * inverse))
}

#[cfg(test)]
mod world_tests {
    use super::*;
    use crate::session::WorldConfirmed;
    use mornlea_domain::{
        ChunkPos, ChunkSnapshot, ChunkSnapshotParts, FiniteVec3, HeldActions, MotionState,
        MotionStateParts, Movement, PalettedSection, PlayerStateParts, Season, SurvivalState,
        SurvivalStateParts, Weather, WorldState, WorldStateParts, chunk_block_index,
    };

    fn world(dimension: Dimension, blocks: impl Fn(BlockPos) -> u16) -> WorldConfirmed {
        let mut world = WorldConfirmed::default();
        for cx in -1..=2 {
            for cz in -1..=1 {
                let mut sections =
                    Box::new(std::array::from_fn(|_| PalettedSection::single(0).unwrap()));
                for sy in 3..=4 {
                    let mut words = vec![0u64; 1024];
                    for y in 0..16 {
                        for z in 0..16 {
                            for x in 0..16 {
                                let pos = BlockPos::new(
                                    cx * 16 + x,
                                    (sy as i32) * 16 - 64 + y,
                                    cz * 16 + z,
                                );
                                let index = chunk_block_index(pos) as usize % 4096;
                                words[index / 4] |= u64::from(blocks(pos)) << ((index % 4) * 15);
                            }
                        }
                    }
                    sections[sy] = PalettedSection::direct(words.into_boxed_slice()).unwrap();
                }
                world
                    .apply_snapshot(
                        &ChunkSnapshot::try_new(ChunkSnapshotParts {
                            dimension,
                            chunk: ChunkPos::new(cx, cz),
                            revision: 1,
                            sections,
                        })
                        .unwrap(),
                        &ClientLimits::try_new().unwrap(),
                    )
                    .unwrap();
            }
        }
        world
    }

    fn authority(
        tick: u64,
        ack: u64,
        dimension: Dimension,
        position: [f32; 3],
        velocity: [f32; 3],
        on_ground: bool,
    ) -> PlayerState {
        PlayerState::new(PlayerStateParts {
            server_tick: tick,
            last_input_sequence: ack,
            dimension,
            motion: MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new(position).unwrap(),
                velocity: FiniteVec3::try_new(velocity).unwrap(),
                on_ground,
            }),
            look: LookAngles::try_new(-std::f32::consts::FRAC_PI_2, 0.0).unwrap(),
            ready: true,
            reset: false,
            mining: MiningState::Idle,
            survival: SurvivalState::try_new(SurvivalStateParts {
                health: 20,
                oxygen: 300,
                hunger: 20,
                saturation_zero: false,
                armor_points: 0,
            })
            .unwrap(),
            world: WorldState::try_new(WorldStateParts {
                day_phase_offset: 0,
                world_time_ticks: 0,
                weather: Weather::Clear,
                season: Season::Spring,
                season_progress: 0,
                temperature: 0,
            })
            .unwrap(),
        })
    }

    fn key(tick: u64) -> ObservationKey {
        ObservationKey::try_new(
            SessionEpoch::try_new(1).unwrap(),
            ConfirmedRevision::new(tick),
            0,
        )
        .unwrap()
    }

    fn owner(state: &PlayerState, world: &WorldConfirmed) -> PredictionReplay {
        let mut owner = PredictionReplay::try_new(
            SessionEpoch::try_new(1).unwrap(),
            ClientLimits::try_new().unwrap(),
        )
        .unwrap();
        owner.apply_world(&key(1), state, world).unwrap();
        owner
    }

    fn control() -> PlayerControl {
        PlayerControl::new(PlayerControlParts {
            movement: Movement {
                move_x: 0,
                move_z: 0,
                jump: false,
            },
            look: LookAngles::try_new(-std::f32::consts::FRAC_PI_2, 0.0).unwrap(),
            actions: HeldActions {
                primary: false,
                eating: false,
                sprinting: false,
                sneaking: false,
            },
        })
    }

    #[test]
    fn projection_keeps_the_actual_authority_dimension() {
        for dimension in [Dimension::OVERWORLD, Dimension::DEPTHS] {
            let world = world(dimension, |_| 0);
            let state = authority(1, 0, dimension, [3.5, 2.0, 5.5], [0.0; 3], true);
            assert_eq!(
                owner(&state, &world)
                    .projection()
                    .unwrap()
                    .world_dimension(),
                Some(dimension)
            );
        }
    }

    #[test]
    fn compact_world_steps_land_on_asymmetric_registered_shapes() {
        for (id, height) in [(2, 1.0), (35, 15.0 / 16.0), (76, 9.0 / 16.0)] {
            let world = world(Dimension::OVERWORLD, |p| {
                if p == BlockPos::new(3, 0, 5) { id } else { 0 }
            });
            let state = authority(
                1,
                0,
                Dimension::OVERWORLD,
                [3.5, 1.2, 5.5],
                [5.0, -10.0, 0.0],
                false,
            );
            let mut owner = owner(&state, &world);
            for sequence in 1..=3 {
                owner.admit_through(sequence).unwrap();
                owner
                    .predict_world(sequence, control(), &world, Dimension::OVERWORLD)
                    .unwrap();
            }
            let predicted = owner.predicted.unwrap();
            assert_eq!(predicted.position[1], height, "id {id}");
            assert!(predicted.on_ground, "id {id}");
        }
    }

    #[test]
    fn water_boundary_is_dry_but_positive_body_volume_uses_fluid_physics() {
        let world = world(Dimension::OVERWORLD, |p| {
            if p.x() == 1 && p.y() == 0 { 27 } else { 0 }
        });
        for (x, expected) in [(0.7, -1.6), (0.7001, -0.32)] {
            let mut owner = owner(
                &authority(1, 0, Dimension::OVERWORLD, [x, 0.0, 0.5], [0.0; 3], false),
                &world,
            );
            owner.admit_through(1).unwrap();
            let witness = owner
                .predict_world(1, control(), &world, Dimension::OVERWORLD)
                .unwrap();
            assert!((witness.velocity()[1] - expected).abs() < 1e-5);
        }
    }

    #[test]
    fn correction_rebuilds_each_step_and_each_ray_in_the_actual_dimension() {
        for dimension in [Dimension::OVERWORLD, Dimension::DEPTHS] {
            let world = world(dimension, |p| {
                if p.y() == 0 || (p.x() == 20 && (1..=3).contains(&p.y())) {
                    2
                } else {
                    0
                }
            });
            let mut owner = owner(
                &authority(1, 0, dimension, [14.0, 1.0, 0.5], [30.0, 0.0, 0.0], true),
                &world,
            );
            for sequence in 1..=8 {
                owner.begin_frame();
                owner.admit_through(sequence).unwrap();
                owner
                    .predict_world(sequence, control(), &world, dimension)
                    .unwrap();
            }
            let witness = owner
                .apply_world(
                    &key(2),
                    &authority(2, 0, dimension, [15.0, 1.0, 0.5], [30.0, 0.0, 0.0], true),
                    &world,
                )
                .unwrap();
            assert_eq!(witness.replayed(), 8);
            assert!((owner.predicted.unwrap().position[0] - 19.7).abs() < 1e-5);
            assert_eq!(
                owner.confirmed_target().unwrap().position(),
                BlockPos::new(20, 1, 0)
            );
            assert_eq!(
                owner.predicted_target().unwrap().position(),
                BlockPos::new(20, 1, 0)
            );
            assert!(owner.reject_world(8, &world, dimension).unwrap().is_some());
            assert_eq!(owner.pending.len(), 7);
            assert!((owner.predicted.unwrap().position[0] - 19.7).abs() < 1e-5);
        }
    }

    #[test]
    fn replay_target_grid_follows_predicted_pose_far_from_confirmed_origin() {
        let dimension = Dimension::DEPTHS;
        let world = world(dimension, |p| {
            if p.y() == 0 || (p.x() == 44 && p.y() == 1) {
                2
            } else {
                0
            }
        });
        let mut owner = owner(
            &authority(1, 0, dimension, [3.0, 1.0, 0.5], [100.0, 0.0, 0.0], true),
            &world,
        );
        for sequence in 1..=8 {
            owner.begin_frame();
            owner.admit_through(sequence).unwrap();
            owner
                .predict_world(sequence, control(), &world, dimension)
                .unwrap();
        }
        owner
            .apply_world(
                &key(2),
                &authority(2, 0, dimension, [4.0, 1.0, 0.5], [100.0, 0.0, 0.0], true),
                &world,
            )
            .unwrap();
        assert!((owner.predicted.unwrap().position[0] - 39.5).abs() < 1e-5);
        assert!(owner.confirmed_target().is_none());
        assert_eq!(
            owner.predicted_target().unwrap().position(),
            BlockPos::new(44, 1, 0)
        );
    }

    #[test]
    fn nonmovement_admission_acknowledges_without_allowing_future_input() {
        let world = world(Dimension::OVERWORLD, |_| 0);
        let state = authority(1, 0, Dimension::OVERWORLD, [0.5, 0.0, 0.5], [0.0; 3], false);
        let mut owner = owner(&state, &world);
        owner.admit_through(0).unwrap();
        assert_eq!(
            owner.predict_world(1, control(), &world, Dimension::OVERWORLD),
            Err(ClientError::InvalidInput)
        );
        assert!(owner.pending.is_empty());
        assert_eq!(owner.frame_steps, 0);
        owner.admit_through(3).unwrap();
        owner.admit_through(3).unwrap();
        assert_eq!(owner.admit_through(2), Err(ClientError::InvalidInput));
        owner
            .apply_world(
                &key(2),
                &authority(2, 3, Dimension::OVERWORLD, [0.5, 0.0, 0.5], [0.0; 3], false),
                &world,
            )
            .unwrap();
        assert_eq!(owner.last_acknowledged_sequence(), 3);
        let before = owner.projection().unwrap();
        assert_eq!(
            owner.apply_world(
                &key(3),
                &authority(3, 4, Dimension::OVERWORLD, [5.0, 0.0, 0.5], [0.0; 3], false),
                &world
            ),
            Err(ClientError::InvalidInput)
        );
        assert_eq!(
            owner.projection().unwrap().confirmed_pose(),
            before.confirmed_pose()
        );
        assert_eq!(owner.last_server_tick, 2);
        owner
            .reset_epoch(SessionEpoch::try_new(2).unwrap())
            .unwrap();
        assert_eq!(owner.admitted_high_water, 0);
    }

    #[test]
    fn missing_world_content_remains_unknown_while_loaded_air_is_empty() {
        let loaded = world(Dimension::OVERWORLD, |_| 0);
        let missing = WorldConfirmed::default();
        let state = authority(1, 0, Dimension::OVERWORLD, [0.5, 1.0, 0.5], [0.0; 3], false);
        for (world, unknown) in [(&missing, true), (&loaded, false)] {
            let mut owner = owner(&state, world);
            owner.admit_through(1).unwrap();
            let witness = owner
                .predict_world(1, control(), world, Dimension::OVERWORLD)
                .unwrap();
            assert_eq!(witness.hit_unknown(), unknown);
            assert!(owner.confirmed_target().is_none());
            assert!(owner.predicted_target().is_none());
        }
    }

    #[test]
    fn world_changes_refresh_targets_without_relabeling_the_confirmed_pose() {
        use mornlea_domain::{BlockChange, BlockChanges, BlockChangesParts};
        let dimension = Dimension::DEPTHS;
        let target = BlockPos::new(5, 1, 0);
        let mut world = world(dimension, |p| if p.y() == 0 || p == target { 2 } else { 0 });
        let mut owner = owner(
            &authority(1, 0, dimension, [0.5, 1.0, 0.5], [0.0; 3], true),
            &world,
        );
        owner.admit_through(1).unwrap();
        owner
            .predict_world(1, control(), &world, dimension)
            .unwrap();
        let original = owner.projection().unwrap();
        assert_eq!(
            owner.refresh_world_targets(&world, dimension, ConfirmedRevision::new(0)),
            Err(ClientError::InvalidInput)
        );
        assert_eq!(
            owner.confirmed_target().unwrap().source_revision(),
            ConfirmedRevision::new(1)
        );
        owner
            .refresh_world_targets(&world, dimension, ConfirmedRevision::new(2))
            .unwrap();
        assert_eq!(
            owner.confirmed_target().unwrap().source_revision(),
            ConfirmedRevision::new(2)
        );
        assert_eq!(
            owner.predicted_target().unwrap().source_revision(),
            ConfirmedRevision::new(2)
        );
        for (base_revision, block, revision) in [(1, 0, 3), (2, 2, 4)] {
            world
                .apply_changes(
                    &BlockChanges::try_new(BlockChangesParts {
                        dimension,
                        chunk: ChunkPos::new(0, 0),
                        base_revision,
                        new_revision: base_revision + 1,
                        changes: vec![BlockChange::try_new(target, block).unwrap()]
                            .into_boxed_slice(),
                    })
                    .unwrap(),
                    &ClientLimits::try_new().unwrap(),
                )
                .unwrap();
            owner
                .refresh_world_targets(&world, dimension, ConfirmedRevision::new(revision))
                .unwrap();
            assert_eq!(
                owner.confirmed_target().map(|t| t.position()),
                (block != 0).then_some(target)
            );
            assert_eq!(
                owner.predicted_target().map(|t| t.position()),
                (block != 0).then_some(target)
            );
        }
        world.remove_chunk(dimension, ChunkPos::new(0, 0));
        owner
            .refresh_world_targets(&world, dimension, ConfirmedRevision::new(5))
            .unwrap();
        assert!(owner.confirmed_target().is_none());
        assert!(owner.predicted_target().is_none());
        let refreshed = owner.projection().unwrap();
        assert_eq!(refreshed.confirmed_pose(), original.confirmed_pose());
        assert_eq!(refreshed.predicted_pose(), original.predicted_pose());
        assert_eq!(refreshed.journal(), original.journal());
        assert_eq!(refreshed.revision(), original.revision());
        assert_eq!(owner.last_server_tick, 1);
        assert_eq!(
            owner.confirmed.as_ref().unwrap().revision,
            ConfirmedRevision::new(1)
        );
    }

    #[test]
    fn coordinate_overflow_refuses_prediction_before_owner_mutation() {
        let world = world(Dimension::OVERWORLD, |_| 0);
        let mut owner = owner(
            &authority(
                1,
                0,
                Dimension::OVERWORLD,
                [0.5, 0.0, 0.5],
                [50000000000.0, 0.0, 0.0],
                true,
            ),
            &world,
        );
        owner.admit_through(1).unwrap();
        assert_eq!(
            owner.predict_world(1, control(), &world, Dimension::OVERWORLD),
            Err(ClientError::InvalidInput)
        );
        assert!(owner.pending.is_empty());
        assert!(owner.predicted.is_none());
        assert_eq!(owner.frame_steps, 0);
        assert_eq!(owner.issued_high_water, 0);
        assert_eq!(owner.last_server_tick, 1);
    }

    #[test]
    fn oversized_sweep_preserves_owner_and_sequence_for_retry() {
        let world = world(Dimension::OVERWORLD, |_| 0);
        let mut owner = owner(
            &authority(
                1,
                0,
                Dimension::OVERWORLD,
                [0.5, 0.0, 0.5],
                [100000.0, 0.0, 0.0],
                true,
            ),
            &world,
        );
        owner.admit_through(1).unwrap();
        assert_eq!(
            owner.predict_world(1, control(), &world, Dimension::OVERWORLD),
            Err(ClientError::Capacity)
        );
        assert!(owner.pending.is_empty());
        assert!(owner.predicted.is_none());
        assert_eq!(owner.frame_steps, 0);
        assert_eq!(owner.issued_high_water, 0);
        assert!(owner.snow_cues.is_empty());
        owner
            .apply_world(
                &key(2),
                &authority(2, 0, Dimension::OVERWORLD, [0.5, 0.0, 0.5], [0.0; 3], false),
                &world,
            )
            .unwrap();
        owner
            .predict_world(1, control(), &world, Dimension::OVERWORLD)
            .unwrap();
        assert_eq!(owner.pending.len(), 1);
    }
}
