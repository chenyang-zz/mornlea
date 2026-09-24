//! Constructed v5 companion saves reject unbounded aggregates before cloning.

use mornlea_storage::{
    COMPANION_MAX_FIFO_ENTRIES, COMPANION_MAX_FILE_LENGTH, COMPANION_MAX_PLAN_STEPS,
    COMPANION_MAX_STORED, COMPANION_MAX_SUMMARY_BYTES, COMPANION_MAX_TASK_COMMAND_BYTES,
    COMPANION_PLAN_STEP_FOLLOW, COMPANION_PLAN_STEP_PLACE, COMPANION_TASK_FAIL_NONE,
    COMPANION_TASK_RUNNING, CompanionBody, CompanionSave, Inventory, ItemStack, PlanStep, PlayerId,
    StorageError, StoredCompanionLifecycle, StoredCompanionQueue, StoredCompanionTask,
    decode_companions, encode_companions,
};

fn companion_id(last: u8) -> PlayerId {
    PlayerId::from_bytes([
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x46, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        last,
    ])
}

fn agent_id(last: u8) -> PlayerId {
    PlayerId::from_bytes([
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x46, 0x17, 0x88, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        last,
    ])
}

fn body(last: u8) -> CompanionBody {
    let mut body = CompanionBody {
        id: companion_id(last),
        dimension: 0,
        position: [-12.5, 70.0, 3.25],
        yaw: 1.25,
        pitch: -0.5,
        inventory: Inventory::default(),
    };
    body.inventory.hotbar.selected = 4;
    body.inventory.hotbar.slots[0] = ItemStack {
        item: 1,
        count: 64,
        durability: 0,
    };
    body.inventory.hotbar.slots[4] = ItemStack {
        item: 10,
        count: 1,
        durability: 131,
    };
    body.inventory.backpack[0] = ItemStack {
        item: 20,
        count: 7,
        durability: 0,
    };
    body
}

fn lifecycle(id: PlayerId, active: bool, epoch: u64) -> StoredCompanionLifecycle {
    let mut lifecycle = StoredCompanionLifecycle {
        id,
        active,
        memory_epoch: epoch,
        memory_revision: 0,
        memory_operation_id: PlayerId::default(),
        summary: String::new(),
        tombstone_operation_id: PlayerId::default(),
    };
    if !active {
        lifecycle.tombstone_operation_id = agent_id(id.to_bytes()[15].wrapping_add(0x40));
    }
    lifecycle
}

fn save(
    records: Vec<CompanionBody>,
    lifecycles: Vec<StoredCompanionLifecycle>,
    queues: Vec<StoredCompanionQueue>,
) -> CompanionSave {
    CompanionSave {
        revision: 1,
        agent_namespace_id: agent_id(0x70),
        records,
        lifecycles,
        queues,
    }
}

fn inactive_pair(count: usize) -> CompanionSave {
    let records: Vec<_> = (1..=count).map(|index| body(index as u8)).collect();
    let lifecycles = records
        .iter()
        .enumerate()
        .map(|(index, body)| lifecycle(body.id, false, (index as u64) + 1))
        .collect();
    save(records, lifecycles, Vec::new())
}

fn assert_corrupt(save: &CompanionSave, needle: &str) {
    match encode_companions(save) {
        Err(StorageError::Corrupt(detail)) => {
            assert!(
                detail.contains(needle),
                "corrupt detail {detail:?} does not contain {needle:?}"
            );
        }
        other => panic!("expected corrupt containing {needle:?}, got {other:?}"),
    }
}

#[test]
fn sixty_five_bodies_report_count_before_an_invalid_body() {
    let mut save = inactive_pair(COMPANION_MAX_STORED + 1);
    save.records[0].dimension = 9;
    assert_corrupt(&save, "companion count");
}

#[test]
fn sixty_four_inactive_bodies_encode() {
    let save = inactive_pair(COMPANION_MAX_STORED);
    let encoded = encode_companions(&save).expect("64 inactive bodies");
    let decoded = decode_companions(&encoded).expect("decode 64 bodies");
    assert_eq!(decoded.records.len(), COMPANION_MAX_STORED);
    assert!(decoded.queues.is_empty());
}

