//! Cross-language Agent cases over the real helper: plan with authority,
//! memory commit with reconcile, block with cancel and shutdown, and release
//! retry with expiry.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mornlea_server::agent::host::{PlanDispatch, PlanHost};
use mornlea_server::agent::http::AgentHttpWire;
use mornlea_server::agent::lease::{AgentWire, ControlPhase, LeaseConfig, LeaseController};
use mornlea_server::agent::memory::{
    CommitReservation, CommitSettled, MemoryMirror, MemoryOwner, ReconcileSettled,
};
use mornlea_server::contracts::{
    AgentErrorCode, AgentHandle, AgentPoll, AgentRequest, AgentResponse, BaseIdentity,
    CancelRequest, Clock, CompanionAction, Deadline, LeaseId, LeasedIdentity, ServerError,
    SnapshotPort,
};
use mornlea_server::core::companion_ingress::{CompanionIngress, CompanionTaskGate};

use super::process::{
    StepClock, acquire_lease, await_block_marker, client_id, companion_id, drain_plan_until,
    namespace_id, operation, plan_snapshot, plan_world, poll_request_until, request_id, run_id,
    serve_mcp, spawn_helper, target_pos, wall_deadline_ms,
};

/// Plans through the real gateway, planner, and model SDK against the real
/// snapshot tools, then installs the validated mine step through the task
/// runner and the sessionless ingress authority.
#[test]
fn rust_plan_python_mcp_and_authority() {
    let token = "plan-child-token";
    let mut child = spawn_helper("plan", token);
    let (_start, clock) = StepClock::start();
    let (mut snapshots, mcp) = serve_mcp(&clock);
    let mut agent = acquire_lease(&clock, &child.endpoint(), token);
    let (lease_id, fence) = agent.current_lease().expect("acquire admitted");
    let mut host = PlanHost::new();

    let ticket = host
        .dispatch_plan(
            &mut agent,
            &mut snapshots,
            &*clock,
            PlanDispatch {
                companion: companion_id(),
                generation: 7,
                request_id: request_id(40),
                run_id: run_id(41),
                client: client_id(),
                namespace: namespace_id(),
                lease: lease_id,
                lease_fence: fence,
                snapshot: plan_snapshot("采一块石头", 99),
                source_tick: 99,
                deadline_unix_ms: wall_deadline_ms(),
            },
        )
        .expect("dispatch admits");
    assert_eq!(ticket.attempt, 1);

    let drained = drain_plan_until(
        &mut host,
        &mut agent,
        &mut snapshots,
        &clock,
        Duration::from_secs(20),
    );
    if drained.completed != 1 {
        panic!(
            "plan outcome missing: report={drained:?} failures={:?} poll={:?} mcp_done={:?}",
            host.take_failures(),
            agent.poll(request_id(40)),
            mcp.wait_done(Duration::from_millis(0)),
        );
    }
    assert_eq!(drained.failed, 0);
    assert_eq!(host.outcome_len(), 1);

    let installed = host.install(99, fence, &plan_world(99));
    assert_eq!(installed.installed, 1);
    assert_eq!(installed.envelopes.len(), 1);
    assert!(installed.rejected.is_empty());
    let envelope = installed
        .envelopes
        .into_iter()
        .next()
        .expect("one permitted effect");
    assert_eq!(envelope.companion_id, companion_id());
    assert_eq!(envelope.source_tick, 99);
    assert_eq!(envelope.request_id, request_id(40));
    assert_eq!(envelope.run_id, run_id(41));
    assert_eq!(envelope.generation, 7);
    assert_eq!(envelope.attempt, ticket.attempt);
    assert_ne!(envelope.snapshot_digest, [0u8; 32]);
    match envelope.action {
        CompanionAction::MineHold { target } => assert_eq!(target, target_pos()),
        other => panic!("expected mine hold, got {other:?}"),
    }

    let snapshot_id = envelope.snapshot_id;
    let mut ingress = CompanionIngress::try_new().expect("ingress");
    let gate = CompanionTaskGate::try_new(
        envelope.companion_id,
        envelope.request_id,
        envelope.run_id,
        envelope.snapshot_id,
        envelope.snapshot_digest,
        envelope.generation,
        envelope.attempt,
    )
    .expect("task gate");
    let receipt = ingress
        .admit(&gate, 99, envelope)
        .expect("task runner emits admissible companion actions");
    assert_eq!(receipt.tick(), 99);

    assert!(
        snapshots.complete(snapshot_id).is_err(),
        "drain completes its own registration exactly once"
    );
    snapshots.close().expect("registry closes");
    mcp.close();
    assert!(
        mcp.wait_done(Duration::from_secs(5)).is_some(),
        "model serve loop settles"
    );
    agent
        .close(Deadline::after(clock.monotonic(), Duration::from_secs(5)).expect("close deadline"))
        .expect("agent closes");
    assert_eq!(agent.control_phase(), ControlPhase::Closed);
    child.shutdown("plan");
}

