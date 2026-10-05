//! Literal startup and terminal controls are separate from actual disk/native consumer cases.
use mornlea_domain::{HostileId, PassiveId};
use mornlea_server::core::step::AuthoritativeFinalReducer;
use mornlea_server::{
    contracts::*,
    state::{AuthorityState, TickContext},
};
use mornlea_storage::{HostileMob, HostileMobs, PassiveMob, PassiveMobs};
fn hostile(id: u64) -> HostileMob {
    let mut target = [0; 16];
    target[0] = 1;
    target[6] = 0x40;
    target[8] = 0x80;
    HostileMob {
        id,
        dimension: 0,
        position: [8.5, 65.0, 8.5],
        velocity: [0.1, 0.2, 0.3],
        on_ground: false,
        yaw: 0.25,
        health: 15,
        attack_cooldown: 3,
        hurt_cooldown: 4,
        burn_cooldown: 5,
        has_target: true,
        player_id: mornlea_storage::PlayerId::from_bytes(target),
        next_repath_ticks: 900,
        distant_ticks: 7,
        kind: 1,
    }
}
fn passive(id: u64) -> PassiveMob {
    PassiveMob {
        id,
        dimension: 0,
        position: [31.5, 65.0, -0.5],
        velocity: [0.3, 0.2, 0.1],
        on_ground: true,
        yaw: 0.5,
        health: 17,
    }
}
fn loaded(h: usize, p: usize) -> (HostileMobs, PassiveMobs) {
    (
        HostileMobs {
            revision: 9,
            records: (1..=h as u64).rev().map(hostile).collect(),
        },
        PassiveMobs {
            revision: 5,
            records: (1..=p as u64).rev().map(passive).collect(),
        },
    )
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
fn enabled(h: usize, p: usize) -> AuthorityState {
    let mut state = base();
    let (h, p) = loaded(h, p);
    state.enable_mob_persistence(h, p).unwrap();
    state
}
#[test]
fn complete_startup_restores_sorted_rosters_and_exact_fields_with_reset_transients() {
    let state = enabled(64, 32);
    let residents = state.residents();
    assert_eq!(residents.actors.len(), 96);
    assert_eq!(residents.runtimes.len(), 96);
    for (index, actor) in residents.actors.iter().enumerate() {
        assert_eq!(actor.lifecycle, ActorLifecycle::Active);
        let runtime = &residents.runtimes[&actor.key];
        assert_eq!(runtime.path, None);
        assert!(!runtime.reset);
        assert_eq!(runtime.controls, None);
        assert_eq!(
            (
                runtime.attack_cooldown,
                runtime.hurt_cooldown,
                runtime.oxygen,
                runtime.peak_y
            ),
            (0, 0, 0, 0.0)
        );
        if index < 64 {
            assert_eq!(
                actor.key,
                ActorKey::Hostile(HostileId::try_new(index as u64 + 1).unwrap())
            );
            let ActorBody::Hostile(body) = &actor.body else {
                panic!("hostile")
            };
            assert_eq!(body, &hostile(index as u64 + 1));
            assert_eq!(actor.motion.position().get(), [8.5, 65.0, 8.5]);
            assert_eq!(actor.motion.velocity().get(), [0.1, 0.2, 0.3]);
            assert!(!actor.motion.on_ground());
            assert_eq!(actor.survival.health(), 15);
            assert_eq!(runtime.burn_cooldown, 5);
            assert_eq!(
                runtime.aux,
                ActorAux::Hostile {
                    distant_ticks: 7,
                    shoot_cooldown: 0,
                    fresh: false
                }
            );
        } else {
            assert_eq!(
                actor.key,
                ActorKey::Passive(PassiveId::try_new(index as u64 - 63).unwrap())
            );
            let ActorBody::Passive(body) = &actor.body else {
                panic!("passive")
            };
            assert_eq!(body, &passive(index as u64 - 63));
            assert_eq!(actor.survival.health(), 17);
            assert_eq!(runtime.burn_cooldown, 0);
            assert_eq!(
                runtime.aux,
                ActorAux::Passive {
                    home: mornlea_domain::BlockPos::new(31, 65, -1),
                    flee_ticks: 0,
                    flee_from: None,
                    graze_ticks: 0,
                    graze_at: None,
                    fresh: false
                }
            );
        }
    }
    assert_eq!(state.save_stats(), SaveStats::default());
    assert_eq!(state.actor_save_current(&SaveKey::Hostiles).unwrap().0, 9);
    assert_eq!(state.actor_save_current(&SaveKey::Passives).unwrap().0, 5);
}
#[test]
fn invalid_second_family_preserves_first_family_and_entire_startup_ownership() {
    let mut state = base();
    let (h, mut p) = loaded(1, 1);
    p.records[0].health = 0;
    assert!(state.enable_mob_persistence(h, p).is_err());
    assert!(!state.mob_persistence_enabled());
    assert!(state.residents().actors.is_empty());
    assert!(state.residents().runtimes.is_empty());
    assert!(state.actor_save_current(&SaveKey::Hostiles).is_none());
    assert!(state.actor_save_current(&SaveKey::Passives).is_none());
}
#[test]
fn duplicate_overlimit_and_nonempty_missing_inputs_refuse_without_partial_restore() {
    for mode in 0..5 {
        let mut state = base();
        let (mut h, mut p) = loaded(1, 1);
        match mode {
            0 => h.records.push(hostile(1)),
            1 => p.records.push(passive(1)),
            2 => h = loaded(65, 0).0,
            3 => p = loaded(0, 33).1,
            _ => h.revision = 0,
        }
        assert!(state.enable_mob_persistence(h, p).is_err());
        assert!(state.residents().actors.is_empty());
        assert_eq!(state.save_stats(), SaveStats::default());
        assert!(state.actor_save_current(&SaveKey::Hostiles).is_none());
    }
}
#[test]
fn startup_refuses_repeat_closing_and_missing_player_owner() {
    let mut state = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        42,
    )
    .unwrap();
    let (h, p) = loaded(1, 1);
    assert!(state.enable_mob_persistence(h, p).is_err());
    let mut state = enabled(1, 1);
    let (h, p) = loaded(2, 2);
    assert!(state.enable_mob_persistence(h, p).is_err());
    assert_eq!(state.residents().actors.len(), 2);
    let mut state = base();
    state.begin_close();
    let (h, p) = loaded(1, 1);
    assert!(state.enable_mob_persistence(h, p).is_err());
    assert!(state.residents().actors.is_empty());
}
// Prepared terminal ownership controls isolate physical retirement from death producers.
fn mark_dead(state: &mut AuthorityState) {
    let mut residents = state.residents();
    for actor in &mut residents.actors {
        actor.lifecycle = ActorLifecycle::Dead;
    }
    state.commit_residents(residents);
}
#[test]
fn settled_terminal_capture_erases_bodies_and_paths_and_saves_complete_empty_rosters() {
    let mut state = enabled(1, 1);
    mark_dead(&mut state);
    state.advance_tick(TickBudget::full()).unwrap();
    assert!(state.residents().actors.is_empty());
    assert!(state.residents().runtimes.is_empty());
    let targets = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].revision, 10);
    assert_eq!(targets[1].revision, 6);
    let SaveValue::Hostiles(h) = &targets[0].value else {
        panic!("hostiles")
    };
    let SaveValue::Passives(p) = &targets[1].value else {
        panic!("passives")
    };
    assert!(h.records.is_empty());
    assert!(p.records.is_empty());
}
#[test]
fn actual_final_reducer_captures_terminal_rosters_without_advancing_old_save_target() {
    let mut state = enabled(1, 1);
    let SaveValue::Hostiles(mut previous) = state
        .actor_save_current(&SaveKey::Hostiles)
        .unwrap()
        .1
        .clone()
    else {
        panic!("hostiles")
    };
    previous.records[0].attack_cooldown = 2;
    state
        .observe_actor_save(SaveValue::Hostiles(previous), true, false)
        .unwrap();
    let held = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(held.len(), 1);
    let old = held[0].clone();
    mark_dead(&mut state);
    state.begin_close();
    assert_eq!(state.run_final(&mut AuthoritativeFinalReducer).unwrap(), 0);
    assert!(state.residents().actors.is_empty());
    assert_eq!(state.save_stats().in_flight, 1);
    let submitted = vec![(old.key.clone(), old.revision)];
    assert_eq!(
        state
            .apply_completion(SaveCompletion {
                ticket: SaveTicket::try_from_raw(1).unwrap(),
                snapshots: held,
                committed: submitted.clone(),
                submitted,
                error: None
            })
            .acked,
        1
    );
    let (_, SaveValue::Hostiles(current)) = state.actor_save_current(&SaveKey::Hostiles).unwrap()
    else {
        panic!("current")
    };
    assert!(current.records.is_empty());
    assert_eq!(old.revision, 10);
    let SaveValue::Hostiles(old_value) = old.value else {
        panic!("old")
    };
    assert_eq!(old_value.records.len(), 1);
    let next = state.select(SaveMode::All, SaveBudget::default());
    assert!(
        next.iter()
            .any(|s| s.key == SaveKey::Hostiles && s.revision == 11)
    );
}
#[test]
fn absent_empty_aggregates_stay_quiet_through_ordinary_and_final_ticks() {
    let mut state = base();
    state
        .enable_mob_persistence(
            HostileMobs {
                revision: 0,
                records: vec![],
            },
            PassiveMobs {
                revision: 0,
                records: vec![],
            },
        )
        .unwrap();
    state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        state
            .select(SaveMode::All, SaveBudget::default())
            .is_empty()
    );
    state.begin_close();
    state.run_final(&mut AuthoritativeFinalReducer).unwrap();
    assert_eq!(state.save_stats(), SaveStats::default());
    assert!(
        state
            .select(SaveMode::All, SaveBudget::default())
            .is_empty()
    );
}
#[test]
fn compound_family_overflow_restores_all_earlier_actor_and_runtime_changes() {
    let mut state = enabled(63, 0);
    let before = state.residents();
    let mut a = before.actors[0].clone();
    a.key = ActorKey::Hostile(HostileId::try_new(64).unwrap());
    a.body = ActorBody::Hostile(hostile(64));
    let mut b = a.clone();
    b.key = ActorKey::Hostile(HostileId::try_new(65).unwrap());
    b.body = ActorBody::Hostile(hostile(65));
    let mut context = TickContext::restage(&mut state, TickBudget::full());
    assert_eq!(
        context.stage(RuleEffect::Compound(vec![
            RuleEffect::Actor(a),
            RuleEffect::Actor(b)
        ])),
        Err(RuleReject::ResourceFull(Resource::Actors))
    );
    drop(context);
    assert_eq!(state.residents().actors, before.actors);
    assert_eq!(state.residents().runtimes, before.runtimes);
}

