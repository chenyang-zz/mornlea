//! Complete companion startup and prepared resident changes exercise the actual authority owner.
#[path = "source_companion_memory.rs"]
mod memory;
#[path = "source_companion_memory_agent.rs"]
mod memory_agent;
use mornlea_domain::{CompanionId, CompanionName};
use mornlea_server::{contracts::*, state::AuthorityState};
use mornlea_storage::{
    CompanionBody, Inventory, PlayerId, StoredCompanionLifecycle, StoredCompanions,
};

fn raw_id(tag: u8) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}
fn id(tag: u8) -> CompanionId {
    CompanionId::try_from_bytes(raw_id(tag)).unwrap()
}
fn save_id(tag: u8) -> PlayerId {
    PlayerId::from_bytes(raw_id(tag))
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
fn loaded(active: u8, retained: u8) -> StoredCompanions {
    StoredCompanions {
        source_schema: 5,
        revision: 9,
        agent_namespace_id: save_id(240),
        records: (1..=retained)
            .rev()
            .map(|tag| CompanionBody {
                id: save_id(tag),
                dimension: 0,
                position: [8.5, 65.0, 8.5],
                yaw: 0.0,
                pitch: 0.0,
                inventory: Inventory::default(),
            })
            .collect(),
        lifecycles: (1..=retained)
            .rev()
            .map(|tag| StoredCompanionLifecycle {
                id: save_id(tag),
                active: tag <= active,
                memory_epoch: 7,
                memory_revision: if tag <= active { 2 } else { 0 },
                memory_operation_id: if tag <= active {
                    save_id(180)
                } else {
                    PlayerId::default()
                },
                summary: if tag <= active {
                    "untouched memory".into()
                } else {
                    String::new()
                },
                tombstone_operation_id: if tag <= active {
                    PlayerId::default()
                } else {
                    save_id(181)
                },
            })
            .collect(),
        queues: Vec::new(),
    }
}
fn base() -> AuthorityState {
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
}
fn current(state: &AuthorityState) -> &mornlea_storage::CompanionSave {
    let SaveValue::Companions(save) = state.actor_save_current(&SaveKey::Companions).unwrap().1
    else {
        panic!("companions")
    };
    save
}

#[test]
fn complete_startup_installs_only_configured_pending_bodies_and_keeps_all_metadata() {
    let mut state = base();
    let original = loaded(4, 64);
    state
        .enable_companion_persistence(&definitions(4), original.clone())
        .unwrap();
    assert!(state.companion_persistence_enabled());
    let residents = state.residents();
    assert_eq!(residents.actors.len(), 4);
    assert_eq!(residents.runtimes.len(), 4);
    assert_eq!(residents.inventories.len(), 4);
    for (index, actor) in residents.actors.iter().enumerate() {
        assert_eq!(actor.key, ActorKey::Companion(id(index as u8 + 1)));
        assert_eq!(actor.lifecycle, ActorLifecycle::Pending);
        let runtime = &residents.runtimes[&actor.key];
        assert_eq!(runtime.path, None);
        let ActorAux::Companion {
            generation,
            attempt,
            task,
            mining_target,
        } = &runtime.aux
        else {
            panic!("companion")
        };
        assert_eq!((*generation, *attempt), (0, 0));
        assert_eq!(task, &mornlea_storage::StoredCompanionTask::default());
        assert!(mining_target.is_none());
    }
    let save = current(&state);
    assert_eq!(save.records.len(), 64);
    assert_eq!(save.agent_namespace_id, original.agent_namespace_id);
    for body in &original.records {
        assert!(save.records.contains(body));
    }
    for lifecycle in &original.lifecycles {
        assert!(save.lifecycles.contains(lifecycle));
    }
    assert_eq!(state.actor_save_current(&SaveKey::Companions).unwrap().0, 9);
    assert_eq!(state.save_stats(), SaveStats::default());
}

#[test]
fn invalid_last_record_refuses_all_startup_owners_and_ledger() {
    let mut state = base();
    let mut aggregate = loaded(4, 64);
    aggregate.records.last_mut().unwrap().dimension = 1;
    assert!(
        state
            .enable_companion_persistence(&definitions(4), aggregate)
            .is_err()
    );
    assert!(!state.companion_persistence_enabled());
    assert!(state.residents().actors.is_empty());
    assert!(state.actor_save_current(&SaveKey::Companions).is_none());
    assert!(state.source_companion_pending_keys().is_empty());
    state
        .enable_companion_persistence(&definitions(4), loaded(4, 64))
        .unwrap();
}

#[test]
fn durable_nonempty_configuration_is_required_before_whole_owner_landing() {
    for (defs, aggregate) in [
        (definitions(0), loaded(0, 1)),
        (definitions(2), loaded(1, 2)),
        (definitions(5), loaded(5, 5)),
    ] {
        let mut state = base();
        assert!(
            state
                .enable_companion_persistence(&defs, aggregate)
                .is_err()
        );
        assert!(state.residents().actors.is_empty());
        assert!(state.actor_save_current(&SaveKey::Companions).is_none());
    }
    let mut aggregate = loaded(1, 1);
    aggregate.revision = 0;
    assert!(
        base()
            .enable_companion_persistence(&definitions(1), aggregate)
            .is_err()
    );
}

#[test]
fn startup_is_one_shot_and_refuses_closing_or_existing_source_registration() {
    let mut state = base();
    state
        .enable_companion_persistence(&definitions(1), loaded(1, 2))
        .unwrap();
    assert!(
        state
            .enable_companion_persistence(&definitions(1), loaded(1, 2))
            .is_err()
    );
    assert!(
        state
            .register_source_companion(id(2), mornlea_domain::ChunkPos::new(0, 0), None)
            .is_err()
    );
    assert_eq!(state.residents().actors.len(), 1);
    let mut state = base();
    state.begin_close();
    assert!(
        state
            .enable_companion_persistence(&definitions(1), loaded(1, 1))
            .is_err()
    );
    let mut state = base();
    state
        .register_source_companion(id(1), mornlea_domain::ChunkPos::new(0, 0), None)
        .unwrap();
    assert!(
        state
            .enable_companion_persistence(&definitions(1), loaded(1, 1))
            .is_err()
    );
    assert_eq!(state.residents().actors.len(), 1);
}

fn with_task(phase: u8) -> StoredCompanions {
    let mut aggregate = loaded(1, 2);
    aggregate
        .queues
        .push(mornlea_storage::StoredCompanionQueue {
            id: save_id(1),
            has_current: true,
            current: mornlea_storage::StoredCompanionTask {
                command: "work".into(),
                state: phase,
                plan_steps: if phase == 4 {
                    vec![mornlea_storage::PlanStep {
                        kind: 1,
                        x: 12,
                        y: 65,
                        z: 8,
                        ..Default::default()
                    }]
                } else {
                    Vec::new()
                },
                start_tick: if phase == 4 { 23 } else { 0 },
                deadline_ticks: if phase == 4 { 1000 } else { 0 },
                ..Default::default()
            },
            pending: vec![" next ".into()],
            summary: String::new(),
        });
    aggregate
}
fn enabled(aggregate: StoredCompanions) -> AuthorityState {
    let mut state = base();
    state
        .enable_companion_persistence(&definitions(1), aggregate)
        .unwrap();
    state
}
fn acknowledge(state: &mut AuthorityState, snapshots: Vec<OwnedSnapshot>) {
    let submitted = snapshots
        .iter()
        .map(|snapshot| (snapshot.key.clone(), snapshot.revision))
        .collect::<Vec<_>>();
    let report = state.apply_completion(SaveCompletion {
        ticket: SaveTicket::try_from_raw(1).unwrap(),
        snapshots,
        committed: submitted.clone(),
        submitted,
        error: None,
    });
    assert!(report.errors.is_empty());
}
// Prepared liveness allows the actual planning take while keeping physics out of raw-state controls.
fn take_planning_without_body_change(state: &mut AuthorityState) {
    let mut residents = state.residents();
    residents.actors[0].lifecycle = ActorLifecycle::Active;
    state.commit_residents(residents);
    assert!(state.take_companion_chat_planning(id(1)).unwrap().is_some());
    let mut residents = state.residents();
    residents.actors[0].lifecycle = ActorLifecycle::Pending;
    state.commit_residents(residents);
}

#[test]
fn idle_pending_capture_keeps_durable_bodies_inactive_metadata_and_revision_clean() {
    let mut state = enabled(loaded(1, 2));
    let old = current(&state).clone();
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(current(&state), &old);
    assert_eq!(state.save_stats(), SaveStats::default());
    assert!(
        state
            .select(SaveMode::All, SaveBudget::default())
            .is_empty()
    );
}

#[test]
fn restored_nonempty_tasks_dirty_first_observation_even_with_equal_normalized_wire() {
    let mut state = enabled(with_task(1));
    let old = current(&state).clone();
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(current(&state), &old);
    let targets = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].key, SaveKey::Companions);
    assert_eq!(targets[0].revision, 10);
}

