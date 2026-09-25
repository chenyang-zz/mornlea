//! Companion v5 length preflight matches `encode_companions` without allocating bytes.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::path::PathBuf;

use mornlea_storage::{
    COMPANION_MAX_FIFO_ENTRIES, COMPANION_MAX_FILE_LENGTH, COMPANION_MAX_PLAN_STEPS,
    COMPANION_MAX_STORED, COMPANION_MAX_SUMMARY_BYTES, COMPANION_MAX_TASK_COMMAND_BYTES,
    COMPANION_PLAN_STEP_FOLLOW, COMPANION_PLAN_STEP_PLACE, COMPANION_TASK_FAIL_NONE,
    COMPANION_TASK_RUNNING, CompanionBody, CompanionSave, Inventory, ItemStack, PlanStep, PlayerId,
    StorageError, StoredCompanionLifecycle, StoredCompanionQueue, StoredCompanionTask,
    companions_encoded_len, decode_companions, encode_companions,
};

struct MeasuredAllocator;

thread_local! {
    static MEASURE_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    static LARGEST_ALLOCATION: Cell<usize> = const { Cell::new(0) };
}

#[global_allocator]
static ALLOCATOR: MeasuredAllocator = MeasuredAllocator;

unsafe impl GlobalAlloc for MeasuredAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = MEASURE_ALLOCATIONS.try_with(|enabled| {
            if enabled.get() {
                let _ = LARGEST_ALLOCATION
                    .try_with(|largest| largest.set(largest.get().max(layout.size())));
            }
        });
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

fn largest_allocation_during(f: impl FnOnce()) -> usize {
    LARGEST_ALLOCATION.with(|largest| largest.set(0));
    MEASURE_ALLOCATIONS.with(|enabled| enabled.set(true));
    f();
    MEASURE_ALLOCATIONS.with(|enabled| enabled.set(false));
    LARGEST_ALLOCATION.with(Cell::get)
}

fn read_go_fixture(relative: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../packages/")
        .join(relative);
    fs::read(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

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

fn assert_preflight_matches_encode(save: &CompanionSave) {
    let encoded = encode_companions(save).expect("encode");
    assert_eq!(
        companions_encoded_len(save).expect("preflight length"),
        encoded.len()
    );
}

fn assert_preflight_corrupt(save: &CompanionSave, needle: &str) {
    match companions_encoded_len(save) {
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
fn preflight_golden_v5_fixture_matches_encode() {
    let golden = read_go_fixture("server/storage/companion/testdata/companions-v5.bin");
    let decoded = decode_companions(&golden).expect("decode golden");
    let save = CompanionSave {
        revision: decoded.revision,
        agent_namespace_id: decoded.agent_namespace_id,
        records: decoded.records,
        lifecycles: decoded.lifecycles,
        queues: decoded.queues,
    };
    assert_preflight_matches_encode(&save);
    assert_eq!(companions_encoded_len(&save).expect("length"), golden.len());
}

#[test]
fn preflight_zero_one_and_four_active_body_shapes_match_encode() {
    assert_preflight_matches_encode(&inactive_pair(0));

    let one = body(1);
    let one_save = save(
        vec![one.clone()],
        vec![lifecycle(one.id, false, 1)],
        Vec::new(),
    );
    assert_preflight_matches_encode(&one_save);

    let records: Vec<_> = (1..=4).map(body).collect();
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
    assert_preflight_matches_encode(&save(records, lifecycles, queues));
}

#[test]
fn preflight_sixty_four_inactive_bodies_match_encode() {
    assert_preflight_matches_encode(&inactive_pair(COMPANION_MAX_STORED));
}

#[test]
fn preflight_rejects_oversized_and_invalid_membership_without_cloning_payloads() {
    let mut oversized = inactive_pair(COMPANION_MAX_STORED + 1);
    oversized.records[0].dimension = 9;
    assert_preflight_corrupt(&oversized, "companion count");

    let records: Vec<_> = (1..=5).map(body).collect();
    let lifecycles = records
        .iter()
        .map(|body| lifecycle(body.id, true, 1))
        .collect();
    assert_preflight_corrupt(&save(records, lifecycles, Vec::new()), "active");

    let one = body(1);
    assert_preflight_corrupt(
        &save(vec![one.clone()], Vec::new(), Vec::new()),
        "lifecycle",
    );
    assert_preflight_corrupt(
        &save(
            vec![one.clone(), one.clone()],
            vec![lifecycle(one.id, true, 1), lifecycle(one.id, true, 1)],
            Vec::new(),
        ),
        "duplicate",
    );
    assert_preflight_corrupt(
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
    assert_preflight_corrupt(
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

    let fifo = save(
        vec![one.clone()],
        vec![lifecycle(one.id, true, 1)],
        vec![StoredCompanionQueue {
            id: one.id,
            pending: vec!["排队".to_owned(); COMPANION_MAX_FIFO_ENTRIES + 1],
            ..StoredCompanionQueue::default()
        }],
    );
    assert_preflight_corrupt(&fifo, "FIFO");

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
    assert_preflight_corrupt(
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

    let oversized_command = save(
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
    );
    let largest_command = largest_allocation_during(|| {
        assert_preflight_corrupt(&oversized_command, "command");
    });
    assert!(
        largest_command <= COMPANION_MAX_TASK_COMMAND_BYTES,
        "preflight allocated {largest_command} bytes for oversized command"
    );

    let mut summary_lifecycle = lifecycle(one.id, true, 1);
    summary_lifecycle.memory_revision = 1;
    summary_lifecycle.memory_operation_id = agent_id(0x81);
    summary_lifecycle.summary = "s".repeat(COMPANION_MAX_SUMMARY_BYTES + 1);
    let oversized_summary = save(vec![one], vec![summary_lifecycle], Vec::new());
    let largest_summary = largest_allocation_during(|| {
        assert_preflight_corrupt(&oversized_summary, "summary");
    });
    assert!(
        largest_summary <= COMPANION_MAX_SUMMARY_BYTES,
        "preflight allocated {largest_summary} bytes for oversized summary"
    );
}

#[test]
fn preflight_maximum_legal_shape_is_exactly_393904_bytes() {
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
    let input = save(records, lifecycles, queues);
    let length = companions_encoded_len(&input).expect("maximum legal preflight");
    assert_eq!(length, COMPANION_MAX_FILE_LENGTH);
    assert_preflight_matches_encode(&input);
}

#[test]
fn preflight_unsorted_valid_save_matches_canonical_encode_without_mutation() {
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
    let length = companions_encoded_len(&input).expect("unsorted preflight");
    let encoded = encode_companions(&input).expect("encode");
    assert_eq!(length, encoded.len());
    assert_eq!(input, before);
}
