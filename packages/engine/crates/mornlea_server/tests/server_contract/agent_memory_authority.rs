//! Scripted Agent terminals qualify authority acceptance separately from real HTTP and disk.
use super::*;
use mornlea_domain::CompanionName;
use mornlea_server::{
    contracts::{SaveBudget, SaveKey, SaveMode, SaveValue, ServerLimits},
    state::AuthorityState,
};
use mornlea_storage::{
    CompanionBody, Inventory, PlayerId, StoredCompanionLifecycle, StoredCompanions,
};

fn loaded(revision: u64) -> StoredCompanions {
    StoredCompanions {
        source_schema: 5,
        revision,
        agent_namespace_id: PlayerId::from_bytes(uuid(3)),
        records: vec![CompanionBody {
            id: PlayerId::from_bytes(companion().bytes()),
            dimension: 0,
            position: [8.5, 65.0, 8.5],
            yaw: 0.0,
            pitch: 0.0,
            inventory: Inventory::default(),
        }],
        lifecycles: vec![StoredCompanionLifecycle {
            id: PlayerId::from_bytes(companion().bytes()),
            active: true,
            memory_epoch: 7,
            memory_revision: 2,
            memory_operation_id: PlayerId::from_bytes(uuid(31)),
            summary: "old".into(),
            tombstone_operation_id: PlayerId::default(),
        }],
        queues: Vec::new(),
    }
}
fn authority_from(loaded: StoredCompanions) -> AuthorityState {
    let mut state = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        42,
    )
    .unwrap();
    state.enable_source_player_restoration(1).unwrap();
    state.enable_live_chunks().unwrap();
    state.enable_actor_saves().unwrap();
    state.enable_player_persistence().unwrap();
    state
        .enable_companion_persistence(
            &[(
                companion(),
                CompanionName::try_from_canonical("Nova".into()).unwrap(),
            )],
            loaded,
        )
        .unwrap();
    state
}
fn authority(revision: u64) -> AuthorityState {
    authority_from(loaded(revision))
}
fn current(state: &AuthorityState) -> &mornlea_storage::CompanionSave {
    let SaveValue::Companions(save) = state.actor_save_current(&SaveKey::Companions).unwrap().1
    else {
        panic!("companions")
    };
    save
}
fn seeded(script: &ScriptAgent, clock: &Arc<StepClock>) -> MemoryOwner {
    let mut owner = super::owner(script.clone(), clock);
    owner.set_mirror(
        companion(),
        MemoryMirror {
            active: true,
            epoch: 7,
            revision: 2,
            operation: Some(operation(31)),
            summary: "old".into(),
            tombstone: None,
        },
    );
    owner
}
fn proposal() -> CommitReservation {
    CommitReservation::try_new(
        companion(),
        7,
        operation(50),
        2,
        "new".into(),
        "Finished.".into(),
    )
    .unwrap()
}