#[test]
fn actual_planning_take_dirties_equal_wire_after_previous_exact_ack() {
    let mut state = enabled(with_task(1));
    state.advance_tick(TickBudget::full()).unwrap();
    let targets = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(targets.len(), 1);
    acknowledge(&mut state, targets);
    let old = current(&state).clone();
    take_planning_without_body_change(&mut state);
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(current(&state), &old);
    let targets = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].revision, 11);
}

#[test]
fn equal_wire_raw_change_during_flight_is_cleared_by_exact_source_ack() {
    let mut state = enabled(with_task(1));
    state.advance_tick(TickBudget::full()).unwrap();
    let old = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(old.len(), 1);
    take_planning_without_body_change(&mut state);
    state.advance_tick(TickBudget::full()).unwrap();
    acknowledge(&mut state, old);
    assert!(
        state
            .select(SaveMode::All, SaveBudget::default())
            .is_empty()
    );
    assert_eq!(
        state.actor_save_current(&SaveKey::Companions).unwrap().0,
        10
    );
}

#[test]
fn settled_active_body_and_inventory_replace_only_the_configured_record() {
    let mut state = enabled(loaded(1, 2));
    let old = current(&state).clone();
    let mut residents = state.residents();
    residents.actors[0].lifecycle = ActorLifecycle::Active;
    residents.actors[0].look = mornlea_domain::LookAngles::try_new(0.75, 0.25).unwrap();
    residents
        .inventories
        .get_mut(&ActorKey::Companion(id(1)))
        .unwrap()
        .selected = mornlea_domain::HotbarSlot::new(3).unwrap();
    state.commit_residents(residents);
    state.advance_tick(TickBudget::full()).unwrap();
    let residents = state.residents();
    let projected = mornlea_server::core::actor_projection::project_companion(
        &residents.actors[0],
        Some(&residents.inventories[&ActorKey::Companion(id(1))]),
    )
    .unwrap();
    assert_eq!(current(&state).records[0], projected);
    assert_ne!(current(&state).records[0], old.records[0]);
    assert_eq!(current(&state).records[1], old.records[1]);
    assert_eq!(current(&state).lifecycles, old.lifecycles);
    assert_eq!(current(&state).agent_namespace_id, old.agent_namespace_id);
}

