//! Lifecycle replacement exercises the actual complete authoritative aggregate owner.
use super::*;

fn operation() -> OperationId {
    OperationId::try_from_bytes(raw_id(200)).unwrap()
}

#[test]
fn reads_borrow_sorted_complete_metadata_and_disabled_or_unknown_identity_is_absent() {
    let mut state = base();
    assert!(state.companion_memory_lifecycles().is_none());
    state
        .enable_companion_persistence(&definitions(1), loaded(1, 64))
        .unwrap();
    let lifecycles = state.companion_memory_lifecycles().unwrap();
    assert_eq!(lifecycles.len(), 64);
    for (index, lifecycle) in lifecycles.iter().enumerate() {
        assert_eq!(lifecycle.id, save_id(index as u8 + 1));
    }
    assert_eq!(
        state
            .companion_memory_lifecycle(id(1))
            .unwrap()
            .memory_revision,
        2
    );
    assert!(!state.companion_memory_lifecycle(id(64)).unwrap().active);
    assert!(state.companion_memory_lifecycle(id(65)).is_none());
}

#[test]
fn replacement_changes_only_active_memory_and_preserves_complete_body_task_aggregate() {
    let mut state = enabled(with_task(4));
    let old = current(&state).clone();
    let residents = state.residents();
    state
        .replace_companion_memory(id(1), 7, 2, 3, operation(), "new memory".into())
        .unwrap();
    let save = current(&state);
    assert_eq!(save.records, old.records);
    assert_eq!(save.queues, old.queues);
    assert_eq!(save.agent_namespace_id, old.agent_namespace_id);
    assert_eq!(save.lifecycles[1], old.lifecycles[1]);
    let lifecycle = &save.lifecycles[0];
    assert!(lifecycle.active);
    assert_eq!(lifecycle.memory_epoch, 7);
    assert_eq!(lifecycle.memory_revision, 3);
    assert_eq!(lifecycle.memory_operation_id, save_id(200));
    assert_eq!(lifecycle.summary, "new memory");
    assert_eq!(lifecycle.tombstone_operation_id, PlayerId::default());
    assert_eq!(state.residents().runtimes, residents.runtimes);
    assert_eq!(state.residents().inventories, residents.inventories);
    let selected = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].revision, 10);
}

#[test]
fn higher_remote_revision_jump_and_stale_expected_idempotence_follow_source_cas() {
    let mut state = enabled(loaded(1, 2));
    state
        .replace_companion_memory(id(1), 7, 2, 20, operation(), "reconciled".into())
        .unwrap();
    let selected = state.select(SaveMode::All, SaveBudget::default());
    acknowledge(&mut state, selected);
    assert_eq!(state.save_stats(), SaveStats::default());
    state
        .replace_companion_memory(id(1), 7, 0, 20, operation(), "reconciled".into())
        .unwrap();
    assert_eq!(state.save_stats(), SaveStats::default());
    assert_eq!(
        state
            .companion_memory_lifecycle(id(1))
            .unwrap()
            .memory_revision,
        20
    );
}

#[test]
fn invalid_or_conflicting_proposals_refuse_without_any_complete_aggregate_mutation() {
    let cases = [
        (id(1), 0, 2, 3, "new".into()),
        (id(1), 8, 2, 3, "new".into()),
        (id(1), 7, 1, 3, "new".into()),
        (id(1), 7, 2, 0, "new".into()),
        (id(1), 7, 2, 2, "new".into()),
        (id(1), 7, 2, 1, "new".into()),
        (id(2), 7, 0, 1, "new".into()),
        (id(3), 7, 0, 1, "new".into()),
        (id(1), 7, 2, 3, "a\0b".into()),
        (id(1), 7, 2, 3, "a".repeat(2049)),
    ];
    for (id, epoch, expected, next, summary) in cases {
        let mut state = enabled(with_task(4));
        let old = current(&state).clone();
        assert!(
            state
                .replace_companion_memory(id, epoch, expected, next, operation(), summary)
                .is_err()
        );
        assert_eq!(current(&state), &old);
        assert_eq!(state.save_stats(), SaveStats::default());
    }
}

#[test]
fn source_summary_byte_boundary_accepts_empty_padding_and_non_nul_controls() {
    for summary in [
        String::new(),
        " padded ".into(),
        "\n\t".into(),
        "é".repeat(1024),
    ] {
        let mut state = enabled(loaded(1, 2));
        state
            .replace_companion_memory(id(1), 7, 2, 3, operation(), summary.clone())
            .unwrap();
        assert_eq!(
            state.companion_memory_lifecycle(id(1)).unwrap().summary,
            summary
        );
    }
    let mut state = enabled(loaded(1, 2));
    assert!(
        state
            .replace_companion_memory(id(1), 7, 2, 3, operation(), "é".repeat(1025))
            .is_err()
    );
}

