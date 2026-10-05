//! Public retained-owner recipes; disk I/O and actual actor capture remain separate consumers.
use mornlea_domain::PlayerId;
use mornlea_server::contracts::*;
use mornlea_server::core::actor_save::ActorSaveLedger;
use mornlea_server::store::mailbox::StoreMailbox;
use mornlea_storage::{
    CompanionBody, CompanionSave, HostileMob, HostileMobsSave, Inventory, PassiveMob,
    PassiveMobsSave, PlayerLocation, PlayerSave, StoredCompanionLifecycle, StoredCompanionQueue,
    player_encoded_len,
};

fn id(tag: u8) -> PlayerId {
    let mut raw = [0; 16];
    raw[0] = tag;
    raw[6] = 0x40;
    raw[8] = 0x80;
    PlayerId::try_from_bytes(raw).unwrap()
}
fn value(tag: u8, revision: u64) -> SaveValue {
    SaveValue::Player(PlayerSave {
        player_id: mornlea_storage::PlayerId::from_bytes(id(tag).bytes()),
        revision,
        display_name: "Ada".into(),
        current: PlayerLocation {
            dimension: 0,
            position: [8.5, 65.0, 8.5],
        },
        yaw: 0.1,
        pitch: 0.2,
        safe: None,
        inventory: Inventory::default(),
        health: 15,
        hunger: 17,
        saturation_milli: 9000,
        exhaustion_milli: 250,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor: [Default::default(); 4],
    })
}
fn changed(mut value: SaveValue, f: impl FnOnce(&mut PlayerSave)) -> SaveValue {
    let SaveValue::Player(player) = &mut value else {
        panic!("expected player")
    };
    f(player);
    value
}
fn budget() -> SaveBudget {
    SaveBudget::default()
}
fn loaded() -> ActorSaveLedger {
    let mut ledger = ActorSaveLedger::default();
    ledger.retain(value(1, 9), 9, false, true, true).unwrap();
    ledger
}
fn take(ledger: &mut ActorSaveLedger) -> OwnedSnapshot {
    let selected = ledger.select(SaveMode::All, budget()).unwrap();
    assert_eq!(selected.len(), 1);
    selected.into_iter().next().unwrap()
}
fn snapshot_player(snapshot: &OwnedSnapshot) -> &PlayerSave {
    let SaveValue::Player(player) = &snapshot.value else {
        panic!("expected player")
    };
    player
}

#[test]
fn latest_during_flight_preserves_exact_target_and_forced_followup() {
    let mut ledger = loaded();
    assert!(ledger.select(SaveMode::All, budget()).unwrap().is_empty());
    let current = changed(value(1, 9), |p| p.hunger = 16);
    assert!(ledger.observe(current.clone(), true, false).unwrap());
    let target = take(&mut ledger);
    assert_eq!(target.revision, 10);
    assert_eq!(snapshot_player(&target).hunger, 16);
    assert_eq!(
        target.estimated_bytes,
        player_encoded_len(snapshot_player(&target)).unwrap()
    );
    let newer = changed(current, |p| p.exhaustion_milli = 350);
    ledger.observe(newer, true, true).unwrap();
    assert!(ledger.matches(&target));
    assert_eq!(ledger.stats().in_flight, 1);
    assert_eq!(ledger.stats().dirty, 1);
    assert!(ledger.select(SaveMode::All, budget()).unwrap().is_empty());
    assert!(ledger.saved(&target).unwrap());
    assert_eq!(ledger.current(&target.key).unwrap().0, 10);
    let followup = ledger.select(SaveMode::Urgent, budget()).unwrap();
    assert_eq!(followup.len(), 1);
    assert_eq!(followup[0].revision, 11);
    assert_eq!(followup[0].urgency, SaveUrgency::Unload);
    assert_eq!(snapshot_player(&followup[0]).exhaustion_milli, 350);
    assert!(ledger.saved(&followup[0]).unwrap());
    assert_eq!(ledger.stats(), SaveStats::default());
    assert!(ledger.save_keys().is_empty());
}

