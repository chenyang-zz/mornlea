//! Source-bound projectile flight and bow draw replay.

use super::*;
use mornlea_domain::{
    BlockPos, ChunkPos, CombatTarget, Command, CommandEnvelope, CommandEnvelopeParts, Dimension,
    Event, EventRecipient, FiniteVec3, HeldActions, HotbarSlot, LookAngles, MotionState,
    MotionStateParts, Movement, PlayerControl, PlayerControlParts, ProjectileId, ProjectileKind,
    Season, SurvivalState, SurvivalStateParts, Weather, WorldState, WorldStateParts,
};
use mornlea_server::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, BlockObservation,
    BowProgress, ChunkKey, EatingProgress, EnvironmentState, InventoryRecord, MiningProgress,
    ProjectileRecord, RuleEffect, RulePhase, RuleTunables, SessionKey, TransportKind,
};
use mornlea_server::rules::projectiles as provider;
use mornlea_storage::{ItemStack, PlayerLocation, PlayerSave};

fn scope(dimension: Dimension, center: ChunkPos, radius: u64) -> provider::ProjectileScope {
    provider::ProjectileScope {
        dimension,
        center,
        radius,
    }
}

fn projectile(id: u64, age: u32) -> ProjectileRecord {
    ProjectileRecord {
        id: ProjectileId::try_new(id).expect("projectile id"),
        owner: ActorKey::Hostile(mornlea_domain::HostileId::try_new(21).expect("hostile id")),
        dimension: Dimension::OVERWORLD,
        position: FiniteVec3::try_new([0.5, 30.0, 0.5]).expect("position"),
        velocity: FiniteVec3::try_new([22.0, 0.0, 0.0]).expect("velocity"),
        kind: ProjectileKind::Shard,
        damage: 3,
        age,
    }
}

fn projectile_step() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::ProjectileStep,
        actor: None,
        command: None,
        internal: None,
    }
}

fn world() -> WorldState {
    WorldState::try_new(WorldStateParts {
        day_phase_offset: 0,
        world_time_ticks: 0,
        weather: Weather::Clear,
        season: Season::Spring,
        season_progress: 0,
        temperature: 0,
    })
    .expect("world")
}

fn observed(x: i32, block: u16) -> BlockObservation {
    BlockObservation::try_new(
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        },
        1,
        1,
        BlockPos::new(x, 1, 0),
        block,
    )
    .expect("block observation")
}

fn air_at(x: i32, y: i32) -> BlockObservation {
    block_at(x, y, 0)
}

fn block_at(x: i32, y: i32, block: u16) -> BlockObservation {
    BlockObservation::try_new(
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(x.div_euclid(16), 0),
        },
        1,
        1,
        BlockPos::new(x, y, 0),
        block,
    )
    .expect("block observation")
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn multiple_players() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        7,
    )
    .expect("authority")
}

fn admit_session(authority: &mut AuthorityState) -> SessionKey {
    admit_session_tag(authority, 1)
}

fn admit_session_tag(authority: &mut AuthorityState, tag: u8) -> SessionKey {
    let id = mornlea_domain::PlayerId::try_from_bytes(uuid(tag)).expect("player id");
    let start = mornlea_protocol::LoginStart::new(id, "Bow", 8).expect("login");
    let inbound = mornlea_protocol::LoginStart::decode_inbound(&start.encode().expect("frame"))
        .expect("inbound");
    let login = mornlea_protocol::admit_login(inbound).expect("admitted");
    authority
        .admit(login, TransportKind::Memory)
        .expect("session")
}

fn player(session: SessionKey) -> ActorRecord {
    let position = [0.5, 64.0, 0.5];
    ActorRecord::try_new(
        ActorKey::Player(session),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("position"),
            velocity: FiniteVec3::try_new([0.0; 3]).expect("velocity"),
            on_ground: true,
        }),
        LookAngles::try_new(0.0, 0.0).expect("look"),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .expect("survival"),
        ActorBody::Player(PlayerSave {
            player_id: mornlea_storage::PlayerId::from_bytes(uuid(1)),
            revision: 1,
            display_name: "Bow".to_owned(),
            current: PlayerLocation {
                dimension: 0,
                position,
            },
            yaw: 0.0,
            pitch: 0.0,
            safe: None,
            inventory: mornlea_storage::Inventory::default(),
            health: 20,
            hunger: 20,
            saturation_milli: 5_000,
            exhaustion_milli: 0,
            respawn_present: false,
            respawn_position: [0.0; 3],
            respawn_dimension: 0,
            armor: [ItemStack::default(); 4],
        }),
    )
    .expect("player")
}

fn hostile(id: u64, x: f32, dimension: Dimension, lifecycle: ActorLifecycle) -> ActorRecord {
    let position = [x, 1.0, 0.5];
    ActorRecord::try_new(
        ActorKey::Hostile(mornlea_domain::HostileId::try_new(id).expect("hostile id")),
        lifecycle,
        dimension,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("position"),
            velocity: FiniteVec3::try_new([0.0; 3]).expect("velocity"),
            on_ground: true,
        }),
        LookAngles::try_new(0.0, 0.0).expect("look"),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .expect("survival"),
        ActorBody::Hostile(mornlea_storage::HostileMob {
            id,
            dimension: i32::from(dimension.get()),
            position,
            velocity: [0.0; 3],
            on_ground: true,
            yaw: 0.0,
            health: 20,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 20,
            has_target: false,
            player_id: mornlea_storage::PlayerId::from_bytes([0; 16]),
            next_repath_ticks: 0,
            distant_ticks: 0,
            kind: 0,
        }),
    )
    .expect("hostile")
}

fn player_target(session: SessionKey, tag: u8, x: f32) -> ActorRecord {
    let mut actor = player(session);
    let position = [x, 1.0, 0.5];
    actor.motion = MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new(position).expect("target position"),
        velocity: FiniteVec3::try_new([0.0; 3]).expect("target velocity"),
        on_ground: true,
    });
    let ActorBody::Player(body) = &mut actor.body else {
        unreachable!()
    };
    body.player_id = mornlea_storage::PlayerId::from_bytes(uuid(tag));
    body.current.position = position;
    actor
}

