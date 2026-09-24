//! Legacy companion queues keep the identity of the body that owned them.

use std::fs;
use std::path::PathBuf;

use mornlea_storage::{
    COMPANION_MAX_FIFO_ENTRIES, COMPANION_PLAN_STEP_FOLLOW, COMPANION_PLAN_STEP_GO_TO,
    COMPANION_PLAN_STEP_MINE, COMPANION_PLAN_STEP_PLACE, COMPANION_SCHEMA_V2, COMPANION_SCHEMA_V3,
    COMPANION_SCHEMA_V4, COMPANION_TASK_FAIL_NONE, COMPANION_TASK_RUNNING, PlanStep, PlayerId,
    StoredCompanionQueue, StoredCompanionTask, decode_companions,
};

fn read_go_fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../../../packages/server/storage/companion/testdata/{name}"
    ));
    fs::read(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn companion_id(last: u8) -> PlayerId {
    PlayerId::from_bytes([
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x46, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        last,
    ])
}

fn follow_player() -> PlayerId {
    PlayerId::from_bytes([
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x46, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0x05,
    ])
}

fn fifo(prefix: &str) -> Vec<String> {
    (1..=COMPANION_MAX_FIFO_ENTRIES)
        .map(|index| format!("{prefix}{index}条"))
        .collect()
}

fn v2_queue() -> StoredCompanionQueue {
    StoredCompanionQueue {
        id: companion_id(1),
        has_current: true,
        current: StoredCompanionTask {
            command: "先去那棵橡树再看一眼".to_owned(),
            plan_steps: vec![
                PlanStep {
                    kind: COMPANION_PLAN_STEP_GO_TO,
                    x: -8,
                    y: 70,
                    z: 6,
                    ..PlanStep::default()
                },
                PlanStep {
                    kind: COMPANION_PLAN_STEP_GO_TO,
                    x: -4,
                    y: 70,
                    z: 9,
                    ..PlanStep::default()
                },
                PlanStep {
                    kind: COMPANION_PLAN_STEP_GO_TO,
                    x: 0,
                    y: 71,
                    z: 12,
                    ..PlanStep::default()
                },
            ],
            step_index: 1,
            state: COMPANION_TASK_RUNNING,
            start_tick: 1200,
            deadline_ticks: 3600,
            fail_reason: COMPANION_TASK_FAIL_NONE,
        },
        pending: fifo("排队指令第"),
        summary: String::new(),
    }
}

fn v3_queue() -> StoredCompanionQueue {
    StoredCompanionQueue {
        id: companion_id(1),
        has_current: true,
        current: StoredCompanionTask {
            command: "去橡树旁挖一格垫一块再跟着我".to_owned(),
            plan_steps: vec![
                PlanStep {
                    kind: COMPANION_PLAN_STEP_GO_TO,
                    x: -8,
                    y: 70,
                    z: 6,
                    ..PlanStep::default()
                },
                PlanStep {
                    kind: COMPANION_PLAN_STEP_MINE,
                    x: -7,
                    y: 69,
                    z: 6,
                    ..PlanStep::default()
                },
                PlanStep {
                    kind: COMPANION_PLAN_STEP_PLACE,
                    x: -6,
                    y: 69,
                    z: 6,
                    block: 18,
                    ..PlanStep::default()
                },
                PlanStep {
                    kind: COMPANION_PLAN_STEP_FOLLOW,
                    player_id: follow_player(),
                    ..PlanStep::default()
                },
            ],
            step_index: 2,
            state: COMPANION_TASK_RUNNING,
            start_tick: 2400,
            deadline_ticks: 0,
            fail_reason: COMPANION_TASK_FAIL_NONE,
        },
        pending: fifo("v3排队第"),
        summary: String::new(),
    }
}

fn v4_queues() -> Vec<StoredCompanionQueue> {
    let mut first = v3_queue();
    first.summary = "阿木记得玩家常在橡树旁停留，上次一起修好了北边的小路。".to_owned();
    let second = StoredCompanionQueue {
        id: companion_id(2),
        pending: vec!["v4仅排队甲".to_owned(), "v4仅排队乙".to_owned()],
        ..StoredCompanionQueue::default()
    };
    vec![first, second]
}

#[test]
fn legacy_nonempty_queues_keep_their_body_owner_and_ordered_content() {
    let cases = [
        (
            "companions-v2.bin",
            COMPANION_SCHEMA_V2,
            41,
            vec![v2_queue()],
        ),
        (
            "companions-v3.bin",
            COMPANION_SCHEMA_V3,
            43,
            vec![v3_queue()],
        ),
        ("companions-v4.bin", COMPANION_SCHEMA_V4, 47, v4_queues()),
    ];
    for (name, schema, revision, want_queues) in cases {
        let golden = read_go_fixture(name);
        let before = golden.clone();
        let decoded = decode_companions(&golden).unwrap_or_else(|err| panic!("{name}: {err}"));
        assert_eq!(golden, before, "{name}: decode rewrote fixture bytes");
        assert_eq!(decoded.source_schema, schema, "{name} schema");
        assert_eq!(decoded.revision, revision, "{name} revision");
        assert_eq!(decoded.records.len(), 2, "{name} bodies");
        assert_eq!(decoded.queues, want_queues, "{name} queues");
        let ids: Vec<_> = decoded.queues.iter().map(|queue| queue.id).collect();
        assert!(
            ids.iter()
                .all(|id| decoded.records.iter().any(|body| body.id == *id)),
            "{name}: a retained queue is not owned by a decoded body"
        );
        if schema == COMPANION_SCHEMA_V4 {
            assert_eq!(ids, vec![companion_id(1), companion_id(2)]);
        } else {
            assert_eq!(ids, vec![companion_id(1)]);
            assert!(
                decoded
                    .records
                    .iter()
                    .any(|body| body.id == companion_id(2)),
                "{name}: the body without work must remain"
            );
        }
    }
}
