//! Companion memory ownership: commit fencing, epoch advance, and shutdown
//! finalization.
//!
//! Expected values mirror the Go authority in
//! `packages/server/server/companion_dialogue.go` (`applyMemoryCommitOutcome`
//! echoes epoch and operation with `committedRevision == baseRevision + 1`,
//! errors re-arm reconcile for the same operation, and shutdown retries
//! pending memory work under a fresh context per attempt).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mornlea_domain::CompanionId;
use mornlea_server::agent::memory::{
    CommitReservation, CommitSettled, DeleteSettled, MemoryMirror, MemoryOwner, ReconcileSettled,
    reconcile_wait_ticks,
};
use mornlea_server::contracts::{
    AgentHandle, AgentPoll, AgentRequest, AgentRequestId, AgentResponse, ClientInstanceId, Clock,
    CommitResponse, Deadline, DeleteResponse, LeaseId, LeasedIdentity, MemoryFinalizer,
    MemoryState, NamespaceId, OperationId, ReconcileResponse, ServerError,
};

/// Step clock the harness holds fixed so every deadline comparison is
/// deterministic.
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
}

impl Clock for StepClock {
    fn monotonic(&self) -> Instant {
        *self.now.lock().unwrap()
    }

    fn unix_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn companion() -> CompanionId {
    CompanionId::try_from_bytes(uuid(20)).unwrap()
}

fn operation(tag: u8) -> OperationId {
    OperationId::try_from_bytes(uuid(tag)).unwrap()
}

fn request_id(tag: u8) -> AgentRequestId {
    AgentRequestId::try_from_bytes(uuid(tag)).unwrap()
}

/// Scripted `AgentHandle` double: FIFO poll script, observed submits, and an
/// optional park gate holding one request in `Pending` until released.
/// Interior mutability shares every observation with the owner under test.
#[derive(Clone)]
struct ScriptAgent {
    state: Arc<ScriptState>,
}

struct ScriptState {
    polls: Mutex<VecDeque<AgentPoll>>,
    submitted: Mutex<Vec<AgentRequest>>,
    parked: Mutex<Vec<AgentRequestId>>,
    terminals: Mutex<BTreeMap<AgentRequestId, AgentPoll>>,
    retired: Mutex<Vec<AgentRequestId>>,
    refuse_retirement: Mutex<BTreeSet<AgentRequestId>>,
}

impl ScriptAgent {
    fn new() -> Self {
        Self {
            state: Arc::new(ScriptState {
                polls: Mutex::new(VecDeque::new()),
                submitted: Mutex::new(Vec::new()),
                parked: Mutex::new(Vec::new()),
                terminals: Mutex::new(BTreeMap::new()),
                retired: Mutex::new(Vec::new()),
                refuse_retirement: Mutex::new(BTreeSet::new()),
            }),
        }
    }

    fn push(&self, poll: AgentPoll) {
        self.state.polls.lock().unwrap().push_back(poll);
    }

    fn park(&self, id: AgentRequestId) {
        self.state.parked.lock().unwrap().push(id);
    }

    fn unpark(&self, id: AgentRequestId) {
        self.state
            .parked
            .lock()
            .unwrap()
            .retain(|parked| *parked != id);
    }

    fn submitted(&self) -> Vec<AgentRequest> {
        self.state.submitted.lock().unwrap().clone()
    }

    fn commit_bodies(&self) -> Vec<(u64, [u8; 16], u64, String)> {
        self.submitted()
            .into_iter()
            .filter_map(|request| match request {
                AgentRequest::Commit(commit) => Some((
                    commit.memory_epoch,
                    commit.operation_id.bytes(),
                    commit.base_revision,
                    commit.summary.clone(),
                )),
                _ => None,
            })
            .collect()
    }
}

fn unavailable() -> ServerError {
    ServerError::Agent {
        code: mornlea_server::contracts::AgentErrorCode::AgentUnavailable,
        status: 503,
    }
}

fn not_found() -> ServerError {
    ServerError::Agent {
        code: mornlea_server::contracts::AgentErrorCode::NotFound,
        status: 404,
    }
}

impl AgentHandle for ScriptAgent {
    fn submit(&mut self, request: AgentRequest) -> Result<AgentRequestId, ServerError> {
        let id = match &request {
            AgentRequest::Commit(commit) => commit.leased.base.request_id,
            AgentRequest::Reconcile(
                mornlea_server::contracts::ReconcileRequest::Active { leased, .. }
                | mornlea_server::contracts::ReconcileRequest::Inactive { leased, .. },
            ) => leased.base.request_id,
            AgentRequest::Delete(delete) => delete.leased.base.request_id,
            _ => return Err(unavailable()),
        };
        self.state.submitted.lock().unwrap().push(request);
        Ok(id)
    }

