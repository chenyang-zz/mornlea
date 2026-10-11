//! Replay cases for the hostile action provider: pre-physics target facts,
//! walker melee intent batches, and hurler ranged shot production.
//!
//! Every frozen number below mirrors a Go oracle row, cited at its case:
//!
//! - Walker melee submitted every tick within 1.8 squared horizontal
//!   distance regardless of attack cooldown, hurlers never melee:
//!   `advanceRunners` in `packages/server/server/hostile_manager.go` with
//!   `TestHostileChaseHoldsAttackWhileCooldownActive`,
//!   `TestHostileChaseCooldownOneAttacksSameTick` and
//!   `TestHurlerNeverSubmitsMeleeIntentWhenWithinReach` in
//!   `packages/server/server/hostile_manager_test.go` /
//!   `hostile_manager_ranged_test.go`.
//! - Nearest active same-dimension target with the smaller `PlayerID` bytes
//!   winning an exact tie: `nearestTarget` / `onlineHostileTargets` in
//!   `packages/server/server/hostile_manager.go` (live means health nonzero).
//! - Hurler shots only at shoot cooldown 0 with a clear eye-to-eye segment,
//!   deterministic spread, damage 3, speed 22, cooldown 40:
//!   `considerHurlerShot`, `hostileRangedLineOfSight` and
//!   `advanceHurlerBand` in `packages/server/server/hostile_manager.go`,
//!   `settleHostileRangedShot` / `hostileShardVelocity` in
//!   `packages/server/sim/entity/hostile_action.go`,
//!   `HostileShotSpread` in `packages/server/updates/sampler.go` with
//!   `TestHostileShotSpreadKAT` in
//!   `packages/server/updates/sampler_test.go`, and
//!   `TestHurlerShootsWhenCooldownReadyAndLOSVisible` /
//!   `TestHurlerDoesNotShootWhenLOSBlocked` in
//!   `packages/server/server/hostile_manager_ranged_test.go`.
//!
//! No case chooses a value the oracle does not pin. The spread and velocity
//! mirrors below are checked against the Go KAT vectors before use.

use super::*;
use mornlea_domain::{ChunkPos, Dimension, FiniteVec3, LookAngles, MotionState, MotionStateParts};
use mornlea_domain::{ProjectileKind, SurvivalState, SurvivalStateParts, Weather};
use mornlea_server::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, BlockObservation,
    ChunkKey, EnvironmentState, RuleEffect, RulePhase, RuleTunables, SessionKey, TransportKind,
};
use mornlea_server::rules::{hostile_actions as provider, projectiles};
use mornlea_storage::{HostileMob, ItemStack, PlayerId as StoredPlayerId};

const AIR: u16 = 0;
const STONE: u16 = 2;

const NIGHTWALKER: u8 = 0;
const HURLER: u8 = 1;

const SEED: i64 = 7;
// Daytime world time: no spawn admission interferes with the scenes.
const NOON: u64 = 1000;
// Eye height (`defaultEyeHeight` in `packages/shared/physics/types.go`).
const EYE_HEIGHT: f32 = 1.62;

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        SEED,
    )
    .expect("authority")
}

fn admit_session(state: &mut AuthorityState, tag: u8, name: &str) -> SessionKey {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = mornlea_domain::PlayerId::try_from_bytes(bytes).expect("player id");
    let start = mornlea_protocol::LoginStart::new(id, name, 8).expect("login start");
    let inbound =
        mornlea_protocol::LoginStart::decode_inbound(&start.encode().expect("encoded").clone())
            .expect("inbound");
    let login = mornlea_protocol::admit_login(inbound).expect("admitted");
    state.admit(login, TransportKind::Memory).expect("session")
}

fn environment(world_time: u64) -> EnvironmentState {
    EnvironmentState {
        seed: SEED,
        next_tick: 0,
        world_time,
        day_phase_offset: 0,
        season_offset: 0,
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty: 0,
        tunables: RuleTunables::source_defaults(),
    }
}

fn stage_environment(context: &mut TickContext<'_>, world_time: u64) {
    context
        .stage(RuleEffect::Environment(environment(world_time)))
        .expect("environment");
}

