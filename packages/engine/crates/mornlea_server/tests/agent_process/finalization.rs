//! Real Python memory/lease/wire ownership through retryable shutdown.
//! Non-memory lifecycle doubles expose phase ordering, not disk durability.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mornlea_server::agent::http::AgentHttpWire;
use mornlea_server::agent::lease::{
    AgentWire, ControlPhase, LeaseConfig, LeaseController, RpcCancellation,
};
use mornlea_server::agent::memory::{
    CommitReservation, CommitSettled, MemoryMirror, MemoryOwner, ReconcileSettled,
};
use mornlea_server::contracts::*;
use mornlea_server::core::shutdown::{ShutdownPorts, shutdown};
use mornlea_server::state::AuthorityState;

use super::integration::HELPER_SERIAL;
use super::process::{
    StepClock, client_id, companion_id, namespace_id, operation, request_id, spawn_helper,
};

#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<&'static str>>>);
impl Events {
    fn note(&self, event: &'static str) {
        self.0.lock().unwrap().push(event);
    }
    fn snapshot(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().clone()
    }
}

/// Lose a response only after actual HTTP commits; gate reconcile without
/// fabricating remote memory. The next attempt observes the real stored result.
struct MemoryFaults {
    inner: AgentHttpWire,
    lose_commit: AtomicBool,
    hold_reconcile: AtomicBool,
    reconcile_entered: AtomicBool,
    commits: AtomicUsize,
    events: Events,
}

