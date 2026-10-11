//! Explicit producer fixtures qualify authority routing, separately from automatic capture.
use mornlea_domain::{ChunkPos, Dimension, PlayerId};
use mornlea_server::core::{acquisition::AcquiredChunkEvent, world::PreparedChunk};
use mornlea_server::{contracts::*, state::AuthorityState};
use mornlea_storage::{
    Chunk, CompanionSave, ContainerSnapshot, HostileMobsSave, Inventory, PassiveMobsSave,
    PlayerLocation, PlayerSave, StorageKind,
};
use std::collections::BTreeSet;

fn id(tag: u8) -> PlayerId {
    let mut bytes = [0; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    PlayerId::try_from_bytes(bytes).unwrap()
}
fn player(tag: u8, revision: u64) -> SaveValue {
    SaveValue::Player(PlayerSave {
        player_id: mornlea_storage::PlayerId::from_bytes(id(tag).bytes()),
        revision,
        display_name: "Ada".into(),
        current: PlayerLocation {
            dimension: 0,
            position: [8.5, 65.0, 8.5],
        },
        yaw: 0.0,
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
fn aggregates() -> Vec<SaveValue> {
    vec![
        SaveValue::Companions(CompanionSave {
            revision: 5,
            agent_namespace_id: mornlea_storage::PlayerId::from_bytes(id(50).bytes()),
            records: vec![],
            lifecycles: vec![],
            queues: vec![],
        }),
        SaveValue::Hostiles(HostileMobsSave {
            revision: 5,
            records: vec![],
        }),
        SaveValue::Passives(PassiveMobsSave {
            revision: 5,
            records: vec![],
        }),
    ]
}
fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        42,
    )
    .unwrap()
}
fn enabled() -> AuthorityState {
    let mut s = authority();
    s.enable_live_chunks().unwrap();
    s.enable_actor_saves().unwrap();
    s
}
fn retain_four(s: &mut AuthorityState) {
    s.retain_actor_save(player(1, 9), 9, true, true, true)
        .unwrap();
    for v in aggregates() {
        s.retain_actor_save(v, 5, true, true, true).unwrap();
    }
}
fn completion(snapshots: Vec<OwnedSnapshot>) -> SaveCompletion {
    let submitted: Vec<_> = snapshots
        .iter()
        .map(|s| (s.key.clone(), s.revision))
        .collect();
    SaveCompletion {
        ticket: SaveTicket::try_from_raw(1).unwrap(),
        snapshots,
        committed: submitted.clone(),
        submitted,
        error: None,
    }
}
fn key(x: i32) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(x, 0),
    }
}
fn acquire(s: &mut AuthorityState, count: i32) {
    s.replace_chunk_wants((0..count).map(key).collect::<BTreeSet<_>>())
        .unwrap();
    for x in 0..count {
        if x == 8 {
            s.advance_tick(TickBudget::full()).unwrap();
        }
        let reservation = s.reserve_chunk_load(key(x)).unwrap();
        let request = ChunkRequestId::try_new(x as u64 + 1).unwrap();
        s.bind_chunk_load(reservation, request).unwrap();
        let prepared = PreparedChunk::try_new(
            key(x),
            reservation.generation(),
            RecoveredChunk {
                chunk: Chunk {
                    sections: vec![
                        ContainerSnapshot {
                            kind: StorageKind::Single,
                            bits: 0,
                            single: 2,
                            palette: vec![],
                            packed: vec![]
                        };
                        24
                    ],
                    drops: vec![Default::default(); 32],
                    furnaces: vec![Default::default(); 32],
                    chests: vec![Default::default(); 16],
                },
                revision: 9,
                persisted_revision: 7,
                needs_rewrite: true,
                recovered: true,
            },
        )
        .unwrap();
        s.offer_acquired(AcquiredChunkEvent::Load {
            key: key(x),
            generation: reservation.generation(),
            request,
            result: Ok(Some(prepared)),
        })
        .unwrap();
    }
    s.advance_tick(TickBudget::full()).unwrap();
}
#[test]
fn admission_requires_live_empty_running_owner_and_closing_retains_flush() {
    let mut s = authority();
    assert!(s.enable_actor_saves().is_err());
    assert!(
        s.retain_actor_save(player(1, 9), 9, true, true, true)
            .is_err()
    );
    s.enable_live_chunks().unwrap();
    s.enable_actor_saves().unwrap();
    s.enable_actor_saves().unwrap();
    retain_four(&mut s);
    assert!(s.begin_close());
    assert!(s.enable_actor_saves().is_err());
    assert!(s.observe_actor_save(player(1, 9), true, true).is_err());
    assert!(
        s.retain_actor_save(player(2, 9), 9, true, true, true)
            .is_err()
    );
    assert_eq!(s.freeze().save_keys.len(), 4);
    let selected = s.select(SaveMode::All, SaveBudget::default());
    assert_eq!(selected.len(), 4);
    assert_eq!(s.apply_completion(completion(selected)).acked, 4);
    assert_eq!(s.save_stats(), SaveStats::default());
    s.mark_closed();
    assert!(s.select(SaveMode::All, SaveBudget::default()).is_empty());
    let mut late = authority();
    late.enable_live_chunks().unwrap();
    late.advance_tick(TickBudget::full()).unwrap();
    assert!(late.enable_actor_saves().is_err());
}
#[test]
fn nineteen_actor_targets_ack_without_the_chunk_only_total_limit() {
    let mut s = enabled();
    for tag in 1..=16 {
        s.retain_actor_save(player(tag, 9), 9, true, true, true)
            .unwrap();
    }
    for v in aggregates() {
        s.retain_actor_save(v, 5, true, true, true).unwrap();
    }
    let targets = s.select(SaveMode::All, SaveBudget::default());
    assert_eq!(targets.len(), 19);
    for (index, t) in targets.iter().enumerate() {
        assert_eq!(t.revision, if index < 16 { 10 } else { 6 });
    }
    let report = s.apply_completion(completion(targets));
    assert!(report.errors.is_empty());
    assert_eq!(report.acked, 19);
    assert_eq!(s.save_stats(), SaveStats::default());
}
#[test]
fn late_forced_latest_survives_selected_target_confirmation() {
    let mut s = enabled();
    s.retain_actor_save(player(1, 9), 9, true, true, true)
        .unwrap();
    let selected = s.select(SaveMode::All, SaveBudget::default());
    let mut newer = player(1, 9);
    let SaveValue::Player(p) = &mut newer else {
        unreachable!()
    };
    p.hunger = 12;
    assert!(s.observe_actor_save(newer, true, true).unwrap());
    assert_eq!(s.apply_completion(completion(selected)).acked, 1);
    assert_eq!(s.actor_save_current(&SaveKey::Player(id(1))).unwrap().0, 10);
    let next = s.select(SaveMode::Urgent, SaveBudget::default());
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].revision, 11);
    assert_eq!(next[0].urgency, SaveUrgency::Unload);
    let SaveValue::Player(p) = &next[0].value else {
        unreachable!()
    };
    assert_eq!(p.hunger, 12);
    assert_eq!(s.apply_completion(completion(next)).acked, 1);
    assert_eq!(s.save_stats(), SaveStats::default());
}
#[test]
fn mixed_forged_actor_preflight_cannot_ack_an_earlier_valid_chunk() {
    let mut s = enabled();
    retain_four(&mut s);
    acquire(&mut s, 1);
    for _ in 0..3 {
        assert!(s.select(SaveMode::Urgent, SaveBudget::default()).is_empty());
    }
    let actors = s.select(SaveMode::All, SaveBudget::default());
    assert_eq!(actors.len(), 4);
    let mut mixed = s.select(SaveMode::All, SaveBudget::default());
    assert_eq!(mixed.len(), 1);
    mixed.extend(actors);
    assert!(s.select(SaveMode::All, SaveBudget::default()).is_empty());
    let exact = mixed.clone();
    let SaveValue::Player(p) = &mut mixed[1].value else {
        unreachable!()
    };
    p.yaw = -0.0;
    let bad = s.apply_completion(completion(mixed));
    assert_eq!(bad.acked, 0);
    assert!(!bad.errors.is_empty());
    assert_eq!(s.live_chunk_facts(key(0)).unwrap().persisted_revision, 7);
    assert_eq!(s.actor_save_current(&SaveKey::Player(id(1))).unwrap().0, 9);
    assert_eq!(s.apply_completion(completion(exact)).acked, 5);
    assert_eq!(s.save_stats(), SaveStats::default());
}
#[test]
fn partial_mixed_ack_keeps_exact_retry_and_newer_companion_target() {
    let mut s = enabled();
    retain_four(&mut s);
    acquire(&mut s, 1);
    let mut targets = s.select(SaveMode::All, SaveBudget::default());
    targets.extend(s.select(SaveMode::All, SaveBudget::default()));
    let expected: Vec<_> = targets
        .iter()
        .filter(|t| t.key != SaveKey::Player(id(1)))
        .cloned()
        .collect();
    let mut newer = aggregates().remove(0);
    let SaveValue::Companions(c) = &mut newer else {
        unreachable!()
    };
    c.agent_namespace_id = mornlea_storage::PlayerId::from_bytes(id(51).bytes());
    s.observe_actor_save(newer, true, false).unwrap();
    let mut partial = completion(targets);
    partial
        .committed
        .retain(|(k, _)| *k == SaveKey::Player(id(1)));
    partial.error = Some(ServerError::Internal {
        invariant: "fixture admitted write failure",
    });
    let report = s.apply_completion(partial);
    assert_eq!(report.acked, 1);
    assert_eq!(report.retry, expected);
    assert_eq!(s.save_stats().in_flight, 4);
    assert!(s.select(SaveMode::All, SaveBudget::default()).is_empty());
    assert_eq!(s.apply_completion(completion(report.retry)).acked, 4);
    let next = s.select(SaveMode::All, SaveBudget::default());
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].key, SaveKey::Companions);
    assert_eq!(next[0].revision, 7);
    assert_eq!(s.apply_completion(completion(next)).acked, 1);
    assert_eq!(s.save_stats(), SaveStats::default());
}
#[test]
fn duplicate_and_ninth_chunk_completions_leave_all_owners_charged() {
    let mut s = enabled();
    retain_four(&mut s);
    acquire(&mut s, 9);
    let actors = s.select(SaveMode::All, SaveBudget::default());
    let chunks = s.select(SaveMode::All, SaveBudget::default());
    assert_eq!(chunks.len(), 8);
    let mut duplicate = completion(actors.clone());
    duplicate.committed.push(duplicate.committed[0].clone());
    assert_eq!(s.apply_completion(duplicate).acked, 0);
    let mut excess = chunks.clone();
    excess.push(
        s.capture_chunk_snapshot(key(8), SaveUrgency::Autosave)
            .unwrap(),
    );
    let rejected = s.apply_completion(completion(excess));
    assert_eq!(rejected.acked, 0);
    assert!(rejected.errors.iter().any(|e| matches!(
        e,
        ServerError::Capacity {
            resource: Resource::SaveChunks,
            limit: 8,
            observed: 9
        }
    )));
    assert_eq!(s.save_stats().in_flight, 12);
    assert_eq!(s.apply_completion(completion(chunks)).acked, 8);
    assert_eq!(s.apply_completion(completion(actors)).acked, 4);
    let last = s.select(SaveMode::All, SaveBudget::default());
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].key, SaveKey::Chunk(key(8)));
    assert_eq!(s.apply_completion(completion(last)).acked, 1);
}
#[test]
fn fresh_refusal_restores_forced_target_and_forgery_fences_before_release() {
    let mut s = enabled();
    s.retain_actor_save(player(1, 9), 9, true, true, true)
        .unwrap();
    s.observe_actor_save(player(1, 9), true, true).unwrap();
    let target = s.select(SaveMode::Urgent, SaveBudget::default()).remove(0);
    s.return_dirty(target.clone());
    assert_eq!(s.save_stats().in_flight, 0);
    let exact = s.select(SaveMode::Urgent, SaveBudget::default());
    assert_eq!(exact, vec![target.clone()]);
    let mut forged = target;
    forged.estimated_bytes += 1;
    s.return_dirty(forged);
    assert_eq!(s.phase(), ServerPhase::Closing);
    assert_eq!(s.save_stats().in_flight, 1);
    assert_eq!(s.apply_completion(completion(exact)).acked, 1);
}
#[test]
fn revision_overflow_fences_capture_but_retains_prior_exact_ack() {
    let mut s = enabled();
    s.retain_actor_save(player(1, 9), 9, true, true, true)
        .unwrap();
    let held = s.select(SaveMode::All, SaveBudget::default());
    s.retain_actor_save(player(2, u64::MAX), u64::MAX, true, true, true)
        .unwrap();
    assert!(s.select(SaveMode::All, SaveBudget::default()).is_empty());
    assert_eq!(s.phase(), ServerPhase::Closing);
    assert!(s.observe_actor_save(player(1, 9), true, true).is_err());
    assert_eq!(s.apply_completion(completion(held)).acked, 1);
    assert_eq!(s.save_stats().dirty, 1);
}