fn uuid_bytes(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn player_record(
    session: SessionKey,
    player_tag: u8,
    dimension: Dimension,
    position: [f32; 3],
    health: u8,
) -> ActorRecord {
    let body = mornlea_storage::PlayerSave {
        player_id: StoredPlayerId::from_bytes(uuid_bytes(player_tag)),
        revision: 1,
        display_name: "Tester".to_owned(),
        current: mornlea_storage::PlayerLocation {
            dimension: dimension.get() as i32,
            position,
        },
        yaw: 0.0,
        pitch: 0.0,
        safe: None,
        inventory: mornlea_storage::Inventory::default(),
        health,
        hunger: 20,
        saturation_milli: 5_000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor: [ItemStack::default(); 4],
    };
    ActorRecord::try_new(
        ActorKey::Player(session),
        ActorLifecycle::Active,
        dimension,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("position"),
            velocity: FiniteVec3::try_new([0.0; 3]).expect("velocity"),
            on_ground: true,
        }),
        LookAngles::try_new(0.0, 0.0).expect("look"),
        SurvivalState::try_new(SurvivalStateParts {
            health,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .expect("survival"),
        ActorBody::Player(body),
    )
    .expect("player actor")
}

fn hostile_record(
    id: u64,
    position: [f32; 3],
    kind: u8,
    health: u8,
    attack_cooldown: u8,
) -> ActorRecord {
    let body = HostileMob {
        id,
        dimension: 0,
        position,
        velocity: [0.0; 3],
        on_ground: true,
        yaw: 0.0,
        health,
        attack_cooldown,
        hurt_cooldown: 0,
        burn_cooldown: 20,
        has_target: false,
        player_id: StoredPlayerId::from_bytes([0u8; 16]),
        next_repath_ticks: 0,
        distant_ticks: 0,
        kind,
    };
    ActorRecord::try_new(
        ActorKey::Hostile(mornlea_domain::HostileId::try_new(id).expect("hostile id")),
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
        ActorBody::Hostile(body),
    )
    .expect("hostile actor")
}

/// Stages pre-step hostile transients (`shootCooldown`, `fresh`) the way the
/// motion provider would have left them.
fn stage_hostile_runtime(context: &mut TickContext<'_>, id: u64, shoot_cooldown: u32, fresh: bool) {
    let key = ActorKey::Hostile(mornlea_domain::HostileId::try_new(id).expect("hostile id"));
    context
        .stage(RuleEffect::Runtime(ActorRuntime {
            key,
            controls: None,
            has_view: false,
            reset: false,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 20,
            oxygen: 0,
            peak_y: 0.0,
            exhaustion_milli: 0,
            saturation_milli: 0,
            since_damage_ticks: 0,
            drown_ticks: 0,
            starvation_ticks: 0,
            eating: None,
            bow: None,
            path: None,
            aux: ActorAux::Hostile {
                distant_ticks: 0,
                shoot_cooldown,
                fresh,
            },
        }))
        .expect("hostile runtime");
}

fn stage_actors(context: &mut TickContext<'_>, actors: &[ActorRecord]) {
    for record in actors {
        context
            .stage(RuleEffect::Actor(record.clone()))
            .expect("actor staging");
    }
}

fn overworld_cell(pos: mornlea_domain::BlockPos) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
    }
}

fn observe(context: &mut TickContext<'_>, pos: mornlea_domain::BlockPos, block: u16) {
    let observed =
        BlockObservation::try_new(overworld_cell(pos), 1, 1, pos, block).expect("observation");
    context.preload_block(observed);
}

/// Observed air corridor for one eye-to-eye segment at feet 1.0.
fn observe_corridor(context: &mut TickContext<'_>, x0: i32, x1: i32) {
    for x in x0..=x1 {
        observe(context, mornlea_domain::BlockPos::new(x, 2, 0), AIR);
    }
}

fn action_call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::HostileActions,
        actor: None,
        command: None,
        internal: None,
    }
}

fn hostile_body(context: &TickContext<'_>, id: u64) -> HostileMob {
    let record = context
        .read()
        .actors()
        .iter()
        .find(|actor| matches!(actor.key, ActorKey::Hostile(key) if key.get() == id))
        .expect("hostile record");
    let ActorBody::Hostile(body) = &record.body else {
        panic!("hostile body");
    };
    body.clone()
}

fn stage_target_runtime(context: &mut TickContext<'_>, key: ActorKey) -> ActorRuntime {
    let runtime = ActorRuntime {
        key,
        controls: None,
        has_view: true,
        reset: false,
        attack_cooldown: 13,
        hurt_cooldown: 17,
        burn_cooldown: 19,
        oxygen: 211,
        peak_y: 77.0,
        exhaustion_milli: 1250,
        saturation_milli: 8500,
        since_damage_ticks: 71,
        drown_ticks: 23,
        starvation_ticks: 31,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Player {
            respawn: None,
            workbench: None,
        },
    };
    context.stage(RuleEffect::Runtime(runtime.clone())).unwrap();
    runtime
}

#[test]
fn walker_targets_live_player_past_dead_nearest_and_dead_tie() {
    // Go `onlineHostileTargets` excludes zero health before `nearestTarget`
    // compares distance or UUID, so neither nearer nor tied dead players mask.
    for dead_x in [1.0, -1.0] {
        let mut state = authority();
        let dead_session = admit_session(&mut state, 1, "walker-dead");
        let live_session = admit_session(&mut state, 9, "walker-live");
        let foreign_session = admit_session(&mut state, 2, "walker-foreign");
        let mut context = TickContext::harness(&mut state, TickBudget::full());
        stage_environment(&mut context, NOON);
        let dead = player_record(dead_session, 1, Dimension::OVERWORLD, [dead_x, 1.0, 0.5], 0);
        stage_actors(
            &mut context,
            &[
                dead.clone(),
                player_record(live_session, 9, Dimension::OVERWORLD, [2.0, 1.0, 0.5], 20),
                player_record(foreign_session, 2, Dimension::DEPTHS, [0.75, 1.0, 0.5], 20),
                hostile_record(11, [0.5, 1.0, 0.5], NIGHTWALKER, 20, 0),
            ],
        );
        let runtime = stage_target_runtime(&mut context, dead.key);
        let plan = provider::plan(&context).unwrap();
        let entries = plan.melee_batch().entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].attacker().get(), 11);
        assert_eq!(entries[0].target(), live_session);
        let report = provider::apply(&mut context, plan).unwrap();
        assert_eq!(report.applied, 1);
        assert_eq!(context.read().actor(dead.key), Some(&dead));
        assert_eq!(context.read().runtime(dead.key), Some(&runtime));
        assert!(context.read().projectiles().is_empty());
    }
}