#[test]
fn wrong_running_runtime_generation_refuses_before_tick_and_keeps_old_aggregate() {
    let mut state = enabled(with_task(4));
    let old = current(&state).clone();
    let mut residents = state.residents();
    let ActorAux::Companion { generation, .. } = &mut residents
        .runtimes
        .get_mut(&ActorKey::Companion(id(1)))
        .unwrap()
        .aux
    else {
        panic!("companion")
    };
    *generation = 2;
    state.commit_residents(residents);
    assert!(state.advance_tick(TickBudget::full()).is_err());
    assert_eq!(state.next_tick(), 0);
    assert_eq!(current(&state), &old);
    assert!(
        state
            .select(SaveMode::All, SaveBudget::default())
            .is_empty()
    );
}

#[test]
fn final_unpublished_capture_keeps_old_target_then_flushes_latest_body() {
    let mut state = enabled(with_task(1));
    state.advance_tick(TickBudget::full()).unwrap();
    let held = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(held.len(), 1);
    let previous = held[0].clone();
    let mut residents = state.residents();
    residents.actors[0].lifecycle = ActorLifecycle::Active;
    residents.actors[0].look = mornlea_domain::LookAngles::try_new(0.75, 0.25).unwrap();
    state.commit_residents(residents);
    state.begin_close();
    state
        .run_final(&mut mornlea_server::core::step::AuthoritativeFinalReducer)
        .unwrap();
    assert_ne!(
        SaveValue::Companions(current(&state).clone()),
        previous.value
    );
    acknowledge(&mut state, held);
    let next = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].revision, 11);
}

#[test]
fn quiet_max_revision_is_accepted_and_actual_dirty_selection_overflow_fences() {
    let mut aggregate = loaded(1, 1);
    aggregate.revision = u64::MAX;
    let mut state = enabled(aggregate);
    state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        state
            .select(SaveMode::All, SaveBudget::default())
            .is_empty()
    );
    let mut aggregate = with_task(1);
    aggregate.revision = u64::MAX;
    let mut state = enabled(aggregate);
    state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        state
            .select(SaveMode::All, SaveBudget::default())
            .is_empty()
    );
    assert_eq!(state.phase(), ServerPhase::Closing);
}

