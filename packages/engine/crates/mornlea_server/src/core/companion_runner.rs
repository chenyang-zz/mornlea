//! Pure companion task runner transitions.
//!
//! The runner is a value state machine: it receives the current tick, the
//! world time, the companion pose and any path outcome delivered at this
//! tick boundary, and returns effects for the caller to apply. It never
//! waits, spawns threads, reads a clock, or touches authority state, so
//! the same inputs always produce the same effects. Pending path work and
//! its deferral live with the runtime caller; this module only decides.
//!
//! The numbers mirror the Go runner one to one: a task deadline is the
//! world time at Running entry plus the timeout minutes converted at
//! 1200 ticks per minute, a follow-tail plan keeps a zero deadline and
//! never expires, path replans wait a fixed 20-tick cooldown, and the
//! third consecutive path failure ends the task as path unreachable.
//!
//! Mine and place steps run through the same runner in plan step order,
//! mirroring the Go interaction runner one to one: the companion first
//! paths to a standing cell adjacent to the target with the go_to movement
//! semantics, then the interaction phase holds mining or submits one
//! placement per tick and judges completion only from settled authority
//! observations. The block rules themselves (reach ray, mineability, break
//! timing, product crediting, placement validity and debit) stay with the
//! authority mining and placement providers; this module never reimplements
//! them, it only decides which action to submit and how to read the result.

use mornlea_domain::{BlockPos, CompanionId, TaskFailure, TaskState};
use mornlea_engine::native::contracts::pathfind::PathGrid;

use super::contracts::{CompanionAction, PlanBlock, PlanStep, ServerError};

/// World ticks in one minute at the fixed 20 ticks per second rate.
pub const TICKS_PER_MINUTE: u64 = 1200;
/// Default timeout minutes when the frozen value carries the unset form.
pub const DEFAULT_TIMEOUT_MINUTES: u32 = 10;
/// Fixed cooldown between path replans, in ticks.
pub const PATH_REPLAN_COOLDOWN_TICKS: u64 = 20;
/// Consecutive path failures that end a task as path unreachable.
pub const MAX_CONSECUTIVE_PATH_FAILURES: u32 = 3;
/// Horizontal distance within which a follower holds position.
pub const FOLLOW_DISTANCE_BLOCKS: f32 = 4.0;
/// Follower path goal drift, in blocks, that discards the current path.
pub const FOLLOW_REPLAN_DRIFT_BLOCKS: f32 = 2.0;
/// Horizontal arrival radius around a waypoint centre, in blocks.
pub const WAYPOINT_ARRIVAL_RADIUS: f32 = 0.35;

/// Smallest accepted task timeout, in minutes (Go `ValidateTaskTimeoutMinutes`).
pub const MIN_TIMEOUT_MINUTES: u32 = 1;
/// Largest accepted task timeout, in minutes (Go `ValidateTaskTimeoutMinutes`).
pub const MAX_TIMEOUT_MINUTES: u32 = 60;

/// A task run timeout frozen at startup, in minutes.
///
/// Only values in `1..=60` construct, the range the Go config loader
/// accepts, so a production install can never carry the unset or an
/// out-of-range value. The value is plain data: once a holder is built
/// from it there is no way to change it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskTimeoutMinutes(u32);

impl TaskTimeoutMinutes {
    /// The Go default used when the config key is absent.
    pub const DEFAULT: Self = Self(DEFAULT_TIMEOUT_MINUTES);

    /// Accepts exactly `1..=60` minutes.
    pub fn try_new(minutes: u32) -> Result<Self, ServerError> {
        if (MIN_TIMEOUT_MINUTES..=MAX_TIMEOUT_MINUTES).contains(&minutes) {
            Ok(Self(minutes))
        } else {
            Err(ServerError::InvalidInput {
                field: "task_timeout_minutes",
            })
        }
    }

    /// The minutes value.
    pub fn get(self) -> u32 {
        self.0
    }
}

/// Deadline for a task entering Running: the entry world time plus the
/// timeout converted to ticks. A follow-tail plan keeps zero, which the
/// expiry check treats as exempt. An unset minute value falls back to
/// the default, matching the Go conversion.
pub fn task_deadline_ticks(world_time: u64, timeout_minutes: u32, follow_tail: bool) -> u64 {
    if follow_tail {
        return 0;
    }
    let minutes = if timeout_minutes < 1 {
        DEFAULT_TIMEOUT_MINUTES
    } else {
        timeout_minutes
    };
    world_time.saturating_add(u64::from(minutes).saturating_mul(TICKS_PER_MINUTE))
}

/// Whether a plan ends in a follow step, the only timeout-exempt shape.
pub fn plan_follow_tail(steps: &[PlanStep]) -> bool {
    matches!(steps.last(), Some(PlanStep::Follow { .. }))
}

/// What one runner action submission did, in pass order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunnerSubmission {
    /// Companions whose action was queued for the next tick.
    pub admitted: Vec<CompanionId>,
    /// Companions whose action the ordinary checks refused, with the reason.
    pub refused: Vec<(CompanionId, ServerError)>,
}

/// A path outcome delivered at a tick boundary. Both search failures
/// (unreachable and budget exhausted) share this one failure form,
/// matching the Go runner, which handles them identically.
#[derive(Clone, Debug, PartialEq)]
pub enum PathOutcome {
    /// Ordered waypoints from start to goal, with the chunk revisions
    /// the search grid carried, for per-tick revalidation.
    Found {
        /// Ordered waypoints from start to goal.
        waypoints: Vec<BlockPos>,
        /// Chunk revisions carried by the searched grid.
        revisions: Vec<(mornlea_domain::ChunkPos, u64)>,
    },
    /// The search produced no path.
    Failed,
}

/// Follow target observation for one tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FollowObservation {
    /// The current step is not a follow step.
    NotFollow,
    /// The target is online and in the companion's dimension.
    Online { position: [f32; 3] },
    /// The target is offline or in another dimension; `offline` selects
    /// between failing the task (offline) and holding position (other
    /// dimension), matching the Go follow runner.
    Away { offline: bool },
}

/// The companion's settled mining progress at this boundary, as the
/// authority mining provider left it after the previous tick. Absent
/// means no active progress (released, cleared, or just settled).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiningObservation {
    /// Block the progress accumulates on.
    pub target: BlockPos,
    /// Ticks accumulated on the unchanged target/block/tool key.
    pub elapsed: u32,
    /// Ticks the block/tool pair requires.
    pub required: u32,
}

/// Settled observations a mine or place step reads at this boundary. The
/// caller fills it only while the current step is a mine or place step;
/// every other step ignores it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InteractionObservation {
    /// Standing cell adjacent to the target the approach paths to.
    pub approach_goal: BlockPos,
    /// Current block at the target, or `None` while its chunk is not Ready.
    pub target_block: Option<u16>,
    /// The companion's settled mining progress.
    pub mining: Option<MiningObservation>,
    /// Whether the saturated product would not fit the companion inventory,
    /// judged by the same resolver the mining provider settles with.
    pub capacity_exceeded: bool,
    /// Whether the companion inventory holds at least one item that places
    /// the planned block.
    pub holds_item: bool,
    /// Eye height above the feet, from the frozen tunables.
    pub eye_height: f32,
    /// Interaction reach, from the frozen tunables.
    pub reach: f32,
}

impl Default for InteractionObservation {
    fn default() -> Self {
        Self {
            approach_goal: BlockPos::new(0, 0, 0),
            target_block: None,
            mining: None,
            capacity_exceeded: false,
            holds_item: false,
            eye_height: 0.0,
            reach: 0.0,
        }
    }
}