#[test]
fn hurler_shoots_live_target_past_nearest_dead_player() {
    let mut state = authority();
    let dead_session = admit_session(&mut state, 1, "hurler-dead");
    let live_session = admit_session(&mut state, 9, "hurler-live");
    let foreign_session = admit_session(&mut state, 2, "hurler-foreign");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    let dead = player_record(dead_session, 1, Dimension::OVERWORLD, [1.0, 1.0, 0.5], 0);
    stage_actors(
        &mut context,
        &[
            dead.clone(),
            player_record(live_session, 9, Dimension::OVERWORLD, [-9.5, 1.0, 0.5], 20),
            player_record(foreign_session, 2, Dimension::DEPTHS, [0.75, 1.0, 0.5], 20),
            hostile_record(11, [0.5, 1.0, 0.5], HURLER, 20, 0),
        ],
    );
    let runtime = stage_target_runtime(&mut context, dead.key);
    observe_corridor(&mut context, -10, 1);
    let report = provider::run(&mut context, action_call()).unwrap();
    assert_eq!(report.applied, 1);
    let shots = context.read().projectiles();
    assert_eq!(shots.len(), 1);
    assert_eq!(shots[0].kind, ProjectileKind::Shard);
    assert_eq!(
        shots[0].velocity.get(),
        shard_velocity_mirror([-10.0, 0.0, 0.0], SEED, context.read().tick(), 11)
    );
    assert!(
        shots[0].velocity.get()[0] < 0.0,
        "aim faces the live target"
    );
    assert_eq!(context.read().actor(dead.key), Some(&dead));
    assert_eq!(context.read().runtime(dead.key), Some(&runtime));
}

#[test]
fn two_dead_players_produce_no_walker_or_hurler_action() {
    for kind in [NIGHTWALKER, HURLER] {
        let mut state = authority();
        let first = admit_session(&mut state, 1, "dead-only-one");
        let second = admit_session(&mut state, 2, "dead-only-two");
        let mut context = TickContext::harness(&mut state, TickBudget::full());
        stage_environment(&mut context, NOON);
        let actors = [
            player_record(first, 1, Dimension::OVERWORLD, [1.0, 1.0, 0.5], 0),
            player_record(second, 2, Dimension::OVERWORLD, [-9.5, 1.0, 0.5], 0),
            hostile_record(11, [0.5, 1.0, 0.5], kind, 20, 0),
        ];
        stage_actors(&mut context, &actors);
        let first_runtime = stage_target_runtime(&mut context, actors[0].key);
        let second_runtime = stage_target_runtime(&mut context, actors[1].key);
        observe_corridor(&mut context, -10, 1);
        let plan = provider::plan(&context).unwrap();
        assert!(plan.melee_batch().entries().is_empty());
        let report = provider::apply(&mut context, plan).unwrap();
        assert_eq!(report.examined, 0);
        assert_eq!(report.applied, 0);
        assert_eq!(context.read().actors(), actors);
        assert_eq!(context.read().runtime(actors[0].key), Some(&first_runtime));
        assert_eq!(context.read().runtime(actors[1].key), Some(&second_runtime));
        assert!(context.read().projectiles().is_empty());
        assert!(context.events().is_empty());
    }
}

// -----------------------------------------------------------------------
// Deterministic shot math mirrors (`sampler.HostileShotSpread` and
// `hostileShardVelocity` in the Go rows cited above). The KAT check below
// pins this mirror against the Go vectors before any scene uses it.
// -----------------------------------------------------------------------

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// `HostileShotSpreadSalt` is ASCII "SHOTSPRD"; the single-axis bound is the
/// fixed 0.06 rad contract; the quantum keeps f32 precision exact.
fn spread_mirror(seed: i64, tick: u64, id: u64) -> (f32, f32) {
    const SALT: u64 = 0x5348_4F54_5350_5244;
    const MAX_RADIANS: f32 = 0.06;
    const QUANTUM: f32 = 1_048_576.0;
    let offset = |hash: u64| {
        let unit = (hash & (1_048_576 - 1)) as f32 / QUANTUM;
        (unit * 2.0 - 1.0) * MAX_RADIANS
    };
    let hash = splitmix64(splitmix64(splitmix64((seed as u64) ^ SALT) ^ tick) ^ id);
    (offset(hash), offset(splitmix64(hash)))
}

fn normalize_mirror(direction: [f32; 3]) -> [f32; 3] {
    let length = f64::from(direction[0])
        .hypot(f64::from(direction[1]))
        .hypot(f64::from(direction[2]));
    let inverse = (1.0 / length) as f32;
    direction.map(|component| component * inverse)
}

fn normalize_yaw_mirror(yaw: f32) -> f32 {
    let mut normalized = (f64::from(yaw) + std::f64::consts::PI) % (2.0 * std::f64::consts::PI);
    if normalized < 0.0 {
        normalized += 2.0 * std::f64::consts::PI;
    }
    (normalized - std::f64::consts::PI) as f32
}

