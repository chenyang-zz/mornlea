//! Bounded melee freeze and live settlement replay.

use super::*;
use mornlea_domain::{
    BlockPos, ChunkPos, CraftingSize, Dimension, Event, EventRecipient, FiniteVec3, HeldActions,
    HostileId, HotbarSlot, LookAngles, MotionState, MotionStateParts, Movement, PassiveId,
    PlayerControl, PlayerControlParts, SurvivalState, SurvivalStateParts, Weather,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, BlockObservation,
    BowProgress, ChunkKey, DamageCause, DropBatch, DropSource, EatingProgress, EnvironmentState,
    HostileMeleeAttack, HostileMeleeBatch, InventoryRecord, RuleCall, RuleEffect, RulePhase,
    RuleTunables, ServerError, SessionKey, TransportKind,
};
use mornlea_server::core::world::ReadyChunk;
use mornlea_server::rules::hostile_outcomes as provider;
use mornlea_server::rules::player_survival as survival_provider;
use mornlea_storage::{
    Chunk, ContainerSnapshot, HostileMob, ItemStack, PassiveMob, PlayerLocation, PlayerSave,
    StorageKind,
};

fn authority() -> AuthorityState {
    let mut state = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        7,
    )
    .unwrap();
    state.advance_tick(TickBudget::full()).unwrap();
    state
}

fn session(state: &mut AuthorityState, tag: u8) -> SessionKey {
    let mut uuid = [0; 16];
    uuid[0] = tag;
    uuid[6] = 0x40;
    uuid[8] = 0x80;
    let id = mornlea_domain::PlayerId::try_from_bytes(uuid).unwrap();
    let login = LoginStart::new(id, "Melee", 8).unwrap();
    let admitted =
        admit_login(LoginStart::decode_inbound(&login.encode().unwrap()).unwrap()).unwrap();
    state.admit(admitted, TransportKind::Memory).unwrap()
}

fn motion(position: [f32; 3]) -> MotionState {
    MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new(position).unwrap(),
        velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
        on_ground: true,
    })
}

fn survival(health: u8) -> SurvivalState {
    SurvivalState::try_new(SurvivalStateParts {
        health,
        oxygen: 300,
        hunger: 20,
        saturation_zero: false,
        armor_points: 0,
    })
    .unwrap()
}

fn player(key: SessionKey, tag: u8, position: [f32; 3]) -> ActorRecord {
    let mut uuid = [0; 16];
    uuid[0] = tag;
    uuid[6] = 0x40;
    uuid[8] = 0x80;
    ActorRecord::try_new(
        ActorKey::Player(key),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        motion(position),
        LookAngles::try_new(0.0, 0.0).unwrap(),
        survival(20),
        ActorBody::Player(PlayerSave {
            player_id: mornlea_storage::PlayerId::from_bytes(uuid),
            revision: 1,
            display_name: "Melee".into(),
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
    .unwrap()
}

fn hostile(id: u64, position: [f32; 3]) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Hostile(HostileId::try_new(id).unwrap()),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        motion(position),
        LookAngles::try_new(0.0, 0.0).unwrap(),
        survival(20),
        ActorBody::Hostile(HostileMob {
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
            player_id: mornlea_storage::PlayerId::from_bytes([0; 16]),
            next_repath_ticks: 0,
            distant_ticks: 0,
            kind: 0,
        }),
    )
    .unwrap()
}

fn passive(id: u64, position: [f32; 3]) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Passive(PassiveId::try_new(id).unwrap()),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        motion(position),
        LookAngles::try_new(0.0, 0.0).unwrap(),
        survival(20),
        ActorBody::Passive(PassiveMob {
            id,
            dimension: 0,
            position,
            velocity: [0.0; 3],
            on_ground: true,
            yaw: 0.0,
            health: 20,
        }),
    )
    .unwrap()
}

fn runtime(key: ActorKey, primary: bool) -> ActorRuntime {
    ActorRuntime {
        key,
        controls: Some(PlayerControl::new(PlayerControlParts {
            movement: Movement {
                move_x: 0,
                move_z: 0,
                jump: false,
            },
            look: LookAngles::try_new(0.0, 0.0).unwrap(),
            actions: HeldActions {
                primary,
                eating: false,
                sprinting: false,
                sneaking: false,
            },
        })),
        has_view: true,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 300,
        peak_y: 1.0,
        exhaustion_milli: 0,
        saturation_milli: 5_000,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Player {
            respawn: None,
            workbench: None,
        },
    }
}

fn hostile_runtime(key: ActorKey) -> ActorRuntime {
    let mut value = runtime(key, false);
    value.controls = None;
    value.aux = ActorAux::Hostile {
        distant_ticks: 0,
        shoot_cooldown: 0,
        fresh: false,
    };
    value
}

fn passive_runtime(key: ActorKey) -> ActorRuntime {
    let mut value = runtime(key, false);
    value.controls = None;
    value.aux = ActorAux::Passive {
        home: BlockPos::new(0, 1, 0),
        flee_ticks: 0,
        flee_from: None,
        graze_ticks: 19,
        graze_at: None,
        fresh: false,
    };
    value
}

fn environment(ctx: &mut TickContext<'_>) {
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
    .unwrap();
}

fn air_ray(ctx: &mut TickContext<'_>, wall: Option<(i32, u16)>) {
    for z in -4..=2 {
        let pos = BlockPos::new(0, 2, z);
        let key = ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, z.div_euclid(16)),
        };
        let block = wall
            .filter(|(at, _)| *at == z)
            .map_or(0, |(_, block)| block);
        ctx.preload_block(BlockObservation::try_new(key, 1, 1, pos, block).unwrap());
    }
}

fn stage_player(
    ctx: &mut TickContext<'_>,
    key: SessionKey,
    tag: u8,
    position: [f32; 3],
    primary: bool,
) {
    let actor = ActorKey::Player(key);
    ctx.stage(RuleEffect::Actor(player(key, tag, position)))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(runtime(actor, primary)))
        .unwrap();
    ctx.preload_inventory(actor, InventoryRecord::empty());
}

#[test]
fn hostile_choice_damages_actual_player_without_owner_event() {
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, 0.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    ctx.stage(RuleEffect::Actor(player(victim, 1, [1.5, 1.0, 0.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(runtime(
        ActorKey::Player(victim),
        false,
    )))
    .unwrap();
    ctx.preload_inventory(ActorKey::Player(victim), InventoryRecord::empty());
    let batch = HostileMeleeBatch::try_new(
        ctx.read().tick(),
        &[HostileMeleeAttack::new(attacker, victim)],
    )
    .unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(outcome.damaged_players, vec![victim]);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        17
    );
    assert!(ctx.events().is_empty());
}

#[test]
fn player_reaches_exactly_three_and_hits_nearest() {
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let near = session(&mut state, 2);
    let far = session(&mut state, 3);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, near, 2, [0.5, 1.0, -2.8], false);
    stage_player(&mut ctx, far, 3, [0.5, 1.0, -3.2], false);
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(outcome.damaged_players, vec![near]);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(near))
            .unwrap()
            .survival
            .health(),
        18
    );
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(far))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert_eq!(ctx.events().len(), 1);
    assert_eq!(
        ctx.events()[0].recipient(),
        EventRecipient::Session(attacker.get())
    );
    assert!(matches!(ctx.events()[0].event(), Event::CombatHit(_)));
    assert!(ctx.mining_suppressed(ActorKey::Player(attacker)));
}

#[test]
fn protected_nearest_blocks_farther_target() {
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let near = session(&mut state, 2);
    let far = session(&mut state, 3);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, near, 2, [0.5, 1.0, -1.0], false);
    stage_player(&mut ctx, far, 3, [0.5, 1.0, -2.0], false);
    let mut protected = ctx.read().runtime(ActorKey::Player(near)).unwrap().clone();
    protected.hurt_cooldown = 2;
    ctx.stage(RuleEffect::Runtime(protected)).unwrap();
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(far))
            .unwrap()
            .survival
            .health(),
        20
    );
}

#[test]
fn closed_wall_blocks_melee_but_water_does_not() {
    for (block, expected) in [(2, 0), (27, 1)] {
        let mut state = authority();
        let attacker = session(&mut state, 1);
        let victim = session(&mut state, 2);
        let mut ctx = TickContext::harness(&mut state, TickBudget::full());
        environment(&mut ctx);
        stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
        stage_player(&mut ctx, victim, 2, [0.5, 1.0, -2.0], false);
        air_ray(&mut ctx, Some((-1, block)));
        let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
        let outcome = provider::advance(&mut ctx, &batch).unwrap();
        assert_eq!(outcome.report.applied, expected, "block {block}");
    }
}

#[test]
fn stale_selected_item_refuses_hit_without_wear() {
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -1.0], false);
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let frame = provider::freeze(&ctx, &batch).unwrap();
    let before = *ctx.read().inventory(ActorKey::Player(attacker)).unwrap();
    let mut after = before;
    after.slots[0] = ItemStack {
        item: 47,
        count: 1,
        durability: 1,
    };
    ctx.stage(RuleEffect::Inventory(
        mornlea_server::contracts::InventoryPatch {
            actor: ActorKey::Player(attacker),
            before,
            after,
        },
    ))
    .unwrap();
    let outcome = provider::settle_frame(&mut ctx, frame).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert_eq!(outcome.report.rejected, 1);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(attacker))
            .unwrap()
            .slots[0],
        after.slots[0]
    );
    assert!(!ctx.mining_suppressed(ActorKey::Player(attacker)));
    assert!(ctx.events().is_empty());
}

#[test]
fn stale_player_body_identity_refuses_frozen_hit() {
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -1.0], false);
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let frame = provider::freeze(&ctx, &batch).unwrap();
    let mut changed = ctx.read().actor(ActorKey::Player(victim)).unwrap().clone();
    let ActorBody::Player(body) = &mut changed.body else {
        unreachable!()
    };
    let mut uuid = [0; 16];
    uuid[0] = 9;
    uuid[6] = 0x40;
    uuid[8] = 0x80;
    body.player_id = mornlea_storage::PlayerId::from_bytes(uuid);
    ctx.stage(RuleEffect::Actor(changed)).unwrap();
    let outcome = provider::settle_frame(&mut ctx, frame).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert_eq!(outcome.report.rejected, 1);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
}

#[test]
fn latest_sword_durability_and_fractional_exhaustion_settle_together() {
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    let victim = ActorKey::Hostile(HostileId::try_new(7).unwrap());
    ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, -1.0])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(victim)))
        .unwrap();
    let before = *ctx.read().inventory(ActorKey::Player(attacker)).unwrap();
    let mut after = before;
    after.slots[0] = ItemStack {
        item: 47,
        count: 1,
        durability: 2,
    };
    ctx.stage(RuleEffect::Inventory(
        mornlea_server::contracts::InventoryPatch {
            actor: ActorKey::Player(attacker),
            before,
            after,
        },
    ))
    .unwrap();
    let mut charge = ctx
        .read()
        .runtime(ActorKey::Player(attacker))
        .unwrap()
        .clone();
    charge.exhaustion_milli = 3_999;
    charge.saturation_milli = 500;
    ctx.stage(RuleEffect::Runtime(charge)).unwrap();
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let frame = provider::freeze(&ctx, &batch).unwrap();
    let before = *ctx.read().inventory(ActorKey::Player(attacker)).unwrap();
    let mut after = before;
    after.slots[0].durability = 1;
    ctx.stage(RuleEffect::Inventory(
        mornlea_server::contracts::InventoryPatch {
            actor: ActorKey::Player(attacker),
            before,
            after,
        },
    ))
    .unwrap();
    let outcome = provider::settle_frame(&mut ctx, frame).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(ctx.read().actor(victim).unwrap().survival.health(), 16);
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(attacker))
            .unwrap()
            .slots[0],
        ItemStack {
            item: 50,
            count: 1,
            durability: 0
        }
    );
    let runtime = ctx.read().runtime(ActorKey::Player(attacker)).unwrap();
    assert_eq!(runtime.exhaustion_milli, 99);
    assert_eq!(runtime.saturation_milli, 0);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(attacker))
            .unwrap()
            .survival
            .hunger(),
        20
    );
}