/// Inputs for one runner tick, all read by the caller at the boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct RunnerTick {
    /// Completed-tick count at this boundary.
    pub tick: u64,
    /// Authoritative world time in ticks.
    pub world_time: u64,
    /// Companion position at this boundary.
    pub position: [f32; 3],
    /// Follow target observation for the current step.
    pub follow: FollowObservation,
    /// Current revisions of the chunks around the companion, read at
    /// this tick boundary; a carried path revision missing here fails
    /// closed.
    pub current_revisions: Vec<(mornlea_domain::ChunkPos, u64)>,
    /// Mine/place observations for the current step.
    pub interaction: InteractionObservation,
}

/// One effect the caller applies, in the order returned.
#[derive(Clone, Debug, PartialEq)]
pub enum RunnerEffect {
    /// Ask the runtime to search for a path to this goal. At most one
    /// request per task is outstanding at any time.
    RequestPath { generation: u64, goal: BlockPos },
    /// Submit this single movement action for the companion this tick.
    Move(CompanionAction),
    /// Submit this single mine hold, mine release, or placement action for
    /// the companion this tick.
    Interact(CompanionAction),
    /// A non-final step finished; the stored step index advances.
    StepProgress { step_index: usize },
    /// The task reached a terminal state through the finish path.
    Finish(TaskState),
}

/// Per-task runner state. It is created when a task enters Running (or
/// restored from its stored form) and dropped when the task finishes, so
/// failure counters never leak into the next task.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskRunner {
    generation: u64,
    step_index: usize,
    deadline_ticks: u64,
    path: Option<Vec<BlockPos>>,
    path_revisions: Vec<(mornlea_domain::ChunkPos, u64)>,
    waypoint: usize,
    consecutive_failures: u32,
    replan_at: Option<u64>,
    outstanding: bool,
    follow_goal: Option<BlockPos>,
    finished: bool,
    interact_step: Option<usize>,
    interaction_ready: bool,
    mine_progress: u32,
    mine_required: u32,
    mining_held: bool,
}

impl TaskRunner {
    /// Fresh state for a task entering Running with its stamped deadline.
    pub fn installed(generation: u64, deadline_ticks: u64) -> Self {
        Self {
            generation,
            step_index: 0,
            deadline_ticks,
            path: None,
            path_revisions: Vec::new(),
            waypoint: 0,
            consecutive_failures: 0,
            replan_at: None,
            outstanding: false,
            follow_goal: None,
            finished: false,
            interact_step: None,
            interaction_ready: false,
            mine_progress: 0,
            mine_required: 0,
            mining_held: false,
        }
    }

    /// State restored from a stored running task: step index and deadline
    /// carry over, while the failure budget starts full and a mine or place
    /// step restarts from its approach, matching the Go restore discipline.
    pub fn restored(generation: u64, step_index: usize, deadline_ticks: u64) -> Self {
        Self {
            step_index,
            ..Self::installed(generation, deadline_ticks)
        }
    }

    /// The generation this runner serves; stale deliveries are discarded.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Current step index within the installed plan.
    pub fn step_index(&self) -> usize {
        self.step_index
    }

    /// Stamped deadline in world ticks; zero means exempt.
    pub fn deadline_ticks(&self) -> u64 {
        self.deadline_ticks
    }

    /// Whether the runner currently holds a usable path.
    pub fn has_path(&self) -> bool {
        self.path.is_some()
    }

    /// Whether a path request is currently outstanding.
    pub fn path_outstanding(&self) -> bool {
        self.outstanding
    }

    /// Consecutive path failures recorded for this task.
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Whether this runner already produced a terminal effect.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Whether the current mine or place step finished its approach and is
    /// in the interaction phase.
    pub fn interaction_ready(&self) -> bool {
        self.interaction_ready && self.interact_step == Some(self.step_index)
    }

    /// Applies one path outcome delivered at a tick boundary. Outcomes
    /// for another generation, or with nothing outstanding, are discarded
    /// without touching the failure budget.
    pub fn on_path_outcome(
        &mut self,
        tick: u64,
        generation: u64,
        outcome: PathOutcome,
    ) -> Vec<RunnerEffect> {
        if self.finished || generation != self.generation || !self.outstanding {
            return Vec::new();
        }
        self.outstanding = false;
        match outcome {
            PathOutcome::Found {
                waypoints,
                revisions,
            } => {
                self.path = Some(waypoints);
                self.path_revisions = revisions;
                self.waypoint = 0;
                self.replan_at = None;
                Vec::new()
            }
            PathOutcome::Failed => {
                self.path = None;
                self.path_revisions = Vec::new();
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                if self.consecutive_failures >= MAX_CONSECUTIVE_PATH_FAILURES {
                    self.finished = true;
                    vec![RunnerEffect::Finish(TaskState::Failed(
                        TaskFailure::PathUnreachable,
                    ))]
                } else {
                    self.replan_at = Some(tick.saturating_add(PATH_REPLAN_COOLDOWN_TICKS));
                    Vec::new()
                }
            }
        }
    }

    /// Clears an outstanding request without recording a failure, used
    /// when the tick-boundary grid was not ready and the search was
    /// deferred before it ever ran.
    pub fn defer_outstanding(&mut self) {
        self.outstanding = false;
    }

    /// Advances the task by one tick. Expiry is judged first, before any
    /// movement, matching the Go tick order. A mining hold left behind by a
    /// step the task has moved past is released in the same pass.
    pub fn on_tick(&mut self, steps: &[PlanStep], input: &RunnerTick) -> Vec<RunnerEffect> {
        let mut effects = self.advance(steps, input);
        self.release_finished_mining(steps, &mut effects);
        effects
    }

    fn advance(&mut self, steps: &[PlanStep], input: &RunnerTick) -> Vec<RunnerEffect> {
        if self.finished {
            return Vec::new();
        }
        if self.deadline_ticks != 0 && input.world_time >= self.deadline_ticks {
            self.finished = true;
            self.path = None;
            self.path_revisions = Vec::new();
            return vec![RunnerEffect::Finish(TaskState::TimedOut)];
        }
        let Some(step) = steps.get(self.step_index) else {
            return Vec::new();
        };
        let interaction = matches!(step, PlanStep::Mine { .. } | PlanStep::Place { .. });
        let goal = match step {
            PlanStep::GoTo { x, y, z } => BlockPos::new(*x, *y, *z),
            PlanStep::Follow { .. } => match self.follow_goal_for(input) {
                Ok(goal) => goal,
                Err(effects) => return effects,
            },
            PlanStep::Mine { .. } | PlanStep::Place { .. } => {
                if self.interact_step != Some(self.step_index) {
                    // A new interaction step (or a leftover from an earlier
                    // one): its interaction memory starts over from the
                    // approach.
                    self.interact_step = Some(self.step_index);
                    self.interaction_ready = false;
                    self.mine_progress = 0;
                    self.mine_required = 0;
                }
                if self.interaction_ready {
                    return self.interact(steps, step, input);
                }
                input.interaction.approach_goal
            }
        };
        let is_follow = matches!(step, PlanStep::Follow { .. });
        if self.outstanding {
            return Vec::new();
        }
        // Every tick revalidates the carried path against the current
        // settled window before any waypoint is consumed: a missing or
        // changed revision discards the path and starts the fixed
        // cooldown, without counting a path failure.
        if self.path.is_some() && !self.path_still_valid(input) {
            self.path = None;
            self.path_revisions = Vec::new();
            self.follow_goal = None;
            self.replan_at = Some(input.tick.saturating_add(PATH_REPLAN_COOLDOWN_TICKS));
            return Vec::new();
        }
        let Some(path) = self.path.clone() else {
            if self.replan_at.is_some_and(|at| input.tick < at) {
                return Vec::new();
            }
            self.outstanding = true;
            if is_follow {
                self.follow_goal = Some(goal);
            }
            return vec![RunnerEffect::RequestPath {
                generation: self.generation,
                goal,
            }];
        };
        // Consume every waypoint already reached; each consumption is a
        // successful path use and clears the failure budget.
        while self.waypoint < path.len() && arrived(input.position, path[self.waypoint]) {
            self.waypoint += 1;
            self.consecutive_failures = 0;
        }
        if self.waypoint >= path.len() {
            self.path = None;
            self.path_revisions = Vec::new();
            self.follow_goal = None;
            if is_follow {
                // A follow step never completes: the path ran out while
                // the target is still away, so search again from here.
                self.outstanding = true;
                return vec![RunnerEffect::RequestPath {
                    generation: self.generation,
                    goal,
                }];
            }
            if interaction {
                // The approach reached the adjacent standing cell. No
                // action is submitted on this tick; the interaction starts
                // on the next one, keeping the one-tick observation lag.
                self.interaction_ready = true;
                self.replan_at = None;
                return Vec::new();
            }
            return self.advance_step(steps);
        }
        vec![RunnerEffect::Move(move_toward(
            input.position,
            path[self.waypoint],
        ))]
    }