#[test]
fn latest_return_to_selected_contents_clears_without_redundant_target() {
    let mut ledger = loaded();
    assert!(!ledger.observe(value(1, 9), true, false).unwrap());
    let current = changed(value(1, 9), |p| p.hunger = 16);
    ledger.observe(current.clone(), true, false).unwrap();
    let target = take(&mut ledger);
    ledger
        .observe(changed(current.clone(), |p| p.hunger = 15), true, true)
        .unwrap();
    ledger.observe(current, true, false).unwrap();
    assert!(ledger.saved(&target).unwrap());
    assert_eq!(ledger.stats(), SaveStats::default());
    assert!(
        ledger
            .select(SaveMode::Urgent, budget())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn missing_current_is_retained_but_only_confirmation_can_make_it_eligible() {
    let mut ledger = ActorSaveLedger::default();
    ledger.retain(value(1, 1), 0, false, false, true).unwrap();
    let current = changed(value(1, 1), |p| p.health = 14);
    ledger.observe(current.clone(), false, true).unwrap();
    assert!(ledger.select(SaveMode::All, budget()).unwrap().is_empty());
    assert_eq!(ledger.stats(), SaveStats::default());
    assert_eq!(ledger.current(&SaveKey::Player(id(1))).unwrap().0, 0);
    assert!(!ledger.observe(current, true, false).unwrap());
    let first = take(&mut ledger);
    assert_eq!(first.revision, 1);
    assert_eq!(snapshot_player(&first).health, 14);
    ledger.saved(&first).unwrap();
    assert_eq!(ledger.current(&first.key).unwrap().0, 1);
}

#[test]
fn refused_owner_restores_urgency_and_forged_preimages_do_not_release() {
    let mut ledger = loaded();
    ledger
        .observe(changed(value(1, 9), |p| p.health = 14), true, true)
        .unwrap();
    let target = ledger.select(SaveMode::Urgent, budget()).unwrap().remove(0);
    let mut bad = target.clone();
    bad.estimated_bytes += 1;
    assert!(!ledger.return_dirty(&bad));
    assert!(ledger.validate_saved(&bad).is_err());
    assert!(ledger.saved(&bad).is_err());
    bad = target.clone();
    bad.urgency = SaveUrgency::Autosave;
    assert!(ledger.validate_saved(&bad).is_err());
    bad = target.clone();
    bad.value = changed(bad.value, |p| p.hunger = 12);
    assert!(ledger.saved(&bad).is_err());
    assert!(ledger.matches(&target));
    assert!(ledger.return_dirty(&target));
    assert_eq!(ledger.stats().in_flight, 0);
    let selected = ledger.select(SaveMode::Urgent, budget()).unwrap();
    assert_eq!(selected, vec![target]);
}

// This backend executes typed failure/success through the actual inline mailbox;
// it is a consumer double, not evidence of physical filesystem durability.
struct RetryBackend(bool);
impl DiskBackend for RetryBackend {
    fn write(&mut self, ticket: SaveTicket, request: SaveRequest) -> SaveCompletion {
        let submitted: Vec<_> = request
            .snapshots
            .iter()
            .map(|s| (s.key.clone(), s.revision))
            .collect();
        let fail = !self.0;
        self.0 = true;
        SaveCompletion {
            ticket,
            snapshots: request.snapshots,
            committed: if fail { vec![] } else { submitted.clone() },
            submitted,
            error: fail.then_some(ServerError::Internal {
                invariant: "injected actor write failure",
            }),
        }
    }
    fn load(&mut self, _key: SaveKey) -> Result<LoadedValue, ServerError> {
        Err(ServerError::InvalidInput {
            field: "unused actor load",
        })
    }
    fn sync(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
}
// Inline poll_tick only borrows stats while dispatching already selected jobs.
// These guards keep this recipe from claiming automatic SaveAuthority composition.
struct DispatchObservation<'a>(&'a ActorSaveLedger);
impl SaveAuthority for DispatchObservation<'_> {
    fn select(&mut self, _mode: SaveMode, _budget: SaveBudget) -> Vec<OwnedSnapshot> {
        panic!("inline dispatch must not capture targets")
    }
    fn return_dirty(&mut self, _snapshot: OwnedSnapshot) {
        panic!("inline dispatch must not return authority targets")
    }
    fn apply_completion(&mut self, _completion: SaveCompletion) -> AckReport {
        panic!("inline dispatch must not apply authority completions")
    }
    fn save_stats(&self) -> SaveStats {
        self.0.stats()
    }
    fn metadata_snapshot(&self) -> OwnedSnapshot {
        panic!("inline dispatch must not capture metadata")
    }
}
fn mailbox(players: usize) -> StoreMailbox<RetryBackend> {
    StoreMailbox::try_new(
        StoreLimits::try_new(2, players, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
        RetryBackend(false),
    )
    .unwrap()
}
fn close_mailbox(mailbox: &mut StoreMailbox<RetryBackend>) {
    mailbox
        .close(
            Deadline::after(std::time::Instant::now(), std::time::Duration::from_secs(1)).unwrap(),
        )
        .unwrap();
}

#[test]
fn failed_admitted_target_stays_charged_until_the_same_retry_is_durable() {
    let mut ledger = loaded();
    let mut mailbox = mailbox(16);
    ledger
        .observe(changed(value(1, 9), |p| p.hunger = 16), true, false)
        .unwrap();
    let selected = take(&mut ledger);
    let ticket = mailbox
        .submit(SaveRequest {
            snapshots: vec![selected.clone()],
        })
        .unwrap();
    mailbox
        .poll_tick(0, budget(), &mut DispatchObservation(&ledger))
        .unwrap();
    assert_eq!(mailbox.drive_workers(), 1);
    let SavePoll::Completed(failed) = StoreHandle::poll(&mut mailbox, ticket) else {
        panic!("expected double completion")
    };
    assert!(failed.error.is_some());
    assert!(failed.committed.is_empty());
    assert_eq!(failed.snapshots, vec![selected.clone()]);
    let retry = failed.snapshots.into_iter().next().unwrap();
    ledger
        .observe(changed(value(1, 9), |p| p.hunger = 15), true, false)
        .unwrap();
    assert!(ledger.has_target(&selected.key, 10));
    ledger.validate_saved(&retry).unwrap();
    assert!(ledger.select(SaveMode::All, budget()).unwrap().is_empty());
    assert_eq!(ledger.stats().in_flight, 1);
    let ticket = mailbox
        .submit(SaveRequest {
            snapshots: vec![retry],
        })
        .unwrap();
    mailbox
        .poll_tick(0, budget(), &mut DispatchObservation(&ledger))
        .unwrap();
    assert_eq!(mailbox.drive_workers(), 1);
    let SavePoll::Completed(durable) = StoreHandle::poll(&mut mailbox, ticket) else {
        panic!("expected double completion")
    };
    assert_eq!(durable.error, None);
    assert_eq!(durable.committed, vec![(selected.key.clone(), 10)]);
    assert_eq!(durable.snapshots, vec![selected.clone()]);
    assert!(ledger.saved(&durable.snapshots[0]).unwrap());
    assert_eq!(take(&mut ledger).revision, 11);
    close_mailbox(&mut mailbox);
}

#[test]
fn mailbox_capacity_refusal_returns_the_exact_selected_owner() {
    let mut ledger = loaded();
    let mut mailbox = mailbox(0);
    ledger
        .observe(changed(value(1, 9), |p| p.health = 14), true, true)
        .unwrap();
    let selected = take(&mut ledger);
    let refused = mailbox
        .submit(SaveRequest {
            snapshots: vec![selected.clone()],
        })
        .unwrap_err();
    assert!(matches!(refused.error, ServerError::Capacity { .. }));
    assert_eq!(refused.request.snapshots, vec![selected.clone()]);
    assert!(ledger.return_dirty(&refused.request.snapshots[0]));
    assert_eq!(
        ledger.select(SaveMode::Urgent, budget()).unwrap(),
        vec![selected]
    );
    close_mailbox(&mut mailbox);
}

#[test]
fn rewrite_and_stale_ack_have_separate_ownership() {
    let mut ledger = ActorSaveLedger::default();
    ledger.retain(value(1, 9), 9, true, true, true).unwrap();
    let target = take(&mut ledger);
    let mut stale = target.clone();
    stale.revision = 9;
    assert!(!ledger.saved(&stale).unwrap());
    assert!(ledger.matches(&target));
    assert!(ledger.saved(&target).unwrap());
    assert_eq!(ledger.stats(), SaveStats::default());
}

#[test]
fn player_capacity_evicts_only_clean_unpinned_entries_after_validation() {
    let mut ledger = ActorSaveLedger::default();
    for tag in 1..=16 {
        ledger.retain(value(tag, 9), 9, false, true, true).unwrap();
    }
    let refusal = ServerError::Capacity {
        resource: Resource::Players,
        limit: 16,
        observed: 17,
    };
    assert_eq!(
        ledger.retain(value(17, 9), 9, false, true, true),
        Err(refusal)
    );
    ledger.set_pinned(&SaveKey::Player(id(1)), false).unwrap();
    let invalid = changed(value(17, 9), |p| p.health = 21);
    assert!(ledger.retain(invalid, 9, false, true, true).is_err());
    assert!(ledger.current(&SaveKey::Player(id(1))).is_some());
    ledger.retain(value(17, 9), 9, false, true, true).unwrap();
    assert!(ledger.current(&SaveKey::Player(id(1))).is_none());
    assert!(ledger.current(&SaveKey::Player(id(17))).is_some());
    ledger
        .observe(changed(value(2, 9), |p| p.health = 14), true, false)
        .unwrap();
    ledger.set_pinned(&SaveKey::Player(id(2)), false).unwrap();
    assert_eq!(
        ledger.retain(value(18, 9), 9, false, true, true),
        Err(refusal)
    );
    let target = take(&mut ledger);
    assert_eq!(
        ledger.retain(value(18, 9), 9, false, true, true),
        Err(refusal)
    );
    ledger.saved(&target).unwrap();
    ledger.retain(value(18, 9), 9, false, true, true).unwrap();
    assert!(ledger.current(&SaveKey::Player(id(2))).is_none());
}

fn companions() -> SaveValue {
    SaveValue::Companions(CompanionSave {
        revision: 5,
        agent_namespace_id: mornlea_storage::PlayerId::from_bytes(id(50).bytes()),
        records: vec![],
        lifecycles: vec![],
        queues: vec![],
    })
}
fn hostile(n: u64) -> HostileMob {
    HostileMob {
        id: n,
        dimension: 0,
        position: [8.5, 65.0, 8.5],
        velocity: [0.0; 3],
        on_ground: true,
        yaw: 0.0,
        health: 20,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        has_target: false,
        player_id: Default::default(),
        next_repath_ticks: 0,
        distant_ticks: 0,
        kind: 0,
    }
}
fn passive(n: u64) -> PassiveMob {
    PassiveMob {
        id: n,
        dimension: 0,
        position: [8.5, 65.0, 8.5],
        velocity: [0.0; 3],
        on_ground: true,
        yaw: 0.0,
        health: 20,
    }
}

#[test]
fn all_families_select_in_key_order_and_canonicalize_aggregate_membership() {
    let mut ledger = ActorSaveLedger::default();
    let hostiles = SaveValue::Hostiles(HostileMobsSave {
        revision: 5,
        records: vec![hostile(2), hostile(1)],
    });
    let passives = SaveValue::Passives(PassiveMobsSave {
        revision: 5,
        records: vec![passive(2), passive(1)],
    });
    for value in [
        passives.clone(),
        hostiles.clone(),
        companions(),
        value(2, 5),
        value(1, 5),
    ] {
        ledger.retain(value, 5, true, true, true).unwrap();
    }
    let mut ordered = hostiles;
    let SaveValue::Hostiles(h) = &mut ordered else {
        unreachable!()
    };
    h.records.reverse();
    assert!(!ledger.observe(ordered, true, false).unwrap());
    let selected = ledger.select(SaveMode::All, budget()).unwrap();
    assert_eq!(
        selected.iter().map(|s| s.key.clone()).collect::<Vec<_>>(),
        vec![
            SaveKey::Player(id(1)),
            SaveKey::Player(id(2)),
            SaveKey::Companions,
            SaveKey::Hostiles,
            SaveKey::Passives
        ]
    );
    assert!(selected.iter().all(|s| s.revision == 6));
    let SaveValue::Passives(p) = &selected[4].value else {
        unreachable!()
    };
    assert_eq!(
        p.records.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![1, 2]
    );
    for target in &selected {
        assert!(ledger.saved(target).unwrap());
    }
    assert_eq!(ledger.stats(), SaveStats::default());
    assert!(!ledger.observe(passives, true, false).unwrap());
}

#[test]
fn whole_first_target_and_later_budget_limits_do_not_split_owners() {
    let mut ledger = ActorSaveLedger::default();
    for tag in 1..=3 {
        ledger.retain(value(tag, 9), 9, true, true, true).unwrap();
    }
    let first = ledger
        .select(
            SaveMode::All,
            SaveBudget {
                chunks: 0,
                estimated_bytes: 0,
            },
        )
        .unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].key, SaveKey::Player(id(1)));
    let second = ledger
        .select(
            SaveMode::All,
            SaveBudget {
                chunks: 2,
                estimated_bytes: 1,
            },
        )
        .unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].key, SaveKey::Player(id(2)));
    assert_eq!(ledger.stats().in_flight, 2);
    let third = take(&mut ledger);
    assert_eq!(third.key, SaveKey::Player(id(3)));
    assert!(ledger.select(SaveMode::All, budget()).unwrap().is_empty());
}