/// `hostileShardVelocity`: yaw/pitch from the normalized aim, deterministic
/// spread offsets, pitch clamped to half pi, speed 22.
fn shard_velocity_mirror(aim: [f32; 3], seed: i64, tick: u64, id: u64) -> [f32; 3] {
    let unit = normalize_mirror(aim);
    let yaw = (-unit[0] as f64).atan2(-unit[2] as f64) as f32;
    let pitch = (f64::from(unit[1]).clamp(-1.0, 1.0).asin()) as f32;
    let (yaw_offset, pitch_offset) = spread_mirror(seed, tick, id);
    let yaw = normalize_yaw_mirror(yaw + yaw_offset);
    let pitch =
        (pitch + pitch_offset).clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
    let cos_pitch = f64::from(pitch).cos() as f32;
    [
        -(f64::from(yaw).sin() as f32) * cos_pitch * 22.0,
        (f64::from(pitch).sin() as f32) * 22.0,
        -(f64::from(yaw).cos() as f32) * cos_pitch * 22.0,
    ]
}

#[test]
fn spread_mirror_matches_go_kat_vectors() {
    // Vectors from `TestHostileShotSpreadKAT` in
    // `packages/server/updates/sampler_test.go`; the mirror must reproduce
    // them bit for bit before grounding the velocity scenes.
    for (seed, tick, id, yaw, pitch) in [
        (0, 1, 1, 0.0020711517, -0.0060293195),
        (42, 13000, 0xdeadbeefcafebabe, -0.049812812, 0.047607422),
        (-7, 987654321, 7777, 0.055351753, -0.058046035),
    ] {
        assert_eq!(
            spread_mirror(seed, tick, id),
            (yaw, pitch),
            "spread mirror drifted from the Go KAT"
        );
    }
    // Executed actual Go `Sampler.HostileShotSpread`, seed 7/id 11, pins
    // the two execution clocks used by the real provider scenes below.
    for (tick, yaw_bits, pitch_bits) in [
        (0, 0x3d5d_7829, 0xbcd8_8947),
        (13, 0x3d29_ef1e, 0xbbd5_a23d),
    ] {
        let (yaw, pitch) = spread_mirror(SEED, tick, 11);
        assert_eq!(yaw.to_bits(), yaw_bits);
        assert_eq!(pitch.to_bits(), pitch_bits);
    }
}

fn shot_at_clocks(tick: u64, world_time: u64) -> [f32; 3] {
    let mut state = authority();
    // Advance the real authority clock while empty, before session admission.
    for _ in 0..tick {
        state.advance_tick(TickBudget::full()).expect("empty tick");
    }
    let session = admit_session(&mut state, 1, "clock-shot");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    assert_eq!(context.read().tick(), tick);
    stage_environment(&mut context, world_time);
    let actors = [
        player_record(session, 1, Dimension::OVERWORLD, [10.5, 1.0, 0.5], 20),
        hostile_record(11, [0.5, 1.0, 0.5], HURLER, 20, 0),
    ];
    stage_actors(&mut context, &actors);
    let player_runtime = stage_target_runtime(&mut context, actors[0].key);
    stage_hostile_runtime(&mut context, 11, 0, false);
    let mut hostile_runtime = context.read().runtime(actors[1].key).unwrap().clone();
    hostile_runtime.attack_cooldown = 13;
    hostile_runtime.hurt_cooldown = 17;
    hostile_runtime.oxygen = 211;
    hostile_runtime.peak_y = 77.0;
    hostile_runtime.exhaustion_milli = 1250;
    context
        .stage(RuleEffect::Runtime(hostile_runtime.clone()))
        .unwrap();
    observe_corridor(&mut context, 0, 10);
    let blocks = context.changed_blocks();
    let events = context.events().to_vec();
    let snapshot = context.read().environment().cloned();
    let report = provider::run(&mut context, action_call()).unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(report.rejected, 0);
    let shots = context.read().projectiles();
    assert_eq!(shots.len(), 1);
    let shard = &shots[0];
    assert_eq!(shard.owner, actors[1].key);
    assert_eq!(shard.kind, ProjectileKind::Shard);
    assert_eq!(shard.dimension, Dimension::OVERWORLD);
    assert_eq!(shard.damage, 3);
    assert_eq!(shard.age, 0);
    assert_eq!(shard.position.get(), [0.5, 1.0 + EYE_HEIGHT, 0.5]);
    let velocity = shard.velocity.get();
    assert_eq!(
        velocity,
        shard_velocity_mirror([10.0, 0.0, 0.0], SEED, tick, 11),
        "execution tick {tick}, calendar time {world_time}"
    );
    let ActorAux::Hostile { shoot_cooldown, .. } = &mut hostile_runtime.aux else {
        unreachable!();
    };
    *shoot_cooldown = 40;
    assert_eq!(context.read().actors(), actors);
    assert_eq!(context.read().runtime(actors[0].key), Some(&player_runtime));
    assert_eq!(
        context.read().runtime(actors[1].key),
        Some(&hostile_runtime)
    );
    assert_eq!(context.read().environment(), snapshot.as_ref());
    assert_eq!(context.changed_blocks(), blocks);
    assert_eq!(context.events(), events);
    velocity
}