fn passive(id: u64, x: f32) -> ActorRecord {
    let position = [x, 1.0, 0.5];
    ActorRecord::try_new(
        ActorKey::Passive(mornlea_domain::PassiveId::try_new(id).expect("passive id")),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("position"),
            velocity: FiniteVec3::try_new([0.0; 3]).expect("velocity"),
            on_ground: true,
        }),
        LookAngles::try_new(0.0, 0.0).expect("look"),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .expect("survival"),
        ActorBody::Passive(mornlea_storage::PassiveMob {
            id,
            dimension: 0,
            position,
            velocity: [0.0; 3],
            on_ground: true,
            yaw: 0.0,
            health: 20,
        }),
    )
    .expect("passive")
}

fn passive_runtime(key: ActorKey) -> ActorRuntime {
    let ActorKey::Passive(_) = key else {
        unreachable!()
    };
    ActorRuntime {
        key,
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 7,
        hurt_cooldown: 8,
        burn_cooldown: 9,
        oxygen: 300,
        peak_y: 1.0,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Passive {
            home: BlockPos::new(0, 1, 0),
            flee_ticks: 3,
            flee_from: None,
            graze_ticks: 12,
            graze_at: Some(BlockPos::new(4, 1, 4)),
            fresh: true,
        },
    }
}

fn wounded(mut actor: ActorRecord, health: u8) -> ActorRecord {
    actor.survival = SurvivalState::try_new(SurvivalStateParts {
        health,
        oxygen: actor.survival.oxygen(),
        hunger: actor.survival.hunger(),
        saturation_zero: actor.survival.saturation_zero(),
        armor_points: actor.survival.armor_points(),
    })
    .expect("wounded survival");
    match &mut actor.body {
        ActorBody::Player(body) => body.health = health,
        ActorBody::Hostile(body) => body.health = health,
        ActorBody::Passive(body) => body.health = health,
        ActorBody::Companion(_) => unreachable!(),
    }
    actor
}

fn arrow(id: u64, owner: SessionKey) -> ProjectileRecord {
    let mut record = projectile(id, 0);
    record.owner = ActorKey::Player(owner);
    record.kind = ProjectileKind::Arrow;
    record.damage = 5;
    record.position = FiniteVec3::try_new([0.5, 1.9, 0.5]).expect("position");
    record
}

fn control(primary: bool) -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: 0,
            move_z: 0,
            jump: false,
        },
        look: LookAngles::try_new(0.0, 0.0).expect("look"),
        actions: HeldActions {
            primary,
            eating: false,
            sprinting: false,
            sneaking: false,
        },
    })
}

fn bow_runtime(actor: ActorKey, primary: bool, ticks: Option<u16>) -> ActorRuntime {
    ActorRuntime {
        key: actor,
        controls: Some(control(primary)),
        has_view: true,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 300,
        peak_y: 64.0,
        exhaustion_milli: 0,
        saturation_milli: 5_000,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: ticks.map(|ticks| BowProgress {
            slot: HotbarSlot::new(0).expect("slot"),
            ticks,
        }),
        path: None,
        aux: ActorAux::Player {
            respawn: None,
            workbench: None,
        },
    }
}

fn bow_scene(
    ctx: &mut TickContext<'_>,
    session: SessionKey,
    primary: bool,
    ticks: Option<u16>,
) -> ActorKey {
    let actor = ActorKey::Player(session);
    ctx.stage(RuleEffect::Actor(player(session)))
        .expect("actor");
    ctx.stage(RuleEffect::Runtime(bow_runtime(actor, primary, ticks)))
        .expect("runtime");
    ctx.stage(RuleEffect::Environment(EnvironmentState {
        seed: 7,
        next_tick: 1,
        world_time: 0,
        day_phase_offset: 0,
        season_offset: 0,
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty: 1,
        tunables: RuleTunables::source_defaults(),
    }))
    .expect("environment");
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = ItemStack {
        item: 62,
        count: 1,
        durability: 120,
    };
    inventory.slots[1] = ItemStack {
        item: 63,
        count: 3,
        durability: 0,
    };
    inventory.slots[10] = ItemStack {
        item: 63,
        count: 2,
        durability: 0,
    };
    ctx.preload_inventory(actor, inventory);
    actor
}

fn bow_call(actor: ActorKey) -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::BowDraw,
        actor: Some(actor),
        command: None,
        internal: None,
    }
}

/// `TestProjectileIntegratesGravityBeforeStepEachTick` pins 18 m/s² and
/// 0.05 seconds; the first live step applies gravity before displacement.
#[test]
fn gravity_first_lifetime() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(projectile(1, 0)),
    })
    .expect("initial projectile");
    provider::advance(
        &mut ctx,
        &[scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 1)],
    )
    .expect("first flight step");
    let current = ctx.snapshot_state(world());
    let first_vy = 0.0f32 - 18.0f32 * 0.05f32;
    assert_eq!(current.projectiles[0].velocity.get(), [22.0, first_vy, 0.0]);
    assert_eq!(
        current.projectiles[0].position.get(),
        [0.5 + 22.0 * 0.05, 30.0 + first_vy * 0.05, 0.5]
    );
    assert_eq!(current.projectiles[0].age, 1);
}

/// `TestProjectileDespawnsWhenLifetimeExhausted` pins removal at the
/// next phase entry after one hundred completed flight steps.
#[test]
fn age_100_expires_at_entry() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(projectile(1, 100)),
    })
    .expect("initial projectile");
    provider::advance(
        &mut ctx,
        &[scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 1)],
    )
    .expect("expiry step");
    assert!(ctx.snapshot_state(world()).projectiles.is_empty());
}

/// `TestProjectileDespawnsWhenNoSessionSubscribesChunk` and
/// `TestProjectileDespawnRuleIgnoresOtherDimensionSessions` pin that
/// visibility uses ready session squares and the projectile dimension.
#[test]
fn orphan_and_other_dimension_scope_remove() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(projectile(1, 0)),
    })
    .expect("initial projectile");
    provider::advance(&mut ctx, &[]).expect("orphan step");
    assert!(ctx.snapshot_state(world()).projectiles.is_empty());

    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(projectile(2, 0)),
    })
    .expect("second projectile");
    provider::advance(
        &mut ctx,
        &[scope(Dimension::DEPTHS, ChunkPos::new(0, 0), 1)],
    )
    .expect("other dimension step");
    assert!(ctx.snapshot_state(world()).projectiles.is_empty());
}

/// The source has at most eight players. Scope overflow refuses before any
/// projectile moves; a large radius is compared arithmetically, never
/// expanded into an unbounded chunk collection.
#[test]
fn nine_scopes_refuse_without_mutation() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(projectile(1, 0)),
    })
    .expect("initial projectile");
    let before = ctx.snapshot_state(world()).projectiles;
    let scopes = [scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 1); 9];
    assert!(provider::advance(&mut ctx, &scopes).is_err());
    assert_eq!(ctx.snapshot_state(world()).projectiles, before);
}