#[test]
fn urgent_selection_skips_ordinary_dirty_records() {
    let mut ledger = loaded();
    ledger.retain(value(2, 9), 9, true, true, true).unwrap();
    ledger
        .observe(changed(value(1, 9), |p| p.health = 14), true, true)
        .unwrap();
    let urgent = ledger.select(SaveMode::Urgent, budget()).unwrap();
    assert_eq!(urgent.len(), 1);
    assert_eq!(urgent[0].key, SaveKey::Player(id(1)));
    assert_eq!(take(&mut ledger).key, SaveKey::Player(id(2)));
}

#[test]
fn invalid_values_and_revision_overflow_leave_selection_atomic() {
    let mut ledger = loaded();
    let before = ledger.current(&SaveKey::Player(id(1))).unwrap().1.clone();
    assert!(ledger.retain(value(1, 9), 9, false, true, true).is_err());
    assert!(ledger.retain(value(2, 9), 8, false, true, true).is_err());
    assert!(ledger.retain(value(2, 0), 0, false, false, true).is_err());
    let metadata = mornlea_storage::Metadata {
        format_version: mornlea_storage::METADATA_CURRENT_VERSION,
        seed: 42,
        spawn_dimension: 0,
        spawn_anchor: mornlea_storage::MetadataChunkPos { x: 0, z: 0 },
        world_time_ticks: 0,
        day_phase_offset: 0,
        weather_kind: 0,
        weather_ticks_remaining: 0,
        depths_spawn_anchor: mornlea_storage::MetadataChunkPos { x: 0, z: 0 },
        depths_seed_salt: 0,
        difficulty: 0,
    };
    assert!(
        ledger
            .retain(SaveValue::Metadata(metadata), 1, true, true, true)
            .is_err()
    );
    assert!(
        ledger
            .observe(
                changed(value(1, 9), |p| p.player_id = Default::default()),
                true,
                false
            )
            .is_err()
    );
    assert_eq!(ledger.current(&SaveKey::Player(id(1))).unwrap().1, &before);
    assert!(ledger.observe(value(2, 9), true, false).is_err());
    ledger
        .retain(value(2, u64::MAX), u64::MAX, true, true, true)
        .unwrap();
    ledger
        .observe(changed(value(1, 9), |p| p.hunger = 16), true, false)
        .unwrap();
    let stats = ledger.stats();
    assert!(ledger.select(SaveMode::All, budget()).is_err());
    assert_eq!(ledger.stats(), stats);
    assert_eq!(stats.in_flight, 0);
}

