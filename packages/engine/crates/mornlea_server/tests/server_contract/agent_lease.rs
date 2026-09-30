//! Agent lease provider: the control state machine over a scripted wire.
//!
//! The frozen machine is `Absent -> AcquirePending -> Active{lease, fence,
//! expiry} -> HeartbeatPending or Frozen -> Closed` with a 15000 ms TTL, a
//! 5000 ms heartbeat cadence, and a control RPC deadline of
//! `min(now + 5 s, expiry)`. Expected values mirror
//! `packages/server/server/companion_agent.go` (lease controller, freeze and
//! release) with the frozen split between `control_revision` (late-result
//! discard) and `lease_fence` (business plan fencing).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use mornlea_domain::{CommandText, CompanionId};
use mornlea_server::agent::lease::{
    AgentWire, ControlPhase, ControlRound, HEARTBEAT_EVERY_MS, LeaseConfig, LeaseController,
    RpcCancellation,
};
use mornlea_server::contracts::{
    AgentErrorCode, AgentHandle, AgentPlan, AgentPoll, AgentRequest, AgentRequestId, AgentResponse,
    BaseIdentity, CancelRequest, CancelResponse, ClientInstanceId, Clock, CommitRequest, Deadline,
    DeleteRequest, DialogueEnvironment, DialogueFact, DialogueRequest, FrozenLease,
    LEASE_EXPIRES_IN_MS, LeaseId, LeaseResponse, LeasedIdentity, MemoryState, NamespaceId,
    Operation, OperationId, PlanRequest, PlanResponse, PlanStep, ReconcileRequest, RunId,
    ServerError, SnapshotId,
};

const SECOND: Duration = Duration::from_secs(1);
const CONTROL_RPC_TIMEOUT: Duration = Duration::from_secs(5);
const TTL: Duration = Duration::from_millis(LEASE_EXPIRES_IN_MS);

/// Test clock the harness can step forward across lease boundaries.
struct StepClock {
    now: Mutex<Instant>,
}

impl StepClock {
    fn start() -> (Instant, Arc<Self>) {
        let now = Instant::now();
        (
            now,
            Arc::new(Self {
                now: Mutex::new(now),
            }),
        )
    }

    fn set(&self, instant: Instant) {
        *self.now.lock().unwrap() = instant;
    }
}

impl Clock for StepClock {
    fn monotonic(&self) -> Instant {
        *self.now.lock().unwrap()
    }

    fn unix_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

fn timeout_error() -> ServerError {
    ServerError::Timeout {
        operation: Operation::AgentRpc,
    }
}

fn unavailable() -> ServerError {
    ServerError::Agent {
        code: AgentErrorCode::AgentUnavailable,
        status: 503,
    }
}

/// The echo-builder type of a scripted response.
type EchoBuilder = Arc<dyn Fn(&AgentRequest) -> Result<AgentResponse, ServerError> + Send + Sync>;

enum Scripted {
    Reply(Result<AgentResponse, ServerError>),
    /// The real Agent echoes the request identity; the closure builds the
    /// response from the request actually observed on the wire.
    Echo(EchoBuilder),
}

struct WireState {
    script: Mutex<VecDeque<Scripted>>,
    rounds: Mutex<Vec<(AgentRequest, Deadline)>>,
    gate: Mutex<Option<mpsc::Receiver<()>>>,
    parked: AtomicBool,
    closed: AtomicBool,
}

/// Scripted wire double: records every round, optionally holds the caller on
/// a gate so a business request can be parked mid-RPC, and answers from a
/// FIFO script. Interior mutability keeps concurrent RPCs independent, like
/// Go's per-call goroutines over one shared client.
#[derive(Clone)]
struct ScriptedWire {
    state: Arc<WireState>,
}

impl ScriptedWire {
    fn new() -> Self {
        Self {
            state: Arc::new(WireState {
                script: Mutex::new(VecDeque::new()),
                rounds: Mutex::new(Vec::new()),
                gate: Mutex::new(None),
                parked: AtomicBool::new(false),
                closed: AtomicBool::new(false),
            }),
        }
    }

    fn push(&self, outcome: Result<AgentResponse, ServerError>) {
        self.state
            .script
            .lock()
            .unwrap()
            .push_back(Scripted::Reply(outcome));
    }

    fn push_echo(
        &self,
        build: impl Fn(&AgentRequest) -> Result<AgentResponse, ServerError> + Send + Sync + 'static,
    ) {
        self.state
            .script
            .lock()
            .unwrap()
            .push_back(Scripted::Echo(Arc::new(build)));
    }

    fn hold(&self) -> mpsc::Sender<()> {
        let (sender, receiver) = mpsc::channel();
        *self.state.gate.lock().unwrap() = Some(receiver);
        sender
    }

    fn rounds(&self) -> Vec<(AgentRequest, Deadline)> {
        self.state.rounds.lock().unwrap().clone()
    }

    fn parked(&self) -> bool {
        self.state.parked.load(Ordering::SeqCst)
    }

