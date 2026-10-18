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

use mornlea_domain::{BlockPos, CompanionId, TaskFailure, TaskState};
use mornlea_engine::native::contracts::pathfind::PathGrid;

use super::contracts::{CompanionAction, PlanStep};

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

/// A path outcome delivered at a tick boundary. Both search failures
/// (unreachable and budget exhausted) share this one failure form,
/// matching the Go runner, which handles them identically.
#[derive(Clone, Debug, PartialEq)]
pub enum PathOutcome {
    /// Ordered waypoints from start to goal.
    Found(Vec<BlockPos>),
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

/// Inputs for one runner tick, all read by the caller at the boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunnerTick {
    /// Completed-tick count at this boundary.
    pub tick: u64,
    /// Authoritative world time in ticks.
    pub world_time: u64,
    /// Companion position at this boundary.
    pub position: [f32; 3],
    /// Follow target observation for the current step.
    pub follow: FollowObservation,
}

/// One effect the caller applies, in the order returned.
#[derive(Clone, Debug, PartialEq)]
pub enum RunnerEffect {
    /// Ask the runtime to search for a path to this goal. At most one
    /// request per task is outstanding at any time.
    RequestPath { generation: u64, goal: BlockPos },
    /// Submit this single movement action for the companion this tick.
    Move(CompanionAction),
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
    waypoint: usize,
    consecutive_failures: u32,
    replan_at: Option<u64>,
    outstanding: bool,
    follow_goal: Option<BlockPos>,
    finished: bool,
}

impl TaskRunner {
    /// Fresh state for a task entering Running with its stamped deadline.
    pub fn installed(generation: u64, deadline_ticks: u64) -> Self {
        Self {
            generation,
            step_index: 0,
            deadline_ticks,
            path: None,
            waypoint: 0,
            consecutive_failures: 0,
            replan_at: None,
            outstanding: false,
            follow_goal: None,
            finished: false,
        }
    }