#[test]
fn codec_valid_extreme_coordinates_restore_without_new_home_range_rejection() {
    for (coordinate, expected) in [
        (2147483520.0f32, 2147483520),
        (2147483648.0, i32::MAX),
        (-2147483648.0, i32::MIN),
        (-2147483904.0, i32::MIN),
        (f32::MAX, i32::MAX),
        (-f32::MAX, i32::MIN),
    ] {
        let mut state = base();
        let (h, mut p) = loaded(0, 1);
        p.records[0].position = [coordinate, 65.0, coordinate];
        state.enable_mob_persistence(h, p).unwrap();
        let residents = state.residents();
        let runtime = &residents.runtimes[&ActorKey::Passive(PassiveId::try_new(1).unwrap())];
        let ActorAux::Passive { home, .. } = runtime.aux else {
            panic!("passive home")
        };
        assert_eq!(home, mornlea_domain::BlockPos::new(expected, 65, expected));
    }
}

#[test]
fn oversize_retained_fixture_fails_before_reduction_or_new_save_capture() {
    let mut state = enabled(1, 0);
    let mut residents = state.residents();
    let record = residents.actors[0].clone();
    residents.actors = vec![record; 111];
    state.commit_residents(residents);
    assert_eq!(
        state.advance_tick(TickBudget::full()),
        Err(ServerError::Capacity {
            resource: Resource::Actors,
            limit: 110,
            observed: 111
        })
    );
    assert_eq!(state.phase(), ServerPhase::Closing);
    assert_eq!(state.residents().actors.len(), 111);
    assert_eq!(state.actor_save_current(&SaveKey::Hostiles).unwrap().0, 9);
    assert!(
        state
            .select(SaveMode::All, SaveBudget::default())
            .is_empty()
    );
}