use mornlea_server::core::{chunk_driver::ChunkDriver, generation_worker::GenerationPool};
use mornlea_server::store::{
    disk::{DiskOptions, DiskStore},
    mailbox::StoreMailbox,
    scheduler::{AutosaveScheduler, SchedulerConfig},
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};
static ROOTS: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "actor-routing-{}-{}",
            std::process::id(),
            ROOTS.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn deadline() -> Deadline {
    Deadline::after(Instant::now(), Duration::from_secs(10)).unwrap()
}
struct RealClock;
impl Clock for RealClock {
    fn monotonic(&self) -> Instant {
        Instant::now()
    }
    fn unix_ms(&self) -> i64 {
        0
    }
}
fn options() -> DiskOptions {
    DiskOptions {
        region_handle_cap: 1,
        create: mornlea_storage::Metadata {
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
        },
    }
}
fn fixture_target(value: SaveValue) -> OwnedSnapshot {
    let (key, revision, bytes) = match &value {
        SaveValue::Player(p) => (
            SaveKey::Player(id(1)),
            p.revision,
            mornlea_storage::player_encoded_len(p).unwrap(),
        ),
        SaveValue::Companions(c) => (
            SaveKey::Companions,
            c.revision,
            mornlea_storage::companions_encoded_len(c).unwrap(),
        ),
        SaveValue::Hostiles(h) => (
            SaveKey::Hostiles,
            h.revision,
            mornlea_storage::hostile_mobs_encoded_len(h).unwrap(),
        ),
        SaveValue::Passives(p) => (
            SaveKey::Passives,
            p.revision,
            mornlea_storage::passive_mobs_encoded_len(p).unwrap(),
        ),
        _ => unreachable!(),
    };
    OwnedSnapshot::try_new(key, revision, bytes, SaveUrgency::Autosave, value).unwrap()
}
fn real_setup(root: &Root) -> (AuthorityState, AutosaveScheduler<DiskStore>) {
    let mut disk = DiskStore::open(&root.0, options()).unwrap();
    // These explicit source-shaped startup fixtures do not certify the loader or gameplay capture.
    let fixtures = std::iter::once(player(1, 9))
        .chain(aggregates())
        .map(fixture_target)
        .collect();
    let seeded = disk.write(
        SaveTicket::try_from_raw(1).unwrap(),
        SaveRequest {
            snapshots: fixtures,
        },
    );
    assert!(seeded.error.is_none());
    assert_eq!(seeded.committed.len(), 4);
    let mut store = AutosaveScheduler::try_new(
        SchedulerConfig::default(),
        StoreMailbox::try_new_background(
            StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
            disk,
        )
        .unwrap(),
    )
    .unwrap();
    let mut s = enabled();
    retain_four(&mut s);
    s.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
    let mut pool = GenerationPool::try_new(42, false, 1).unwrap();
    let mut driver = ChunkDriver::new();
    driver
        .start_load(&mut s, &mut store, key(0), deadline())
        .unwrap();
    drain_driver(&mut driver, &mut s, &mut store, &mut pool);
    s.advance_tick(TickBudget::full()).unwrap();
    driver.start_generation(&mut s, &mut pool, key(0)).unwrap();
    drain_driver(&mut driver, &mut s, &mut store, &mut pool);
    s.advance_tick(TickBudget::full()).unwrap();
    pool.close(deadline()).unwrap();
    assert_eq!(s.save_stats().dirty, 5);
    (s, store)
}
fn drain_driver(
    driver: &mut ChunkDriver,
    s: &mut AuthorityState,
    store: &mut AutosaveScheduler<DiskStore>,
    pool: &mut GenerationPool,
) {
    let until = deadline();
    loop {
        store.drive_workers();
        let report = driver.poll(s, store, pool);
        if report.retained == 0 {
            assert!(report.first_error.is_none());
            return;
        }
        assert!(!until.expired(Instant::now()));
        thread::yield_now();
    }
}
fn reopen(root: &Root, s: &AuthorityState) {
    let captured = s
        .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
        .unwrap();
    let SaveValue::ChunkView(view) = captured.value else {
        unreachable!()
    };
    let mut disk = DiskStore::open(&root.0, options()).unwrap();
    let LoadedValue::Player(p) = disk.load(SaveKey::Player(id(1))).unwrap() else {
        unreachable!()
    };
    assert_eq!(p.revision, 10);
    assert_eq!(p.hunger, 17);
    assert_eq!(p.current.position, [8.5, 65.0, 8.5]);
    let LoadedValue::Companions(c) = disk.load(SaveKey::Companions).unwrap() else {
        unreachable!()
    };
    assert_eq!(c.revision, 6);
    assert_eq!(c.agent_namespace_id.to_bytes(), id(50).bytes());
    assert!(c.records.is_empty());
    let LoadedValue::Hostiles(h) = disk.load(SaveKey::Hostiles).unwrap() else {
        unreachable!()
    };
    assert_eq!(h.revision, 6);
    assert!(h.records.is_empty());
    let LoadedValue::Passives(p) = disk.load(SaveKey::Passives).unwrap() else {
        unreachable!()
    };
    assert_eq!(p.revision, 6);
    assert!(p.records.is_empty());
    let LoadedValue::Chunk(c) = disk.load(SaveKey::Chunk(key(0))).unwrap() else {
        unreachable!()
    };
    assert_eq!(c.revision, view.revision());
    assert_eq!(c.chunk, view.materialize().chunk);
    disk.close().unwrap();
}
#[test]
fn actual_background_scheduler_autosaves_both_groups_and_reopens_all_families() {
    let root = Root::new();
    let (mut s, mut store) = real_setup(&root);
    let until = deadline();
    let mut dispatched = 0;
    let mut tick = 0;
    loop {
        store.drive_workers();
        let r = store
            .poll_tick(tick, SaveBudget::default(), &mut s)
            .unwrap();
        dispatched += r.autosave;
        if r.stats == SaveStats::default() {
            break;
        }
        assert!(!until.expired(Instant::now()));
        tick += 1;
        thread::yield_now();
    }
    assert_eq!(dispatched, 5);
    assert!(store.last_error().is_none());
    assert_eq!(store.tracked_submits(), 0);
    store.close(deadline()).unwrap();
    reopen(&root, &s);
}
#[test]
fn actual_closing_fresh_flush_persists_retained_actor_and_chunk_owners() {
    let root = Root::new();
    let (mut s, mut store) = real_setup(&root);
    s.begin_close();
    assert_eq!(s.freeze().save_keys.len(), 5);
    let report = store.flush(deadline(), &mut s, &RealClock).unwrap();
    // The scheduler reports successful cohorts: actors, chunk, then metadata.
    assert_eq!(report.durable, 3);
    assert_eq!(s.save_stats(), SaveStats::default());
    store.close(deadline()).unwrap();
    reopen(&root, &s);
}