#[test]
fn foreign_companion_overlay_and_missing_inventory_refuse_complete_capture() {
    for foreign in [true, false] {
        let mut state = enabled(loaded(1, 2));
        let old = current(&state).clone();
        let mut residents = state.residents();
        if foreign {
            let inventory = residents.inventories[&ActorKey::Companion(id(1))];
            residents
                .inventories
                .insert(ActorKey::Companion(id(2)), inventory);
        } else {
            residents.inventories.remove(&ActorKey::Companion(id(1)));
        }
        state.commit_residents(residents);
        assert!(state.advance_tick(TickBudget::full()).is_err());
        assert_eq!(state.next_tick(), 0);
        assert_eq!(current(&state), &old);
    }
}

#[test]
fn companion_body_or_runtime_hidden_under_another_family_key_refuses_whole_ownership() {
    for body_lane in [true, false] {
        let mut state = enabled(loaded(1, 2));
        let old = current(&state).clone();
        let mut residents = state.residents();
        let foreign = ActorKey::Hostile(mornlea_domain::HostileId::try_new(1).unwrap());
        if body_lane {
            let mut actor = residents.actors[0].clone();
            actor.key = foreign;
            residents.actors.push(actor);
        } else {
            let runtime = residents.runtimes[&ActorKey::Companion(id(1))].clone();
            residents.runtimes.insert(foreign, runtime);
        }
        state.commit_residents(residents);
        assert!(state.advance_tick(TickBudget::full()).is_err());
        assert_eq!(state.next_tick(), 0);
        assert_eq!(current(&state), &old);
    }
}

#[test]
fn startup_rejects_hidden_companion_owners_before_aggregate_handoff() {
    for body_lane in [true, false] {
        let template = enabled(loaded(1, 2)).residents();
        let mut state = base();
        let mut residents = state.residents();
        let foreign = ActorKey::Hostile(mornlea_domain::HostileId::try_new(1).unwrap());
        if body_lane {
            let mut actor = template.actors[0].clone();
            actor.key = foreign;
            residents.actors.push(actor);
        } else {
            let runtime = template.runtimes[&ActorKey::Companion(id(1))].clone();
            residents.runtimes.insert(foreign, runtime);
        }
        state.commit_residents(residents);
        assert!(
            state
                .enable_companion_persistence(&definitions(1), loaded(1, 2))
                .is_err()
        );
        assert!(!state.companion_persistence_enabled());
        assert!(state.actor_save_current(&SaveKey::Companions).is_none());
    }
}

#[test]
fn oversized_running_task_refuses_without_replacing_any_body_or_old_flight() {
    let mut state = enabled(with_task(4));
    state.advance_tick(TickBudget::full()).unwrap();
    let held = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(held.len(), 1);
    let old = current(&state).clone();
    let mut residents = state.residents();
    let ActorAux::Companion { task, .. } = &mut residents
        .runtimes
        .get_mut(&ActorKey::Companion(id(1)))
        .unwrap()
        .aux
    else {
        panic!("companion")
    };
    task.plan_steps
        .resize(5001, mornlea_storage::PlanStep::default());
    state.commit_residents(residents);
    assert!(state.advance_tick(TickBudget::full()).is_err());
    assert_eq!(current(&state), &old);
    assert_eq!(state.save_stats().in_flight, 1);
    assert_eq!(held[0].revision, 10);
}

#[test]
fn existing_mob_owners_survive_complete_companion_startup_in_either_order() {
    for mobs_first in [true, false] {
        let mut state = base();
        if mobs_first {
            state
                .enable_mob_persistence(
                    mornlea_storage::HostileMobs {
                        revision: 9,
                        records: Vec::new(),
                    },
                    mornlea_storage::PassiveMobs {
                        revision: 5,
                        records: Vec::new(),
                    },
                )
                .unwrap();
        }
        state
            .enable_companion_persistence(&definitions(1), loaded(1, 2))
            .unwrap();
        if !mobs_first {
            state
                .enable_mob_persistence(
                    mornlea_storage::HostileMobs {
                        revision: 9,
                        records: Vec::new(),
                    },
                    mornlea_storage::PassiveMobs {
                        revision: 5,
                        records: Vec::new(),
                    },
                )
                .unwrap();
        }
        state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(state.actor_save_current(&SaveKey::Hostiles).unwrap().0, 9);
        assert_eq!(state.actor_save_current(&SaveKey::Passives).unwrap().0, 5);
        assert_eq!(state.actor_save_current(&SaveKey::Companions).unwrap().0, 9);
        assert!(
            state
                .select(SaveMode::All, SaveBudget::default())
                .is_empty()
        );
    }
}