/// Polls one memory owner until a commit settles or the bound expires.
fn commits_until(owner: &mut MemoryOwner, timeout: Duration) -> Vec<CommitSettled> {
    let deadline = Instant::now() + timeout;
    loop {
        let settled = owner.poll_commits();
        if !settled.is_empty() {
            return settled;
        }
        assert!(Instant::now() < deadline, "commit never settled");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Polls one memory owner until a reconcile settles or the bound expires.
fn reconciles_until(owner: &mut MemoryOwner, timeout: Duration) -> Vec<ReconcileSettled> {
    let deadline = Instant::now() + timeout;
    loop {
        let settled = owner.poll_reconciles();
        if !settled.is_empty() {
            return settled;
        }
        assert!(Instant::now() < deadline, "reconcile never settled");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Commits one reserved operation against the real store, then reconciles
/// the exact epoch and revision back.
#[test]
fn real_memory_commit_reconcile() {
    let token = "memory-child-token";
    let mut child = spawn_helper("memory", token);
    let (_start, clock) = StepClock::start();
    let agent = acquire_lease(&clock, &child.endpoint(), token);
    let (lease_id, _) = agent.current_lease().expect("acquire admitted");
    let mut owner = MemoryOwner::new(
        Box::new(agent),
        clock.clone(),
        client_id(),
        namespace_id(),
        lease_id,
    );

    owner.set_mirror(
        companion_id(),
        MemoryMirror {
            active: true,
            epoch: 1,
            revision: 0,
            operation: None,
            summary: String::new(),
            tombstone: None,
        },
    );
    owner
        .reconcile(companion_id(), request_id(50))
        .expect("reconcile admits");
    match reconciles_until(&mut owner, Duration::from_secs(15)).as_slice() {
        [ReconcileSettled::Ready { companion, .. }] => assert_eq!(*companion, companion_id()),
        other => panic!("zero reconcile never converged: {other:?}"),
    }
    let mirror = owner.mirror(companion_id()).expect("mirror kept");
    assert_eq!(mirror.revision, 0);

    owner
        .reserve(
            CommitReservation::try_new(
                companion_id(),
                1,
                operation(60),
                0,
                "采集确认".to_owned(),
                "已经完成了。".to_owned(),
            )
            .expect("reservation"),
        )
        .expect("reserve admits");
    owner
        .commit(companion_id(), request_id(51))
        .expect("commit submits");
    match commits_until(&mut owner, Duration::from_secs(15)).as_slice() {
        [
            CommitSettled::Applied {
                companion,
                revision,
            },
        ] => {
            assert_eq!(*companion, companion_id());
            assert_eq!(*revision, 1);
        }
        other => panic!("commit never applied: {other:?}"),
    }
    let mirror = owner.mirror(companion_id()).expect("mirror kept");
    assert_eq!(mirror.epoch, 1);
    assert_eq!(mirror.revision, 1);
    assert_eq!(mirror.operation, Some(operation(60)));
    assert!(
        owner.reservation(companion_id()).is_none(),
        "applied commit consumes its reservation"
    );

    owner
        .reconcile(companion_id(), request_id(52))
        .expect("second reconcile admits");
    match reconciles_until(&mut owner, Duration::from_secs(15)).as_slice() {
        [ReconcileSettled::Ready { companion, .. }] => assert_eq!(*companion, companion_id()),
        other => panic!("committed reconcile never converged: {other:?}"),
    }
    let mirror = owner.mirror(companion_id()).expect("mirror kept");
    assert_eq!(mirror.epoch, 1);
    assert_eq!(mirror.revision, 1);
    assert_eq!(mirror.operation, Some(operation(60)));
    child.shutdown("memory");
}

/// Submits one run cancel against the admitted run and polls it to the
/// helper answer.
fn cancel_run_until(
    agent: &mut LeaseController,
    lease_id: LeaseId,
    request_tag: u8,
    run_tag: u8,
    timeout: Duration,
) {
    let cancel = AgentRequest::Cancel(CancelRequest {
        leased: LeasedIdentity {
            base: BaseIdentity {
                request_id: request_id(request_tag),
                client_instance_id: client_id(),
                namespace_id: namespace_id(),
            },
            lease_id,
        },
        run_id: run_id(run_tag),
    });
    let id = agent.submit(cancel).expect("cancel submits");
    match poll_request_until(agent, id, timeout) {
        AgentPoll::Completed(AgentResponse::Cancel(response)) => {
            assert!(response.cancelled, "helper cancels the admitted run");
            assert_eq!(response.run_id, run_id(run_tag));
        }
        AgentPoll::Completed(other) => panic!("cancel answered another response: {other:?}"),
        AgentPoll::Failed(error) => panic!("cancel failed: {error:?}"),
        AgentPoll::Pending => panic!("cancel stayed pending after settle"),
    }
}

/// Blocks the helper model, cancels and times out the run, then shuts the
/// lease, registry, model service, and wire down with nothing leaked.
#[test]
fn block_cancel_deadline_and_shutdown() {
    let token = "block-child-token";
    let mut child = spawn_helper("block", token);
    let (_start, clock) = StepClock::start();
    let (mut snapshots, mcp) = serve_mcp(&clock);
    let mut agent = acquire_lease(&clock, &child.endpoint(), token);
    let (lease_id, fence) = agent.current_lease().expect("acquire admitted");
    let mut host = PlanHost::new();
    let dispatch = |request_tag: u8, run_tag: u8, source_tick: u64| PlanDispatch {
        companion: companion_id(),
        generation: 7,
        request_id: request_id(request_tag),
        run_id: run_id(run_tag),
        client: client_id(),
        namespace: namespace_id(),
        lease: lease_id,
        lease_fence: fence,
        snapshot: plan_snapshot("BLOCK_UNTIL_CANCEL", source_tick),
        source_tick,
        deadline_unix_ms: wall_deadline_ms(),
    };

    let first = host
        .dispatch_plan(&mut agent, &mut snapshots, &*clock, dispatch(40, 41, 200))
        .expect("blocking dispatch admits");
    assert_eq!(first.attempt, 1);
    await_block_marker(
        &mut host,
        &mut agent,
        &mut snapshots,
        &mcp,
        &clock,
        child.marker(),
        request_id(40),
        Duration::from_secs(10),
        "first block",
    );
    cancel_run_until(&mut agent, lease_id, 42, 41, Duration::from_secs(10));
    let drained = drain_plan_until(
        &mut host,
        &mut agent,
        &mut snapshots,
        &clock,
        Duration::from_secs(15),
    );
    assert_eq!(drained.failed, 1);
    assert_eq!(drained.completed, 0);
    assert!(
        !host.plan_inflight(companion_id()),
        "cancelled run frees its worker only after cleanup"
    );
    let failures = host.take_failures();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].companion, companion_id());
    assert_eq!(failures[0].attempt, first.attempt);
    std::fs::remove_file(child.marker()).expect("clear the model barrier");

    let second = host
        .dispatch_plan(&mut agent, &mut snapshots, &*clock, dispatch(44, 45, 201))
        .expect("second blocking dispatch admits");
    await_block_marker(
        &mut host,
        &mut agent,
        &mut snapshots,
        &mcp,
        &clock,
        child.marker(),
        request_id(44),
        Duration::from_secs(10),
        "second block",
    );
    let pacer = clock.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1200));
        pacer.set(Instant::now() + Duration::from_secs(2));
    });
    match agent.cancel(
        request_id(44),
        Deadline::after(clock.monotonic(), Duration::from_secs(1)).expect("wait deadline"),
    ) {
        Err(ServerError::Timeout { .. }) => {}
        other => panic!("caller wait never timed out: {other:?}"),
    }
    assert!(
        host.plan_inflight(companion_id()),
        "caller timeout alone frees no worker"
    );
    cancel_run_until(&mut agent, lease_id, 46, 45, Duration::from_secs(10));
    let drained = drain_plan_until(
        &mut host,
        &mut agent,
        &mut snapshots,
        &clock,
        Duration::from_secs(15),
    );
    assert_eq!(drained.failed, 1);
    assert!(
        !host.plan_inflight(companion_id()),
        "timed-out run frees its worker only after cleanup"
    );
    assert_eq!(host.take_failures().len(), 1);
    assert_eq!(second.attempt, 2);
    std::fs::remove_file(child.marker()).expect("clear the model barrier");

    let third = host
        .dispatch_plan(
            &mut agent,
            &mut snapshots,
            &*clock,
            PlanDispatch {
                companion: companion_id(),
                generation: 7,
                request_id: request_id(48),
                run_id: run_id(49),
                client: client_id(),
                namespace: namespace_id(),
                lease: lease_id,
                lease_fence: fence,
                snapshot: plan_snapshot("采一块石头", 202),
                source_tick: 202,
                deadline_unix_ms: wall_deadline_ms(),
            },
        )
        .expect("business capacity returns after cleanup");
    assert_eq!(third.attempt, 3);
    let drained = drain_plan_until(
        &mut host,
        &mut agent,
        &mut snapshots,
        &clock,
        Duration::from_secs(20),
    );
    assert_eq!(
        drained.completed,
        1,
        "third plan must complete after cleanup, drain report: {drained:?} failures={:?} poll={:?} mcp_done={:?}",
        host.take_failures(),
        agent.poll(request_id(48)),
        mcp.wait_done(Duration::from_millis(0)),
    );
    let installed = host.install(202, fence, &plan_world(202));
    assert_eq!(installed.installed, 1);
    assert_eq!(installed.envelopes.len(), 1);
    assert!(installed.rejected.is_empty());
    let snapshot_id = installed
        .envelopes
        .into_iter()
        .next()
        .expect("one permitted effect")
        .snapshot_id;
    assert!(
        snapshots.complete(snapshot_id).is_err(),
        "drain completes its own registration exactly once"
    );

    let frozen = agent
        .freeze(&*clock)
        .expect("shutdown freezes the live lease");
    assert_eq!(frozen.lease, lease_id);
    assert_eq!(frozen.lease_fence, fence);
    agent
        .release(
            &frozen,
            Deadline::after(clock.monotonic(), Duration::from_secs(5)).expect("release deadline"),
        )
        .expect("release ok");
    assert!(
        agent.freeze(&*clock).is_none(),
        "released lease retains nothing"
    );
    snapshots.close().expect("registry closes");
    assert!(
        snapshots
            .register(
                namespace_id(),
                companion_id(),
                9,
                plan_snapshot("采一块石头", 203),
                Deadline::after(clock.monotonic(), Duration::from_secs(5)).expect("probe deadline"),
            )
            .is_err(),
        "closed registry admits nothing"
    );
    mcp.close();
    assert!(
        mcp.wait_done(Duration::from_secs(5)).is_some(),
        "model serve loop settles"
    );
    agent
        .close(Deadline::after(clock.monotonic(), Duration::from_secs(5)).expect("close deadline"))
        .expect("agent closes");
    assert_eq!(agent.control_phase(), ControlPhase::Closed);
    assert_eq!(host.outcome_len(), 0);
    child.shutdown("block");
}

