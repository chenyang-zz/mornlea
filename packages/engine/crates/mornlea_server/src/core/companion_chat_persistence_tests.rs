//! Saved-task ownership and normalization execute the actual serial chat owner.
use super::*;
use crate::core::contracts::{ActorAux, ActorKey, ActorRuntime};
use mornlea_storage::{
    CompanionBody, CompanionSave, Inventory, PlanStep as SavedStep, PlayerId as SaveId,
    StoredCompanionLifecycle, StoredCompanionQueue, StoredCompanionTask, StoredCompanions,
    decode_companions, encode_companions,
};

fn raw_id(tag: u8) -> [u8; 16] {
    let mut value = [0; 16];
    value[0] = tag;
    value[6] = 0x40;
    value[8] = 0x80;
    value
}
fn id(tag: u8) -> CompanionId {
    CompanionId::try_from_bytes(raw_id(tag)).unwrap()
}
fn save_id(tag: u8) -> SaveId {
    SaveId::from_bytes(raw_id(tag))
}
fn definitions(count: u8) -> Vec<(CompanionId, CompanionName)> {
    (1..=count)
        .map(|tag| {
            (
                id(tag),
                CompanionName::try_from_canonical(format!("Nova{tag}")).unwrap(),
            )
        })
        .collect()
}
fn body(tag: u8) -> CompanionBody {
    CompanionBody {
        id: save_id(tag),
        dimension: 0,
        position: [0.5, 65.0, 0.5],
        yaw: 0.0,
        pitch: 0.0,
        inventory: Inventory::default(),
    }
}
fn lifecycle(tag: u8, active: bool) -> StoredCompanionLifecycle {
    StoredCompanionLifecycle {
        id: save_id(tag),
        active,
        memory_epoch: 7,
        memory_revision: if active { 2 } else { 0 },
        memory_operation_id: if active {
            save_id(180)
        } else {
            SaveId::default()
        },
        summary: if active {
            "untouched memory".into()
        } else {
            String::new()
        },
        tombstone_operation_id: if active {
            SaveId::default()
        } else {
            save_id(181)
        },
    }
}
fn task(state: u8) -> StoredCompanionTask {
    StoredCompanionTask {
        command: " work ".into(),
        state,
        plan_steps: if state == 4 {
            vec![
                SavedStep {
                    kind: 1,
                    x: 1,
                    y: 65,
                    z: 2,
                    ..Default::default()
                },
                SavedStep {
                    kind: 3,
                    x: 2,
                    y: 65,
                    z: 2,
                    ..Default::default()
                },
                SavedStep {
                    kind: 4,
                    x: 3,
                    y: 65,
                    z: 2,
                    block: 2,
                    ..Default::default()
                },
            ]
        } else {
            Vec::new()
        },
        step_index: if state == 4 { 2 } else { 0 },
        start_tick: if state == 4 { 900 } else { 0 },
        deadline_ticks: if state == 4 { 1200 } else { 0 },
        fail_reason: if state == 6 { 1 } else { 0 },
    }
}
fn aggregate(state: u8) -> StoredCompanions {
    StoredCompanions {
        source_schema: 5,
        revision: 9,
        agent_namespace_id: save_id(240),
        records: vec![body(2), body(1)],
        lifecycles: vec![lifecycle(2, false), lifecycle(1, true)],
        queues: vec![StoredCompanionQueue {
            id: save_id(1),
            has_current: true,
            current: task(state),
            pending: vec![" first ".into(), "second".into()],
            summary: String::new(),
        }],
    }
}
fn runtime(generation: u64, task: StoredCompanionTask) -> ActorRuntime {
    ActorRuntime {
        key: ActorKey::Companion(id(1)),
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 300,
        peak_y: 65.0,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Companion {
            generation,
            attempt: 0,
            task,
            mining_target: None,
        },
    }
}
fn runtimes(
    tasks: &BTreeMap<CompanionId, (u64, StoredCompanionTask)>,
) -> BTreeMap<ActorKey, ActorRuntime> {
    tasks
        .iter()
        .map(|(&id, (generation, task))| {
            let mut runtime = runtime(*generation, task.clone());
            runtime.key = ActorKey::Companion(id);
            (runtime.key, runtime)
        })
        .collect()
}
fn save(value: StoredCompanions) -> CompanionSave {
    CompanionSave {
        revision: value.revision,
        agent_namespace_id: value.agent_namespace_id,
        records: value.records,
        lifecycles: value.lifecycles,
        queues: value.queues,
    }
}