#[test]
fn calendar_time_does_not_change_actual_shot_spread() {
    for tick in [0, 13] {
        assert_eq!(shot_at_clocks(tick, 0), shot_at_clocks(tick, 1000));
    }
}

#[test]
fn execution_tick_changes_actual_shot_spread() {
    assert_ne!(shot_at_clocks(0, 0), shot_at_clocks(13, 0));
}

// -----------------------------------------------------------------------
// Melee production.
// -----------------------------------------------------------------------

/// A walker 1.5 blocks from its target submits one melee entry naming the
/// target session, every tick, with no cooldown or health side effects.
#[test]
fn walker_in_range_submits_melee_entry() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-one");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::OVERWORLD, [0.5, 1.0, 0.5], 20),
            hostile_record(11, [2.0, 1.0, 0.5], NIGHTWALKER, 20, 0),
        ],
    );
    let plan = provider::plan(&context).expect("action plan");
    let batch = plan.melee_batch();
    assert_eq!(
        batch.entries().len(),
        1,
        "one in-range walker admits one entry"
    );
    assert_eq!(batch.entries()[0].attacker().get(), 11);
    assert_eq!(batch.entries()[0].target(), session);
    let report = provider::apply(&mut context, plan).expect("action apply");
    assert_eq!(
        report,
        PhaseReport {
            examined: 1,
            applied: 1,
            carried: 0,
            rejected: 0,
        }
    );
    let body = hostile_body(&context, 11);
    assert_eq!(
        body.attack_cooldown, 0,
        "the producer never touches cooldowns"
    );
    assert_eq!(body.health, 20, "the producer never damages");
    assert!(
        context
            .read()
            .runtime(ActorKey::Hostile(
                mornlea_domain::HostileId::try_new(11).unwrap()
            ))
            .is_none(),
        "melee-only ticks stage no runtime"
    );
}

/// Cooldown 5 holds settlement but not production: the entry is still
/// admitted and every cooldown and health value is unchanged.
#[test]
fn walker_submits_while_attack_cooldown_active() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-cool");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::OVERWORLD, [0.5, 1.0, 0.5], 20),
            hostile_record(11, [2.0, 1.0, 0.5], NIGHTWALKER, 20, 5),
        ],
    );
    let plan = provider::plan(&context).expect("action plan");
    assert_eq!(
        plan.melee_batch().entries().len(),
        1,
        "cooling walkers still submit while in range"
    );
    provider::apply(&mut context, plan).expect("action apply");
    let body = hostile_body(&context, 11);
    assert_eq!(
        body.attack_cooldown, 5,
        "cooling timers are settlement state"
    );
    assert_eq!(body.hurt_cooldown, 0);
    assert_eq!(body.burn_cooldown, 20);
    assert_eq!(body.health, 20);
}

/// Ten blocks out, no entry and no candidates.
#[test]
fn walker_out_of_range_submits_nothing() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-far");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::OVERWORLD, [0.5, 1.0, 0.5], 20),
            hostile_record(11, [10.5, 1.0, 0.5], NIGHTWALKER, 20, 0),
        ],
    );
    let plan = provider::plan(&context).expect("action plan");
    assert!(plan.melee_batch().entries().is_empty());
    let report = provider::apply(&mut context, plan).expect("action apply");
    assert_eq!(report.examined, 0);
    assert_eq!(report.applied, 0);
    assert_eq!(report.rejected, 0);
}

/// Zero-health walkers neither attack nor shoot.
#[test]
fn dead_walker_submits_nothing() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-dead");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::OVERWORLD, [0.5, 1.0, 0.5], 20),
            hostile_record(11, [2.0, 1.0, 0.5], NIGHTWALKER, 0, 0),
        ],
    );
    let plan = provider::plan(&context).expect("action plan");
    assert!(plan.melee_batch().entries().is_empty());
}

/// Fresh spawns produce no intent on their spawn tick.
#[test]
fn fresh_spawn_submits_nothing() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-fresh");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::OVERWORLD, [0.5, 1.0, 0.5], 20),
            hostile_record(11, [2.0, 1.0, 0.5], NIGHTWALKER, 20, 0),
        ],
    );
    stage_hostile_runtime(&mut context, 11, 0, true);
    let plan = provider::plan(&context).expect("action plan");
    assert!(plan.melee_batch().entries().is_empty());
    let report = provider::apply(&mut context, plan).expect("action apply");
    assert_eq!(report.examined, 0);
}

/// A depths player is no target for an overworld walker.
#[test]
fn cross_dimension_target_excluded() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-depths");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::DEPTHS, [0.5, 1.0, 0.5], 20),
            hostile_record(11, [2.0, 1.0, 0.5], NIGHTWALKER, 20, 0),
        ],
    );
    let plan = provider::plan(&context).expect("action plan");
    assert!(plan.melee_batch().entries().is_empty());
}