#[test]
fn freeze_preserves_old_ack_and_distinct_latest_unsaved_accounting() {
    let mut ledger = loaded();
    ledger
        .observe(changed(value(1, 9), |p| p.health = 14), true, false)
        .unwrap();
    let target = take(&mut ledger);
    assert_eq!(ledger.stats().dirty, 0);
    assert_eq!(
        ledger.stats().estimated_unsaved_bytes,
        target.estimated_bytes
    );
    let newer = changed(value(1, 9), |p| {
        p.health = 14;
        p.respawn_present = true;
        p.respawn_position = [8.0, 65.0, 8.0];
    });
    ledger.observe(newer, true, false).unwrap();
    assert_eq!(ledger.stats().dirty, 1);
    assert!(ledger.stats().estimated_unsaved_bytes > target.estimated_bytes);
    assert_eq!(ledger.save_keys(), vec![target.key.clone()]);
    ledger.freeze();
    let frozen = ServerError::InvalidState {
        phase: ServerPhase::Closing,
    };
    assert_eq!(ledger.observe(value(1, 9), true, false), Err(frozen));
    assert_eq!(
        ledger.retain(value(2, 9), 9, false, true, true),
        Err(frozen)
    );
    assert_eq!(ledger.set_pinned(&target.key, false), Err(frozen));
    assert_eq!(ledger.select(SaveMode::All, budget()), Err(frozen));
    assert!(ledger.saved(&target).unwrap());
    assert_eq!(ledger.stats().dirty, 1);
    assert_eq!(ledger.stats().in_flight, 0);
}