#[test]
fn startup_reserves_runtime_and_inventory_capacity_before_any_owned_transfer() {
    let template = enabled(loaded(1, 1)).residents();
    for runtime_lane in [true, false] {
        let max_existing = if runtime_lane { 106 } else { 8 };
        for overflow in [false, true] {
            let mut state = base();
            let mut residents = state.residents();
            for number in 1..=max_existing + usize::from(overflow) {
                let key =
                    ActorKey::Hostile(mornlea_domain::HostileId::try_new(number as u64).unwrap());
                if runtime_lane {
                    let mut runtime = template.runtimes[&ActorKey::Companion(id(1))].clone();
                    runtime.key = key;
                    runtime.aux = ActorAux::Hostile {
                        distant_ticks: 0,
                        shoot_cooldown: 0,
                        fresh: false,
                    };
                    residents.runtimes.insert(key, runtime);
                } else {
                    residents.inventories.insert(key, InventoryRecord::empty());
                }
            }
            state.commit_residents(residents.clone());
            let result = state.enable_companion_persistence(&definitions(4), loaded(4, 4));
            if overflow {
                assert!(result.is_err());
                assert!(state.residents().actors.is_empty());
                assert_eq!(state.residents().runtimes, residents.runtimes);
                assert_eq!(state.residents().inventories, residents.inventories);
                assert!(state.actor_save_current(&SaveKey::Companions).is_none());
                assert!(!state.companion_persistence_enabled());
            } else {
                result.unwrap();
                assert_eq!(
                    state.residents().runtimes.len(),
                    if runtime_lane { 110 } else { 4 }
                );
                assert_eq!(
                    state.residents().inventories.len(),
                    if runtime_lane { 4 } else { 12 }
                );
            }
        }
    }
}