#[test]
fn armor_uses_frozen_points_and_wears_each_reducing_piece() {
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -1.0], false);
    let before = *ctx.read().inventory(ActorKey::Player(attacker)).unwrap();
    let mut after = before;
    after.slots[0] = ItemStack {
        item: 49,
        count: 1,
        durability: 2,
    };
    ctx.stage(RuleEffect::Inventory(
        mornlea_server::contracts::InventoryPatch {
            actor: ActorKey::Player(attacker),
            before,
            after,
        },
    ))
    .unwrap();
    let before = *ctx.read().inventory(ActorKey::Player(victim)).unwrap();
    let mut after = before;
    for (slot, item) in [58, 59, 60, 61].into_iter().enumerate() {
        after.armor[slot] = ItemStack {
            item,
            count: 1,
            durability: 2,
        };
    }
    ctx.stage(RuleEffect::Inventory(
        mornlea_server::contracts::InventoryPatch {
            actor: ActorKey::Player(victim),
            before,
            after,
        },
    ))
    .unwrap();
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        18
    );
    let armor = &ctx
        .read()
        .inventory(ActorKey::Player(victim))
        .unwrap()
        .armor;
    assert!(armor.iter().all(|piece| piece.durability == 1));
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .armor_points(),
        15
    );
}

#[test]
fn passive_hit_arms_flee_and_preserves_vertical_velocity() {
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    let victim = ActorKey::Passive(PassiveId::try_new(11).unwrap());
    ctx.stage(RuleEffect::Actor(passive(11, [0.5, 1.0, -1.0])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(passive_runtime(victim)))
        .unwrap();
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    let victim_record = ctx.read().actor(victim).unwrap();
    assert_eq!(victim_record.survival.health(), 18);
    assert_eq!(victim_record.motion.velocity().get()[1], 0.0);
    assert!(victim_record.motion.velocity().get()[2] < 0.0);
    let ActorAux::Passive {
        flee_ticks,
        flee_from,
        graze_ticks,
        ..
    } = &ctx.read().runtime(victim).unwrap().aux
    else {
        unreachable!()
    };
    assert_eq!((*flee_ticks, *graze_ticks), (60, 0));
    assert_eq!(flee_from.unwrap().get(), [0.5, 1.0, 0.5]);
}

#[test]
fn mutual_lethal_frozen_players_both_settle() {
    let mut state = authority();
    let first = session(&mut state, 1);
    let second = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, first, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, second, 2, [0.5, 1.0, -1.0], true);
    let mut facing = ctx.read().actor(ActorKey::Player(second)).unwrap().clone();
    facing.look = LookAngles::try_new(std::f32::consts::PI, 0.0).unwrap();
    ctx.stage(RuleEffect::Actor(facing)).unwrap();
    for key in [first, second] {
        let mut record = ctx.read().actor(ActorKey::Player(key)).unwrap().clone();
        record.survival = survival(2);
        let ActorBody::Player(body) = &mut record.body else {
            unreachable!()
        };
        body.health = 2;
        ctx.stage(RuleEffect::Actor(record)).unwrap();
    }
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 2);
    assert_eq!(outcome.damaged_players, vec![second, first]);
    for key in [first, second] {
        let record = ctx.read().actor(ActorKey::Player(key)).unwrap();
        assert_eq!(record.survival.health(), 0);
        assert_eq!(record.lifecycle, ActorLifecycle::Active);
        assert_eq!(
            ctx.read()
                .runtime(ActorKey::Player(key))
                .unwrap()
                .attack_cooldown,
            10
        );
    }
    assert!(ctx.read().damage_intents().is_empty());
}

#[test]
fn stale_batch_tick_refuses_without_effects() {
    // A batch minted for another tick is not this tick's choices: the freeze
    // rejects it before any cooldown, inventory, event or suppression edit,
    // so the overlaid snapshot reads back identical.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, 0.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    stage_player(&mut ctx, victim, 1, [1.5, 1.0, 0.5], false);
    let before_actor = ctx
        .read()
        .actor(ActorKey::Hostile(attacker))
        .unwrap()
        .clone();
    let before_runtime = ctx
        .read()
        .runtime(ActorKey::Player(victim))
        .unwrap()
        .clone();
    let before_health = ctx
        .read()
        .actor(ActorKey::Player(victim))
        .unwrap()
        .survival
        .health();
    let batch =
        HostileMeleeBatch::try_new(0, &[HostileMeleeAttack::new(attacker, victim)]).unwrap();
    let Err(error) = provider::advance(&mut ctx, &batch) else {
        panic!("a stale batch tick must refuse");
    };
    assert!(matches!(
        error,
        ServerError::InvalidInput {
            field: "combat_tick"
        }
    ));
    assert_eq!(
        ctx.read().actor(ActorKey::Hostile(attacker)).unwrap(),
        &before_actor
    );
    assert_eq!(
        ctx.read().runtime(ActorKey::Player(victim)).unwrap(),
        &before_runtime
    );
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        before_health
    );
    assert!(ctx.events().is_empty());
    assert!(!ctx.mining_suppressed(ActorKey::Player(victim)));
}

#[test]
fn malformed_actor_position_refuses_before_effects() {
    // Coordinates outside the `i32` floor bounds cannot enter the numerical
    // walks: the freeze reports the malformed record before any effect.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, victim, 1, [1.0e10, 1.0, 0.5], false);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let Err(error) = provider::advance(&mut ctx, &batch) else {
        panic!("an out-of-bounds position must refuse");
    };
    assert!(matches!(
        error,
        ServerError::InvalidInput {
            field: "combat_actor"
        }
    ));
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert_eq!(
        ctx.read()
            .runtime(ActorKey::Player(victim))
            .unwrap()
            .attack_cooldown,
        0
    );
    assert!(ctx.events().is_empty());
}

#[test]
fn missing_prerequisites_refuse_without_effects() {
    // A missing environment record refuses with the environment invariant.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    stage_player(&mut ctx, victim, 1, [1.5, 1.0, 0.5], false);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let Err(error) = provider::advance(&mut ctx, &batch) else {
        panic!("a missing environment must refuse");
    };
    assert!(matches!(error, ServerError::Internal { .. }));
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert!(ctx.events().is_empty());

    // A staged actor without its runtime record refuses with the runtime
    // invariant and leaves the staged snapshot alone.
    let mut state = authority();
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, 0.5])))
        .unwrap();
    stage_player(&mut ctx, victim, 2, [1.5, 1.0, 0.5], false);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let Err(error) = provider::advance(&mut ctx, &batch) else {
        panic!("a missing runtime must refuse");
    };
    assert!(matches!(error, ServerError::Internal { .. }));
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert!(ctx.events().is_empty());

    // A hostile actor paired with a player-aux runtime refuses with the aux
    // invariant instead of settling anything.
    let mut state = authority();
    let victim = session(&mut state, 3);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, 0.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(runtime(
        ActorKey::Hostile(attacker),
        false,
    )))
    .unwrap();
    stage_player(&mut ctx, victim, 3, [1.5, 1.0, 0.5], false);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let Err(error) = provider::advance(&mut ctx, &batch) else {
        panic!("a mismatched aux must refuse");
    };
    assert!(matches!(error, ServerError::Internal { .. }));
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert!(ctx.events().is_empty());
}

#[test]
fn just_outside_reach_misses() {
    // The reach is inclusive at exactly three: a target box entered one
    // hundredth past the reach is no target at all, with no hit, event or
    // suppression edit.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -2.81], false);
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert!(outcome.damaged_players.is_empty());
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert!(ctx.events().is_empty());
    assert!(!ctx.mining_suppressed(ActorKey::Player(attacker)));
}

#[test]
fn symmetric_targets_prefer_kind_then_id() {
    // Two equidistant off-axis targets hit the same slab distance: the wire
    // kind (`Player` 1, `Hostile` 2, `Passive` 3) outranks geometry, so the
    // player takes the hit even though the hostile snapshot comes first.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    let rival = ActorKey::Hostile(HostileId::try_new(7).unwrap());
    ctx.stage(RuleEffect::Actor(hostile(7, [0.2, 1.0, -1.0])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(rival)))
        .unwrap();
    stage_player(&mut ctx, victim, 2, [0.8, 1.0, -1.0], false);
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(outcome.damaged_players, vec![victim]);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        18
    );
    assert_eq!(ctx.read().actor(rival).unwrap().survival.health(), 20);

    // Within one family the numeric ID breaks the tie: hostile 3 wins over
    // hostile 9 at the mirrored position.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    let higher = ActorKey::Hostile(HostileId::try_new(9).unwrap());
    let lower = ActorKey::Hostile(HostileId::try_new(3).unwrap());
    ctx.stage(RuleEffect::Actor(hostile(9, [0.2, 1.0, -1.0])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(higher)))
        .unwrap();
    ctx.stage(RuleEffect::Actor(hostile(3, [0.8, 1.0, -1.0])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(lower)))
        .unwrap();
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert!(outcome.damaged_players.is_empty());
    assert_eq!(ctx.read().actor(lower).unwrap().survival.health(), 18);
    assert_eq!(ctx.read().actor(higher).unwrap().survival.health(), 20);
}

#[test]
fn equal_distance_wall_permits_hit() {
    // Occlusion needs a strictly nearer block: a closed wall entered at
    // exactly the target distance permits the hit.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -1.3], false);
    air_ray(&mut ctx, Some((-2, 2)));
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(outcome.damaged_players, vec![victim]);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        18
    );
}

#[test]
fn open_doors_pass_melee_ray() {
    // An open lower door never blocks the ray.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -2.0], false);
    air_ray(&mut ctx, Some((-1, 63)));
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        18
    );

    // An open upper door over an open lower door passes as well: the upper
    // half consults the cell below before blocking.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -2.0], false);
    for z in -4..=2 {
        let pos = BlockPos::new(0, 2, z);
        let key = ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, z.div_euclid(16)),
        };
        let block = if z == -1 { 70 } else { 0 };
        ctx.preload_block(BlockObservation::try_new(key, 1, 1, pos, block).unwrap());
    }
    ctx.preload_block(
        BlockObservation::try_new(
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, -1),
            },
            1,
            1,
            BlockPos::new(0, 1, -1),
            63,
        )
        .unwrap(),
    );
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        18
    );
}

#[test]
fn unavailable_traversed_chunk_suppresses_hit() {
    // A ray crossing a chunk the overlay never preloaded suppresses the
    // intent instead of swinging through unknown geometry.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -2.0], false);
    for z in 0..=2 {
        let pos = BlockPos::new(0, 2, z);
        let key = ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, z.div_euclid(16)),
        };
        ctx.preload_block(BlockObservation::try_new(key, 1, 1, pos, 0).unwrap());
    }
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert!(outcome.damaged_players.is_empty());
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert!(ctx.events().is_empty());
    assert!(!ctx.mining_suppressed(ActorKey::Player(attacker)));
}

#[test]
fn both_bow_forms_are_excluded() {
    // Either bow wire form (`62` or `65`) in the primary hand never starts a
    // melee intent, no matter how clean the ray is.
    for item in [62, 65] {
        let mut state = authority();
        let attacker = session(&mut state, 1);
        let victim = session(&mut state, 2);
        let mut ctx = TickContext::harness(&mut state, TickBudget::full());
        environment(&mut ctx);
        stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
        stage_player(&mut ctx, victim, 2, [0.5, 1.0, -1.0], false);
        let before = *ctx.read().inventory(ActorKey::Player(attacker)).unwrap();
        let mut after = before;
        after.slots[0] = ItemStack {
            item,
            count: 1,
            durability: 1,
        };
        ctx.stage(RuleEffect::Inventory(
            mornlea_server::contracts::InventoryPatch {
                actor: ActorKey::Player(attacker),
                before,
                after,
            },
        ))
        .unwrap();
        air_ray(&mut ctx, None);
        let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
        let outcome = provider::advance(&mut ctx, &batch).unwrap();
        assert_eq!(outcome.report.applied, 0, "bow form {item}");
        assert_eq!(
            ctx.read()
                .actor(ActorKey::Player(victim))
                .unwrap()
                .survival
                .health(),
            20,
            "bow form {item}"
        );
        assert!(ctx.events().is_empty(), "bow form {item}");
    }
}