    fn closed(&self) -> bool {
        self.state.closed.load(Ordering::SeqCst)
    }
}

impl AgentWire for ScriptedWire {
    fn rpc_cancellable(
        &self,
        request: AgentRequest,
        deadline: Deadline,
        cancellation: &RpcCancellation,
    ) -> Result<AgentResponse, ServerError> {
        if cancellation.is_cancelled() {
            return Err(unavailable());
        }
        self.state
            .rounds
            .lock()
            .unwrap()
            .push((request.clone(), deadline));
        // Bind before the branch so the gate guard drops immediately instead
        // of living through the park below.
        let receiver = self.state.gate.lock().unwrap().take();
        if let Some(receiver) = receiver {
            self.state.parked.store(true, Ordering::SeqCst);
            loop {
                if cancellation.is_cancelled() || self.state.closed.load(Ordering::SeqCst) {
                    self.state.parked.store(false, Ordering::SeqCst);
                    return Err(unavailable());
                }
                if deadline.expired(Instant::now()) {
                    self.state.parked.store(false, Ordering::SeqCst);
                    return Err(ServerError::Timeout {
                        operation: Operation::AgentRpc,
                    });
                }
                match receiver.recv_timeout(Duration::from_millis(5)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
            self.state.parked.store(false, Ordering::SeqCst);
        }
        match self.state.script.lock().unwrap().pop_front() {
            Some(Scripted::Reply(outcome)) => outcome,
            Some(Scripted::Echo(build)) => build(&request),
            None => Err(unavailable()),
        }
    }

    fn close(&self) {
        self.state.closed.store(true, Ordering::SeqCst);
    }
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn base_identity(request_tag: u8) -> BaseIdentity {
    BaseIdentity {
        request_id: AgentRequestId::try_from_bytes(uuid(request_tag)).unwrap(),
        client_instance_id: ClientInstanceId::try_from_bytes(uuid(2)).unwrap(),
        namespace_id: NamespaceId::try_from_bytes(uuid(3)).unwrap(),
    }
}

fn config() -> LeaseConfig {
    let base = base_identity(1);
    LeaseConfig {
        client_instance_id: base.client_instance_id,
        namespace_id: base.namespace_id,
    }
}

fn lease_id(tag: u8) -> LeaseId {
    LeaseId::try_from_bytes(uuid(tag)).unwrap()
}

fn acquire_response(request: &AgentRequest, lease: LeaseId) -> AgentResponse {
    let AgentRequest::Acquire(base) = request else {
        panic!("not an acquire round");
    };
    AgentResponse::Acquire(LeaseResponse {
        leased: LeasedIdentity {
            base: base.clone(),
            lease_id: lease,
        },
    })
}

fn heartbeat_response(request: &AgentRequest) -> AgentResponse {
    let AgentRequest::Heartbeat(leased) = request else {
        panic!("not a heartbeat round");
    };
    AgentResponse::Heartbeat(LeaseResponse {
        leased: leased.clone(),
    })
}

fn release_response(request: &AgentRequest) -> AgentResponse {
    let AgentRequest::Release(leased) = request else {
        panic!("not a release round");
    };
    AgentResponse::Release(LeaseResponse {
        leased: leased.clone(),
    })
}

fn plan_request(lease: LeaseId) -> PlanRequest {
    PlanRequest::try_new(
        LeasedIdentity {
            base: base_identity(4),
            lease_id: lease,
        },
        RunId::try_from_bytes(uuid(5)).unwrap(),
        CompanionId::try_from_bytes(uuid(6)).unwrap(),
        9,
        SnapshotId::try_from_bytes(uuid(7)).unwrap(),
        [0xaa; 32],
        1_800_000_000_000,
        "http://127.0.0.1:45831/mcp".to_owned(),
        "test-capability".to_owned(),
        CommandText::try_from_canonical("采一块石头".to_owned()).unwrap(),
    )
    .unwrap()
}

fn plan_response(request: &PlanRequest) -> AgentResponse {
    AgentResponse::Plan(PlanResponse {
        leased: request.leased.clone(),
        run_id: request.run_id,
        companion_id: request.companion_id,
        generation: request.generation,
        snapshot_id: request.snapshot_id,
        snapshot_digest: request.snapshot_digest,
        plan: AgentPlan::try_new(
            "走近箱子".to_owned(),
            vec![PlanStep::GoTo { x: 8, y: 64, z: -2 }],
        )
        .unwrap(),
    })
}

/// Waits until the predicate holds, bounded by a real-time budget so a stuck
/// machine fails the test instead of hanging it.
fn wait_until(predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut predicate = predicate;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    predicate()
}

fn controller(wire: &ScriptedWire, clock: &Arc<StepClock>) -> LeaseController {
    LeaseController::try_new(config(), Arc::new(wire.clone()), clock.clone()).unwrap()
}

/// Drives one acquire round to success by hand and returns the round token.
fn acquire_lease(agent: &LeaseController, lease: LeaseId) -> ControlRound {
    let round = agent.start_control_round().expect("acquire round");
    let response = acquire_response(&round.request, lease);
    agent.settle_control(round.clone(), Ok(response));
    round
}

#[test]
fn hung_acquire_heartbeat() {
    let (start, clock) = StepClock::start();
    let wire = ScriptedWire::new();
    let agent = controller(&wire, &clock);

    // A hung acquire with no lease: the RPC deadline is exactly now + 5 s
    // (the min against the absent expiry), the failure clears, and the
    // machine stays Absent.
    wire.push(Err(timeout_error()));
    agent.refresh();
    let rounds = wire.rounds();
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].1.instant(), start + CONTROL_RPC_TIMEOUT);
    assert_eq!(agent.control_phase(), ControlPhase::Absent);
    assert!(agent.current_lease().is_none());

    // A late acquire outcome replaying the hung round's token carries the
    // consumed revision, so settling it now discards it.
    let (late_request, late_deadline) = rounds[0].clone();
    let late = ControlRound {
        revision: 1,
        request: late_request,
        deadline: late_deadline,
    };
    let late_success = match &late.request {
        AgentRequest::Acquire(base) => AgentResponse::Acquire(LeaseResponse {
            leased: LeasedIdentity {
                base: base.clone(),
                lease_id: lease_id(4),
            },
        }),
        _ => panic!("expected acquire round"),
    };
    agent.settle_control(late, Ok(late_success));
    assert_eq!(agent.control_phase(), ControlPhase::Absent);
    assert!(agent.current_lease().is_none());

    // A fresh acquire installs the lease with the frozen TTL.
    acquire_lease(&agent, lease_id(4));
    assert_eq!(agent.control_phase(), ControlPhase::Active);
    let (installed_lease, fence) = agent.current_lease().expect("active lease");
    assert_eq!(installed_lease, lease_id(4));
    assert_eq!(fence, 1);

    // One heartbeat cadence step (5000 ms < TTL 15000) keeps the lease and
    // the fence, and the RPC deadline is the new now + 5 s.
    clock.set(start + Duration::from_millis(HEARTBEAT_EVERY_MS));
    let round = agent.start_control_round().expect("heartbeat round");
    assert_eq!(
        round.deadline.instant(),
        start + Duration::from_millis(HEARTBEAT_EVERY_MS) + CONTROL_RPC_TIMEOUT
    );
    let response = heartbeat_response(&round.request);
    agent.settle_control(round, Ok(response));
    assert_eq!(agent.control_phase(), ControlPhase::Active);
    let (kept_lease, kept_fence) = agent.current_lease().expect("still active");
    assert_eq!(kept_lease, installed_lease);
    assert_eq!(kept_fence, fence);

    // As expiry approaches, the deadline clamps to the lease expiry. The
    // successful heartbeat above renewed the expiry to its settle time plus
    // the frozen TTL.
    let renewed_expiry = start + Duration::from_millis(HEARTBEAT_EVERY_MS + LEASE_EXPIRES_IN_MS);
    clock.set(renewed_expiry - SECOND);
    let round = agent.start_control_round().expect("heartbeat round");
    assert_eq!(
        round.deadline.instant(),
        renewed_expiry,
        "deadline = min(now + 5s, expiry)"
    );
    // The hung heartbeat outcome clears the lease; the next refresh
    // reacquires under a new fence.
    agent.settle_control(round, Err(timeout_error()));
    assert_eq!(agent.control_phase(), ControlPhase::Absent);
    assert!(agent.current_lease().is_none());
    acquire_lease(&agent, lease_id(5));
    let (new_lease, new_fence) = agent.current_lease().expect("reacquired");
    assert_eq!(new_lease, lease_id(5));
    assert_eq!(new_fence, 2, "reacquire creates a new lease_fence");
}

#[test]
fn late_fence_ignored() {
    let (_start, clock) = StepClock::start();
    let wire = ScriptedWire::new();
    let agent = controller(&wire, &clock);

    let first = acquire_lease(&agent, lease_id(4));
    assert_eq!(agent.current_lease(), Some((lease_id(4), 1)));

    // A hung heartbeat clears the active lease and consumes its revision.
    let heartbeat = agent.start_control_round().expect("heartbeat round");
    agent.settle_control(heartbeat, Err(timeout_error()));
    assert_eq!(agent.control_phase(), ControlPhase::Absent);
    assert!(agent.current_lease().is_none());

    // A late acquire result replaying the first round's token can never
    // revive the cleared fence.
    let late = ControlRound {
        revision: first.revision,
        request: first.request.clone(),
        deadline: first.deadline,
    };
    agent.settle_control(late, Ok(acquire_response(&first.request, lease_id(4))));
    assert_eq!(agent.control_phase(), ControlPhase::Absent);
    assert!(agent.current_lease().is_none());

    // The next acquire round installs a new lease under a new fence; the old
    // identity stays retired.
    acquire_lease(&agent, lease_id(8));
    assert_eq!(agent.current_lease(), Some((lease_id(8), 2)));
    let _ = clock;
}

#[test]
fn heartbeat_does_not_invalidate_plan() {
    let (_start, clock) = StepClock::start();
    let wire = ScriptedWire::new();
    let mut agent = controller(&wire, &clock);

    acquire_lease(&agent, lease_id(4));

    // Admit a plan under the live lease and park its RPC on the gate; the
    // parked rpc holds the gate before popping the script.
    let request = plan_request(lease_id(4));
    wire.push(Ok(plan_response(&request)));
    let gate = wire.hold();
    let id = agent
        .submit(AgentRequest::Plan(request.clone()))
        .expect("plan admitted");
    assert!(
        wait_until(|| wire
            .rounds()
            .iter()
            .any(|(request, _)| matches!(request, AgentRequest::Plan(_)))
            && wire.parked()),
        "plan rpc started"
    );
    assert_eq!(agent.poll(id), AgentPoll::Pending);

    // A successful heartbeat preserves the lease fence, so the parked plan
    // is still admissible when its outcome arrives.
    let round = agent.start_control_round().expect("heartbeat round");
    let response = heartbeat_response(&round.request);
    agent.settle_control(round, Ok(response));
    assert_eq!(agent.current_lease(), Some((lease_id(4), 1)));

    let _ = gate.send(());
    assert!(wait_until(|| !matches!(agent.poll(id), AgentPoll::Pending)));
    assert_eq!(
        agent.poll(id),
        AgentPoll::Completed(plan_response(&request))
    );
    let _ = clock;
}

#[test]
fn reacquire_fences_old_plan() {
    let (_start, clock) = StepClock::start();
    let wire = ScriptedWire::new();
    let mut agent = controller(&wire, &clock);

    acquire_lease(&agent, lease_id(4));
    assert_eq!(agent.current_lease(), Some((lease_id(4), 1)));

    // Park a plan admitted under lease one. The parked rpc holds the gate
    // before popping the script, so the first entry is the heartbeat's and
    // the second is the plan's.
    let request = plan_request(lease_id(4));
    wire.push(Err(timeout_error()));
    wire.push(Ok(plan_response(&request)));
    let gate = wire.hold();
    let id = agent.submit(AgentRequest::Plan(request.clone())).unwrap();
    assert!(
        wait_until(|| wire
            .rounds()
            .iter()
            .any(|(request, _)| matches!(request, AgentRequest::Plan(_)))
            && wire.parked()),
        "plan rpc started"
    );

    // The heartbeat fails, the lease clears, and the next refresh reacquires
    // under a new lease id and fence.
    agent.refresh();
    assert!(agent.current_lease().is_none());
    acquire_lease(&agent, lease_id(10));
    assert_eq!(agent.current_lease(), Some((lease_id(10), 2)));

    // The parked plan outcome arrives after the reacquire: the old fence no
    // longer matches, so the result is refused instead of published.
    let _ = gate.send(());
    assert!(wait_until(|| !matches!(agent.poll(id), AgentPoll::Pending)));
    assert_eq!(agent.poll(id), AgentPoll::Failed(unavailable()));

    // A brand-new plan naming the old lease is refused at admission too.
    assert!(
        agent
            .submit(AgentRequest::Plan(plan_request(lease_id(4))))
            .is_err()
    );
}

#[test]
fn freeze_release_retry_and_expiry() {
    let (start, clock) = StepClock::start();
    let wire = ScriptedWire::new();
    let mut agent = controller(&wire, &clock);

    acquire_lease(&agent, lease_id(4));

    // Freeze retains only a still-unexpired lease and stops the machine.
    clock.set(start + SECOND);
    let frozen: FrozenLease = agent.freeze(&*clock).expect("frozen lease");
    assert_eq!(frozen.lease, lease_id(4));
    assert_eq!(frozen.lease_fence, 1);
    assert_eq!(frozen.expires_at, start + TTL);
    assert_eq!(agent.control_phase(), ControlPhase::Frozen);

    // A failed release keeps the frozen identity for the retry.
    wire.push(Err(unavailable()));
    let deadline = Deadline::after(clock.monotonic(), SECOND).unwrap();
    assert!(agent.release(&frozen, deadline).is_err());
    let retained = agent.freeze(&*clock).expect("identity retained");
    assert_eq!(retained.lease, frozen.lease);
    assert_eq!(retained.lease_fence, frozen.lease_fence);

    // The release retry addresses the same identity and succeeds; the real
    // Agent echoes the request, which the scripted wire reproduces.
    wire.push_echo(|request| Ok(release_response(request)));
    let retry_deadline = Deadline::after(clock.monotonic(), SECOND).unwrap();
    agent.release(&frozen, retry_deadline).expect("release ok");
    // After a successful release nothing is retained.
    assert!(agent.freeze(&*clock).is_none());

    // Expiry permits the skip: an expired frozen lease returns None instead
    // of an invented receipt.
    let (expiry_start, expiry_clock) = StepClock::start();
    let expiry_wire = ScriptedWire::new();
    let mut expiring = controller(&expiry_wire, &expiry_clock);
    acquire_lease(&expiring, lease_id(4));
    expiry_clock.set(expiry_start + SECOND);
    let _frozen = expiring
        .freeze(&*expiry_clock)
        .expect("frozen before expiry");
    expiry_clock.set(expiry_start + TTL + Duration::from_millis(1));
    assert!(expiring.freeze(&*expiry_clock).is_none());

    // Close is idempotent: the second close succeeds, the wire is closed,
    // and the machine stays Closed with no further admissions.
    let close_deadline = Deadline::after(expiry_clock.monotonic(), SECOND).unwrap();
    expiring.close(close_deadline).expect("first close");
    expiring.close(close_deadline).expect("second close");
    assert_eq!(expiring.control_phase(), ControlPhase::Closed);
    assert!(expiry_wire.closed());
    assert!(expiring.freeze(&*expiry_clock).is_none());
    assert!(
        expiring
            .submit(AgentRequest::Plan(plan_request(lease_id(4))))
            .is_err()
    );
}

fn tagged_plan(tag: u8) -> PlanRequest {
    let mut request = plan_request(lease_id(4));
    request.leased.base.request_id = base_identity(tag).request_id;
    request
}

#[test]
fn terminal_business_requests_remain_charged_until_retirement() {
    let (start, clock) = StepClock::start();
    let wire = ScriptedWire::new();
    let mut agent = controller(&wire, &clock);
    acquire_lease(&agent, lease_id(4));
    for tag in 20..84 {
        let request = tagged_plan(tag);
        wire.push(Ok(plan_response(&request)));
        let id = agent.submit(AgentRequest::Plan(request.clone())).unwrap();
        assert!(wait_until(|| !matches!(agent.poll(id), AgentPoll::Pending)));
        assert_eq!(
            agent.poll(id),
            AgentPoll::Completed(plan_response(&request))
        );
        assert_eq!(
            agent.poll(id),
            AgentPoll::Completed(plan_response(&request))
        );
    }
    assert_eq!(agent.retained_requests(), 64);
    assert_eq!(agent.pending_business_workers(), 0);
    assert_eq!(wire.rounds().len(), 64);
    assert_eq!(
        agent.submit(AgentRequest::Plan(tagged_plan(20))),
        Err(ServerError::InvalidInput {
            field: "agent_request_id"
        })
    );
    assert_eq!(
        agent.submit(AgentRequest::Plan(tagged_plan(84))),
        Err(ServerError::Capacity {
            resource: mornlea_server::contracts::Resource::AgentRuns,
            limit: 64,
            observed: 65,
        })
    );
    assert_eq!(wire.rounds().len(), 64, "refusal must not call the wire");
    agent
        .cancel(base_identity(20).request_id, Deadline::at(start))
        .unwrap();
    assert_eq!(agent.retained_requests(), 63);
    assert_eq!(
        agent.poll(base_identity(20).request_id),
        AgentPoll::Failed(unavailable())
    );
    let request = tagged_plan(20);
    wire.push(Ok(plan_response(&request)));
    let id = agent.submit(AgentRequest::Plan(request.clone())).unwrap();
    assert!(wait_until(|| !matches!(agent.poll(id), AgentPoll::Pending)));
    assert_eq!(
        agent.poll(id),
        AgentPoll::Completed(plan_response(&request))
    );
    assert_eq!(agent.retained_requests(), 64);
    assert_eq!(wire.rounds().len(), 65);
    for tag in 20..84 {
        agent
            .cancel(base_identity(tag).request_id, Deadline::at(start))
            .unwrap();
    }
    assert_eq!(agent.retained_requests(), 0);
}

#[test]
fn cancel_business_request_reaps_cooperative_worker_and_preserves_other_request() {
    let (start, clock) = StepClock::start();
    let wire = ScriptedWire::new();
    let mut agent = controller(&wire, &clock);
    acquire_lease(&agent, lease_id(4));
    let gate = wire.hold();
    let held = agent.submit(AgentRequest::Plan(tagged_plan(20))).unwrap();
    assert!(wait_until(|| wire.parked()));
    let independent = tagged_plan(21);
    wire.push(Ok(plan_response(&independent)));
    let independent_id = agent
        .submit(AgentRequest::Plan(independent.clone()))
        .unwrap();
    assert!(wait_until(|| !matches!(
        agent.poll(independent_id),
        AgentPoll::Pending
    )));
    assert_eq!(
        agent.poll(independent_id),
        AgentPoll::Completed(plan_response(&independent))
    );
    let before = Instant::now();
    let result = agent.cancel(held, Deadline::at(start + Duration::from_millis(30)));
    // Release the bounded fixture even if cancellation failed to reach the wire.
    drop(gate);
    assert_eq!(result, Ok(()));
    assert!(before.elapsed() < Duration::from_millis(200));
    assert_eq!(agent.retained_requests(), 1);
    assert_eq!(agent.pending_business_workers(), 0);
    assert_eq!(agent.poll(held), AgentPoll::Failed(unavailable()));
    assert_eq!(
        agent.poll(independent_id),
        AgentPoll::Completed(plan_response(&independent))
    );
    agent.cancel(independent_id, Deadline::at(start)).unwrap();
}

/// This bounded double deliberately ignores cancellation to exercise retained
/// ownership when the wire cannot finish within the consumer's caller budget.
struct NoncooperativeWire {
    gate: Mutex<mpsc::Receiver<()>>,
    started: Mutex<mpsc::Sender<()>>,
}

impl AgentWire for NoncooperativeWire {
    fn rpc_cancellable(
        &self,
        request: AgentRequest,
        _deadline: Deadline,
        _cancellation: &RpcCancellation,
    ) -> Result<AgentResponse, ServerError> {
        self.started.lock().unwrap().send(()).unwrap();
        let _ = self.gate.lock().unwrap().recv_timeout(SECOND);
        let AgentRequest::Plan(request) = request else {
            panic!("expected business request")
        };
        Ok(plan_response(&request))
    }