#[test]
fn repeated_prepared_terminal_cycles_release_every_mob_runtime() {
    let mut state = enabled(1, 1);
    let templates = state.residents();
    for _ in 0..100 {
        mark_dead(&mut state);
        state.advance_tick(TickBudget::full()).unwrap();
        assert!(state.residents().actors.is_empty());
        assert!(state.residents().runtimes.is_empty());
        let mut context = TickContext::restage(&mut state, TickBudget::full());
        for actor in &templates.actors {
            context
                .stage(RuleEffect::Compound(vec![
                    RuleEffect::Actor(actor.clone()),
                    RuleEffect::Runtime(templates.runtimes[&actor.key].clone()),
                ]))
                .unwrap();
        }
        let residents = context.resident_snapshot();
        drop(context);
        state.commit_residents(residents);
        assert_eq!(state.residents().actors.len(), 2);
        assert_eq!(state.residents().runtimes.len(), 2);
    }
}

#[test]
fn passive_capacity_and_invalid_lifecycle_preserve_existing_ownership() {
    let mut state = enabled(0, 32);
    let before = state.residents();
    let mut incoming = before.actors[0].clone();
    incoming.key = ActorKey::Passive(PassiveId::try_new(33).unwrap());
    incoming.body = ActorBody::Passive(passive(33));
    let mut context = TickContext::restage(&mut state, TickBudget::full());
    assert_eq!(
        context.stage(RuleEffect::Actor(incoming.clone())),
        Err(RuleReject::ResourceFull(Resource::Actors))
    );
    incoming.lifecycle = ActorLifecycle::Pending;
    assert_eq!(
        context.stage(RuleEffect::Actor(incoming)),
        Err(RuleReject::StaleObservation)
    );
    drop(context);
    assert_eq!(state.residents().actors, before.actors);
}

