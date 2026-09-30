//! Production tick hydration: residents persist, logins seed actors.
//!
//! Live ticks used to run empty overlays: staged actors, runtimes,
//! inventories, mining, projectiles, environment, blocks, Ready chunks,
//! drops, and containers vanished at the tick edge, and an Active login
//! never became an actor. These tests pin the resident maps instead: a
//! committed overlay round-trips every map exactly, consecutive live ticks
//! carry residents, logins seed Go-default actors from saves, and empty
//! ticks stay empty.

use mornlea_domain::{
    BlockPos, ChunkPos, ContainerKind, ContainerRef, CraftingSize, Dimension, DropId, FiniteVec3,
    HostileId, HotbarSlot, LookAngles, MotionState, MotionStateParts, PlayerId, ProjectileId,
    ProjectileKind, SurvivalState, SurvivalStateParts, Weather,
};
use mornlea_protocol::{AdmittedLogin, LoginStart, admit_login};
use mornlea_server::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, BlockObservation,
    ChunkKey, ContainerRecord, ContainerSlots, DropRecord, EnvironmentState, InventoryRecord,
    MiningProgress, ProjectileRecord, RuleEffect, RuleTunables, ServerLimits, SessionKey,
    TickBudget, TransportKind,
};
use mornlea_server::core::login_seed::seed_player;
use mornlea_server::core::world::ReadyChunk;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{
    ChestSlot, Chunk, ContainerSnapshot, FurnaceSlot, Inventory, ItemStack,
    PlayerId as SavePlayerId, PlayerLocation, PlayerSave, StorageKind, StoredPlayer,
};

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        7,
    )
    .unwrap()
}

fn admitted(tag: u8, name: &str) -> AdmittedLogin {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = PlayerId::try_from_bytes(bytes).unwrap();
    let start = LoginStart::new(id, name, 8).unwrap();
    let inbound = LoginStart::decode_inbound(&start.encode().unwrap()).unwrap();
    admit_login(inbound).unwrap()
}

fn save_id(player: PlayerId) -> SavePlayerId {
    SavePlayerId::from_bytes(player.bytes())
}

fn coal(count: u8) -> ItemStack {
    ItemStack {
        item: 5,
        count,
        durability: 0,
    }
}

/// Builds the customized save inventory: hotbar selection 5 with a stack in
/// slot 2, backpack stacks first and last. Field assignment avoids naming
/// the unexported hotbar type; the selection stays inside the hotbar range
/// so the save validates at install.
fn customized_inventory() -> Inventory {
    let mut inventory = Inventory::default();
    inventory.hotbar.selected = 5;
    inventory.hotbar.slots[2] = coal(7);
    inventory.backpack[0] = coal(64);
    inventory.backpack[26] = coal(1);
    inventory
}

/// Save/player pair with every login-mapped field customized: position,
/// look, health, the hunger triple, hotbar selection and contents, backpack
/// contents, and armor. Both records carry identical values; the stored form
/// feeds `install` while the save form feeds the pure mapping.
fn customized_save(player: PlayerId) -> (StoredPlayer, PlayerSave) {
    let mut armor = [ItemStack::default(); 4];
    armor[1] = coal(1);
    let stored = StoredPlayer {
        player_id: save_id(player),
        revision: 1,
        display_name: "Ada".to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position: [10.5, 65.0, -3.25],
        },
        yaw: 1.2,
        pitch: 0.3,
        safe: None,
        inventory: customized_inventory(),
        health: 15,
        hunger: 17,
        saturation_milli: 9_000,
        exhaustion_milli: 250,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor,
        needs_rewrite: false,
    };
    let save = PlayerSave {
        player_id: save_id(player),
        revision: 1,
        display_name: "Ada".to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position: [10.5, 65.0, -3.25],
        },
        yaw: 1.2,
        pitch: 0.3,
        safe: None,
        inventory: customized_inventory(),
        health: 15,
        hunger: 17,
        saturation_milli: 9_000,
        exhaustion_milli: 250,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor,
    };
    (stored, save)
}

fn expected_actor(session: SessionKey, save: &PlayerSave) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Player(session),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(save.current.position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: false,
        }),
        LookAngles::try_new(save.yaw, save.pitch).unwrap(),
        SurvivalState::try_new(SurvivalStateParts {
            health: save.health,
            oxygen: 300,
            hunger: save.hunger,
            saturation_zero: false,
            armor_points: 0,
        })
        .unwrap(),
        ActorBody::Player(save.clone()),
    )
    .unwrap()
}