#[test]
fn restores_running_exact_progress_fifo_and_synthetic_issuer_without_started() {
    let loaded = aggregate(4);
    let before = loaded.clone();
    let (book, tasks) = CompanionChatBook::from_persisted(&definitions(1), &loaded).unwrap();
    assert_eq!(tasks[&id(1)], (1, loaded.queues[0].current.clone()));
    let queue = book.queue_view(id(1)).unwrap();
    let current = queue.current.unwrap();
    assert_eq!(current.phase, CompanionChatPhase::Running);
    assert_eq!(current.generation, 1);
    assert_eq!(current.source_tick, 0);
    assert!(current.plan.is_none());
    assert_eq!(current.command.as_str(), " work ");
    assert!(current.issuer.session.is_none());
    assert_eq!(current.issuer.player_id.bytes(), raw_id(0));
    assert_eq!(current.issuer.player_name.as_str(), "未知发令者");
    assert_eq!(current.issuer.position.get(), [0.0, 1.0, 0.0]);
    assert!(current.issuer.look_hit.is_none());
    assert_eq!(
        queue
            .pending
            .iter()
            .map(|v| v.0.as_str())
            .collect::<Vec<_>>(),
        [" first ", "second"]
    );
    assert!(
        queue
            .pending
            .iter()
            .all(|v| v.1.session.is_none() && v.2 == 0)
    );
    assert!(book.decided.is_empty());
    assert_eq!(book.external_lifecycle_used(), 0);
    assert_eq!(loaded, before);
}

#[test]
fn queued_planning_validating_normalize_without_losing_source_command() {
    for state in 1..=3 {
        let loaded = aggregate(state);
        let (book, tasks) = CompanionChatBook::from_persisted(&definitions(1), &loaded).unwrap();
        assert_eq!(tasks[&id(1)], (1, task(1)));
        assert_eq!(
            book.queue_view(id(1)).unwrap().current.unwrap().phase,
            CompanionChatPhase::Queued
        );
        let queues = book.snapshot_persisted(&loaded, &BTreeMap::new()).unwrap();
        assert_eq!(queues[0].current, task(1));
        assert_eq!(queues[0].pending, loaded.queues[0].pending);
    }
}

#[test]
fn running_snapshot_round_trips_codec_and_preserves_memory_and_progress() {
    let loaded = aggregate(4);
    let (book, tasks) = CompanionChatBook::from_persisted(&definitions(1), &loaded).unwrap();
    let queues = book.snapshot_persisted(&loaded, &runtimes(&tasks)).unwrap();
    assert_eq!(queues, loaded.queues);
    let mut complete = loaded.clone();
    complete.queues = queues;
    let decoded = decode_companions(&encode_companions(&save(complete)).unwrap()).unwrap();
    assert_eq!(decoded.queues, loaded.queues);
    assert_eq!(
        decoded.lifecycles.iter().find(|v| v.active).unwrap(),
        &loaded.lifecycles[1]
    );
    assert_eq!(decoded.agent_namespace_id, loaded.agent_namespace_id);
}

#[test]
fn terminal_currents_are_omitted_and_fifo_first_generation_stays_fresh() {
    for state in 5..=8 {
        let loaded = aggregate(state);
        let (mut book, tasks) =
            CompanionChatBook::from_persisted(&definitions(1), &loaded).unwrap();
        assert!(tasks.is_empty());
        assert!(book.queue_view(id(1)).unwrap().current.is_none());
        let queue = book
            .snapshot_persisted(&loaded, &BTreeMap::new())
            .unwrap()
            .remove(0);
        assert!(!queue.has_current);
        assert_eq!(queue.current, StoredCompanionTask::default());
        assert_eq!(queue.pending, loaded.queues[0].pending);
        book.promote_heads().unwrap();
        let current = book.queue_view(id(1)).unwrap().current.unwrap();
        assert_eq!(current.generation, 1);
        assert_eq!(current.command.as_str(), " first ");
    }
}

