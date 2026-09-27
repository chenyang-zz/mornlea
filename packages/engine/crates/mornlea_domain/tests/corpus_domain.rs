//! Closed domain corpus dispatcher integration test.

#[path = "../../../tests/runtime_corpus.rs"]
pub(crate) mod runtime_corpus;

#[path = "corpus_domain/support.rs"]
pub mod support;

#[path = "corpus_domain/identity_text.rs"]
pub mod identity_text;

#[path = "corpus_domain/values.rs"]
pub mod values;

#[path = "corpus_domain/command_control.rs"]
pub mod command_control;

#[path = "corpus_domain/command_inventory.rs"]
pub mod command_inventory;

#[path = "corpus_domain/event_player.rs"]
pub mod event_player;

#[path = "corpus_domain/event_world.rs"]
pub mod event_world;

#[path = "corpus_domain/event_inventory.rs"]
pub mod event_inventory;

#[path = "corpus_domain/event_people.rs"]
pub mod event_people;

#[path = "corpus_domain/event_mobs.rs"]
pub mod event_mobs;

#[path = "corpus_domain/event_objects.rs"]
pub mod event_objects;

#[path = "corpus_domain/event_chat.rs"]
pub mod event_chat;

use runtime_corpus::{CorpusConsumer, FrozenCase, try_load_cases_from_root};
use std::collections::HashSet;
use support::{
    DispatchError, ExecuteCase, ExecutedCase, OwnsCase, assert_domain_normalized, execute_checked,
};

/// Input evidence stays with `external:runtime-authority` because ordering
/// alone cannot produce admitted/discarded world effects. Independent
/// `command_order` tests prove the pure domain ordering contract.
const EXTERNAL_AUTHORITY_CASE_IDS: [&str; 2] = [
    "domain.input/45/session-sequence-arrival",
    "domain.input/45/stale-sequence-no-effect",
];

/// Exact integrated total across the eleven closed topics.
const TOTAL_DOMAIN_CASES: usize = 533;

/// One closed topic adapter: its frozen count, ownership predicate, and
/// executor. Counts are consumed from each topic's own frozen constant so the
/// gate cannot drift from the per-topic contracts those topics pin themselves.
struct TopicAdapter {
    name: &'static str,
    exact_count: usize,
    owns: OwnsCase,
    execute: ExecuteCase,
}

/// The frozen partition. Order does not affect dispatch, but the counts are
/// part of the exact single-ownership contract.
const TOPICS: &[TopicAdapter] = &[
    TopicAdapter {
        name: "identity_text",
        exact_count: identity_text::EXPECTED_COUNT,
        owns: identity_text::owns,
        execute: identity_text::execute,
    },
    TopicAdapter {
        name: "values",
        exact_count: values::EXPECTED_COUNT,
        owns: values::owns,
        execute: values::execute,
    },
    TopicAdapter {
        name: "command_control",
        exact_count: command_control::EXPECTED_COUNT,
        owns: command_control::owns,
        execute: command_control::execute,
    },
    TopicAdapter {
        name: "command_inventory",
        exact_count: command_inventory::EXPECTED_COUNT,
        owns: command_inventory::owns,
        execute: command_inventory::execute,
    },
    TopicAdapter {
        name: "event_player",
        exact_count: event_player::EXPECTED_COUNT,
        owns: event_player::owns,
        execute: event_player::execute,
    },
    TopicAdapter {
        name: "event_world",
        exact_count: event_world::EXPECTED_COUNT,
        owns: event_world::owns,
        execute: event_world::execute,
    },
    TopicAdapter {
        name: "event_inventory",
        exact_count: event_inventory::EXPECTED_COUNT,
        owns: event_inventory::owns,
        execute: event_inventory::execute,
    },
    TopicAdapter {
        name: "event_people",
        exact_count: event_people::EXPECTED_COUNT,
        owns: event_people::owns,
        execute: event_people::execute,
    },
    TopicAdapter {
        name: "event_mobs",
        exact_count: event_mobs::EXPECTED_COUNT,
        owns: event_mobs::owns,
        execute: event_mobs::execute,
    },
    TopicAdapter {
        name: "event_objects",
        exact_count: event_objects::EXPECTED_COUNT,
        owns: event_objects::owns,
        execute: event_objects::execute,
    },
    TopicAdapter {
        name: "event_chat",
        exact_count: event_chat::EXPECTED_COUNT,
        owns: event_chat::owns,
        execute: event_chat::execute,
    },
];