    /// Marks the current step done: a non-final step reports progress and
    /// the final one completes the task.
    fn advance_step(&mut self, steps: &[PlanStep]) -> Vec<RunnerEffect> {
        self.step_index += 1;
        if self.step_index >= steps.len() {
            self.finished = true;
            return vec![RunnerEffect::Finish(TaskState::Completed)];
        }
        vec![RunnerEffect::StepProgress {
            step_index: self.step_index,
        }]
    }

    /// Ends the task with a failure reason through the finish path.
    fn fail(&mut self, reason: TaskFailure) -> Vec<RunnerEffect> {
        self.finished = true;
        vec![RunnerEffect::Finish(TaskState::Failed(reason))]
    }

    /// One interaction-phase tick: mine holds and reads the settled
    /// progress, place checks reach and submits, then reads the block.
    fn interact(
        &mut self,
        steps: &[PlanStep],
        step: &PlanStep,
        input: &RunnerTick,
    ) -> Vec<RunnerEffect> {
        let observation = &input.interaction;
        match *step {
            PlanStep::Mine { x, y, z } => {
                self.hold_mining(steps, BlockPos::new(x, y, z), observation)
            }
            PlanStep::Place { x, y, z, block } => {
                let target = BlockPos::new(x, y, z);
                let block = plan_block_id(block);
                // A target chunk that is not Ready is never judged.
                let Some(current) = observation.target_block else {
                    return Vec::new();
                };
                if current == block {
                    // The planned block is in place: normally this
                    // companion's placement settled on the previous tick.
                    // Another actor placing the same block also meets the
                    // step goal, so ownership is not distinguished.
                    return self.advance_step(steps);
                }
                if current != AIR_BLOCK {
                    // Occupied by something else: never overwrite.
                    return self.fail(TaskFailure::WorldChanged);
                }
                if !observation.holds_item {
                    return self.fail(TaskFailure::InventoryFull);
                }
                if !within_interaction_reach(
                    input.position,
                    observation.eye_height,
                    observation.reach,
                    target,
                ) {
                    // Placement settlement does not check distance, so this
                    // gate is the only one: wait for the body to settle and
                    // let the deadline bound the wait.
                    return Vec::new();
                }
                vec![RunnerEffect::Interact(CompanionAction::Place {
                    target,
                    block,
                })]
            }
            PlanStep::GoTo { .. } | PlanStep::Follow { .. } => Vec::new(),
        }
    }

    /// One mining tick: keep holding, and use the settled progress as the
    /// only completion and failure signal.
    ///
    /// - Progress regressing or its required ticks changing means another
    ///   actor replaced the target: the task fails as world changed and the
    ///   new block is never mined.
    /// - Saturated progress with the block still present means the
    ///   settlement refused for capacity; when the product cannot fit, the
    ///   task fails as inventory full with the block unchanged.
    /// - Progress gone with the target emptied completes the step only when
    ///   the last observation was one tick short of the requirement (the
    ///   settlement tick); otherwise another actor removed the block.
    fn hold_mining(
        &mut self,
        steps: &[PlanStep],
        target: BlockPos,
        observation: &InteractionObservation,
    ) -> Vec<RunnerEffect> {
        let Some(block) = observation.target_block else {
            // Never judge a half-loaded world; the hold is left as is.
            return Vec::new();
        };
        let progress = observation
            .mining
            .filter(|progress| progress.target == target);
        let Some(progress) = progress else {
            if block == AIR_BLOCK {
                if self.mine_required != 0
                    && self.mine_progress.saturating_add(1) >= self.mine_required
                {
                    return self.advance_step(steps);
                }
                return self.fail(TaskFailure::WorldChanged);
            }
            // The block is present and no progress accumulated yet: keep
            // holding; zero progress is the authority's out-of-reach signal.
            return self.hold(target);
        };
        if self.mine_required != 0
            && (progress.elapsed < self.mine_progress || progress.required != self.mine_required)
        {
            return self.fail(TaskFailure::WorldChanged);
        }
        if progress.elapsed == progress.required && observation.capacity_exceeded {
            return self.fail(TaskFailure::InventoryFull);
        }
        self.mine_progress = progress.elapsed;
        self.mine_required = progress.required;
        self.hold(target)
    }

    fn hold(&mut self, target: BlockPos) -> Vec<RunnerEffect> {
        self.mining_held = true;
        vec![RunnerEffect::Interact(CompanionAction::MineHold { target })]
    }

    /// Releases a mining hold the task has moved past. The authority keeps
    /// a companion hold across ticks until a release, so a hold left on a
    /// finished step could resume mining unattended. A terminal effect
    /// needs no release action here: the finish path clears the hold and
    /// the progress with the task, and the running generation that could
    /// carry the action is already gone.
    fn release_finished_mining(&mut self, steps: &[PlanStep], effects: &mut Vec<RunnerEffect>) {
        if !self.mining_held || self.finished {
            return;
        }
        let holding = self.interaction_ready
            && self.interact_step == Some(self.step_index)
            && matches!(steps.get(self.step_index), Some(PlanStep::Mine { .. }));
        if holding {
            return;
        }
        self.mining_held = false;
        effects.push(RunnerEffect::Interact(CompanionAction::MineRelease));
    }

    /// Whether the carried path may still be used: the waypoint cursor
    /// must be inside the path and every carried chunk revision must be
    /// present with the same value in the current window.
    fn path_still_valid(&self, input: &RunnerTick) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        if self.waypoint >= path.len() {
            return false;
        }
        self.path_revisions.iter().all(|(chunk, revision)| {
            input
                .current_revisions
                .iter()
                .any(|(current, value)| current == chunk && value == revision)
        })
    }

    /// Resolves the dynamic follow goal for this tick, or the effects
    /// that end or hold the task when there is no goal to chase.
    fn follow_goal_for(&mut self, input: &RunnerTick) -> Result<BlockPos, Vec<RunnerEffect>> {
        match input.follow {
            FollowObservation::Online { position } => {
                if within_follow_distance(input.position, position) {
                    self.path = None;
                    self.follow_goal = None;
                    return Err(Vec::new());
                }
                let goal = standing_cell(position);
                if self.path.is_some()
                    && self
                        .follow_goal
                        .is_some_and(|known| drifted(known, position))
                {
                    // The target moved on: drop the stale path and let the
                    // caller search again toward the current goal.
                    self.path = None;
                    self.follow_goal = Some(goal);
                }
                Ok(goal)
            }
            FollowObservation::Away { offline: true } => {
                self.finished = true;
                self.path = None;
                Err(vec![RunnerEffect::Finish(TaskState::Failed(
                    TaskFailure::WorldChanged,
                ))])
            }
            FollowObservation::Away { offline: false } | FollowObservation::NotFollow => {
                self.path = None;
                self.follow_goal = None;
                Err(Vec::new())
            }
        }
    }
}