#[test]
fn restored_follow_stop_does_not_leak_into_the_next_command() {
    let mut loaded = aggregate(4);
    let task = &mut loaded.queues[0].current;
    task.plan_steps.push(SavedStep {
        kind: 2,
        player_id: save_id(71),
        ..Default::default()
    });
    task.deadline_ticks = 0;
    let (mut book, _) = CompanionChatBook::from_persisted(&definitions(1), &loaded).unwrap();
    assert!(book.peek_stoppable(id(1)).is_some());
    assert!(book.stop_current(id(1)).is_some());
    assert!(book.peek_stoppable(id(1)).is_none());
    book.promote_heads().unwrap();
    let current = book.queue_view(id(1)).unwrap().current.unwrap();
    assert_eq!(current.generation, 2);
    assert_eq!(current.command.as_str(), " first ");
    assert!(book.peek_stoppable(id(1)).is_none());
}

#[test]
fn restored_finite_running_is_not_stoppable() {
    let (book, _) = CompanionChatBook::from_persisted(&definitions(1), &aggregate(4)).unwrap();
    assert!(book.peek_stoppable(id(1)).is_none());
}

#[test]
fn terminal_finish_clears_current_but_preserves_fifo() {
    let loaded = aggregate(4);
    let (mut book, _) = CompanionChatBook::from_persisted(&definitions(1), &loaded).unwrap();
    assert!(book.commit_finish(id(1), 1, TaskState::Completed).is_some());
    let queue = book
        .snapshot_persisted(&loaded, &BTreeMap::new())
        .unwrap()
        .remove(0);
    assert!(!queue.has_current);
    assert_eq!(queue.pending, loaded.queues[0].pending);
    book.promote_heads().unwrap();
    assert_eq!(
        book.queue_view(id(1)).unwrap().current.unwrap().generation,
        2
    );
}

#[test]
fn planning_snapshot_ignores_stale_runtime_progress() {
    let loaded = aggregate(1);
    let (mut book, tasks) = CompanionChatBook::from_persisted(&definitions(1), &loaded).unwrap();
    assert!(book.take_queued_for_planning(id(1)).is_some());
    let mut running = runtimes(&tasks);
    let runtime = running.values_mut().next().unwrap();
    runtime.key = ActorKey::Companion(id(2));
    let queue = book
        .snapshot_persisted(&loaded, &running)
        .unwrap()
        .remove(0);
    assert_eq!(queue.current, task(1));
}