fn forged_chunk_cannot_ack_valid_actor(alter: impl FnOnce(&mut OwnedSnapshot)) {
    let mut s = enabled();
    s.retain_actor_save(player(1, 9), 9, true, true, true)
        .unwrap();
    acquire(&mut s, 1);
    let mut exact = s.select(SaveMode::All, SaveBudget::default());
    exact.extend(s.select(SaveMode::All, SaveBudget::default()));
    assert_eq!(exact.len(), 2);
    let mut forged = exact.clone();
    alter(&mut forged[1]);
    let before = s.save_stats();
    let report = s.apply_completion(completion(forged));
    assert_eq!(report.acked, 0);
    assert!(!report.errors.is_empty());
    assert_eq!(s.save_stats(), before);
    assert_eq!(s.actor_save_current(&SaveKey::Player(id(1))).unwrap().0, 9);
    assert_eq!(s.live_chunk_facts(key(0)).unwrap().persisted_revision, 7);
    assert_eq!(s.apply_completion(completion(exact)).acked, 2);
    assert_eq!(s.save_stats(), SaveStats::default());
}
#[test]
fn mixed_forged_chunk_estimate_cannot_ack_an_earlier_valid_actor() {
    forged_chunk_cannot_ack_valid_actor(|s| s.estimated_bytes += 1);
}
#[test]
fn mixed_forged_chunk_urgency_cannot_ack_an_earlier_valid_actor() {
    forged_chunk_cannot_ack_valid_actor(|s| s.urgency = SaveUrgency::Unload);
}
#[test]
fn mixed_forged_chunk_body_cannot_ack_an_earlier_valid_actor() {
    forged_chunk_cannot_ack_valid_actor(|s| {
        let SaveValue::ChunkView(view) = &s.value else {
            unreachable!()
        };
        let mut changed = view.materialize();
        changed.chunk.sections[0].single = 1;
        s.value = SaveValue::Chunk(changed);
    });
}
