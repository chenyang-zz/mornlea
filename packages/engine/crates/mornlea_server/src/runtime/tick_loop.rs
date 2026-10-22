//! The production single-tick assembly.
//!
//! One [`ServerTick::step`] is one authoritative tick in the Go server
//! order (`server.go` `step`: companion task orchestration before the
//! engine step, publication after it):
//!
//! 1. the companion plan loop runs exactly once: planner outcomes, then
//!    expiry and the task runner pass, then new planning dispatch;
//! 2. the runner actions of that pass are submitted through the core
//!    companion fence, so this tick's step consumes them;
//! 3. the authority advances one tick;
//! 4. the tick is published to the session outboxes. The authority
//!    publishes inside `advance_tick`, so this step adds no second
//!    publication call: one would deliver every frame twice.
//!
//! Session input is queued in the core when it arrives, before the step.
//! Values that steer the tick are frozen at startup in [`TickStartup`] and
//! handed down by value; nothing here reads configuration or the
//! environment. Tick pacing, the host clock and dropped-tick policy are
//! not part of this assembly.

use mornlea_domain::CompanionId;

use crate::contracts::{ServerError, ServerPhase, TickBudget, TickPublication};
use crate::core::companion_runner::{RunnerSubmission, TaskTimeoutMinutes};
use crate::runtime::companion::CompanionRuntime;
use crate::runtime::companion_planning::{
    CompanionPlanLoop, CompanionPlanningPorts, CompanionPlanningReport,
};
use crate::runtime::companion_runner::PendingCompanionPaths;
use crate::state::AuthorityState;

/// Values frozen once at startup and never re-read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TickStartup {
    task_timeout: TaskTimeoutMinutes,
    budget: TickBudget,
}

impl TickStartup {
    /// Startup values from already frozen parts.
    pub fn new(task_timeout: TaskTimeoutMinutes, budget: TickBudget) -> Self {
        Self {
            task_timeout,
            budget,
        }
    }

    /// Startup values for a started companion runtime: its frozen task
    /// timeout is read exactly once, here.
    pub fn for_companions(runtime: &CompanionRuntime, budget: TickBudget) -> Self {
        Self::new(runtime.task_timeout_minutes(), budget)
    }

    /// The frozen companion task timeout.
    pub fn task_timeout(&self) -> TaskTimeoutMinutes {
        self.task_timeout
    }

    /// The per-tick work budget.
    pub fn budget(&self) -> TickBudget {
        self.budget
    }
}

/// What one tick did.
#[derive(Debug, PartialEq)]
pub struct TickReport {
    /// The published tick.
    pub publication: TickPublication,
    /// The plan loop pass, including the runner actions it emitted.
    pub planning: CompanionPlanningReport,
    /// How the core fence took those runner actions.
    pub submission: RunnerSubmission,
    /// Path searches issued this tick; their outcomes are delivered to the
    /// runner on the next tick, never this one.
    pub path_searches: usize,
}

/// The tick assembly: the plan loop and the path searches it holds across
/// tick boundaries, over the startup-frozen values.
pub struct ServerTick {
    budget: TickBudget,
    plan_loop: CompanionPlanLoop,
    pending_paths: PendingCompanionPaths,
    companions: Vec<CompanionId>,
}

impl ServerTick {
    /// Builds the assembly for the configured companions. The frozen
    /// timeout moves into the plan loop here and has no setter.
    pub fn new(startup: TickStartup, companions: Vec<CompanionId>) -> Self {
        Self {
            budget: startup.budget(),
            plan_loop: CompanionPlanLoop::new(startup.task_timeout()),
            pending_paths: PendingCompanionPaths::new(),
            companions,
        }
    }

    /// The task timeout every install of this assembly stamps.
    pub fn task_timeout(&self) -> TaskTimeoutMinutes {
        self.plan_loop.task_timeout()
    }