#[test]
fn disabled_owner_refuses_replacement_and_healthy_closing_allows_final_memory() {
    assert!(
        base()
            .replace_companion_memory(id(1), 7, 2, 3, operation(), "final".into())
            .is_err()
    );
    let mut state = enabled(loaded(1, 2));
    state.begin_close();
    state
        .replace_companion_memory(id(1), 7, 2, 3, operation(), "final".into())
        .unwrap();
    state
        .run_final(&mut mornlea_server::core::step::AuthoritativeFinalReducer)
        .unwrap();
    assert_eq!(
        state.companion_memory_lifecycle(id(1)).unwrap().summary,
        "final"
    );
    let selected = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].revision, 10);
}

#[test]
fn occupied_max_aggregate_revision_refuses_changed_or_idempotent_memory_and_failed_retry() {
    for mode in 0..3 {
        let mut aggregate = with_task(1);
        aggregate.revision = u64::MAX - 1;
        let mut state = enabled(aggregate);
        state.advance_tick(TickBudget::full()).unwrap();
        let held = state.select(SaveMode::All, SaveBudget::default());
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].revision, u64::MAX);
        if mode == 2 {
            let submitted = held
                .iter()
                .map(|snapshot| (snapshot.key.clone(), snapshot.revision))
                .collect();
            let report = state.apply_completion(SaveCompletion {
                ticket: SaveTicket::try_from_raw(1).unwrap(),
                snapshots: held,
                submitted,
                committed: Vec::new(),
                error: Some(ServerError::Io {
                    operation: Operation::SyncPayload,
                    kind: std::io::ErrorKind::PermissionDenied,
                }),
            });
            assert_eq!(report.retry.len(), 1);
        }
        let old = current(&state).clone();
        let stats = state.save_stats();
        let result = if mode == 1 {
            state.replace_companion_memory(
                id(1),
                7,
                1,
                2,
                OperationId::try_from_bytes(raw_id(180)).unwrap(),
                "untouched memory".into(),
            )
        } else {
            state.replace_companion_memory(id(1), 7, 2, 3, operation(), "new".into())
        };
        assert_eq!(
            result,
            Err(ServerError::Internal {
                invariant: "actor save revision space"
            })
        );
        assert_eq!(current(&state), &old);
        assert_eq!(state.save_stats(), stats);
    }
}

#[test]
fn acknowledged_max_revision_refuses_before_idempotence_or_proposal_validation() {
    let mut aggregate = loaded(1, 2);
    aggregate.revision = u64::MAX;
    let mut state = enabled(aggregate);
    let old = current(&state).clone();
    assert_eq!(
        state.replace_companion_memory(id(1), 0, 2, 0, operation(), "\0".into()),
        Err(ServerError::Internal {
            invariant: "actor save revision space"
        })
    );
    assert_eq!(current(&state), &old);
}

#[test]
fn old_complete_target_stays_immutable_and_exact_ack_leaves_new_memory_dirty() {
    let mut state = enabled(with_task(4));
    state.advance_tick(TickBudget::full()).unwrap();
    let held = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(held.len(), 1);
    let old = held[0].clone();
    state
        .replace_companion_memory(id(1), 7, 2, 3, operation(), "new memory".into())
        .unwrap();
    let SaveValue::Companions(previous) = &old.value else {
        panic!("companions")
    };
    assert_eq!(previous.lifecycles[0].memory_revision, 2);
    assert_eq!(state.save_stats().in_flight, 1);
    acknowledge(&mut state, held);
    assert_eq!(state.save_stats().dirty, 1);
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.companion_memory_lifecycle(id(1)).unwrap().summary,
        "new memory"
    );
    let next = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].revision, 11);
    let SaveValue::Companions(latest) = &next[0].value else {
        panic!("companions")
    };
    assert_eq!(latest.lifecycles[0].memory_revision, 3);
    assert_eq!(latest.records, previous.records);
    assert_eq!(latest.queues, previous.queues);
}

#[test]
fn source_idempotent_current_loaded_memory_does_not_start_a_save_or_clear_existing_dirty() {
    let mut state = enabled(loaded(1, 2));
    let old = current(&state).clone();
    state
        .replace_companion_memory(
            id(1),
            7,
            1,
            2,
            OperationId::try_from_bytes(raw_id(180)).unwrap(),
            "untouched memory".into(),
        )
        .unwrap();
    assert_eq!(state.save_stats(), SaveStats::default());
    assert_eq!(current(&state), &old);
    state
        .replace_companion_memory(id(1), 7, 2, 3, operation(), "new memory".into())
        .unwrap();
    let stats = state.save_stats();
    state
        .replace_companion_memory(id(1), 7, 0, 3, operation(), "new memory".into())
        .unwrap();
    assert_eq!(state.save_stats(), stats);
}