    fn close(&self) {}
}

#[test]
fn cancel_business_timeout_uses_wall_budget_and_retains_owned_worker_for_retry() {
    let (start, clock) = StepClock::start();
    let (gate, receiver) = mpsc::channel();
    let (started, ready) = mpsc::channel();
    let wire = Arc::new(NoncooperativeWire {
        gate: Mutex::new(receiver),
        started: Mutex::new(started),
    });
    let mut agent = LeaseController::try_new(config(), wire, clock.clone()).unwrap();
    acquire_lease(&agent, lease_id(4));
    let id = agent.submit(AgentRequest::Plan(tagged_plan(20))).unwrap();
    ready
        .recv_timeout(SECOND)
        .expect("business worker entered wire");
    assert_eq!(agent.pending_business_workers(), 1);
    let before = Instant::now();
    let result = agent.cancel(id, Deadline::at(start + Duration::from_millis(20)));
    let elapsed = before.elapsed();
    let retained = agent.retained_requests();
    let unfinished = agent.pending_business_workers();
    // Opening the gate before assertions keeps failure cleanup bounded.
    let _ = gate.send(());
    assert_eq!(result, Err(timeout_error()));
    assert!(elapsed < Duration::from_millis(200), "elapsed {elapsed:?}");
    assert_eq!(retained, 1);
    assert_eq!(unfinished, 1);
    assert!(wait_until(|| agent.pending_business_workers() == 0));
    assert_eq!(
        agent.poll(id),
        AgentPoll::Failed(unavailable()),
        "cancelled success must not publish"
    );
    agent.cancel(id, Deadline::at(start)).unwrap();
    assert_eq!(agent.retained_requests(), 0);
    assert_eq!(agent.cancel(id, Deadline::at(start)), Err(unavailable()));
}

#[test]
fn panicked_business_worker_reports_internal_and_is_reclaimed() {
    let (start, clock) = StepClock::start();
    let wire = ScriptedWire::new();
    let mut agent = controller(&wire, &clock);
    acquire_lease(&agent, lease_id(4));
    wire.push_echo(|_| panic!("bounded business producer panic"));
    let id = agent.submit(AgentRequest::Plan(tagged_plan(20))).unwrap();
    assert!(wait_until(|| !matches!(agent.poll(id), AgentPoll::Pending)));
    let error = ServerError::Internal {
        invariant: "agent business worker",
    };
    assert_eq!(agent.poll(id), AgentPoll::Failed(error));
    assert_eq!(agent.pending_business_workers(), 0);
    assert_eq!(agent.cancel(id, Deadline::at(start)), Err(error));
    assert_eq!(agent.retained_requests(), 0);
    assert_eq!(agent.cancel(id, Deadline::at(start)), Err(unavailable()));
}

/// Separate bounded gates permit actual control and business producers to be
/// held concurrently without serializing the controller's wire calls.
struct ShutdownWire {
    control_gate: Mutex<Option<mpsc::Receiver<()>>>,
    business_gate: Mutex<Option<mpsc::Receiver<()>>>,
    entered: mpsc::Sender<bool>,
    closed: AtomicBool,
}

impl AgentWire for ShutdownWire {
    fn rpc_cancellable(
        &self,
        request: AgentRequest,
        _deadline: Deadline,
        _cancellation: &RpcCancellation,
    ) -> Result<AgentResponse, ServerError> {
        let control = matches!(request, AgentRequest::Heartbeat(_));
        let receiver = if control {
            self.control_gate.lock().unwrap().take()
        } else {
            self.business_gate.lock().unwrap().take()
        };
        self.entered.send(control).unwrap();
        if let Some(receiver) = receiver {
            let _ = receiver.recv_timeout(SECOND);
        }
        match request {
            AgentRequest::Heartbeat(_) => Ok(heartbeat_response(&request)),
            AgentRequest::Plan(ref request) => Ok(plan_response(request)),
            _ => panic!("expected seeded control or business request"),
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

fn shutdown_wire() -> (
    Arc<ShutdownWire>,
    mpsc::Sender<()>,
    mpsc::Sender<()>,
    mpsc::Receiver<bool>,
) {
    let (control, control_gate) = mpsc::channel();
    let (business, business_gate) = mpsc::channel();
    let (entered, ready) = mpsc::channel();
    (
        Arc::new(ShutdownWire {
            control_gate: Mutex::new(Some(control_gate)),
            business_gate: Mutex::new(Some(business_gate)),
            entered,
            closed: AtomicBool::new(false),
        }),
        control,
        business,
        ready,
    )
}

#[test]
fn freeze_retains_noncooperative_control_join_without_waiting() {
    let (start, clock) = StepClock::start();
    let (wire, control, _business, ready) = shutdown_wire();
    let mut agent = LeaseController::try_new(config(), wire, clock.clone()).unwrap();
    acquire_lease(&agent, lease_id(4));
    agent.spawn_control_worker().unwrap();
    assert!(ready.recv_timeout(SECOND).expect("heartbeat entered"));
    let before = Instant::now();
    let frozen = agent.freeze(&*clock).expect("retained live lease");
    let elapsed = before.elapsed();
    let retained = agent.retained_control_workers();
    let pending = agent.pending_control_worker();
    let frozen_again = agent.freeze(&*clock).expect("same frozen lease");
    let _ = control.send(());
    let close = agent.close(Deadline::at(start + SECOND));
    assert!(
        elapsed < Duration::from_millis(100),
        "freeze elapsed {elapsed:?}"
    );
    assert_eq!(frozen.lease, lease_id(4));
    assert_eq!(frozen.lease_fence, 1);
    assert_eq!(frozen_again, frozen);
    assert_eq!(retained, 1);
    assert!(pending);
    assert_eq!(close, Ok(()));
    assert_eq!(agent.retained_control_workers(), 0);
    assert!(agent.current_lease().is_none());
    assert_eq!(agent.spawn_control_worker(), Err(unavailable()));
}

#[test]
fn freeze_cancels_cooperative_control_rpc_without_closing_wire() {
    let (start, clock) = StepClock::start();
    let wire = ScriptedWire::new();
    let mut agent = controller(&wire, &clock);
    acquire_lease(&agent, lease_id(4));
    let gate = wire.hold();
    agent.spawn_control_worker().unwrap();
    assert!(wait_until(|| wire.parked()));
    let before = Instant::now();
    let frozen = agent.freeze(&*clock);
    let elapsed = before.elapsed();
    let promptly_stopped = wait_until(|| !agent.pending_control_worker());
    drop(gate);
    assert!(frozen.is_some());
    assert!(
        elapsed < Duration::from_millis(100),
        "freeze elapsed {elapsed:?}"
    );
    assert!(promptly_stopped);
    assert!(before.elapsed() < Duration::from_millis(200));
    assert!(!wire.closed(), "freeze must not close the shared wire");
    assert_eq!(agent.retained_control_workers(), 1);
    assert_eq!(agent.spawn_control_worker(), Err(unavailable()));
    agent.close(Deadline::at(start)).unwrap();
    assert_eq!(agent.retained_control_workers(), 0);
}

#[test]
fn close_bounds_both_owned_worker_lanes_and_retries_reclamation() {
    let (start, clock) = StepClock::start();
    let (wire, control, business, ready) = shutdown_wire();
    let mut agent = LeaseController::try_new(config(), wire.clone(), clock.clone()).unwrap();
    acquire_lease(&agent, lease_id(4));
    agent.spawn_control_worker().unwrap();
    assert!(ready.recv_timeout(SECOND).expect("heartbeat entered"));
    agent.submit(AgentRequest::Plan(tagged_plan(20))).unwrap();
    assert!(!ready.recv_timeout(SECOND).expect("business entered"));
    let before = Instant::now();
    let result = agent.close(Deadline::at(start + Duration::from_millis(20)));
    let elapsed = before.elapsed();
    let retained_control = agent.retained_control_workers();
    let pending_control = agent.pending_control_worker();
    let retained_business = agent.retained_requests();
    let pending_business = agent.pending_business_workers();
    let wire_closed = wire.closed.load(Ordering::SeqCst);
    let lease_cleared = agent.current_lease().is_none();
    let repeated_pending = agent.close(Deadline::at(start));
    let _ = control.send(());
    let _ = business.send(());
    assert_eq!(
        result,
        Err(ServerError::Timeout {
            operation: Operation::Close
        })
    );
    assert!(
        elapsed < Duration::from_millis(200),
        "close elapsed {elapsed:?}"
    );
    assert!(wire_closed);
    assert_eq!(
        repeated_pending,
        Err(ServerError::Timeout {
            operation: Operation::Close
        })
    );
    assert!(lease_cleared);
    assert_eq!(retained_control, 1);
    assert!(pending_control);
    assert_eq!(retained_business, 1);
    assert_eq!(pending_business, 1);
    assert!(wait_until(
        || !agent.pending_control_worker() && agent.pending_business_workers() == 0
    ));
    agent.close(Deadline::at(start)).unwrap();
    assert_eq!(agent.retained_control_workers(), 0);
    assert_eq!(agent.retained_requests(), 0);
    agent.close(Deadline::at(start)).unwrap();
    assert_eq!(
        agent.submit(AgentRequest::Plan(tagged_plan(21))),
        Err(unavailable())
    );
    assert_eq!(agent.spawn_control_worker(), Err(unavailable()));
    assert!(agent.current_lease().is_none());
}

#[test]
fn close_reclaims_finished_control_and_business_panics_before_returning_error() {
    for control in [true, false] {
        let (start, clock) = StepClock::start();
        let wire = ScriptedWire::new();
        let mut agent = controller(&wire, &clock);
        acquire_lease(&agent, lease_id(4));
        wire.push_echo(|_| panic!("bounded shutdown producer panic"));
        if control {
            agent.spawn_control_worker().unwrap();
            assert!(wait_until(|| !agent.pending_control_worker()));
        } else {
            agent.submit(AgentRequest::Plan(tagged_plan(20))).unwrap();
            agent.submit(AgentRequest::Plan(tagged_plan(21))).unwrap();
            assert!(wait_until(|| agent.pending_business_workers() == 0));
        }
        let error = ServerError::Internal {
            invariant: if control {
                "agent control worker"
            } else {
                "agent business worker"
            },
        };
        assert_eq!(agent.close(Deadline::at(start)), Err(error));
        assert_eq!(agent.retained_control_workers(), 0);
        assert_eq!(agent.retained_requests(), 0);
        assert_eq!(agent.close(Deadline::at(start)), Ok(()));
    }
}

fn cleanup_request(tag: u8, kind: u8) -> AgentRequest {
    let leased = LeasedIdentity {
        base: base_identity(tag),
        lease_id: lease_id(4),
    };
    let companion_id = CompanionId::try_from_bytes(uuid(6)).unwrap();
    let operation_id = OperationId::try_from_bytes(uuid(7)).unwrap();
    match kind {
        0 => AgentRequest::Commit(CommitRequest {
            leased,
            companion_id,
            memory_epoch: 1,
            base_revision: 0,
            operation_id,
            summary: "confirmed".to_owned(),
        }),
        1 => AgentRequest::Reconcile(ReconcileRequest::Active {
            leased,
            companion_id,
            memory_epoch: 1,
            mirror: MemoryState::Absent,
        }),
        2 => AgentRequest::Reconcile(ReconcileRequest::Inactive {
            leased,
            companion_id,
            memory_epoch: 2,
            tombstone_operation_id: operation_id,
        }),
        3 => AgentRequest::Delete(DeleteRequest {
            leased,
            companion_id,
            old_memory_epoch: 1,
            new_memory_epoch: 2,
            tombstone_operation_id: operation_id,
        }),
        _ => AgentRequest::Cancel(CancelRequest {
            leased,
            run_id: RunId::try_from_bytes(uuid(8)).unwrap(),
        }),
    }
}

#[test]
fn frozen_lease_admits_only_retained_finalization_identity() {
    let (start, clock) = StepClock::start();
    let wire = ScriptedWire::new();
    let mut agent = controller(&wire, &clock);
    acquire_lease(&agent, lease_id(4));
    let frozen = agent.freeze(&*clock).unwrap();
    let rounds = wire.rounds().len();
    assert_eq!(
        agent.submit(AgentRequest::Plan(tagged_plan(20))),
        Err(unavailable())
    );
    let dialogue = DialogueRequest {
        leased: LeasedIdentity {
            base: base_identity(21),
            lease_id: lease_id(4),
        },
        run_id: RunId::try_from_bytes(uuid(8)).unwrap(),
        companion_id: CompanionId::try_from_bytes(uuid(6)).unwrap(),
        generation: 1,
        memory_epoch: 1,
        deadline_unix_ms: 1_800_000_001_000,
        persona: String::new(),
        fact_node: DialogueFact::Idle,
        environment: DialogueEnvironment {
            exposed_blocks: Vec::new(),
            heights: Vec::new(),
        },
        terminal: false,
    };
    assert_eq!(
        agent.submit(AgentRequest::Dialogue(dialogue)),
        Err(unavailable())
    );
    assert_eq!(wire.rounds().len(), rounds);
    for kind in 0..5 {
        wire.push(Err(unavailable()));
        let id = agent
            .submit(cleanup_request(30 + kind, kind))
            .expect("finalizer admitted");
        assert!(wait_until(|| !matches!(agent.poll(id), AgentPoll::Pending)));
        assert_eq!(wire.rounds().len(), rounds + usize::from(kind) + 1);
        assert_eq!(agent.poll(id), AgentPoll::Failed(unavailable()));
        agent.cancel(id, Deadline::at(start)).unwrap();
    }
    assert!(agent.current_lease().is_none());
    assert_eq!(agent.retained_requests(), 0);
    for mismatch in 0..3 {
        let AgentRequest::Cancel(mut request) = cleanup_request(40 + mismatch, 4) else {
            unreachable!()
        };
        match mismatch {
            0 => {
                request.leased.base.client_instance_id =
                    ClientInstanceId::try_from_bytes(uuid(99)).unwrap()
            }
            1 => request.leased.base.namespace_id = NamespaceId::try_from_bytes(uuid(99)).unwrap(),
            _ => request.leased.lease_id = lease_id(99),
        }
        assert_eq!(
            agent.submit(AgentRequest::Cancel(request)),
            Err(unavailable())
        );
    }
    assert_eq!(wire.rounds().len(), rounds + 5);
    wire.push(Err(unavailable()));
    assert_eq!(
        agent.release(&frozen, Deadline::at(start + SECOND)),
        Err(unavailable())
    );
    let id = agent
        .submit(cleanup_request(45, 4))
        .expect("failed release retains finalizer");
    assert!(wait_until(|| !matches!(agent.poll(id), AgentPoll::Pending)));
    agent.cancel(id, Deadline::at(start)).unwrap();
    wire.push_echo(|request| Ok(release_response(request)));
    agent
        .release(&frozen, Deadline::at(start + SECOND))
        .unwrap();
    assert!(agent.current_lease().is_none());
    let rounds = wire.rounds().len();
    assert_eq!(agent.submit(cleanup_request(46, 4)), Err(unavailable()));
    assert_eq!(wire.rounds().len(), rounds);
    agent.close(Deadline::at(start)).unwrap();
    assert_eq!(agent.submit(cleanup_request(47, 4)), Err(unavailable()));

    let mut expired = controller(&ScriptedWire::new(), &clock);
    acquire_lease(&expired, lease_id(4));
    expired.freeze(&*clock).unwrap();
    clock.set(start + TTL);
    assert_eq!(expired.submit(cleanup_request(48, 4)), Err(unavailable()));
    assert!(expired.current_lease().is_none());
    expired.close(Deadline::at(start + TTL)).unwrap();
}

#[test]
fn freeze_fences_held_plan_completion() {
    let (start, clock) = StepClock::start();
    let wire = ScriptedWire::new();
    let mut agent = controller(&wire, &clock);
    acquire_lease(&agent, lease_id(4));
    let request = tagged_plan(20);
    wire.push(Ok(plan_response(&request)));
    let gate = wire.hold();
    let id = agent.submit(AgentRequest::Plan(request)).unwrap();
    assert!(wait_until(|| wire.parked()));
    agent.freeze(&*clock).unwrap();
    let _ = gate.send(());
    assert!(wait_until(|| !matches!(agent.poll(id), AgentPoll::Pending)));
    let actual = agent.poll(id);
    agent.cancel(id, Deadline::at(start)).unwrap();
    agent.close(Deadline::at(start)).unwrap();
    assert_eq!(actual, AgentPoll::Failed(unavailable()));
}

#[test]
fn release_fences_held_frozen_cleanup_completion() {
    let (start, clock) = StepClock::start();
    let wire = ScriptedWire::new();
    let mut agent = controller(&wire, &clock);
    acquire_lease(&agent, lease_id(4));
    let frozen = agent.freeze(&*clock).unwrap();
    for _ in 0..2 {
        wire.push_echo(|request| match request {
            AgentRequest::Cancel(request) => Ok(AgentResponse::Cancel(CancelResponse {
                leased: request.leased.clone(),
                run_id: request.run_id,
                cancelled: true,
            })),
            AgentRequest::Release(_) => Ok(release_response(request)),
            _ => unreachable!(),
        });
    }
    let gate = wire.hold();
    let admitted = agent.submit(cleanup_request(20, 4));
    let id = match admitted {
        Ok(id) => id,
        Err(error) => {
            drop(gate);
            agent.close(Deadline::at(start)).unwrap();
            panic!("frozen cleanup refused: {error:?}")
        }
    };
    assert!(wait_until(|| wire.parked()));
    let released = agent.release(&frozen, Deadline::at(start + SECOND));
    let _ = gate.send(());
    assert!(wait_until(|| !matches!(agent.poll(id), AgentPoll::Pending)));
    let actual = agent.poll(id);
    agent.cancel(id, Deadline::at(start)).unwrap();
    agent.close(Deadline::at(start)).unwrap();
    assert_eq!(released, Ok(()));
    assert_eq!(actual, AgentPoll::Failed(unavailable()));
}