    /// Runs one tick in the order in the module docs.
    ///
    /// A failed or closing authority refuses before any stage runs, with
    /// the error `advance_tick` itself would return (the retained tick
    /// failure first, then the phase). Any authority error from a later
    /// stage is also returned unchanged; no companion task is failed or
    /// dropped because of it.
    pub fn step(
        &mut self,
        state: &mut AuthorityState,
        ports: CompanionPlanningPorts<'_>,
    ) -> Result<TickReport, ServerError> {
        if let Some(error) = state.tick_failure() {
            return Err(error);
        }
        if state.phase() != ServerPhase::Running {
            return Err(ServerError::InvalidState {
                phase: state.phase(),
            });
        }
        let planning =
            self.plan_loop
                .drive(state, &self.companions, &mut self.pending_paths, ports)?;
        let path_searches = self.pending_paths.len();
        let submission = state.submit_runner_actions(planning.actions.clone())?;
        let publication = state.advance_tick(self.budget)?;
        Ok(TickReport {
            publication,
            planning,
            submission,
            path_searches,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::companion_planning::tests::{
        full_tick_budget, give_players_inventories,
    };
    use crate::runtime::companion_planning::tests::{Rig, go_home_plan, queued_fixture, timeout};

    /// The planning fixture with an empty inventory for every player, so
    /// the authority can run whole ticks over it.
    fn tickable_fixture() -> (
        crate::core::state::companion_planning::tests::PlanningFixture,
        CompanionId,
    ) {
        let (mut fixture, id) = queued_fixture();
        give_players_inventories(&mut fixture.authority);
        (fixture, id)
    }

    fn server_tick(companion: CompanionId) -> ServerTick {
        ServerTick::new(
            TickStartup::new(timeout(), full_tick_budget()),
            vec![companion],
        )
    }

    /// The plan loop reads the authority before the tick advances: the
    /// planning snapshot names the tick this step then publishes.
    #[test]
    fn plan_loop_runs_before_the_tick_advances() {
        let (mut fixture, id) = tickable_fixture();
        let mut tick = server_tick(id);
        let mut rig = Rig::new();
        let before = fixture.authority.next_tick();
        let report = rig
            .with_ports(|ports| tick.step(&mut fixture.authority, ports))
            .unwrap();
        assert_eq!(report.planning.dispatched, vec![id]);
        let requests = rig.agent().plan_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(rig.snapshots().source_ticks, vec![before]);
        assert_eq!(report.publication.tick, before);
        assert_eq!(fixture.authority.next_tick(), before + 1);
    }

    /// Runner actions are submitted before the tick advances, so the step
    /// that emitted them also consumes them and leaves nothing queued.
    #[test]
    fn runner_actions_are_consumed_by_the_same_step() {
        let (mut fixture, id) = tickable_fixture();
        let mut tick = server_tick(id);
        let mut rig = Rig::new();
        rig.with_ports(|ports| tick.step(&mut fixture.authority, ports))
            .unwrap();
        let request = rig.agent().plan_requests()[0].clone();
        rig.agent().complete(&request, go_home_plan());
        let mut moved = false;
        for _ in 0..8 {
            let report = rig
                .with_ports(|ports| tick.step(&mut fixture.authority, ports))
                .unwrap();
            if report.planning.actions.is_empty() {
                continue;
            }
            assert_eq!(report.submission.admitted, vec![id]);
            assert!(
                fixture.authority.drain_companions(usize::MAX).is_empty(),
                "the emitting step consumed its actions"
            );
            moved = true;
            break;
        }
        assert!(moved, "the runner emitted a movement action");
    }

    /// A failed or closing authority refuses before any stage runs, with
    /// the error the tick itself would return: no planning is dispatched
    /// and no tick advances.
    #[test]
    fn unhealthy_authority_refuses_before_any_stage() {
        let (mut fixture, id) = tickable_fixture();
        let mut tick = server_tick(id);
        let mut rig = Rig::new();
        let before = fixture.authority.next_tick();
        let failure = ServerError::Internal {
            invariant: "tick loop test failure",
        };
        let retained = fixture.authority.fail_tick(failure);
        assert_eq!(
            rig.with_ports(|ports| tick.step(&mut fixture.authority, ports)),
            Err(retained)
        );
        assert!(rig.agent().plan_requests().is_empty());
        assert_eq!(fixture.authority.next_tick(), before);

        let (mut fixture, id) = tickable_fixture();
        let mut tick = server_tick(id);
        assert!(fixture.authority.begin_close());
        assert_eq!(
            rig.with_ports(|ports| tick.step(&mut fixture.authority, ports)),
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing
            })
        );
        assert!(rig.agent().plan_requests().is_empty());
    }
}