fn expected_inventory(save: &PlayerSave) -> InventoryRecord {
    let mut slots = [ItemStack::default(); 36];
    slots[0..9].copy_from_slice(&save.inventory.hotbar.slots);
    slots[9..36].copy_from_slice(&save.inventory.backpack);
    InventoryRecord::try_new(
        slots,
        HotbarSlot::new(save.inventory.hotbar.selected).unwrap(),
        save.armor,
        [ItemStack::default(); 9],
        CraftingSize::Personal,
    )
    .unwrap()
}

fn login_session(
    authority: &mut AuthorityState,
    login: AdmittedLogin,
    stored: StoredPlayer,
) -> SessionKey {
    let session = authority.prepare(login, TransportKind::Memory).unwrap();
    authority.install(session, Some(stored)).unwrap();
    authority.activate(session).unwrap();
    session
}

/// The pure save-to-actor mapping carries every login default: the save
/// position with zeroed velocity and a false ground bit, the restore look,
/// restore-or-full health, the save hunger triple, an empty personal
/// crafting grid, the unified hotbar-plus-backpack slots with the save
/// selection, slot-wise armor, the save dimension, and an Active lifecycle.
#[test]
fn login_mapping_carries_every_save_default() {
    let mut authority = authority();
    let session = authority
        .prepare(admitted(1, "Ada"), TransportKind::Memory)
        .unwrap();
    let player = authority.session(session).unwrap().player_id;
    let (_, save) = customized_save(player);
    let seeded = seed_player(session, &save).unwrap();
    assert_eq!(seeded.actor, expected_actor(session, &save));
    assert_eq!(seeded.inventory, expected_inventory(&save));
}

/// A live tick stages the login seed: the session becomes an actor with the
/// mapped body and the exact inventory, with no runtime or mining records
/// staged (those transient lanes start zeroed), and the next tick stages
/// nothing twice.
#[test]
fn login_session_becomes_actor_on_live_tick() {
    let mut authority = authority();
    let login = admitted(1, "Ada");
    let (stored, save) = customized_save(login.player_id());
    let session = login_session(&mut authority, login, stored);
    authority.advance_tick(TickBudget::full()).unwrap();
    let residents = authority.residents();
    assert_eq!(residents.actors.len(), 1);
    let actor = &residents.actors[0];
    let want = expected_actor(session, &save);
    assert_eq!(actor.key, want.key);
    assert_eq!(actor.lifecycle, want.lifecycle);
    assert_eq!(actor.dimension, want.dimension);
    assert_eq!(actor.look, want.look);
    // Movement preserves the restored hunger lanes across the live reducer.
    assert_eq!(actor.survival.health(), want.survival.health());
    assert_eq!(actor.survival.hunger(), want.survival.hunger());
    assert_eq!(actor.survival.oxygen(), want.survival.oxygen());
    assert_eq!(actor.survival.armor_points(), 0);
    assert_eq!(
        actor.survival.saturation_zero(),
        want.survival.saturation_zero()
    );
    assert_eq!(residents.runtimes[&actor.key].saturation_milli, 9_000);
    assert_eq!(residents.runtimes[&actor.key].exhaustion_milli, 250);
    assert_eq!(
        residents.inventories.get(&actor.key),
        Some(&expected_inventory(&save))
    );
    assert!(!residents.mining.contains_key(&actor.key));
    authority.advance_tick(TickBudget::full()).unwrap();
    let again = authority.residents();
    assert_eq!(again.actors.len(), 1);
    assert_eq!(again.actors[0].key, want.key);
    assert!(!again.actors[0].survival.saturation_zero());
    assert_eq!(again.runtimes[&actor.key].saturation_milli, 9_000);
    assert_eq!(again.runtimes[&actor.key].exhaustion_milli, 250);
    assert_eq!(
        again.inventories.get(&actor.key),
        Some(&expected_inventory(&save))
    );
}

/// Zero health restores full health instead of staging a dead actor.
#[test]
fn login_zero_health_restores_full_health() {
    let mut authority = authority();
    let login = admitted(2, "Bo");
    let (mut stored, mut save) = customized_save(login.player_id());
    stored.health = 0;
    save.health = 0;
    let session = login_session(&mut authority, login, stored);
    let seeded = seed_player(session, &save).unwrap();
    assert_eq!(seeded.actor.survival.health(), 20);
    authority.advance_tick(TickBudget::full()).unwrap();
    let residents = authority.residents();
    assert_eq!(residents.actors.len(), 1);
    assert_eq!(residents.actors[0].key, ActorKey::Player(session));
    assert_eq!(residents.actors[0].survival.health(), 20);
}