/// Loads the domain corpus once and dispatches every case to its single owner.
///
/// The retained `FrozenCase` inside each `ExecutedCase` stays the comparison
/// source: no topic reloads its case, and no path here re-reads expectation
/// files inside the dispatch loop. Gaps (a case owned by nobody), overlaps
/// (a case owned by several topics), and duplicates (a repeated case id) all
/// fail here, and every topic must execute exactly its frozen count.
fn dispatch_domain_partition() -> Result<Vec<ExecutedCase>, DispatchError> {
    let root = runtime_corpus::find_repo_root();
    let domain_cases =
        try_load_cases_from_root(&root, CorpusConsumer::Domain).map_err(|error| {
            DispatchError::InvalidCase {
                case_id: "dispatch_domain_partition".to_string(),
                message: format!("failed to load domain cases: {error}"),
            }
        })?;
    dispatch_domain_cases(&domain_cases)
}

fn dispatch_domain_cases(cases: &[FrozenCase]) -> Result<Vec<ExecutedCase>, DispatchError> {
    let mut executed = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut counts = vec![0usize; TOPICS.len()];

    for case in cases {
        if !seen_ids.insert(case.id.clone()) {
            return Err(DispatchError::InvalidCase {
                case_id: case.id.clone(),
                message: "duplicate domain case id".to_string(),
            });
        }

        let mut owners = TOPICS
            .iter()
            .enumerate()
            .filter(|(_, topic)| (topic.owns)(case));
        let Some((owner_index, owner)) = owners.next() else {
            return Err(DispatchError::InvalidCase {
                case_id: case.id.clone(),
                message: "case has no owning topic".to_string(),
            });
        };
        if owners.next().is_some() {
            return Err(DispatchError::InvalidCase {
                case_id: case.id.clone(),
                message: "case is owned by multiple topics".to_string(),
            });
        }

        let actual = execute_checked(case, owner.execute)?;
        counts[owner_index] += 1;
        executed.push(ExecutedCase {
            case: case.clone(),
            actual,
        });
    }

    for (topic, count) in TOPICS.iter().zip(counts.iter()) {
        if *count != topic.exact_count {
            return Err(DispatchError::InvalidCase {
                case_id: "dispatch_domain_cases".to_string(),
                message: format!(
                    "topic {} expected exactly {} cases, but executed {count}",
                    topic.name, topic.exact_count
                ),
            });
        }
    }
    if executed.len() != TOTAL_DOMAIN_CASES {
        return Err(DispatchError::InvalidCase {
            case_id: "dispatch_domain_cases".to_string(),
            message: format!(
                "expected exactly {TOTAL_DOMAIN_CASES} dispatched domain cases, but executed {}",
                executed.len()
            ),
        });
    }

    Ok(executed)
}

#[test]
fn corpus_domain_executes_exact_unique_partition() {
    let executed = dispatch_domain_partition().expect("dispatch domain partition");

    let mut unique_executed_ids = HashSet::new();
    for case in &executed {
        assert!(
            unique_executed_ids.insert(case.case.id.as_str()),
            "duplicate executed case id: {}",
            case.case.id
        );
    }
    assert_eq!(
        unique_executed_ids.len(),
        TOTAL_DOMAIN_CASES,
        "the gate must report exactly {TOTAL_DOMAIN_CASES} unique executed ids"
    );

    for case in &executed {
        assert_domain_normalized(case);
    }
}