/// Whether the position is within the horizontal arrival radius of the
/// waypoint centre.
fn arrived(position: [f32; 3], cell: BlockPos) -> bool {
    let dx = position[0] - (cell.x() as f32 + 0.5);
    let dz = position[2] - (cell.z() as f32 + 0.5);
    dx * dx + dz * dz <= WAYPOINT_ARRIVAL_RADIUS * WAYPOINT_ARRIVAL_RADIUS
}

/// Place-block table shared with the agent host mapping.
///
/// Values are the frozen core block ids in declaration order, copied
/// read-only from the agent host's plan mapping.
pub fn plan_block_id(block: PlanBlock) -> u16 {
    match block {
        PlanBlock::Brick => 21,
        PlanBlock::Chest => 11,
        PlanBlock::Clay => 24,
        PlanBlock::Cobblestone => 13,
        PlanBlock::Dirt => 3,
        PlanBlock::Furnace => 9,
        PlanBlock::Glass => 20,
        PlanBlock::Grass => 4,
        PlanBlock::Gravel => 16,
        PlanBlock::IronBlock => 10,
        PlanBlock::Leaves => 19,
        PlanBlock::LightBlock => 12,
        PlanBlock::MossyCobblestone => 26,
        PlanBlock::OakLog => 17,
        PlanBlock::OakPlanks => 18,
        PlanBlock::RoofTile => 23,
        PlanBlock::Sand => 15,
        PlanBlock::SmoothStone => 14,
        PlanBlock::SnowBlock => 25,
        PlanBlock::Stone => 2,
        PlanBlock::StoneBrick => 6,
        PlanBlock::WhiteWool => 22,
        PlanBlock::Workbench => 45,
    }
}

/// Block id of air.
const AIR_BLOCK: u16 = 0;

/// Whether the eye-to-target-centre distance is within the interaction
/// reach, the same anchor the authority mining ray uses. Occlusion is not
/// judged: a placement target is always an air cell.
pub fn within_interaction_reach(
    position: [f32; 3],
    eye_height: f32,
    reach: f32,
    target: BlockPos,
) -> bool {
    let eye_y = position[1] + eye_height;
    let dx = position[0] - (target.x() as f32 + 0.5);
    let dy = eye_y - (target.y() as f32 + 0.5);
    let dz = position[2] - (target.z() as f32 + 0.5);
    dx * dx + dy * dy + dz * dz <= reach * reach
}

/// The fixed approach goal for a mine or place target: the first adjacent
/// standing cell, same layer then one up then one down, each in -X, +X,
/// -Z, +Z order. A standing cell has air feet and head over a non-air
/// support; an unreadable cell never stands. When nothing stands the +X
/// same-layer cell is returned and path search decides reachability.
pub fn interaction_goal(target: BlockPos, block_at: impl Fn(BlockPos) -> Option<u16>) -> BlockPos {
    let offset = |dx: i32, dy: i32, dz: i32| {
        BlockPos::new(
            target.x().saturating_add(dx),
            target.y().saturating_add(dy),
            target.z().saturating_add(dz),
        )
    };
    for dy in [0, 1, -1] {
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let cell = offset(dx, dy, dz);
            let feet = block_at(cell);
            let head = block_at(offset(dx, dy + 1, dz));
            let support = block_at(offset(dx, dy - 1, dz));
            if feet == Some(AIR_BLOCK)
                && head == Some(AIR_BLOCK)
                && support.is_some_and(|block| block != AIR_BLOCK)
            {
                return cell;
            }
        }
    }
    offset(1, 0, 0)
}

/// The standing (feet) cell a position occupies.
pub fn standing_cell(position: [f32; 3]) -> BlockPos {
    BlockPos::new(
        position[0].floor() as i32,
        position[1].floor() as i32,
        position[2].floor() as i32,
    )
}

fn within_follow_distance(from: [f32; 3], to: [f32; 3]) -> bool {
    let dx = from[0] - to[0];
    let dz = from[2] - to[2];
    dx * dx + dz * dz <= FOLLOW_DISTANCE_BLOCKS * FOLLOW_DISTANCE_BLOCKS
}

fn drifted(goal: BlockPos, target: [f32; 3]) -> bool {
    let dx = goal.x() as f32 + 0.5 - target[0];
    let dz = goal.z() as f32 + 0.5 - target[2];
    dx * dx + dz * dz > FOLLOW_REPLAN_DRIFT_BLOCKS * FOLLOW_REPLAN_DRIFT_BLOCKS
}