    fn poll(&mut self, id: AgentRequestId) -> AgentPoll {
        if let Some(poll) = self.state.terminals.lock().unwrap().get(&id) {
            return poll.clone();
        }
        if self.state.parked.lock().unwrap().contains(&id) {
            return AgentPoll::Pending;
        }
        self.state
            .polls
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(AgentPoll::Pending)
    }

    fn cancel(&mut self, id: AgentRequestId, _deadline: Deadline) -> Result<(), ServerError> {
        if self.state.refuse_retirement.lock().unwrap().contains(&id) {
            return Err(ServerError::Timeout {
                operation: mornlea_server::contracts::Operation::AgentRpc,
            });
        }
        self.state.retired.lock().unwrap().push(id);
        Ok(())
    }

    fn freeze(&mut self, _clock: &dyn Clock) -> Option<mornlea_server::contracts::FrozenLease> {
        None
    }

    fn release(
        &mut self,
        _lease: &mornlea_server::contracts::FrozenLease,
        _deadline: Deadline,
    ) -> Result<(), ServerError> {
        Ok(())
    }

    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}

fn owner(script: ScriptAgent, clock: &Arc<StepClock>) -> MemoryOwner {
    MemoryOwner::new(
        Box::new(script),
        clock.clone(),
        ClientInstanceId::try_from_bytes(uuid(2)).unwrap(),
        NamespaceId::try_from_bytes(uuid(3)).unwrap(),
        LeaseId::try_from_bytes(uuid(30)).unwrap(),
    )
}

fn active_mirror(epoch: u64) -> MemoryMirror {
    MemoryMirror {
        active: true,
        epoch,
        revision: 0,
        operation: None,
        summary: String::new(),
        tombstone: None,
    }
}

fn reservation() -> CommitReservation {
    CommitReservation::try_new(
        companion(),
        1,
        operation(60),
        0,
        "关服确认的 mirror".to_owned(),
        "已经完成了。".to_owned(),
    )
    .unwrap()
}

fn leased_for(request_tag: u8) -> LeasedIdentity {
    LeasedIdentity {
        base: mornlea_server::contracts::BaseIdentity {
            request_id: request_id(request_tag),
            client_instance_id: ClientInstanceId::try_from_bytes(uuid(2)).unwrap(),
            namespace_id: NamespaceId::try_from_bytes(uuid(3)).unwrap(),
        },
        lease_id: LeaseId::try_from_bytes(uuid(30)).unwrap(),
    }
}

fn commit_response(epoch: u64, op_tag: u8, revision: u64) -> AgentResponse {
    AgentResponse::Commit(CommitResponse {
        leased: leased_for(50),
        companion_id: companion(),
        memory_epoch: epoch,
        operation_id: operation(op_tag),
        committed_revision: NonZeroU64::new(revision).unwrap(),
    })
}

/// An unknown commit keeps the same operation reserved and retries it
/// identically; a fenced commit finally applies with `revision == base + 1`.
#[test]
fn unknown_commit_same_operation() {
    let (_start, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = owner(script.clone(), &clock);
    owner.set_mirror(companion(), active_mirror(1));
    owner.reserve(reservation()).expect("reservation admits");

    owner
        .commit(companion(), request_id(50))
        .expect("commit submits");
    script.push(AgentPoll::Failed(not_found()));
    let settled = owner.poll_commits();
    assert_eq!(
        settled,
        vec![CommitSettled::Failed {
            companion: companion(),
            error: not_found(),
        }]
    );
    assert_eq!(
        owner.reservation(companion()),
        Some(&reservation()),
        "unknown commit drops the reserved operation"
    );

    owner
        .commit(companion(), request_id(51))
        .expect("retry submits");
    let bodies = script.commit_bodies();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0], bodies[1], "retry opens a new operation");
    assert_eq!(bodies[1].0, 1);
    assert_eq!(bodies[1].1, operation(60).bytes());
    assert_eq!(bodies[1].2, 0);