#[test]
fn large_scope_is_arithmetic_and_overflow_refuses() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(projectile(1, 0)),
    })
    .expect("projectile");
    let huge = scope(
        Dimension::OVERWORLD,
        ChunkPos::new(i32::MIN, i32::MAX),
        i64::MAX as u64,
    );
    provider::advance(&mut ctx, &[huge]).expect("large radius");
    assert_eq!(ctx.snapshot_state(world()).projectiles.len(), 1);
    let before = ctx.snapshot_state(world()).projectiles;
    let invalid = scope(
        Dimension::OVERWORLD,
        ChunkPos::new(0, 0),
        i64::MAX as u64 + 1,
    );
    assert!(provider::advance(&mut ctx, &[invalid]).is_err());
    assert_eq!(ctx.snapshot_state(world()).projectiles, before);
}

/// A long malformed flight reaches the fixed DDA work ceiling after 512
/// known air cells. Capacity is explicit, and the unprocessed projectile
/// stays at its exact prior state with no hit intent.
#[test]
fn ray_work_overflow_retains_projectile() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let mut record = projectile(1, 0);
    record.position = FiniteVec3::try_new([0.5, 30.5, 0.5]).expect("position");
    record.velocity = FiniteVec3::try_new([30_000.0, 0.0, 0.0]).expect("velocity");
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(record.clone()),
    })
    .expect("projectile");
    for x in 0..=512 {
        ctx.preload_block(air_at(x, 30));
    }
    let result = provider::advance(
        &mut ctx,
        &[scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 100)],
    )
    .map(|outcome| outcome.report);
    assert_eq!(
        result,
        Err(mornlea_server::contracts::ServerError::Capacity {
            resource: mornlea_server::contracts::Resource::RuleEffects,
            limit: 512,
            observed: 513,
        })
    );
    assert_eq!(ctx.snapshot_state(world()).projectiles, vec![record]);
    assert!(ctx.read().damage_intents().is_empty());
}

/// Finishing on the last allowed DDA cell is a completed flight, not an
/// overflow; the next cell would require another visit and must refuse.
#[test]
fn ray_exactly_512_cells_can_complete() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let mut record = projectile(1, 0);
    record.position = FiniteVec3::try_new([0.5, 30.5, 0.5]).expect("position");
    record.velocity = FiniteVec3::try_new([10_218.0, 0.0, 0.0]).expect("velocity");
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(record),
    })
    .expect("projectile");
    for x in 0..=511 {
        ctx.preload_block(air_at(x, 30));
    }
    provider::advance(
        &mut ctx,
        &[scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 100)],
    )
    .expect("complete boundary ray");
    let after = ctx.snapshot_state(world()).projectiles;
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].age, 1);
    assert_eq!(after[0].position.get()[0], 0.5 + 10_218.0 * 0.05);
}

/// Finite velocity can still overflow f32 squared length. With known air
/// along the actual walk, the DDA must still report explicit capacity.
#[test]
fn huge_finite_velocity_refuses_without_mutation() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let mut record = projectile(1, 0);
    record.position = FiniteVec3::try_new([0.5, 30.5, 0.5]).expect("position");
    record.velocity = FiniteVec3::try_new([1.0e30, 0.0, 0.0]).expect("finite velocity");
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(record.clone()),
    })
    .expect("projectile");
    for x in 0..=512 {
        ctx.preload_block(air_at(x, 30));
    }
    assert_eq!(
        provider::advance(
            &mut ctx,
            &[scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 0)],
        )
        .map(|outcome| outcome.report),
        Err(mornlea_server::contracts::ServerError::Capacity {
            resource: mornlea_server::contracts::Resource::RuleEffects,
            limit: 512,
            observed: 513,
        })
    );
    assert_eq!(ctx.snapshot_state(world()).projectiles, vec![record]);
    assert!(ctx.read().damage_intents().is_empty());
}

/// A long segment can terminate before the work ceiling at a near wall or
/// an unavailable cell; neither outcome consumes an unvisited-cell budget.
#[test]
fn long_ray_observes_early_wall_and_unknown() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut record = projectile(1, 0);
    record.position = FiniteVec3::try_new([0.5, 30.5, 0.5]).expect("position");
    record.velocity = FiniteVec3::try_new([30_000.0, 0.0, 0.0]).expect("velocity");
    let scopes = [scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 100)];

    let mut wall = TickContext::harness(&mut authority, TickBudget::full());
    wall.stage(RuleEffect::Projectile {
        before: None,
        after: Some(record.clone()),
    })
    .expect("projectile");
    wall.preload_block(air_at(0, 30));
    wall.preload_block(block_at(1, 30, 2));
    provider::advance(&mut wall, &scopes).expect("early wall");
    assert!(wall.snapshot_state(world()).projectiles.is_empty());

    let mut unknown = TickContext::harness(&mut authority, TickBudget::full());
    unknown
        .stage(RuleEffect::Projectile {
            before: None,
            after: Some(record),
        })
        .expect("projectile");
    provider::advance(&mut unknown, &scopes).expect("early unknown");
    assert_eq!(unknown.snapshot_state(world()).projectiles.len(), 1);
}

#[test]
fn projectile_phase_rejects_actor_payload() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    assert_eq!(
        provider::run(&mut ctx, projectile_step())
            .expect("shape")
            .examined,
        0
    );
    let call = RuleCall {
        actor: Some(ActorKey::Hostile(
            mornlea_domain::HostileId::try_new(21).expect("hostile id"),
        )),
        ..projectile_step()
    };
    assert!(provider::run(&mut ctx, call).is_err());
}