/// Canonical forward movement toward a waypoint: yaw faces the target,
/// forward input is full, and a waypoint at least one block above the
/// feet asks for a jump. Displacement itself stays with the physics
/// kernel on the authority tick.
pub fn move_toward(position: [f32; 3], target: BlockPos) -> CompanionAction {
    let dx = target.x() as f32 + 0.5 - position[0];
    let dz = target.z() as f32 + 0.5 - position[2];
    let feet_y = position[1].floor() as i32;
    CompanionAction::Move {
        move_x: 0,
        move_z: 1,
        jump: target.y() > feet_y,
        yaw: (-dx).atan2(-dz),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go_to(x: i32, y: i32, z: i32) -> PlanStep {
        PlanStep::GoTo { x, y, z }
    }

    fn tick(tick: u64, world_time: u64, position: [f32; 3]) -> RunnerTick {
        RunnerTick {
            tick,
            world_time,
            position,
            follow: FollowObservation::NotFollow,
            current_revisions: Vec::new(),
            interaction: InteractionObservation::default(),
        }
    }

    #[test]
    fn deadline_stamped_at_running_entry_not_before() {
        // 10 minutes at 1200 ticks per minute from the entry world time.
        assert_eq!(task_deadline_ticks(500, 10, false), 500 + 12_000);
        assert_eq!(task_deadline_ticks(0, 1, false), 1200);
        // Follow-tail plans keep the zero exemption.
        assert_eq!(task_deadline_ticks(500, 10, true), 0);
        // Unset minutes fall back to the default.
        assert_eq!(task_deadline_ticks(0, 0, false), 12_000);
    }

    #[test]
    fn task_timeout_accepts_only_the_go_config_range() {
        // Go config.go validates taskTimeoutMinutes to 1..=60 and
        // defaults an absent key to 10.
        for minutes in [0, MAX_TIMEOUT_MINUTES + 1, u32::MAX] {
            assert_eq!(
                TaskTimeoutMinutes::try_new(minutes),
                Err(ServerError::InvalidInput {
                    field: "task_timeout_minutes"
                })
            );
        }
        for minutes in MIN_TIMEOUT_MINUTES..=MAX_TIMEOUT_MINUTES {
            assert_eq!(TaskTimeoutMinutes::try_new(minutes).unwrap().get(), minutes);
        }
        assert_eq!(TaskTimeoutMinutes::DEFAULT.get(), DEFAULT_TIMEOUT_MINUTES);
        assert_eq!(DEFAULT_TIMEOUT_MINUTES, 10);
    }

    #[test]
    fn timeout_is_pure_tick_comparison() {
        let steps = vec![go_to(3, 64, 0)];
        let mut runner = TaskRunner::installed(1, task_deadline_ticks(100, 1, false));
        let before = runner.on_tick(&steps, &tick(10, 100 + 1199, [0.5, 64.0, 0.5]));
        assert!(!matches!(before.as_slice(), [RunnerEffect::Finish(_)]));
        let at = runner.on_tick(&steps, &tick(11, 100 + 1200, [0.5, 64.0, 0.5]));
        assert_eq!(at, vec![RunnerEffect::Finish(TaskState::TimedOut)]);
        // A zero deadline never expires.
        let mut exempt = TaskRunner::installed(1, 0);
        let never = exempt.on_tick(&steps, &tick(12, u64::MAX - 1, [0.5, 64.0, 0.5]));
        assert!(!matches!(never.as_slice(), [RunnerEffect::Finish(_)]));
    }

    #[test]
    fn single_outstanding_and_cooldown_and_third_failure() {
        let steps = vec![go_to(10, 64, 0)];
        let mut runner = TaskRunner::installed(7, 0);
        let first = runner.on_tick(&steps, &tick(100, 0, [0.5, 64.0, 0.5]));
        assert_eq!(
            first,
            vec![RunnerEffect::RequestPath {
                generation: 7,
                goal: BlockPos::new(10, 64, 0)
            }]
        );
        // No second request while one is outstanding.
        assert!(
            runner
                .on_tick(&steps, &tick(101, 0, [0.5, 64.0, 0.5]))
                .is_empty()
        );
        // First failure: cooldown until tick 121, no request before it.
        assert!(
            runner
                .on_path_outcome(101, 7, PathOutcome::Failed)
                .is_empty()
        );
        assert!(
            runner
                .on_tick(&steps, &tick(120, 0, [0.5, 64.0, 0.5]))
                .is_empty()
        );
        assert_eq!(
            runner
                .on_tick(&steps, &tick(121, 0, [0.5, 64.0, 0.5]))
                .len(),
            1
        );
        assert!(
            runner
                .on_path_outcome(121, 7, PathOutcome::Failed)
                .is_empty()
        );
        assert_eq!(
            runner
                .on_tick(&steps, &tick(141, 0, [0.5, 64.0, 0.5]))
                .len(),
            1
        );
        // Third consecutive failure finishes as path unreachable.
        let third = runner.on_path_outcome(141, 7, PathOutcome::Failed);
        assert_eq!(
            third,
            vec![RunnerEffect::Finish(TaskState::Failed(
                TaskFailure::PathUnreachable
            ))]
        );
        // A fresh runner for the next task starts with a full budget.
        let mut next = TaskRunner::installed(8, 0);
        assert_eq!(next.consecutive_failures(), 0);
        assert_eq!(
            next.on_tick(&steps, &tick(142, 0, [0.5, 64.0, 0.5])).len(),
            1
        );
    }

    #[test]
    fn stale_outcome_discarded_and_defer_costs_no_failure() {
        let steps = vec![go_to(10, 64, 0)];
        let mut runner = TaskRunner::installed(3, 0);
        let _ = runner.on_tick(&steps, &tick(1, 0, [0.5, 64.0, 0.5]));
        // Wrong generation: discarded, request still outstanding.
        assert!(
            runner
                .on_path_outcome(1, 99, PathOutcome::Failed)
                .is_empty()
        );
        assert!(runner.path_outstanding());
        assert_eq!(runner.consecutive_failures(), 0);
        // Grid-not-ready deferral clears the request at no failure cost.
        runner.defer_outstanding();
        assert!(!runner.path_outstanding());
        assert_eq!(runner.consecutive_failures(), 0);
        assert_eq!(
            runner.on_tick(&steps, &tick(2, 0, [0.5, 64.0, 0.5])).len(),
            1
        );
    }

    #[test]
    fn path_result_moves_only_on_a_later_tick() {
        let steps = vec![go_to(1, 64, 0), go_to(2, 64, 0)];
        let mut runner = TaskRunner::installed(1, 0);
        let _ = runner.on_tick(&steps, &tick(1, 0, [0.5, 64.0, 0.5]));
        // Delivering the result produces no movement by itself.
        let delivered = runner.on_path_outcome(
            1,
            1,
            PathOutcome::Found {
                waypoints: vec![BlockPos::new(0, 64, 0), BlockPos::new(1, 64, 0)],
                revisions: Vec::new(),
            },
        );
        assert!(delivered.is_empty());
        // The next tick moves toward the first unreached waypoint.
        let moved = runner.on_tick(&steps, &tick(2, 0, [0.5, 64.0, 0.5]));
        assert!(matches!(moved.as_slice(), [RunnerEffect::Move(_)]));
        // Reaching the final waypoint of step one reports progress.
        let progress = runner.on_tick(&steps, &tick(3, 0, [1.5, 64.0, 0.5]));
        assert_eq!(progress, vec![RunnerEffect::StepProgress { step_index: 1 }]);
        // The second step requests a fresh path, then completes the task.
        let _ = runner.on_tick(&steps, &tick(4, 0, [1.5, 64.0, 0.5]));
        let _ = runner.on_path_outcome(
            4,
            1,
            PathOutcome::Found {
                waypoints: vec![BlockPos::new(2, 64, 0)],
                revisions: Vec::new(),
            },
        );
        let done = runner.on_tick(&steps, &tick(5, 0, [2.5, 64.0, 0.5]));
        assert_eq!(done, vec![RunnerEffect::Finish(TaskState::Completed)]);
    }

    #[test]
    fn stale_path_revisions_discard_without_strike_and_cool_down() {
        let steps = vec![go_to(10, 64, 0)];
        let mut runner = TaskRunner::installed(1, 0);
        let _ = runner.on_tick(&steps, &tick(10, 0, [0.5, 64.0, 0.5]));
        let _ = runner.on_path_outcome(
            10,
            1,
            PathOutcome::Found {
                waypoints: vec![BlockPos::new(5, 64, 0), BlockPos::new(10, 64, 0)],
                revisions: vec![(mornlea_domain::ChunkPos::new(0, 0), 5)],
            },
        );
        // The world underfoot changed after the result returned: the
        // path is discarded, a 20-tick cooldown starts, no strike lands.
        let mut changed = tick(11, 0, [0.5, 64.0, 0.5]);
        changed.current_revisions = vec![(mornlea_domain::ChunkPos::new(0, 0), 6)];
        assert!(runner.on_tick(&steps, &changed).is_empty());
        assert!(!runner.has_path());
        assert_eq!(runner.consecutive_failures(), 0);
        assert!(
            runner
                .on_tick(&steps, &tick(30, 0, [0.5, 64.0, 0.5]))
                .is_empty()
        );
        assert_eq!(
            runner.on_tick(&steps, &tick(31, 0, [0.5, 64.0, 0.5])).len(),
            1
        );
    }

    #[test]
    fn matching_path_revisions_consume_normally() {
        let steps = vec![go_to(10, 64, 0)];
        let mut runner = TaskRunner::installed(1, 0);
        let _ = runner.on_tick(&steps, &tick(10, 0, [0.5, 64.0, 0.5]));
        let _ = runner.on_path_outcome(
            10,
            1,
            PathOutcome::Found {
                waypoints: vec![BlockPos::new(5, 64, 0), BlockPos::new(10, 64, 0)],
                revisions: vec![(mornlea_domain::ChunkPos::new(0, 0), 5)],
            },
        );
        let mut current = tick(11, 0, [0.5, 64.0, 0.5]);
        current.current_revisions = vec![(mornlea_domain::ChunkPos::new(0, 0), 5)];
        let moved = runner.on_tick(&steps, &current);
        assert!(matches!(moved.as_slice(), [RunnerEffect::Move(_)]));

        // A missing current revision also fails closed, without a strike.
        let mut missing = tick(12, 0, [0.5, 64.0, 0.5]);
        missing.current_revisions = Vec::new();
        assert!(runner.on_tick(&steps, &missing).is_empty());
        assert!(!runner.has_path());
        assert_eq!(runner.consecutive_failures(), 0);
    }

    #[test]
    fn follow_moves_toward_target_and_holds_when_close() {
        let player = mornlea_domain::PlayerId::try_from_bytes([
            1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1,
        ])
        .unwrap();
        let steps = vec![PlanStep::Follow { player_id: player }];
        let mut runner = TaskRunner::installed(1, task_deadline_ticks(0, 10, true));
        assert_eq!(runner.deadline_ticks(), 0);
        let mut input = tick(1, 5000, [0.5, 64.0, 0.5]);
        input.follow = FollowObservation::Online {
            position: [10.5, 64.0, 0.5],
        };
        let request = runner.on_tick(&steps, &input);
        assert_eq!(
            request,
            vec![RunnerEffect::RequestPath {
                generation: 1,
                goal: BlockPos::new(10, 64, 0)
            }]
        );
        let _ = runner.on_path_outcome(
            1,
            1,
            PathOutcome::Found {
                waypoints: vec![BlockPos::new(5, 64, 0)],
                revisions: Vec::new(),
            },
        );
        let moved = runner.on_tick(&steps, &input);
        assert!(matches!(moved.as_slice(), [RunnerEffect::Move(_)]));
        // Inside follow distance the companion holds and never completes.
        input.position = [7.5, 64.0, 0.5];
        assert!(runner.on_tick(&steps, &input).is_empty());
        // Offline target fails the follow task as world changed.
        input.follow = FollowObservation::Away { offline: true };
        let failed = runner.on_tick(&steps, &input);
        assert_eq!(
            failed,
            vec![RunnerEffect::Finish(TaskState::Failed(
                TaskFailure::WorldChanged
            ))]
        );
    }
}

/// One path search the runtime must run: the tick-boundary grid built
/// from settled state, with the standing start and goal cells. The grid
/// is immutable after issue, so a later world change cannot leak into
/// the search.
#[derive(Debug)]
pub struct CompanionPathRequest {
    /// Companion the search belongs to.
    pub companion: CompanionId,
    /// Task generation the search belongs to; stale results are dropped.
    pub generation: u64,
    /// Standing cell the search starts from.
    pub start: BlockPos,
    /// Standing cell the search must reach.
    pub goal: BlockPos,
    /// Frozen tick-boundary path grid.
    pub grid: PathGrid,
}

/// One path outcome handed back to the authority at a tick boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct CompanionPathOutcome {
    /// Companion the outcome belongs to.
    pub companion: CompanionId,
    /// Task generation the outcome belongs to.
    pub generation: u64,
    /// The search result.
    pub outcome: PathOutcome,
}