#[test]
fn four_active_queues_encode_and_a_fifth_active_rejects() {
    let records: Vec<_> = (1..=4).map(|index| body(index)).collect();
    let lifecycles = records
        .iter()
        .map(|body| lifecycle(body.id, true, 1))
        .collect();
    let queues = records
        .iter()
        .map(|body| StoredCompanionQueue {
            id: body.id,
            pending: vec!["跟我来".to_owned()],
            ..StoredCompanionQueue::default()
        })
        .collect();
    encode_companions(&save(records, lifecycles, queues)).expect("four active queues");

    let records: Vec<_> = (1..=5).map(|index| body(index)).collect();
    let lifecycles = records
        .iter()
        .map(|body| lifecycle(body.id, true, 1))
        .collect();
    assert_corrupt(&save(records, lifecycles, Vec::new()), "active");
}

#[test]
fn membership_and_text_bounds_reject_without_bytes() {
    let one = body(1);
    let two = body(2);
    assert_corrupt(
        &save(vec![one.clone()], Vec::new(), Vec::new()),
        "lifecycle",
    );
    assert_corrupt(
        &save(
            vec![one.clone()],
            vec![
                lifecycle(one.id, true, 1),
                lifecycle(companion_id(3), false, 1),
            ],
            Vec::new(),
        ),
        "lifecycle",
    );
    assert_corrupt(
        &save(
            vec![one.clone(), one.clone()],
            vec![lifecycle(one.id, true, 1), lifecycle(one.id, true, 1)],
            Vec::new(),
        ),
        "duplicate",
    );
    assert_corrupt(
        &save(
            vec![one.clone(), two.clone()],
            vec![lifecycle(one.id, true, 1), lifecycle(one.id, true, 2)],
            Vec::new(),
        ),
        "duplicate",
    );
    assert_corrupt(
        &save(
            vec![one.clone()],
            vec![lifecycle(one.id, true, 1)],
            vec![StoredCompanionQueue {
                id: companion_id(9),
                pending: vec!["跟我来".to_owned()],
                ..StoredCompanionQueue::default()
            }],
        ),
        "queue",
    );
    assert_corrupt(
        &save(
            vec![one.clone()],
            vec![lifecycle(one.id, false, 1)],
            vec![StoredCompanionQueue {
                id: one.id,
                pending: vec!["跟我来".to_owned()],
                ..StoredCompanionQueue::default()
            }],
        ),
        "inactive",
    );
    assert_corrupt(
        &save(
            vec![one.clone()],
            vec![lifecycle(one.id, true, 1)],
            vec![
                StoredCompanionQueue {
                    id: one.id,
                    pending: vec!["甲".to_owned()],
                    ..StoredCompanionQueue::default()
                },
                StoredCompanionQueue {
                    id: one.id,
                    pending: vec!["乙".to_owned()],
                    ..StoredCompanionQueue::default()
                },
            ],
        ),
        "duplicate",
    );

    let fifo = save(
        vec![one.clone()],
        vec![lifecycle(one.id, true, 1)],
        vec![StoredCompanionQueue {
            id: one.id,
            pending: vec!["排队".to_owned(); COMPANION_MAX_FIFO_ENTRIES + 1],
            ..StoredCompanionQueue::default()
        }],
    );
    assert_corrupt(&fifo, "FIFO");

    let mut steps = vec![
        PlanStep {
            kind: COMPANION_PLAN_STEP_PLACE,
            x: 0,
            y: 64,
            z: 0,
            block: 18,
            ..PlanStep::default()
        };
        COMPANION_MAX_PLAN_STEPS
    ];
    steps.push(PlanStep {
        kind: COMPANION_PLAN_STEP_FOLLOW,
        player_id: agent_id(0x05),
        ..PlanStep::default()
    });
    assert_corrupt(
        &save(
            vec![one.clone()],
            vec![lifecycle(one.id, true, 1)],
            vec![StoredCompanionQueue {
                id: one.id,
                has_current: true,
                current: StoredCompanionTask {
                    command: "走".to_owned(),
                    plan_steps: steps,
                    step_index: 0,
                    state: COMPANION_TASK_RUNNING,
                    start_tick: 1,
                    fail_reason: COMPANION_TASK_FAIL_NONE,
                    ..StoredCompanionTask::default()
                },
                ..StoredCompanionQueue::default()
            }],
        ),
        "plan steps",
    );
    assert_corrupt(
        &save(
            vec![one.clone()],
            vec![lifecycle(one.id, true, 1)],
            vec![StoredCompanionQueue {
                id: one.id,
                has_current: true,
                current: StoredCompanionTask {
                    command: "c".repeat(COMPANION_MAX_TASK_COMMAND_BYTES + 1),
                    state: COMPANION_TASK_RUNNING,
                    plan_steps: vec![PlanStep {
                        kind: COMPANION_PLAN_STEP_PLACE,
                        y: 64,
                        block: 18,
                        ..PlanStep::default()
                    }],
                    step_index: 0,
                    start_tick: 1,
                    ..StoredCompanionTask::default()
                },
                ..StoredCompanionQueue::default()
            }],
        ),
        "command",
    );
    let mut summary = lifecycle(one.id, true, 1);
    summary.memory_revision = 1;
    summary.memory_operation_id = agent_id(0x81);
    summary.summary = "s".repeat(COMPANION_MAX_SUMMARY_BYTES + 1);
    assert_corrupt(&save(vec![one], vec![summary], Vec::new()), "summary");
}