/// `TestProjectileSetKeepsIDsStrictlySortedWithOldestEviction` pins a
/// production spawn at capacity: the minimum identity is evicted, while a
/// direct defensive insertion remains a capacity refusal.
#[test]
fn spawn_129_evicts_1() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    for id in 1..=128 {
        ctx.stage(RuleEffect::Projectile {
            before: None,
            after: Some(projectile(id, 0)),
        })
        .expect("initial bounded projectile");
    }
    assert!(
        ctx.stage(RuleEffect::Projectile {
            before: None,
            after: Some(projectile(129, 0)),
        })
        .is_err()
    );
    let before_invalid = ctx.snapshot_state(world()).projectiles;
    let mut invalid = projectile(129, 0);
    invalid.damage = 0;
    assert!(provider::spawn(&mut ctx, invalid).is_err());
    assert_eq!(ctx.snapshot_state(world()).projectiles, before_invalid);
    let aged_birth = projectile(129, 1);
    assert!(provider::spawn(&mut ctx, aged_birth).is_err());
    assert_eq!(ctx.snapshot_state(world()).projectiles, before_invalid);
    let mut oversized_hit = projectile(129, 0);
    oversized_hit.damage = 21;
    assert!(provider::spawn(&mut ctx, oversized_hit).is_err());
    assert_eq!(ctx.snapshot_state(world()).projectiles, before_invalid);
    provider::spawn(&mut ctx, projectile(129, 0)).expect("production spawn");
    let ids: Vec<_> = ctx
        .snapshot_state(world())
        .projectiles
        .iter()
        .map(|record| record.id.get())
        .collect();
    assert_eq!(ids, (2..=129).collect::<Vec<_>>());
}

/// `TestProjectileDespawnsWhenSegmentEntersSolidBlock` pins the first
/// segment crossing x=1 as a wall hit. An unknown origin cell makes the
/// ray unresolved and cannot manufacture a wall hit.
#[test]
fn wall_hit_and_unknown_ray() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let mut initial = projectile(1, 0);
    initial.position = FiniteVec3::try_new([0.5, 1.5, 0.5]).expect("position");
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(initial.clone()),
    })
    .expect("initial projectile");
    ctx.preload_block(observed(0, 0));
    ctx.preload_block(observed(1, 2));
    provider::advance(
        &mut ctx,
        &[scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 0)],
    )
    .expect("wall step");
    assert!(ctx.snapshot_state(world()).projectiles.is_empty());

    let mut second = TickContext::harness(&mut authority, TickBudget::full());
    second
        .stage(RuleEffect::Projectile {
            before: None,
            after: Some(initial),
        })
        .expect("second projectile");
    second.preload_block(observed(1, 2));
    provider::advance(
        &mut second,
        &[scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 0)],
    )
    .expect("unresolved ray step");
    assert_eq!(second.snapshot_state(world()).projectiles.len(), 1);
}

/// `TestBowReleaseTierBoundaries` pins the first hold as tick one, a release
/// before six as silent, six through nineteen as speed sixteen/damage two,
/// and twenty as speed thirty/damage five. Ammo scans slot one before ten.
#[test]
fn bow_5_6_19_20_and_first_hold() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let session = admit_session(&mut authority);
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = bow_scene(&mut ctx, session, true, None);
    provider::run(&mut ctx, bow_call(actor)).expect("first hold");
    assert_eq!(
        ctx.read()
            .runtime(actor)
            .expect("runtime")
            .bow
            .expect("progress")
            .ticks,
        1
    );
    assert_eq!(
        ctx.read().inventory(actor).expect("inventory").slots[1].count,
        3
    );

    for (ticks, damage, speed) in [
        (5, None, None),
        (6, Some(2), Some(16.0)),
        (19, Some(2), Some(16.0)),
        (20, Some(5), Some(30.0)),
    ] {
        let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
        let session = admit_session(&mut authority);
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        let actor = bow_scene(&mut ctx, session, false, Some(ticks));
        provider::run(&mut ctx, bow_call(actor)).expect("release");
        assert!(ctx.read().runtime(actor).expect("runtime").bow.is_none());
        let inventory = ctx.read().inventory(actor).copied().expect("inventory");
        let records = ctx.snapshot_state(world()).projectiles;
        if let Some(damage) = damage {
            assert_eq!(records.len(), 1, "draw {ticks}");
            assert_eq!(records[0].damage, damage, "draw {ticks}");
            let velocity = records[0].velocity.get();
            assert_eq!(velocity, [0.0, 0.0, -speed.expect("speed")], "draw {ticks}");
            assert_eq!(inventory.slots[1].count, 2, "draw {ticks}");
            assert_eq!(inventory.slots[10].count, 2, "draw {ticks}");
            assert_eq!(inventory.slots[0].durability, 119, "draw {ticks}");
        } else {
            assert!(records.is_empty(), "draw {ticks}");
            assert_eq!(inventory.slots[1].count, 3, "draw {ticks}");
            assert_eq!(inventory.slots[0].durability, 120, "draw {ticks}");
        }
    }
}

/// `advanceBowDraw` debits ammo and wear before the source spawn hash's
/// bounded collision chain. Exhausting all sixty-four candidate identities
/// leaves the debit settled and clears draw progress.
#[test]
fn bow_defensive_spawn_refusal_keeps_debit() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let session = admit_session(&mut authority);
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = bow_scene(&mut ctx, session, false, Some(20));
    let position = FiniteVec3::try_new([
        0.5,
        64.0 + RuleTunables::source_defaults().eye_height(),
        0.5,
    ])
    .expect("eye");
    assert_eq!(ctx.read().tick(), 0);
    // Go `updates.Sampler.ProjectileSpawnHash(7,0,Overworld,Arrow,1,eye)`.
    assert_eq!(
        provider::derive_spawn_id(
            &ctx,
            ProjectileKind::Arrow,
            Dimension::OVERWORLD,
            actor,
            position
        )
        .expect("source hash")
        .get(),
        10_170_434_196_450_693_864,
    );
    for _ in 0..64 {
        let id = provider::derive_spawn_id(
            &ctx,
            ProjectileKind::Arrow,
            Dimension::OVERWORLD,
            actor,
            position,
        )
        .expect("candidate id");
        ctx.stage(RuleEffect::Projectile {
            before: None,
            after: Some(ProjectileRecord {
                id,
                owner: actor,
                dimension: Dimension::OVERWORLD,
                position,
                velocity: FiniteVec3::try_new([0.0; 3]).expect("velocity"),
                kind: ProjectileKind::Arrow,
                damage: 2,
                age: 0,
            }),
        })
        .expect("occupy hash candidate");
    }
    provider::run(&mut ctx, bow_call(actor)).expect("release");
    assert_eq!(ctx.snapshot_state(world()).projectiles.len(), 64);
    let inventory = ctx.read().inventory(actor).copied().expect("inventory");
    assert_eq!(inventory.slots[1].count, 2);
    assert_eq!(inventory.slots[0].durability, 119);
    assert!(ctx.read().runtime(actor).expect("runtime").bow.is_none());
}

