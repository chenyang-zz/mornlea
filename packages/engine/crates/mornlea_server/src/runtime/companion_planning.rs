//! Runtime assembly for the companion plan loop.
//!
//! The loop owns no authority state and performs no model work itself:
//! the Agent host dispatches and polls off the tick, outcomes are settled
//! at the tick boundary, the core installs or finishes tasks through its
//! checked seams, and the task runner owns all movement. One drive is one
//! tick-boundary pass in the Go order: apply planner outcomes, expire and
//! run tasks (inside the runner drive), then dispatch new planning. The
//! dialogue and memory lanes keep their place in that order as
//! placeholders only; this assembly does not drive them.

use std::collections::BTreeMap;

use mornlea_domain::{CompanionId, TaskFailure, TaskState};

use crate::agent::host::{InstallReject, PlanDispatch, PlanFailKind, PlanHost};
use crate::contracts::{
    AgentHandle, AgentRequestId, ClientInstanceId, Clock, LeaseId, NamespaceId, RunId, ServerError,
    SnapshotPort,
};
use crate::core::companion_chat::CompanionChatPhase;
use crate::runtime::companion_runner::{PendingCompanionPaths, drive_companion_runners};
use crate::state::AuthorityState;

/// Bare identities and service handles one drive borrows. The lease is a
/// plain frozen value read from the lease owner by the caller; identity
/// minting stays with the caller so the loop never reaches for entropy.
pub struct CompanionPlanningPorts<'a> {
    /// Agent business handle (dispatch and poll).
    pub agent: &'a mut dyn AgentHandle,
    /// Frozen-snapshot registry port.
    pub snapshots: &'a mut dyn SnapshotPort,
    /// Clock for deadlines; the loop reads no other time source.
    pub clock: &'a dyn Clock,
    /// Client instance presented on every Agent request.
    pub client: ClientInstanceId,
    /// Agent namespace from the durable companion aggregate.
    pub namespace: NamespaceId,
    /// Current lease and fence, if the lease owner holds one.
    pub lease: Option<(LeaseId, u64)>,
    /// Mints raw UUIDv4 bytes for request and run identities.
    pub mint_id: &'a mut dyn FnMut() -> Result<[u8; 16], ServerError>,
}

/// What one drive did, for the caller and for tests.
#[derive(Debug, Default, PartialEq)]
pub struct CompanionPlanningReport {
    /// Movement actions the task runner emitted this pass.
    pub actions: Vec<(CompanionId, crate::contracts::CompanionAction)>,
    /// Companions a new planning request was dispatched for.
    pub dispatched: Vec<CompanionId>,
    /// Companions whose plan was installed into the core this pass.
    pub installed: Vec<CompanionId>,
    /// Companions whose current task was finished as failed this pass.
    pub failed: Vec<(CompanionId, TaskFailure)>,
    /// First-action envelopes the Agent host produced at install and the
    /// loop deliberately did not submit: the task runner owns movement.
    pub ignored_first_actions: usize,
}

/// Agent plan deadline handed to the host: thirty seconds from the
/// caller's clock, mirroring the Go planner bridge default.
const PLAN_DEADLINE_MS: i64 = 30_000;

/// Finishes one companion's current task as failed through the core
/// seam, recording it in the report when the task was really finished.
fn finish_failed(
    state: &mut AuthorityState,
    report: &mut CompanionPlanningReport,
    companion: CompanionId,
    generation: u64,
    reason: TaskFailure,
) -> Result<(), ServerError> {
    if state.finish_companion_chat_task(companion, generation, TaskState::Failed(reason))? {
        report.failed.push((companion, reason));
    }
    Ok(())
}

/// One tracked in-flight dispatch: the core generation it belongs to and
/// the host attempt that names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrackedDispatch {
    generation: u64,
    attempt: u64,
}

/// The companion plan loop: an Agent host plus the dispatches this loop
/// has in flight, held across tick boundaries.
#[derive(Default)]
pub struct CompanionPlanLoop {
    host: PlanHost,
    tracked: BTreeMap<CompanionId, TrackedDispatch>,
}

impl CompanionPlanLoop {
    /// An empty loop with no in-flight planning.
    pub fn new() -> Self {
        Self::default()
    }

    /// The Agent host, for callers that drive dialogue or inspect gates.
    pub fn host(&self) -> &PlanHost {
        &self.host
    }

    /// The Agent host, mutably, for the dialogue lane's own dispatch.
    pub fn host_mut(&mut self) -> &mut PlanHost {
        &mut self.host
    }