/// Effects of one authority runner pass: movement actions to submit for
/// this tick and path searches for the runtime to run.
#[derive(Debug, Default)]
pub struct CompanionRunnerOutput {
    /// At most one movement action per companion.
    pub actions: Vec<(CompanionId, CompanionAction)>,
    /// At most one outstanding search per companion.
    pub path_requests: Vec<CompanionPathRequest>,
}

/// Builds the tick-boundary path grid for one companion from settled
/// state: the 33x9x33 window around the standing cell, every covered
/// chunk Ready. A window whose chunks are not all Ready returns `None`;
/// the caller defers the search to a later tick without recording a
/// failure, matching the Go grid construction.
pub fn build_path_grid(
    view: &super::state::AuthorityReadView<'_>,
    dimension: mornlea_domain::Dimension,
    center: BlockPos,
) -> Option<PathGrid> {
    use mornlea_engine::native::contracts::pathfind::{PathBlockTable, PathCell, PathRevision};

    const HORIZONTAL_RADIUS: i32 = 16;
    const VERTICAL_RADIUS: i32 = 4;
    const WORLD_MIN_Y: i32 = -64;
    const WORLD_MAX_Y: i32 = 320;

    // The window is always the fixed 33x9x33 around the centre, as in
    // Go: heights are never clamped, and cells outside the world
    // vertical range read as air in the fill below.
    let origin = PathCell {
        x: center.x().checked_sub(HORIZONTAL_RADIUS)?,
        y: center.y().checked_sub(VERTICAL_RADIUS)?,
        z: center.z().checked_sub(HORIZONTAL_RADIUS)?,
    };
    let size = [
        (2 * HORIZONTAL_RADIUS + 1) as u32,
        (2 * VERTICAL_RADIUS + 1) as u32,
        (2 * HORIZONTAL_RADIUS + 1) as u32,
    ];
    let chunk_origin = [origin.x >> 4, origin.z >> 4];
    let chunk_end = [
        (center.x().checked_add(HORIZONTAL_RADIUS)?) >> 4,
        (center.z().checked_add(HORIZONTAL_RADIUS)?) >> 4,
    ];
    let mut revisions = Vec::with_capacity(9);
    for cz in chunk_origin[1]..=chunk_end[1] {
        for cx in chunk_origin[0]..=chunk_end[0] {
            let key = super::contracts::ChunkKey {
                dimension,
                pos: mornlea_domain::ChunkPos::new(cx, cz),
            };
            revisions.push(PathRevision {
                chunk: [cx, cz],
                revision: view.ready_chunk_revision(key)?,
            });
        }
    }
    // Passable ids mirror the Go companion path block table: air, crops,
    // snow layers, the door upper half, torches, short grass and saplings
    // pass; everything else blocks.
    let mut passable: Vec<u16> = vec![0, 84, 89];
    passable.extend(37..=44u16);
    passable.extend(46..=53u16);
    passable.extend(54..=61u16);
    passable.push(70);
    passable.extend(71..=75u16);
    passable.extend(85..=88u16);
    let table = PathBlockTable::from_passable_ids(&passable).ok()?;
    let cells = size[0] as usize * size[1] as usize * size[2] as usize;
    let mut blocks = vec![u16::MAX; cells];
    for lx in 0..size[0] as i32 {
        for lz in 0..size[2] as i32 {
            for ly in 0..size[1] as i32 {
                let pos = BlockPos::new(origin.x + lx, origin.y + ly, origin.z + lz);
                if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&pos.y()) {
                    // Out-of-world heights read as air, as the Go chunk
                    // read normalises them.
                    blocks[(lx as usize * size[2] as usize + lz as usize) * size[1] as usize
                        + ly as usize] = 0;
                } else if let Some(observed) = view.observation(dimension, pos) {
                    let index = (lx as usize * size[2] as usize + lz as usize) * size[1] as usize
                        + ly as usize;
                    blocks[index] = observed.block;
                }
            }
        }
    }
    PathGrid::try_new(origin, size, blocks.into_boxed_slice(), table, revisions).ok()
}

#[cfg(test)]
mod interaction_tests {
    //! Pure mine/place executor transitions, scenario by scenario against
    //! the Go interaction runner (`advanceInteractionRunner`,
    //! `holdCompanionMining`, `submitCompanionPlacement`,
    //! `releaseFinishedMining` in `packages/server/server/companion_interact.go`).
    use super::*;
    use crate::core::contracts::PlanBlock;

    const AT_GOAL: [f32; 3] = [9.5, 0.0, 8.5];

    fn target_pos() -> BlockPos {
        BlockPos::new(10, 0, 8)
    }

    fn goal_pos() -> BlockPos {
        BlockPos::new(9, 0, 8)
    }