#[test]
fn hostile_honors_choice_over_nearest() {
    // The batch choice is the attack: a hostile ordered onto the farther
    // player leaves the nearer one untouched.
    let mut state = authority();
    let near = session(&mut state, 1);
    let far = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, 0.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    stage_player(&mut ctx, near, 1, [1.5, 1.0, 0.5], false);
    stage_player(&mut ctx, far, 2, [2.0, 1.0, 0.5], false);
    let batch =
        HostileMeleeBatch::try_new(ctx.read().tick(), &[HostileMeleeAttack::new(attacker, far)])
            .unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(outcome.damaged_players, vec![far]);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(far))
            .unwrap()
            .survival
            .health(),
        17
    );
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(near))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert!(ctx.events().is_empty());
}

#[test]
fn absent_batch_entry_means_no_hostile_attack() {
    // A saved chase target without a batch entry never swings: an empty
    // batch over an in-range pair settles nothing.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, 0.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    stage_player(&mut ctx, victim, 1, [1.5, 1.0, 0.5], false);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert_eq!(outcome.report.examined, 2);
    assert!(outcome.damaged_players.is_empty());
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    let ActorBody::Hostile(body) = &ctx.read().actor(ActorKey::Hostile(attacker)).unwrap().body
    else {
        unreachable!()
    };
    assert_eq!((body.attack_cooldown, body.hurt_cooldown), (0, 0));
    assert!(ctx.events().is_empty());
}

#[test]
fn wrong_dimension_excludes_hostile_attack() {
    // Horizontal range alone never qualifies: a victim in the depths is not
    // in the attacker's dimension.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, 0.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    let mut record = player(victim, 1, [1.5, 1.0, 0.5]);
    record.dimension = Dimension::DEPTHS;
    let ActorBody::Player(body) = &mut record.body else {
        unreachable!()
    };
    body.current.dimension = 1;
    ctx.stage(RuleEffect::Actor(record)).unwrap();
    ctx.stage(RuleEffect::Runtime(runtime(
        ActorKey::Player(victim),
        false,
    )))
    .unwrap();
    ctx.preload_inventory(ActorKey::Player(victim), InventoryRecord::empty());
    let batch = HostileMeleeBatch::try_new(
        ctx.read().tick(),
        &[HostileMeleeAttack::new(attacker, victim)],
    )
    .unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert!(outcome.damaged_players.is_empty());
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert!(ctx.events().is_empty());
}

#[test]
fn out_of_range_hostile_does_not_attack() {
    // Past `1.8` blocks of horizontal distance the batch entry is only a
    // wish: no intent, no cooldown spend beyond the decrement, no event.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, 0.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    stage_player(&mut ctx, victim, 1, [2.5, 1.0, 0.5], false);
    let batch = HostileMeleeBatch::try_new(
        ctx.read().tick(),
        &[HostileMeleeAttack::new(attacker, victim)],
    )
    .unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert!(outcome.damaged_players.is_empty());
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert!(ctx.events().is_empty());
}

#[test]
fn dead_combatants_are_excluded() {
    // A zero-health hostile cannot start an attack.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    let mut record = hostile(7, [0.5, 1.0, 0.5]);
    record.survival = survival(0);
    let ActorBody::Hostile(body) = &mut record.body else {
        unreachable!()
    };
    body.health = 0;
    ctx.stage(RuleEffect::Actor(record)).unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    stage_player(&mut ctx, victim, 1, [1.5, 1.0, 0.5], false);
    let batch = HostileMeleeBatch::try_new(
        ctx.read().tick(),
        &[HostileMeleeAttack::new(attacker, victim)],
    )
    .unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert!(ctx.events().is_empty());

    // A zero-health target cannot be selected either.
    let mut state = authority();
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, 0.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    let mut record = player(victim, 2, [1.5, 1.0, 0.5]);
    record.survival = survival(0);
    let ActorBody::Player(body) = &mut record.body else {
        unreachable!()
    };
    body.health = 0;
    ctx.stage(RuleEffect::Actor(record)).unwrap();
    ctx.stage(RuleEffect::Runtime(runtime(
        ActorKey::Player(victim),
        false,
    )))
    .unwrap();
    ctx.preload_inventory(ActorKey::Player(victim), InventoryRecord::empty());
    let batch = HostileMeleeBatch::try_new(
        ctx.read().tick(),
        &[HostileMeleeAttack::new(attacker, victim)],
    )
    .unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert!(outcome.damaged_players.is_empty());
    assert!(ctx.events().is_empty());
}

#[test]
fn hurler_never_melees() {
    // The `BoneThrower` kind (`1`) hurls from range: even a valid batch
    // entry at touching distance builds no intent.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    let mut record = hostile(7, [0.5, 1.0, 0.5]);
    let ActorBody::Hostile(body) = &mut record.body else {
        unreachable!()
    };
    body.kind = 1;
    ctx.stage(RuleEffect::Actor(record)).unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    stage_player(&mut ctx, victim, 1, [1.5, 1.0, 0.5], false);
    let batch = HostileMeleeBatch::try_new(
        ctx.read().tick(),
        &[HostileMeleeAttack::new(attacker, victim)],
    )
    .unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert!(outcome.damaged_players.is_empty());
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert!(ctx.events().is_empty());
}

#[test]
fn hostile_cooldown_one_permits_two_refuses() {
    // Cooldowns decrement in the frozen copy: `1` reaches zero and permits
    // the hit, `2` only steps down to one and refuses it.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    let mut record = hostile(7, [0.5, 1.0, 0.5]);
    let ActorBody::Hostile(body) = &mut record.body else {
        unreachable!()
    };
    body.attack_cooldown = 1;
    ctx.stage(RuleEffect::Actor(record)).unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    stage_player(&mut ctx, victim, 1, [1.5, 1.0, 0.5], false);
    let batch = HostileMeleeBatch::try_new(
        ctx.read().tick(),
        &[HostileMeleeAttack::new(attacker, victim)],
    )
    .unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        17
    );
    let ActorBody::Hostile(body) = &ctx.read().actor(ActorKey::Hostile(attacker)).unwrap().body
    else {
        unreachable!()
    };
    assert_eq!(body.attack_cooldown, 20);

    let mut state = authority();
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    let mut record = hostile(7, [0.5, 1.0, 0.5]);
    let ActorBody::Hostile(body) = &mut record.body else {
        unreachable!()
    };
    body.attack_cooldown = 2;
    ctx.stage(RuleEffect::Actor(record)).unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    stage_player(&mut ctx, victim, 2, [1.5, 1.0, 0.5], false);
    let batch = HostileMeleeBatch::try_new(
        ctx.read().tick(),
        &[HostileMeleeAttack::new(attacker, victim)],
    )
    .unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert!(outcome.damaged_players.is_empty());
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    let ActorBody::Hostile(body) = &ctx.read().actor(ActorKey::Hostile(attacker)).unwrap().body
    else {
        unreachable!()
    };
    assert_eq!(body.attack_cooldown, 1);
    assert_eq!(
        ctx.read()
            .runtime(ActorKey::Player(victim))
            .unwrap()
            .hurt_cooldown,
        0
    );
    assert!(ctx.events().is_empty());
}

#[test]
fn lower_hostile_id_wins_single_victim() {
    // Batch construction sorts entries by attacker, so the lower family ID
    // reserves the shared victim even when listed second: one hit, one
    // rejection, and the loser's cooldowns stay decremented, not spent.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let higher = HostileId::try_new(9).unwrap();
    let lower = HostileId::try_new(3).unwrap();
    ctx.stage(RuleEffect::Actor(hostile(9, [0.5, 1.0, 0.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        higher,
    ))))
    .unwrap();
    ctx.stage(RuleEffect::Actor(hostile(3, [0.5, 1.0, -0.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        lower,
    ))))
    .unwrap();
    stage_player(&mut ctx, victim, 1, [1.5, 1.0, 0.5], false);
    let batch = HostileMeleeBatch::try_new(
        ctx.read().tick(),
        &[
            HostileMeleeAttack::new(higher, victim),
            HostileMeleeAttack::new(lower, victim),
        ],
    )
    .unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(outcome.report.rejected, 1);
    assert_eq!(outcome.damaged_players, vec![victim]);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        17
    );
    let ActorBody::Hostile(body) = &ctx.read().actor(ActorKey::Hostile(lower)).unwrap().body else {
        unreachable!()
    };
    assert_eq!(body.attack_cooldown, 20);
    let ActorBody::Hostile(body) = &ctx.read().actor(ActorKey::Hostile(higher)).unwrap().body
    else {
        unreachable!()
    };
    assert_eq!(body.attack_cooldown, 0);
    assert!(ctx.events().is_empty());
}

#[test]
fn hostile_reservation_blocks_later_player_hit() {
    // Hostile intents are produced before player intents, so the hostile
    // reserves the shared victim: its hit lands while the player's later
    // intent is rejected with no charge, wear, event or suppression.
    let mut state = authority();
    let player_key = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    ctx.stage(RuleEffect::Actor(hostile(7, [1.5, 1.0, -1.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    stage_player(&mut ctx, player_key, 1, [0.5, 1.0, -3.0], true);
    let mut aimed = ctx
        .read()
        .actor(ActorKey::Player(player_key))
        .unwrap()
        .clone();
    aimed.look = LookAngles::try_new(std::f32::consts::PI, 0.0).unwrap();
    ctx.stage(RuleEffect::Actor(aimed)).unwrap();
    let before = *ctx.read().inventory(ActorKey::Player(player_key)).unwrap();
    let mut armed = before;
    armed.slots[0] = ItemStack {
        item: 47,
        count: 1,
        durability: 2,
    };
    ctx.stage(RuleEffect::Inventory(
        mornlea_server::contracts::InventoryPatch {
            actor: ActorKey::Player(player_key),
            before,
            after: armed,
        },
    ))
    .unwrap();
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -1.5], false);
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(
        ctx.read().tick(),
        &[HostileMeleeAttack::new(attacker, victim)],
    )
    .unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(outcome.report.rejected, 1);
    assert_eq!(outcome.damaged_players, vec![victim]);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        17
    );
    assert_eq!(
        ctx.read()
            .runtime(ActorKey::Player(victim))
            .unwrap()
            .hurt_cooldown,
        20
    );
    assert_eq!(
        ctx.read()
            .runtime(ActorKey::Player(player_key))
            .unwrap()
            .attack_cooldown,
        0
    );
    assert_eq!(
        ctx.read()
            .runtime(ActorKey::Player(player_key))
            .unwrap()
            .exhaustion_milli,
        0
    );
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(player_key))
            .unwrap()
            .slots[0],
        ItemStack {
            item: 47,
            count: 1,
            durability: 2
        }
    );
    assert!(ctx.events().is_empty());
    assert!(!ctx.mining_suppressed(ActorKey::Player(player_key)));
}

#[test]
fn zero_health_actor_still_advances_cooldowns() {
    // Zero-health actors stay in the snapshot for cooldown progress even
    // though they can neither attack nor be selected.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    let mut record = hostile(7, [0.5, 1.0, 0.5]);
    record.survival = survival(0);
    let ActorBody::Hostile(body) = &mut record.body else {
        unreachable!()
    };
    body.health = 0;
    body.attack_cooldown = 2;
    body.hurt_cooldown = 5;
    ctx.stage(RuleEffect::Actor(record)).unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    stage_player(&mut ctx, victim, 1, [1.5, 1.0, 0.5], false);
    let batch = HostileMeleeBatch::try_new(
        ctx.read().tick(),
        &[HostileMeleeAttack::new(attacker, victim)],
    )
    .unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert!(outcome.damaged_players.is_empty());
    let ActorBody::Hostile(body) = &ctx.read().actor(ActorKey::Hostile(attacker)).unwrap().body
    else {
        unreachable!()
    };
    assert_eq!((body.attack_cooldown, body.hurt_cooldown), (1, 4));
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert!(ctx.events().is_empty());
}