mod actual_disk {
    use super::*;
    use mornlea_domain::{ChunkPos, Dimension, Event, PassiveDespawnReason, PassiveDespawnRecord};
    use mornlea_protocol::{LoginStart, admit_login};
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
    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "source-mob-persistence-{}-{}",
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
    struct RealClock;
    impl Clock for RealClock {
        fn monotonic(&self) -> Instant {
            Instant::now()
        }
        fn unix_ms(&self) -> i64 {
            0
        }
    }
    fn deadline() -> Deadline {
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
    fn flat() -> mornlea_storage::Chunk {
        mornlea_storage::Chunk {
            sections: (0..24)
                .map(|index| mornlea_storage::ContainerSnapshot {
                    kind: mornlea_storage::StorageKind::Single,
                    bits: 0,
                    single: if index < 9 { 2 } else { 0 },
                    palette: vec![],
                    packed: vec![],
                })
                .collect(),
            drops: vec![Default::default(); 32],
            furnaces: vec![Default::default(); 32],
            chests: vec![Default::default(); 16],
        }
    }
    fn target(value: SaveValue) -> OwnedSnapshot {
        let (key, revision, bytes) = match &value {
            SaveValue::Hostiles(body) => (
                SaveKey::Hostiles,
                body.revision,
                mornlea_storage::hostile_mobs_encoded_len(body).unwrap(),
            ),
            SaveValue::Passives(body) => (
                SaveKey::Passives,
                body.revision,
                mornlea_storage::passive_mobs_encoded_len(body).unwrap(),
            ),
            SaveValue::Chunk(body) => (SaveKey::Chunk(chunk_key()), body.revision, 1),
            _ => panic!("fixture target"),
        };
        OwnedSnapshot::try_new(key, revision, bytes, SaveUrgency::Autosave, value).unwrap()
    }
    fn stored_player() -> mornlea_storage::StoredPlayer {
        let mut bytes = [0; 16];
        bytes[0] = 1;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        mornlea_storage::StoredPlayer {
            player_id: mornlea_storage::PlayerId::from_bytes(bytes),
            revision: 9,
            display_name: "Ada".into(),
            current: mornlea_storage::PlayerLocation {
                dimension: 0,
                position: [8.5, 80.0, 8.5],
            },
            yaw: 0.0,
            pitch: 0.0,
            safe: None,
            inventory: Default::default(),
            health: 20,
            hunger: 20,
            saturation_milli: 5000,
            exhaustion_milli: 0,
            respawn_present: false,
            respawn_position: [0.0; 3],
            respawn_dimension: 0,
            armor: [Default::default(); 4],
            needs_rewrite: false,
        }
    }
    fn setup(
        root: &Root,
        burn: u8,
    ) -> (
        AuthorityState,
        AutosaveScheduler<DiskStore>,
        SessionKey,
        TickPublication,
    ) {
        let mut disk = DiskStore::open(&root.0, options()).unwrap();
        let mut h = hostile(1);
        h.kind = 0;
        h.health = 1;
        h.burn_cooldown = burn;
        h.position = [2.5, 80.0, 2.5];
        h.velocity = [0.0; 3];
        h.on_ground = true;
        h.has_target = false;
        h.player_id = mornlea_storage::PlayerId::from_bytes([0; 16]);
        let mut p = passive(1);
        p.position = [3.5, 80.0, 3.5];
        p.velocity = [0.0; 3];
        p.on_ground = true;
        let snapshots = vec![
            target(SaveValue::Hostiles(mornlea_storage::HostileMobsSave {
                revision: 9,
                records: vec![h],
            })),
            target(SaveValue::Passives(mornlea_storage::PassiveMobsSave {
                revision: 5,
                records: vec![p],
            })),
            target(SaveValue::Chunk(mornlea_storage::ChunkSave {
                key: mornlea_storage::ChunkKey {
                    dimension: 0,
                    x: 0,
                    z: 0,
                },
                revision: 9,
                chunk: flat(),
            })),
        ];
        let completion = disk.write(
            SaveTicket::try_from_raw(1).unwrap(),
            SaveRequest { snapshots },
        );
        assert_eq!(completion.error, None);
        assert_eq!(completion.committed.len(), 3);
        let LoadedValue::Hostiles(h) = disk.load(SaveKey::Hostiles).unwrap() else {
            panic!("loaded hostiles")
        };
        let LoadedValue::Passives(p) = disk.load(SaveKey::Passives).unwrap() else {
            panic!("loaded passives")
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
        state.enable_mob_persistence(h, p).unwrap();
        let player = stored_player();
        let id = mornlea_domain::PlayerId::try_from_bytes(player.player_id.to_bytes()).unwrap();
        let start = LoginStart::new(id, "Ada", 8).unwrap();
        let login =
            admit_login(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap()).unwrap();
        let session = state.prepare(login, TransportKind::Memory).unwrap();
        assert!(!state.prepare_player_cache(session).unwrap());
        state.install(session, Some(player)).unwrap();
        state.activate(session).unwrap();
        let mut store = AutosaveScheduler::try_new(
            SchedulerConfig::default(),
            StoreMailbox::try_new_background(
                StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
                disk,
            )
            .unwrap(),
        )
        .unwrap();
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
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        pool.close(deadline()).unwrap();
        assert!(
            state.residents().actors.iter().any(
                |a| a.key == ActorKey::Player(session) && a.lifecycle == ActorLifecycle::Active
            )
        );
        (state, store, session, publication)
    }
    fn autosave(state: &mut AuthorityState, store: &mut AutosaveScheduler<DiskStore>) {
        let until = deadline();
        let mut tick = 0;
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
        assert_eq!(store.tracked_submits(), 0);
    }
    fn drops(state: &mut AuthorityState) -> Vec<DropRecord> {
        let context = TickContext::restage(state, TickBudget::full());
        context.read().drops(chunk_key()).to_vec()
    }
    #[test]
    fn actual_disk_restore_native_tick_autosave_and_reopen_every_live_field() {
        let root = Root::new();
        let (mut state, mut store, _, _) = setup(&root, 20);
        let (_, SaveValue::Hostiles(expected_h)) =
            state.actor_save_current(&SaveKey::Hostiles).unwrap()
        else {
            panic!("hostiles")
        };
        let mut expected_h = expected_h.clone();
        let (_, SaveValue::Passives(expected_p)) =
            state.actor_save_current(&SaveKey::Passives).unwrap()
        else {
            panic!("passives")
        };
        let mut expected_p = expected_p.clone();
        assert_eq!(expected_h.records.len(), 1);
        assert_eq!(expected_h.records[0].burn_cooldown, 19);
        assert_eq!(expected_p.records.len(), 1);
        autosave(&mut state, &mut store);
        expected_h.revision = state.actor_save_current(&SaveKey::Hostiles).unwrap().0;
        expected_p.revision = state.actor_save_current(&SaveKey::Passives).unwrap().0;
        assert_eq!(expected_h.revision, 10);
        assert_eq!(expected_p.revision, 6);
        store.close(deadline()).unwrap();
        let mut disk = DiskStore::open(&root.0, options()).unwrap();
        assert_eq!(
            disk.load(SaveKey::Hostiles).unwrap(),
            LoadedValue::Hostiles(HostileMobs {
                revision: expected_h.revision,
                records: expected_h.records
            })
        );
        assert_eq!(
            disk.load(SaveKey::Passives).unwrap(),
            LoadedValue::Passives(PassiveMobs {
                revision: expected_p.revision,
                records: expected_p.records
            })
        );
        disk.close().unwrap();
    }
    #[test]
    fn actual_burn_and_passive_death_publish_once_retire_and_final_flush_empty_rosters() {
        let root = Root::new();
        let (mut state, mut store, session, first) = setup(&root, 2);
        assert!(
            first
                .events
                .iter()
                .any(|e| matches!(e.event(), Event::PassiveSpawn(_)))
        );
        let mut residents = state.residents();
        let passive = residents
            .actors
            .iter_mut()
            .find(|a| matches!(a.key, ActorKey::Passive(_)))
            .unwrap();
        // A prepared lethal damage input exercises actual ordinary passive death settlement.
        passive.survival =
            mornlea_domain::SurvivalState::try_new(mornlea_domain::SurvivalStateParts {
                health: 0,
                oxygen: 300,
                hunger: 20,
                saturation_zero: false,
                armor_points: 0,
            })
            .unwrap();
        let ActorBody::Passive(body) = &mut passive.body else {
            panic!("passive")
        };
        body.health = 0;
        state.commit_residents(residents);
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        assert!(publication.events.iter().any(|e|matches!(e.event(),Event::PassiveDespawn(batch)
            if batch.despawns()==[PassiveDespawnRecord::new(PassiveId::try_new(1).unwrap(),PassiveDespawnReason::Died)])));
        let residents = state.residents();
        assert_eq!(residents.actors.len(), 1);
        assert_eq!(residents.actors[0].key, ActorKey::Player(session));
        let SaveValue::Player(captured) = state
            .actor_save_current(&SaveKey::Player(
                mornlea_domain::PlayerId::try_from_bytes(stored_player().player_id.to_bytes())
                    .unwrap(),
            ))
            .unwrap()
            .1
        else {
            panic!("indexed player")
        };
        assert_eq!(captured.current.position, [8.5, 80.0, 8.5]);
        assert!(
            !residents
                .runtimes
                .keys()
                .any(|key| matches!(key, ActorKey::Hostile(_) | ActorKey::Passive(_)))
        );
        let settled_drops = drops(&mut state);
        assert!(
            settled_drops
                .iter()
                .any(|d| d.stack.item == 45 && d.stack.count == 1)
        );
        assert!(
            settled_drops
                .iter()
                .any(|d| d.stack.item == 53 && d.stack.count == 1)
        );
        let third = state.advance_tick(TickBudget::full()).unwrap();
        assert!(!third.events.iter().any(|e| matches!(
            e.event(),
            Event::HostileDespawn(_) | Event::PassiveDespawn(_)
        )));
        assert_eq!(drops(&mut state).len(), settled_drops.len());
        state.begin_close();
        state.run_final(&mut AuthoritativeFinalReducer).unwrap();
        store.flush(deadline(), &mut state, &RealClock).unwrap();
        store.close(deadline()).unwrap();
        let mut disk = DiskStore::open(&root.0, options()).unwrap();
        let LoadedValue::Hostiles(h) = disk.load(SaveKey::Hostiles).unwrap() else {
            panic!("hostiles")
        };
        let LoadedValue::Passives(p) = disk.load(SaveKey::Passives).unwrap() else {
            panic!("passives")
        };
        assert!(h.records.is_empty());
        assert!(p.records.is_empty());
        assert_eq!(h.revision, 10);
        assert_eq!(p.revision, 6);
        disk.close().unwrap();
    }
    #[test]
    fn actual_quiet_removal_preserves_independent_shard_without_duplicate_loot() {
        let root = Root::new();
        let (mut state, mut store, _, _) = setup(&root, 20);
        let mut residents = state.residents();
        let hostile = ActorKey::Hostile(HostileId::try_new(1).unwrap());
        let passive = ActorKey::Passive(PassiveId::try_new(1).unwrap());
        for actor in &mut residents.actors {
            if actor.key == hostile || actor.key == passive {
                let position = if actor.key == hostile {
                    [1000.5, 80.0, 1000.5]
                } else {
                    [3.5, -65.0, 3.5]
                };
                actor.motion = mornlea_domain::MotionState::new(mornlea_domain::MotionStateParts {
                    position: mornlea_domain::FiniteVec3::try_new(position).unwrap(),
                    velocity: mornlea_domain::FiniteVec3::try_new([0.0; 3]).unwrap(),
                    on_ground: false,
                });
                match &mut actor.body {
                    ActorBody::Hostile(body) => {
                        body.position = position;
                        body.velocity = [0.0; 3];
                    }
                    ActorBody::Passive(body) => {
                        body.position = position;
                        body.velocity = [0.0; 3];
                    }
                    _ => unreachable!(),
                }
            }
        }
        let ActorAux::Hostile { distant_ticks, .. } =
            &mut residents.runtimes.get_mut(&hostile).unwrap().aux
        else {
            panic!("hostile runtime")
        };
        *distant_ticks = 599;
        let projectile = ProjectileRecord {
            id: mornlea_domain::ProjectileId::try_new(41).unwrap(),
            owner: hostile,
            dimension: Dimension::OVERWORLD,
            position: mornlea_domain::FiniteVec3::try_new([8.5, 85.0, 8.5]).unwrap(),
            velocity: mornlea_domain::FiniteVec3::try_new([1.0, 0.0, 0.0]).unwrap(),
            kind: mornlea_domain::ProjectileKind::Shard,
            damage: 3,
            age: 0,
        };
        residents.projectiles.push(projectile.clone());
        state.commit_residents(residents);
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        assert!(publication.events.iter().any(|e|matches!(e.event(),Event::PassiveDespawn(batch)
            if batch.despawns()==[PassiveDespawnRecord::new(PassiveId::try_new(1).unwrap(),PassiveDespawnReason::Vanished)])));
        assert!(
            !state
                .residents()
                .actors
                .iter()
                .any(|a| a.key == hostile || a.key == passive)
        );
        assert!(drops(&mut state).is_empty());
        let first = state.residents().projectiles[0].clone();
        assert_eq!(first.owner, hostile);
        assert_eq!(first.age, 1);
        assert_ne!(first.position, projectile.position);
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        assert!(!publication.events.iter().any(|e| matches!(
            e.event(),
            Event::HostileDespawn(_) | Event::PassiveDespawn(_)
        )));
        assert_eq!(state.residents().projectiles[0].age, 2);
        assert_ne!(state.residents().projectiles[0].position, first.position);
        assert!(drops(&mut state).is_empty());
        store.close(deadline()).unwrap();
    }
    #[test]
    fn actual_missing_disk_families_stay_fileless_after_ordinary_final_and_flush() {
        let root = Root::new();
        let mut disk = DiskStore::open(&root.0, options()).unwrap();
        for key in [SaveKey::Hostiles, SaveKey::Passives] {
            assert_eq!(
                disk.load(key),
                Err(ServerError::Io {
                    operation: Operation::Load,
                    kind: std::io::ErrorKind::NotFound
                })
            );
        }
        let mut state = base();
        state
            .enable_mob_persistence(
                HostileMobs {
                    revision: 0,
                    records: vec![],
                },
                PassiveMobs {
                    revision: 0,
                    records: vec![],
                },
            )
            .unwrap();
        let mut store = AutosaveScheduler::try_new(
            SchedulerConfig::default(),
            StoreMailbox::try_new_background(
                StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
                disk,
            )
            .unwrap(),
        )
        .unwrap();
        state.advance_tick(TickBudget::full()).unwrap();
        state.begin_close();
        state.run_final(&mut AuthoritativeFinalReducer).unwrap();
        store.flush(deadline(), &mut state, &RealClock).unwrap();
        store.close(deadline()).unwrap();
        assert!(!root.0.join("hostile_mobs.bin").exists());
        assert!(!root.0.join("passive_mobs.bin").exists());
    }
    #[test]
    fn actual_corrupt_future_and_io_aggregate_loads_are_hard_failures() {
        for family in [SaveKey::Hostiles, SaveKey::Passives] {
            for mode in 0..3 {
                let root = Root::new();
                let mut disk = DiskStore::open(&root.0, options()).unwrap();
                let (name, magic) = if family == SaveKey::Hostiles {
                    ("hostile_mobs.bin", b"MHST")
                } else {
                    ("passive_mobs.bin", b"PMST")
                };
                if mode == 2 {
                    fs::create_dir(root.0.join(name)).unwrap();
                } else {
                    let bytes = if mode == 0 {
                        vec![0; 12]
                    } else {
                        let mut bytes = magic.to_vec();
                        bytes.extend_from_slice(&99u32.to_le_bytes());
                        bytes.extend_from_slice(&1u32.to_le_bytes());
                        bytes
                    };
                    fs::write(root.0.join(name), bytes).unwrap();
                }
                let failure = disk.load(family.clone()).unwrap_err();
                match (mode, failure) {
                    (
                        0,
                        ServerError::Storage {
                            kind: StorageFailure::Corrupt,
                            ..
                        },
                    )
                    | (
                        1,
                        ServerError::Storage {
                            kind: StorageFailure::FutureVersion,
                            ..
                        },
                    )
                    | (2, ServerError::Io { .. }) => {}
                    _ => panic!("unexpected aggregate failure: {failure:?}"),
                }
                let state = base();
                assert!(!state.mob_persistence_enabled());
                assert!(state.residents().actors.is_empty());
                disk.close().unwrap();
            }
        }
    }
    #[test]
    fn actual_disk_failure_retries_exact_old_roster_then_flushes_newer_native_death() {
        use mornlea_server::store::io::{DiskIo, IoPhase};
        use std::{
            io::{self, Write},
            sync::{Arc, Mutex, atomic::AtomicBool},
        };
        struct FailOnce {
            armed: Arc<AtomicBool>,
            attempts: Arc<Mutex<Vec<Vec<u8>>>>,
            hostile_payload: bool,
        }
        impl DiskIo for FailOnce {
            fn write(&mut self, file: &mut fs::File, bytes: &[u8]) -> io::Result<usize> {
                self.hostile_payload = bytes.starts_with(b"MHST");
                if bytes.starts_with(b"MHST") {
                    self.attempts.lock().unwrap().push(bytes.to_vec());
                }
                file.write(bytes)
            }
            fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> io::Result<()> {
                if point == IoFaultPoint::TempSync
                    && phase == IoPhase::Before
                    && self.hostile_payload
                    && self.armed.swap(false, Ordering::AcqRel)
                {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                Ok(())
            }
        }
        let root = Root::new();
        let (mut state, mut store, _, _) = setup(&root, 20);
        autosave(&mut state, &mut store);
        store.close(deadline()).unwrap();
        assert_eq!(state.actor_save_current(&SaveKey::Hostiles).unwrap().0, 10);
        state.advance_tick(TickBudget::full()).unwrap();
        let (_, SaveValue::Hostiles(old)) = state.actor_save_current(&SaveKey::Hostiles).unwrap()
        else {
            panic!("old")
        };
        let mut old = old.clone();
        old.revision = 11;
        let armed = Arc::new(AtomicBool::new(true));
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let disk = DiskStore::with_io(
            &root.0,
            options(),
            Box::new({
                let armed = armed.clone();
                let attempts = attempts.clone();
                move || {
                    Box::new(FailOnce {
                        armed: armed.clone(),
                        attempts: attempts.clone(),
                        hostile_payload: false,
                    })
                }
            }),
        )
        .unwrap();
        let mut store = AutosaveScheduler::try_new(
            SchedulerConfig::default(),
            StoreMailbox::try_new_background(
                StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
                disk,
            )
            .unwrap(),
        )
        .unwrap();
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
                kind: io::ErrorKind::PermissionDenied
            })
        );
        assert_eq!(
            attempts.lock().unwrap()[0],
            mornlea_storage::encode_hostile_mobs(&old).unwrap()
        );
        let mut residents = state.residents();
        let key = ActorKey::Hostile(HostileId::try_new(1).unwrap());
        residents.runtimes.get_mut(&key).unwrap().burn_cooldown = 1;
        let ActorBody::Hostile(body) = &mut residents
            .actors
            .iter_mut()
            .find(|a| a.key == key)
            .unwrap()
            .body
        else {
            panic!("hostile")
        };
        body.burn_cooldown = 1;
        state.commit_residents(residents);
        state.advance_tick(TickBudget::full()).unwrap();
        assert!(
            !state
                .residents()
                .actors
                .iter()
                .any(|actor| actor.key == key)
        );
        let (_, SaveValue::Hostiles(current)) =
            state.actor_save_current(&SaveKey::Hostiles).unwrap()
        else {
            panic!("newer")
        };
        assert!(current.records.is_empty());
        assert_eq!(state.actor_save_current(&SaveKey::Hostiles).unwrap().0, 10);
        state.begin_close();
        state.run_final(&mut AuthoritativeFinalReducer).unwrap();
        store.flush(deadline(), &mut state, &RealClock).unwrap();
        store.close(deadline()).unwrap();
        let attempts = attempts.lock().unwrap();
        assert_eq!(attempts.len(), 3);
        assert_eq!(attempts[0], attempts[1]);
        let mut empty = old;
        empty.revision = 12;
        empty.records.clear();
        assert_eq!(
            attempts[2],
            mornlea_storage::encode_hostile_mobs(&empty).unwrap()
        );
        let mut disk = DiskStore::open(&root.0, options()).unwrap();
        assert_eq!(
            disk.load(SaveKey::Hostiles).unwrap(),
            LoadedValue::Hostiles(HostileMobs {
                revision: 12,
                records: vec![]
            })
        );
        disk.close().unwrap();
    }
}