#[test]
fn companion_full_lifecycle_and_queue_content_remain_owned() {
    let SaveValue::Companions(mut aggregate) = companions() else {
        unreachable!()
    };
    let body_id = mornlea_storage::PlayerId::from_bytes(id(3).bytes());
    aggregate.records.push(CompanionBody {
        id: body_id,
        dimension: 0,
        position: [8.5, 65.0, 8.5],
        yaw: 0.0,
        pitch: 0.0,
        inventory: Inventory::default(),
    });
    aggregate.lifecycles.push(StoredCompanionLifecycle {
        id: body_id,
        active: true,
        memory_epoch: 1,
        memory_revision: 0,
        memory_operation_id: Default::default(),
        summary: String::new(),
        tombstone_operation_id: Default::default(),
    });
    aggregate.queues.push(StoredCompanionQueue {
        id: body_id,
        pending: vec!["mine stone".into()],
        ..Default::default()
    });
    let mut ledger = ActorSaveLedger::default();
    ledger
        .retain(
            SaveValue::Companions(aggregate.clone()),
            5,
            false,
            true,
            true,
        )
        .unwrap();
    aggregate.queues[0].pending.push("return home".into());
    ledger
        .observe(SaveValue::Companions(aggregate.clone()), true, false)
        .unwrap();
    let target = take(&mut ledger);
    let SaveValue::Companions(captured) = &target.value else {
        unreachable!()
    };
    assert_eq!(captured.lifecycles, aggregate.lifecycles);
    assert_eq!(captured.queues, aggregate.queues);
    assert_eq!(captured.agent_namespace_id, aggregate.agent_namespace_id);
    assert_eq!(target.revision, 6);
    aggregate.lifecycles[0].summary = "settled memory".into();
    aggregate.lifecycles[0].memory_revision = 1;
    aggregate.lifecycles[0].memory_operation_id =
        mornlea_storage::PlayerId::from_bytes(id(51).bytes());
    ledger
        .observe(SaveValue::Companions(aggregate), true, false)
        .unwrap();
    ledger.saved(&target).unwrap();
    assert_eq!(take(&mut ledger).revision, 7);
}

