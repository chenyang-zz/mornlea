//! Actual Python HTTP settlement accepts a complete authority ledger without claiming a disk flush.
use super::*;
use mornlea_domain::CompanionName;
use mornlea_server::{
    contracts::{MemoryFinalizer, SaveBudget, SaveKey, SaveMode, SaveValue, ServerLimits},
    state::AuthorityState,
};
use mornlea_storage::{
    CompanionBody, Inventory, PlayerId, StoredCompanionLifecycle, StoredCompanions,
};

#[test]
fn real_python_commit_and_fresh_final_drain_accept_complete_authority_before_confirmation() {
    let _serial = HELPER_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let token = "authoritative-memory-token";
    let mut child = spawn_helper("memory", token);
    let (_, clock) = StepClock::start();
    let mut agent = acquire_lease(&clock, &child.endpoint(), token);
    let (lease, _) = agent.current_lease().unwrap();
    let mut state = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        42,
    )
    .unwrap();
    state.enable_source_player_restoration(1).unwrap();
    state.enable_live_chunks().unwrap();
    state.enable_actor_saves().unwrap();
    state.enable_player_persistence().unwrap();
    let saved = PlayerId::from_bytes(companion_id().bytes());
    state
        .enable_companion_persistence(
            &[(
                companion_id(),
                CompanionName::try_from_canonical("Nova".into()).unwrap(),
            )],
            StoredCompanions {
                source_schema: 5,
                revision: 9,
                agent_namespace_id: PlayerId::from_bytes(namespace_id().bytes()),
                records: vec![CompanionBody {
                    id: saved,
                    dimension: 0,
                    position: [8.5, 65.0, 8.5],
                    yaw: 0.0,
                    pitch: 0.0,
                    inventory: Inventory::default(),
                }],
                lifecycles: vec![StoredCompanionLifecycle {
                    id: saved,
                    active: true,
                    memory_epoch: 1,
                    memory_revision: 0,
                    memory_operation_id: PlayerId::default(),
                    summary: String::new(),
                    tombstone_operation_id: PlayerId::default(),
                }],
                queues: Vec::new(),
            },
        )
        .unwrap();
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
    owner.reconcile(companion_id(), request_id(50)).unwrap();
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        let settled = owner.poll_reconciles_authoritative(&mut state);
        if !settled.is_empty() {
            assert!(matches!(
                settled.as_slice(),
                [ReconcileSettled::Ready { .. }]
            ));
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1))
    }
    let proposal = CommitReservation::try_new(
        companion_id(),
        1,
        operation(60),
        0,
        "actual committed memory".into(),
        "Finished.".into(),
    )
    .unwrap();
    owner.reserve(proposal.clone()).unwrap();
    owner.commit(companion_id(), request_id(51)).unwrap();
    loop {
        let settled = owner.poll_commits_authoritative(&mut state);
        if !settled.is_empty() {
            assert_eq!(settled.len(), 1);
            assert!(matches!(
                settled[0].outcome,
                CommitSettled::Applied { revision: 1, .. }
            ));
            assert_eq!(settled[0].fulfilled, Some(proposal));
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1))
    }
    assert_eq!(
        state
            .companion_memory_lifecycle(companion_id())
            .unwrap()
            .memory_revision,
        1
    );
    assert!(owner.reservation(companion_id()).is_none());
    let _frozen = agent.freeze(&*clock).unwrap();
    owner
        .reserve(
            CommitReservation::try_new(
                companion_id(),
                1,
                operation(61),
                1,
                "actual final memory".into(),
                "Saved.".into(),
            )
            .unwrap(),
        )
        .unwrap();
    state.begin_close();
    state
        .run_final(&mut mornlea_server::core::step::AuthoritativeFinalReducer)
        .unwrap();
    let deadline = Deadline::after(clock.monotonic(), Duration::from_secs(15)).unwrap();
    let until = Instant::now() + Duration::from_secs(15);
    {
        let mut finalizer = owner.authoritative_finalizer();
        finalizer.begin_attempt(deadline).unwrap();
        let mut completed = 0;
        loop {
            let report = finalizer.drain_authority(&mut state, deadline).unwrap();
            completed += report.completed;
            if report.outstanding == 0 {
                break;
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1))
        }
        assert_eq!(completed, 1);
    }
    let lifecycle = state.companion_memory_lifecycle(companion_id()).unwrap();
    assert_eq!(lifecycle.memory_revision, 2);
    assert_eq!(lifecycle.summary, "actual final memory");
    assert_eq!(
        lifecycle.memory_operation_id.to_bytes(),
        operation(61).bytes()
    );
    let target = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(target.len(), 1);
    assert_eq!(target[0].revision, 10);
    let SaveValue::Companions(latest) = &target[0].value else {
        panic!("companions")
    };
    assert_eq!(latest.lifecycles[0].summary, "actual final memory");
    assert!(owner.reservation(companion_id()).is_none());
    assert_eq!(owner.mirror(companion_id()).unwrap().revision, 2);
    assert!(matches!(
        state.actor_save_current(&SaveKey::Companions).unwrap().1,
        SaveValue::Companions(_)
    ));
    agent.close(deadline).unwrap();
    child.shutdown("memory");
}