/// `TestBowLastDurabilityPointSwapsToBrokenForm` and the shared tool-wear
/// branch both replace a bow at one or historical zero durability.
#[test]
fn bow_last_and_zero_durability_break() {
    for durability in [1, 0] {
        let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
        let session = admit_session(&mut authority);
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        let actor = bow_scene(&mut ctx, session, false, Some(6));
        let mut inventory = ctx.read().inventory(actor).copied().expect("inventory");
        inventory.slots[0].durability = durability;
        ctx.preload_inventory(actor, inventory);
        provider::run(&mut ctx, bow_call(actor)).expect("release");
        let after = ctx.read().inventory(actor).copied().expect("inventory");
        assert_eq!(after.slots[0].item, 65, "durability {durability}");
        assert_eq!(after.slots[0].durability, 0, "durability {durability}");
        assert_eq!(after.slots[1].count, 2, "durability {durability}");
        assert_eq!(ctx.snapshot_state(world()).projectiles.len(), 1);
    }
}

/// `TestBowNeverStartsWithoutArrow` and the release interruption matrix pin
/// read-only entry admission and interruption precedence over any debit.
#[test]
fn bow_no_ammo_and_release_interruptions() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let session = admit_session(&mut authority);
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = bow_scene(&mut ctx, session, true, None);
    let mut inventory = ctx.read().inventory(actor).copied().expect("inventory");
    inventory.slots[1] = ItemStack::default();
    inventory.slots[10] = ItemStack::default();
    ctx.preload_inventory(actor, inventory);
    provider::run(&mut ctx, bow_call(actor)).expect("empty quiver");
    assert!(ctx.read().runtime(actor).expect("runtime").bow.is_none());
    assert!(ctx.snapshot_state(world()).projectiles.is_empty());

    for interruption in 0..3 {
        let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
        let session = admit_session(&mut authority);
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        let actor = bow_scene(&mut ctx, session, false, Some(20));
        match interruption {
            0 => {
                let mut runtime = ctx.read().runtime(actor).cloned().expect("runtime");
                runtime.reset = true;
                ctx.stage(RuleEffect::Runtime(runtime)).expect("reset");
            }
            1 => {
                let mut runtime = ctx.read().runtime(actor).cloned().expect("runtime");
                runtime.has_view = false;
                ctx.stage(RuleEffect::Runtime(runtime)).expect("view lost");
            }
            _ => {
                let inventory = ctx.read().inventory(actor).copied().expect("inventory");
                ctx.preload_inventory(
                    actor,
                    inventory.with_selected(HotbarSlot::new(1).expect("slot")),
                );
            }
        }
        provider::run(&mut ctx, bow_call(actor)).expect("interrupted release");
        assert!(ctx.read().runtime(actor).expect("runtime").bow.is_none());
        assert!(ctx.snapshot_state(world()).projectiles.is_empty());
        let after = ctx.read().inventory(actor).copied().expect("inventory");
        assert_eq!(after.slots[0].durability, 120);
        assert_eq!(after.slots[1].count, 3);
    }
}

/// Go `LookDirection(1.234,0.321).Mul(30)` uses f64 trig rounded to f32
/// before the f32 multiplications; the three frozen bits are source output.
#[test]
fn bow_direction_matches_go_bits() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let session = admit_session(&mut authority);
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = bow_scene(&mut ctx, session, false, Some(20));
    let mut current = ctx.read().actor(actor).cloned().expect("player");
    current.look = LookAngles::try_new(1.234, 0.321).expect("look");
    ctx.stage(RuleEffect::Actor(current)).expect("new look");
    provider::run(&mut ctx, bow_call(actor)).expect("release");
    let projectiles = ctx.snapshot_state(world()).projectiles;
    assert_eq!(projectiles.len(), 1);
    assert_eq!(
        projectiles[0].velocity.get().map(f32::to_bits),
        [0xc1d6_f22d, 0x4117_7290, 0xc116_8556],
    );
}

/// A current moved target wins over its stale pose. The two equal-distance
/// hostiles resolve to the lower stable ID and emit one damage receipt even
/// when the batch entry is called a second time after removal.
#[test]
fn moving_target_and_duplicate_collision() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let owner = admit_session(&mut authority);
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    ctx.stage(RuleEffect::Actor(hostile(
        22,
        4.0,
        Dimension::OVERWORLD,
        ActorLifecycle::Active,
    )))
    .expect("old pose");
    ctx.stage(RuleEffect::Actor(hostile(
        22,
        1.8,
        Dimension::OVERWORLD,
        ActorLifecycle::Active,
    )))
    .expect("moved pose");
    ctx.stage(RuleEffect::Actor(hostile(
        21,
        1.8,
        Dimension::OVERWORLD,
        ActorLifecycle::Active,
    )))
    .expect("equal target");
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(arrow(1, owner)),
    })
    .expect("arrow");
    let scopes = [scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 0)];
    provider::advance(&mut ctx, &scopes).expect("hit step");
    assert!(ctx.snapshot_state(world()).projectiles.is_empty());
    let target = ActorKey::Hostile(mornlea_domain::HostileId::try_new(21).expect("id"));
    assert_eq!(
        ctx.read()
            .actor(target)
            .expect("settled target")
            .survival
            .health(),
        15
    );
    assert!(ctx.read().damage_intents().is_empty());
    provider::advance(&mut ctx, &scopes).expect("second entry");
    assert_eq!(
        ctx.read().actor(target).expect("target").survival.health(),
        15
    );
    assert!(ctx.read().damage_intents().is_empty());
}

/// Go settles each hit in ascending projectile ID order; a lethal first hit
/// removes that candidate before the second arrow chooses its target.
#[test]
fn lethal_first_hit_redirects_second_arrow() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let owner = admit_session(&mut authority);
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    ctx.stage(RuleEffect::Actor(wounded(
        hostile(21, 1.3, Dimension::OVERWORLD, ActorLifecycle::Active),
        5,
    )))
    .expect("near target");
    ctx.stage(RuleEffect::Actor(hostile(
        22,
        1.8,
        Dimension::OVERWORLD,
        ActorLifecycle::Active,
    )))
    .expect("far target");
    for id in 1..=2 {
        ctx.stage(RuleEffect::Projectile {
            before: None,
            after: Some(arrow(id, owner)),
        })
        .expect("arrow");
    }
    provider::advance(
        &mut ctx,
        &[scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 0)],
    )
    .expect("ordered impacts");
    let near = ActorKey::Hostile(mornlea_domain::HostileId::try_new(21).expect("id"));
    let far = ActorKey::Hostile(mornlea_domain::HostileId::try_new(22).expect("id"));
    assert_eq!(ctx.read().actor(near).expect("near").survival.health(), 0);
    assert_eq!(ctx.read().actor(far).expect("far").survival.health(), 15);
    assert!(ctx.read().damage_intents().is_empty());
}