/// An exact-distance tie takes the smaller `PlayerID` bytes: session 2
/// carries uuid tag 3 against session 1 uuid tag 9.
#[test]
fn equal_distance_tie_prefers_smaller_player_id() {
    let mut state = authority();
    let first = admit_session(&mut state, 1, "action-tie-one");
    let second = admit_session(&mut state, 2, "action-tie-two");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(first, 9, Dimension::OVERWORLD, [4.5, 1.0, 0.5], 20),
            player_record(second, 3, Dimension::OVERWORLD, [6.5, 1.0, 0.5], 20),
            hostile_record(11, [5.5, 1.0, 0.5], NIGHTWALKER, 20, 0),
        ],
    );
    let plan = provider::plan(&context).expect("action plan");
    let entries = plan.melee_batch().entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].target(),
        second,
        "smaller uuid bytes win the tie"
    );
}

/// A hurler inside melee reach submits no melee entry: with its shoot
/// cooldown at 1 it also stages no shot, so the tick is fully quiet.
#[test]
fn hurler_within_reach_submits_no_melee() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-hurler");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::OVERWORLD, [0.5, 1.0, 0.5], 20),
            hostile_record(11, [1.2, 1.0, 0.5], HURLER, 20, 0),
        ],
    );
    stage_hostile_runtime(&mut context, 11, 1, false);
    observe_corridor(&mut context, 0, 1);
    let plan = provider::plan(&context).expect("action plan");
    assert!(plan.melee_batch().entries().is_empty());
    let report = provider::apply(&mut context, plan).expect("action apply");
    assert_eq!(report.examined, 0);
    assert_eq!(report.applied, 0);
    assert!(context.read().projectiles().is_empty());
}

/// A zero-health player is never a target even while active.
#[test]
fn dead_player_is_never_target() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-ghost");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::OVERWORLD, [0.5, 1.0, 0.5], 0),
            hostile_record(11, [2.0, 1.0, 0.5], NIGHTWALKER, 20, 0),
        ],
    );
    let plan = provider::plan(&context).expect("action plan");
    assert!(plan.melee_batch().entries().is_empty());
}

/// A full 64-resident house in range yields an accepted 64-entry batch in
/// hostile-ID order.
#[test]
fn full_house_of_sixty_four_is_accepted() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-full");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    let mut actors = vec![player_record(
        session,
        1,
        Dimension::OVERWORLD,
        [0.5, 1.0, 0.5],
        20,
    )];
    for id in 1..=64u64 {
        actors.push(hostile_record(id, [1.0, 1.0, 0.5], NIGHTWALKER, 20, 0));
    }
    stage_actors(&mut context, &actors);
    let plan = provider::plan(&context).expect("action plan");
    let entries = plan.melee_batch().entries();
    assert_eq!(entries.len(), 64);
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(entry.attacker().get(), index as u64 + 1);
        assert_eq!(entry.target(), session);
    }
    let report = provider::apply(&mut context, plan).expect("action apply");
    assert_eq!(report.examined, 64);
    assert_eq!(report.applied, 64);
    assert_eq!(report.rejected, 0);
}

// -----------------------------------------------------------------------
// Hurler shots.
// -----------------------------------------------------------------------

/// Cooldown 0 plus a clear segment stages exactly one shard and the
/// 40-cooldown runtime, with source-exact eye math and spread bits.
#[test]
fn clear_shot_stages_shard_and_cooldown() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-shot");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::OVERWORLD, [10.5, 1.0, 0.5], 20),
            hostile_record(11, [0.5, 1.0, 0.5], HURLER, 20, 0),
        ],
    );
    observe_corridor(&mut context, 0, 10);
    let plan = provider::plan(&context).expect("action plan");
    assert!(
        plan.melee_batch().entries().is_empty(),
        "hurlers never melee"
    );
    let report = provider::apply(&mut context, plan).expect("action apply");
    assert_eq!(report.examined, 1);
    assert_eq!(report.applied, 1);
    assert_eq!(report.rejected, 0);
    let staged = context.read().projectiles();
    assert_eq!(staged.len(), 1, "exactly one shard");
    let shard = &staged[0];
    assert_eq!(shard.kind, ProjectileKind::Shard);
    assert_eq!(shard.damage, 3, "shard damage is the frozen 3");
    assert_eq!(shard.age, 0);
    assert_eq!(
        shard.owner,
        ActorKey::Hostile(mornlea_domain::HostileId::try_new(11).unwrap())
    );
    assert_eq!(shard.dimension, Dimension::OVERWORLD);
    // Eye math: feet plus the tunable eye height, both ends level.
    assert_eq!(shard.position.get(), [0.5, 1.0 + EYE_HEIGHT, 0.5]);
    // Spread bits: the KAT-pinned mirror drives the exact velocity.
    let aim = [10.0, 0.0, 0.0];
    let expected = shard_velocity_mirror(aim, SEED, context.read().tick(), 11);
    assert_eq!(
        shard.velocity.get(),
        expected,
        "spread bits match the oracle"
    );
    let speed = {
        let v = shard.velocity.get();
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    };
    assert!((speed - 22.0).abs() < 1e-3, "shard speed is the frozen 22");
    let runtime = context
        .read()
        .runtime(ActorKey::Hostile(
            mornlea_domain::HostileId::try_new(11).unwrap(),
        ))
        .expect("shot runtime");
    let ActorAux::Hostile { shoot_cooldown, .. } = runtime.aux else {
        panic!("hostile aux");
    };
    assert_eq!(
        shoot_cooldown, 40,
        "a fired shot consumes the full cooldown"
    );
}