/// Test-only wire wrapper dropping the first release answer while the real
/// service, state, and provider stay untouched.
struct LoseReleaseOnce {
    inner: Arc<AgentHttpWire>,
    armed: AtomicBool,
}

impl LoseReleaseOnce {
    /// Arms one dropped release answer over the real wire.
    fn new(inner: Arc<AgentHttpWire>) -> Self {
        Self {
            inner,
            armed: AtomicBool::new(true),
        }
    }
}

impl AgentWire for LoseReleaseOnce {
    fn rpc(&self, request: AgentRequest, deadline: Deadline) -> Result<AgentResponse, ServerError> {
        if matches!(request, AgentRequest::Release(_)) && self.armed.swap(false, Ordering::SeqCst) {
            return Err(ServerError::Agent {
                code: AgentErrorCode::AgentUnavailable,
                status: 503,
            });
        }
        self.inner.rpc(request, deadline)
    }

    fn close(&self) {
        self.inner.close();
    }
}

/// Loses one release answer, keeps the same lease across the retry, then
/// proves expiry still skips without an invented receipt.
#[test]
fn release_failure_retains_same_lease() {
    let token = "release-child-token";
    let mut child = spawn_helper("release", token);
    let (_start, clock) = StepClock::start();
    let wire = Arc::new(
        AgentHttpWire::try_new(&child.endpoint(), token, clock.clone()).expect("loopback wire"),
    );
    let faults = Arc::new(LoseReleaseOnce::new(wire));
    let mut agent = LeaseController::try_new(
        LeaseConfig {
            client_instance_id: client_id(),
            namespace_id: namespace_id(),
        },
        faults.clone(),
        clock.clone(),
    )
    .expect("lease controller");
    agent.refresh();
    let (lease_id, fence) = agent.current_lease().expect("acquire admitted");

    let frozen = agent.freeze(&*clock).expect("frozen lease");
    assert_eq!(frozen.lease, lease_id);
    assert_eq!(frozen.lease_fence, fence);
    let failed = agent
        .release(
            &frozen,
            Deadline::after(clock.monotonic(), Duration::from_secs(5)).expect("release deadline"),
        )
        .expect_err("lost release fails");
    match failed {
        ServerError::Agent { code, status } => {
            assert_eq!(status, 503);
            assert_eq!(code.status(), 503);
        }
        other => panic!("lost release hid its transport error: {other:?}"),
    }
    let retained = agent.freeze(&*clock).expect("identity retained");
    assert_eq!(retained.lease, lease_id);
    assert_eq!(retained.lease_fence, fence);
    agent
        .release(
            &frozen,
            Deadline::after(clock.monotonic(), Duration::from_secs(5)).expect("retry deadline"),
        )
        .expect("release retry ok");
    assert!(
        agent.freeze(&*clock).is_none(),
        "released lease retains nothing"
    );

    let (expiry_start, expiry_clock) = StepClock::start();
    let mut expiring = LeaseController::try_new(
        LeaseConfig {
            client_instance_id: client_id(),
            namespace_id: namespace_id(),
        },
        faults,
        expiry_clock.clone(),
    )
    .expect("expiry controller");
    expiring.refresh();
    assert!(
        expiring.current_lease().is_some(),
        "expiry controller acquires"
    );
    let _live = expiring
        .freeze(&*expiry_clock)
        .expect("frozen before expiry");
    expiry_clock.set(expiry_start + Duration::from_secs(15) + Duration::from_millis(1));
    assert!(
        expiring.freeze(&*expiry_clock).is_none(),
        "expired lease releases without an invented receipt"
    );
    expiring
        .close(
            Deadline::after(expiry_clock.monotonic(), Duration::from_secs(5))
                .expect("close deadline"),
        )
        .expect("expiry controller closes");
    assert_eq!(expiring.control_phase(), ControlPhase::Closed);
    agent
        .close(Deadline::after(clock.monotonic(), Duration::from_secs(5)).expect("close deadline"))
        .expect("agent closes");
    child.shutdown("release");
}