#[test]
fn stale_selected_slot_refuses_hit() {
    // Only the frozen slot identifies the swing: moving the selection after
    // the freeze leaves every hit effect absent.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -1.0], false);
    let before = *ctx.read().inventory(ActorKey::Player(attacker)).unwrap();
    let mut armed = before;
    armed.slots[0] = ItemStack {
        item: 47,
        count: 1,
        durability: 2,
    };
    ctx.stage(RuleEffect::Inventory(
        mornlea_server::contracts::InventoryPatch {
            actor: ActorKey::Player(attacker),
            before,
            after: armed,
        },
    ))
    .unwrap();
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let frame = provider::freeze(&ctx, &batch).unwrap();
    let before = *ctx.read().inventory(ActorKey::Player(attacker)).unwrap();
    let mut after = before;
    after.selected = HotbarSlot::new(1).unwrap();
    ctx.stage(RuleEffect::Inventory(
        mornlea_server::contracts::InventoryPatch {
            actor: ActorKey::Player(attacker),
            before,
            after,
        },
    ))
    .unwrap();
    let outcome = provider::settle_frame(&mut ctx, frame).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert_eq!(outcome.report.rejected, 1);
    assert!(outcome.damaged_players.is_empty());
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert_eq!(
        ctx.read()
            .runtime(ActorKey::Player(victim))
            .unwrap()
            .hurt_cooldown,
        0
    );
    assert_eq!(
        ctx.read()
            .runtime(ActorKey::Player(attacker))
            .unwrap()
            .attack_cooldown,
        0
    );
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(attacker))
            .unwrap()
            .slots[0],
        ItemStack {
            item: 47,
            count: 1,
            durability: 2
        }
    );
    assert_eq!(
        ctx.read()
            .runtime(ActorKey::Player(attacker))
            .unwrap()
            .exhaustion_milli,
        0
    );
    assert!(ctx.events().is_empty());
    assert!(!ctx.mining_suppressed(ActorKey::Player(attacker)));
}

#[test]
fn stale_selected_count_refuses_hit() {
    // The frozen count is identity too: splitting the stack after the freeze
    // refuses the hit with no partial sword spend.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -1.0], false);
    let before = *ctx.read().inventory(ActorKey::Player(attacker)).unwrap();
    let mut armed = before;
    armed.slots[0] = ItemStack {
        item: 47,
        count: 1,
        durability: 2,
    };
    ctx.stage(RuleEffect::Inventory(
        mornlea_server::contracts::InventoryPatch {
            actor: ActorKey::Player(attacker),
            before,
            after: armed,
        },
    ))
    .unwrap();
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let frame = provider::freeze(&ctx, &batch).unwrap();
    let before = *ctx.read().inventory(ActorKey::Player(attacker)).unwrap();
    let mut after = before;
    after.slots[0].count = 2;
    ctx.stage(RuleEffect::Inventory(
        mornlea_server::contracts::InventoryPatch {
            actor: ActorKey::Player(attacker),
            before,
            after,
        },
    ))
    .unwrap();
    let outcome = provider::settle_frame(&mut ctx, frame).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert_eq!(outcome.report.rejected, 1);
    assert!(outcome.damaged_players.is_empty());
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(attacker))
            .unwrap()
            .slots[0],
        ItemStack {
            item: 47,
            count: 2,
            durability: 2
        }
    );
    assert!(ctx.events().is_empty());
    assert!(!ctx.mining_suppressed(ActorKey::Player(attacker)));
}

#[test]
fn stale_dimension_refuses_hit() {
    // A victim that changed dimension after the freeze is no longer the
    // frozen victim: no damage, no hurt cooldown, no attacker spend.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -1.0], false);
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let frame = provider::freeze(&ctx, &batch).unwrap();
    let mut moved = ctx.read().actor(ActorKey::Player(victim)).unwrap().clone();
    moved.dimension = Dimension::DEPTHS;
    let ActorBody::Player(body) = &mut moved.body else {
        unreachable!()
    };
    body.current.dimension = 1;
    ctx.stage(RuleEffect::Actor(moved)).unwrap();
    let outcome = provider::settle_frame(&mut ctx, frame).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert_eq!(outcome.report.rejected, 1);
    assert!(outcome.damaged_players.is_empty());
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert_eq!(
        ctx.read()
            .runtime(ActorKey::Player(victim))
            .unwrap()
            .hurt_cooldown,
        0
    );
    assert_eq!(
        ctx.read()
            .runtime(ActorKey::Player(attacker))
            .unwrap()
            .attack_cooldown,
        0
    );
    assert!(ctx.events().is_empty());
    assert!(!ctx.mining_suppressed(ActorKey::Player(attacker)));
}

#[test]
fn stale_lifecycle_refuses_hit() {
    // A victim that left `Active` after the freeze fails live validation:
    // the frozen hit settles nothing.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -1.0], false);
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let frame = provider::freeze(&ctx, &batch).unwrap();
    let mut ended = ctx.read().actor(ActorKey::Player(victim)).unwrap().clone();
    ended.lifecycle = ActorLifecycle::Dead;
    ctx.stage(RuleEffect::Actor(ended)).unwrap();
    let outcome = provider::settle_frame(&mut ctx, frame).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert_eq!(outcome.report.rejected, 1);
    assert!(outcome.damaged_players.is_empty());
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert_eq!(
        ctx.read()
            .runtime(ActorKey::Player(attacker))
            .unwrap()
            .attack_cooldown,
        0
    );
    assert!(ctx.events().is_empty());
    assert!(!ctx.mining_suppressed(ActorKey::Player(attacker)));
}

#[test]
fn unreduced_hits_wear_no_armor() {
    // Bare fists against bare skin deal raw `2` at zero points with nothing
    // to wear: the victim loses 2 health and both inventories read back
    // identical.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -1.0], false);
    let before_attacker = *ctx.read().inventory(ActorKey::Player(attacker)).unwrap();
    let before_victim = *ctx.read().inventory(ActorKey::Player(victim)).unwrap();
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        18
    );
    assert_eq!(
        ctx.read().inventory(ActorKey::Player(attacker)).unwrap(),
        &before_attacker
    );
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(victim))
            .unwrap()
            .armor,
        before_victim.armor
    );

    // The helper lower-bound row the settlement relies on: raw `1` at two
    // points floors to effective `1`, which equals the raw damage, so no
    // piece wears.
    let mut armor = [ItemStack::default(); 4];
    let effective =
        mornlea_server::rules::inventory::settle_damage(DamageCause::Melee, 1, 2, &mut armor);
    assert_eq!(effective, 1);
    assert_eq!(armor, [ItemStack::default(); 4]);
}

#[test]
fn non_sword_holds_no_wear() {
    // Only intact swords (`47..=49`) spend durability: a generic held item
    // keeps its exact stack across a successful hit.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    let before = *ctx.read().inventory(ActorKey::Player(attacker)).unwrap();
    let mut held = before;
    held.slots[0] = ItemStack {
        item: 1,
        count: 1,
        durability: 5,
    };
    ctx.stage(RuleEffect::Inventory(
        mornlea_server::contracts::InventoryPatch {
            actor: ActorKey::Player(attacker),
            before,
            after: held,
        },
    ))
    .unwrap();
    let target = ActorKey::Hostile(HostileId::try_new(7).unwrap());
    ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, -1.0])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(target)))
        .unwrap();
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(ctx.read().actor(target).unwrap().survival.health(), 18);
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(attacker))
            .unwrap()
            .slots[0],
        ItemStack {
            item: 1,
            count: 1,
            durability: 5
        }
    );
}

#[test]
fn victim_eating_and_bow_reset_on_hit() {
    // A landed hit interrupts the victim: eating progress and bow draw both
    // clear while the damage timer restarts.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, 0.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    stage_player(&mut ctx, victim, 1, [1.5, 1.0, 0.5], false);
    let mut busy = ctx
        .read()
        .runtime(ActorKey::Player(victim))
        .unwrap()
        .clone();
    busy.eating = Some(EatingProgress {
        slot: HotbarSlot::new(0).unwrap(),
        item: 1,
        ticks: 3,
    });
    busy.bow = Some(BowProgress {
        slot: HotbarSlot::new(0).unwrap(),
        ticks: 2,
    });
    busy.since_damage_ticks = 41;
    ctx.stage(RuleEffect::Runtime(busy)).unwrap();
    let batch = HostileMeleeBatch::try_new(
        ctx.read().tick(),
        &[HostileMeleeAttack::new(attacker, victim)],
    )
    .unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        17
    );
    let runtime = ctx.read().runtime(ActorKey::Player(victim)).unwrap();
    assert_eq!(runtime.eating, None);
    assert_eq!(runtime.bow, None);
    assert_eq!(runtime.since_damage_ticks, 0);
    assert_eq!(runtime.hurt_cooldown, 20);
}

#[test]
fn two_attackers_single_victim_single_charge() {
    // One victim takes exactly one hit no matter how many attackers select
    // it: the first intent reserves the victim, and only the winner spends a
    // sword point, takes the exhaustion charge and owns the `CombatHit`.
    let mut state = authority();
    let first = session(&mut state, 1);
    let second = session(&mut state, 2);
    let victim = session(&mut state, 3);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, first, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, second, 2, [0.5, 1.0, -3.5], true);
    let mut aimed = ctx.read().actor(ActorKey::Player(second)).unwrap().clone();
    aimed.look = LookAngles::try_new(std::f32::consts::PI, 0.0).unwrap();
    ctx.stage(RuleEffect::Actor(aimed)).unwrap();
    for key in [first, second] {
        let before = *ctx.read().inventory(ActorKey::Player(key)).unwrap();
        let mut armed = before;
        armed.slots[0] = ItemStack {
            item: 47,
            count: 1,
            durability: 2,
        };
        ctx.stage(RuleEffect::Inventory(
            mornlea_server::contracts::InventoryPatch {
                actor: ActorKey::Player(key),
                before,
                after: armed,
            },
        ))
        .unwrap();
    }
    stage_player(&mut ctx, victim, 3, [0.5, 1.0, -1.5], false);
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(outcome.report.rejected, 1);
    assert_eq!(outcome.damaged_players, vec![victim]);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        16
    );
    let first_won = ctx
        .read()
        .runtime(ActorKey::Player(first))
        .unwrap()
        .attack_cooldown
        == 10;
    let second_won = ctx
        .read()
        .runtime(ActorKey::Player(second))
        .unwrap()
        .attack_cooldown
        == 10;
    assert!(first_won ^ second_won, "exactly one attacker wins");
    let (winner, loser) = if first_won {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(winner))
            .unwrap()
            .slots[0],
        ItemStack {
            item: 47,
            count: 1,
            durability: 1
        }
    );
    assert_eq!(
        ctx.read()
            .runtime(ActorKey::Player(winner))
            .unwrap()
            .exhaustion_milli,
        100
    );
    assert_eq!(
        ctx.read().inventory(ActorKey::Player(loser)).unwrap().slots[0],
        ItemStack {
            item: 47,
            count: 1,
            durability: 2
        }
    );
    assert_eq!(
        ctx.read()
            .runtime(ActorKey::Player(loser))
            .unwrap()
            .exhaustion_milli,
        0
    );
    assert_eq!(ctx.events().len(), 1);
    assert_eq!(
        ctx.events()[0].recipient(),
        EventRecipient::Session(winner.get())
    );
    assert!(matches!(ctx.events()[0].event(), Event::CombatHit(_)));
}

#[test]
fn bucket_suppression_survives_successful_melee() {
    // A pre-existing mining suppression (such as a bucket receipt) is never
    // cleared: the successful hit keeps it while landing its own damage.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let victim = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    stage_player(&mut ctx, victim, 2, [0.5, 1.0, -1.0], false);
    ctx.suppress_mining(ActorKey::Player(attacker)).unwrap();
    air_ray(&mut ctx, None);
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        18
    );
    assert!(ctx.mining_suppressed(ActorKey::Player(attacker)));
}