#[test]
fn commit_updates_complete_authority_before_working_mirror_and_fulfills_the_whole_reservation_once()
{
    let (_, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = seeded(&script, &clock);
    let mut state = authority(9);
    let old = current(&state).clone();
    owner.reserve(proposal()).unwrap();
    owner.commit(companion(), request_id(50)).unwrap();
    script.push(AgentPoll::Completed(commit_response(7, 50, 3)));
    let settled = owner.poll_commits_authoritative(&mut state);
    assert_eq!(settled.len(), 1);
    assert_eq!(
        settled[0].outcome,
        CommitSettled::Applied {
            companion: companion(),
            revision: 3
        }
    );
    assert_eq!(settled[0].fulfilled, Some(proposal()));
    assert!(owner.reservation(companion()).is_none());
    assert_eq!(owner.mirror(companion()).unwrap().revision, 3);
    assert_eq!(
        state
            .companion_memory_lifecycle(companion())
            .unwrap()
            .summary,
        "new"
    );
    assert_eq!(current(&state).records, old.records);
    assert_eq!(current(&state).queues, old.queues);
    assert_eq!(current(&state).agent_namespace_id, old.agent_namespace_id);
    assert!(owner.poll_commits_authoritative(&mut state).is_empty());
    let target = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(target.len(), 1);
    assert_eq!(target[0].revision, 10);
}

#[test]
fn aggregate_revision_refusal_keeps_the_complete_reservation_old_mirror_and_no_fulfilled_effect() {
    let (_, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = seeded(&script, &clock);
    let mut state = authority(u64::MAX);
    let old = current(&state).clone();
    let mirror = owner.mirror(companion()).unwrap().clone();
    owner.reserve(proposal()).unwrap();
    owner.commit(companion(), request_id(50)).unwrap();
    script.push(AgentPoll::Completed(commit_response(7, 50, 3)));
    let settled = owner.poll_commits_authoritative(&mut state);
    assert_eq!(
        settled[0].outcome,
        CommitSettled::Failed {
            companion: companion(),
            error: ServerError::Internal {
                invariant: "actor save revision space"
            }
        }
    );
    assert_eq!(settled[0].fulfilled, None);
    assert_eq!(owner.reservation(companion()), Some(&proposal()));
    assert_eq!(owner.mirror(companion()), Some(&mirror));
    assert_eq!(current(&state), &old);
    assert!(!owner.is_ready(companion()));
    assert_eq!(
        script.state.retired.lock().unwrap().as_slice(),
        &[request_id(50)]
    );
}

#[test]
fn higher_reconcile_installs_using_current_authoritative_revision_before_ready() {
    let (_, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = seeded(&script, &clock);
    let mut state = authority(9);
    owner.reconcile(companion(), request_id(50)).unwrap();
    script.push(AgentPoll::Completed(AgentResponse::Reconcile(
        ReconcileResponse::Active {
            leased: leased_for(50),
            companion_id: companion(),
            memory_epoch: 7,
            memory: MemoryState::Present {
                revision: NonZeroU64::new(20).unwrap(),
                operation_id: operation(51),
                summary: "remote".into(),
            },
        },
    )));
    let settled = owner.poll_reconciles_authoritative(&mut state);
    assert_eq!(
        settled,
        vec![ReconcileSettled::Ready {
            companion: companion(),
            fulfilled: None
        }]
    );
    assert!(owner.is_ready(companion()));
    assert_eq!(
        state
            .companion_memory_lifecycle(companion())
            .unwrap()
            .memory_revision,
        20
    );
    assert_eq!(owner.mirror(companion()).unwrap().summary, "remote");
}

fn remote(epoch: u64, revision: u64, op: u8, summary: &str) -> AgentResponse {
    AgentResponse::Reconcile(ReconcileResponse::Active {
        leased: leased_for(50),
        companion_id: companion(),
        memory_epoch: epoch,
        memory: MemoryState::Present {
            revision: NonZeroU64::new(revision).unwrap(),
            operation_id: operation(op),
            summary: summary.into(),
        },
    })
}
fn inactive_loaded() -> StoredCompanions {
    let mut aggregate = loaded(9);
    let second = PlayerId::from_bytes(uuid(21));
    let mut body = aggregate.records[0].clone();
    body.id = second;
    aggregate.records.push(body);
    aggregate.lifecycles.push(StoredCompanionLifecycle {
        id: second,
        active: false,
        memory_epoch: 8,
        memory_revision: 0,
        memory_operation_id: PlayerId::default(),
        summary: String::new(),
        tombstone_operation_id: PlayerId::from_bytes(uuid(52)),
    });
    aggregate
}

#[test]
fn commit_namespace_disabled_missing_closed_and_stale_epoch_refusals_preserve_semantic_ownership() {
    for mode in 0..5 {
        let (_, clock) = StepClock::start();
        let script = ScriptAgent::new();
        let mut owner = seeded(&script, &clock);
        let mut aggregate = loaded(9);
        if mode == 0 {
            aggregate.agent_namespace_id = PlayerId::from_bytes(uuid(4))
        }
        if mode == 4 {
            aggregate.lifecycles[0].memory_epoch = 8
        }
        let mut state = authority_from(aggregate);
        if mode == 1 {
            state = AuthorityState::try_new(
                ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
                42,
            )
            .unwrap()
        }
        if mode == 3 {
            state.mark_closed()
        }
        let mut reserved = proposal();
        if mode == 2 {
            reserved.companion = CompanionId::try_from_bytes(uuid(21)).unwrap();
            owner.set_mirror(
                reserved.companion,
                owner.mirror(companion()).unwrap().clone(),
            )
        }
        let identity = reserved.companion;
        let mirror = owner.mirror(identity).unwrap().clone();
        owner.reserve(reserved.clone()).unwrap();
        owner.commit(identity, request_id(50)).unwrap();
        let AgentResponse::Commit(mut response) = commit_response(7, 50, 3) else {
            unreachable!()
        };
        response.companion_id = identity;
        script.push(AgentPoll::Completed(AgentResponse::Commit(response)));
        let settled = owner.poll_commits_authoritative(&mut state);
        assert!(matches!(settled[0].outcome, CommitSettled::Failed { .. }));
        assert!(settled[0].fulfilled.is_none());
        assert_eq!(owner.reservation(identity), Some(&reserved));
        assert_eq!(owner.mirror(identity), Some(&mirror));
        assert!(!owner.is_ready(identity));
    }
}

#[test]
fn exact_equal_reconcile_is_quiet_even_at_max_and_divergent_states_keep_reservation() {
    for (epoch, revision, op, summary, ready) in [
        (7, 2, 31, "old", true),
        (7, 1, 31, "old", false),
        (7, 2, 32, "old", false),
        (7, 2, 31, "different", false),
        (8, 2, 31, "old", false),
        (7, 3, 50, "new", false),
    ] {
        let (_, clock) = StepClock::start();
        let script = ScriptAgent::new();
        let mut owner = seeded(&script, &clock);
        let mut state = authority(u64::MAX);
        let old = current(&state).clone();
        let mirror = owner.mirror(companion()).unwrap().clone();
        owner.reserve(proposal()).unwrap();
        owner.reconcile(companion(), request_id(50)).unwrap();
        script.push(AgentPoll::Completed(remote(epoch, revision, op, summary)));
        let settled = owner.poll_reconciles_authoritative(&mut state);
        assert_eq!(owner.is_ready(companion()), ready);
        assert_eq!(current(&state), &old);
        assert_eq!(owner.mirror(companion()), Some(&mirror));
        assert_eq!(owner.reservation(companion()), Some(&proposal()));
        assert!(
            matches!(
                settled[0],
                ReconcileSettled::Ready {
                    fulfilled: None,
                    ..
                }
            ) == ready
        );
    }
}

#[test]
fn exact_reserved_reconcile_fulfills_once_and_unrelated_higher_revision_retains_reservation() {
    for (revision, op, summary, fulfills) in [(3, 50, "new", true), (20, 51, "unrelated", false)] {
        let (_, clock) = StepClock::start();
        let script = ScriptAgent::new();
        let mut owner = seeded(&script, &clock);
        let mut state = authority(9);
        owner.reserve(proposal()).unwrap();
        owner.reconcile(companion(), request_id(50)).unwrap();
        script.push(AgentPoll::Completed(remote(7, revision, op, summary)));
        let settled = owner.poll_reconciles_authoritative(&mut state);
        assert_eq!(
            settled,
            vec![ReconcileSettled::Ready {
                companion: companion(),
                fulfilled: if fulfills { Some(proposal()) } else { None }
            }]
        );
        assert_eq!(owner.reservation(companion()).is_none(), fulfills);
        assert_eq!(
            state
                .companion_memory_lifecycle(companion())
                .unwrap()
                .memory_revision,
            revision
        );
        assert!(owner.poll_reconciles_authoritative(&mut state).is_empty());
    }
}

#[test]
fn stale_working_mirror_never_overwrites_the_current_durable_base_on_reconcile() {
    let (_, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = seeded(&script, &clock);
    let mut state = authority(9);
    state
        .replace_companion_memory(companion(), 7, 2, 5, operation(53), "already".into())
        .unwrap();
    owner.reconcile(companion(), request_id(50)).unwrap();
    script.push(AgentPoll::Completed(remote(7, 20, 54, "new remote")));
    assert!(matches!(
        owner.poll_reconciles_authoritative(&mut state).as_slice(),
        [ReconcileSettled::Ready { .. }]
    ));
    assert_eq!(
        state
            .companion_memory_lifecycle(companion())
            .unwrap()
            .memory_revision,
        20
    );
    assert_eq!(owner.mirror(companion()).unwrap().revision, 20);
}

#[test]
fn absent_reconcile_requires_zero_durable_memory_and_closed_owner_never_becomes_ready() {
    for mode in 0..3 {
        let (_, clock) = StepClock::start();
        let script = ScriptAgent::new();
        let mut owner = seeded(&script, &clock);
        let mut aggregate = loaded(9);
        if mode != 1 {
            let lc = &mut aggregate.lifecycles[0];
            lc.memory_revision = 0;
            lc.memory_operation_id = PlayerId::default();
            lc.summary.clear()
        };
        let mut state = authority_from(aggregate);
        if mode == 2 {
            state.mark_closed()
        };
        let old = current(&state).clone();
        owner.reconcile(companion(), request_id(50)).unwrap();
        script.push(AgentPoll::Completed(AgentResponse::Reconcile(
            ReconcileResponse::Active {
                leased: leased_for(50),
                companion_id: companion(),
                memory_epoch: 7,
                memory: MemoryState::Absent,
            },
        )));
        let settled = owner.poll_reconciles_authoritative(&mut state);
        assert_eq!(owner.is_ready(companion()), mode == 0);
        assert_eq!(current(&state), &old);
        assert_eq!(
            matches!(settled[0], ReconcileSettled::Ready { .. }),
            mode == 0
        );
    }
}

#[test]
fn inactive_reconcile_reads_exact_durable_tombstone_and_refuses_a_different_operation() {
    for op in [52, 53] {
        let (_, clock) = StepClock::start();
        let script = ScriptAgent::new();
        let mut owner = seeded(&script, &clock);
        let mut state = authority_from(inactive_loaded());
        let second = CompanionId::try_from_bytes(uuid(21)).unwrap();
        owner.set_mirror(
            second,
            MemoryMirror {
                active: false,
                epoch: 8,
                revision: 0,
                operation: None,
                summary: String::new(),
                tombstone: Some(operation(52)),
            },
        );
        owner.reconcile(second, request_id(50)).unwrap();
        script.push(AgentPoll::Completed(AgentResponse::Reconcile(
            ReconcileResponse::Inactive {
                leased: leased_for(50),
                companion_id: second,
                memory_epoch: 8,
                tombstone_operation_id: operation(op),
            },
        )));
        let settled = owner.poll_reconciles_authoritative(&mut state);
        assert_eq!(owner.is_ready(second), op == 52);
        assert_eq!(
            matches!(settled[0], ReconcileSettled::Ready { .. }),
            op == 52
        );
        assert_eq!(
            state.save_stats(),
            mornlea_server::contracts::SaveStats::default()
        );
    }
}

#[test]
fn delete_acknowledgment_requires_the_already_durable_advanced_tombstone() {
    for accepted in [false, true] {
        let (_, clock) = StepClock::start();
        let script = ScriptAgent::new();
        let mut owner = seeded(&script, &clock);
        let mut state = if accepted {
            authority_from(inactive_loaded())
        } else {
            authority(9)
        };
        let second = CompanionId::try_from_bytes(uuid(21)).unwrap();
        owner.set_mirror(
            second,
            MemoryMirror {
                active: true,
                epoch: 7,
                revision: 2,
                operation: Some(operation(31)),
                summary: "old".into(),
                tombstone: None,
            },
        );
        let mirror = owner.mirror(second).unwrap().clone();
        owner.delete(second, request_id(50), operation(52)).unwrap();
        script.push(AgentPoll::Completed(AgentResponse::Delete(
            DeleteResponse {
                leased: leased_for(50),
                companion_id: second,
                memory_epoch: 8,
                tombstone_operation_id: operation(52),
            },
        )));
        let settled = owner.poll_deletes_authoritative(&mut state);
        assert_eq!(
            matches!(settled[0], DeleteSettled::Deleted { .. }),
            accepted
        );
        if accepted {
            assert!(!owner.mirror(second).unwrap().active);
            assert_eq!(owner.mirror(second).unwrap().revision, 0);
            assert_eq!(owner.pending().outstanding, 0)
        } else {
            assert_eq!(owner.mirror(second), Some(&mirror));
            assert_eq!(owner.pending().outstanding, 1)
        }
    }
}

#[test]
fn authoritative_finalizer_refuses_an_authorityless_drain_and_confirms_into_closing_ledger() {
    let (now, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = seeded(&script, &clock);
    let mut state = authority(9);
    owner.reserve(proposal()).unwrap();
    let deadline = Deadline::after(now, Duration::from_secs(5)).unwrap();
    {
        let mut finalizer = owner.authoritative_finalizer();
        finalizer.begin_attempt(deadline).unwrap();
        assert_eq!(
            finalizer.drain(deadline),
            Err(ServerError::InvalidInput {
                field: "memory_authority"
            })
        );
    }
    state.begin_close();
    let first = owner.drain_authoritative(&mut state, deadline).unwrap();
    assert_eq!(first.outstanding, 1);
    echo_memory_request(&script, 0);
    owner.drain_authoritative(&mut state, deadline).unwrap();
    echo_memory_request(&script, 1);
    let mut finalizer = owner.authoritative_finalizer();
    let result = finalizer.drain_authority(&mut state, deadline).unwrap();
    assert_eq!(result.completed, 1);
    assert_eq!(result.outstanding, 0);
    assert_eq!(
        state
            .companion_memory_lifecycle(companion())
            .unwrap()
            .summary,
        "new"
    );
    assert_eq!(state.save_stats().dirty, 1);
}

#[test]
fn one_refused_reconcile_does_not_suppress_a_later_exact_inactive_companion() {
    let (_, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = seeded(&script, &clock);
    let mut aggregate = inactive_loaded();
    aggregate.revision = u64::MAX;
    let mut state = authority_from(aggregate);
    let second = CompanionId::try_from_bytes(uuid(21)).unwrap();
    owner.set_mirror(
        second,
        MemoryMirror {
            active: false,
            epoch: 8,
            revision: 0,
            operation: None,
            summary: String::new(),
            tombstone: Some(operation(52)),
        },
    );
    owner.reconcile(companion(), request_id(50)).unwrap();
    owner.reconcile(second, request_id(51)).unwrap();
    script.push(AgentPoll::Completed(remote(7, 3, 50, "new")));
    script.push(AgentPoll::Completed(AgentResponse::Reconcile(
        ReconcileResponse::Inactive {
            leased: leased_for(51),
            companion_id: second,
            memory_epoch: 8,
            tombstone_operation_id: operation(52),
        },
    )));
    let settled = owner.poll_reconciles_authoritative(&mut state);
    assert_eq!(settled.len(), 2);
    assert!(matches!(settled[0], ReconcileSettled::NotReady { .. }));
    assert!(matches!(settled[1], ReconcileSettled::Ready { .. }));
    assert!(!owner.is_ready(companion()));
    assert!(owner.is_ready(second));
    assert_eq!(
        state
            .companion_memory_lifecycle(companion())
            .unwrap()
            .memory_revision,
        2
    );
}

#[test]
fn final_drain_cannot_count_a_remote_commit_when_authority_cas_refuses_it() {
    let (now, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = seeded(&script, &clock);
    let mut state = authority(u64::MAX);
    owner.reserve(proposal()).unwrap();
    let deadline = Deadline::after(now, Duration::from_secs(5)).unwrap();
    owner.begin_attempt(deadline).unwrap();
    owner.drain_authoritative(&mut state, deadline).unwrap();
    echo_memory_request(&script, 0);
    owner.drain_authoritative(&mut state, deadline).unwrap();
    echo_memory_request(&script, 1);
    let result = owner.drain_authoritative(&mut state, deadline).unwrap();
    assert_eq!(result.completed, 0);
    assert_eq!(result.outstanding, 1);
    assert_eq!(owner.reservation(companion()), Some(&proposal()));
    assert_eq!(owner.mirror(companion()).unwrap().revision, 2);
    assert_eq!(
        state
            .companion_memory_lifecycle(companion())
            .unwrap()
            .memory_revision,
        2
    );
    assert!(matches!(
        script.submitted().last(),
        Some(AgentRequest::Reconcile(_))
    ));
}

#[test]
fn authoritative_terminals_fence_a_different_response_companion_before_any_acceptance() {
    for reconcile in [false, true] {
        let (_, clock) = StepClock::start();
        let script = ScriptAgent::new();
        let mut owner = seeded(&script, &clock);
        let mut state = authority(9);
        let old = current(&state).clone();
        owner.reserve(proposal()).unwrap();
        if reconcile {
            owner.reconcile(companion(), request_id(50)).unwrap();
            let AgentResponse::Reconcile(ReconcileResponse::Active {
                leased,
                memory_epoch,
                memory,
                ..
            }) = remote(7, 3, 50, "new")
            else {
                unreachable!()
            };
            script.push(AgentPoll::Completed(AgentResponse::Reconcile(
                ReconcileResponse::Active {
                    leased,
                    companion_id: CompanionId::try_from_bytes(uuid(21)).unwrap(),
                    memory_epoch,
                    memory,
                },
            )));
            assert!(matches!(
                owner.poll_reconciles_authoritative(&mut state).as_slice(),
                [ReconcileSettled::Fenced { .. }]
            ))
        } else {
            owner.commit(companion(), request_id(50)).unwrap();
            let AgentResponse::Commit(mut response) = commit_response(7, 50, 3) else {
                unreachable!()
            };
            response.companion_id = CompanionId::try_from_bytes(uuid(21)).unwrap();
            script.push(AgentPoll::Completed(AgentResponse::Commit(response)));
            let result = owner.poll_commits_authoritative(&mut state);
            assert!(matches!(result[0].outcome, CommitSettled::Fenced { .. }));
            assert!(result[0].fulfilled.is_none())
        }
        assert_eq!(current(&state), &old);
        assert_eq!(owner.reservation(companion()), Some(&proposal()));
    }
}

#[test]
fn publicly_mutated_reservations_are_refused_before_overflow_or_fulfillment_payload_copy() {
    for mode in 0..5 {
        let (_, clock) = StepClock::start();
        let script = ScriptAgent::new();
        let mut owner = seeded(&script, &clock);
        let mut state = authority(9);
        let mut reserved = proposal();
        match mode {
            0 => reserved.base_revision = u64::MAX,
            1 => reserved.memory_epoch = 0,
            2 => reserved.summary = "a".repeat(2049),
            3 => reserved.line = "a".repeat(257),
            _ => reserved.line = "bad\0line".into(),
        };
        let old = current(&state).clone();
        owner.reserve(reserved.clone()).unwrap();
        owner.commit(companion(), request_id(50)).unwrap();
        script.push(AgentPoll::Completed(commit_response(
            reserved.memory_epoch,
            50,
            3,
        )));
        let settled = owner.poll_commits_authoritative(&mut state);
        assert_eq!(
            settled[0].outcome,
            CommitSettled::Failed {
                companion: companion(),
                error: ServerError::InvalidInput {
                    field: "memory_proposal"
                }
            }
        );
        assert!(settled[0].fulfilled.is_none());
        assert_eq!(owner.reservation(companion()), Some(&reserved));
        assert_eq!(current(&state), &old);
    }
}

#[test]
fn configured_authority_refuses_the_trait_default_remote_only_finalizer() {
    let (now, clock) = StepClock::start();
    let script = ScriptAgent::new();
    let mut owner = seeded(&script, &clock);
    let deadline = Deadline::after(now, Duration::from_secs(5)).unwrap();
    owner.begin_attempt(deadline).unwrap();
    let mut state = authority(9);
    assert_eq!(
        owner.drain_authority(&mut state, deadline),
        Err(ServerError::InvalidInput {
            field: "memory_authority"
        })
    );
    let mut disabled = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        42,
    )
    .unwrap();
    assert_eq!(
        owner
            .drain_authority(&mut disabled, deadline)
            .unwrap()
            .outstanding,
        0
    );
}

#[test]
fn mismatched_delete_epoch_or_tombstone_stays_fenced_before_authoritative_validation() {
    for wrong_epoch in [true, false] {
        let (_, clock) = StepClock::start();
        let script = ScriptAgent::new();
        let mut owner = seeded(&script, &clock);
        let mut state = authority_from(inactive_loaded());
        let second = CompanionId::try_from_bytes(uuid(21)).unwrap();
        let mirror = MemoryMirror {
            active: true,
            epoch: 7,
            revision: 2,
            operation: Some(operation(31)),
            summary: "old".into(),
            tombstone: None,
        };
        owner.set_mirror(second, mirror.clone());
        owner.delete(second, request_id(50), operation(52)).unwrap();
        script.push(AgentPoll::Completed(AgentResponse::Delete(
            DeleteResponse {
                leased: leased_for(50),
                companion_id: second,
                memory_epoch: if wrong_epoch { 9 } else { 8 },
                tombstone_operation_id: operation(if wrong_epoch { 52 } else { 53 }),
            },
        )));
        assert_eq!(
            owner.poll_deletes_authoritative(&mut state),
            vec![DeleteSettled::Fenced { companion: second }]
        );
        assert_eq!(owner.mirror(second), Some(&mirror));
        assert_eq!(owner.pending().outstanding, 1);
    }
}