#[test]
fn corpus_domain_excludes_external_authority_case() {
    let root = runtime_corpus::find_repo_root();

    let domain_cases =
        try_load_cases_from_root(&root, CorpusConsumer::Domain).expect("load domain cases");
    let authority_cases = try_load_cases_from_root(&root, CorpusConsumer::ExternalRuntimeAuthority)
        .expect("load external runtime authority cases");
    for id in EXTERNAL_AUTHORITY_CASE_IDS {
        assert!(
            !domain_cases.iter().any(|case| case.id == id),
            "external authority case {id} must not be loaded as domain work",
        );
        let authority = authority_cases
            .iter()
            .find(|case| case.id == id)
            .unwrap_or_else(|| panic!("external authority case {id} missing from its consumer"));
        for topic in TOPICS {
            assert!(
                !(topic.owns)(authority),
                "external authority case {id} must not dispatch to topic {}",
                topic.name
            );
        }
    }
}

#[test]
fn corpus_domain_comparator_detects_semantic_drift() {
    let executed = dispatch_domain_partition().expect("dispatch domain partition");
    let accepted = executed
        .iter()
        .find(|case| case.actual.get("kind").and_then(|kind| kind.as_str()) == Some("ok"))
        .expect("at least one accepted domain case");

    // The unmutated pair passes comparison before the drift is injected.
    assert_domain_normalized(&ExecutedCase {
        case: accepted.case.clone(),
        actual: accepted.actual.clone(),
    });

    // Mutate exactly one non-wire semantic field of the frozen expectation.
    // The retained FrozenCase is the comparison source, so the mutation models
    // expectation drift rather than a reloaded or regenerated outcome.
    let mut mutated_case = accepted.case.clone();
    let fields = mutated_case
        .normalized
        .get_mut("fields")
        .and_then(|fields| fields.as_object_mut())
        .expect("accepted expectation has a fields object");
    let mutation_value = serde_json::json!({ "corpus_gate_mutation": true });
    let field_name = fields
        .keys()
        .find(|key| key.as_str() != "wire")
        .expect("accepted expectation has a non-wire field")
        .clone();
    let prior_value = fields
        .insert(field_name.clone(), mutation_value.clone())
        .expect("mutated field had a prior value");
    assert_ne!(
        prior_value, mutation_value,
        "the injected drift must actually change the field"
    );

    let mutated = ExecutedCase {
        case: mutated_case,
        actual: accepted.actual.clone(),
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_domain_normalized(&mutated);
    }));
    assert!(
        outcome.is_err(),
        "drift in expectation field '{field_name}' must fail comparison"
    );
}

#[test]
fn corpus_domain_rejects_mutated_input_consumer_at_dispatch() {
    let root = runtime_corpus::find_repo_root();
    let mut domain_cases =
        try_load_cases_from_root(&root, CorpusConsumer::Domain).expect("load domain cases");
    assert_eq!(domain_cases.len(), TOTAL_DOMAIN_CASES);

    let mutated = domain_cases
        .first_mut()
        .expect("the domain partition must contain a case");
    let mutated_id = mutated.id.clone();
    mutated
        .input_json
        .as_mut()
        .and_then(serde_json::Value::as_object_mut)
        .expect("domain case input is an object")
        .insert("consumer".to_string(), serde_json::json!("corpus_frame"));

    match dispatch_domain_cases(&domain_cases) {
        Err(support::DispatchError::InvalidCase { case_id, .. }) => {
            assert_eq!(case_id, mutated_id);
        }
        Err(error) => panic!("unexpected dispatch error: {error}"),
        Ok(_) => panic!("mutated input consumer must fail integrated dispatch"),
    }
}