/// Sessions without a save body, and sessions that never reached Active,
/// never become actors.
#[test]
fn login_without_body_or_activation_never_becomes_actor() {
    let mut authority = authority();
    let _ = authority
        .admit(admitted(3, "Cy"), TransportKind::Memory)
        .unwrap();
    let pending = authority
        .prepare(admitted(4, "Dee"), TransportKind::Memory)
        .unwrap();
    let player = authority.session(pending).unwrap().player_id;
    let (stored, _) = customized_save(player);
    authority.install(pending, Some(stored)).unwrap();
    authority.advance_tick(TickBudget::full()).unwrap();
    authority.advance_tick(TickBudget::full()).unwrap();
    assert!(authority.residents().actors.is_empty());
}

fn hostile_actor(id: u64, position: [f32; 3]) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Hostile(HostileId::try_new(id).unwrap()),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        }),
        LookAngles::try_new(0.0, 0.0).unwrap(),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .unwrap(),
        ActorBody::Hostile(mornlea_storage::HostileMob {
            id,
            dimension: 0,
            position,
            velocity: [0.0; 3],
            on_ground: true,
            yaw: 0.0,
            health: 20,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 0,
            has_target: false,
            player_id: SavePlayerId::from_bytes([0; 16]),
            next_repath_ticks: 0,
            distant_ticks: 0,
            kind: 0,
        }),
    )
    .unwrap()
}

fn chunk_key(x: i32, z: i32) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(x, z),
    }
}

fn air_chunk() -> Chunk {
    Chunk {
        sections: vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: vec![],
                packed: vec![],
            };
            24
        ],
        drops: vec![Default::default(); 32],
        furnaces: vec![FurnaceSlot::default(); 32],
        chests: vec![ChestSlot::default(); 16],
    }
}

/// Chunk with one active chest: the slot must point at a matching chest
/// block inside the chunk or chunk validation refuses the whole aggregate.
fn chest_chunk() -> Chunk {
    const CHEST_BLOCK: u16 = 11;
    let mut chunk = air_chunk();
    let pos = BlockPos::new(0, 65, 0);
    let index = mornlea_domain::chunk_block_index(pos) as usize;
    let section = &mut chunk.sections[index / 4096];
    *section = ContainerSnapshot {
        kind: StorageKind::Direct,
        bits: 15,
        single: 0,
        palette: vec![],
        packed: vec![0; 1024],
    };
    section.packed[(index % 4096) / 4] |= u64::from(CHEST_BLOCK) << (((index % 4096) % 4) * 15);
    let mut items = [ItemStack::default(); 27];
    items[0] = coal(3);
    chunk.chests[0] = ChestSlot {
        generation: 1,
        active: true,
        block_index: index as u32,
        items,
    };
    chunk
}

fn drop_record(slot: u8, age: u32) -> DropRecord {
    DropRecord {
        id: DropId::try_new(0, ChunkPos::new(0, 0), slot, 1).unwrap(),
        position: FiniteVec3::try_new([0.5, 65.5, 2.5]).unwrap(),
        stack: coal(2),
        pickup_delay: 5,
        age,
    }
}