#[test]
fn complete_source_raw_observation_outcomes_emit_actual_encoded_targets_for_go_pairing() {
    fn row(snapshot: &OwnedSnapshot) -> serde_json::Value {
        let SaveValue::Companions(save) = &snapshot.value else {
            panic!("companions")
        };
        let encode = |value: &mornlea_storage::CompanionSave| {
            mornlea_storage::encode_companions(value)
                .unwrap()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let mut normalized = save.clone();
        normalized.revision = 1;
        serde_json::json!({"revision":snapshot.revision,"encoded":encode(save),"normalized":encode(&normalized)})
    }
    let mut idle = enabled(loaded(1, 2));
    idle.advance_tick(TickBudget::full()).unwrap();
    assert!(idle.select(SaveMode::All, SaveBudget::default()).is_empty());
    let mut state = enabled(with_task(1));
    state.advance_tick(TickBudget::full()).unwrap();
    let first = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(first.len(), 1);
    let first_row = row(&first[0]);
    acknowledge(&mut state, first);
    take_planning_without_body_change(&mut state);
    state.advance_tick(TickBudget::full()).unwrap();
    let next = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(next.len(), 1);
    let second_row = row(&next[0]);
    assert_eq!(first_row["normalized"], second_row["normalized"]);
    assert_eq!(next[0].revision, 11);
    let mut held = enabled(with_task(1));
    held.advance_tick(TickBudget::full()).unwrap();
    let target = held.select(SaveMode::All, SaveBudget::default());
    assert_eq!(target.len(), 1);
    let held_row = row(&target[0]);
    take_planning_without_body_change(&mut held);
    held.advance_tick(TickBudget::full()).unwrap();
    acknowledge(&mut held, target);
    assert!(held.select(SaveMode::All, SaveBudget::default()).is_empty());
    println!(
        "COMPANION_RAW_ORACLE {}",
        serde_json::json!({"idle":[],"queued_phase":[first_row,second_row],"held":[held_row]})
    );
}

mod actual_disk {
    use super::*;
    use mornlea_domain::{ChunkPos, Dimension};
    use mornlea_server::core::{chunk_driver::ChunkDriver, generation_worker::GenerationPool};
    use mornlea_server::store::{
        disk::{DiskOptions, DiskStore},
        mailbox::StoreMailbox,
        scheduler::{AutosaveScheduler, SchedulerConfig},
    };
    use std::{
        collections::BTreeSet,
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        thread,
        time::{Duration, Instant},
    };
    static ROOTS: AtomicU64 = AtomicU64::new(0);
    pub(super) struct Root(PathBuf);
    impl Root {
        pub(super) fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "source-companion-persistence-{}-{}",
                std::process::id(),
                ROOTS.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    pub(super) struct RealClock;
    impl Clock for RealClock {
        fn monotonic(&self) -> Instant {
            Instant::now()
        }
        fn unix_ms(&self) -> i64 {
            0
        }
    }
    pub(super) fn deadline() -> Deadline {
        Deadline::after(Instant::now(), Duration::from_secs(10)).unwrap()
    }
    fn options() -> DiskOptions {
        DiskOptions {
            region_handle_cap: 1,
            create: mornlea_storage::Metadata {
                format_version: mornlea_storage::METADATA_CURRENT_VERSION,
                seed: 42,
                spawn_dimension: 0,
                spawn_anchor: mornlea_storage::MetadataChunkPos { x: 0, z: 0 },
                world_time_ticks: 1000,
                day_phase_offset: 0,
                weather_kind: 0,
                weather_ticks_remaining: 0,
                depths_spawn_anchor: mornlea_storage::MetadataChunkPos { x: 0, z: 0 },
                depths_seed_salt: 0,
                difficulty: 0,
            },
        }
    }
    fn chunk_key() -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        }
    }
    fn save(aggregate: StoredCompanions) -> mornlea_storage::CompanionSave {
        mornlea_storage::CompanionSave {
            revision: aggregate.revision,
            agent_namespace_id: aggregate.agent_namespace_id,
            records: aggregate.records,
            lifecycles: aggregate.lifecycles,
            queues: aggregate.queues,
        }
    }
    fn target(value: SaveValue) -> OwnedSnapshot {
        let (key, revision, bytes) = match &value {
            SaveValue::Companions(body) => (
                SaveKey::Companions,
                body.revision,
                mornlea_storage::companions_encoded_len(body).unwrap(),
            ),
            SaveValue::Chunk(body) => (SaveKey::Chunk(chunk_key()), body.revision, 1),
            _ => panic!("fixture target"),
        };
        OwnedSnapshot::try_new(key, revision, bytes, SaveUrgency::Autosave, value).unwrap()
    }
    pub(super) fn scheduler(disk: DiskStore) -> AutosaveScheduler<DiskStore> {
        AutosaveScheduler::try_new(
            SchedulerConfig::default(),
            StoreMailbox::try_new_background(
                StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
                disk,
            )
            .unwrap(),
        )
        .unwrap()
    }
    pub(super) fn setup(
        root: &Root,
        aggregate: StoredCompanions,
    ) -> (AuthorityState, AutosaveScheduler<DiskStore>) {
        let mut disk = DiskStore::open(&root.0, options()).unwrap();
        let flat = mornlea_storage::Chunk {
            sections: (0..24)
                .map(|index| mornlea_storage::ContainerSnapshot {
                    kind: mornlea_storage::StorageKind::Single,
                    bits: 0,
                    single: if index < 9 { 2 } else { 0 },
                    palette: Vec::new(),
                    packed: Vec::new(),
                })
                .collect(),
            drops: vec![Default::default(); 32],
            furnaces: vec![Default::default(); 32],
            chests: vec![Default::default(); 16],
        };
        let completion = disk.write(
            SaveTicket::try_from_raw(1).unwrap(),
            SaveRequest {
                snapshots: vec![
                    target(SaveValue::Companions(save(aggregate))),
                    target(SaveValue::Chunk(mornlea_storage::ChunkSave {
                        key: mornlea_storage::ChunkKey {
                            dimension: 0,
                            x: 0,
                            z: 0,
                        },
                        revision: 9,
                        chunk: flat,
                    })),
                ],
            },
        );
        assert_eq!(completion.error, None);
        assert_eq!(completion.committed.len(), 2);
        let LoadedValue::Companions(aggregate) = disk.load(SaveKey::Companions).unwrap() else {
            panic!("companions")
        };
        let mut state = AuthorityState::try_new_with_metadata(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            disk.metadata().clone(),
        )
        .unwrap();
        state.enable_source_player_restoration(1).unwrap();
        state.enable_live_chunks().unwrap();
        state.enable_actor_saves().unwrap();
        state.enable_player_persistence().unwrap();
        state
            .enable_companion_persistence(&definitions(1), aggregate)
            .unwrap();
        let mut store = scheduler(disk);
        state
            .replace_chunk_wants(BTreeSet::from([chunk_key()]))
            .unwrap();
        let mut driver = ChunkDriver::new();
        let mut pool = GenerationPool::try_new(42, false, 1).unwrap();
        driver
            .start_load(&mut state, &mut store, chunk_key(), deadline())
            .unwrap();
        let until = deadline();
        loop {
            store.drive_workers();
            let report = driver.poll(&mut state, &mut store, &mut pool);
            assert_eq!(report.first_error, None);
            if report.retained == 0 {
                break;
            }
            assert!(!until.expired(Instant::now()));
            thread::yield_now();
        }
        state.advance_tick(TickBudget::full()).unwrap();
        pool.close(deadline()).unwrap();
        assert_eq!(
            state.residents().actors[0].lifecycle,
            ActorLifecycle::Active
        );
        (state, store)
    }
    pub(super) fn autosave(state: &mut AuthorityState, store: &mut AutosaveScheduler<DiskStore>) {
        let until = deadline();
        let mut tick = 6000;
        loop {
            store.drive_workers();
            let report = store.poll_tick(tick, SaveBudget::default(), state).unwrap();
            if report.stats == SaveStats::default()
                && store.tracked_submits() == 0
                && !store.metadata_pending()
            {
                break;
            }
            assert!(!until.expired(Instant::now()));
            tick += 1;
            thread::yield_now();
        }
        assert_eq!(store.last_error(), None);
    }
    pub(super) fn reopen(root: &Root, expected: mornlea_storage::CompanionSave) {
        let mut disk = DiskStore::open(&root.0, options()).unwrap();
        assert_eq!(
            disk.load(SaveKey::Companions).unwrap(),
            LoadedValue::Companions(StoredCompanions {
                source_schema: 5,
                revision: expected.revision,
                agent_namespace_id: expected.agent_namespace_id,
                records: expected.records,
                lifecycles: expected.lifecycles,
                queues: expected.queues
            })
        );
        disk.close().unwrap();
    }
    #[test]
    fn background_disk_restore_native_placement_task_progress_autosave_and_complete_reopen() {
        let root = Root::new();
        let original = with_task(4);
        let (mut state, mut store) = setup(&root, original.clone());
        let residents = state.residents();
        let runtime = &residents.runtimes[&ActorKey::Companion(id(1))];
        let ActorAux::Companion {
            generation,
            attempt,
            task,
            mining_target,
        } = &runtime.aux
        else {
            panic!("companion")
        };
        assert_eq!((*generation, *attempt), (1, 0));
        assert_eq!(task, &original.queues[0].current);
        assert!(mining_target.is_none());
        assert_eq!(runtime.path, None);
        assert_eq!(current(&state).queues, original.queues);
        assert_eq!(current(&state).records[1], original.records[0]);
        autosave(&mut state, &mut store);
        let mut expected = current(&state).clone();
        expected.revision = state.actor_save_current(&SaveKey::Companions).unwrap().0;
        assert_eq!(expected.revision, 10);
        store.close(deadline()).unwrap();
        reopen(&root, expected);
    }
    #[test]
    fn missing_and_legacy_merge_are_saved_by_actual_startup_caller_before_enable() {
        for legacy in [false, true] {
            let root = Root::new();
            let mut disk = DiskStore::open(&root.0, options()).unwrap();
            assert!(disk.load(SaveKey::Companions).is_err());
            let input = if legacy {
                let mut input = loaded(1, 1);
                input.source_schema = 4;
                input.agent_namespace_id = PlayerId::default();
                input.lifecycles.clear();
                input.queues.clear();
                input
            } else {
                StoredCompanions::default()
            };
            let active = vec![loaded(1, 1).records.remove(0)];
            let mut tag = 200;
            let mut generate = || {
                tag += 1;
                Ok::<_, std::convert::Infallible>(save_id(tag))
            };
            let (merged, changed) =
                mornlea_storage::merge_companions_v5(&input, &active, Some(&mut generate)).unwrap();
            assert!(changed);
            let completion = disk.write(
                SaveTicket::try_from_raw(1).unwrap(),
                SaveRequest {
                    snapshots: vec![target(SaveValue::Companions(save(merged)))],
                },
            );
            assert_eq!(completion.error, None);
            assert_eq!(completion.committed.len(), 1);
            let LoadedValue::Companions(durable) = disk.load(SaveKey::Companions).unwrap() else {
                panic!("companions")
            };
            let expected = durable.clone();
            let mut state = base();
            state
                .enable_companion_persistence(&definitions(1), durable)
                .unwrap();
            assert_eq!(
                state.actor_save_current(&SaveKey::Companions).unwrap().0,
                expected.revision
            );
            assert_eq!(
                current(&state).agent_namespace_id,
                expected.agent_namespace_id
            );
            assert_eq!(current(&state).lifecycles, expected.lifecycles);
            disk.close().unwrap();
        }
    }

    // The hook fails one real companion temporary-file sync, never replacing the disk backend.
    struct FailOnce {
        armed: std::sync::Arc<std::sync::atomic::AtomicBool>,
        attempts: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
        companion_payload: bool,
    }
    impl mornlea_server::store::io::DiskIo for FailOnce {
        fn write(&mut self, file: &mut fs::File, bytes: &[u8]) -> std::io::Result<usize> {
            self.companion_payload = bytes.starts_with(b"MCAI");
            if self.companion_payload {
                self.attempts.lock().unwrap().push(bytes.to_vec());
            }
            std::io::Write::write(file, bytes)
        }
        fn boundary(
            &mut self,
            point: IoFaultPoint,
            phase: mornlea_server::store::io::IoPhase,
        ) -> std::io::Result<()> {
            if point == IoFaultPoint::TempSync
                && phase == mornlea_server::store::io::IoPhase::Before
                && self.companion_payload
                && self.armed.swap(false, Ordering::AcqRel)
            {
                return Err(std::io::ErrorKind::PermissionDenied.into());
            }
            Ok(())
        }
    }
    pub(super) fn failing_disk(
        root: &Root,
        attempts: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    ) -> DiskStore {
        let armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        DiskStore::with_io(
            &root.0,
            options(),
            Box::new(move || {
                Box::new(FailOnce {
                    armed: armed.clone(),
                    attempts: attempts.clone(),
                    companion_payload: false,
                })
            }),
        )
        .unwrap()
    }
    #[test]
    fn actual_startup_companion_sync_failure_keeps_authority_unconstructed_and_missing() {
        let root = Root::new();
        let attempts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut disk = failing_disk(&root, attempts.clone());
        let state = base();
        let completion = disk.write(
            SaveTicket::try_from_raw(1).unwrap(),
            SaveRequest {
                snapshots: vec![target(SaveValue::Companions(save(loaded(1, 1))))],
            },
        );
        assert_eq!(
            completion.error,
            Some(ServerError::Io {
                operation: Operation::SyncPayload,
                kind: std::io::ErrorKind::PermissionDenied
            })
        );
        assert!(completion.committed.is_empty());
        assert_eq!(attempts.lock().unwrap().len(), 1);
        assert!(!state.companion_persistence_enabled());
        assert!(state.residents().actors.is_empty());
        assert!(state.actor_save_current(&SaveKey::Companions).is_none());
        assert!(disk.load(SaveKey::Companions).is_err());
        disk.close().unwrap();
    }
    #[test]
    fn failed_actual_target_retries_identical_bytes_before_latest_final_body_task_flush() {
        let root = Root::new();
        let (mut state, mut store) = setup(&root, with_task(1));
        autosave(&mut state, &mut store);
        store.close(deadline()).unwrap();
        assert_eq!(
            state.actor_save_current(&SaveKey::Companions).unwrap().0,
            10
        );
        let task = state.take_companion_chat_planning(id(1)).unwrap().unwrap();
        state.advance_tick(TickBudget::full()).unwrap();
        let mut old = current(&state).clone();
        old.revision = 11;
        let attempts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
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
        assert_eq!(
            attempts.lock().unwrap()[0],
            mornlea_storage::encode_companions(&old).unwrap()
        );
        assert!(
            state
                .install_companion_chat_plan(
                    id(1),
                    task.generation,
                    AgentPlan::try_new("move".into(), vec![PlanStep::GoTo { x: 12, y: 80, z: 8 }])
                        .unwrap()
                )
                .unwrap()
        );
        let mut residents = state.residents();
        residents.actors[0].look = mornlea_domain::LookAngles::try_new(0.75, 0.25).unwrap();
        state.commit_residents(residents);
        state.begin_close();
        state
            .run_final(&mut mornlea_server::core::step::AuthoritativeFinalReducer)
            .unwrap();
        let mut latest = current(&state).clone();
        latest.revision = 12;
        assert_ne!(latest.queues, old.queues);
        assert_ne!(latest.records, old.records);
        store.flush(deadline(), &mut state, &RealClock).unwrap();
        store.close(deadline()).unwrap();
        let bytes = attempts.lock().unwrap();
        assert_eq!(bytes.len(), 3);
        assert_eq!(bytes[0], bytes[1]);
        assert_eq!(
            bytes[2],
            mornlea_storage::encode_companions(&latest).unwrap()
        );
        drop(bytes);
        assert_eq!(
            state.actor_save_current(&SaveKey::Companions).unwrap().0,
            12
        );
        reopen(&root, latest);
    }
}
