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
//! collision grid and the body-in-fluid bit — are caller-owned mirror facts
//! supplied per call, because this crate holds no block mirror. The same
//! borrowed grid feeds the step-time ray-target recording: each step and
//! replay walks the accepted engine raycast kernel along the confirmed and
//! the predicted pose's look rays and records the first target cell each
//! reaches, stamped with the confirmed base's revision; no grid is read
//! outside step time and no hit is ever invented.

use crate::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, ObservationKey, SessionEpoch,
};
use crate::presentation::{Correction, CorrectionReason, FiniteRay, MovementIntent, Pose};
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
/// bit. Both are mirror facts the caller derives from its own world view;
/// this crate holds no block mirror, so no second derivation exists here.
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
struct PendingStep {
    sequence: u64,
    control: PlayerControl,
}

/// The latest confirmed authoritative base a replay starts from: the kernel
/// state, the authority's look, mining and hunger, the confirmed revision of
/// the observation that carried it, and whether the authority was ready.
struct ConfirmedBase {
    revision: ConfirmedRevision,
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
pub struct PredictionReplay {
    epoch: SessionEpoch,
    limits: ClientLimits,
    confirmed: Option<ConfirmedBase>,
    predicted: Option<PhysicsState>,
    pending: Vec<PendingStep>,
    issued_high_water: u64,
    last_acknowledged: u64,
    last_server_tick: u64,
    correction: Option<Correction>,
    frame_steps: u16,
    confirmed_target: Option<RayTarget>,
    predicted_target: Option<RayTarget>,
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
            last_acknowledged: 0,
            last_server_tick: 0,
            correction: None,
            frame_steps: 0,
            confirmed_target: None,
            predicted_target: None,
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
        Ok(PlayerProjectionState::try_new(
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
        // The step-time recorded ray targets ride beside their poses. The
        // re-derivation happened inside the step or replay that borrowed the
        // grid; reporting here is pure, so a repeated projection is equal.
        .with_confirmed_target(self.confirmed_target)
        .with_predicted_target(self.predicted_target))
    }

    /// Opens the next rendered frame's step budget. The per-frame ceiling
    /// exists so one long frame cannot journal an unbounded burst of steps;
    /// the ceiling-plus-one step rejects typed until this runs.
    pub fn begin_frame(&mut self) {
        self.frame_steps = 0;
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
        let base = self
            .confirmed
            .as_ref()
            .filter(|base| base.ready)
            .ok_or(ClientError::InvalidState)?;
        let movement = control.movement();
        if !(-1..=1).contains(&movement.move_x) || !(-1..=1).contains(&movement.move_z) {
            return Err(ClientError::InvalidInput);
        }
        if sequence == 0 || sequence <= self.issued_high_water {
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
        let (state, clipped, used_step, hit_unknown) =
            kernel_step(current, &effective, environment)?;
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
        // The step-time target refresh: both recorded rays are re-derived
        // against the grid this very step borrowed, so no grid is ever read
        // outside a step or replay. The ray walk runs over checked inputs —
        // finite origin, checked angles, the landed reach — so its kernel
        // refusal is structurally unreachable and still surfaces typed
        // rather than being swallowed into a silent no-hit.
        self.refresh_ray_targets(environment)?;
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
        if key.epoch() != self.epoch {
            return Err(ClientError::StaleEpoch);
        }
        if state.server_tick() <= self.last_server_tick {
            // The duplicate-acknowledgement no-op: the same observation is
            // old news, never a second acknowledgement or a rollback.
            return CorrectionWitness::try_new(0, 0, false);
        }
        if state.dimension() != Dimension::OVERWORLD {
            return Err(ClientError::InvalidInput);
        }
        let last = state.last_input_sequence();
        if last > self.issued_high_water {
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
            self.issued_high_water = last;
            self.last_acknowledged = self.last_acknowledged.max(last);
            self.refresh_ray_targets(environment)?;
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
            self.issued_high_water = last;
            self.last_acknowledged = last;
        } else {
            self.pending.retain(|step| step.sequence > last);
            self.last_acknowledged = self.last_acknowledged.max(last);
        }
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
        if self.pending.iter().all(|step| step.sequence != sequence) {
            return Ok(None);
        }
        self.pending.retain(|step| step.sequence != sequence);
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
        self.last_acknowledged = 0;
        self.last_server_tick = 0;
        self.correction = None;
        self.frame_steps = 0;
        self.confirmed_target = None;
        self.predicted_target = None;
        Ok(())
    }

    /// Rebuilds the attributed predicted state: the frozen kernel folds the
    /// remaining pending entries in sequence order starting from the latest
    /// confirmed base — never from the pre-correction prediction. With no
    /// pending entries nothing is attributable and the confirmed base is the
    /// whole projection.
    fn replay(&mut self, environment: &StepEnvironment<'_>) -> Result<(), ClientError> {
        let Some(base) = self.confirmed.as_ref() else {
            return Ok(());
        };
        let mut state = base.state;
        for step in &self.pending {
            state = kernel_step(state, &step.control, environment)?.0;
        }
        self.predicted = (!self.pending.is_empty()).then_some(state);
        self.refresh_ray_targets(environment)?;
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
        environment: &StepEnvironment<'_>,
    ) -> Result<(), ClientError> {
        let Some(base) = self.confirmed.as_ref() else {
            self.confirmed_target = None;
            self.predicted_target = None;
            return Ok(());
        };
        let grid = environment.grid();
        self.confirmed_target = ray_target(base.state.position, base.look, base.revision, grid)?;
        self.predicted_target = match (self.predicted, self.pending.last()) {
            (Some(state), Some(step)) => {
                ray_target(state.position, step.control.look(), base.revision, grid)?
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
    let look = control.look();
    let yaw = f64::from(look.yaw());
    let controls = PhysicsControls {
        move_x: control.movement().move_x,
        move_z: control.movement().move_z,
        jump: control.movement().jump,
        yaw_sin: yaw.sin() as f32,
        yaw_cos: yaw.cos() as f32,
        body_in_fluid: environment.body_in_fluid(),
        sprinting: control.actions().sprinting,
        sneaking: control.actions().sneaking,
    };
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