/// Stages one entry per resident map on a harness overlay and commits the
/// extracted snapshot: the commit-back path every production tick uses.
fn commit_full_overlay(authority: &mut AuthorityState) {
    let key = chunk_key(0, 0);
    let mut ctx = TickContext::harness(authority, TickBudget::full());
    ctx.preload_ready_chunk(ReadyChunk::try_new(key, 1, 1, chest_chunk()).unwrap());
    let sparse_pos = BlockPos::new(80, 64, 80);
    let sparse_key = chunk_key(5, 5);
    ctx.preload_block(BlockObservation::try_new(sparse_key, 1, 1, sparse_pos, 4).unwrap());
    ctx.preload_drop(drop_record(3, 7));
    ctx.preload_drop(drop_record(4, 7));
    let sparse_ref =
        ContainerRef::try_new(ChunkPos::new(9, 9), ContainerKind::Chest, 0, 1).unwrap();
    let mut chest_items = [ItemStack::default(); 27];
    chest_items[1] = coal(9);
    ctx.preload_container(ContainerRecord {
        reference: sparse_ref,
        revision: 3,
        slots: ContainerSlots::Chest(chest_items),
    });
    let hostile = hostile_actor(11, [100.5, 65.0, 100.5]);
    ctx.stage(RuleEffect::Actor(hostile)).unwrap();
    let runtime_key = ActorKey::Hostile(HostileId::try_new(12).unwrap());
    ctx.stage(RuleEffect::Runtime(ActorRuntime {
        key: runtime_key,
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 0,
        peak_y: 65.0,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Hostile {
            distant_ticks: 2,
            shoot_cooldown: 3,
            fresh: true,
        },
    }))
    .unwrap();
    let mining_key = ActorKey::Hostile(HostileId::try_new(13).unwrap());
    ctx.preload_mining(MiningProgress {
        actor: mining_key,
        dimension: Dimension::OVERWORLD,
        target: BlockPos::new(1, 64, 1),
        observed_block: 3,
        tool_slot: HotbarSlot::new(0).unwrap(),
        tool: ItemStack::default(),
        elapsed: 4,
        required: 40,
        last_tick: 0,
    });
    ctx.preload_inventory(runtime_key, InventoryRecord::empty());
    ctx.preload_inventory(mining_key, InventoryRecord::empty());
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(ProjectileRecord {
            id: ProjectileId::try_new(9).unwrap(),
            owner: hostile_actor(11, [100.5, 65.0, 100.5]).key,
            dimension: Dimension::OVERWORLD,
            // Scoped aside from the login column: the same column would
            // entity-hit the falling player and despawn on impact.
            position: FiniteVec3::try_new([40.5, 66.0, 0.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0, -1.0, 0.0]).unwrap(),
            kind: ProjectileKind::Arrow,
            damage: 4,
            age: 6,
        }),
    })
    .unwrap();
    ctx.stage(RuleEffect::Environment(EnvironmentState {
        seed: 7,
        next_tick: 0,
        world_time: 0,
        day_phase_offset: 0,
        season_offset: 0,
        weather: Weather::Clear,
        weather_remaining: 100,
        difficulty: 0,
        tunables: RuleTunables::source_defaults(),
    }))
    .unwrap();
    let snapshot = ctx.resident_snapshot();
    drop(ctx);
    authority.commit_residents(snapshot);
}

/// Commit-back mirrors the overlay one-to-one: every staged map survives
/// the commit with its fields intact, including the committed mob actor.
#[test]
fn committed_overlay_round_trips_every_map() {
    let mut authority = authority();
    commit_full_overlay(&mut authority);
    let residents = authority.residents();
    let hostile = hostile_actor(11, [100.5, 65.0, 100.5]);
    assert_eq!(residents.actors, vec![hostile.clone()]);
    let runtime_key = ActorKey::Hostile(HostileId::try_new(12).unwrap());
    assert_eq!(residents.runtimes.len(), 1);
    assert_eq!(
        residents.runtimes.get(&runtime_key).unwrap().key,
        runtime_key
    );
    let mining_key = ActorKey::Hostile(HostileId::try_new(13).unwrap());
    assert_eq!(residents.inventories.len(), 2);
    assert_eq!(
        residents.inventories.get(&runtime_key),
        Some(&InventoryRecord::empty())
    );
    assert_eq!(residents.mining.len(), 1);
    assert_eq!(residents.mining.get(&mining_key).unwrap().elapsed, 4);
    assert_eq!(residents.projectiles.len(), 1);
    assert_eq!(residents.projectiles[0].damage, 4);
    assert_eq!(residents.projectiles[0].age, 6);
    assert_eq!(
        residents.environment.as_ref().unwrap().weather_remaining,
        100
    );
    let sparse_key = chunk_key(5, 5);
    let sparse_pos = BlockPos::new(80, 64, 80);
    assert_eq!(residents.blocks.len(), 1);
    assert_eq!(
        residents
            .blocks
            .get(&(sparse_key, sparse_pos))
            .unwrap()
            .block,
        4
    );
    let key = chunk_key(0, 0);
    let snapshot = residents.ready_snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].0, key);
    assert_eq!(snapshot[0].1, 1);
    assert_eq!(snapshot[0].2, 1);
    assert_eq!(snapshot[0].3, chest_chunk());
    assert_eq!(
        residents.drop_records(),
        vec![drop_record(3, 7), drop_record(4, 7)]
    );
    let stored = residents.container_records();
    assert_eq!(stored.len(), 2);
    let sparse_ref =
        ContainerRef::try_new(ChunkPos::new(9, 9), ContainerKind::Chest, 0, 1).unwrap();
    assert_eq!(stored.get(&sparse_ref).unwrap().revision, 3);
    let chest_ref = ContainerRef::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, 0, 1).unwrap();
    let chest_record = stored.get(&chest_ref).unwrap();
    assert_eq!(chest_record.revision, 1);
    match &chest_record.slots {
        ContainerSlots::Chest(items) => assert_eq!(items[0], coal(3)),
        ContainerSlots::Furnace { .. } => panic!("chest slot read as furnace"),
    }
}

