//! Pure migration cases compare actual merge output with independently executed Go encoding.
use mornlea_storage::{
    COMPANION_PLAN_STEP_GO_TO, COMPANION_TASK_RUNNING, CompanionBody, CompanionMergeError,
    CompanionSave, Inventory, PlanStep, PlayerId, StorageError, StoredCompanionLifecycle,
    StoredCompanionQueue, StoredCompanionTask, StoredCompanions, encode_companions,
    merge_companions_v5,
};
use serde_json::Value;

fn id(tag: u8) -> PlayerId {
    let mut bytes = [0; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    PlayerId::from_bytes(bytes)
}
fn body(tag: u8) -> CompanionBody {
    CompanionBody {
        id: id(tag),
        dimension: 0,
        position: [f32::from(tag) + 0.5, 65.0, 8.5],
        yaw: 0.25,
        pitch: 0.125,
        inventory: Inventory::default(),
    }
}
fn lifecycle(tag: u8, active: bool) -> StoredCompanionLifecycle {
    StoredCompanionLifecycle {
        id: id(tag),
        active,
        memory_epoch: 7,
        memory_revision: if active { 2 } else { 0 },
        memory_operation_id: if active {
            id(180 + tag)
        } else {
            PlayerId::default()
        },
        summary: if active {
            "retained mirror".into()
        } else {
            String::new()
        },
        tombstone_operation_id: if active {
            PlayerId::default()
        } else {
            id(180 + tag)
        },
    }
}
fn current() -> StoredCompanions {
    StoredCompanions {
        source_schema: 5,
        revision: 9,
        agent_namespace_id: id(240),
        records: vec![body(2), body(1)],
        lifecycles: vec![lifecycle(2, false), lifecycle(1, true)],
        queues: vec![StoredCompanionQueue {
            id: id(1),
            has_current: true,
            current: StoredCompanionTask {
                command: "walk".into(),
                state: COMPANION_TASK_RUNNING,
                plan_steps: vec![PlanStep {
                    kind: COMPANION_PLAN_STEP_GO_TO,
                    x: 1,
                    y: 65,
                    z: 1,
                    ..Default::default()
                }],
                start_tick: 1,
                deadline_ticks: 20,
                ..Default::default()
            },
            pending: vec!["first".into(), "second".into()],
            summary: String::new(),
        }],
    }
}
#[derive(Default)]
struct Fixture {
    loaded: StoredCompanions,
    active: Vec<CompanionBody>,
    nil_generator: bool,
    fail_at: usize,
    invalid_at: usize,
}
fn fixture(name: &str) -> Fixture {
    let mut f = Fixture::default();
    match name {
        "missing-four" => f.active = vec![body(4), body(2), body(3), body(1)],
        "missing-none" => {}
        "legacy-one" | "legacy-two" | "legacy-three" => {
            f.loaded = StoredCompanions {
                source_schema: 1,
                revision: 9,
                records: vec![body(2), body(1)],
                ..Default::default()
            };
            f.active = vec![body(1)];
            if name != "legacy-one" {
                f.loaded.source_schema = if name == "legacy-two" { 2 } else { 3 };
                f.loaded.queues = vec![StoredCompanionQueue {
                    id: id(1),
                    pending: vec!["first".into(), "second".into()],
                    ..Default::default()
                }];
            }
        }
        "legacy-four-order" | "entropy-failure" | "invalid-entropy" => {
            f.loaded = StoredCompanions {
                source_schema: 4,
                revision: 9,
                records: vec![body(3), body(1), body(2)],
                queues: vec![
                    StoredCompanionQueue {
                        id: id(1),
                        summary: "migration summary".into(),
                        ..Default::default()
                    },
                    StoredCompanionQueue {
                        id: id(2),
                        pending: vec!["first".into(), "second".into()],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };
            f.active = vec![body(2), body(1)];
            if name == "entropy-failure" {
                f.fail_at = 2;
            }
            if name == "invalid-entropy" {
                f.invalid_at = 2;
            }
        }
        "v5-unchanged-max" | "v5-transition" | "nil-generator" | "epoch-overflow" => {
            f.loaded = current();
            match name {
                "v5-unchanged-max" => {
                    f.loaded.revision = u64::MAX;
                    f.active = vec![body(1)];
                    f.nil_generator = true;
                }
                "v5-transition" => {
                    f.active = vec![body(3), body(2)];
                    f.active[1].position = [77.5, 321.0, 99.5];
                }
                "nil-generator" => {
                    f.active = vec![body(2)];
                    f.nil_generator = true;
                }
                _ => f.loaded.lifecycles[1].memory_epoch = u64::MAX,
            }
        }
        "sixty-four" | "sixty-five" => {
            f.loaded = StoredCompanions {
                source_schema: 5,
                revision: 9,
                agent_namespace_id: id(240),
                records: (1..=64).map(body).collect(),
                lifecycles: (1..=64).map(|tag| lifecycle(tag, false)).collect(),
                queues: vec![],
            };
            f.active = if name == "sixty-five" {
                vec![body(65)]
            } else {
                vec![body(64), body(63), body(62), body(61)]
            };
        }
        "five-active" => f.active = (1..=5).map(body).collect(),
        "revision-overflow" => {
            f.loaded = StoredCompanions {
                source_schema: 1,
                revision: u64::MAX,
                records: vec![body(1)],
                ..Default::default()
            };
            f.active = vec![body(1)];
        }
        "future" => f.loaded.source_schema = 6,
        "malformed-missing" => f.loaded.revision = 1,
        "duplicate-active" => f.active = vec![body(1), body(1)],
        _ => panic!("unknown fixture {name}"),
    }
    f
}
type MergeResult = Result<(StoredCompanions, bool), CompanionMergeError<StorageError>>;
fn exercise(f: &Fixture) -> (MergeResult, usize) {
    let before = f.loaded.clone();
    let active = f.active.clone();
    let mut calls = 0;
    let mut generate = || {
        calls += 1;
        if calls == f.fail_at {
            return Err(StorageError::Corrupt("injected entropy".into()));
        }
        Ok(if calls == f.invalid_at {
            PlayerId::default()
        } else {
            id(u8::try_from(200 + calls).unwrap())
        })
    };
    let result = merge_companions_v5(
        &f.loaded,
        &f.active,
        if f.nil_generator {
            None
        } else {
            Some(&mut generate)
        },
    );
    assert_eq!(f.loaded, before);
    assert_eq!(f.active, active);
    (result, calls)
}
fn save(stored: StoredCompanions) -> CompanionSave {
    CompanionSave {
        revision: stored.revision,
        agent_namespace_id: stored.agent_namespace_id,
        records: stored.records,
        lifecycles: stored.lifecycles,
        queues: stored.queues,
    }
}
fn compare(name: &str) {
    let rows: Vec<Value> =
        serde_json::from_str(include_str!("companion_merge/go-merge-v5.json")).unwrap();
    assert_eq!(rows.len(), 19);
    let selected: Vec<_> = rows.iter().filter(|r| r["id"] == name).collect();
    assert_eq!(selected.len(), 1);
    let row = selected[0];
    let (result, calls) = exercise(&fixture(name));
    assert_eq!(calls, row["calls"].as_u64().unwrap() as usize, "{name}");
    match result {
        Ok((stored, changed)) => {
            assert!(row["error"].is_null());
            assert_eq!(changed, row["changed"].as_bool().unwrap());
            assert_eq!(stored.source_schema, 5);
            let encoded = encode_companions(&save(stored)).unwrap();
            let hex: String = encoded.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(hex, row["encoded_hex"].as_str().unwrap(), "{name}");
        }
        Err(error) => {
            let class = match error {
                CompanionMergeError::Identity(error) => {
                    assert_eq!(error, StorageError::Corrupt("injected entropy".into()));
                    "entropy"
                }
                CompanionMergeError::Storage(StorageError::Corrupt(_)) => "corrupt",
                CompanionMergeError::Storage(StorageError::FutureVersion(_)) => "future",
                CompanionMergeError::Storage(StorageError::OutputTooSmall { .. }) => {
                    panic!("merge has no caller buffer")
                }
            };
            assert_eq!(class, row["error"].as_str().unwrap(), "{name}");
        }
    }
}
macro_rules! oracle {
    ($name:ident,$id:literal) => {
        #[test]
        fn $name() {
            compare($id);
        }
    };
}
oracle!(missing_four, "missing-four");
oracle!(missing_none, "missing-none");
oracle!(legacy_one, "legacy-one");
oracle!(legacy_two, "legacy-two");
oracle!(legacy_three, "legacy-three");
oracle!(legacy_four_order, "legacy-four-order");
oracle!(v5_unchanged_max, "v5-unchanged-max");
oracle!(v5_transition, "v5-transition");
oracle!(nil_generator, "nil-generator");
oracle!(sixty_four, "sixty-four");
oracle!(sixty_five, "sixty-five");
oracle!(five_active, "five-active");
oracle!(epoch_overflow, "epoch-overflow");
oracle!(revision_overflow, "revision-overflow");
oracle!(entropy_failure, "entropy-failure");
oracle!(invalid_entropy, "invalid-entropy");
oracle!(future, "future");
oracle!(malformed_missing, "malformed-missing");
oracle!(duplicate_active, "duplicate-active");

#[test]
fn literal_legacy_summary_becomes_independent_memory_revision_in_canonical_order() {
    let (result, calls) = exercise(&fixture("legacy-four-order"));
    let (stored, changed) = result.unwrap();
    assert!(changed);
    assert_eq!(calls, 3);
    assert_eq!(stored.revision, 10);
    assert_eq!(stored.agent_namespace_id, id(201));
    assert_eq!(stored.lifecycles[0].memory_epoch, 1);
    assert_eq!(stored.lifecycles[0].memory_revision, 1);
    assert_eq!(stored.lifecycles[0].memory_operation_id, id(202));
    assert_eq!(stored.lifecycles[0].summary, "migration summary");
    assert_eq!(stored.lifecycles[2].tombstone_operation_id, id(203));
    assert!(!stored.lifecycles[2].active);
    assert_eq!(stored.queues.len(), 1);
    assert_eq!(stored.queues[0].id, id(2));
    assert!(stored.queues[0].summary.is_empty());
    assert_eq!(stored.queues[0].pending, ["first", "second"]);
}
#[test]
fn literal_transitions_preserve_saved_body_and_clear_old_queue_and_mirror() {
    let f = fixture("v5-transition");
    let (result, calls) = exercise(&f);
    let (mut stored, changed) = result.unwrap();
    assert!(changed);
    assert_eq!(calls, 1);
    assert_eq!(stored.revision, 10);
    assert!(stored.queues.is_empty());
    assert_eq!(stored.records[1], body(2));
    assert_eq!(stored.lifecycles[0].memory_epoch, 8);
    assert!(!stored.lifecycles[0].active);
    assert_eq!(stored.lifecycles[0].tombstone_operation_id, id(201));
    assert_eq!(stored.lifecycles[1].memory_epoch, 8);
    assert!(stored.lifecycles[1].active);
    assert_eq!(stored.lifecycles[1].memory_revision, 0);
    assert!(stored.lifecycles[1].summary.is_empty());
    assert_eq!(stored.lifecycles[2].memory_epoch, 1);
    assert!(stored.lifecycles[2].active);
    stored.records[1].position[0] = 99.0;
    assert_eq!(f.loaded.records[0], body(2));
}
#[test]
fn malformed_legacy_and_v5_inputs_refuse_before_entropy_or_unchecked_copy() {
    for mode in 0..7 {
        let mut f = fixture("legacy-one");
        match mode {
            0 => f.loaded.records.push(body(1)),
            1 => f.loaded.agent_namespace_id = id(240),
            2 => f.loaded.lifecycles.push(lifecycle(1, true)),
            3 => {
                f.loaded.source_schema = 3;
                f.loaded.queues.push(StoredCompanionQueue {
                    id: id(1),
                    summary: "too early".into(),
                    ..Default::default()
                });
            }
            4 => {
                f.loaded.source_schema = 4;
                f.loaded.queues.push(StoredCompanionQueue {
                    id: id(3),
                    pending: vec!["orphan".into()],
                    ..Default::default()
                });
            }
            5 => {
                f.loaded = current();
                f.loaded.lifecycles.clear();
            }
            _ => f.active[0].id = PlayerId::default(),
        }
        let (result, calls) = exercise(&f);
        assert!(matches!(
            result,
            Err(CompanionMergeError::Storage(StorageError::Corrupt(_)))
        ));
        assert_eq!(calls, 0);
    }
}

#[test]
fn identity_io_failure_preserves_kind_and_original_non_clone_cause() {
    #[derive(Debug)]
    struct EntropyFault(u64);
    impl std::fmt::Display for EntropyFault {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "entropy fault {}", self.0)
        }
    }
    impl std::error::Error for EntropyFault {}
    let fixture = fixture("legacy-four-order");
    let before = fixture.loaded.clone();
    let mut calls = 0;
    let mut generate = || {
        calls += 1;
        if calls == 2 {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                EntropyFault(37),
            ))
        } else {
            Ok(id(201))
        }
    };
    let error =
        merge_companions_v5(&fixture.loaded, &fixture.active, Some(&mut generate)).unwrap_err();
    assert_eq!(
        std::error::Error::source(&error)
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let mornlea_storage::CompanionMergeError::Identity(error) = error else {
        panic!("entropy failure was classified as save corruption");
    };
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(
        error
            .get_ref()
            .unwrap()
            .downcast_ref::<EntropyFault>()
            .unwrap()
            .0,
        37
    );
    assert_eq!(calls, 2);
    assert_eq!(fixture.loaded, before);
}

#[test]
fn unchanged_value_accepts_explicit_infallible_without_entropy() {
    let fixture = fixture("v5-unchanged-max");
    let (stored, changed) =
        merge_companions_v5::<std::convert::Infallible>(&fixture.loaded, &fixture.active, None)
            .unwrap();
    assert!(!changed);
    assert_eq!(stored.revision, u64::MAX);
}
