//! Bounded melee freeze and live settlement replay.

use super::*;
use mornlea_domain::{
    BlockPos, ChunkPos, Dimension, Event, EventRecipient, FiniteVec3, HeldActions, HostileId,
    HotbarSlot, LookAngles, MotionState, MotionStateParts, Movement, PassiveId, PlayerControl,
    PlayerControlParts, SurvivalState, SurvivalStateParts, Weather,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::{
    ActorAux, ActorBody, ActorLifecycle, ActorRecord, ActorRuntime, BlockObservation, BowProgress,
    ChunkKey, DamageCause, EatingProgress, EnvironmentState, HostileMeleeAttack, HostileMeleeBatch,
    InventoryRecord, RuleEffect, RuleTunables, ServerError, SessionKey, TransportKind,
};
use mornlea_server::rules::hostile_outcomes as provider;
use mornlea_storage::{HostileMob, ItemStack, PassiveMob, PlayerLocation, PlayerSave};

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
