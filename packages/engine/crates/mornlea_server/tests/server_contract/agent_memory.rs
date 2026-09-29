//! Companion memory ownership: commit fencing, epoch advance, and shutdown
//! finalization.
//!
//! Expected values mirror the Go authority in
//! `packages/server/server/companion_dialogue.go` (`applyMemoryCommitOutcome`
//! echoes epoch and operation with `committedRevision == baseRevision + 1`,
//! errors re-arm reconcile for the same operation, and shutdown retries
//! pending memory work under a fresh context per attempt).

use std::collections::VecDeque;
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
}

impl ScriptAgent {
    fn new() -> Self {
        Self {
            state: Arc::new(ScriptState {
                polls: Mutex::new(VecDeque::new()),
                submitted: Mutex::new(Vec::new()),
                parked: Mutex::new(Vec::new()),
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

    fn cancel(&mut self, _id: AgentRequestId, _deadline: Deadline) -> Result<(), ServerError> {
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