/// Player armor freezes before each hit. Durability one reduces the first
/// arrow then expires, so the next arrow sees the updated inventory.
#[test]
fn player_armor_runtime_and_owner_events_settle_serially() {
    let mut authority = multiple_players();
    let owner = admit_session_tag(&mut authority, 1);
    let victim = admit_session_tag(&mut authority, 2);
    authority
        .advance_tick(TickBudget::full())
        .expect("tick one");
    let target = ActorKey::Player(victim);
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    ctx.stage(RuleEffect::Actor(player_target(victim, 2, 1.3)))
        .expect("target");
    let mut inventory = InventoryRecord::empty();
    inventory.armor[0] = ItemStack {
        item: 58,
        count: 1,
        durability: 1,
    };
    ctx.preload_inventory(target, inventory);
    let mut runtime = bow_runtime(target, false, Some(10));
    runtime.since_damage_ticks = 17;
    runtime.attack_cooldown = 9;
    runtime.eating = Some(EatingProgress {
        slot: HotbarSlot::new(0).expect("slot"),
        item: 1,
        ticks: 4,
    });
    ctx.stage(RuleEffect::Runtime(runtime)).expect("runtime");
    for id in 1..=2 {
        ctx.stage(RuleEffect::Projectile {
            before: None,
            after: Some(arrow(id, owner)),
        })
        .expect("arrow");
    }
    provider::advance(
        &mut ctx,
        &[scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 0)],
    )
    .expect("impacts");
    let actor = ctx.read().actor(target).cloned().expect("settled player");
    assert_eq!(actor.survival.health(), 11);
    assert_eq!(actor.motion.velocity().get(), [0.7, 0.0, 0.0]);
    let ActorBody::Player(body) = actor.body else {
        unreachable!()
    };
    assert_eq!(body.health, 11);
    assert_eq!(body.armor[0].durability, 0);
    assert_eq!(
        ctx.read().inventory(target).expect("inventory").armor[0].durability,
        0
    );
    let runtime = ctx.read().runtime(target).expect("runtime");
    assert_eq!(runtime.since_damage_ticks, 0);
    assert!(runtime.eating.is_none());
    assert!(runtime.bow.is_none());
    assert_eq!(runtime.attack_cooldown, 9);
    assert!(ctx.read().damage_intents().is_empty());
    assert_eq!(ctx.events().len(), 2);
    for event in ctx.events() {
        assert_eq!(event.recipient(), EventRecipient::Session(owner.get()));
        let Event::CombatHit(hit) = event.event() else {
            panic!("owner combat confirmation");
        };
        assert_eq!(hit.damage(), 5);
        assert_eq!(hit.target(), CombatTarget::Player);
    }
}

#[test]
fn passive_flee_and_shard_silence() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let owner = admit_session(&mut authority);
    authority
        .advance_tick(TickBudget::full())
        .expect("tick one");
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let target = ActorKey::Passive(mornlea_domain::PassiveId::try_new(31).expect("id"));
    ctx.stage(RuleEffect::Actor(passive(31, 1.3)))
        .expect("passive");
    ctx.stage(RuleEffect::Runtime(passive_runtime(target)))
        .expect("runtime");
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(arrow(1, owner)),
    })
    .expect("arrow");
    provider::advance(
        &mut ctx,
        &[scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 0)],
    )
    .expect("passive impact");
    let actor = ctx.read().actor(target).expect("settled passive");
    assert_eq!(actor.survival.health(), 15);
    assert_eq!(actor.motion.velocity().get(), [0.35, 0.0, 0.0]);
    let ActorBody::Passive(body) = &actor.body else {
        unreachable!()
    };
    assert_eq!(body.health, 15);
    assert_eq!(body.velocity, [0.35, 0.0, 0.0]);
    let ActorAux::Passive {
        flee_ticks,
        flee_from,
        graze_ticks,
        graze_at,
        fresh,
        ..
    } = &ctx.read().runtime(target).expect("runtime").aux
    else {
        unreachable!()
    };
    assert_eq!(*flee_ticks, 60);
    assert_eq!(flee_from.expect("origin").get(), [0.5, 1.9, 0.5]);
    assert_eq!(*graze_ticks, 0);
    assert_eq!(*graze_at, Some(BlockPos::new(4, 1, 4)));
    assert!(*fresh);
    assert_eq!(ctx.events().len(), 1);
    let Event::CombatHit(hit) = ctx.events()[0].event() else {
        panic!("arrow confirmation");
    };
    assert_eq!(hit.target(), CombatTarget::Passive);

    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let victim = admit_session_tag(&mut authority, 2);
    authority
        .advance_tick(TickBudget::full())
        .expect("tick one");
    let mut shard_ctx = TickContext::harness(&mut authority, TickBudget::full());
    let player_key = ActorKey::Player(victim);
    shard_ctx
        .stage(RuleEffect::Actor(player_target(victim, 2, 1.3)))
        .expect("player");
    shard_ctx.preload_inventory(player_key, InventoryRecord::empty());
    shard_ctx
        .stage(RuleEffect::Runtime(bow_runtime(player_key, false, None)))
        .expect("runtime");
    let mut shard = projectile(1, 0);
    shard.position = FiniteVec3::try_new([1.3, 1.9, 0.5]).expect("position");
    shard.velocity = FiniteVec3::try_new([0.0, 20.0, 0.0]).expect("vertical velocity");
    shard_ctx
        .stage(RuleEffect::Projectile {
            before: None,
            after: Some(shard),
        })
        .expect("shard");
    let outcome = provider::advance(
        &mut shard_ctx,
        &[scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 0)],
    )
    .expect("shard impact");
    assert_eq!(
        outcome.damaged_players,
        vec![victim],
        "the health decrease names the victim explicitly"
    );
    let player = shard_ctx.read().actor(player_key).cloned().expect("player");
    assert_eq!(player.survival.health(), 17);
    assert_eq!(player.motion.velocity().get(), [0.0; 3]);
    assert!(shard_ctx.events().is_empty());
}