impl AgentWire for MemoryFaults {
    fn rpc_cancellable(
        &self,
        request: AgentRequest,
        deadline: Deadline,
        cancellation: &RpcCancellation,
    ) -> Result<AgentResponse, ServerError> {
        if matches!(&request, AgentRequest::Reconcile(_)) {
            self.reconcile_entered.store(true, Ordering::SeqCst);
            while self.hold_reconcile.load(Ordering::SeqCst) {
                if cancellation.is_cancelled() {
                    return Err(ServerError::Cancelled);
                }
                if deadline.expired(Instant::now()) {
                    return Err(ServerError::Timeout {
                        operation: Operation::AgentRpc,
                    });
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let is_commit = matches!(&request, AgentRequest::Commit(_));
        let is_reconcile = matches!(&request, AgentRequest::Reconcile(_));
        let is_release = matches!(&request, AgentRequest::Release(_));
        let response = self
            .inner
            .rpc_cancellable(request, deadline, cancellation)?;
        if is_commit {
            self.commits.fetch_add(1, Ordering::SeqCst);
            self.events.note("commit-confirmed");
            if self.lose_commit.swap(false, Ordering::SeqCst) {
                return Err(ServerError::Timeout {
                    operation: Operation::AgentRpc,
                });
            }
        }
        if is_reconcile {
            self.events.note("reconcile-confirmed");
        }
        if is_release {
            self.events.note("release-confirmed");
        }
        Ok(response)
    }
    fn close(&self) {
        self.events.note("wire-close");
        self.inner.close();
    }
}

/// A failed assertion also opens the test gate before helper teardown.
struct OpenGateOnDrop(Arc<MemoryFaults>);
impl Drop for OpenGateOnDrop {
    fn drop(&mut self) {
        self.0.hold_reconcile.store(false, Ordering::SeqCst);
    }
}

fn unused() -> ServerError {
    ServerError::Internal {
        invariant: "memory shutdown integration unused port",
    }
}

/// Explicit non-memory doubles only record the shutdown boundary calls.
struct Lifecycle(Events);
impl FinalReducer for Lifecycle {
    fn reduce_final(&mut self, authority: &mut AuthorityState) -> Result<u64, ServerError> {
        self.0.note("final-tick");
        Ok(authority.next_tick())
    }
}
impl WorkerLifecycle for Lifecycle {
    fn stop_new(&mut self) -> Result<(), ServerError> {
        self.0.note("stop-workers");
        Ok(())
    }
    fn cancel(&mut self) -> Result<(), ServerError> {
        self.0.note("cancel-workers");
        Ok(())
    }
    fn wait(&mut self, _: Deadline) -> Result<(), ServerError> {
        self.0.note("wait-workers");
        Ok(())
    }
    fn close(&mut self, _: Deadline) -> Result<(), ServerError> {
        self.0.note("workers-close");
        Ok(())
    }
}
impl ActorPersistence for Lifecycle {
    fn flush(&mut self, _: SaveKey, _: Deadline) -> Result<FlushReport, ServerError> {
        self.0.note("flush");
        Ok(FlushReport::default())
    }
}
impl McpLifecycle for Lifecycle {
    fn close(&mut self, _: Deadline) -> Result<(), ServerError> {
        self.0.note("mcp-close");
        Ok(())
    }
}
impl StoreHandle for Lifecycle {
    fn submit(&mut self, request: SaveRequest) -> Result<SaveTicket, SubmitSaveError> {
        Err(SubmitSaveError {
            error: unused(),
            request,
        })
    }
    fn poll(&mut self, _: SaveTicket) -> SavePoll {
        panic!("unused memory integration save poll")
    }
    fn poll_tick(
        &mut self,
        _: u64,
        _: SaveBudget,
        _: &mut dyn SaveAuthority,
    ) -> Result<SaveScheduleReport, ServerError> {
        Err(unused())
    }
    fn cancel_pending(&mut self) -> Result<Vec<OwnedSnapshot>, ServerError> {
        Err(unused())
    }
    fn flush(
        &mut self,
        _: Deadline,
        _: &mut dyn SaveAuthority,
        _: &dyn Clock,
    ) -> Result<FlushReport, ServerError> {
        Err(unused())
    }
    fn sync(&mut self, _: Deadline) -> Result<(), ServerError> {
        self.0.note("store-sync");
        Ok(())
    }
    fn close(&mut self, _: Deadline) -> Result<(), ServerError> {
        self.0.note("store-close");
        Ok(())
    }
}

#[test]
fn real_memory_shutdown_retry_confirms_lost_commit_before_release() {
    let _serial = HELPER_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let token = "memory-shutdown-token";
    let mut child = spawn_helper("memory-shutdown", token);
    let (_, clock) = StepClock::start();
    let events = Events::default();
    let faults = Arc::new(MemoryFaults {
        inner: AgentHttpWire::try_new(&child.endpoint(), token, clock.clone()).unwrap(),
        lose_commit: AtomicBool::new(true),
        hold_reconcile: AtomicBool::new(false),
        reconcile_entered: AtomicBool::new(false),
        commits: AtomicUsize::new(0),
        events: events.clone(),
    });
    let _open_gate = OpenGateOnDrop(faults.clone());
    let mut agent = LeaseController::try_new(
        LeaseConfig {
            client_instance_id: client_id(),
            namespace_id: namespace_id(),
        },
        faults.clone(),
        clock.clone(),
    )
    .unwrap();
    agent.refresh();
    let (lease, fence) = agent.current_lease().unwrap();
    let mut owner = MemoryOwner::new(
        Box::new(agent.clone()),
        clock.clone(),
        client_id(),
        namespace_id(),
        lease,
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
    // The real gateway establishes the companion epoch on initial reconcile.
    owner.reconcile(companion_id(), request_id(50)).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let settled = owner.poll_reconciles();
        if !settled.is_empty() {
            assert!(
                matches!(settled.as_slice(), [ReconcileSettled::Ready { .. }]),
                "actual initial reconcile: {settled:?}"
            );
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
    faults.reconcile_entered.store(false, Ordering::SeqCst);
    faults.hold_reconcile.store(true, Ordering::SeqCst);
    let reservation = CommitReservation::try_new(
        companion_id(),
        1,
        operation(60),
        0,
        "confirmed original operation".to_owned(),
        "Done.".to_owned(),
    )
    .unwrap();
    owner.reserve(reservation.clone()).unwrap();
    owner.commit(companion_id(), request_id(51)).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let lost = loop {
        let settled = owner.poll_commits();
        if !settled.is_empty() {
            break settled;
        }
        assert!(Instant::now() < until, "actual commit did not settle");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(
        matches!(
            lost.as_slice(),
            [CommitSettled::Failed {
                error: ServerError::Timeout {
                    operation: Operation::AgentRpc
                },
                ..
            }]
        ),
        "actual lost commit: {lost:?}"
    );
    assert_eq!(faults.commits.load(Ordering::SeqCst), 1);
    assert_eq!(owner.reservation(companion_id()), Some(&reservation));
    assert_eq!(agent.retained_requests(), 0);

    let mut authority = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        7,
    )
    .unwrap();
    let mut reducer = Lifecycle(events.clone());
    let mut workers = Lifecycle(events.clone());
    let mut actors = Lifecycle(events.clone());
    let mut store = Lifecycle(events.clone());
    let mut mcp = Lifecycle(events.clone());
    let mut ports = ShutdownPorts {
        reducer: &mut reducer,
        workers: &mut workers,
        actors: &mut actors,
        memory: &mut owner,
        store: &mut store,
        agent: &mut agent,
        mcp: &mut mcp,
        clock: &*clock,
    };
    let deadline = Deadline::after(clock.monotonic(), Duration::from_millis(40)).unwrap();
    let failure = shutdown(&mut authority, &mut ports, deadline).unwrap_err();
    assert_eq!(
        failure.error,
        ServerError::Timeout {
            operation: Operation::Shutdown
        }
    );
    assert_eq!(failure.report.next, ShutdownPhase::FinalizeMemory);
    assert_eq!(failure.report.outstanding, 1);
    assert!(
        faults.reconcile_entered.load(Ordering::SeqCst),
        "actual finalizer did not admit reconcile"
    );
    assert_eq!(ports.agent.freeze(&*clock).unwrap().lease_fence, fence);
    assert!(
        !events
            .snapshot()
            .iter()
            .any(|event| matches!(*event, "flush" | "release-confirmed" | "wire-close"))
    );

    faults.hold_reconcile.store(false, Ordering::SeqCst);
    let deadline = Deadline::after(clock.monotonic(), Duration::from_secs(5)).unwrap();
    let report = shutdown(&mut authority, &mut ports, deadline).unwrap();
    assert_eq!(report.next, ShutdownPhase::Closed);
    assert_eq!(report.final_tick, Some(0));
    assert_eq!(ports.memory.pending().outstanding, 0);
    let observed = events.snapshot();
    assert_eq!(
        observed
            .iter()
            .filter(|event| **event == "final-tick")
            .count(),
        1
    );
    assert_eq!(
        observed
            .iter()
            .filter(|event| **event == "release-confirmed")
            .count(),
        1
    );
    assert_eq!(
        observed
            .iter()
            .filter(|event| **event == "wire-close")
            .count(),
        1
    );
    let confirmed = observed
        .iter()
        .rposition(|event| *event == "reconcile-confirmed")
        .unwrap();
    let flush = observed.iter().position(|event| *event == "flush").unwrap();
    let release = observed
        .iter()
        .position(|event| *event == "release-confirmed")
        .unwrap();
    let close = observed
        .iter()
        .position(|event| *event == "wire-close")
        .unwrap();
    assert!(confirmed < flush && flush < release && release < close);
    assert_eq!(
        shutdown(&mut authority, &mut ports, deadline).unwrap(),
        report
    );
    assert_eq!(events.snapshot(), observed);
    assert_eq!(owner.reservation(companion_id()), None);
    let mirror = owner.mirror(companion_id()).unwrap();
    assert_eq!(
        (mirror.epoch, mirror.revision, mirror.operation),
        (1, 1, Some(operation(60)))
    );
    assert_eq!(mirror.summary, reservation.summary);
    assert_eq!(faults.commits.load(Ordering::SeqCst), 1);
    assert_eq!(agent.retained_requests(), 0);
    assert_eq!(agent.control_phase(), ControlPhase::Closed);
    child.shutdown("memory-shutdown");
}