/// Cooldown 1 stages nothing this tick even with a clear segment.
#[test]
fn cooling_hurler_stages_nothing() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-cool-shot");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::OVERWORLD, [10.5, 1.0, 0.5], 20),
            hostile_record(11, [0.5, 1.0, 0.5], HURLER, 20, 0),
        ],
    );
    stage_hostile_runtime(&mut context, 11, 1, false);
    observe_corridor(&mut context, 0, 10);
    let plan = provider::plan(&context).expect("action plan");
    let report = provider::apply(&mut context, plan).expect("action apply");
    assert_eq!(report.examined, 0);
    assert_eq!(report.applied, 0);
    assert!(context.read().projectiles().is_empty());
    let runtime = context
        .read()
        .runtime(ActorKey::Hostile(
            mornlea_domain::HostileId::try_new(11).unwrap(),
        ))
        .expect("preset runtime");
    let ActorAux::Hostile { shoot_cooldown, .. } = runtime.aux else {
        panic!("hostile aux");
    };
    assert_eq!(
        shoot_cooldown, 1,
        "the producer never drains shoot cooldowns"
    );
}

/// A stone cell on the eye segment blocks the shot; the cooldown stays ready.
#[test]
fn blocked_line_of_sight_stages_nothing() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-wall");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::OVERWORLD, [10.5, 1.0, 0.5], 20),
            hostile_record(11, [0.5, 1.0, 0.5], HURLER, 20, 0),
        ],
    );
    observe_corridor(&mut context, 0, 10);
    observe(&mut context, mornlea_domain::BlockPos::new(5, 2, 0), STONE);
    let plan = provider::plan(&context).expect("action plan");
    let report = provider::apply(&mut context, plan).expect("action apply");
    assert_eq!(report.examined, 0, "blocked shots are never admitted");
    assert_eq!(report.applied, 0);
    assert!(context.read().projectiles().is_empty());
    assert!(
        context
            .read()
            .runtime(ActorKey::Hostile(
                mornlea_domain::HostileId::try_new(11).unwrap()
            ))
            .is_none(),
        "a refused shot consumes no cooldown"
    );
}

/// An exhausted projectile port refuses the spawn: nothing stages and no
/// cooldown is consumed. The rehash window is occupied through the accepted
/// ports, so no hash bits are invented here.
#[test]
fn exhausted_projectile_capacity_stages_no_cooldown() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-full-port");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::OVERWORLD, [10.5, 1.0, 0.5], 20),
            hostile_record(11, [0.5, 1.0, 0.5], HURLER, 20, 0),
        ],
    );
    observe_corridor(&mut context, 0, 10);
    let plan = provider::plan(&context).expect("action plan");
    assert_eq!(plan.melee_batch().entries().len(), 0);
    // Occupy the whole 64-deep rehash window behind the admitted shot.
    let owner = ActorKey::Hostile(mornlea_domain::HostileId::try_new(11).unwrap());
    let eye = FiniteVec3::try_new([0.5, 1.0 + EYE_HEIGHT, 0.5]).expect("eye");
    for _ in 0..64 {
        let id = projectiles::derive_spawn_id(
            &context,
            ProjectileKind::Shard,
            Dimension::OVERWORLD,
            owner,
            eye,
        )
        .expect("rehash window has room");
        projectiles::spawn(
            &mut context,
            mornlea_server::contracts::ProjectileRecord {
                id,
                owner,
                dimension: Dimension::OVERWORLD,
                position: eye,
                velocity: FiniteVec3::try_new([22.0, 0.0, 0.0]).expect("velocity"),
                kind: ProjectileKind::Shard,
                damage: 3,
                age: 0,
            },
        )
        .expect("port accepts the filler");
    }
    assert_eq!(context.read().projectiles().len(), 64);
    let report = provider::apply(&mut context, plan).expect("action apply");
    assert_eq!(report.applied, 0, "the refused shot stages no shard");
    assert_eq!(report.rejected, 1, "the refused shot counts rejected");
    assert_eq!(context.read().projectiles().len(), 64, "no eviction either");
    assert!(
        context
            .read()
            .runtime(ActorKey::Hostile(
                mornlea_domain::HostileId::try_new(11).unwrap()
            ))
            .is_none(),
        "a refused spawn consumes no cooldown"
    );
}

/// A candidate that goes stale before apply (cooldown now nonzero) stages
/// nothing for that intent without an error.
#[test]
fn stale_cooldown_at_apply_stages_nothing() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-stale");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::OVERWORLD, [10.5, 1.0, 0.5], 20),
            hostile_record(11, [0.5, 1.0, 0.5], HURLER, 20, 0),
        ],
    );
    observe_corridor(&mut context, 0, 10);
    let plan = provider::plan(&context).expect("action plan");
    stage_hostile_runtime(&mut context, 11, 5, false);
    let report = provider::apply(&mut context, plan).expect("action apply");
    assert_eq!(report.applied, 0);
    assert_eq!(report.rejected, 1, "the stale intent counts rejected");
    assert!(context.read().projectiles().is_empty());
}