#[test]
fn zero_exhaustion_threshold_pins_to_one_in_combat() {
    // A zero melee threshold settles exactly like a threshold of one: the
    // combat charge drains the same lanes to the same remainder while the
    // victim still takes its hit.
    let mut snapshots = Vec::new();
    for threshold in [0, 1] {
        let mut state = authority();
        let attacker = session(&mut state, 1);
        let mut ctx = TickContext::harness(&mut state, TickBudget::full());
        let defaults = RuleTunables::source_defaults();
        let tunables = RuleTunables::try_new(
            defaults.physics(),
            100,
            40,
            20,
            80,
            18,
            threshold,
            32,
            1_600,
            200,
            5,
            3,
            50,
            6.0,
            1.62,
            10,
            40,
            6_000,
            1.25,
        )
        .unwrap();
        ctx.stage(RuleEffect::Environment(EnvironmentState {
            seed: 7,
            next_tick: 1,
            world_time: 0,
            day_phase_offset: 0,
            season_offset: 0,
            weather: Weather::Clear,
            weather_remaining: 0,
            difficulty: 1,
            tunables,
        }))
        .unwrap();
        stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
        let target = ActorKey::Hostile(HostileId::try_new(7).unwrap());
        ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, -1.0])))
            .unwrap();
        ctx.stage(RuleEffect::Runtime(hostile_runtime(target)))
            .unwrap();
        air_ray(&mut ctx, None);
        let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
        let outcome = provider::advance(&mut ctx, &batch).unwrap();
        assert_eq!(outcome.report.applied, 1, "threshold {threshold}");
        assert_eq!(
            ctx.read().actor(target).unwrap().survival.health(),
            18,
            "threshold {threshold}"
        );
        let record = ctx
            .read()
            .actor(ActorKey::Player(attacker))
            .unwrap()
            .clone();
        let runtime = ctx
            .read()
            .runtime(ActorKey::Player(attacker))
            .unwrap()
            .clone();
        let ActorBody::Player(body) = &record.body else {
            unreachable!()
        };
        assert_eq!(
            (body.hunger, body.saturation_milli, body.exhaustion_milli),
            (0, 0, 0)
        );
        assert_eq!((runtime.exhaustion_milli, runtime.saturation_milli), (0, 0));
        assert_eq!(record.survival.hunger(), 0);
        snapshots.push((record, runtime));
    }
    assert_eq!(snapshots[0], snapshots[1], "zero pins to one");
}

// ---------------------------------------------------------------------
// Hostile and player death outputs and reset (`settleHostileDeaths`,
// `settleDeaths`, `packages/server/sim/entity/hostile.go` and `death.go`).
// ---------------------------------------------------------------------

fn death_call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::HostilePlayerDeaths,
        actor: None,
        command: None,
        internal: None,
    }
}

fn empty_chunk_data() -> Chunk {
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
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    }
}

fn preload_empty(ctx: &mut TickContext<'_>, dimension: Dimension, x: i32, z: i32) {
    ctx.preload_ready_chunk(
        ReadyChunk::try_new(
            ChunkKey {
                dimension,
                pos: ChunkPos::new(x, z),
            },
            1,
            1,
            empty_chunk_data(),
        )
        .unwrap(),
    );
}

fn dead_hostile(id: u64, kind: u8, position: [f32; 3]) -> ActorRecord {
    let mut record = hostile(id, position);
    record.survival = survival(0);
    let ActorBody::Hostile(body) = &mut record.body else {
        unreachable!()
    };
    body.health = 0;
    body.kind = kind;
    record
}

fn stage_dead_hostile(ctx: &mut TickContext<'_>, id: u64, kind: u8, position: [f32; 3]) {
    let key = ActorKey::Hostile(HostileId::try_new(id).unwrap());
    ctx.stage(RuleEffect::Actor(dead_hostile(id, kind, position)))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(key)))
        .unwrap();
}

fn environment_with(ctx: &mut TickContext<'_>, seed: i64, world_time: u64) {
    ctx.stage(RuleEffect::Environment(EnvironmentState {
        seed,
        next_tick: 1,
        world_time,
        day_phase_offset: 0,
        season_offset: 0,
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty: 1,
        tunables: RuleTunables::source_defaults(),
    }))
    .unwrap();
}

fn drop_stacks(ctx: &TickContext<'_>, key: ChunkKey) -> Vec<ItemStack> {
    ctx.read()
        .drops(key)
        .iter()
        .map(|record| record.stack)
        .collect()
}

/// Fills every physical drop slot of one ready chunk with single stones at
/// distinct cells, so later rehearsals refuse with exhausted capacity while
/// earlier fixtures stay observable.
fn fill_chunk(ctx: &mut TickContext<'_>, key: ChunkKey) {
    let tick = ctx.read().tick();
    for cell in 0..32 {
        let batch = DropBatch::try_new(
            DropSource::Death {
                actor: ActorKey::Hostile(HostileId::try_new(1).unwrap()),
                tick,
            },
            key.dimension,
            FiniteVec3::try_new([
                (key.pos.x() * 16 + (cell % 8)) as f32 + 0.5,
                70.5,
                (key.pos.z() * 16 + (cell / 8)) as f32 + 0.5,
            ])
            .unwrap(),
            vec![ItemStack {
                item: 1,
                count: 1,
                durability: 0,
            }],
            5,
        )
        .unwrap();
        ctx.stage(RuleEffect::Drops(batch)).unwrap();
    }
    assert_eq!(drop_stacks(ctx, key).len(), 32, "the chunk must fill");
}

fn stack(item: u16, count: u8, durability: u16) -> ItemStack {
    ItemStack {
        item,
        count,
        durability,
    }
}

fn sorted_stacks(mut stacks: Vec<ItemStack>) -> Vec<ItemStack> {
    stacks.sort_by_key(|stack| (stack.item, stack.count, stack.durability));
    stacks
}

#[test]
fn death_call_shape_refusals() {
    // Any shape beyond the bare batch call refuses before effects: a foreign
    // phase and a populated actor lane both report the combat call field.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, victim, 1, [1.5, 1.0, 0.5], false);
    let before = ctx.read().actor(ActorKey::Player(victim)).unwrap().clone();
    let Err(error) = provider::run(
        &mut ctx,
        RuleCall {
            phase: RulePhase::PassiveStepDeaths,
            actor: None,
            command: None,
            internal: None,
        },
    ) else {
        panic!("a foreign phase must refuse");
    };
    assert!(matches!(
        error,
        ServerError::InvalidInput {
            field: "combat_call"
        }
    ));
    let Err(error) = provider::run(
        &mut ctx,
        RuleCall {
            phase: RulePhase::HostilePlayerDeaths,
            actor: Some(ActorKey::Player(victim)),
            command: None,
            internal: None,
        },
    ) else {
        panic!("a populated actor lane must refuse");
    };
    assert!(matches!(
        error,
        ServerError::InvalidInput {
            field: "combat_call"
        }
    ));
    assert_eq!(ctx.read().actor(ActorKey::Player(victim)).unwrap(), &before);
    assert!(ctx.events().is_empty());
}

#[test]
fn walker_death_drops_single_flesh_and_marks_dead() {
    // A zero-health walker drops one rotten flesh at its death chunk and
    // leaves `Active` in the same settlement (`settleHostileDeaths`).
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    stage_dead_hostile(&mut ctx, 5, 0, [0.5, 1.0, 0.5]);
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        report,
        mornlea_server::contracts::PhaseReport {
            examined: 1,
            applied: 1,
            carried: 0,
            rejected: 0,
        }
    );
    let key = ActorKey::Hostile(HostileId::try_new(5).unwrap());
    let record = ctx.read().actor(key).unwrap();
    assert_eq!(record.lifecycle, ActorLifecycle::Dead);
    assert_eq!(record.survival.health(), 0);
    assert_eq!(
        drop_stacks(
            &ctx,
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, 0),
            }
        ),
        vec![stack(45, 1, 0)]
    );
    assert!(ctx.events().is_empty());
}

#[test]
fn hurler_death_matches_go_known_answer_pair() {
    // The hurler batch replays the frozen known answer bit-for-bit: seed `0`,
    // time `4000`, id `21` drops two bones plus a full-durability bow, drawn
    // once per kill and never re-rolled (`hostileDeathBatch`).
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment_with(&mut ctx, 0, 4000);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    stage_dead_hostile(&mut ctx, 21, 1, [0.5, 1.0, 0.5]);
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(
        drop_stacks(
            &ctx,
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, 0),
            }
        ),
        vec![stack(64, 2, 0), stack(62, 1, 120)]
    );
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Hostile(HostileId::try_new(21).unwrap()))
            .unwrap()
            .lifecycle,
        ActorLifecycle::Dead
    );
}

#[test]
fn hurler_empty_batch_skips_drops_but_marks_dead() {
    // Both hurler rolls missing stages no loot at all, and the death still
    // completes (`dropHostileLoot` early return).
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment_with(&mut ctx, 42, 4000);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    stage_dead_hostile(&mut ctx, 0xdead_beef_cafe_babe, 1, [0.5, 1.0, 0.5]);
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (1, 1, 0)
    );
    assert!(
        drop_stacks(
            &ctx,
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, 0),
            }
        )
        .is_empty()
    );
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Hostile(
                HostileId::try_new(0xdead_beef_cafe_babe).unwrap()
            ))
            .unwrap()
            .lifecycle,
        ActorLifecycle::Dead
    );
}

#[test]
fn walker_ring_spills_to_neighbor_when_death_chunk_full() {
    // A full death chunk refuses the rehearsal, so the first-fit walk spills
    // the whole batch onto the nearest ready ring neighbor at its clamped
    // column (`deathDropChunks` ring order).
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let home = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    };
    let neighbor = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(1, 0),
    };
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 1, 0);
    fill_chunk(&mut ctx, home);
    stage_dead_hostile(&mut ctx, 5, 0, [15.5, 1.0, 0.5]);
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (1, 1, 0)
    );
    assert_eq!(drop_stacks(&ctx, home).len(), 32);
    assert_eq!(drop_stacks(&ctx, neighbor), vec![stack(45, 1, 0)]);
    let landed = ctx.read().drops(neighbor)[0].position.get();
    assert_eq!(landed, [16.5, 1.5, 0.5]);
}

#[test]
fn walker_all_chunks_full_omits_loot_but_completes_death() {
    // No ready chunk with room omits the loot deterministically while the
    // death still completes; the omission is not a rejection.
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let home = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    };
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    fill_chunk(&mut ctx, home);
    stage_dead_hostile(&mut ctx, 5, 0, [0.5, 1.0, 0.5]);
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (1, 1, 0)
    );
    assert_eq!(drop_stacks(&ctx, home).len(), 32);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Hostile(HostileId::try_new(5).unwrap()))
            .unwrap()
            .lifecycle,
        ActorLifecycle::Dead
    );
}

#[test]
fn lower_hostile_id_loot_lands_first() {
    // Hostile deaths settle in hostile-ID ascending order, so the walker's
    // flesh occupies the first physical slot ahead of the hurler pair.
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment_with(&mut ctx, 0, 4000);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    stage_dead_hostile(&mut ctx, 21, 1, [0.5, 1.0, 0.5]);
    stage_dead_hostile(&mut ctx, 3, 0, [0.5, 1.0, 0.5]);
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!((report.examined, report.applied), (2, 2));
    assert_eq!(
        drop_stacks(
            &ctx,
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, 0),
            }
        ),
        vec![stack(45, 1, 0), stack(64, 2, 0), stack(62, 1, 120)]
    );
}

#[test]
fn walker_invalid_pose_marks_dead_without_loot() {
    // An out-of-span death pose refuses the block walk, so the death completes
    // lootless instead of saturating into a chunk.
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    stage_dead_hostile(&mut ctx, 5, 0, [1e20, 1.0, 0.5]);
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (1, 1, 0)
    );
    assert!(
        drop_stacks(
            &ctx,
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, 0),
            }
        )
        .is_empty()
    );
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Hostile(HostileId::try_new(5).unwrap()))
            .unwrap()
            .lifecycle,
        ActorLifecycle::Dead
    );
}

#[test]
fn dead_hostile_record_is_not_resettled() {
    // A `Dead` record is terminal: the runner sees no candidate and stages
    // nothing.
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    let key = ActorKey::Hostile(HostileId::try_new(5).unwrap());
    let mut record = dead_hostile(5, 0, [0.5, 1.0, 0.5]);
    record.lifecycle = ActorLifecycle::Dead;
    ctx.stage(RuleEffect::Actor(record.clone())).unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(key)))
        .unwrap();
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (0, 0, 0)
    );
    assert_eq!(ctx.read().actor(key).unwrap(), &record);
    assert!(
        drop_stacks(
            &ctx,
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, 0),
            }
        )
        .is_empty()
    );
}