#[test]
fn unsorted_ids_encode_canonically_without_mutating_input() {
    let first = body(2);
    let second = body(1);
    let input = save(
        vec![first.clone(), second.clone()],
        vec![
            lifecycle(first.id, false, 2),
            lifecycle(second.id, false, 1),
        ],
        Vec::new(),
    );
    let before = input.clone();
    let encoded = encode_companions(&input).expect("unsorted valid ids");
    assert_eq!(input, before);
    let decoded = decode_companions(&encoded).expect("decode canonical order");
    assert_eq!(decoded.records[0].id, companion_id(1));
    assert_eq!(decoded.records[1].id, companion_id(2));
}

#[test]
fn maximum_legal_aggregate_is_exactly_393904_bytes() {
    let records: Vec<_> = (1..=COMPANION_MAX_STORED)
        .map(|index| body(index as u8))
        .collect();
    let mut lifecycles = Vec::with_capacity(records.len());
    let mut queues = Vec::with_capacity(4);
    for (index, body) in records.iter().enumerate() {
        let active = index < 4;
        let mut lifecycle = lifecycle(body.id, active, (index as u64) + 1);
        if active {
            lifecycle.memory_revision = (index as u64) + 1;
            lifecycle.memory_operation_id = agent_id(0x80 + index as u8);
            lifecycle.summary = "s".repeat(COMPANION_MAX_SUMMARY_BYTES);
            let mut steps = Vec::with_capacity(COMPANION_MAX_PLAN_STEPS);
            for step in 0..COMPANION_MAX_PLAN_STEPS - 1 {
                steps.push(PlanStep {
                    kind: COMPANION_PLAN_STEP_PLACE,
                    x: step as i32,
                    y: 64,
                    z: -(step as i32),
                    block: 18,
                    ..PlanStep::default()
                });
            }
            steps.push(PlanStep {
                kind: COMPANION_PLAN_STEP_FOLLOW,
                player_id: agent_id(0x05),
                ..PlanStep::default()
            });
            queues.push(StoredCompanionQueue {
                id: body.id,
                has_current: true,
                current: StoredCompanionTask {
                    command: "c".repeat(COMPANION_MAX_TASK_COMMAND_BYTES),
                    plan_steps: steps,
                    step_index: (COMPANION_MAX_PLAN_STEPS - 1) as i32,
                    state: COMPANION_TASK_RUNNING,
                    start_tick: 1,
                    ..StoredCompanionTask::default()
                },
                pending: vec![
                    "p".repeat(COMPANION_MAX_TASK_COMMAND_BYTES);
                    COMPANION_MAX_FIFO_ENTRIES
                ],
                summary: String::new(),
            });
        }
        lifecycles.push(lifecycle);
    }
    let encoded =
        encode_companions(&save(records, lifecycles, queues)).expect("maximum legal save");
    assert_eq!(encoded.len(), COMPANION_MAX_FILE_LENGTH);
    decode_companions(&encoded).expect("maximum legal save decodes");
}