    /// Whether this loop tracks a planning dispatch for the companion.
    pub fn planning_in_flight(&self, companion: CompanionId) -> bool {
        self.tracked.contains_key(&companion)
    }

    /// One tick-boundary pass. See the module docs for the order. An
    /// authority (settled-read) failure anywhere in the pass propagates
    /// unchanged as the outer error: it is never converted into a
    /// per-companion failure and no companion is dropped because of it.
    pub fn drive(
        &mut self,
        state: &mut AuthorityState,
        companions: &[CompanionId],
        pending_paths: &mut PendingCompanionPaths,
        ports: CompanionPlanningPorts<'_>,
        task_timeout_minutes: u32,
    ) -> Result<CompanionPlanningReport, ServerError> {
        let CompanionPlanningPorts {
            agent,
            snapshots,
            clock,
            client,
            namespace,
            lease,
            mint_id,
        } = ports;
        let mut report = CompanionPlanningReport::default();

        // Apply planner outcomes: poll once, then settle every outcome at
        // this tick boundary. Failures the host already classified finish
        // only their own companion's current task.
        self.host.drain_outcomes(agent, snapshots, clock)?;
        for failure in self.host.take_failures() {
            let Some(tracked) = self.tracked.get(&failure.companion).copied() else {
                continue;
            };
            if tracked.attempt != failure.attempt {
                continue;
            }
            self.tracked.remove(&failure.companion);
            let reason = match failure.kind {
                PlanFailKind::InvalidPlan => TaskFailure::InvalidPlan,
                PlanFailKind::Unavailable => TaskFailure::PlannerUnavailable,
            };
            finish_failed(
                state,
                &mut report,
                failure.companion,
                tracked.generation,
                reason,
            )?;
        }

        // Install: rebuild each outcome companion's current world first.
        // A settled-read failure aborts the whole pass unchanged, before
        // any companion is installed, failed, or dropped.
        let mut worlds = std::collections::BTreeMap::new();
        for companion in self.host.outcome_companions() {
            if let Ok(world) = state.companion_current_world(companion)? {
                worlds.insert(companion, world);
            }
        }
        let lease_fence = lease.map(|(_, fence)| fence).unwrap_or(0);
        let install_report = self
            .host
            .install_each(state.next_tick(), lease_fence, &worlds);
        // The host's first-action envelopes are deliberately not
        // submitted: the task runner below owns all movement.
        report.ignored_first_actions = install_report.envelopes.len();
        for installed in self.host.take_installed() {
            let Some(tracked) = self.tracked.get(&installed.companion).copied() else {
                continue;
            };
            if tracked.attempt != installed.attempt {
                continue;
            }
            self.tracked.remove(&installed.companion);
            if state.install_companion_chat_plan_with_timeout(
                installed.companion,
                tracked.generation,
                installed.plan,
                task_timeout_minutes,
            )? {
                report.installed.push(installed.companion);
            }
        }
        for rejection in &install_report.rejected {
            let Some(tracked) = self.tracked.get(&rejection.companion).copied() else {
                continue;
            };
            if tracked.attempt != rejection.attempt {
                continue;
            }
            self.tracked.remove(&rejection.companion);
            match rejection.reason {
                InstallReject::WorldChanged => finish_failed(
                    state,
                    &mut report,
                    rejection.companion,
                    tracked.generation,
                    TaskFailure::WorldChanged,
                )?,
                InstallReject::InvalidPlan => finish_failed(
                    state,
                    &mut report,
                    rejection.companion,
                    tracked.generation,
                    TaskFailure::InvalidPlan,
                )?,
                InstallReject::TaskFull => finish_failed(
                    state,
                    &mut report,
                    rejection.companion,
                    tracked.generation,
                    TaskFailure::PlannerUnavailable,
                )?,
                InstallReject::StaleIdentities => {
                    // Identity mismatch: the gate is released and only
                    // this companion's task fails, so it can never wedge
                    // the way a silently discarded outcome would.
                    self.host
                        .clear_plan_gate(rejection.companion, rejection.attempt);
                    finish_failed(
                        state,
                        &mut report,
                        rejection.companion,
                        tracked.generation,
                        TaskFailure::PlannerUnavailable,
                    )?;
                }
            }
        }

        // Expire and run installed tasks; path outcomes from this pass
        // land no earlier than the next one.
        report.actions = drive_companion_runners(state, pending_paths)?;

        // Dispatch new planning. Chat promotion runs first at its place
        // in the order, so a task finished above can hand its slot to
        // the next FIFO command in this same pass. The dialogue and
        // memory lanes are placeholders in this order and are not driven
        // here, beyond the dialogue gate check below.
        state.prepare_companion_chat()?;
        for &companion in companions {
            if self.tracked.contains_key(&companion) || self.host.plan_inflight(companion) {
                continue;
            }
            let Some(view) = state.companion_chat_queue(companion) else {
                continue;
            };
            let Some(current) = view.current else {
                continue;
            };
            if current.phase != CompanionChatPhase::Queued {
                continue;
            }
            // An inactive body keeps the task queued for a later pass.
            let Some(taken) = state.take_companion_chat_planning(companion)? else {
                continue;
            };
            let generation = taken.generation;
            if self.host.dialogue_inflight(companion) {
                finish_failed(
                    state,
                    &mut report,
                    companion,
                    generation,
                    TaskFailure::PlannerUnavailable,
                )?;
                continue;
            }
            let snapshot = match state.companion_planning_snapshot(companion) {
                Ok(snapshot) => snapshot,
                Err(ServerError::InvalidInput { .. }) => {
                    finish_failed(
                        state,
                        &mut report,
                        companion,
                        generation,
                        TaskFailure::PlannerUnavailable,
                    )?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let Some((lease_id, lease_fence)) = lease else {
                finish_failed(
                    state,
                    &mut report,
                    companion,
                    generation,
                    TaskFailure::PlannerUnavailable,
                )?;
                continue;
            };
            let identities = (|| {
                let request = AgentRequestId::try_from_bytes(mint_id()?).map_err(|_| {
                    ServerError::InvalidInput {
                        field: "request_id",
                    }
                })?;
                let run = RunId::try_from_bytes(mint_id()?)
                    .map_err(|_| ServerError::InvalidInput { field: "run_id" })?;
                Ok::<_, ServerError>((request, run))
            })();
            let (request_id, run_id) = match identities {
                Ok(identities) => identities,
                Err(_) => {
                    finish_failed(
                        state,
                        &mut report,
                        companion,
                        generation,
                        TaskFailure::PlannerUnavailable,
                    )?;
                    continue;
                }
            };
            let dispatch = PlanDispatch {
                companion,
                generation,
                request_id,
                run_id,
                client,
                namespace,
                lease: lease_id,
                lease_fence,
                source_tick: snapshot.source_tick(),
                deadline_unix_ms: clock.unix_ms().saturating_add(PLAN_DEADLINE_MS),
                snapshot,
            };
            match self.host.dispatch_plan(agent, snapshots, clock, dispatch) {
                Ok(ticket) => {
                    self.tracked.insert(
                        companion,
                        TrackedDispatch {
                            generation,
                            attempt: ticket.attempt,
                        },
                    );
                    report.dispatched.push(companion);
                }
                Err(
                    ServerError::Capacity { .. }
                    | ServerError::InvalidInput { .. }
                    | ServerError::Agent { .. },
                ) => {
                    finish_failed(
                        state,
                        &mut report,
                        companion,
                        generation,
                        TaskFailure::PlannerUnavailable,
                    )?;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap as Map;
    use std::time::Instant;

    use mornlea_domain::{ChatBody, PlayerId, TaskState as DomainTaskState};
    use mornlea_storage::COMPANION_TASK_COMPLETED;

    use crate::agent::host::DialogueDispatch;
    use crate::contracts::{
        AgentPoll, AgentRequest, AgentRequestId, AgentResponse, DialogueEnvironment, DialogueFact,
        LeaseId as Lease, Operation, PlanResponse, PlanStep, RunId, SnapshotId,
        SnapshotRegistration,
    };
    use crate::core::state::companion_planning::tests::{
        FIXTURES, add_companion, admit_command, companion_actor, drift_online_player, motion,
        planning_fixture_with_runtime, stored_task,
    };

    fn uuid(tag: u8) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0] = tag;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        bytes
    }

    struct TestClock(Instant);
    impl Clock for TestClock {
        fn monotonic(&self) -> Instant {
            self.0
        }
        fn unix_ms(&self) -> i64 {
            1_800_000_000_000
        }
    }

    #[derive(Default)]
    struct FakeAgent {
        submitted: Vec<AgentRequest>,
        terminals: Map<AgentRequestId, AgentPoll>,
    }

    impl FakeAgent {
        fn plan_requests(&self) -> Vec<crate::contracts::PlanRequest> {
            self.submitted
                .iter()
                .filter_map(|request| match request {
                    AgentRequest::Plan(plan) => Some(plan.clone()),
                    _ => None,
                })
                .collect()
        }
        fn complete(
            &mut self,
            request: &crate::contracts::PlanRequest,
            plan: crate::contracts::AgentPlan,
        ) {
            self.terminals.insert(
                request.leased.base.request_id,
                AgentPoll::Completed(AgentResponse::Plan(PlanResponse {
                    leased: request.leased.clone(),
                    run_id: request.run_id,
                    companion_id: request.companion_id,
                    generation: request.generation,
                    snapshot_id: request.snapshot_id,
                    snapshot_digest: request.snapshot_digest,
                    plan,
                })),
            );
        }
        fn fail(&mut self, request: &crate::contracts::PlanRequest, error: ServerError) {
            self.terminals
                .insert(request.leased.base.request_id, AgentPoll::Failed(error));
        }
    }

    impl AgentHandle for FakeAgent {
        fn submit(&mut self, request: AgentRequest) -> Result<AgentRequestId, ServerError> {
            let id = match &request {
                AgentRequest::Plan(plan) => plan.leased.base.request_id,
                AgentRequest::Dialogue(d) => d.leased.base.request_id,
                AgentRequest::Cancel(c) => c.leased.base.request_id,
                _ => {
                    return Err(ServerError::InvalidInput {
                        field: "fake_agent_request",
                    });
                }
            };
            self.submitted.push(request);
            Ok(id)
        }
        fn poll(&mut self, id: AgentRequestId) -> AgentPoll {
            self.terminals
                .get(&id)
                .cloned()
                .unwrap_or(AgentPoll::Pending)
        }
        fn cancel(
            &mut self,
            _id: AgentRequestId,
            _deadline: crate::contracts::Deadline,
        ) -> Result<(), ServerError> {
            Ok(())
        }
        fn freeze(&mut self, _clock: &dyn Clock) -> Option<crate::contracts::FrozenLease> {
            None
        }
        fn release(
            &mut self,
            _lease: &crate::contracts::FrozenLease,
            _deadline: crate::contracts::Deadline,
        ) -> Result<(), ServerError> {
            Ok(())
        }
        fn close(&mut self, _deadline: crate::contracts::Deadline) -> Result<(), ServerError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeSnapshots;
    impl SnapshotPort for FakeSnapshots {
        fn register(
            &mut self,
            _namespace: NamespaceId,
            _companion: CompanionId,
            _generation: u64,
            _snapshot: crate::contracts::PlanningSnapshot,
            _deadline: crate::contracts::Deadline,
        ) -> Result<SnapshotRegistration, ServerError> {
            SnapshotRegistration::try_new(
                SnapshotId::try_from_bytes(uuid(90)).unwrap(),
                [3u8; 32],
                vec![0u8; 32],
                "http://127.0.0.1:1/mcp".to_owned(),
            )
        }
        fn complete(&mut self, _id: SnapshotId) -> Result<(), ServerError> {
            Ok(())
        }
        fn cancel(&mut self, _id: SnapshotId) -> Result<(), ServerError> {
            Ok(())
        }
        fn close(&mut self) -> Result<(), ServerError> {
            Ok(())
        }
    }

    struct Rig {
        agent: FakeAgent,
        snapshots: FakeSnapshots,
        clock: TestClock,
        mint_tag: u8,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                agent: FakeAgent::default(),
                snapshots: FakeSnapshots,
                clock: TestClock(Instant::now()),
                mint_tag: 100,
            }
        }
    }

    // The ports value is built inline in each call to keep the mint
    // closure's borrow simple.
    fn drive(
        plan_loop: &mut CompanionPlanLoop,
        state: &mut AuthorityState,
        companions: &[CompanionId],
        pending: &mut PendingCompanionPaths,
        rig: &mut Rig,
        timeout: u32,
    ) -> Result<CompanionPlanningReport, ServerError> {
        let mut tag = rig.mint_tag;
        let report = {
            let mut mint = move || {
                tag = tag.wrapping_add(1);
                Ok(uuid(tag))
            };
            let ports = CompanionPlanningPorts {
                agent: &mut rig.agent,
                snapshots: &mut rig.snapshots,
                clock: &rig.clock,
                client: crate::contracts::ClientInstanceId::try_from_bytes(uuid(2)).unwrap(),
                namespace: NamespaceId::try_from_bytes(uuid(3)).unwrap(),
                lease: Some((Lease::try_from_bytes(uuid(30)).unwrap(), 9)),
                mint_id: &mut mint,
            };
            plan_loop.drive(state, companions, pending, ports, timeout)
        };
        rig.mint_tag = tag;
        report
    }

    fn queued_fixture() -> (
        crate::core::state::companion_planning::tests::PlanningFixture,
        CompanionId,
    ) {
        let fixture = planning_fixture_with_runtime(FIXTURES[2].1);
        let id = fixture.companion;
        (fixture, id)
    }

    fn go_home_plan() -> crate::contracts::AgentPlan {
        crate::contracts::AgentPlan::try_new(
            "go".to_owned(),
            vec![PlanStep::GoTo {
                x: -1,
                y: 315,
                z: -16,
            }],
        )
        .unwrap()
    }

    fn failed_facts(state: &mut AuthorityState) -> usize {
        state
            .take_decided_chat_facts()
            .iter()
            .filter(|fact| {
                matches!(
                    fact.body,
                    ChatBody::Task {
                        state: DomainTaskState::Failed(_),
                        ..
                    }
                )
            })
            .count()
    }

    #[test]
    fn plan_loop_dispatches_queued_task_once_and_holds_in_flight() {
        let (mut fixture, id) = queued_fixture();
        let mut plan_loop = CompanionPlanLoop::new();
        let mut rig = Rig::new();
        let mut pending = PendingCompanionPaths::new();
        let report = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert_eq!(report.dispatched, vec![id]);
        assert_eq!(rig.agent.plan_requests().len(), 1);
        assert!(plan_loop.planning_in_flight(id));
        // A second pass with the request still pending dispatches nothing.
        let report = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert!(report.dispatched.is_empty());
        assert_eq!(rig.agent.plan_requests().len(), 1);
        let view = fixture.authority.companion_chat_queue(id).unwrap();
        assert_eq!(
            view.current.unwrap().phase,
            crate::core::companion_chat::CompanionChatPhase::Planning
        );
    }

    #[test]
    fn plan_loop_inactive_companion_stays_queued() {
        let (mut fixture, id) = queued_fixture();
        companion_actor(&mut fixture.authority, id).lifecycle =
            crate::contracts::ActorLifecycle::Dead;
        let mut plan_loop = CompanionPlanLoop::new();
        let mut rig = Rig::new();
        let mut pending = PendingCompanionPaths::new();
        drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert!(rig.agent.plan_requests().is_empty());
        let view = fixture.authority.companion_chat_queue(id).unwrap();
        assert_eq!(
            view.current.unwrap().phase,
            crate::core::companion_chat::CompanionChatPhase::Queued
        );
    }

    #[test]
    fn plan_loop_dialogue_in_flight_fails_planner_unavailable_same_drive() {
        let (mut fixture, id) = queued_fixture();
        let mut plan_loop = CompanionPlanLoop::new();
        let mut rig = Rig::new();
        // Occupy the companion's dialogue gate directly through the host.
        let dispatch = DialogueDispatch {
            companion: id,
            generation: 1,
            memory_epoch: 0,
            request_id: AgentRequestId::try_from_bytes(uuid(41)).unwrap(),
            run_id: RunId::try_from_bytes(uuid(42)).unwrap(),
            client: crate::contracts::ClientInstanceId::try_from_bytes(uuid(2)).unwrap(),
            namespace: NamespaceId::try_from_bytes(uuid(3)).unwrap(),
            lease: Lease::try_from_bytes(uuid(30)).unwrap(),
            lease_fence: 9,
            persona: "calm".to_owned(),
            fact: DialogueFact::Idle,
            environment: DialogueEnvironment {
                exposed_blocks: vec![],
                heights: vec![],
            },
            terminal: false,
            deadline_unix_ms: 1_800_000_030_000,
        };
        plan_loop
            .host_mut()
            .dispatch_dialogue(&mut rig.agent, &rig.clock, dispatch)
            .unwrap();
        let mut pending = PendingCompanionPaths::new();
        let report = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert_eq!(report.failed, vec![(id, TaskFailure::PlannerUnavailable)]);
        assert!(
            rig.agent.plan_requests().is_empty(),
            "no plan may be dispatched"
        );
        assert!(
            fixture
                .authority
                .companion_chat_queue(id)
                .unwrap()
                .current
                .is_none()
        );
    }

    #[test]
    fn plan_loop_capacity_full_fails_planner_unavailable_same_drive() {
        let (mut fixture, id) = queued_fixture();
        let mut plan_loop = CompanionPlanLoop::new();
        let mut rig = Rig::new();
        // Fill every worker slot with dialogue for other companions.
        for tag in 0..4u8 {
            let other = CompanionId::try_from_bytes(uuid(50 + tag)).unwrap();
            let dispatch = DialogueDispatch {
                companion: other,
                generation: 1,
                memory_epoch: 0,
                request_id: AgentRequestId::try_from_bytes(uuid(60 + tag)).unwrap(),
                run_id: RunId::try_from_bytes(uuid(70 + tag)).unwrap(),
                client: crate::contracts::ClientInstanceId::try_from_bytes(uuid(2)).unwrap(),
                namespace: NamespaceId::try_from_bytes(uuid(3)).unwrap(),
                lease: Lease::try_from_bytes(uuid(30)).unwrap(),
                lease_fence: 9,
                persona: "calm".to_owned(),
                fact: DialogueFact::Idle,
                environment: DialogueEnvironment {
                    exposed_blocks: vec![],
                    heights: vec![],
                },
                terminal: false,
                deadline_unix_ms: 1_800_000_030_000,
            };
            plan_loop
                .host_mut()
                .dispatch_dialogue(&mut rig.agent, &rig.clock, dispatch)
                .unwrap();
        }
        let mut pending = PendingCompanionPaths::new();
        let report = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert_eq!(report.failed, vec![(id, TaskFailure::PlannerUnavailable)]);
        assert!(rig.agent.plan_requests().is_empty());
    }

    #[test]
    fn plan_loop_snapshot_build_failure_fails_planner_unavailable() {
        let (mut fixture, id) = queued_fixture();
        companion_actor(&mut fixture.authority, id).motion = motion([2_147_483_632.0, 64.0, 0.0]);
        let mut plan_loop = CompanionPlanLoop::new();
        let mut rig = Rig::new();
        let mut pending = PendingCompanionPaths::new();
        let report = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert_eq!(report.failed, vec![(id, TaskFailure::PlannerUnavailable)]);
        assert!(
            fixture
                .authority
                .companion_chat_queue(id)
                .unwrap()
                .current
                .is_none()
        );
    }

    #[test]
    fn plan_loop_agent_timeout_fails_planner_unavailable() {
        let (mut fixture, id) = queued_fixture();
        let mut plan_loop = CompanionPlanLoop::new();
        let mut rig = Rig::new();
        let mut pending = PendingCompanionPaths::new();
        drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        let request = rig.agent.plan_requests()[0].clone();
        rig.agent.fail(
            &request,
            ServerError::Timeout {
                operation: Operation::AgentRpc,
            },
        );
        let report = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert_eq!(report.failed, vec![(id, TaskFailure::PlannerUnavailable)]);
        assert!(
            fixture
                .authority
                .companion_chat_queue(id)
                .unwrap()
                .current
                .is_none()
        );
        assert_eq!(failed_facts(&mut fixture.authority), 1);
    }

    #[test]
    fn plan_loop_agent_malformed_fails_invalid_plan() {
        let (mut fixture, id) = queued_fixture();
        let mut plan_loop = CompanionPlanLoop::new();
        let mut rig = Rig::new();
        let mut pending = PendingCompanionPaths::new();
        drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        let request = rig.agent.plan_requests()[0].clone();
        rig.agent.fail(
            &request,
            ServerError::Agent {
                code: crate::contracts::AgentErrorCode::InvalidModelOutput,
                status: 422,
            },
        );
        let report = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert_eq!(report.failed, vec![(id, TaskFailure::InvalidPlan)]);
        assert!(
            fixture
                .authority
                .companion_chat_queue(id)
                .unwrap()
                .current
                .is_none()
        );
    }

    #[test]
    fn plan_loop_installs_and_runner_completes_without_double_drive() {
        let (mut fixture, id) = queued_fixture();
        let mut plan_loop = CompanionPlanLoop::new();
        let mut rig = Rig::new();
        let mut pending = PendingCompanionPaths::new();
        // The fixture's world time is a large sentinel and its install
        // start tick is the completed-tick count, so this test uses a
        // timeout whose deadline clears that sentinel; the arithmetic
        // assertion below still pins minutes x 1200 exactly.
        const BIG_TIMEOUT: u32 = 2_000_000_000;
        drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            BIG_TIMEOUT,
        )
        .unwrap();
        let request = rig.agent.plan_requests()[0].clone();
        rig.agent.complete(&request, go_home_plan());
        let report = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            BIG_TIMEOUT,
        )
        .unwrap();
        assert_eq!(report.installed, vec![id]);
        assert_eq!(
            report.ignored_first_actions, 1,
            "host first action must not drive movement"
        );
        // Deadline stamped at Running entry from the bare frozen minutes.
        let stored = stored_task(&fixture.authority, id);
        assert_eq!(
            stored.deadline_ticks,
            stored.start_tick + u64::from(BIG_TIMEOUT) * 1200
        );
        assert_ne!(stored.start_tick, 0);
        // The runner issued its path search this pass; the outcome lands
        // no earlier than the next pass, which completes the go_to.
        // The path outcome lands on this pass and the runner starts
        // moving; the loop's caller owns the physics that would move
        // the body, so the test places the body on the goal cell the
        // movement aimed at, and the next pass completes the step.
        let report = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            BIG_TIMEOUT,
        )
        .unwrap();
        assert_eq!(
            report.actions.len(),
            1,
            "runner movement, not a host first action"
        );
        companion_actor(&mut fixture.authority, id).motion = motion([-0.5, 315.0, -15.5]);
        drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            BIG_TIMEOUT,
        )
        .unwrap();
        assert!(
            fixture
                .authority
                .companion_chat_queue(id)
                .unwrap()
                .current
                .is_none(),
            "runner completes the task"
        );
        assert_eq!(
            stored_task(&fixture.authority, id).state,
            COMPANION_TASK_COMPLETED
        );
    }

    #[test]
    fn plan_loop_follow_tail_install_keeps_deadline_exemption() {
        let (mut fixture, id) = queued_fixture();
        // A follow target from the frozen snapshot's online set.
        let snapshot = fixture.authority.companion_planning_snapshot(id).unwrap();
        let player = snapshot.online_players[0].player_id;
        let mut plan_loop = CompanionPlanLoop::new();
        let mut rig = Rig::new();
        let mut pending = PendingCompanionPaths::new();
        drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        let request = rig.agent.plan_requests()[0].clone();
        let plan = crate::contracts::AgentPlan::try_new(
            "follow".to_owned(),
            vec![PlanStep::Follow { player_id: player }],
        )
        .unwrap();
        rig.agent.complete(&request, plan);
        let report = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert_eq!(report.installed, vec![id]);
        assert_eq!(
            stored_task(&fixture.authority, id).deadline_ticks,
            0,
            "follow tail stays exempt"
        );
    }

    #[test]
    fn plan_loop_world_changed_at_install_fails_only_that_companion_and_no_replan() {
        let (mut fixture, id) = queued_fixture();
        let other = CompanionId::try_from_bytes(uuid(21)).unwrap();
        add_companion(&mut fixture, other, "Milo", "go");
        let mut plan_loop = CompanionPlanLoop::new();
        let mut rig = Rig::new();
        let mut pending = PendingCompanionPaths::new();
        let both = [id, other];
        drive(
            &mut plan_loop,
            &mut fixture.authority,
            &both,
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert_eq!(rig.agent.plan_requests().len(), 2);
        for request in rig.agent.plan_requests() {
            rig.agent.complete(&request, go_home_plan());
        }
        // The first companion's body leaves before install: its current
        // world can no longer be rebuilt, a per-companion world change.
        companion_actor(&mut fixture.authority, id).lifecycle =
            crate::contracts::ActorLifecycle::Dead;
        let report = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &both,
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert_eq!(report.failed, vec![(id, TaskFailure::WorldChanged)]);
        assert_eq!(
            report.installed,
            vec![other],
            "the other companion installs unaffected"
        );
        assert!(
            fixture
                .authority
                .companion_chat_queue(id)
                .unwrap()
                .current
                .is_none()
        );
        // No same-drive or later re-plan of the failed command.
        assert_eq!(rig.agent.plan_requests().len(), 2);
        drive(
            &mut plan_loop,
            &mut fixture.authority,
            &both,
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert_eq!(rig.agent.plan_requests().len(), 2);
    }

    #[test]
    fn plan_loop_outer_authority_error_propagates_unchanged_and_fails_nobody() {
        let (mut fixture, id) = queued_fixture();
        let mut plan_loop = CompanionPlanLoop::new();
        let mut rig = Rig::new();
        let mut pending = PendingCompanionPaths::new();
        drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        let request = rig.agent.plan_requests()[0].clone();
        rig.agent.complete(&request, go_home_plan());
        let failure = ServerError::Internal {
            invariant: "test_authority_failure",
        };
        fixture.authority.fail_tick(failure);
        let error = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap_err();
        assert_eq!(
            error, failure,
            "the settled-read error propagates unchanged"
        );
        // Nobody was failed or dropped: the task is still Planning.
        let view = fixture.authority.companion_chat_queue(id).unwrap();
        assert_eq!(
            view.current.unwrap().phase,
            crate::core::companion_chat::CompanionChatPhase::Planning
        );
        assert_eq!(failed_facts(&mut fixture.authority), 0);
    }

    #[test]
    fn plan_loop_outcome_for_replaced_task_discarded_silently() {
        let (mut fixture, id) = queued_fixture();
        let mut plan_loop = CompanionPlanLoop::new();
        let mut rig = Rig::new();
        let mut pending = PendingCompanionPaths::new();
        drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        let request = rig.agent.plan_requests()[0].clone();
        rig.agent.complete(&request, go_home_plan());
        // The task ends outside the loop before its outcome lands.
        let issuer = fixture
            .authority
            .companion_chat_queue(id)
            .unwrap()
            .current
            .unwrap()
            .issuer
            .clone();
        fixture
            .authority
            .finish_companion_chat_task(
                id,
                1,
                DomainTaskState::Failed(TaskFailure::PlannerUnavailable),
            )
            .unwrap();
        // The manual finish above emitted exactly one failed fact; this
        // call drains it, so anything counted afterwards is new.
        assert_eq!(failed_facts(&mut fixture.authority), 1);
        let report = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert!(report.installed.is_empty());
        assert_eq!(
            failed_facts(&mut fixture.authority),
            0,
            "the stale outcome adds no failure"
        );
        // The companion can still plan its next command: nothing wedged.
        admit_command(&mut fixture.authority, id, "again", issuer);
        drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert_eq!(rig.agent.plan_requests().len(), 2);
    }

    #[test]
    fn plan_loop_identity_mismatch_clears_gate_and_fails_task() {
        let (mut fixture, id) = queued_fixture();
        let mut plan_loop = CompanionPlanLoop::new();
        let mut rig = Rig::new();
        let mut pending = PendingCompanionPaths::new();
        drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        let request = rig.agent.plan_requests()[0].clone();
        // Echo the plan under a different run identity than dispatched.
        let mut response = PlanResponse {
            leased: request.leased.clone(),
            run_id: RunId::try_from_bytes(uuid(77)).unwrap(),
            companion_id: request.companion_id,
            generation: request.generation,
            snapshot_id: request.snapshot_id,
            snapshot_digest: request.snapshot_digest,
            plan: go_home_plan(),
        };
        response.run_id = RunId::try_from_bytes(uuid(78)).unwrap();
        rig.agent.terminals.insert(
            request.leased.base.request_id,
            AgentPoll::Completed(AgentResponse::Plan(response)),
        );
        let issuer = fixture
            .authority
            .companion_chat_queue(id)
            .unwrap()
            .current
            .unwrap()
            .issuer
            .clone();
        let report = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert_eq!(report.failed, vec![(id, TaskFailure::PlannerUnavailable)]);
        assert!(
            !plan_loop.host().plan_inflight(id),
            "the gate is cleared, never wedged"
        );
        assert!(!plan_loop.planning_in_flight(id));
        assert!(
            fixture
                .authority
                .companion_chat_queue(id)
                .unwrap()
                .current
                .is_none()
        );
        // And the next command can plan immediately.
        admit_command(&mut fixture.authority, id, "again", issuer);
        drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert_eq!(rig.agent.plan_requests().len(), 2);
    }

    #[test]
    fn plan_loop_follow_target_drift_at_install_is_world_changed() {
        let (mut fixture, id) = queued_fixture();
        let snapshot = fixture.authority.companion_planning_snapshot(id).unwrap();
        let player: PlayerId = snapshot.online_players[0].player_id;
        let mut plan_loop = CompanionPlanLoop::new();
        let mut rig = Rig::new();
        let mut pending = PendingCompanionPaths::new();
        drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        let request = rig.agent.plan_requests()[0].clone();
        let plan = crate::contracts::AgentPlan::try_new(
            "follow".to_owned(),
            vec![PlanStep::Follow { player_id: player }],
        )
        .unwrap();
        rig.agent.complete(&request, plan);
        drift_online_player(&mut fixture.authority, player);
        let report = drive(
            &mut plan_loop,
            &mut fixture.authority,
            &[id],
            &mut pending,
            &mut rig,
            7,
        )
        .unwrap();
        assert_eq!(report.failed, vec![(id, TaskFailure::WorldChanged)]);
        assert!(
            fixture
                .authority
                .companion_chat_queue(id)
                .unwrap()
                .current
                .is_none()
        );
    }
}