    /// State restored from a stored running task: step index and deadline
    /// carry over, while the failure budget starts full, matching the Go
    /// restore discipline.
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
            PathOutcome::Found(waypoints) => {
                self.path = Some(waypoints);
                self.waypoint = 0;
                self.replan_at = None;
                Vec::new()
            }
            PathOutcome::Failed => {
                self.path = None;
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
    /// movement, matching the Go tick order.
    pub fn on_tick(&mut self, steps: &[PlanStep], input: RunnerTick) -> Vec<RunnerEffect> {
        if self.finished {
            return Vec::new();
        }
        if self.deadline_ticks != 0 && input.world_time >= self.deadline_ticks {
            self.finished = true;
            self.path = None;
            return vec![RunnerEffect::Finish(TaskState::TimedOut)];
        }
        let Some(step) = steps.get(self.step_index) else {
            return Vec::new();
        };
        let goal = match step {
            PlanStep::GoTo { x, y, z } => BlockPos::new(*x, *y, *z),
            PlanStep::Follow { .. } => match self.follow_goal_for(input) {
                Ok(goal) => goal,
                Err(effects) => return effects,
            },
            // Mine and place steps belong to the interaction executor,
            // which is a separate slice; the runner leaves them untouched.
            PlanStep::Mine { .. } | PlanStep::Place { .. } => return Vec::new(),
        };
        let is_follow = matches!(step, PlanStep::Follow { .. });
        if self.outstanding {
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
        let mut consumed = false;
        while self.waypoint < path.len() && arrived(input.position, path[self.waypoint]) {
            self.waypoint += 1;
            self.consecutive_failures = 0;
            consumed = true;
        }
        if self.waypoint >= path.len() {
            self.path = None;
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
            self.step_index += 1;
            if self.step_index >= steps.len() {
                self.finished = true;
                return vec![RunnerEffect::Finish(TaskState::Completed)];
            }
            return vec![RunnerEffect::StepProgress {
                step_index: self.step_index,
            }];
        }
        let _ = consumed;
        vec![RunnerEffect::Move(move_toward(
            input.position,
            path[self.waypoint],
        ))]
    }

    /// Resolves the dynamic follow goal for this tick, or the effects
    /// that end or hold the task when there is no goal to chase.
    fn follow_goal_for(&mut self, input: RunnerTick) -> Result<BlockPos, Vec<RunnerEffect>> {
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
    fn timeout_is_pure_tick_comparison() {
        let steps = vec![go_to(3, 64, 0)];
        let mut runner = TaskRunner::installed(1, task_deadline_ticks(100, 1, false));
        let before = runner.on_tick(&steps, tick(10, 100 + 1199, [0.5, 64.0, 0.5]));
        assert!(!matches!(before.as_slice(), [RunnerEffect::Finish(_)]));
        let at = runner.on_tick(&steps, tick(11, 100 + 1200, [0.5, 64.0, 0.5]));
        assert_eq!(at, vec![RunnerEffect::Finish(TaskState::TimedOut)]);
        // A zero deadline never expires.
        let mut exempt = TaskRunner::installed(1, 0);
        let never = exempt.on_tick(&steps, tick(12, u64::MAX - 1, [0.5, 64.0, 0.5]));
        assert!(!matches!(never.as_slice(), [RunnerEffect::Finish(_)]));
    }

    #[test]
    fn single_outstanding_and_cooldown_and_third_failure() {
        let steps = vec![go_to(10, 64, 0)];
        let mut runner = TaskRunner::installed(7, 0);
        let first = runner.on_tick(&steps, tick(100, 0, [0.5, 64.0, 0.5]));
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
                .on_tick(&steps, tick(101, 0, [0.5, 64.0, 0.5]))
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
                .on_tick(&steps, tick(120, 0, [0.5, 64.0, 0.5]))
                .is_empty()
        );
        assert_eq!(
            runner.on_tick(&steps, tick(121, 0, [0.5, 64.0, 0.5])).len(),
            1
        );
        assert!(
            runner
                .on_path_outcome(121, 7, PathOutcome::Failed)
                .is_empty()
        );
        assert_eq!(
            runner.on_tick(&steps, tick(141, 0, [0.5, 64.0, 0.5])).len(),
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
            next.on_tick(&steps, tick(142, 0, [0.5, 64.0, 0.5])).len(),
            1
        );
    }

    #[test]
    fn stale_outcome_discarded_and_defer_costs_no_failure() {
        let steps = vec![go_to(10, 64, 0)];
        let mut runner = TaskRunner::installed(3, 0);
        let _ = runner.on_tick(&steps, tick(1, 0, [0.5, 64.0, 0.5]));
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
            runner.on_tick(&steps, tick(2, 0, [0.5, 64.0, 0.5])).len(),
            1
        );
    }

    #[test]
    fn path_result_moves_only_on_a_later_tick() {
        let steps = vec![go_to(1, 64, 0), go_to(2, 64, 0)];
        let mut runner = TaskRunner::installed(1, 0);
        let _ = runner.on_tick(&steps, tick(1, 0, [0.5, 64.0, 0.5]));
        // Delivering the result produces no movement by itself.
        let delivered = runner.on_path_outcome(
            1,
            1,
            PathOutcome::Found(vec![BlockPos::new(0, 64, 0), BlockPos::new(1, 64, 0)]),
        );
        assert!(delivered.is_empty());
        // The next tick moves toward the first unreached waypoint.
        let moved = runner.on_tick(&steps, tick(2, 0, [0.5, 64.0, 0.5]));
        assert!(matches!(moved.as_slice(), [RunnerEffect::Move(_)]));
        // Reaching the final waypoint of step one reports progress.
        let progress = runner.on_tick(&steps, tick(3, 0, [1.5, 64.0, 0.5]));
        assert_eq!(progress, vec![RunnerEffect::StepProgress { step_index: 1 }]);
        // The second step requests a fresh path, then completes the task.
        let _ = runner.on_tick(&steps, tick(4, 0, [1.5, 64.0, 0.5]));
        let _ = runner.on_path_outcome(4, 1, PathOutcome::Found(vec![BlockPos::new(2, 64, 0)]));
        let done = runner.on_tick(&steps, tick(5, 0, [2.5, 64.0, 0.5]));
        assert_eq!(done, vec![RunnerEffect::Finish(TaskState::Completed)]);
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
        let request = runner.on_tick(&steps, input);
        assert_eq!(
            request,
            vec![RunnerEffect::RequestPath {
                generation: 1,
                goal: BlockPos::new(10, 64, 0)
            }]
        );
        let _ = runner.on_path_outcome(1, 1, PathOutcome::Found(vec![BlockPos::new(5, 64, 0)]));
        let moved = runner.on_tick(&steps, input);
        assert!(matches!(moved.as_slice(), [RunnerEffect::Move(_)]));
        // Inside follow distance the companion holds and never completes.
        input.position = [7.5, 64.0, 0.5];
        assert!(runner.on_tick(&steps, input).is_empty());
        // Offline target fails the follow task as world changed.
        input.follow = FollowObservation::Away { offline: true };
        let failed = runner.on_tick(&steps, input);
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

    let origin = PathCell {
        x: center.x().checked_sub(HORIZONTAL_RADIUS)?,
        y: center.y().checked_sub(VERTICAL_RADIUS)?.max(WORLD_MIN_Y),
        z: center.z().checked_sub(HORIZONTAL_RADIUS)?,
    };
    let top_y = center
        .y()
        .checked_add(VERTICAL_RADIUS)?
        .min(WORLD_MAX_Y - 1);
    if top_y < origin.y {
        return None;
    }
    let size = [
        (2 * HORIZONTAL_RADIUS + 1) as u32,
        u32::try_from(top_y - origin.y + 1).ok()?,
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
                if let Some(observed) = view.observation(dimension, pos) {
                    let index = (lx as usize * size[2] as usize + lz as usize) * size[1] as usize
                        + ly as usize;
                    blocks[index] = observed.block;
                }
            }
        }
    }
    PathGrid::try_new(origin, size, blocks.into_boxed_slice(), table, revisions).ok()
}