    script.push(AgentPoll::Completed(commit_response(1, 60, 1)));
    let settled = owner.poll_commits();
    assert_eq!(
        settled,
        vec![CommitSettled::Applied {
            companion: companion(),
            revision: 1,
        }]
    );
    let mirror = owner.mirror(companion()).expect("mirror kept");
    assert_eq!(mirror.revision, 1);
    assert_eq!(mirror.operation, Some(operation(60)));
    assert!(owner.reservation(companion()).is_none());
}

/// Commit and delete responses are fenced by epoch, operation, and revision:
/// a wrong epoch or a skipped revision changes nothing, delete advances the
/// epoch by exactly one, and the old epoch can never resurrect memory.
#[test]
fn epoch_and_revision_fence() {
    let (_start, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = owner(script.clone(), &clock);
    owner.set_mirror(companion(), active_mirror(1));
    owner.reserve(reservation()).expect("reservation admits");
    owner
        .commit(companion(), request_id(50))
        .expect("commit submits");

    script.push(AgentPoll::Completed(commit_response(2, 60, 1)));
    assert_eq!(
        owner.poll_commits(),
        vec![CommitSettled::Fenced {
            companion: companion()
        }],
        "wrong epoch applies"
    );
    assert_eq!(
        owner.mirror(companion()).unwrap().revision,
        0,
        "wrong epoch mutates the mirror"
    );

    owner
        .commit(companion(), request_id(52))
        .expect("retry submits");
    script.push(AgentPoll::Completed(commit_response(1, 60, 5)));
    assert_eq!(
        owner.poll_commits(),
        vec![CommitSettled::Fenced {
            companion: companion()
        }],
        "skipped revision applies"
    );
    assert!(owner.reservation(companion()).is_some());

    owner
        .delete(companion(), request_id(53), operation(61))
        .expect("delete submits");
    script.push(AgentPoll::Completed(AgentResponse::Delete(
        DeleteResponse {
            leased: leased_for(53),
            companion_id: companion(),
            memory_epoch: 2,
            tombstone_operation_id: operation(61),
        },
    )));
    assert_eq!(
        owner.poll_deletes(),
        vec![DeleteSettled::Deleted {
            companion: companion(),
            epoch: 2,
        }]
    );
    let mirror = owner.mirror(companion()).expect("mirror kept");
    assert!(!mirror.active);
    assert_eq!(mirror.epoch, 2);
    assert_eq!(mirror.tombstone, Some(operation(61)));

    // The old epoch cannot resurrect memory: an epoch-1 commit has no
    // reservation to apply to, and an epoch-1 reconcile is fenced.
    assert!(
        owner.commit(companion(), request_id(54)).is_err(),
        "commit after delete submits"
    );
    owner
        .reconcile(companion(), request_id(55))
        .expect("reconcile submits");
    script.push(AgentPoll::Completed(AgentResponse::Reconcile(
        ReconcileResponse::Active {
            leased: leased_for(55),
            companion_id: companion(),
            memory_epoch: 1,
            memory: MemoryState::Absent,
        },
    )));
    assert_eq!(
        owner.poll_reconciles(),
        vec![ReconcileSettled::Fenced {
            companion: companion()
        }]
    );
    let mirror = owner.mirror(companion()).expect("mirror kept");
    assert!(!mirror.active, "old epoch resurrects memory");
    assert_eq!(mirror.epoch, 2);
}

/// Shutdown finalization retries pending memory work under a fresh context
/// per attempt: the first attempt times out with the operation retained, and
/// the second attempt completes it.
#[test]
fn shutdown_reconcile_fresh_context() {
    let (start, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = owner(script.clone(), &clock);
    owner.set_mirror(companion(), active_mirror(1));
    owner.reserve(reservation()).expect("reservation admits");
    owner
        .commit(companion(), request_id(50))
        .expect("commit submits");
    script.park(request_id(50));

    let first_deadline = Deadline::at(start + Duration::from_millis(50));
    owner
        .begin_attempt(first_deadline)
        .expect("first attempt opens");
    assert_eq!(owner.attempt(), 1);
    let failed = owner.drain(first_deadline).expect_err("drain succeeds");
    assert_eq!(
        failed,
        ServerError::Timeout {
            operation: mornlea_server::contracts::Operation::Shutdown,
        }
    );
    assert_eq!(
        owner.reservation(companion()),
        Some(&reservation()),
        "timed-out attempt drops the operation identity"
    );

    let second_deadline = Deadline::at(start + Duration::from_secs(20));
    owner
        .begin_attempt(second_deadline)
        .expect("second attempt opens");
    assert_eq!(owner.attempt(), 2);
    assert_eq!(
        owner.attempt_deadline(),
        Some(second_deadline),
        "second attempt reuses the expired context"
    );
    script.unpark(request_id(50));
    script.push(AgentPoll::Completed(commit_response(1, 60, 1)));
    let report = owner.drain(second_deadline).expect("retry drains");
    assert_eq!(report.completed, 1);
    assert_eq!(report.outstanding, 0);
    assert_eq!(owner.mirror(companion()).unwrap().revision, 1);
}

#[test]
fn pending_memory_counts_semantic_ownership_once_and_cleanup_separately() {
    let (_, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = owner(script.clone(), &clock);
    assert_eq!(owner.pending().outstanding, 0);
    owner.set_mirror(companion(), active_mirror(1));
    owner.reserve(reservation()).unwrap();
    assert_eq!(owner.pending().outstanding, 1);
    owner.commit(companion(), request_id(50)).unwrap();
    assert_eq!(owner.pending().outstanding, 1);
    script.push(AgentPoll::Failed(unavailable()));
    script
        .state
        .refuse_retirement
        .lock()
        .unwrap()
        .insert(request_id(50));
    assert_eq!(owner.poll_commits().len(), 1);
    assert_eq!(owner.pending().outstanding, 2);
    owner
        .delete(companion(), request_id(51), operation(61))
        .unwrap();
    assert_eq!(owner.pending().outstanding, 2);
    script.push(AgentPoll::Completed(AgentResponse::Delete(
        DeleteResponse {
            leased: leased_for(51),
            companion_id: companion(),
            memory_epoch: 99,
            tombstone_operation_id: operation(61),
        },
    )));
    assert_eq!(
        owner.poll_deletes(),
        vec![DeleteSettled::Fenced {
            companion: companion()
        }]
    );
    assert_eq!(owner.pending().outstanding, 2);
    assert_eq!(owner.pending().completed, 0);
}

#[test]
fn pending_memory_retains_delete_intent_after_rpc_retirement() {
    let (_, clock) = StepClock::start();
    for terminal in [
        AgentPoll::Failed(unavailable()),
        AgentPoll::Completed(AgentResponse::Delete(DeleteResponse {
            leased: leased_for(51),
            companion_id: companion(),
            memory_epoch: 99,
            tombstone_operation_id: operation(61),
        })),
    ] {
        let script = ScriptAgent::new();
        let mut owner = owner(script.clone(), &clock);
        owner.set_mirror(companion(), active_mirror(1));
        owner
            .delete(companion(), request_id(51), operation(61))
            .unwrap();
        script.push(terminal);
        assert_eq!(owner.poll_deletes().len(), 1);
        assert_eq!(*script.state.retired.lock().unwrap(), vec![request_id(51)]);
        assert!(owner.reservation(companion()).is_none());
        assert_eq!(owner.pending().outstanding, 1);
    }
}

/// Reconcile backoff waits `1, 2, 4, 8, 16, 32` ticks capped: consecutive
/// failures arm the wait, ticks count it down, and later companions still
/// converge in the same batch.
#[test]
fn reconcile_retry_backoff_schedule() {
    assert_eq!(
        [1, 2, 3, 4, 5, 6, 7].map(reconcile_wait_ticks),
        [1, 2, 4, 8, 16, 32, 32]
    );
    let (_start, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = owner(script.clone(), &clock);
    let other = CompanionId::try_from_bytes(uuid(21)).unwrap();
    owner.set_mirror(companion(), active_mirror(1));
    owner.set_mirror(other, active_mirror(1));

    owner
        .reconcile(companion(), request_id(50))
        .expect("first submits");
    owner
        .reconcile(other, request_id(51))
        .expect("second submits");
    script.push(AgentPoll::Failed(unavailable()));
    script.push(AgentPoll::Completed(AgentResponse::Reconcile(
        ReconcileResponse::Active {
            leased: leased_for(51),
            companion_id: other,
            memory_epoch: 1,
            memory: MemoryState::Absent,
        },
    )));
    let settled = owner.poll_reconciles();
    assert_eq!(settled.len(), 2);
    assert!(matches!(settled[0], ReconcileSettled::NotReady { .. }));
    assert!(matches!(settled[1], ReconcileSettled::Ready { .. }));
    assert_eq!(owner.retry_attempts(companion()), 1);
    assert_eq!(owner.retry_wait(companion()), 1);
    assert!(owner.is_ready(other));
}

#[test]
fn memory_terminal_lanes_reclaim_once_and_fenced_delete_can_retry() {
    let (_, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = owner(script.clone(), &clock);
    owner.set_mirror(companion(), active_mirror(1));
    owner.reserve(reservation()).unwrap();
    owner.commit(companion(), request_id(50)).unwrap();
    script.state.terminals.lock().unwrap().insert(
        request_id(50),
        AgentPoll::Completed(commit_response(1, 60, 1)),
    );
    assert_eq!(owner.poll_commits().len(), 1);
    assert!(owner.poll_commits().is_empty());
    owner.reconcile(companion(), request_id(51)).unwrap();
    script.state.terminals.lock().unwrap().insert(
        request_id(51),
        AgentPoll::Completed(AgentResponse::Reconcile(ReconcileResponse::Active {
            leased: leased_for(51),
            companion_id: companion(),
            memory_epoch: 1,
            memory: MemoryState::Present {
                revision: NonZeroU64::new(1).unwrap(),
                operation_id: operation(60),
                summary: reservation().summary,
            },
        })),
    );
    assert_eq!(owner.poll_reconciles().len(), 1);
    assert!(owner.poll_reconciles().is_empty());
    owner
        .delete(companion(), request_id(52), operation(61))
        .unwrap();
    script.state.terminals.lock().unwrap().insert(
        request_id(52),
        AgentPoll::Completed(AgentResponse::Delete(DeleteResponse {
            leased: leased_for(52),
            companion_id: companion(),
            memory_epoch: 99,
            tombstone_operation_id: operation(61),
        })),
    );
    assert_eq!(
        owner.poll_deletes(),
        vec![DeleteSettled::Fenced {
            companion: companion()
        }]
    );
    assert!(owner.poll_deletes().is_empty());
    assert!(owner.mirror(companion()).unwrap().active);
    owner
        .delete(companion(), request_id(53), operation(61))
        .unwrap();
    script.state.terminals.lock().unwrap().insert(
        request_id(53),
        AgentPoll::Completed(AgentResponse::Delete(DeleteResponse {
            leased: leased_for(53),
            companion_id: companion(),
            memory_epoch: 2,
            tombstone_operation_id: operation(61),
        })),
    );
    assert_eq!(owner.poll_deletes().len(), 1);
    assert!(owner.poll_deletes().is_empty());
    assert_eq!(
        *script.state.retired.lock().unwrap(),
        vec![
            request_id(50),
            request_id(51),
            request_id(52),
            request_id(53)
        ]
    );
}

#[test]
fn memory_retirement_refusal_bounds_requests_then_reaps() {
    let (_, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = owner(script.clone(), &clock);
    owner.set_mirror(companion(), active_mirror(1));
    owner.reserve(reservation()).unwrap();
    for tag in 100..164 {
        let id = request_id(tag);
        owner.commit(companion(), id).unwrap();
        script
            .state
            .terminals
            .lock()
            .unwrap()
            .insert(id, AgentPoll::Failed(unavailable()));
        script.state.refuse_retirement.lock().unwrap().insert(id);
        assert_eq!(owner.poll_commits().len(), 1);
        assert!(owner.poll_commits().is_empty());
    }
    let before = script.submitted().len();
    assert_eq!(
        owner.commit(companion(), request_id(200)),
        Err(ServerError::Capacity {
            resource: mornlea_server::contracts::Resource::AgentRuns,
            limit: 64,
            observed: 65
        })
    );
    assert_eq!(script.submitted().len(), before);
    script.state.refuse_retirement.lock().unwrap().clear();
    owner.commit(companion(), request_id(200)).unwrap();
    assert_eq!(script.state.retired.lock().unwrap().len(), 64);
    assert_eq!(owner.reservation(companion()), Some(&reservation()));
}

struct EchoMemoryWire;

impl mornlea_server::agent::lease::AgentWire for EchoMemoryWire {
    fn rpc_cancellable(
        &self,
        request: AgentRequest,
        _deadline: Deadline,
        _cancellation: &mornlea_server::agent::lease::RpcCancellation,
    ) -> Result<AgentResponse, ServerError> {
        use mornlea_server::contracts::{LeaseResponse, ReconcileRequest};
        Ok(match request {
            AgentRequest::Acquire(base) => AgentResponse::Acquire(LeaseResponse {
                leased: LeasedIdentity {
                    base,
                    lease_id: leased_for(1).lease_id,
                },
            }),
            AgentRequest::Commit(request) => AgentResponse::Commit(CommitResponse {
                leased: request.leased,
                companion_id: request.companion_id,
                memory_epoch: request.memory_epoch,
                operation_id: request.operation_id,
                committed_revision: NonZeroU64::new(request.base_revision + 1).unwrap(),
            }),
            AgentRequest::Reconcile(ReconcileRequest::Active {
                leased,
                companion_id,
                memory_epoch,
                mirror,
            }) => AgentResponse::Reconcile(ReconcileResponse::Active {
                leased,
                companion_id,
                memory_epoch,
                memory: mirror,
            }),
            AgentRequest::Delete(request) => AgentResponse::Delete(DeleteResponse {
                leased: request.leased,
                companion_id: request.companion_id,
                memory_epoch: request.new_memory_epoch,
                tombstone_operation_id: request.tombstone_operation_id,
            }),
            _ => return Err(unavailable()),
        })
    }
    fn close(&self) {}
}

#[test]
fn real_lease_reclaims_more_than_sixty_four_memory_cycles() {
    use mornlea_server::agent::lease::{LeaseConfig, LeaseController};
    let (_, clock) = StepClock::start();
    let mut agent = LeaseController::try_new(
        LeaseConfig {
            client_instance_id: leased_for(1).base.client_instance_id,
            namespace_id: leased_for(1).base.namespace_id,
        },
        Arc::new(EchoMemoryWire),
        clock.clone(),
    )
    .unwrap();
    agent.refresh();
    agent.freeze(&*clock).unwrap();
    let mut observed = agent.clone();
    let mut owner = MemoryOwner::new(
        Box::new(agent),
        clock.clone(),
        leased_for(1).base.client_instance_id,
        leased_for(1).base.namespace_id,
        leased_for(1).lease_id,
    );
    for cycle in 0..65u8 {
        let tag = 10 + cycle * 3;
        owner.set_mirror(companion(), active_mirror(1));
        owner.reserve(reservation()).unwrap();
        owner.commit(companion(), request_id(tag)).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while owner.poll_commits().is_empty() {
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert_eq!(owner.mirror(companion()).unwrap().revision, 1);
        assert_eq!(observed.retained_requests(), 0, "commit cycle {cycle}");
        owner.reconcile(companion(), request_id(tag + 1)).unwrap();
        while owner.poll_reconciles().is_empty() {
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert!(owner.is_ready(companion()));
        assert_eq!(observed.retained_requests(), 0, "reconcile cycle {cycle}");
        owner
            .delete(companion(), request_id(tag + 2), operation(61))
            .unwrap();
        while owner.poll_deletes().is_empty() {
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert!(!owner.mirror(companion()).unwrap().active);
        assert_eq!(observed.retained_requests(), 0, "delete cycle {cycle}");
    }
    observed.close(Deadline::at(clock.monotonic())).unwrap();
}

#[test]
fn repeatable_memory_refusals_do_not_rearm_or_mutate_twice() {
    let (_, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = owner(script.clone(), &clock);
    owner.set_mirror(companion(), active_mirror(1));
    owner.reserve(reservation()).unwrap();
    owner.reconcile(companion(), request_id(50)).unwrap();
    script
        .state
        .terminals
        .lock()
        .unwrap()
        .insert(request_id(50), AgentPoll::Failed(unavailable()));
    assert_eq!(
        owner.poll_reconciles(),
        vec![ReconcileSettled::NotReady {
            companion: companion()
        }]
    );
    for _ in 0..2 {
        assert!(owner.poll_reconciles().is_empty());
    }
    assert_eq!(owner.retry_attempts(companion()), 1);
    assert_eq!(owner.reservation(companion()), Some(&reservation()));
    owner
        .delete(companion(), request_id(51), operation(61))
        .unwrap();
    script.state.terminals.lock().unwrap().insert(
        request_id(51),
        AgentPoll::Completed(commit_response(1, 60, 1)),
    );
    assert_eq!(
        owner.poll_deletes(),
        vec![DeleteSettled::Failed {
            companion: companion(),
            error: unavailable()
        }]
    );
    for _ in 0..2 {
        assert!(owner.poll_deletes().is_empty());
    }
    assert_eq!(owner.mirror(companion()), Some(&active_mirror(1)));
    assert_eq!(owner.reservation(companion()), Some(&reservation()));
    assert_eq!(
        *script.state.retired.lock().unwrap(),
        vec![request_id(50), request_id(51)]
    );
}

#[test]
fn unresolved_delete_intent_refuses_conflicting_retry() {
    let (_, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = owner(script.clone(), &clock);
    owner.set_mirror(companion(), active_mirror(1));
    owner
        .delete(companion(), request_id(50), operation(61))
        .unwrap();
    script
        .state
        .terminals
        .lock()
        .unwrap()
        .insert(request_id(50), AgentPoll::Failed(unavailable()));
    assert_eq!(owner.poll_deletes().len(), 1);
    assert_eq!(
        owner.delete(companion(), request_id(51), operation(62)),
        Err(ServerError::InvalidInput {
            field: "memory delete intent"
        })
    );
    assert_eq!(script.submitted().len(), 1);
    assert_eq!(owner.mirror(companion()), Some(&active_mirror(1)));
    owner
        .delete(companion(), request_id(52), operation(61))
        .unwrap();
    let AgentRequest::Delete(retry) = script.submitted()[1].clone() else {
        panic!("delete")
    };
    assert_eq!(retry.new_memory_epoch, 2);
    assert_eq!(retry.tombstone_operation_id, operation(61));
}

#[test]
fn unresolved_delete_intents_refuse_sixty_fifth_distinct_name() {
    let (_, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = owner(script.clone(), &clock);
    for tag in 100..164u8 {
        let id = CompanionId::try_from_bytes(uuid(tag)).unwrap();
        owner.set_mirror(id, active_mirror(1));
        owner.delete(id, request_id(tag), operation(61)).unwrap();
        script
            .state
            .terminals
            .lock()
            .unwrap()
            .insert(request_id(tag), AgentPoll::Failed(unavailable()));
        assert_eq!(owner.poll_deletes().len(), 1);
    }
    let id = CompanionId::try_from_bytes(uuid(200)).unwrap();
    owner.set_mirror(id, active_mirror(1));
    assert_eq!(
        owner.delete(id, request_id(200), operation(61)),
        Err(ServerError::Capacity {
            resource: mornlea_server::contracts::Resource::AgentRuns,
            limit: 64,
            observed: 65
        })
    );
    assert_eq!(script.submitted().len(), 64);
    assert_eq!(owner.mirror(id), Some(&active_mirror(1)));
}