    fn mine() -> PlanStep {
        PlanStep::Mine { x: 10, y: 0, z: 8 }
    }

    fn place() -> PlanStep {
        PlanStep::Place {
            x: 10,
            y: 0,
            z: 8,
            block: PlanBlock::Dirt,
        }
    }

    fn observed(tick: u64, position: [f32; 3], interaction: InteractionObservation) -> RunnerTick {
        RunnerTick {
            tick,
            world_time: 0,
            position,
            follow: FollowObservation::NotFollow,
            current_revisions: Vec::new(),
            interaction,
        }
    }

    fn sight(block: Option<u16>) -> InteractionObservation {
        InteractionObservation {
            approach_goal: goal_pos(),
            target_block: block,
            mining: None,
            capacity_exceeded: false,
            holds_item: true,
            eye_height: 1.62,
            reach: 6.0,
        }
    }

    fn mining(elapsed: u32, required: u32) -> Option<MiningObservation> {
        Some(MiningObservation {
            target: target_pos(),
            elapsed,
            required,
        })
    }

    /// Drives the approach: request toward the adjacent goal, deliver a
    /// one-cell path, then the arrival tick switches to interaction
    /// without submitting anything.
    fn approached(steps: &[PlanStep], runner: &mut TaskRunner) {
        let request = runner.on_tick(steps, &observed(1, AT_GOAL, sight(Some(3))));
        assert_eq!(
            request,
            vec![RunnerEffect::RequestPath {
                generation: 1,
                goal: goal_pos()
            }]
        );
        let _ = runner.on_path_outcome(
            1,
            1,
            PathOutcome::Found {
                waypoints: vec![goal_pos()],
                revisions: Vec::new(),
            },
        );
        assert!(
            runner
                .on_tick(steps, &observed(2, AT_GOAL, sight(Some(3))))
                .is_empty(),
            "the arrival tick submits no action"
        );
        assert!(runner.interaction_ready());
    }

    #[test]
    fn mine_step_paths_to_the_adjacent_cell_before_holding() {
        let steps = vec![mine()];
        let mut runner = TaskRunner::installed(1, 0);
        approached(&steps, &mut runner);
        let hold = runner.on_tick(&steps, &observed(3, AT_GOAL, sight(Some(3))));
        assert_eq!(
            hold,
            vec![RunnerEffect::Interact(CompanionAction::MineHold {
                target: target_pos()
            })]
        );
        // The interaction phase never asks for another path.
        let mut progressing = sight(Some(3));
        progressing.mining = mining(1, 5);
        let again = runner.on_tick(&steps, &observed(4, AT_GOAL, progressing));
        assert_eq!(
            again,
            vec![RunnerEffect::Interact(CompanionAction::MineHold {
                target: target_pos()
            })]
        );
    }

    #[test]
    fn mine_completion_needs_progress_evidence_else_world_changed() {
        let steps = vec![mine()];
        let mut runner = TaskRunner::installed(1, 0);
        approached(&steps, &mut runner);
        for elapsed in 1..=4 {
            let mut progressing = sight(Some(3));
            progressing.mining = mining(elapsed, 5);
            let _ = runner.on_tick(
                &steps,
                &observed(2 + u64::from(elapsed), AT_GOAL, progressing),
            );
        }
        // The settlement tick cleared progress and emptied the cell.
        let done = runner.on_tick(&steps, &observed(9, AT_GOAL, sight(Some(0))));
        assert_eq!(done, vec![RunnerEffect::Finish(TaskState::Completed)]);

        // Without progress evidence an emptied target is a world change.
        let mut other = TaskRunner::installed(1, 0);
        approached(&steps, &mut other);
        let removed = other.on_tick(&steps, &observed(3, AT_GOAL, sight(Some(0))));
        assert_eq!(
            removed,
            vec![RunnerEffect::Finish(TaskState::Failed(
                TaskFailure::WorldChanged
            ))]
        );
    }

    #[test]
    fn mine_progress_regression_fails_world_changed() {
        let steps = vec![mine()];
        let mut runner = TaskRunner::installed(1, 0);
        approached(&steps, &mut runner);
        let mut progressing = sight(Some(2));
        progressing.mining = mining(3, 30);
        let _ = runner.on_tick(&steps, &observed(3, AT_GOAL, progressing));
        // The target was replaced: progress restarts and the timing rule
        // changes, so the task fails rather than mining the new block.
        let mut replaced = sight(Some(3));
        replaced.mining = mining(1, 5);
        assert_eq!(
            runner.on_tick(&steps, &observed(4, AT_GOAL, replaced)),
            vec![RunnerEffect::Finish(TaskState::Failed(
                TaskFailure::WorldChanged
            ))]
        );
    }

    #[test]
    fn saturated_mine_without_capacity_fails_inventory_full() {
        let steps = vec![mine()];
        let mut runner = TaskRunner::installed(1, 0);
        approached(&steps, &mut runner);
        let mut saturated = sight(Some(3));
        saturated.mining = mining(5, 5);
        // Capacity still available: keep holding (the settlement retries).
        assert_eq!(
            runner.on_tick(&steps, &observed(3, AT_GOAL, saturated)),
            vec![RunnerEffect::Interact(CompanionAction::MineHold {
                target: target_pos()
            })]
        );
        saturated.capacity_exceeded = true;
        assert_eq!(
            runner.on_tick(&steps, &observed(4, AT_GOAL, saturated)),
            vec![RunnerEffect::Finish(TaskState::Failed(
                TaskFailure::InventoryFull
            ))]
        );
    }

    #[test]
    fn advancing_past_a_mine_step_releases_the_hold() {
        let steps = vec![mine(), PlanStep::GoTo { x: 9, y: 0, z: 8 }];
        let mut runner = TaskRunner::installed(1, 0);
        approached(&steps, &mut runner);
        let mut last = sight(Some(3));
        last.mining = mining(4, 5);
        let _ = runner.on_tick(&steps, &observed(3, AT_GOAL, last));
        let advanced = runner.on_tick(&steps, &observed(4, AT_GOAL, sight(Some(0))));
        assert_eq!(
            advanced,
            vec![
                RunnerEffect::StepProgress { step_index: 1 },
                RunnerEffect::Interact(CompanionAction::MineRelease),
            ]
        );
    }

    #[test]
    fn unready_target_chunk_waits_without_judgement() {
        let steps = vec![mine()];
        let mut runner = TaskRunner::installed(1, 0);
        approached(&steps, &mut runner);
        assert!(
            runner
                .on_tick(&steps, &observed(3, AT_GOAL, sight(None)))
                .is_empty()
        );
        assert!(!runner.is_finished());
    }

    #[test]
    fn place_submits_once_in_reach_and_completes_on_observed_block() {
        let steps = vec![place()];
        let mut runner = TaskRunner::installed(1, 0);
        approached(&steps, &mut runner);
        assert_eq!(
            runner.on_tick(&steps, &observed(3, AT_GOAL, sight(Some(0)))),
            vec![RunnerEffect::Interact(CompanionAction::Place {
                target: target_pos(),
                block: 3
            })]
        );
        assert_eq!(
            runner.on_tick(&steps, &observed(4, AT_GOAL, sight(Some(3)))),
            vec![RunnerEffect::Finish(TaskState::Completed)]
        );
    }

    #[test]
    fn place_out_of_reach_waits_without_submitting() {
        let steps = vec![place()];
        let mut runner = TaskRunner::installed(1, 0);
        approached(&steps, &mut runner);
        let mut short = sight(Some(0));
        short.reach = 0.5;
        assert!(
            runner
                .on_tick(&steps, &observed(3, AT_GOAL, short))
                .is_empty()
        );
        assert!(!runner.is_finished());
    }

