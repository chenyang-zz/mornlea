//! Real disk ownership is exercised with an explicitly scripted remote Agent provider.
use super::actual_disk::{
    RealClock, Root, autosave, deadline, failing_disk, reopen, scheduler, setup,
};
use super::*;
use mornlea_server::agent::memory::{CommitReservation, MemoryMirror, MemoryOwner};
use std::{
    collections::BTreeMap,
    num::NonZeroU64,
    sync::{Arc, Mutex},
    thread,
    time::Instant,
};

/// Immediate memory replies are a test double; filesystem writes below use the actual backend.
#[derive(Default)]
struct EchoAgent {
    responses: BTreeMap<AgentRequestId, AgentResponse>,
}
impl AgentHandle for EchoAgent {
    fn submit(&mut self, request: AgentRequest) -> Result<AgentRequestId, ServerError> {
        let response = match request {
            AgentRequest::Commit(r) => AgentResponse::Commit(CommitResponse {
                leased: r.leased,
                companion_id: r.companion_id,
                memory_epoch: r.memory_epoch,
                operation_id: r.operation_id,
                committed_revision: NonZeroU64::new(r.base_revision + 1).unwrap(),
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
            _ => {
                return Err(ServerError::InvalidInput {
                    field: "scripted_memory_request",
                });
            }
        };
        let identity = match &response {
            AgentResponse::Commit(r) => r.leased.base.request_id,
            AgentResponse::Reconcile(ReconcileResponse::Active { leased, .. }) => {
                leased.base.request_id
            }
            _ => unreachable!(),
        };
        self.responses.insert(identity, response);
        Ok(identity)
    }
    fn poll(&mut self, id: AgentRequestId) -> AgentPoll {
        self.responses
            .remove(&id)
            .map(AgentPoll::Completed)
            .unwrap_or(AgentPoll::Pending)
    }
    fn cancel(&mut self, id: AgentRequestId, _deadline: Deadline) -> Result<(), ServerError> {
        self.responses.remove(&id);
        Ok(())
    }
    fn freeze(&mut self, _clock: &dyn Clock) -> Option<FrozenLease> {
        None
    }
    fn release(&mut self, _lease: &FrozenLease, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        self.responses.clear();
        Ok(())
    }
}

#[test]
fn final_agent_acceptance_updates_latest_ledger_before_real_failed_target_retry_flush_and_reopen() {
    let root = Root::new();
    let (mut state, mut store) = setup(&root, with_task(1));
    autosave(&mut state, &mut store);
    store.close(deadline()).unwrap();
    state.take_companion_chat_planning(id(1)).unwrap().unwrap();
    state.advance_tick(TickBudget::full()).unwrap();
    let mut old = current(&state).clone();
    old.revision = 11;
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let mut store = scheduler(failing_disk(&root, attempts.clone()));
    let until = deadline();
    let mut tick = 6000;
    loop {
        store.drive_workers();
        store
            .poll_tick(tick, SaveBudget::default(), &mut state)
            .unwrap();
        if store.pending_retry_jobs() > 0 {
            break;
        }
        assert!(!until.expired(Instant::now()));
        tick += 1;
        thread::yield_now()
    }
    let mut owner = MemoryOwner::new(
        Box::<EchoAgent>::default(),
        Arc::new(RealClock),
        ClientInstanceId::try_from_bytes(raw_id(220)).unwrap(),
        NamespaceId::try_from_bytes(raw_id(240)).unwrap(),
        LeaseId::try_from_bytes(raw_id(221)).unwrap(),
    );
    owner.set_mirror(
        id(1),
        MemoryMirror {
            active: true,
            epoch: 7,
            revision: 2,
            operation: Some(OperationId::try_from_bytes(raw_id(180)).unwrap()),
            summary: "untouched memory".into(),
            tombstone: None,
        },
    );
    owner
        .reserve(
            CommitReservation::try_new(
                id(1),
                7,
                OperationId::try_from_bytes(raw_id(200)).unwrap(),
                2,
                "final Agent memory".into(),
                "Finished.".into(),
            )
            .unwrap(),
        )
        .unwrap();
    state.begin_close();
    state
        .run_final(&mut mornlea_server::core::step::AuthoritativeFinalReducer)
        .unwrap();
    let final_deadline = deadline();
    {
        let mut finalizer = owner.authoritative_finalizer();
        finalizer.begin_attempt(final_deadline).unwrap();
        let mut completed = 0;
        loop {
            let result = finalizer
                .drain_authority(&mut state, final_deadline)
                .unwrap();
            completed += result.completed;
            if result.outstanding == 0 {
                break;
            }
            assert!(!final_deadline.expired(Instant::now()))
        }
        assert_eq!(completed, 1);
    }
    assert!(owner.reservation(id(1)).is_none());
    let mut latest = current(&state).clone();
    latest.revision = 12;
    assert_eq!(latest.lifecycles[0].summary, "final Agent memory");
    assert_eq!(latest.records, old.records);
    assert_eq!(latest.queues, old.queues);
    assert_eq!(latest.lifecycles[1], old.lifecycles[1]);
    store.flush(deadline(), &mut state, &RealClock).unwrap();
    store.close(deadline()).unwrap();
    let bytes = attempts.lock().unwrap();
    assert_eq!(bytes.len(), 3);
    assert_eq!(bytes[0], mornlea_storage::encode_companions(&old).unwrap());
    assert_eq!(bytes[0], bytes[1]);
    assert_eq!(
        bytes[2],
        mornlea_storage::encode_companions(&latest).unwrap()
    );
    drop(bytes);
    reopen(&root, latest);
}
