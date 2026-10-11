//! Source lifecycle controls use literal loaded fixtures; physical consumer cases are separate.
use mornlea_domain::PlayerId;
use mornlea_protocol::{AdmittedLogin, LoginStart, admit_login};
use mornlea_server::core::actor_save::ActorSaveLedger;
use mornlea_server::transport::{common::TransportAuthority, live::LoginDriver};
use mornlea_server::{contracts::*, state::AuthorityState};
use mornlea_storage::{Inventory, PlayerLocation, PlayerSave, StoredPlayer};
use std::time::{Duration, Instant};
fn id(tag: u8) -> PlayerId {
    let mut b = [0; 16];
    b[0] = tag;
    b[6] = 0x40;
    b[8] = 0x80;
    PlayerId::try_from_bytes(b).unwrap()
}
fn login(tag: u8, name: &str) -> AdmittedLogin {
    admit_login(
        LoginStart::decode_inbound(&LoginStart::new(id(tag), name, 8).unwrap().encode().unwrap())
            .unwrap(),
    )
    .unwrap()
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
    s.enable_source_player_restoration(1).unwrap();
    s.enable_live_chunks().unwrap();
    s.enable_actor_saves().unwrap();
    s.enable_player_persistence().unwrap();
    s
}
fn save(tag: u8) -> PlayerSave {
    PlayerSave {
        player_id: mornlea_storage::PlayerId::from_bytes(id(tag).bytes()),
        revision: 9,
        display_name: "Ada".into(),
        current: PlayerLocation {
            dimension: 0,
            position: [8.5, 65.0, 8.5],
        },
        yaw: 0.1,
        pitch: 0.2,
        safe: Some(PlayerLocation {
            dimension: 0,
            position: [7.5, 65.0, 8.5],
        }),
        inventory: Inventory::default(),
        health: 15,
        hunger: 17,
        saturation_milli: 9000,
        exhaustion_milli: 250,
        respawn_present: false,
        respawn_position: [0.; 3],
        respawn_dimension: 0,
        armor: [Default::default(); 4],
    }
}
fn stored(p: PlayerSave, rewrite: bool) -> StoredPlayer {
    StoredPlayer {
        player_id: p.player_id,
        revision: p.revision,
        display_name: p.display_name,
        current: p.current,
        yaw: p.yaw,
        pitch: p.pitch,
        safe: p.safe,
        inventory: p.inventory,
        health: p.health,
        hunger: p.hunger,
        saturation_milli: p.saturation_milli,
        exhaustion_milli: p.exhaustion_milli,
        respawn_present: p.respawn_present,
        respawn_position: p.respawn_position,
        respawn_dimension: p.respawn_dimension,
        armor: p.armor,
        needs_rewrite: rewrite,
    }
}
fn prepare(
    s: &mut AuthorityState,
    tag: u8,
    name: &str,
    loaded: Option<StoredPlayer>,
) -> SessionKey {
    let k = s.prepare(login(tag, name), TransportKind::Memory).unwrap();
    assert!(!s.prepare_player_cache(k).unwrap());
    s.install(k, loaded).unwrap();
    k
}
fn current(s: &AuthorityState, tag: u8) -> &PlayerSave {
    let SaveValue::Player(p) = s.actor_save_current(&SaveKey::Player(id(tag))).unwrap().1 else {
        unreachable!()
    };
    p
}
#[test]
fn source_persistence_enable_requires_empty_healthy_source_and_actor_owners() {
    let mut s = authority();
    assert!(!s.player_persistence_enabled());
    assert!(s.enable_player_persistence().is_err());
    s.enable_live_chunks().unwrap();
    s.enable_actor_saves().unwrap();
    assert!(s.enable_player_persistence().is_err());
    s.enable_source_player_restoration(1).unwrap();
    s.enable_player_persistence().unwrap();
    s.enable_player_persistence().unwrap();
    assert!(s.player_persistence_enabled());
    s.begin_close();
    assert!(s.enable_player_persistence().is_err());
    let mut s = authority();
    s.enable_source_player_restoration(1).unwrap();
    s.enable_live_chunks().unwrap();
    s.enable_actor_saves().unwrap();
    s.retain_actor_save(SaveValue::Player(save(1)), 9, false, true, false)
        .unwrap();
    assert!(s.enable_player_persistence().is_err());
}
#[test]
fn guarded_release_preserves_pinned_dirty_and_held_players() {
    let mut l = ActorSaveLedger::default();
    l.retain(SaveValue::Player(save(1)), 9, false, true, true)
        .unwrap();
    assert!(!l.release_player(id(1)).unwrap());
    l.set_pinned(&SaveKey::Player(id(1)), false).unwrap();
    assert!(l.release_player(id(1)).unwrap());
    assert!(l.current(&SaveKey::Player(id(1))).is_none());
    l.retain(SaveValue::Player(save(1)), 9, true, true, false)
        .unwrap();
    assert!(!l.release_player(id(1)).unwrap());
    let selected = l.select(SaveMode::All, SaveBudget::default()).unwrap();
    assert!(!l.release_player(id(1)).unwrap());
    l.saved(&selected[0]).unwrap();
    assert!(l.release_player(id(1)).unwrap());
    assert!(!l.release_player(id(1)).unwrap());
    let mut missing = save(2);
    missing.revision = 1;
    l.retain(SaveValue::Player(missing), 0, false, false, false)
        .unwrap();
    assert!(l.release_player(id(2)).unwrap());
}
#[test]
fn missing_confirmation_saves_source_fallback_without_pending_pose_capture() {
    let mut s = enabled();
    let session = prepare(&mut s, 1, "Ada", None);
    assert!(s.select(SaveMode::All, SaveBudget::default()).is_empty());
    s.activate(session).unwrap();
    s.advance_tick(TickBudget::full()).unwrap();
    let selected = s.select(SaveMode::All, SaveBudget::default());
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].revision, 1);
    let SaveValue::Player(p) = &selected[0].value else {
        unreachable!()
    };
    assert_eq!(p.current.position, [0.5, 321.0, 0.5]);
    assert_eq!(p.health, 0);
    assert_eq!(
        (p.hunger, p.saturation_milli, p.exhaustion_milli),
        (20, 5000, 0)
    );
    assert_eq!(p.inventory, Inventory::default());
    assert_eq!(p.armor, [Default::default(); 4]);
}
#[test]
fn eighteen_unconfirmed_aborts_leave_no_cache_or_save_residue() {
    let mut s = enabled();
    for _ in 0..18 {
        let session = prepare(&mut s, 1, "Ada", None);
        s.retire(session, CloseReason::PeerGone).unwrap();
        assert!(s.actor_save_current(&SaveKey::Player(id(1))).is_none());
        assert_eq!(s.save_stats(), SaveStats::default());
    }
}
#[test]
fn never_spawned_loaded_pending_preserves_original_pose_and_safe() {
    let mut s = enabled();
    let session = prepare(&mut s, 1, "Ada", Some(stored(save(1), true)));
    s.activate(session).unwrap();
    s.advance_tick(TickBudget::full()).unwrap();
    let p = current(&s, 1);
    assert_eq!(p.current, save(1).current);
    assert_eq!(p.safe, save(1).safe);
    assert_eq!(p.health, 15);
    s.retire(session, CloseReason::PeerGone).unwrap();
    assert_eq!(s.residents().actors.len(), 0);
    assert_eq!(current(&s, 1).current, save(1).current);
    assert_eq!(
        s.select(SaveMode::All, SaveBudget::default())[0].revision,
        10
    );
}
#[test]
fn cached_rename_abort_preserves_old_name_and_confirm_changes_it_once() {
    let mut s = enabled();
    let first = prepare(&mut s, 1, "Ada", Some(stored(save(1), true)));
    s.activate(first).unwrap();
    s.retire(first, CloseReason::PeerGone).unwrap();
    let next = s.prepare(login(1, "Grace"), TransportKind::Memory).unwrap();
    assert!(s.prepare_player_cache(next).unwrap());
    assert_eq!(current(&s, 1).display_name, "Ada");
    s.retire(next, CloseReason::PeerGone).unwrap();
    assert_eq!(current(&s, 1).display_name, "Ada");
    let next = s.prepare(login(1, "Grace"), TransportKind::Memory).unwrap();
    assert!(s.prepare_player_cache(next).unwrap());
    s.activate(next).unwrap();
    assert_eq!(current(&s, 1).display_name, "Grace");
    s.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(current(&s, 1).display_name, "Grace");
}
#[test]
fn sixteen_offline_dirty_players_refuse_a_new_load_placeholder() {
    let mut s = enabled();
    for tag in 1..=16 {
        let k = prepare(&mut s, tag, "Ada", None);
        s.activate(k).unwrap();
        s.retire(k, CloseReason::PeerGone).unwrap();
        assert!(s.residents().actors.is_empty());
    }
    let next = s.prepare(login(17, "Ada"), TransportKind::Memory).unwrap();
    assert_eq!(
        s.prepare_player_cache(next),
        Err(ServerError::Capacity {
            resource: Resource::Players,
            limit: 16,
            observed: 17
        })
    );
    s.retire(next, CloseReason::PeerGone).unwrap();
    assert_eq!(s.save_stats().dirty, 16);
}
struct Loads {
    starts: usize,
    cancels: usize,
    outcome: Option<LoadPoll>,
    refuse: bool,
}
impl PlayerLoadPort for Loads {
    fn start(&mut self, _: PlayerId, _: Deadline) -> Result<LoginTicket, ServerError> {
        self.starts += 1;
        if self.refuse {
            return Err(ServerError::Internal {
                invariant: "fixture forbids cache-hit disk load",
            });
        }
        LoginTicket::try_from_raw(100)
    }
    fn poll(&mut self, ticket: LoginTicket) -> LoadPoll {
        assert_eq!(ticket.get(), 100, "poll must use original provider receipt");
        self.outcome.take().unwrap_or(LoadPoll::Pending)
    }
    fn cancel(&mut self, ticket: LoginTicket) -> Result<(), ServerError> {
        assert_eq!(
            ticket.get(),
            100,
            "cancel must use original provider receipt"
        );
        self.cancels += 1;
        Ok(())
    }
}
fn deadline() -> Deadline {
    Deadline::after(Instant::now(), Duration::from_secs(5)).unwrap()
}
#[test]
fn actual_shared_login_driver_uses_latest_cache_before_load_start() {
    let mut s = enabled();
    let mut d = LoginDriver::new();
    let mut l = Loads {
        starts: 0,
        cancels: 0,
        outcome: Some(LoadPoll::Loaded(Some(stored(save(1), false)))),
        refuse: false,
    };
    let t = d
        .bind(&mut s, &mut l)
        .begin_login(login(1, "Ada"), TransportKind::Memory, deadline())
        .unwrap();
    assert_eq!(t.get(), 1);
    assert!(matches!(
        d.bind(&mut s, &mut l).poll_login(t),
        LoginPoll::Ready { .. }
    ));
    let first = d.bind(&mut s, &mut l).commit_login(t).unwrap();
    let mut latest = save(1);
    latest.hunger = 12;
    s.observe_actor_save(SaveValue::Player(latest), true, false)
        .unwrap();
    let held = s.select(SaveMode::All, SaveBudget::default());
    assert_eq!(held.len(), 1);
    s.retire(first, CloseReason::PeerGone).unwrap();
    l.refuse = true;
    let t2 = d
        .bind(&mut s, &mut l)
        .begin_login(login(1, "Ada"), TransportKind::Tcp, deadline())
        .unwrap();
    assert_eq!(t2.get(), 2);
    assert_eq!(l.starts, 1);
    assert!(matches!(
        d.bind(&mut s, &mut l).poll_login(t2),
        LoginPoll::Ready { .. }
    ));
    assert_eq!(d.bind(&mut s, &mut l).poll_login(t), LoginPoll::Pending);
    let second = d.bind(&mut s, &mut l).commit_login(t2).unwrap();
    assert_eq!(current(&s, 1).hunger, 12);
    assert_eq!(s.save_stats().in_flight, 1);
    s.retire(second, CloseReason::PeerGone).unwrap();
    let t3 = d
        .bind(&mut s, &mut l)
        .begin_login(login(1, "Grace"), TransportKind::Memory, deadline())
        .unwrap();
    d.bind(&mut s, &mut l).cancel_login(t3);
    assert_eq!(l.starts, 1);
    assert_eq!(l.cancels, 0);
    assert_eq!(current(&s, 1).display_name, "Ada");
}

