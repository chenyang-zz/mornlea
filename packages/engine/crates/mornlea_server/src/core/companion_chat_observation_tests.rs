//! Raw source comparisons retain task generation and model text outside the save wire format.
use super::*;
use mornlea_storage::{CompanionBody, Inventory, PlayerId as SaveId, StoredCompanionLifecycle};

fn save_id(tag: u8) -> SaveId {
    let mut bytes = [0; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    SaveId::from_bytes(bytes)
}
fn fixture() -> (CompanionChatBook, CompanionId, Vec<StoredCompanionQueue>) {
    let id = CompanionId::try_from_bytes(save_id(1).to_bytes()).unwrap();
    let queue = StoredCompanionQueue {
        id: save_id(1),
        has_current: true,
        current: StoredCompanionTask {
            command: "work".into(),
            state: 4,
            plan_steps: vec![mornlea_storage::PlanStep {
                kind: 1,
                x: 1,
                y: 65,
                z: 2,
                ..Default::default()
            }],
            start_tick: 23,
            deadline_ticks: 1000,
            ..Default::default()
        },
        ..Default::default()
    };
    let aggregate = StoredCompanions {
        source_schema: 5,
        revision: 9,
        agent_namespace_id: save_id(240),
        records: vec![CompanionBody {
            id: save_id(1),
            dimension: 0,
            position: [8.5, 65.0, 8.5],
            yaw: 0.0,
            pitch: 0.0,
            inventory: Inventory::default(),
        }],
        lifecycles: vec![StoredCompanionLifecycle {
            id: save_id(1),
            active: true,
            memory_epoch: 7,
            memory_revision: 2,
            memory_operation_id: save_id(180),
            summary: "memory".into(),
            tombstone_operation_id: SaveId::default(),
        }],
        queues: vec![queue.clone()],
    };
    let (book, _) = CompanionChatBook::from_persisted(
        &[(
            id,
            CompanionName::try_from_canonical("Nova".into()).unwrap(),
        )],
        &aggregate,
    )
    .unwrap();
    (book, id, vec![queue])
}
#[test]
fn model_summary_only_change_compares_different_with_identical_wire_queue() {
    let (mut book, id, queues) = fixture();
    let plan = AgentPlan::try_new("a".into(), vec![PlanStep::GoTo { x: 1, y: 65, z: 2 }]).unwrap();
    book.slots
        .get_mut(&id)
        .unwrap()
        .current
        .as_mut()
        .unwrap()
        .plan = Some(plan);
    let first = book.persistence_observation(&queues).unwrap();
    book.slots
        .get_mut(&id)
        .unwrap()
        .current
        .as_mut()
        .unwrap()
        .plan
        .as_mut()
        .unwrap()
        .summary = "b".into();
    let next = book.persistence_observation(&queues).unwrap();
    assert_ne!(first, next);
    assert_eq!(first[0].queue, next[0].queue);
}
#[test]
fn generation_only_change_compares_different_without_fabricated_saved_generation() {
    let (mut book, id, queues) = fixture();
    let first = book.persistence_observation(&queues).unwrap();
    let slot = book.slots.get_mut(&id).unwrap();
    slot.generation = 2;
    slot.current.as_mut().unwrap().generation = 2;
    let next = book.persistence_observation(&queues).unwrap();
    assert_ne!(first, next);
    assert_eq!(first[0].queue, next[0].queue);
}
#[test]
fn restored_running_has_no_model_summary_and_empty_idle_observation_is_quiet() {
    let (book, _, queues) = fixture();
    let observed = book.persistence_observation(&queues).unwrap();
    assert_eq!(observed[0].summary, "");
    assert_eq!(observed[0].generation, Some(1));
    let empty = CompanionChatBook::new();
    assert!(empty.persistence_observation(&[]).unwrap().is_empty());
}
#[test]
fn raw_summary_plan_and_fifo_sizes_refuse_before_observation_copy() {
    for oversized_plan in [true, false] {
        let (mut book, id, queues) = fixture();
        let plan = AgentPlan {
            summary: if oversized_plan {
                "a".into()
            } else {
                "a".repeat(513)
            },
            steps: vec![
                PlanStep::GoTo { x: 1, y: 65, z: 2 };
                if oversized_plan { 5001 } else { 1 }
            ],
        };
        book.slots
            .get_mut(&id)
            .unwrap()
            .current
            .as_mut()
            .unwrap()
            .plan = Some(plan);
        assert!(book.persistence_observation(&queues).is_err());
    }
    let (book, _, mut queues) = fixture();
    queues[0].pending = vec!["work".into(); 17];
    assert!(book.persistence_observation(&queues).is_err());
    assert!(
        book.persistence_observation(&vec![queues[0].clone(); 5])
            .is_err()
    );
}