#[test]
fn missing_impact_lanes_preserve_projectile_and_target() {
    let mut authority = multiple_players();
    let owner = admit_session_tag(&mut authority, 1);
    let victim = admit_session_tag(&mut authority, 2);
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let target = ActorKey::Player(victim);
    ctx.stage(RuleEffect::Actor(player_target(victim, 2, 1.3)))
        .expect("target");
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(arrow(1, owner)),
    })
    .expect("arrow");
    let scopes = [scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 0)];
    let expected = Err(mornlea_server::contracts::ServerError::InvalidInput {
        field: "projectile_target",
    });
    assert_eq!(
        provider::advance(&mut ctx, &scopes).map(|outcome| outcome.report),
        expected
    );
    ctx.preload_inventory(target, InventoryRecord::empty());
    assert_eq!(
        provider::advance(&mut ctx, &scopes).map(|outcome| outcome.report),
        expected
    );
    assert_eq!(
        ctx.read().actor(target).expect("target").survival.health(),
        20
    );
    assert_eq!(ctx.snapshot_state(world()).projectiles.len(), 1);
    assert!(ctx.events().is_empty());

    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let owner = admit_session(&mut authority);
    let mut passive_ctx = TickContext::harness(&mut authority, TickBudget::full());
    let target = ActorKey::Passive(mornlea_domain::PassiveId::try_new(31).expect("id"));
    passive_ctx
        .stage(RuleEffect::Actor(passive(31, 1.3)))
        .expect("passive");
    passive_ctx
        .stage(RuleEffect::Projectile {
            before: None,
            after: Some(arrow(1, owner)),
        })
        .expect("arrow");
    assert_eq!(
        provider::advance(&mut passive_ctx, &scopes).map(|outcome| outcome.report),
        expected
    );
    assert_eq!(
        passive_ctx
            .read()
            .actor(target)
            .expect("target")
            .survival
            .health(),
        20
    );
    assert_eq!(passive_ctx.snapshot_state(world()).projectiles.len(), 1);
}

/// An opening fixture may contain a legacy oversized hit, but a live Arrow
/// confirmation must validate before its projectile or target mutates.
#[test]
fn invalid_arrow_confirmation_preserves_hit_state() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let owner = admit_session(&mut authority);
    authority
        .advance_tick(TickBudget::full())
        .expect("tick one");
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let target = ActorKey::Hostile(mornlea_domain::HostileId::try_new(21).expect("id"));
    ctx.stage(RuleEffect::Actor(hostile(
        21,
        1.3,
        Dimension::OVERWORLD,
        ActorLifecycle::Active,
    )))
    .expect("target");
    let mut legacy = arrow(1, owner);
    legacy.damage = 21;
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(legacy.clone()),
    })
    .expect("fixture record");
    assert_eq!(
        provider::advance(
            &mut ctx,
            &[scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 0)],
        )
        .map(|outcome| outcome.report),
        Err(mornlea_server::contracts::ServerError::InvalidInput {
            field: "projectile_target"
        })
    );
    assert_eq!(ctx.snapshot_state(world()).projectiles, vec![legacy]);
    assert_eq!(
        ctx.read().actor(target).expect("target").survival.health(),
        20
    );
    assert!(ctx.events().is_empty());
}

/// An entity behind the wall cannot beat an equal segment entry into the
/// wall, and a target in another dimension cannot consume the arrow.
#[test]
fn block_tie_wins_and_stale_target_is_ignored() {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let owner = admit_session(&mut authority);
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    ctx.stage(RuleEffect::Actor(hostile(
        21,
        1.300_000_1,
        Dimension::OVERWORLD,
        ActorLifecycle::Active,
    )))
    .expect("target");
    ctx.stage(RuleEffect::Projectile {
        before: None,
        after: Some(arrow(1, owner)),
    })
    .expect("arrow");
    ctx.preload_block(observed(0, 0));
    ctx.preload_block(observed(1, 2));
    let scopes = [scope(Dimension::OVERWORLD, ChunkPos::new(0, 0), 0)];
    provider::advance(&mut ctx, &scopes).expect("wall step");
    assert!(ctx.snapshot_state(world()).projectiles.is_empty());
    assert!(ctx.read().damage_intents().is_empty());

    let mut next = TickContext::harness(&mut authority, TickBudget::full());
    next.stage(RuleEffect::Actor(hostile(
        21,
        1.3,
        Dimension::DEPTHS,
        ActorLifecycle::Active,
    )))
    .expect("stale dimension");
    next.stage(RuleEffect::Projectile {
        before: None,
        after: Some(arrow(2, owner)),
    })
    .expect("arrow");
    provider::advance(&mut next, &scopes).expect("stale target step");
    assert_eq!(next.snapshot_state(world()).projectiles.len(), 1);
    assert!(next.read().damage_intents().is_empty());
}

fn player_input(
    ctx: &mut TickContext<'_>,
    session: SessionKey,
    sequence: u64,
    input: PlayerControl,
) -> Result<PhaseReport, ServerError> {
    let envelope = CommandEnvelope::try_new(CommandEnvelopeParts {
        tick: 0,
        session: session.get(),
        sequence,
        arrival_index: sequence,
        command: Command::PlayerInput(input),
    })
    .unwrap();
    mornlea_server::rules::player_motion::run(
        ctx,
        RuleCall {
            phase: RulePhase::PlayerCommand,
            actor: None,
            command: Some(&envelope),
            internal: None,
        },
    )
}

#[test]
fn intake_primary_advances_bow_before_motion() {
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    let session = admit_session(&mut authority);
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = bow_scene(&mut ctx, session, false, None);
    let before = ctx.read().actor(actor).unwrap().clone();
    player_input(&mut ctx, session, 1, control(true)).unwrap();
    provider::run(&mut ctx, bow_call(actor)).unwrap();
    assert_eq!(
        ctx.read().runtime(actor).unwrap().bow,
        Some(BowProgress {
            slot: HotbarSlot::new(0).unwrap(),
            ticks: 1
        })
    );
    assert_eq!(
        ctx.read().actor(actor),
        Some(&before),
        "intake and bow never move the actor"
    );
    assert_eq!(ctx.read().inventory(actor).unwrap().slots[1].count, 3);
    assert!(ctx.snapshot_state(world()).projectiles.is_empty());
}