// A detached fixture qualifies the exit boundary, not the original inventory writer.
fn fixture_exit_inventory(s: &mut AuthorityState, k: SessionKey, full: bool) {
    let mut residents = s.residents();
    let slot = residents
        .actors
        .iter()
        .position(|a| a.key == ActorKey::Player(k))
        .unwrap();
    let actor = &mut residents.actors[slot];
    actor.lifecycle = ActorLifecycle::Active;
    actor.motion = mornlea_domain::MotionState::new(mornlea_domain::MotionStateParts {
        position: mornlea_domain::FiniteVec3::try_new([8.5, 65.0, 8.5]).unwrap(),
        velocity: mornlea_domain::FiniteVec3::try_new([0.; 3]).unwrap(),
        on_ground: true,
    });
    let inventory = residents.inventories.get_mut(&ActorKey::Player(k)).unwrap();
    if full {
        inventory.slots = [mornlea_storage::ItemStack {
            item: 1,
            count: 64,
            durability: 0,
        }; 36];
    }
    for (index, cell) in inventory.crafting.iter_mut().enumerate() {
        *cell = mornlea_storage::ItemStack {
            item: 1,
            count: index as u8 + 1,
            durability: 0,
        };
    }
    s.commit_residents(residents);
}
#[test]
fn disconnect_reclaims_every_grid_cell_before_force_capture_and_actor_release() {
    let mut s = enabled();
    let k = prepare(&mut s, 1, "Ada", None);
    s.activate(k).unwrap();
    fixture_exit_inventory(&mut s, k, false);
    s.retire(k, CloseReason::PeerGone).unwrap();
    let p = current(&s, 1);
    assert_eq!(
        p.inventory.hotbar.slots[0],
        mornlea_storage::ItemStack {
            item: 1,
            count: 45,
            durability: 0
        }
    );
    assert!(s.residents().actors.is_empty());
    let targets = s.select(SaveMode::Urgent, SaveBudget::default());
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].urgency, SaveUrgency::Unload);
}
#[test]
fn failed_exit_repack_preserves_session_inventory_and_exact_held_owner() {
    let mut s = enabled();
    let k = prepare(&mut s, 1, "Ada", None);
    s.activate(k).unwrap();
    let held = s.select(SaveMode::All, SaveBudget::default());
    fixture_exit_inventory(&mut s, k, true);
    let before = s.residents().inventories[&ActorKey::Player(k)];
    let body = current(&s, 1).clone();
    assert!(s.retire(k, CloseReason::PeerGone).is_err());
    assert_eq!(s.phase(), ServerPhase::Closing);
    assert_eq!(s.session(k).unwrap().phase, SessionPhase::Active);
    assert_eq!(s.residents().inventories[&ActorKey::Player(k)], before);
    assert_eq!(current(&s, 1), &body);
    assert_eq!(s.save_stats().in_flight, 1);
    let submitted = held
        .iter()
        .map(|s| (s.key.clone(), s.revision))
        .collect::<Vec<_>>();
    assert_eq!(
        s.apply_completion(SaveCompletion {
            ticket: SaveTicket::try_from_raw(1).unwrap(),
            snapshots: held,
            submitted: submitted.clone(),
            committed: submitted,
            error: None
        })
        .acked,
        1
    );
}