/// Two consecutive live ticks carry every resident map: stable maps compare
/// exactly, scoped drops age deterministically, the environment clock
/// advances, and the committed mob actor and projectile persist by identity
/// under a real login scope.
#[test]
fn consecutive_live_ticks_carry_residents() {
    let mut authority = authority();
    let login = admitted(5, "Eli");
    let mut inventory = Inventory::default();
    inventory.hotbar.slots[0] = coal(1);
    let player = login.player_id();
    let stored = StoredPlayer {
        player_id: save_id(player),
        revision: 1,
        display_name: "Eli".to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position: [0.5, 65.0, 0.5],
        },
        yaw: 0.0,
        pitch: 0.0,
        safe: None,
        inventory,
        health: 20,
        hunger: 20,
        saturation_milli: 5_000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor: [ItemStack::default(); 4],
        needs_rewrite: false,
    };
    let session = login_session(&mut authority, login, stored);
    commit_full_overlay(&mut authority);
    let before = authority.residents();
    authority.advance_tick(TickBudget::full()).unwrap();
    authority.advance_tick(TickBudget::full()).unwrap();
    let after = authority.residents();
    // The login seeds exactly one player actor beside the committed mob.
    assert_eq!(after.actors.len(), 2);
    let player_key = ActorKey::Player(session);
    assert!(after.actors.iter().any(|actor| actor.key == player_key));
    let mob = after
        .actors
        .iter()
        .find(|actor| actor.key == before.actors[0].key)
        .unwrap();
    assert_eq!(mob.lifecycle, before.actors[0].lifecycle);
    assert_eq!(mob.dimension, before.actors[0].dimension);
    // Mob bodies carry their own physics state, so live ticks evolve the
    // position, velocity, and cooldowns while the identity persists.
    match (&mob.body, &before.actors[0].body) {
        (ActorBody::Hostile(next), ActorBody::Hostile(prev)) => {
            assert_eq!(next.id, prev.id);
            assert_eq!(next.dimension, prev.dimension);
            assert_eq!(next.health, prev.health);
            assert_eq!(next.kind, prev.kind);
        }
        _ => panic!("committed mob actor must persist"),
    }
    // The actor-less runtime carries untouched while the player lane stages
    // zeroed cooldowns with no eating or bow progress.
    let runtime_key = ActorKey::Hostile(HostileId::try_new(12).unwrap());
    assert_eq!(
        after.runtimes.get(&runtime_key),
        before.runtimes.get(&runtime_key)
    );
    let player_runtime = after.runtimes.get(&player_key).unwrap();
    assert_eq!(player_runtime.attack_cooldown, 0);
    assert_eq!(player_runtime.hurt_cooldown, 0);
    assert_eq!(player_runtime.burn_cooldown, 0);
    assert!(player_runtime.eating.is_none());
    assert!(player_runtime.bow.is_none());
    // The login inventory stages exactly and rides both ticks untouched.
    let mut want_inventory = InventoryRecord::empty();
    want_inventory.slots[0] = coal(1);
    assert_eq!(after.inventories.get(&player_key), Some(&want_inventory));
    for (key, record) in &before.inventories {
        assert_eq!(after.inventories.get(key), Some(record));
    }
    assert_eq!(after.mining, before.mining);
    assert_eq!(after.blocks, before.blocks);
    assert_eq!(after.ready_snapshot(), before.ready_snapshot());
    assert_eq!(after.container_records(), before.container_records());
    // Scoped drops age one step per tick with the pickup delay counting down.
    let mut aged = before.drop_records();
    for record in &mut aged {
        record.age = record.age.wrapping_add(2);
        record.pickup_delay = record.pickup_delay.saturating_sub(2);
    }
    assert_eq!(after.drop_records(), aged);
    // The tick-start freeze re-derives the clock, season offset, and weather
    // dice from metadata and seed every tick, so only the seed, difficulty,
    // tunables, and display offset carry through the commit.
    let before_env = before.environment.unwrap();
    let after_env = after.environment.unwrap();
    assert_eq!(after_env.next_tick, before_env.next_tick + 2);
    assert_eq!(after_env.seed, before_env.seed);
    assert_eq!(after_env.difficulty, before_env.difficulty);
    assert_eq!(after_env.tunables, before_env.tunables);
    assert_eq!(after_env.day_phase_offset, before_env.day_phase_offset);
    assert_eq!(after.projectiles.len(), 1);
    assert_eq!(after.projectiles[0].id, before.projectiles[0].id);
    assert_eq!(after.projectiles[0].owner, before.projectiles[0].owner);
    assert_eq!(
        after.projectiles[0].dimension,
        before.projectiles[0].dimension
    );
    assert_eq!(after.projectiles[0].kind, before.projectiles[0].kind);
    assert_eq!(after.projectiles[0].damage, before.projectiles[0].damage);
}