/// A plan applied on a later tick refuses before any effect.
#[test]
fn tick_mismatch_refuses_before_effects() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-tick");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::OVERWORLD, [10.5, 1.0, 0.5], 20),
            hostile_record(11, [0.5, 1.0, 0.5], HURLER, 20, 0),
        ],
    );
    observe_corridor(&mut context, 0, 10);
    let plan = provider::plan(&context).expect("action plan");
    drop(context);
    state.advance_tick(TickBudget::full()).expect("next tick");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[
            player_record(session, 1, Dimension::OVERWORLD, [10.5, 1.0, 0.5], 20),
            hostile_record(11, [0.5, 1.0, 0.5], HURLER, 20, 0),
        ],
    );
    observe_corridor(&mut context, 0, 10);
    let err = provider::apply(&mut context, plan).expect_err("stale plan must refuse");
    assert_eq!(
        err,
        ServerError::InvalidInput {
            field: "hostile_action_tick"
        }
    );
    assert!(context.read().projectiles().is_empty());
    assert!(
        context
            .read()
            .runtime(ActorKey::Hostile(
                mornlea_domain::HostileId::try_new(11).unwrap()
            ))
            .is_none(),
        "a refused tick stages nothing"
    );
}

/// Only an empty `HostileActions` call runs; anything else refuses.
#[test]
fn invalid_call_shape_is_refused() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-shape");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, NOON);
    stage_actors(
        &mut context,
        &[player_record(
            session,
            1,
            Dimension::OVERWORLD,
            [0.5, 1.0, 0.5],
            20,
        )],
    );
    let addressed = provider::run(
        &mut context,
        RuleCall {
            phase: RulePhase::HostileActions,
            actor: Some(ActorKey::Player(session)),
            command: None,
            internal: None,
        },
    )
    .expect_err("addressed calls must refuse");
    assert_eq!(
        addressed,
        ServerError::InvalidInput {
            field: "hostile_action_call"
        }
    );
    let wrong_phase = provider::run(
        &mut context,
        RuleCall {
            phase: RulePhase::HostileMotion,
            actor: None,
            command: None,
            internal: None,
        },
    )
    .expect_err("other phases must refuse");
    assert_eq!(wrong_phase, ServerError::InvalidInput { field: "phase" });
    // The accepted shape runs end to end through plan plus apply.
    let report = provider::run(&mut context, action_call()).expect("empty call runs");
    assert_eq!(report.carried, 0);
}

/// Planning without an environment snapshot is an internal invariant breach.
#[test]
fn missing_environment_is_internal() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "action-no-env");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_actors(
        &mut context,
        &[player_record(
            session,
            1,
            Dimension::OVERWORLD,
            [0.5, 1.0, 0.5],
            20,
        )],
    );
    let err = provider::plan(&context).expect_err("plan needs the environment");
    assert!(
        matches!(err, ServerError::Internal { .. }),
        "missing environment is internal, got {err:?}"
    );
}

fn geometry_snapshot(ctx: &TickContext<'_>) -> mornlea_server::contracts::FixtureState {
    ctx.snapshot_state(
        WorldState::try_new(mornlea_domain::WorldStateParts {
            day_phase_offset: 0,
            world_time_ticks: 0,
            weather: Weather::Clear,
            season: mornlea_domain::Season::Spring,
            season_progress: 0,
            temperature: 0,
        })
        .unwrap(),
    )
}

#[test]
fn geometry_shot_planning_refuses_unrepresentable_eyes() {
    for bad_hostile in [true, false] {
        for axis in [0, 1, 2] {
            let mut state = authority();
            let session = admit_session(&mut state, 1, "eye-geometry");
            let mut ctx = TickContext::harness(&mut state, TickBudget::full());
            stage_environment(&mut ctx, NOON);
            let mut hostile = [0.5, 1.0, 0.5];
            let mut target = [10.5, 1.0, 0.5];
            if bad_hostile {
                hostile[axis] = i32::MAX as f32;
            } else {
                target[axis] = i32::MAX as f32;
            }
            stage_actors(
                &mut ctx,
                &[
                    hostile_record(11, hostile, HURLER, 20, 0),
                    player_record(session, 1, Dimension::OVERWORLD, target, 20),
                ],
            );
            stage_hostile_runtime(&mut ctx, 11, 0, false);
            let before = geometry_snapshot(&ctx);
            let events = ctx.events().to_vec();
            assert!(matches!(
                provider::plan(&ctx),
                Err(ServerError::InvalidInput { field: "actor" })
            ));
            assert_eq!(geometry_snapshot(&ctx), before);
            assert_eq!(ctx.events(), events);
        }
    }
}

#[test]
fn geometry_negative_clear_shot_remains_admitted() {
    let mut state = authority();
    let session = admit_session(&mut state, 1, "negative-shot");
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut ctx, NOON);
    stage_actors(
        &mut ctx,
        &[
            hostile_record(11, [-10.5, 1.0, 0.5], HURLER, 20, 0),
            player_record(session, 1, Dimension::OVERWORLD, [-0.5, 1.0, 0.5], 20),
        ],
    );
    observe_corridor(&mut ctx, -11, -1);
    let plan = provider::plan(&ctx).unwrap();
    let report = provider::apply(&mut ctx, plan).unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(ctx.read().projectiles().len(), 1);
}