    #[test]
    fn place_failure_reasons_follow_go() {
        let steps = vec![place()];
        let mut occupied = TaskRunner::installed(1, 0);
        approached(&steps, &mut occupied);
        assert_eq!(
            occupied.on_tick(&steps, &observed(3, AT_GOAL, sight(Some(2)))),
            vec![RunnerEffect::Finish(TaskState::Failed(
                TaskFailure::WorldChanged
            ))]
        );
        let mut empty = TaskRunner::installed(1, 0);
        approached(&steps, &mut empty);
        let mut without = sight(Some(0));
        without.holds_item = false;
        assert_eq!(
            empty.on_tick(&steps, &observed(3, AT_GOAL, without)),
            vec![RunnerEffect::Finish(TaskState::Failed(
                TaskFailure::InventoryFull
            ))]
        );
    }

    #[test]
    fn timeout_during_mine_still_times_out() {
        let steps = vec![mine()];
        let mut runner = TaskRunner::installed(1, 1200);
        approached(&steps, &mut runner);
        let mut late = observed(3, AT_GOAL, sight(Some(3)));
        late.interaction.mining = None;
        late.world_time = 1200;
        assert_eq!(
            runner.on_tick(&steps, &late),
            vec![RunnerEffect::Finish(TaskState::TimedOut)]
        );
    }

    #[test]
    fn restored_interaction_restarts_from_the_approach() {
        let steps = vec![PlanStep::GoTo { x: 9, y: 0, z: 8 }, mine()];
        let mut runner = TaskRunner::restored(1, 1, 0);
        assert!(!runner.interaction_ready());
        // Even with sim progress visible, a restored step re-approaches.
        let mut visible = sight(Some(3));
        visible.mining = mining(3, 5);
        assert_eq!(
            runner.on_tick(&steps, &observed(1, AT_GOAL, visible)),
            vec![RunnerEffect::RequestPath {
                generation: 1,
                goal: goal_pos()
            }]
        );
    }

    #[test]
    fn interaction_goal_order_and_fallback_follow_go() {
        // Only +Z at the same layer stands: -X, +X and -Z feet are solid.
        let world = |pos: BlockPos| -> Option<u16> {
            if pos.y() < 0 {
                return Some(2);
            }
            if pos.y() == 0
                && (pos == BlockPos::new(9, 0, 8)
                    || pos == BlockPos::new(11, 0, 8)
                    || pos == BlockPos::new(10, 0, 7))
            {
                return Some(2);
            }
            Some(0)
        };
        assert_eq!(
            interaction_goal(target_pos(), world),
            BlockPos::new(10, 0, 9)
        );
        // Same layer first, -X before +X.
        assert_eq!(
            interaction_goal(target_pos(), |pos| Some(if pos.y() < 0 { 2 } else { 0 })),
            goal_pos()
        );
        // Nothing stands (everything unready): the +X sentinel.
        assert_eq!(
            interaction_goal(target_pos(), |_| None),
            BlockPos::new(11, 0, 8)
        );
    }

    #[test]
    fn upper_layer_wins_over_lower_when_the_same_layer_is_blocked() {
        // -X column: solid up to y=0, so (9,1,8) stands one layer up.
        // +X column: solid only at y=-2, so (11,-1,8) stands one layer down.
        // -Z and +Z columns are solid throughout. No same-layer cell stands
        // (-X feet solid, +X has no support), so Go picks dy=+1 before dy=-1.
        let world = |pos: BlockPos| -> Option<u16> {
            match (pos.x(), pos.z()) {
                (9, 8) => Some(if pos.y() <= 0 { 2 } else { 0 }),
                (11, 8) => Some(if pos.y() == -2 { 2 } else { 0 }),
                (10, 7) | (10, 9) => Some(2),
                _ => Some(if pos.y() < -2 { 2 } else { 0 }),
            }
        };
        assert_eq!(
            interaction_goal(target_pos(), world),
            BlockPos::new(9, 1, 8)
        );
    }

    /// Observes `first`, then `second`, as consecutive mining ticks.
    fn mine_twice(first: (u32, u32), second: (u32, u32)) -> Vec<RunnerEffect> {
        let steps = vec![mine()];
        let mut runner = TaskRunner::installed(1, 0);
        approached(&steps, &mut runner);
        let mut seen = sight(Some(3));
        seen.mining = mining(first.0, first.1);
        let _ = runner.on_tick(&steps, &observed(3, AT_GOAL, seen));
        let mut next = sight(Some(3));
        next.mining = mining(second.0, second.1);
        runner.on_tick(&steps, &observed(4, AT_GOAL, next))
    }

    #[test]
    fn changed_required_ticks_fail_world_changed_without_regression() {
        let world_changed = vec![RunnerEffect::Finish(TaskState::Failed(
            TaskFailure::WorldChanged,
        ))];
        // Elapsed holds or grows, but the timing rule changed: another
        // block/tool key now accumulates on the target.
        assert_eq!(mine_twice((1, 30), (1, 5)), world_changed);
        assert_eq!(mine_twice((1, 30), (2, 5)), world_changed);
        // Same rule, growing progress: keep holding.
        assert_eq!(
            mine_twice((1, 30), (2, 30)),
            vec![RunnerEffect::Interact(CompanionAction::MineHold {
                target: target_pos()
            })]
        );
    }

    /// Observes progress `(elapsed, 5)`, then the target emptied with the
    /// progress gone.
    fn emptied_after(elapsed: u32) -> Vec<RunnerEffect> {
        let steps = vec![mine()];
        let mut runner = TaskRunner::installed(1, 0);
        approached(&steps, &mut runner);
        let mut seen = sight(Some(3));
        seen.mining = mining(elapsed, 5);
        let _ = runner.on_tick(&steps, &observed(3, AT_GOAL, seen));
        runner.on_tick(&steps, &observed(4, AT_GOAL, sight(Some(0))))
    }

    #[test]
    fn emptied_target_completes_only_one_tick_short_of_required() {
        let world_changed = vec![RunnerEffect::Finish(TaskState::Failed(
            TaskFailure::WorldChanged,
        ))];
        assert_eq!(emptied_after(2), world_changed);
        assert_eq!(emptied_after(3), world_changed);
        assert_eq!(
            emptied_after(4),
            vec![RunnerEffect::Finish(TaskState::Completed)]
        );
    }

    #[test]
    fn a_second_place_step_approaches_its_own_target_first() {
        let second_goal = BlockPos::new(12, 0, 5);
        let steps = vec![
            place(),
            PlanStep::Place {
                x: 13,
                y: 0,
                z: 5,
                block: PlanBlock::Dirt,
            },
        ];
        let mut runner = TaskRunner::installed(1, 0);
        approached(&steps, &mut runner);
        assert_eq!(
            runner.on_tick(&steps, &observed(3, AT_GOAL, sight(Some(0)))),
            vec![RunnerEffect::Interact(CompanionAction::Place {
                target: target_pos(),
                block: 3
            })]
        );
        assert_eq!(
            runner.on_tick(&steps, &observed(4, AT_GOAL, sight(Some(3)))),
            vec![RunnerEffect::StepProgress { step_index: 1 }]
        );
        // The second step starts from its own approach: no inherited
        // in-position state, so it paths before any placement.
        let mut next = sight(Some(0));
        next.approach_goal = second_goal;
        assert_eq!(
            runner.on_tick(&steps, &observed(5, AT_GOAL, next)),
            vec![RunnerEffect::RequestPath {
                generation: 1,
                goal: second_goal
            }]
        );
        assert!(!runner.interaction_ready());
    }
}