#[test]
fn stale_retirement_does_not_poison_an_enabled_healthy_authority() {
    let mut s = enabled();
    let k = prepare(&mut s, 1, "Ada", None);
    s.retire(k, CloseReason::PeerGone).unwrap();
    assert_eq!(
        s.retire(k, CloseReason::PeerGone),
        Err(ServerError::StaleSession { session: k })
    );
    assert_eq!(s.phase(), ServerPhase::Running);
    let next = prepare(&mut s, 2, "Ada", None);
    s.activate(next).unwrap();
}

// Background disk and native acquisition are actual consumers; reload failure is injected.
mod physical {
    use mornlea_domain::{ChunkPos, Dimension, Identities, PlayerId};
    use mornlea_protocol::{
        ClientHello, ClientPacket, LoginStart, PlayerInput, ProtocolCodec, SelectHotbar,
        ServerPacket, State, encode_uvarint, read_frame_ref,
    };
    use mornlea_server::core::{
        acquisition::LiveChunkPhase, chunk_driver::ChunkDriver, generation_worker::GenerationPool,
    };
    use mornlea_server::store::{
        disk::{DiskOptions, DiskStore},
        mailbox::StoreMailbox,
        scheduler::{AutosaveScheduler, SchedulerConfig},
    };
    use mornlea_server::transport::{
        common::TransportAuthority, live::LoginDriver, memory::MemoryTransport,
    };
    use mornlea_server::{contracts::*, state::AuthorityState};
    use mornlea_storage::{
        Chunk, ChunkSave, ContainerSnapshot, Inventory, ItemStack, Metadata, MetadataChunkPos,
        PlayerLocation, PlayerSave, StorageKind, player_encoded_len,
    };
    use std::{
        collections::BTreeSet,
        fs,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    const BOUND: Duration = Duration::from_secs(5);
    static ROOTS: AtomicU64 = AtomicU64::new(0);
    struct Root(PathBuf, bool);
    impl Root {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "mornlea-source-player-persistence-{}-{}",
                std::process::id(),
                ROOTS.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path, true)
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            if self.1 {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
    fn deadline() -> Deadline {
        Deadline::after(Instant::now(), BOUND).unwrap()
    }
    fn player() -> PlayerId {
        let mut bytes = [0; 16];
        bytes[0] = 1;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        PlayerId::try_from_bytes(bytes).unwrap()
    }
    fn key(dimension: Dimension, x: i32, z: i32) -> ChunkKey {
        ChunkKey {
            dimension,
            pos: ChunkPos::new(x, z),
        }
    }
    fn options(dimension: Dimension, anchor: ChunkPos) -> DiskOptions {
        DiskOptions {
            region_handle_cap: 1,
            create: Metadata {
                format_version: mornlea_storage::METADATA_CURRENT_VERSION,
                seed: 42,
                spawn_dimension: i32::from(dimension.get()),
                spawn_anchor: MetadataChunkPos {
                    x: anchor.x(),
                    z: anchor.z(),
                },
                world_time_ticks: 0,
                day_phase_offset: 0,
                weather_kind: 0,
                weather_ticks_remaining: 0,
                depths_spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
                depths_seed_salt: 0,
                difficulty: 0,
            },
        }
    }
    fn saved_player() -> PlayerSave {
        let mut armor = [ItemStack::default(); 4];
        armor[1] = ItemStack {
            item: 5,
            count: 1,
            durability: 0,
        };
        PlayerSave {
            player_id: mornlea_storage::PlayerId::from_bytes(player().bytes()),
            revision: 9,
            display_name: "Ada".into(),
            current: PlayerLocation {
                dimension: 0,
                position: [8.5, 65.0, 8.5],
            },
            yaw: 0.1,
            pitch: 0.2,
            safe: None,
            inventory: {
                let mut inv = Inventory::default();
                inv.hotbar.selected = 3;
                inv.hotbar.slots[3] = ItemStack {
                    item: 1,
                    count: 7,
                    durability: 0,
                };
                inv
            },
            health: 15,
            hunger: 17,
            saturation_milli: 9000,
            exhaustion_milli: 250,
            respawn_present: false,
            respawn_position: [0.0; 3],
            respawn_dimension: 0,
            armor,
        }
    }
    fn air() -> Chunk {
        Chunk {
            sections: vec![
                ContainerSnapshot {
                    kind: StorageKind::Single,
                    bits: 0,
                    single: 0,
                    palette: vec![],
                    packed: vec![]
                };
                24
            ],
            drops: vec![Default::default(); 32],
            furnaces: vec![Default::default(); 32],
            chests: vec![Default::default(); 16],
        }
    }
    struct StepClock(Instant);
    impl Clock for StepClock {
        fn monotonic(&self) -> Instant {
            self.0
        }
        fn unix_ms(&self) -> i64 {
            1000
        }
    }
    // Both owners must close before the world directory is reused or removed.
    struct Fixture {
        wanted: BTreeSet<ChunkKey>,
        chunks: ChunkDriver,
        generation: GenerationPool,
        store: AutosaveScheduler<CountingDisk>,
        root: Root,
        loads: Arc<AtomicU64>,
        refuse_load: Arc<AtomicBool>,
    }
    impl Fixture {
        fn new(
            save: Option<PlayerSave>,
            dimension: Dimension,
            anchor: ChunkPos,
            chunks: Vec<(ChunkKey, Chunk)>,
        ) -> (Self, AuthorityState) {
            assert_eq!(Identities::current().engine_abi, 11);
            let root = Root::new();
            let mut disk = DiskStore::open(&root.0, options(dimension, anchor)).unwrap();
            let mut snapshots = Vec::new();
            if let Some(save) = save {
                snapshots.push(
                    OwnedSnapshot::try_new(
                        SaveKey::Player(player()),
                        save.revision,
                        player_encoded_len(&save).unwrap(),
                        SaveUrgency::Autosave,
                        SaveValue::Player(save),
                    )
                    .unwrap(),
                );
            }
            for (key, chunk) in chunks {
                snapshots.push(
                    OwnedSnapshot::try_new(
                        SaveKey::Chunk(key),
                        9,
                        1,
                        SaveUrgency::Autosave,
                        SaveValue::Chunk(ChunkSave {
                            key: mornlea_storage::ChunkKey {
                                dimension: i32::from(key.dimension.get()),
                                x: key.pos.x(),
                                z: key.pos.z(),
                            },
                            revision: 9,
                            chunk,
                        }),
                    )
                    .unwrap(),
                );
            }
            let expected: Vec<_> = snapshots
                .iter()
                .map(|s| (s.key.clone(), s.revision))
                .collect();
            let completion = disk.write(
                SaveTicket::try_from_raw(1).unwrap(),
                SaveRequest {
                    snapshots: snapshots.clone(),
                },
            );
            assert_eq!(completion.snapshots, snapshots);
            assert_eq!(completion.error, None);
            assert_eq!(completion.committed.len(), expected.len());
            for key in expected {
                assert!(completion.committed.contains(&key));
            }
            let mut state = AuthorityState::try_new_with_metadata(
                ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
                disk.metadata().clone(),
            )
            .unwrap();
            state.enable_source_player_restoration(1).unwrap();
            state.enable_live_chunks().unwrap();
            state.enable_actor_saves().unwrap();
            state.enable_player_persistence().unwrap();
            let loads = Arc::new(AtomicU64::new(0));
            let refuse_load = Arc::new(AtomicBool::new(false));
            let store = AutosaveScheduler::try_new(
                SchedulerConfig::default(),
                StoreMailbox::try_new_background(
                    StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
                    CountingDisk {
                        disk,
                        loads: loads.clone(),
                        refuse_load: refuse_load.clone(),
                    },
                )
                .unwrap(),
            )
            .unwrap();
            (
                Self {
                    wanted: BTreeSet::new(),
                    chunks: ChunkDriver::new(),
                    generation: GenerationPool::try_new(42, false, 1).unwrap(),
                    store,
                    root,
                    loads,
                    refuse_load,
                },
                state,
            )
        }
        fn acquire(&mut self, state: &mut AuthorityState, key: ChunkKey) -> TickPublication {
            self.wanted.insert(key);
            state.replace_chunk_wants(self.wanted.clone()).unwrap();
            self.chunks
                .start_load(state, &mut self.store, key, deadline())
                .unwrap();
            let until = Instant::now() + BOUND;
            loop {
                self.store.drive_workers();
                let report = self
                    .chunks
                    .poll(state, &mut self.store, &mut self.generation);
                assert_eq!(report.first_error, None);
                if report.retained == 0 {
                    break;
                }
                assert!(Instant::now() < until, "actual chunk poll stalled");
                thread::yield_now();
            }
            assert_eq!(self.chunks.pending_generations(), 0);
            let publication = state.advance_tick(TickBudget::full()).unwrap();
            assert_eq!(
                state.live_chunk_facts(key).unwrap().phase,
                LiveChunkPhase::Ready
            );
            assert_eq!(state.live_chunk_facts(key).unwrap().persisted_revision, 9);
            publication
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let generated = self.generation.close(deadline());
            let stored = self.store.close(deadline());
            // Failed joins preserve the fixture tree for the still-owning process.
            self.root.1 = generated.is_ok() && stored.is_ok();
        }
    }

    use mornlea_server::core::step::AuthoritativeFinalReducer;
    use mornlea_server::transport::tcp::TcpTransport;
    use std::io::{self, Read, Write};
    use std::net::TcpStream;
    struct CountingDisk {
        disk: DiskStore,
        loads: Arc<AtomicU64>,
        refuse_load: Arc<AtomicBool>,
    }
    impl DiskBackend for CountingDisk {
        fn load(&mut self, key: SaveKey) -> Result<LoadedValue, ServerError> {
            if matches!(key, SaveKey::Player(_)) {
                self.loads.fetch_add(1, Ordering::SeqCst);
                if self.refuse_load.load(Ordering::SeqCst) {
                    return Err(ServerError::Internal {
                        invariant: "injected player reload failure",
                    });
                }
            }
            self.disk.load(key)
        }
        fn write(&mut self, t: SaveTicket, r: SaveRequest) -> SaveCompletion {
            self.disk.write(t, r)
        }
        fn sync(&mut self) -> Result<(), ServerError> {
            self.disk.sync()
        }
        fn close(&mut self) -> Result<(), ServerError> {
            self.disk.close()
        }
    }
    fn frame(
        wire: &mut Wire,
        endpoint: &mut dyn TransportAuthority,
        clock: &StepClock,
        state: State,
    ) -> ServerPacket {
        let until = Instant::now() + BOUND;
        loop {
            wire.poll(endpoint, clock);
            wire.flush(endpoint);
            let frames = wire.receive();
            if !frames.is_empty() {
                assert_eq!(frames.len(), 1);
                let parsed = read_frame_ref(&frames[0]).unwrap();
                return ProtocolCodec::new()
                    .unwrap()
                    .decode_server(state, parsed.packet_id, parsed.payload)
                    .unwrap();
            }
            assert!(Instant::now() < until, "actual peer frame stalled");
            thread::yield_now();
        }
    }
    fn connect(
        f: &mut Fixture,
        s: &mut AuthorityState,
        d: &mut LoginDriver,
        kind: TransportKind,
        ticket: u64,
        clock: &StepClock,
    ) -> (Wire, SessionKey) {
        let mut wire = Wire::open(kind, &mut d.bind(s, &mut f.store), clock);
        let hello = ClientPacket::ClientHello(
            ClientHello::decode_inbound(&encode_uvarint(Identities::current().protocol)).unwrap(),
        );
        wire.send(
            MemoryTransport::encode_frame(&hello).unwrap(),
            &mut d.bind(s, &mut f.store),
            clock,
        );
        assert!(matches!(
            frame(
                &mut wire,
                &mut d.bind(s, &mut f.store),
                clock,
                State::Handshake
            ),
            ServerPacket::ServerHello(_)
        ));
        wire.acknowledge(1, &mut d.bind(s, &mut f.store));
        let start = LoginStart::new(player(), "Ada", 8).unwrap();
        wire.send(
            MemoryTransport::encode_frame(&ClientPacket::LoginStart(
                LoginStart::decode_inbound(&start.encode().unwrap()).unwrap(),
            ))
            .unwrap(),
            &mut d.bind(s, &mut f.store),
            clock,
        );
        let until = Instant::now() + BOUND;
        let session = loop {
            f.store.drive_workers();
            wire.poll(&mut d.bind(s, &mut f.store), clock);
            match d
                .bind(s, &mut f.store)
                .poll_login(LoginTicket::try_from_raw(ticket).unwrap())
            {
                LoginPoll::Ready { session, .. } => break session,
                LoginPoll::Failed { error, .. } => panic!("actual login failed: {error:?}"),
                LoginPoll::Pending => {
                    assert!(Instant::now() < until);
                    thread::yield_now();
                }
            }
        };
        assert!(matches!(
            frame(&mut wire, &mut d.bind(s, &mut f.store), clock, State::Login),
            ServerPacket::LoginSuccess(_)
        ));
        wire.acknowledge(1, &mut d.bind(s, &mut f.store));
        assert_eq!(s.session(session).unwrap().phase, SessionPhase::Active);
        (wire, session)
    }
    fn current(s: &AuthorityState) -> &PlayerSave {
        let (_, SaveValue::Player(p)) = s.actor_save_current(&SaveKey::Player(player())).unwrap()
        else {
            panic!("player body")
        };
        p
    }
    #[test]
    fn actual_missing_load_aborts_leave_no_player_file_or_cache_charge() {
        let (mut fixture, mut state) =
            Fixture::new(None, Dimension::OVERWORLD, ChunkPos::new(0, 0), vec![]);
        let mut login = LoginDriver::new();
        for _ in 0..18 {
            let ticket = login
                .bind(&mut state, &mut fixture.store)
                .begin_login(super::login(1, "Ada"), TransportKind::Memory, deadline())
                .unwrap();
            let until = Instant::now() + BOUND;
            loop {
                fixture.store.drive_workers();
                match login
                    .bind(&mut state, &mut fixture.store)
                    .poll_login(ticket)
                {
                    LoginPoll::Ready { .. } => break,
                    LoginPoll::Failed { error, .. } => panic!("missing load: {error:?}"),
                    LoginPoll::Pending => {
                        assert!(Instant::now() < until);
                        thread::yield_now();
                    }
                }
            }
            login
                .bind(&mut state, &mut fixture.store)
                .cancel_login(ticket);
            assert!(
                state
                    .actor_save_current(&SaveKey::Player(player()))
                    .is_none()
            );
            assert_eq!(state.save_stats(), SaveStats::default());
        }
        assert_eq!(fixture.loads.load(Ordering::SeqCst), 18);
        fixture.generation.close(deadline()).unwrap();
        fixture.store.close(deadline()).unwrap();
        assert_eq!(
            fs::read_dir(fixture.root.0.join("players"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn actual_memory_settled_capture_autosaves_all_fields_and_reopens_player() {
        let chunk_key = key(Dimension::OVERWORLD, 0, 0);
        let (mut fixture, mut state) = Fixture::new(
            Some(saved_player()),
            Dimension::OVERWORLD,
            ChunkPos::new(0, 0),
            vec![(chunk_key, air())],
        );
        let mut login = LoginDriver::new();
        let clock = StepClock(Instant::now());
        let (mut memory, session) = connect(
            &mut fixture,
            &mut state,
            &mut login,
            TransportKind::Memory,
            1,
            &clock,
        );
        fixture.acquire(&mut state, chunk_key);
        memory.send(
            MemoryTransport::encode_frame(&ClientPacket::SelectHotbar(
                SelectHotbar::new(1, 7).unwrap(),
            ))
            .unwrap(),
            &mut login.bind(&mut state, &mut fixture.store),
            &clock,
        );
        state.advance_tick(TickBudget::full()).unwrap();
        let expected = current(&state).clone();
        assert_eq!(expected.inventory.hotbar.selected, 7);
        let until = Instant::now() + BOUND;
        let mut tick = 0;
        let mut submitted = 0;
        loop {
            fixture.store.drive_workers();
            let report = fixture
                .store
                .poll_tick(tick, SaveBudget::default(), &mut state)
                .unwrap();
            submitted += report.autosave;
            if report.stats == SaveStats::default()
                && fixture.store.tracked_submits() == 0
                && !fixture.store.metadata_pending()
            {
                break;
            }
            assert!(Instant::now() < until);
            tick += 1;
            thread::yield_now();
        }
        assert_eq!(submitted, 1);
        assert_eq!(fixture.store.tracked_submits(), 0);
        assert_eq!(state.session(session).unwrap().phase, SessionPhase::Active);
        fixture.generation.close(deadline()).unwrap();
        fixture.store.close(deadline()).unwrap();
        let mut disk = DiskStore::open(
            &fixture.root.0,
            options(Dimension::OVERWORLD, ChunkPos::new(0, 0)),
        )
        .unwrap();
        let LoadedValue::Player(actual) = disk.load(SaveKey::Player(player())).unwrap() else {
            panic!("actual player")
        };
        let mut expected = super::stored(expected, false);
        expected.revision = state
            .actor_save_current(&SaveKey::Player(player()))
            .unwrap()
            .0;
        assert_eq!(actual, expected);
        assert_eq!(actual.revision, 10);
        disk.close().unwrap();
    }

    #[test]
    fn actual_memory_tick_tcp_cache_reconnect_and_final_flush_reopen_latest_player() {
        let chunk_key = key(Dimension::OVERWORLD, 0, 0);
        let (mut fixture, mut state) = Fixture::new(
            Some(saved_player()),
            Dimension::OVERWORLD,
            ChunkPos::new(0, 0),
            vec![(chunk_key, air())],
        );
        let mut login = LoginDriver::new();
        let clock = StepClock(Instant::now());
        let (mut memory, session) = connect(
            &mut fixture,
            &mut state,
            &mut login,
            TransportKind::Memory,
            1,
            &clock,
        );
        fixture.acquire(&mut state, chunk_key);
        assert_eq!(
            state
                .settled_read()
                .unwrap()
                .actor(ActorKey::Player(session))
                .unwrap()
                .lifecycle,
            ActorLifecycle::Active
        );
        let input = ClientPacket::PlayerInput(
            PlayerInput::new(1, 1, 0, false, 1.2, 0.3, false, false, false, false).unwrap(),
        );
        let select = ClientPacket::SelectHotbar(SelectHotbar::new(2, 5).unwrap());
        for packet in [input, select] {
            memory.send(
                MemoryTransport::encode_frame(&packet).unwrap(),
                &mut login.bind(&mut state, &mut fixture.store),
                &clock,
            );
        }
        state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(current(&state).inventory.hotbar.selected, 5);
        assert_eq!((current(&state).yaw, current(&state).pitch), (1.2, 0.3));
        assert_ne!(
            current(&state).current.position,
            saved_player().current.position
        );
        assert_eq!(fixture.loads.load(Ordering::SeqCst), 1);
        // Hold an exact older target, then settle a different current value before reconnect.
        let held = state.select(SaveMode::All, SaveBudget::default());
        assert_eq!(held.len(), 1);
        memory.send(
            MemoryTransport::encode_frame(&ClientPacket::SelectHotbar(
                SelectHotbar::new(3, 6).unwrap(),
            ))
            .unwrap(),
            &mut login.bind(&mut state, &mut fixture.store),
            &clock,
        );
        state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(current(&state).inventory.hotbar.selected, 6);
        memory.close(&mut login.bind(&mut state, &mut fixture.store));
        assert_eq!(state.session(session).unwrap().phase, SessionPhase::Retired);
        fixture.refuse_load.store(true, Ordering::SeqCst);
        let (mut tcp, next) = connect(
            &mut fixture,
            &mut state,
            &mut login,
            TransportKind::Tcp,
            2,
            &clock,
        );
        assert_eq!(fixture.loads.load(Ordering::SeqCst), 1);
        assert_eq!(current(&state).inventory.hotbar.selected, 6);
        assert_eq!(state.save_stats().in_flight, 1);
        // This is an explicit refused-admission control; actual scheduler durability follows.
        for snapshot in held {
            state.return_dirty(snapshot);
        }
        state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(
            state
                .settled_read()
                .unwrap()
                .inventory(ActorKey::Player(next))
                .unwrap()
                .selected
                .get(),
            6
        );
        tcp.close(&mut login.bind(&mut state, &mut fixture.store));
        state.begin_close();
        state.run_final(&mut AuthoritativeFinalReducer).unwrap();
        let expected = current(&state).clone();
        let report = fixture.store.flush(deadline(), &mut state, &clock).unwrap();
        assert_eq!((report.failed, report.outstanding), (0, 0));
        assert_eq!(state.save_stats(), SaveStats::default());
        fixture.generation.close(deadline()).unwrap();
        fixture.store.close(deadline()).unwrap();
        let mut reopened = DiskStore::open(
            &fixture.root.0,
            options(Dimension::OVERWORLD, ChunkPos::new(0, 0)),
        )
        .unwrap();
        let mut actual = super::stored(expected.clone(), false);
        actual.revision = state
            .actor_save_current(&SaveKey::Player(player()))
            .unwrap()
            .0;
        let LoadedValue::Player(durable) = reopened.load(SaveKey::Player(player())).unwrap() else {
            panic!("durable player")
        };
        assert_eq!(durable, actual);
        assert!(durable.revision > 9);
        reopened.close().unwrap();
    }
    enum Wire {
        Memory {
            transport: MemoryTransport,
            id: ConnectionId,
        },
        Tcp {
            transport: TcpTransport,
            id: ConnectionId,
            peer: TcpStream,
            received: Vec<u8>,
        },
    }
    impl Wire {
        fn open(
            kind: TransportKind,
            endpoint: &mut dyn TransportAuthority,
            clock: &StepClock,
        ) -> Self {
            match kind {
                TransportKind::Memory => {
                    let mut transport = MemoryTransport::new();
                    let id = transport.connect(clock.monotonic()).unwrap();
                    Self::Memory { transport, id }
                }
                TransportKind::Tcp => {
                    let mut transport = TcpTransport::bind_loopback().unwrap();
                    let peer = TcpStream::connect_timeout(&transport.local_addr().unwrap(), BOUND)
                        .unwrap();
                    peer.set_nodelay(true).unwrap();
                    peer.set_write_timeout(Some(BOUND)).unwrap();
                    let until = Instant::now() + BOUND;
                    let id = loop {
                        if let Some(id) = transport.accept_one(endpoint, clock).unwrap() {
                            break id;
                        }
                        assert!(Instant::now() < until);
                        thread::yield_now();
                    };
                    peer.set_nonblocking(true).unwrap();
                    Self::Tcp {
                        transport,
                        id,
                        peer,
                        received: Vec::new(),
                    }
                }
            }
        }
        fn send(
            &mut self,
            bytes: Vec<u8>,
            endpoint: &mut dyn TransportAuthority,
            clock: &StepClock,
        ) {
            match self {
                Self::Memory { transport, id } => {
                    transport.send(*id, bytes, endpoint, clock);
                }
                Self::Tcp {
                    transport,
                    id,
                    peer,
                    ..
                } => {
                    // Test-peer writes are small and deadline bounded, outside authority.
                    peer.set_nonblocking(false).unwrap();
                    peer.write_all(&bytes).unwrap();
                    peer.set_nonblocking(true).unwrap();
                    transport.pump_in(*id, endpoint, clock);
                }
            }
        }
        fn poll(
            &mut self,
            endpoint: &mut dyn TransportAuthority,
            clock: &StepClock,
        ) -> ConnectionProgress {
            match self {
                Self::Memory { transport, id } => transport.poll(*id, endpoint, clock),
                Self::Tcp { transport, id, .. } => {
                    transport.pump_in(*id, endpoint, clock);
                    transport.poll(*id, endpoint, clock)
                }
            }
        }
        fn flush(&mut self, endpoint: &mut dyn TransportAuthority) {
            if let Self::Tcp { transport, id, .. } = self {
                transport.flush_out(*id, endpoint);
            }
        }
        fn receive(&mut self) -> Vec<Vec<u8>> {
            match self {
                Self::Memory { transport, id } => transport.receive(*id, 64, 1 << 20),
                Self::Tcp { peer, received, .. } => {
                    let mut buf = [0; 4096];
                    for _ in 0..16 {
                        match peer.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => received.extend_from_slice(&buf[..n]),
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                            Err(error) => panic!("peer read: {error}"),
                        }
                    }
                    let mut frames = Vec::new();
                    while let Ok(frame) = read_frame_ref(received) {
                        let consumed = frame.consumed;
                        frames.push(received.drain(..consumed).collect());
                    }
                    frames
                }
            }
        }
        fn acknowledge(&mut self, count: usize, endpoint: &mut dyn TransportAuthority) {
            match self {
                Self::Memory { transport, id } => {
                    transport.acknowledge(*id, count, endpoint);
                }
                Self::Tcp { .. } => self.flush(endpoint),
            }
        }
        fn close(&mut self, endpoint: &mut dyn TransportAuthority) {
            match self {
                Self::Memory { transport, id } => {
                    transport.close(*id, CloseReason::PeerGone, endpoint)
                }
                Self::Tcp { transport, id, .. } => {
                    transport.close(*id, CloseReason::PeerGone, endpoint)
                }
            }
        }
    }
}

#[test]
fn exact_durable_ack_allows_clean_offline_cache_eviction_before_next_load() {
    let mut state = enabled();
    for tag in 1..=16 {
        let session = prepare(&mut state, tag, "Ada", None);
        state.activate(session).unwrap();
        state.retire(session, CloseReason::PeerGone).unwrap();
    }
    let targets = state.select(SaveMode::All, SaveBudget::default());
    assert_eq!(targets.len(), 16);
    let submitted: Vec<_> = targets
        .iter()
        .map(|s| (s.key.clone(), s.revision))
        .collect();
    assert_eq!(
        state
            .apply_completion(SaveCompletion {
                ticket: SaveTicket::try_from_raw(1).unwrap(),
                snapshots: targets,
                committed: submitted.clone(),
                submitted,
                error: None
            })
            .acked,
        16
    );
    let session = state
        .prepare(login(17, "Ada"), TransportKind::Memory)
        .unwrap();
    assert!(!state.prepare_player_cache(session).unwrap());
    for tag in 1..=16 {
        assert!(
            state
                .actor_save_current(&SaveKey::Player(id(tag)))
                .is_none()
        );
    }
    state.retire(session, CloseReason::PeerGone).unwrap();
    assert_eq!(state.save_stats(), SaveStats::default());
}

#[test]
fn enabled_loading_cancel_routes_original_provider_ticket_and_releases_placeholder() {
    let mut state = enabled();
    let mut driver = LoginDriver::new();
    let mut loads = Loads {
        starts: 0,
        cancels: 0,
        outcome: None,
        refuse: false,
    };
    let ticket = driver
        .bind(&mut state, &mut loads)
        .begin_login(login(1, "Ada"), TransportKind::Memory, deadline())
        .unwrap();
    assert_eq!(ticket.get(), 1);
    assert_eq!(
        driver.bind(&mut state, &mut loads).poll_login(ticket),
        LoginPoll::Pending
    );
    driver.bind(&mut state, &mut loads).cancel_login(ticket);
    assert_eq!(loads.cancels, 1);
    assert_eq!(driver.pending(), 0);
    assert!(state.actor_save_current(&SaveKey::Player(id(1))).is_none());
    assert_eq!(state.save_stats(), SaveStats::default());
    let next = driver
        .bind(&mut state, &mut loads)
        .begin_login(login(1, "Ada"), TransportKind::Tcp, deadline())
        .unwrap();
    assert_eq!(next.get(), 2);
    driver.bind(&mut state, &mut loads).cancel_login(next);
    assert_eq!(loads.cancels, 2);
}