#[test]
fn player_death_repacks_drops_resets_and_teleports_to_anchor() {
    // One death settles repack-first, per-slot ring drops with clear-on-
    // success, full stat reset and anchor teleport in a single settlement
    // (`settleDeath` plus `beginReset`).
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    let key = ActorKey::Player(victim);
    let mut record = player(victim, 1, [0.5, 1.0, 0.5]);
    record.survival = survival(0);
    let ActorBody::Player(body) = &mut record.body else {
        unreachable!()
    };
    body.health = 0;
    ctx.stage(RuleEffect::Actor(record)).unwrap();
    ctx.stage(RuleEffect::Runtime(runtime(key, false))).unwrap();
    let mut before = InventoryRecord::empty();
    before.crafting_size = CraftingSize::Workbench;
    before.slots[0] = stack(1, 10, 0);
    before.slots[5] = stack(35, 3, 0);
    before.armor[1] = stack(58, 1, 2);
    before.crafting[0] = stack(35, 10, 0);
    ctx.preload_inventory(key, before);
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        report,
        mornlea_server::contracts::PhaseReport {
            examined: 1,
            applied: 1,
            carried: 0,
            rejected: 0,
        }
    );
    // The grid recovered into the pack before the drop walk, and every placed
    // slot cleared: wheat from the grid merged with the pack wheat.
    let after = ctx.read().inventory(key).unwrap();
    assert_eq!(after.crafting_size, CraftingSize::Personal);
    assert_eq!(after.crafting, [ItemStack::default(); 9]);
    assert!(after.slots.iter().all(|slot| *slot == ItemStack::default()));
    assert!(after.armor.iter().all(|slot| *slot == ItemStack::default()));
    assert_eq!(
        sorted_stacks(drop_stacks(
            &ctx,
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, 0),
            }
        )),
        sorted_stacks(vec![stack(1, 10, 0), stack(35, 13, 0), stack(58, 1, 2)])
    );
    // Full stat reset with the bed record preserved and no events.
    let settled = ctx.read().actor(key).unwrap();
    assert_eq!(settled.lifecycle, ActorLifecycle::Respawning);
    assert_eq!(settled.survival.health(), 20);
    assert_eq!(settled.survival.hunger(), 20);
    assert_eq!(settled.motion.position().get(), [0.5, 321.0, 0.5]);
    assert_eq!(settled.motion.velocity().get(), [0.0; 3]);
    assert_eq!(settled.dimension, Dimension::OVERWORLD);
    let ActorBody::Player(save) = &settled.body else {
        unreachable!()
    };
    assert_eq!(
        (
            save.health,
            save.hunger,
            save.saturation_milli,
            save.exhaustion_milli
        ),
        (20, 20, 5_000, 0)
    );
    assert_eq!(
        save.current,
        PlayerLocation {
            dimension: 0,
            position: [0.5, 321.0, 0.5],
        }
    );
    assert!(!save.respawn_present);
    let settled_runtime = ctx.read().runtime(key).unwrap();
    assert_eq!(
        (
            settled_runtime.oxygen,
            settled_runtime.exhaustion_milli,
            settled_runtime.saturation_milli,
            settled_runtime.since_damage_ticks,
            settled_runtime.drown_ticks,
            settled_runtime.starvation_ticks,
            settled_runtime.attack_cooldown,
            settled_runtime.hurt_cooldown,
        ),
        (300, 0, 5_000, 0, 0, 0, 0, 0)
    );
    assert_eq!(settled_runtime.eating, None);
    assert_eq!(settled_runtime.bow, None);
    assert_eq!(settled_runtime.peak_y, 321.0);
    assert!(ctx.events().is_empty());
}

#[test]
fn player_repack_impossible_preserves_everything_for_retry() {
    // A pack with no room anywhere refuses the whole recovery: nothing
    // settles for that actor, the gate still matches next tick.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    let key = ActorKey::Player(victim);
    let mut record = player(victim, 1, [0.5, 1.0, 0.5]);
    record.survival = survival(0);
    let ActorBody::Player(body) = &mut record.body else {
        unreachable!()
    };
    body.health = 0;
    ctx.stage(RuleEffect::Actor(record)).unwrap();
    ctx.stage(RuleEffect::Runtime(runtime(key, false))).unwrap();
    let mut before = InventoryRecord::empty();
    before.slots = [stack(1, 64, 0); 36];
    before.crafting_size = CraftingSize::Workbench;
    before.crafting[0] = stack(1, 1, 0);
    ctx.preload_inventory(key, before);
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (1, 0, 1)
    );
    assert_eq!(ctx.read().inventory(key).unwrap(), &before);
    let settled = ctx.read().actor(key).unwrap();
    assert_eq!(settled.lifecycle, ActorLifecycle::Active);
    assert_eq!(settled.survival.health(), 0);
    assert!(
        drop_stacks(
            &ctx,
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, 0),
            }
        )
        .is_empty()
    );
    assert!(ctx.events().is_empty());
}

#[test]
fn player_unplaceable_slots_stay_for_respawn() {
    // Slots no ready chunk can take stay with the player: the death still
    // completes and nothing is destroyed.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let home = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    };
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    fill_chunk(&mut ctx, home);
    let key = ActorKey::Player(victim);
    let mut record = player(victim, 1, [0.5, 1.0, 0.5]);
    record.survival = survival(0);
    let ActorBody::Player(body) = &mut record.body else {
        unreachable!()
    };
    body.health = 0;
    ctx.stage(RuleEffect::Actor(record)).unwrap();
    ctx.stage(RuleEffect::Runtime(runtime(key, false))).unwrap();
    let mut before = InventoryRecord::empty();
    before.slots = [stack(1, 64, 0); 36];
    before.armor[1] = stack(58, 1, 2);
    before.crafting_size = CraftingSize::Workbench;
    ctx.preload_inventory(key, before);
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (1, 1, 0)
    );
    let after = ctx.read().inventory(key).unwrap();
    assert_eq!(after.slots, before.slots);
    assert_eq!(after.armor, before.armor);
    assert_eq!(after.crafting_size, CraftingSize::Personal);
    assert_eq!(drop_stacks(&ctx, home).len(), 32);
    let settled = ctx.read().actor(key).unwrap();
    assert_eq!(settled.lifecycle, ActorLifecycle::Respawning);
    assert_eq!(settled.survival.health(), 20);
    let ActorBody::Player(body) = &settled.body else {
        unreachable!()
    };
    assert_eq!(body.armor, before.armor);
}

fn death_capacity_chunk(
    ctx: &mut TickContext<'_>,
    key: ChunkKey,
    occupied: usize,
    partial: Option<ItemStack>,
    revision: u64,
) {
    let mut chunk = empty_chunk_data();
    for (index, slot) in chunk.drops.iter_mut().enumerate() {
        slot.generation = index as u32 + 10;
        if index < occupied {
            *slot = mornlea_storage::DropSlot {
                active: true,
                stack: stack(1, 64, 0),
                block_index: mornlea_domain::chunk_block_index(BlockPos::new(
                    key.pos.x() * 16 + (index % 16) as i32,
                    70,
                    key.pos.z() * 16 + (index / 16) as i32,
                )),
                age_ticks: 100 + index as u32,
                pickup_delay_ticks: 2,
                ..*slot
            };
        }
    }
    if let Some(stack) = partial {
        chunk.drops[0].stack = stack;
        chunk.drops[0].block_index = mornlea_domain::chunk_block_index(BlockPos::new(0, 1, 0));
    }
    ctx.preload_ready_chunk(ReadyChunk::try_new(key, 1, revision, chunk).unwrap());
}

fn stage_capacity_death(ctx: &mut TickContext<'_>, victim: SessionKey, before: InventoryRecord) {
    let key = ActorKey::Player(victim);
    let mut record = player(victim, 1, [0.5, 1.0, 0.5]);
    record.survival = survival(0);
    let ActorBody::Player(body) = &mut record.body else {
        unreachable!()
    };
    body.health = 0;
    body.armor = before.armor;
    ctx.stage(RuleEffect::Actor(record)).unwrap();
    ctx.stage(RuleEffect::Runtime(runtime(key, false))).unwrap();
    ctx.preload_inventory(key, before);
}

#[test]
fn cumulative_player_death_capacity_uses_neighbor_or_retains_later_stack() {
    for neighbor_ready in [false, true] {
        let mut state = authority();
        let victim = session(&mut state, 1);
        let mut ctx = TickContext::harness(&mut state, TickBudget::full());
        environment(&mut ctx);
        let home = ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        };
        let neighbor = ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(-1, -1),
        };
        death_capacity_chunk(&mut ctx, home, 31, None, 1);
        if neighbor_ready {
            preload_empty(&mut ctx, neighbor.dimension, -1, -1);
        }
        let original_drops = ctx.read().drops(home).to_vec();
        let mut before = InventoryRecord::empty();
        before.slots[0] = stack(3, 1, 0);
        before.slots[1] = stack(4, 2, 0);
        stage_capacity_death(&mut ctx, victim, before);
        let report = provider::run(&mut ctx, death_call()).unwrap();
        assert_eq!(
            (report.examined, report.applied, report.rejected),
            (1, 1, 0)
        );
        let key = ActorKey::Player(victim);
        let after = ctx.read().inventory(key).unwrap();
        assert_eq!(after.slots[0], ItemStack::default());
        assert_eq!(
            after.slots[1],
            if neighbor_ready {
                ItemStack::default()
            } else {
                before.slots[1]
            }
        );
        let local = ctx.read().drops(home).to_vec();
        assert_eq!(&local[..31], original_drops.as_slice());
        assert_eq!(local[31].id.slot(), 31);
        assert_eq!(local[31].id.generation(), 42);
        assert_eq!(local[31].stack, before.slots[0]);
        assert_eq!(local[31].position.get(), [0.5, 1.5, 0.5]);
        assert_eq!((local[31].age, local[31].pickup_delay), (0, 40));
        if neighbor_ready {
            let remote = ctx.read().drops(neighbor).to_vec();
            assert_eq!(remote.len(), 1);
            assert_eq!((remote[0].id.slot(), remote[0].id.generation()), (0, 1));
            assert_eq!(remote[0].position.get(), [-0.5, 1.5, -0.5]);
            assert_eq!(remote[0].stack, before.slots[1]);
        }
        let settled = ctx.read().actor(key).unwrap();
        assert_eq!(settled.lifecycle, ActorLifecycle::Respawning);
        assert_eq!(settled.survival.health(), 20);
        assert!(ctx.events().is_empty());
        let local = ctx.read().drops(home).to_vec();
        let remote = ctx.read().drops(neighbor).to_vec();
        let after = *after;
        let record = settled.clone();
        let report = provider::run(&mut ctx, death_call()).unwrap();
        assert_eq!(
            (report.examined, report.applied, report.rejected),
            (0, 0, 0)
        );
        assert_eq!(ctx.read().inventory(key), Some(&after));
        assert_eq!(ctx.read().actor(key), Some(&record));
        assert_eq!(ctx.read().drops(home), local);
        assert_eq!(ctx.read().drops(neighbor), remote);
    }
}