#[test]
fn complete_memory_cas_outcomes_emit_actual_encoded_targets_for_go_pairing() {
    fn encode(save: &mornlea_storage::CompanionSave) -> String {
        mornlea_storage::encode_companions(save)
            .unwrap()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
    fn row(snapshot: &OwnedSnapshot) -> serde_json::Value {
        let SaveValue::Companions(save) = &snapshot.value else {
            panic!("companions")
        };
        serde_json::json!({"revision":snapshot.revision,"encoded":encode(save)})
    }
    let proposals = [
        ("replace", 1, 7, 2, 3, 200, "new memory".to_owned()),
        ("jump", 1, 7, 2, 20, 200, "reconciled".to_owned()),
        ("idempotent", 1, 7, 1, 2, 180, "untouched memory".to_owned()),
        ("epoch", 1, 8, 2, 3, 200, "new".to_owned()),
        ("expected", 1, 7, 1, 3, 200, "new".to_owned()),
        ("inactive", 2, 7, 0, 1, 200, "new".to_owned()),
        ("missing", 3, 7, 0, 1, 200, "new".to_owned()),
        ("zero_epoch", 1, 0, 2, 3, 200, "new".to_owned()),
        ("zero_next", 1, 7, 2, 0, 200, "new".to_owned()),
        ("equal_next", 1, 7, 2, 2, 200, "new".to_owned()),
        ("nul", 1, 7, 2, 3, 200, "a\0b".to_owned()),
        ("oversize", 1, 7, 2, 3, 200, "a".repeat(2049)),
        ("empty", 1, 7, 2, 3, 200, String::new()),
        ("controls", 1, 7, 2, 3, 200, " padded \n\t".to_owned()),
        ("utf8_boundary", 1, 7, 2, 3, 200, "é".repeat(1024)),
    ];
    let mut report = serde_json::Map::new();
    for (name, tag, epoch, expected, next, op, summary) in proposals {
        let mut state = enabled(loaded(1, 2));
        let accepted = state
            .replace_companion_memory(
                id(tag),
                epoch,
                expected,
                next,
                OperationId::try_from_bytes(raw_id(op)).unwrap(),
                summary,
            )
            .is_ok();
        let normalized = encode(current(&state));
        let selected = state.select(SaveMode::All, SaveBudget::default());
        let rows: Vec<_> = selected.iter().map(row).collect();
        acknowledge(&mut state, selected);
        report.insert(
            name.into(),
            serde_json::json!({"accepted":accepted,"normalized":normalized,"rows":rows}),
        );
    }
    let mut state = enabled(loaded(1, 2));
    state
        .replace_companion_memory(id(1), 7, 2, 3, operation(), "first".into())
        .unwrap();
    let held = state.select(SaveMode::All, SaveBudget::default());
    let first = row(&held[0]);
    state
        .replace_companion_memory(
            id(1),
            7,
            3,
            4,
            OperationId::try_from_bytes(raw_id(201)).unwrap(),
            "second".into(),
        )
        .unwrap();
    let normalized = encode(current(&state));
    acknowledge(&mut state, held);
    let next = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(next.len(), 1);
    let second = row(&next[0]);
    acknowledge(&mut state, next);
    report.insert(
        "held".into(),
        serde_json::json!({"accepted":true,"normalized":normalized,"rows":[first,second]}),
    );
    println!(
        "COMPANION_MEMORY_ORACLE {}",
        serde_json::Value::Object(report)
    );
}

mod actual_disk {
    use super::super::actual_disk::{
        RealClock, Root, autosave, deadline, failing_disk, reopen, scheduler, setup,
    };
    use super::*;
    use std::{
        sync::{Arc, Mutex},
        thread,
        time::Instant,
    };
    #[test]
    fn actual_failed_body_task_target_retries_before_latest_memory_final_flush_and_reopen() {
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
            thread::yield_now();
        }
        assert_eq!(
            store.last_error(),
            Some(ServerError::Io {
                operation: Operation::SyncPayload,
                kind: std::io::ErrorKind::PermissionDenied
            })
        );
        state
            .replace_companion_memory(id(1), 7, 2, 3, operation(), "new final memory".into())
            .unwrap();
        state.begin_close();
        state
            .run_final(&mut mornlea_server::core::step::AuthoritativeFinalReducer)
            .unwrap();
        let mut latest = current(&state).clone();
        latest.revision = 12;
        assert_eq!(latest.records, old.records);
        assert_eq!(latest.queues, old.queues);
        assert_eq!(latest.lifecycles[0].summary, "new final memory");
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
    #[test]
    fn real_disk_zero_memory_bootstrap_accepts_higher_remote_revision_and_keeps_tombstone() {
        let root = Root::new();
        let mut aggregate = loaded(1, 2);
        let active = aggregate
            .lifecycles
            .iter_mut()
            .find(|lifecycle| lifecycle.active)
            .unwrap();
        active.memory_revision = 0;
        active.memory_operation_id = PlayerId::default();
        active.summary.clear();
        let (mut state, mut store) = setup(&root, aggregate);
        autosave(&mut state, &mut store);
        let old = current(&state).clone();
        state
            .replace_companion_memory(id(1), 7, 0, 20, operation(), "reconciled zero".into())
            .unwrap();
        state.advance_tick(TickBudget::full()).unwrap();
        autosave(&mut state, &mut store);
        let mut expected = current(&state).clone();
        expected.revision = state.actor_save_current(&SaveKey::Companions).unwrap().0;
        assert_eq!(expected.revision, 11);
        assert_eq!(expected.records, old.records);
        assert_eq!(expected.lifecycles[1], old.lifecycles[1]);
        assert_eq!(expected.lifecycles[0].memory_revision, 20);
        store.close(deadline()).unwrap();
        reopen(&root, expected);
    }
}