/// Proves one frozen expectation is semantically load-bearing: it clones the
/// loaded `FrozenCase`, applies exactly one mutation to its frozen expected
/// normalized JSON, keeps the independently executed actual value, and
/// requires the frozen comparator to panic inside `catch_unwind`.
///
/// The unmutated pair passes comparison first — outside `catch_unwind`, so a
/// baseline mismatch fails the test on its own — which makes the later panic
/// attributable to the mutation alone rather than to a hash, path or JSON
/// shape problem.
fn require_expectation_drift_panics(case_id: &str, mutate: impl FnOnce(&mut serde_json::Value)) {
    let executed = dispatch_domain_partition().expect("dispatch domain partition");
    let source = executed
        .iter()
        .find(|case| case.case.id == case_id)
        .unwrap_or_else(|| panic!("missing case {case_id}"));

    // The unmutated expectation passes comparison.
    assert_domain_normalized(source);

    let mut mutated = source.case.clone();
    mutate(&mut mutated.normalized);
    let drifted = ExecutedCase {
        case: mutated,
        actual: source.actual.clone(),
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_domain_normalized(&drifted);
    }));
    assert!(
        outcome.is_err(),
        "mutated expectation for {case_id} must fail comparison"
    );
}

#[test]
fn corpus_domain_rejects_mutated_hostile_kind_semantics() {
    require_expectation_drift_panics("domain.event/1/hostile-spawn-seed", |normalized| {
        normalized["fields"]["spawns"][0]["kind"] = serde_json::json!(1);
    });
}

#[test]
fn corpus_domain_rejects_mutated_hostile_health_semantics() {
    require_expectation_drift_panics("domain.event/1/hostile-spawn-seed", |normalized| {
        normalized["fields"]["spawns"][0]["health"] = serde_json::json!(11);
    });
}

#[test]
fn corpus_domain_rejects_mutated_passive_grazing_semantics() {
    require_expectation_drift_panics("domain.event/1/passive-state-seed", |normalized| {
        normalized["fields"]["states"][0]["grazing"] = serde_json::json!(true);
    });
}

#[test]
fn corpus_domain_rejects_mutated_passive_despawn_reason_semantics() {
    require_expectation_drift_panics("domain.event/1/passive-despawn-seed", |normalized| {
        normalized["fields"]["despawns"][0]["reason"] = serde_json::json!(1);
    });
}

#[test]
fn corpus_domain_rejects_mutated_projectile_dimension_semantics() {
    require_expectation_drift_panics(
        "domain.event/1/projectile-spawn-all-kind-dimension-combinations",
        |normalized| {
            normalized["fields"]["spawns"][0]["dimension"] = serde_json::json!(1);
        },
    );
}

#[test]
fn corpus_domain_rejects_mutated_projectile_velocity_semantics() {
    require_expectation_drift_panics(
        "domain.event/1/projectile-spawn-all-kind-dimension-combinations",
        |normalized| {
            normalized["fields"]["spawns"][0]["velocity"][0] = serde_json::json!("00000000");
        },
    );
}

#[test]
fn corpus_domain_rejects_mutated_item_drop_block_index_semantics() {
    require_expectation_drift_panics("domain.event/1/item-drop-upserts-seed", |normalized| {
        normalized["fields"]["drops"][0]["block_index"] = serde_json::json!(18);
    });
}

#[test]
fn corpus_domain_rejects_mutated_item_drop_order_semantics() {
    require_expectation_drift_panics("domain.event/1/item-drop-upserts-seed", |normalized| {
        let drops = normalized["fields"]["drops"]
            .as_array_mut()
            .expect("seed expectation carries at least two drops");
        drops.swap(0, 1);
    });
}

#[test]
fn corpus_domain_rejects_mutated_chat_branch_semantics() {
    require_expectation_drift_panics("domain.event/1/chat-accepted", |normalized| {
        normalized["category"] = serde_json::json!("speech");
    });
}