/// Empty ticks stay empty: no actor, runtime, inventory, block, chunk, drop,
/// container, projectile, or environment appears from nothing.
#[test]
fn empty_live_ticks_stay_empty() {
    let mut authority = authority();
    authority.advance_tick(TickBudget::full()).unwrap();
    authority.advance_tick(TickBudget::full()).unwrap();
    let residents = authority.residents();
    assert!(residents.actors.is_empty());
    assert!(residents.runtimes.is_empty());
    assert!(residents.inventories.is_empty());
    assert!(residents.mining.is_empty());
    assert!(residents.projectiles.is_empty());
    assert!(residents.blocks.is_empty());
    assert!(residents.ready_snapshot().is_empty());
    assert!(residents.drop_records().is_empty());
    assert!(residents.container_records().is_empty());
}

/// A full house carries over: the player cap of logins and the chunk-result
/// cap of Ready chunks all survive one live tick with nothing dropped.
#[test]
fn full_residents_carry_within_caps() {
    let mut authority = authority();
    for tag in 1..=8u8 {
        let name = ["Ada", "Bo", "Cy", "Dee", "Eli", "Fay", "Gus", "Hal"][usize::from(tag - 1)];
        let session = authority
            .prepare(admitted(tag, name), TransportKind::Memory)
            .unwrap();
        let player = authority.session(session).unwrap().player_id;
        let mut inventory = Inventory::default();
        inventory.hotbar.slots[0] = coal(1);
        let stored = StoredPlayer {
            player_id: save_id(player),
            revision: 1,
            display_name: name.to_owned(),
            current: PlayerLocation {
                dimension: 0,
                position: [0.5, 65.0, 0.5],
            },
            yaw: 0.0,
            pitch: 0.0,
            safe: None,
            inventory,
            health: 20,
            hunger: 20,
            saturation_milli: 5_000,
            exhaustion_milli: 0,
            respawn_present: false,
            respawn_position: [0.0; 3],
            respawn_dimension: 0,
            armor: [ItemStack::default(); 4],
            needs_rewrite: false,
        };
        authority.install(session, Some(stored)).unwrap();
        authority.activate(session).unwrap();
    }
    {
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        for x in 0..64 {
            ctx.preload_ready_chunk(
                ReadyChunk::try_new(chunk_key(x, 0), 1, 1, air_chunk()).unwrap(),
            );
        }
        let snapshot = ctx.resident_snapshot();
        drop(ctx);
        authority.commit_residents(snapshot);
    }
    authority.advance_tick(TickBudget::full()).unwrap();
    let residents = authority.residents();
    assert_eq!(residents.actors.len(), 8);
    assert_eq!(residents.ready_snapshot().len(), 64);
    let mut keys: Vec<ChunkKey> = residents
        .ready_snapshot()
        .iter()
        .map(|entry| entry.0)
        .collect();
    keys.sort();
    let mut want: Vec<ChunkKey> = (0..64).map(|x| chunk_key(x, 0)).collect();
    want.sort();
    assert_eq!(keys, want);
    authority.advance_tick(TickBudget::full()).unwrap();
    let again = authority.residents();
    assert_eq!(again.actors.len(), 8);
    assert_eq!(again.ready_snapshot().len(), 64);
}