#[test]
fn intake_release_fires_with_new_look_before_motion() {
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    let session = admit_session(&mut authority);
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = bow_scene(&mut ctx, session, true, Some(20));
    let before_pose = ctx.read().actor(actor).unwrap().motion;
    let input = PlayerControl::new(PlayerControlParts {
        look: LookAngles::try_new(1.234, 0.321).unwrap(),
        movement: control(false).movement(),
        actions: control(false).actions(),
    });
    player_input(&mut ctx, session, 1, input).unwrap();
    assert_eq!(
        ctx.read().runtime(actor).unwrap().bow.unwrap().ticks,
        20,
        "valid release leaves progress for BowDraw"
    );
    provider::run(&mut ctx, bow_call(actor)).unwrap();
    let arrows = ctx.snapshot_state(world()).projectiles;
    assert_eq!(arrows.len(), 1);
    assert_eq!(
        arrows[0].velocity.get().map(f32::to_bits),
        [0xc1d6_f22d, 0x4117_7290, 0xc116_8556]
    );
    assert_eq!(ctx.read().actor(actor).unwrap().motion, before_pose);
    assert_eq!(ctx.read().inventory(actor).unwrap().slots[1].count, 2);
    assert_eq!(
        ctx.read().inventory(actor).unwrap().slots[0].durability,
        119
    );
}

#[test]
fn invalid_intake_interrupts_actions_and_never_releases_bow() {
    for valid_release_after in [false, true] {
        let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
        let session = admit_session(&mut authority);
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        let actor = bow_scene(&mut ctx, session, true, Some(20));
        let mut runtime = ctx.read().runtime(actor).unwrap().clone();
        runtime.eating = Some(EatingProgress {
            slot: HotbarSlot::new(1).unwrap(),
            item: 36,
            ticks: 7,
        });
        ctx.stage(RuleEffect::Runtime(runtime)).unwrap();
        ctx.stage(RuleEffect::Mining {
            actor,
            progress: Some(MiningProgress {
                actor,
                dimension: Dimension::OVERWORLD,
                target: BlockPos::new(0, 63, 0),
                observed_block: 2,
                tool_slot: HotbarSlot::new(0).unwrap(),
                tool: ItemStack::default(),
                elapsed: 3,
                required: 10,
                last_tick: 0,
            }),
        })
        .unwrap();
        let inventory = *ctx.read().inventory(actor).unwrap();
        let before = ctx.read().actor(actor).unwrap().clone();
        let invalid = PlayerControl::new(PlayerControlParts {
            movement: Movement {
                move_x: 2,
                move_z: 0,
                jump: false,
            },
            look: control(false).look(),
            actions: control(false).actions(),
        });
        assert_eq!(
            player_input(&mut ctx, session, 1, invalid),
            Err(ServerError::InvalidInput { field: "control" })
        );
        assert_eq!(ctx.read().runtime(actor).unwrap().controls, None);
        assert_eq!(ctx.read().runtime(actor).unwrap().eating, None);
        assert_eq!(ctx.read().runtime(actor).unwrap().bow, None);
        assert_eq!(ctx.read().mining(actor), None);
        if valid_release_after {
            player_input(&mut ctx, session, 2, control(false)).unwrap();
        }
        provider::run(&mut ctx, bow_call(actor)).unwrap();
        assert_eq!(
            ctx.read().runtime(actor).unwrap().bow,
            None,
            "a later valid release cannot resurrect invalidated progress"
        );
        assert_eq!(
            ctx.read().inventory(actor),
            Some(&inventory),
            "no arrow debit or bow wear"
        );
        assert!(ctx.snapshot_state(world()).projectiles.is_empty());
        assert_eq!(ctx.read().actor(actor), Some(&before));
    }
}

#[test]
fn stale_and_tied_invalid_intake_preserve_winning_draw() {
    for stale_sequence in [1, 2] {
        let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
        let session = admit_session(&mut authority);
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        let actor = bow_scene(&mut ctx, session, true, Some(20));
        let mut initial = ctx.read().runtime(actor).unwrap().clone();
        initial.eating = Some(EatingProgress {
            slot: HotbarSlot::new(1).unwrap(),
            item: 36,
            ticks: 7,
        });
        ctx.stage(RuleEffect::Runtime(initial)).unwrap();
        let mining = MiningProgress {
            actor,
            dimension: Dimension::OVERWORLD,
            target: BlockPos::new(0, 63, 0),
            observed_block: 2,
            tool_slot: HotbarSlot::new(0).unwrap(),
            tool: ItemStack::default(),
            elapsed: 3,
            required: 10,
            last_tick: 0,
        };
        ctx.stage(RuleEffect::Mining {
            actor,
            progress: Some(mining.clone()),
        })
        .unwrap();
        player_input(&mut ctx, session, 2, control(true)).unwrap();
        let before = ctx.read().runtime(actor).unwrap().clone();
        let invalid = PlayerControl::new(PlayerControlParts {
            movement: Movement {
                move_x: 2,
                move_z: 0,
                jump: false,
            },
            look: control(false).look(),
            actions: control(false).actions(),
        });
        assert_eq!(
            player_input(&mut ctx, session, stale_sequence, invalid).unwrap(),
            PhaseReport {
                examined: 1,
                applied: 0,
                carried: 0,
                rejected: 1
            }
        );
        assert_eq!(ctx.read().runtime(actor), Some(&before));
        assert_eq!(ctx.read().mining(actor), Some(&mining));
        assert_eq!(ctx.deferred(RulePhase::PlayerMotion).len(), 1);
        provider::run(&mut ctx, bow_call(actor)).unwrap();
        assert_eq!(ctx.read().runtime(actor).unwrap().bow.unwrap().ticks, 21);
        assert!(ctx.snapshot_state(world()).projectiles.is_empty());
    }
}

#[test]
fn invalid_then_valid_release_does_not_resurrect_draw() {
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    let session = admit_session(&mut authority);
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = bow_scene(&mut ctx, session, true, Some(20));
    let inventory = *ctx.read().inventory(actor).unwrap();
    let invalid = PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: 2,
            move_z: 0,
            jump: false,
        },
        look: control(false).look(),
        actions: control(false).actions(),
    });
    assert_eq!(
        player_input(&mut ctx, session, 1, invalid),
        Err(ServerError::InvalidInput { field: "control" })
    );
    player_input(&mut ctx, session, 2, control(false)).unwrap();
    provider::run(&mut ctx, bow_call(actor)).unwrap();
    assert_eq!(ctx.read().runtime(actor).unwrap().bow, None);
    assert_eq!(ctx.read().inventory(actor), Some(&inventory));
    assert!(ctx.snapshot_state(world()).projectiles.is_empty());
}