#[test]
fn corpus_structure() {
    let root = runtime_corpus::find_repo_root();

    // 1. Verify consumer counts
    let domain_cases =
        try_load_cases_from_root(&root, CorpusConsumer::Domain).expect("load domain cases");
    assert_eq!(
        domain_cases.len(),
        533,
        "expected exactly 533 mornlea_domain cases"
    );

    let authority_cases = try_load_cases_from_root(&root, CorpusConsumer::ExternalRuntimeAuthority)
        .expect("load authority cases");
    assert_eq!(authority_cases.len(), EXTERNAL_AUTHORITY_CASE_IDS.len());
    let authority_ids: HashSet<_> = authority_cases
        .iter()
        .map(|case| case.id.as_str())
        .collect();
    assert_eq!(authority_ids, HashSet::from(EXTERNAL_AUTHORITY_CASE_IDS));

    let agent_cases = try_load_cases_from_root(&root, CorpusConsumer::ExternalAgentContract)
        .expect("load agent contract cases");
    assert_eq!(
        agent_cases.len(),
        154,
        "expected exactly 154 external:agent-contract cases"
    );

    let frame_cases =
        try_load_cases_from_root(&root, CorpusConsumer::Frame).expect("load frame cases");
    assert_eq!(
        frame_cases.len(),
        3,
        "expected exactly 3 corpus_frame cases"
    );

    // 2. Verify all domain cases are json format and operation admit
    for case in &domain_cases {
        assert_eq!(
            case.operation, "admit",
            "case {} operation must be 'admit', got '{}'",
            case.id, case.operation
        );
        assert_eq!(
            case.input_format,
            runtime_corpus::InputFormat::Json,
            "case {} input_format must be json",
            case.id
        );
    }

    // Authority observations remain outside every pure domain topic.
    for authority_case in &authority_cases {
        for topic in TOPICS {
            assert!(
                !(topic.owns)(authority_case),
                "external case claimed by {}",
                topic.name
            );
        }
    }

    // 4. Exact single ownership of every domain case
    let mut seen_ids = HashSet::new();
    let mut identity_text_count = 0;
    let mut values_count = 0;
    let mut command_control_count = 0;
    let mut command_inventory_count = 0;
    let mut event_player_count = 0;
    let mut event_world_count = 0;
    let mut event_inventory_count = 0;
    let mut event_people_count = 0;
    let mut event_mobs_count = 0;
    let mut event_objects_count = 0;
    let mut event_chat_count = 0;

    for case in &domain_cases {
        assert!(
            seen_ids.insert(case.id.clone()),
            "duplicate domain case id: {}",
            case.id
        );

        let mut matches = 0;
        if identity_text::owns(case) {
            matches += 1;
            identity_text_count += 1;
        }
        if values::owns(case) {
            matches += 1;
            values_count += 1;
        }
        if command_control::owns(case) {
            matches += 1;
            command_control_count += 1;
        }
        if command_inventory::owns(case) {
            matches += 1;
            command_inventory_count += 1;
        }
        if event_player::owns(case) {
            matches += 1;
            event_player_count += 1;
        }
        if event_world::owns(case) {
            matches += 1;
            event_world_count += 1;
        }
        if event_inventory::owns(case) {
            matches += 1;
            event_inventory_count += 1;
        }
        if event_people::owns(case) {
            matches += 1;
            event_people_count += 1;
        }
        if event_mobs::owns(case) {
            matches += 1;
            event_mobs_count += 1;
        }
        if event_objects::owns(case) {
            matches += 1;
            event_objects_count += 1;
        }
        if event_chat::owns(case) {
            matches += 1;
            event_chat_count += 1;
        }

        assert_eq!(
            matches, 1,
            "case {} owned by {} modules (expected exactly 1)",
            case.id, matches
        );
    }

    assert_eq!(identity_text_count, 31, "identity_text count mismatch");
    assert_eq!(values_count, 99, "values count mismatch");
    assert_eq!(command_control_count, 28, "command_control count mismatch");
    assert_eq!(
        command_inventory_count, 54,
        "command_inventory count mismatch"
    );
    assert_eq!(event_player_count, 47, "event_player count mismatch");
    assert_eq!(event_world_count, 38, "event_world count mismatch");
    assert_eq!(event_inventory_count, 33, "event_inventory count mismatch");
    assert_eq!(event_people_count, 46, "event_people count mismatch");
    assert_eq!(event_mobs_count, 68, "event_mobs count mismatch");
    assert_eq!(event_objects_count, 45, "event_objects count mismatch");
    assert_eq!(event_chat_count, 44, "event_chat count mismatch");
}