#[test]
fn cumulative_player_death_merge_observes_earlier_slot_and_failed_attempt_rollback() {
    for (occupied, initial, first, later, expected_local, expected_remote) in [
        (31, 62, 2, 64, vec![64, 64], vec![]),
        (31, 62, 3, 64, vec![64, 1], vec![64]),
        (32, 62, 1, 2, vec![63], vec![2]),
        (32, 62, 2, 3, vec![64], vec![3]),
        (32, 62, 3, 2, vec![64], vec![3]),
    ] {
        let mut state = authority();
        let victim = session(&mut state, 1);
        let mut ctx = TickContext::harness(&mut state, TickBudget::full());
        environment(&mut ctx);
        let home = ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        };
        let neighbor = ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(-1, -1),
        };
        death_capacity_chunk(&mut ctx, home, occupied, Some(stack(3, initial, 0)), 1);
        preload_empty(&mut ctx, neighbor.dimension, -1, -1);
        let original = ctx.read().drops(home).to_vec();
        let mut before = InventoryRecord::empty();
        before.slots[0] = stack(3, first, 0);
        before.slots[1] = stack(3, later, 0);
        stage_capacity_death(&mut ctx, victim, before);
        let report = provider::run(&mut ctx, death_call()).unwrap();
        assert_eq!((report.applied, report.rejected), (1, 0));
        let local = ctx.read().drops(home).to_vec();
        assert_eq!(&local[1..occupied], &original[1..]);
        assert_eq!(local[0].id, original[0].id);
        assert_eq!(local[0].age, original[0].age);
        assert_eq!(local[0].pickup_delay, 40);
        let counts: Vec<_> = local
            .iter()
            .filter(|drop| drop.stack.item == 3)
            .map(|drop| drop.stack.count)
            .collect();
        assert_eq!(counts, expected_local);
        let remote = ctx.read().drops(neighbor).to_vec();
        assert_eq!(
            remote
                .iter()
                .map(|drop| drop.stack.count)
                .collect::<Vec<_>>(),
            expected_remote
        );
        if let Some(drop) = remote.first() {
            assert_eq!((drop.id.slot(), drop.id.generation()), (0, 1));
            assert_eq!(drop.position.get(), [-0.5, 1.5, -0.5]);
            assert_eq!((drop.age, drop.pickup_delay), (0, 40));
        }
        let key = ActorKey::Player(victim);
        assert!(
            ctx.read()
                .inventory(key)
                .unwrap()
                .slots
                .iter()
                .all(|slot| *slot == ItemStack::default())
        );
        assert_eq!(
            ctx.read().actor(key).unwrap().lifecycle,
            ActorLifecycle::Respawning
        );
    }
}

#[test]
fn cumulative_player_death_revision_exhaustion_keeps_inventory_and_armor() {
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let home = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    };
    death_capacity_chunk(&mut ctx, home, 0, None, u64::MAX);
    let mut before = InventoryRecord::empty();
    before.slots[0] = stack(3, 2, 0);
    before.armor[1] = stack(58, 1, 2);
    stage_capacity_death(&mut ctx, victim, before);
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!((report.applied, report.rejected), (1, 0));
    let key = ActorKey::Player(victim);
    assert_eq!(ctx.read().inventory(key), Some(&before));
    assert!(ctx.read().drops(home).is_empty());
    let settled = ctx.read().actor(key).unwrap();
    assert_eq!(settled.lifecycle, ActorLifecycle::Respawning);
    let ActorBody::Player(body) = &settled.body else {
        unreachable!()
    };
    assert_eq!(body.armor, before.armor);
}

#[test]
fn cumulative_player_death_maximum_slots_preserves_durable_forms() {
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let home = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    };
    let neighbor = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(-1, -1),
    };
    preload_empty(&mut ctx, home.dimension, 0, 0);
    preload_empty(&mut ctx, neighbor.dimension, -1, -1);
    let mut before = InventoryRecord::empty();
    for (index, slot) in before.slots.iter_mut().enumerate() {
        *slot = stack(10, 1, index as u16 + 1);
    }
    for (index, slot) in before.armor.iter_mut().enumerate() {
        *slot = stack(58 + index as u16, 1, index as u16 + 1);
    }
    stage_capacity_death(&mut ctx, victim, before);
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!((report.applied, report.rejected), (1, 0));
    let local = ctx.read().drops(home).to_vec();
    let remote = ctx.read().drops(neighbor).to_vec();
    assert_eq!((local.len(), remote.len()), (32, 8));
    assert_eq!(
        local.iter().map(|drop| drop.stack).collect::<Vec<_>>(),
        before.slots[..32]
    );
    assert_eq!(
        remote.iter().map(|drop| drop.stack).collect::<Vec<_>>(),
        before.slots[32..]
            .iter()
            .chain(before.armor.iter())
            .copied()
            .collect::<Vec<_>>()
    );
    for (drops, origin) in [(&local, [0.5, 1.5, 0.5]), (&remote, [-0.5, 1.5, -0.5])] {
        for (index, drop) in drops.iter().enumerate() {
            assert_eq!((drop.id.slot(), drop.id.generation()), (index as u8, 1));
            assert_eq!(drop.position.get(), origin);
            assert_eq!((drop.age, drop.pickup_delay), (0, 40));
        }
    }
    let key = ActorKey::Player(victim);
    let after = ctx.read().inventory(key).unwrap();
    assert!(
        after
            .slots
            .iter()
            .chain(after.armor.iter())
            .all(|slot| *slot == ItemStack::default())
    );
    let settled = ctx.read().actor(key).unwrap();
    assert_eq!(settled.lifecycle, ActorLifecycle::Respawning);
    let ActorBody::Player(body) = &settled.body else {
        unreachable!()
    };
    assert_eq!(body.armor, [ItemStack::default(); 4]);
    assert_eq!(settled.survival.armor_points(), 0);
}

#[test]
fn cumulative_player_death_later_player_observes_prior_death_drops() {
    let mut state = authority();
    let first = session(&mut state, 1);
    let second = session(&mut state, 2);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let home = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    };
    let neighbor = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(-1, -1),
    };
    death_capacity_chunk(&mut ctx, home, 31, None, 1);
    preload_empty(&mut ctx, neighbor.dimension, -1, -1);
    for (victim, item) in [(first, 3), (second, 4)] {
        let mut before = InventoryRecord::empty();
        before.slots[0] = stack(item, 64, 0);
        before.slots[1] = stack(item + 2, 64, 0);
        stage_capacity_death(&mut ctx, victim, before);
    }
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!((report.applied, report.rejected), (2, 0));
    assert_eq!(ctx.read().drops(home)[31].stack, stack(3, 64, 0));
    assert_eq!(
        drop_stacks(&ctx, neighbor),
        vec![stack(5, 64, 0), stack(4, 64, 0), stack(6, 64, 0)]
    );
    for victim in [first, second] {
        let key = ActorKey::Player(victim);
        assert_eq!(
            ctx.read().actor(key).unwrap().lifecycle,
            ActorLifecycle::Respawning
        );
        assert!(
            ctx.read()
                .inventory(key)
                .unwrap()
                .slots
                .iter()
                .all(|slot| *slot == ItemStack::default())
        );
    }
}

#[test]
fn player_teleport_prefers_live_respawn_position() {
    // A live bed respawn wins over the world anchor: the reset lands exactly
    // on the recorded position and dimension.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    let key = ActorKey::Player(victim);
    let mut record = player(victim, 1, [0.5, 1.0, 0.5]);
    record.survival = survival(0);
    let ActorBody::Player(body) = &mut record.body else {
        unreachable!()
    };
    body.health = 0;
    body.respawn_present = true;
    body.respawn_position = [100.5, 70.0, -200.5];
    body.respawn_dimension = 0;
    ctx.stage(RuleEffect::Actor(record)).unwrap();
    ctx.stage(RuleEffect::Runtime(runtime(key, false))).unwrap();
    ctx.preload_inventory(key, InventoryRecord::empty());
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (1, 1, 0)
    );
    let settled = ctx.read().actor(key).unwrap();
    assert_eq!(settled.lifecycle, ActorLifecycle::Respawning);
    assert_eq!(settled.motion.position().get(), [100.5, 70.0, -200.5]);
    assert_eq!(settled.motion.velocity().get(), [0.0; 3]);
    assert_eq!(settled.dimension, Dimension::OVERWORLD);
    let ActorBody::Player(save) = &settled.body else {
        unreachable!()
    };
    assert!(save.respawn_present);
    assert_eq!(save.respawn_position, [100.5, 70.0, -200.5]);
}

#[test]
fn player_double_settle_noop_across_survival_provider() {
    // The runner flips the lifecycle exactly once: the survival provider then
    // refuses the already-reset record instead of settling again.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    let key = ActorKey::Player(victim);
    let mut record = player(victim, 1, [0.5, 1.0, 0.5]);
    record.survival = survival(0);
    let ActorBody::Player(body) = &mut record.body else {
        unreachable!()
    };
    body.health = 0;
    ctx.stage(RuleEffect::Actor(record)).unwrap();
    ctx.stage(RuleEffect::Runtime(runtime(key, false))).unwrap();
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(1, 5, 0);
    ctx.preload_inventory(key, inventory);
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!((report.examined, report.applied), (1, 1));
    let drops = drop_stacks(
        &ctx,
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        },
    );
    assert_eq!(drops, vec![stack(1, 5, 0)]);
    let Err(error) = survival_provider::run(
        &mut ctx,
        RuleCall {
            phase: RulePhase::PlayerPostPhysics,
            actor: Some(key),
            command: None,
            internal: None,
        },
    ) else {
        panic!("the survival provider must refuse the reset record");
    };
    assert!(matches!(
        error,
        ServerError::InvalidInput { field: "actor" }
    ));
    let settled = ctx.read().actor(key).unwrap();
    assert_eq!(settled.lifecycle, ActorLifecycle::Respawning);
    assert_eq!(settled.survival.health(), 20);
    assert_eq!(
        drop_stacks(
            &ctx,
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, 0),
            }
        ),
        drops
    );
}

#[test]
fn respawning_player_is_not_a_death_candidate() {
    // An already-reset record is skipped silently: no candidate, no staging.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    let key = ActorKey::Player(victim);
    let mut record = player(victim, 1, [0.5, 1.0, 0.5]);
    record.lifecycle = ActorLifecycle::Respawning;
    record.survival = survival(0);
    let ActorBody::Player(body) = &mut record.body else {
        unreachable!()
    };
    body.health = 0;
    ctx.stage(RuleEffect::Actor(record.clone())).unwrap();
    ctx.stage(RuleEffect::Runtime(runtime(key, false))).unwrap();
    ctx.preload_inventory(key, InventoryRecord::empty());
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (0, 0, 0)
    );
    assert_eq!(ctx.read().actor(key).unwrap(), &record);
}

// ---------------------------------------------------------------------
// Combat-to-death integration: `Combat` then `HostilePlayerDeaths` in one
// tick (`settleHostileDeaths`, plus `settleDeath` with `beginReset` for the
// mutual case).
// ---------------------------------------------------------------------

#[test]
fn lethal_player_hit_settles_hostile_death_once() {
    // The death phase stages nothing before combat kills: the negative pins
    // the order, then one killing intent settles loot plus `Dead` exactly
    // once across a repeated death phase.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    let victim = ActorKey::Hostile(HostileId::try_new(7).unwrap());
    let mut doomed = hostile(7, [0.5, 1.0, -1.0]);
    doomed.survival = survival(2);
    let ActorBody::Hostile(body) = &mut doomed.body else {
        unreachable!()
    };
    body.health = 2;
    ctx.stage(RuleEffect::Actor(doomed)).unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(victim)))
        .unwrap();
    air_ray(&mut ctx, None);
    let home = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, -1),
    };
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, -1);

    // Death before combat stages nothing: nobody sits at zero health yet.
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (0, 0, 0)
    );
    let record = ctx.read().actor(victim).unwrap();
    assert_eq!(record.survival.health(), 2);
    assert_eq!(record.lifecycle, ActorLifecycle::Active);
    assert!(drop_stacks(&ctx, home).is_empty());
    assert!(ctx.events().is_empty());

    // One bare-fist hit (`2` damage) lands the killing blow without settling
    // the death itself.
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert!(outcome.damaged_players.is_empty());
    let record = ctx.read().actor(victim).unwrap();
    assert_eq!(record.survival.health(), 0);
    assert_eq!(record.lifecycle, ActorLifecycle::Active);
    assert_eq!(ctx.events().len(), 1);

    // The same-tick death phase settles loot plus `Dead` without new events.
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (1, 1, 0)
    );
    let record = ctx.read().actor(victim).unwrap();
    assert_eq!(record.lifecycle, ActorLifecycle::Dead);
    assert_eq!(record.survival.health(), 0);
    assert_eq!(drop_stacks(&ctx, home), vec![stack(45, 1, 0)]);
    assert_eq!(ctx.events().len(), 1);

    // A repeated death phase settles nothing further: no second loot and no
    // duplicate removal.
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (0, 0, 0)
    );
    assert_eq!(
        ctx.read().actor(victim).unwrap().lifecycle,
        ActorLifecycle::Dead
    );
    assert_eq!(drop_stacks(&ctx, home), vec![stack(45, 1, 0)]);
    assert_eq!(ctx.events().len(), 1);
}