#[test]
fn review_codec_valid_nan_keeps_reflexive_exact_target_identity() {
    let initial = changed(value(1, 9), |p| {
        p.respawn_present = false;
        p.respawn_position = [f32::from_bits(0x7fc01234), f32::INFINITY, -0.0];
    });
    let mut ledger = ActorSaveLedger::default();
    ledger.retain(initial, 9, true, true, true).unwrap();
    let target = take(&mut ledger);
    assert_eq!(
        snapshot_player(&target).respawn_position[0].to_bits(),
        0x7fc01234
    );
    assert!(ledger.matches(&target));
    ledger.validate_saved(&target).unwrap();
    assert!(ledger.return_dirty(&target));
    let retry = take(&mut ledger);
    assert!(ledger.matches(&target));
    assert_eq!(
        snapshot_player(&retry).respawn_position.map(f32::to_bits),
        snapshot_player(&target).respawn_position.map(f32::to_bits)
    );
    assert!(ledger.saved(&retry).unwrap());
    assert_eq!(ledger.current(&retry.key).unwrap().0, 10);
    // Source IEEE comparison still treats absent NaN content as unequal after ACK.
    assert_eq!(ledger.stats().dirty, 1);
    assert_eq!(ledger.stats().in_flight, 0);
}

fn signed_zero_identity(family: u8) {
    let initial = match family {
        0 => changed(value(1, 9), |p| p.yaw = 0.0),
        1 => {
            let SaveValue::Companions(mut aggregate) = companions() else {
                unreachable!()
            };
            let body_id = mornlea_storage::PlayerId::from_bytes(id(3).bytes());
            aggregate.revision = 9;
            aggregate.records.push(CompanionBody {
                id: body_id,
                dimension: 0,
                position: [8.5, 65.0, 8.5],
                yaw: 0.0,
                pitch: 0.0,
                inventory: Inventory::default(),
            });
            aggregate.lifecycles.push(StoredCompanionLifecycle {
                id: body_id,
                active: true,
                memory_epoch: 1,
                memory_revision: 0,
                memory_operation_id: Default::default(),
                summary: String::new(),
                tombstone_operation_id: Default::default(),
            });
            SaveValue::Companions(aggregate)
        }
        2 => SaveValue::Hostiles(HostileMobsSave {
            revision: 9,
            records: vec![hostile(1)],
        }),
        3 => SaveValue::Passives(PassiveMobsSave {
            revision: 9,
            records: vec![passive(1)],
        }),
        _ => panic!("unsupported actor family"),
    };
    let mut ledger = ActorSaveLedger::default();
    ledger.retain(initial, 9, true, true, true).unwrap();
    let target = take(&mut ledger);
    let mut forged = target.clone();
    match &mut forged.value {
        SaveValue::Player(player) => player.yaw = -0.0,
        SaveValue::Companions(aggregate) => aggregate.records[0].yaw = -0.0,
        SaveValue::Hostiles(aggregate) => aggregate.records[0].yaw = -0.0,
        SaveValue::Passives(aggregate) => aggregate.records[0].yaw = -0.0,
        _ => unreachable!(),
    }
    assert!(!ledger.matches(&forged));
    assert!(ledger.validate_saved(&forged).is_err());
    assert!(!ledger.return_dirty(&forged));
    assert!(ledger.saved(&forged).is_err());
    assert!(ledger.matches(&target));
    assert!(!ledger.observe(forged.value, true, false).unwrap());
    assert!(ledger.saved(&target).unwrap());
    assert_eq!(ledger.stats(), SaveStats::default());
}
#[test]
fn review_player_signed_zero_is_semantically_equal_but_not_exact_identity() {
    signed_zero_identity(0);
}
#[test]
fn review_companion_signed_zero_is_semantically_equal_but_not_exact_identity() {
    signed_zero_identity(1);
}
#[test]
fn review_hostile_signed_zero_is_semantically_equal_but_not_exact_identity() {
    signed_zero_identity(2);
}
#[test]
fn review_passive_signed_zero_is_semantically_equal_but_not_exact_identity() {
    signed_zero_identity(3);
}

#[test]
fn review_eligible_initial_value_requires_an_explicit_save_obligation() {
    let mut ledger = ActorSaveLedger::default();
    assert!(ledger.retain(value(1, 1), 0, false, true, true).is_err());
    assert!(ledger.current(&SaveKey::Player(id(1))).is_none());
    let initial = SaveValue::Hostiles(HostileMobsSave {
        revision: 1,
        records: vec![],
    });
    assert!(
        ledger
            .retain(initial.clone(), 0, false, true, true)
            .is_err()
    );
    ledger
        .retain(initial.clone(), 0, false, false, true)
        .unwrap();
    assert!(!ledger.observe(initial, false, false).unwrap());
    assert!(ledger.select(SaveMode::All, budget()).unwrap().is_empty());
    ledger
        .observe(
            SaveValue::Hostiles(HostileMobsSave {
                revision: 1,
                records: vec![hostile(1)],
            }),
            true,
            false,
        )
        .unwrap();
    assert_eq!(take(&mut ledger).revision, 1);
}