#[test]
fn four_configured_and_sixty_four_retained_preserve_empty_queues() {
    let mut loaded = aggregate(4);
    loaded.queues.clear();
    loaded.records = (1..=64).rev().map(body).collect();
    loaded.lifecycles = (1..=64).rev().map(|tag| lifecycle(tag, tag <= 4)).collect();
    let (book, tasks) = CompanionChatBook::from_persisted(&definitions(4), &loaded).unwrap();
    assert!(tasks.is_empty());
    assert!(
        book.snapshot_persisted(&loaded, &BTreeMap::new())
            .unwrap()
            .is_empty()
    );
    let mut inactive = loaded.clone();
    inactive.lifecycles = (1..=64).map(|tag| lifecycle(tag, false)).collect();
    let (book, tasks) = CompanionChatBook::from_persisted(&[], &inactive).unwrap();
    assert!(tasks.is_empty());
    assert!(
        book.snapshot_persisted(&inactive, &BTreeMap::new())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn malformed_aggregate_or_configuration_refuses_complete_restore_without_input_mutation() {
    for mode in 0..12 {
        let mut loaded = aggregate(4);
        let mut defs = definitions(1);
        match mode {
            0 => loaded.source_schema = 4,
            1 => loaded.revision = 0,
            2 => loaded.records = (1..=65).map(body).collect(),
            3 => loaded.queues[0].id = save_id(3),
            4 => loaded.queues[0].current.command = "x".repeat(1025),
            5 => loaded.queues[0].current.plan_steps = vec![SavedStep::default(); 5001],
            6 => loaded.lifecycles.clear(),
            7 => loaded.agent_namespace_id = SaveId::default(),
            8 => defs = definitions(5),
            9 => defs.push(defs[0].clone()),
            10 => defs = definitions(2),
            _ => defs.clear(),
        }
        let before = loaded.clone();
        assert!(CompanionChatBook::from_persisted(&defs, &loaded).is_err());
        assert_eq!(loaded, before);
    }
}

#[test]
fn running_snapshot_refuses_wrong_identity_progress_or_generation_without_mutation() {
    let loaded = aggregate(4);
    let (book, tasks) = CompanionChatBook::from_persisted(&definitions(1), &loaded).unwrap();
    let before = book.clone();
    for mode in 0..9 {
        let mut values = runtimes(&tasks);
        if mode == 0 {
            values.clear();
        } else {
            let runtime = values.values_mut().next().unwrap();
            if mode == 1 {
                runtime.key = ActorKey::Companion(id(2));
            }
            if mode == 2 {
                runtime.aux = ActorAux::Player {
                    respawn: None,
                    workbench: None,
                };
            }
            if let ActorAux::Companion {
                generation, task, ..
            } = &mut runtime.aux
            {
                match mode {
                    3 => *generation = 2,
                    4 => task.state = 1,
                    5 => task.command = "other".into(),
                    6 => task.step_index = 3,
                    7 => task.plan_steps = vec![SavedStep::default(); 5001],
                    8 => task.command = "x".repeat(1025),
                    _ => {}
                }
            }
        }
        assert!(book.snapshot_persisted(&loaded, &values).is_err());
        assert_eq!(book, before);
    }
}

#[test]
fn restored_and_snapshot_outputs_have_independent_ownership() {
    let loaded = aggregate(4);
    let before = loaded.clone();
    let (book, mut tasks) = CompanionChatBook::from_persisted(&definitions(1), &loaded).unwrap();
    let mut queues = book.snapshot_persisted(&loaded, &runtimes(&tasks)).unwrap();
    queues[0].pending[0].push_str(" changed");
    tasks.get_mut(&id(1)).unwrap().1.plan_steps.clear();
    assert_eq!(loaded, before);
    assert_eq!(
        book.queue_view(id(1))
            .unwrap()
            .current
            .unwrap()
            .command
            .as_str(),
        " work "
    );
}

#[test]
fn four_restored_tasks_have_independent_fresh_generations_and_sorted_snapshots() {
    let mut loaded = aggregate(4);
    loaded.records = (1..=4).rev().map(body).collect();
    loaded.lifecycles = (1..=4).rev().map(|tag| lifecycle(tag, true)).collect();
    loaded.queues = (1..=4)
        .rev()
        .map(|tag| StoredCompanionQueue {
            id: save_id(tag),
            has_current: true,
            current: task(if tag % 2 == 0 { 2 } else { 4 }),
            pending: vec![format!("next{tag}")],
            summary: String::new(),
        })
        .collect();
    let (book, tasks) = CompanionChatBook::from_persisted(&definitions(4), &loaded).unwrap();
    assert_eq!(tasks.len(), 4);
    assert!(tasks.values().all(|(generation, _)| *generation == 1));
    let queues = book.snapshot_persisted(&loaded, &runtimes(&tasks)).unwrap();
    assert_eq!(
        queues.iter().map(|queue| queue.id).collect::<Vec<_>>(),
        (1..=4).map(save_id).collect::<Vec<_>>()
    );
    for (index, queue) in queues.iter().enumerate() {
        assert_eq!(queue.current, task(if index % 2 == 0 { 4 } else { 1 }));
        assert_eq!(queue.pending, [format!("next{}", index + 1)]);
    }
}

#[test]
fn restored_fifo_accepts_sixteen_in_order_and_refuses_seventeen() {
    let mut loaded = aggregate(1);
    loaded.queues[0].pending = (0..16).map(|value| format!(" {value} ")).collect();
    let (book, _) = CompanionChatBook::from_persisted(&definitions(1), &loaded).unwrap();
    assert_eq!(
        book.snapshot_persisted(&loaded, &BTreeMap::new()).unwrap()[0].pending,
        loaded.queues[0].pending
    );
    loaded.queues[0].pending.push("overflow".into());
    assert!(CompanionChatBook::from_persisted(&definitions(1), &loaded).is_err());
}