#[test]
fn mutual_lethal_hostile_and_player_both_settle() {
    // Simultaneous hostile-plus-player killing intents settle both deaths in
    // one death phase, each with its own loot staged.
    let mut state = authority();
    let player_key = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, player_key, 1, [0.5, 1.0, 0.5], true);
    let hostile_id = HostileId::try_new(7).unwrap();
    let hostile_key = ActorKey::Hostile(hostile_id);
    let mut doomed = hostile(7, [0.5, 1.0, -1.0]);
    doomed.survival = survival(2);
    let ActorBody::Hostile(body) = &mut doomed.body else {
        unreachable!()
    };
    body.health = 2;
    ctx.stage(RuleEffect::Actor(doomed)).unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(hostile_key)))
        .unwrap();
    // Lower the attacker to exactly one hostile hit (`3` damage).
    let mut frail = ctx
        .read()
        .actor(ActorKey::Player(player_key))
        .unwrap()
        .clone();
    frail.survival = survival(3);
    let ActorBody::Player(body) = &mut frail.body else {
        unreachable!()
    };
    body.health = 3;
    ctx.stage(RuleEffect::Actor(frail)).unwrap();
    air_ray(&mut ctx, None);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, -1);
    let player_chunk = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    };
    let hostile_chunk = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, -1),
    };

    // Death before combat stages nothing for either side.
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (0, 0, 0)
    );
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(player_key))
            .unwrap()
            .survival
            .health(),
        3
    );
    assert_eq!(ctx.read().actor(hostile_key).unwrap().survival.health(), 2);
    assert!(ctx.events().is_empty());

    // Both intents land: the hostile hit kills the player while the bare-fist
    // reply kills the hostile.
    let batch = HostileMeleeBatch::try_new(
        ctx.read().tick(),
        &[HostileMeleeAttack::new(hostile_id, player_key)],
    )
    .unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 2);
    assert_eq!(outcome.damaged_players, vec![player_key]);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(player_key))
            .unwrap()
            .survival
            .health(),
        0
    );
    assert_eq!(ctx.read().actor(hostile_key).unwrap().survival.health(), 0);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(player_key))
            .unwrap()
            .lifecycle,
        ActorLifecycle::Active
    );
    assert_eq!(
        ctx.read().actor(hostile_key).unwrap().lifecycle,
        ActorLifecycle::Active
    );
    assert_eq!(ctx.events().len(), 1);

    // One death phase settles both: `Dead` plus flesh for the hostile,
    // `Respawning` with an empty drop chunk for the looted-nothing player.
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (2, 2, 0)
    );
    assert_eq!(
        ctx.read().actor(hostile_key).unwrap().lifecycle,
        ActorLifecycle::Dead
    );
    let settled = ctx.read().actor(ActorKey::Player(player_key)).unwrap();
    assert_eq!(settled.lifecycle, ActorLifecycle::Respawning);
    assert_eq!(settled.survival.health(), 20);
    assert_eq!(drop_stacks(&ctx, hostile_chunk), vec![stack(45, 1, 0)]);
    assert!(drop_stacks(&ctx, player_chunk).is_empty());
    // No death or despawn events are fabricated: the only event stays the
    // attacker-only `CombatHit`.
    assert_eq!(ctx.events().len(), 1);
    assert_eq!(
        ctx.events()[0].recipient(),
        EventRecipient::Session(player_key.get())
    );
    assert!(matches!(ctx.events()[0].event(), Event::CombatHit(_)));
}

#[test]
fn stale_batches_refuse_without_effects() {
    // A batch minted for another tick refuses before any cooldown, loot,
    // health or event edit.
    let mut state = authority();
    let victim = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    let attacker = HostileId::try_new(7).unwrap();
    ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, 0.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(ActorKey::Hostile(
        attacker,
    ))))
    .unwrap();
    stage_player(&mut ctx, victim, 1, [1.5, 1.0, 0.5], false);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, 0);
    let home = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    };
    let batch =
        HostileMeleeBatch::try_new(0, &[HostileMeleeAttack::new(attacker, victim)]).unwrap();
    let Err(error) = provider::advance(&mut ctx, &batch) else {
        panic!("a stale batch tick must refuse");
    };
    assert!(matches!(
        error,
        ServerError::InvalidInput {
            field: "combat_tick"
        }
    ));
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(victim))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Hostile(attacker))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert!(drop_stacks(&ctx, home).is_empty());
    assert!(ctx.events().is_empty());

    // A batch naming a removed (`Dead`) actor settles nothing: the kill below
    // stages loot plus `Dead`, and the late batch leaves loot, health and
    // events exactly as the settlement left them.
    let mut state = authority();
    let hunter = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, hunter, 1, [0.5, 1.0, 0.5], true);
    let removed = ActorKey::Hostile(HostileId::try_new(7).unwrap());
    let mut doomed = hostile(7, [0.5, 1.0, -1.0]);
    doomed.survival = survival(2);
    let ActorBody::Hostile(body) = &mut doomed.body else {
        unreachable!()
    };
    body.health = 2;
    ctx.stage(RuleEffect::Actor(doomed)).unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(removed)))
        .unwrap();
    air_ray(&mut ctx, None);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, -1);
    let grave = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, -1),
    };
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!((report.examined, report.applied), (1, 1));
    let late = HostileMeleeBatch::try_new(
        ctx.read().tick(),
        &[HostileMeleeAttack::new(
            HostileId::try_new(7).unwrap(),
            hunter,
        )],
    )
    .unwrap();
    let outcome = provider::advance(&mut ctx, &late).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert!(outcome.damaged_players.is_empty());
    assert_eq!(
        ctx.read().actor(removed).unwrap().lifecycle,
        ActorLifecycle::Dead
    );
    assert_eq!(ctx.read().actor(removed).unwrap().survival.health(), 0);
    assert_eq!(
        ctx.read()
            .actor(ActorKey::Player(hunter))
            .unwrap()
            .survival
            .health(),
        20
    );
    assert_eq!(drop_stacks(&ctx, grave), vec![stack(45, 1, 0)]);
    assert_eq!(ctx.events().len(), 1);

    // A reordered identity refuses: swapping the hostile mob id after the
    // freeze rejects the hit with no damage, loot or event.
    let mut state = authority();
    let striker = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, striker, 1, [0.5, 1.0, 0.5], true);
    let target = ActorKey::Hostile(HostileId::try_new(7).unwrap());
    ctx.stage(RuleEffect::Actor(hostile(7, [0.5, 1.0, -1.0])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(target)))
        .unwrap();
    air_ray(&mut ctx, None);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, -1);
    let field = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, -1),
    };
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let frame = provider::freeze(&ctx, &batch).unwrap();
    let mut renamed = ctx.read().actor(target).unwrap().clone();
    let ActorBody::Hostile(body) = &mut renamed.body else {
        unreachable!()
    };
    body.id = 8;
    ctx.stage(RuleEffect::Actor(renamed)).unwrap();
    let outcome = provider::settle_frame(&mut ctx, frame).unwrap();
    assert_eq!(outcome.report.applied, 0);
    assert_eq!(outcome.report.rejected, 1);
    assert_eq!(ctx.read().actor(target).unwrap().survival.health(), 20);
    assert!(drop_stacks(&ctx, field).is_empty());
    assert!(ctx.events().is_empty());
}

#[test]
fn death_phase_follows_combat_phase_with_passives_untouched() {
    // The death phase visibly follows the combat phase in-fixture: kills land
    // only after combat staged them, and passive advancement is untouched.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    let victim = ActorKey::Hostile(HostileId::try_new(7).unwrap());
    let mut doomed = hostile(7, [0.5, 1.0, -1.0]);
    doomed.survival = survival(2);
    let ActorBody::Hostile(body) = &mut doomed.body else {
        unreachable!()
    };
    body.health = 2;
    ctx.stage(RuleEffect::Actor(doomed)).unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(victim)))
        .unwrap();
    let stray = ActorKey::Passive(PassiveId::try_new(11).unwrap());
    ctx.stage(RuleEffect::Actor(passive(11, [8.5, 1.0, 0.5])))
        .unwrap();
    ctx.stage(RuleEffect::Runtime(passive_runtime(stray)))
        .unwrap();
    air_ray(&mut ctx, None);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, -1);
    let home = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, -1),
    };
    let before_body = ctx.read().actor(stray).unwrap().clone();
    let before_runtime = ctx.read().runtime(stray).unwrap().clone();

    // Death before combat stages nothing: the kill has not been staged yet.
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (0, 0, 0)
    );
    let record = ctx.read().actor(victim).unwrap();
    assert_eq!(record.survival.health(), 2);
    assert_eq!(record.lifecycle, ActorLifecycle::Active);
    assert!(drop_stacks(&ctx, home).is_empty());

    // Combat through the phase dispatcher stages the kill but leaves the
    // record `Active`: settlement belongs to the death phase.
    let report = provider::run(
        &mut ctx,
        RuleCall {
            phase: RulePhase::Combat,
            actor: None,
            command: None,
            internal: None,
        },
    )
    .unwrap();
    assert_eq!(report.applied, 1);
    let record = ctx.read().actor(victim).unwrap();
    assert_eq!(record.survival.health(), 0);
    assert_eq!(record.lifecycle, ActorLifecycle::Active);
    assert!(drop_stacks(&ctx, home).is_empty());

    // The death phase visibly follows: it settles the staged kill with loot
    // plus `Dead`.
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (1, 1, 0)
    );
    assert_eq!(
        ctx.read().actor(victim).unwrap().lifecycle,
        ActorLifecycle::Dead
    );
    assert_eq!(drop_stacks(&ctx, home), vec![stack(45, 1, 0)]);

    // The stray passive never advanced: combat and death left it identical.
    assert_eq!(ctx.read().actor(stray).unwrap(), &before_body);
    assert_eq!(ctx.read().runtime(stray).unwrap(), &before_runtime);
}

#[test]
fn combat_hit_events_precede_death_staging() {
    // Attacker-only `CombatHit` events precede death staging, and the death
    // providers fabricate no death or despawn events.
    let mut state = authority();
    let attacker = session(&mut state, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    environment(&mut ctx);
    stage_player(&mut ctx, attacker, 1, [0.5, 1.0, 0.5], true);
    let victim = ActorKey::Hostile(HostileId::try_new(7).unwrap());
    let mut doomed = hostile(7, [0.5, 1.0, -1.0]);
    doomed.survival = survival(2);
    let ActorBody::Hostile(body) = &mut doomed.body else {
        unreachable!()
    };
    body.health = 2;
    ctx.stage(RuleEffect::Actor(doomed)).unwrap();
    ctx.stage(RuleEffect::Runtime(hostile_runtime(victim)))
        .unwrap();
    air_ray(&mut ctx, None);
    preload_empty(&mut ctx, Dimension::OVERWORLD, 0, -1);
    let home = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, -1),
    };
    assert!(ctx.events().is_empty());

    // The hit lands first with exactly one attacker-only `CombatHit`.
    let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).unwrap();
    let outcome = provider::advance(&mut ctx, &batch).unwrap();
    assert_eq!(outcome.report.applied, 1);
    assert_eq!(ctx.events().len(), 1);
    assert_eq!(
        ctx.events()[0].recipient(),
        EventRecipient::Session(attacker.get())
    );
    assert!(matches!(ctx.events()[0].event(), Event::CombatHit(_)));

    // Death staging settles loot plus `Dead` while the event lane stays
    // exactly as combat left it.
    let report = provider::run(&mut ctx, death_call()).unwrap();
    assert_eq!((report.examined, report.applied), (1, 1));
    assert_eq!(
        ctx.read().actor(victim).unwrap().lifecycle,
        ActorLifecycle::Dead
    );
    assert_eq!(drop_stacks(&ctx, home), vec![stack(45, 1, 0)]);
    assert_eq!(ctx.events().len(), 1);
    for staged in ctx.events() {
        assert_eq!(staged.recipient(), EventRecipient::Session(attacker.get()));
        assert!(matches!(staged.event(), Event::CombatHit(_)));
    }
}
