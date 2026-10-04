//! Replay cases for the hostile lifecycle provider: night spawn admission,
//! deterministic targeting, kernel movement, and the burn/distant pass.
//!
//! Every frozen number below mirrors a Go oracle row, cited at its case:
//!
//! - Spawn window 13000..=23000 on the seasonally effective phase, peaceful
//!   gating at entry, one candidate per tick anchored at the sorted-session
//!   `worldtime % len` pick, `SplitMix64(seed ^ time)` radius 24..=48 with the
//!   +X/-X/+Z/-Z axis table, the candidate-hash low-byte < 13 gate, the
//!   `hash % 3` kind dispatch, the per-kind 48-block local caps (walker 8,
//!   hurler 4) and the global 64 resident cap:
//!   `advanceHostileSpawn` in `packages/server/sim/entity/hostile_spawn.go`
//!   with `TestHostileSpawnOnlyWithinNightWindow`,
//!   `TestHostileSpawnPicksAnchorBySortedSessionAndWorldTime`,
//!   `TestHostileSpawnColumnDerivationStaysInContractWindow`,
//!   `TestHostileSpawnRejectsAtGlobalCap`,
//!   `TestHostileSpawnPeacefulGatesAtEntry`,
//!   `TestHostileSpawnGateMatchesHashLowByte` and
//!   `TestHostileSpawnKindFollowsCandidateHashRule` in
//!   `packages/server/sim/entity/hostile_spawn_test.go` /
//!   `hostile_kind_test.go`.
//! - The dark-spawn light boundary (block light <= 7 at the candidate, torch
//!   falloff 14 - distance): `hostileBlockLight` in
//!   `packages/server/sim/entity/block_light_query.go` with
//!   `TestHostileSpawnRejectsBrightCandidate` in `hostile_spawn_test.go`.
//! - Movement through the F1 physics kernel with the neutral reset, the
//!   fresh-spawn skip and removal below the world floor:
//!   `advanceHostileMovement` in `packages/server/sim/entity/hostile.go` with
//!   `TestHostileMovementReusesPlayerPhysicsStep` and
//!   `TestHostileMovementAdvancesEachActorOnceInIDOrder` in
//!   `packages/server/sim/entity/hostile_test.go`.
//! - Targeting with the equal-distance stable-ID tie:
//!   `nearestTarget` in `packages/server/server/hostile_manager.go`.
//! - Daylight burn every 20th exposed tick with covered/night reset, and the
//!   distant despawn (horizontal > 64 from every active same-dimension player
//!   accumulates to 600, then a no-drop removal; <= 64 resets):
//!   `advanceHostileBurn` / `advanceHostileDistant` in
//!   `packages/server/sim/entity/hostile.go` with
//!   `TestHostileBurnDamagesEveryTwentyTicksWhenExposed`,
//!   `TestHostileBurnTimerResetsAtNight`,
//!   `TestHostileDistantDespawnAfterSixHundredActiveTicks` and
//!   `TestHostileDistantCounterResetsWithinRange` in
//!   `packages/server/sim/entity/hostile_lifecycle_test.go`.
//!
//! No case chooses a value the oracle does not pin.

use super::*;
use mornlea_domain::{
    ChunkPos, Dimension, FiniteVec3, LookAngles, MotionState, MotionStateParts, SurvivalState,
    SurvivalStateParts, Weather,
};
use mornlea_server::contracts::{
    ActorAux, ActorBody, ActorLifecycle, ActorRecord, ActorRuntime, BlockObservation, ChunkKey,
    EnvironmentState, RuleEffect, RulePhase, RuleTunables, SessionKey, TransportKind,
};
use mornlea_server::rules::hostile_actors as provider;
use mornlea_storage::{HostileMob, ItemStack, PlayerId as StoredPlayerId};

// Frozen block and world bounds, mirrored from `packages/shared/core`
// (`block.go` id order, `pos.go` MinY/MaxY).
const AIR: u16 = 0;
const STONE: u16 = 2;
const GRASS: u16 = 4;
const WATER: u16 = 27;
const TORCH_STANDING: u16 = 71;
const MAX_Y: i32 = 320;

// Hostile kind bytes (`hostile.go`: nightwalker 0, bone thrower 1).
const NIGHTWALKER: u8 = 0;
const HURLER: u8 = 1;

// Frozen spawn constants (`hostile_spawn.go` const block).
const NIGHT_START: u64 = 13000;
const NIGHT_END: u64 = 23000;
const SPAWN_GATE: u64 = 13;

// Season clock constants (`core.YearTicks`, `core.SeasonLengthTicks`).
const YEAR_TICKS: u64 = 288_000;

// World seed used by every scene below.
const SEED: i64 = 7;

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        SEED,
    )
    .expect("authority")
}

/// Mints two real session keys (1 and 2) through the admission path.
fn two_sessions(state: &mut AuthorityState) -> (SessionKey, SessionKey) {
    let first = admit_session(state, 1, "hostile-one");
    let second = admit_session(state, 2, "hostile-two");
    (first, second)
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

fn environment(world_time: u64, difficulty: u8) -> EnvironmentState {
    EnvironmentState {
        seed: SEED,
        next_tick: 0,
        world_time,
        day_phase_offset: 0,
        season_offset: equinox_offset(world_time),
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty,
        tunables: RuleTunables::source_defaults(),
    }
}

/// Season offset that puts the tick on the equinox (`DayArcTicks` 12000), so
/// the season warp is the identity and the effective phase equals
/// `worldtime % 24000`. Mirrors the `yearIndex == 0` anchor of
/// `TestHostileBurnFollowsSeasonalEffectivePhase` (equinox case).
fn equinox_offset(world_time: u64) -> u32 {
    ((YEAR_TICKS - world_time % YEAR_TICKS) % YEAR_TICKS) as u32
}

fn stage_environment(context: &mut TickContext<'_>, world_time: u64, difficulty: u8) {
    context
        .stage(RuleEffect::Environment(environment(world_time, difficulty)))
        .expect("environment");
}

/// Mints one real session key through the admission path; session keys are
/// process-local nonzero ids, so a key minted before the harness context
/// borrows the authority is a valid fixture identity.
fn anchor_session(state: &mut AuthorityState) -> SessionKey {
    admit_session(state, 1, "hostile-anchor")
}

fn uuid_bytes(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

#[allow(clippy::too_many_arguments)]
fn player_actor(session: SessionKey, player_tag: u8, position: [f32; 3]) -> ActorRecord {
    let mut body = mornlea_storage::PlayerSave {
        player_id: StoredPlayerId::from_bytes(uuid_bytes(player_tag)),
        revision: 1,
        display_name: "Tester".to_owned(),
        current: mornlea_storage::PlayerLocation {
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
    };
    body.current.position = position;
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
        ActorBody::Player(body),
    )
    .expect("player actor")
}

/// One restored hostile record (`validTestHostile` in
/// `packages/server/sim/entity/hostile_test.go`): full health, burn timer at
/// the full 20-tick period, grounded at the given position.
fn hostile_actor(id: u64, position: [f32; 3], kind: u8) -> ActorRecord {
    hostile_actor_with(
        id,
        position,
        kind,
        20,
        0,
        false,
        StoredPlayerId::from_bytes([0u8; 16]),
    )
}

#[allow(clippy::too_many_arguments)]
fn hostile_actor_with(
    id: u64,
    position: [f32; 3],
    kind: u8,
    burn_cooldown: u8,
    distant_ticks: u16,
    has_target: bool,
    target: StoredPlayerId,
) -> ActorRecord {
    let body = HostileMob {
        id,
        dimension: 0,
        position,
        velocity: [0.0; 3],
        on_ground: true,
        yaw: 0.0,
        health: 20,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown,
        has_target,
        player_id: target,
        next_repath_ticks: 0,
        distant_ticks,
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

/// The three cells of one flat spawn column: support at y=0, two air above.
fn preload_flat_column(context: &mut TickContext<'_>, x: i32, z: i32) {
    observe(context, mornlea_domain::BlockPos::new(x, 0, z), GRASS);
    observe(context, mornlea_domain::BlockPos::new(x, 1, z), AIR);
    observe(context, mornlea_domain::BlockPos::new(x, 2, z), AIR);
}

fn motion_call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::HostileMotion,
        actor: None,
        command: None,
        internal: None,
    }
}

fn burn_call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::HostileBurnDistant,
        actor: None,
        command: None,
        internal: None,
    }
}

fn fluid_runtime(key: ActorKey) -> ActorRuntime {
    ActorRuntime {
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
            shoot_cooldown: 0,
            fresh: false,
        },
    }
}

fn airborne_hostile(position: [f32; 3]) -> ActorRecord {
    let mut actor = hostile_actor(21, position, NIGHTWALKER);
    actor.motion = MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new(position).expect("position"),
        velocity: FiniteVec3::try_new([0.0; 3]).expect("velocity"),
        on_ground: false,
    });
    let ActorBody::Hostile(body) = &mut actor.body else {
        unreachable!();
    };
    body.on_ground = false;
    actor
}

fn assert_fluid_entry(position: [f32; 3], water: Option<[i32; 3]>, immersed: bool) {
    use mornlea_engine::native::contracts::collision::{Aabb, CollisionCell, CollisionGrid};
    use mornlea_engine::native::contracts::physics::{
        PhysicsControls, PhysicsOp, PhysicsRequest, PhysicsState, SweepBounds,
    };
    use mornlea_engine::native::physics::NativePhysics;
    let mut state = authority();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, 1000, 0);
    let actor = airborne_hostile(position);
    let runtime = fluid_runtime(actor.key);
    stage_actors(&mut context, std::slice::from_ref(&actor));
    context.stage(RuleEffect::Runtime(runtime.clone())).unwrap();
    let origin = [
        position[0].floor() as i32 - 4,
        -2,
        position[2].floor() as i32 - 4,
    ];
    let empty = CollisionCell::try_new(
        true,
        [Aabb {
            minimum: [0.0; 3],
            maximum: [0.0; 3],
        }; 8],
        0,
    )
    .unwrap();
    let cells = vec![empty; 9 * 9 * 9];
    for y in -2..=6 {
        for x in origin[0]..origin[0] + 9 {
            for z in origin[2]..origin[2] + 9 {
                observe(
                    &mut context,
                    mornlea_domain::BlockPos::new(x, y, z),
                    if water == Some([x, y, z]) { WATER } else { AIR },
                );
            }
        }
    }
    // Source `SubmersionFlagsWithTunables` pins the flag per scene; the
    // numerical expectation calls the real kernel without copying its math.
    let expected = NativePhysics
        .step(&PhysicsRequest {
            state: PhysicsState {
                position,
                velocity: [0.0; 3],
                on_ground: false,
            },
            controls: PhysicsControls {
                move_x: 0,
                move_z: 0,
                jump: false,
                yaw_sin: 0.0,
                yaw_cos: 1.0,
                body_in_fluid: immersed,
                sprinting: false,
                sneaking: false,
            },
            tuning: RuleTunables::source_defaults().physics(),
            sweep: SweepBounds {
                minimum: [-2.0; 3],
                maximum: [2.0; 3],
            },
            grid: CollisionGrid::try_new(origin, [9, 9, 9], &cells).unwrap(),
        })
        .unwrap()
        .state;
    let blocks = context.changed_blocks();
    let events = context.events().to_vec();
    provider::run(&mut context, motion_call()).expect("fluid motion");
    let actual = find_hostile(&context, 21);
    assert_eq!(
        actual.motion.position().get().map(f32::to_bits),
        expected.position.map(f32::to_bits)
    );
    assert_eq!(
        actual.motion.velocity().get().map(f32::to_bits),
        expected.velocity.map(f32::to_bits)
    );
    assert_eq!(actual.motion.on_ground(), expected.on_ground);
    assert_eq!(actual.survival, actor.survival);
    assert_eq!(actual.look, actor.look);
    assert_eq!(actual.lifecycle, actor.lifecycle);
    assert_eq!(context.read().runtime(actor.key), Some(&runtime));
    assert_eq!(context.changed_blocks(), blocks);
    assert_eq!(context.events(), events);
    let ActorBody::Hostile(body) = &actual.body else {
        unreachable!();
    };
    assert_eq!(body.position, actual.motion.position().get());
    assert_eq!(body.velocity, actual.motion.velocity().get());
    assert_eq!(body.on_ground, actual.motion.on_ground());
}

#[test]
fn torso_water_enters_immersed_native_physics() {
    assert_fluid_entry([0.5, 1.0, 0.5], Some([0, 2, 0]), true);
}

#[test]
fn fractional_horizontal_upper_cells_enter_immersed_native_physics() {
    for (position, water) in [
        ([0.8, 1.0, 0.5], [1, 1, 0]),
        ([0.5, 1.0, 0.8], [0, 1, 1]),
        ([-0.1, 1.0, 0.5], [0, 1, 0]),
        ([0.5, 1.0, -0.1], [0, 1, 0]),
        ([-2.5, 1.0, -2.5], [-3, 2, -3]),
    ] {
        assert_fluid_entry(position, Some(water), true);
    }
}

#[test]
fn touching_upper_cells_and_dry_rooms_exclude_immersion() {
    for (position, water) in [
        ([0.7, 1.0, 0.5], Some([1, 1, 0])),
        ([0.5, 1.0, 0.7], Some([0, 1, 1])),
        ([-0.3, 1.0, 0.5], Some([0, 1, 0])),
        ([0.5, 1.0, -0.3], Some([0, 1, 0])),
        ([0.5, 1.2, 0.5], Some([0, 3, 0])),
        ([-2.5, 1.0, -2.5], None),
    ] {
        assert_fluid_entry(position, water, false);
    }
}

fn assert_fluid_coordinate_refusal(value: f32) {
    for axis in 0..3 {
        let mut position = [0.5, 1.0, 0.5];
        position[axis] = value;
        let mut state = authority();
        let mut context = TickContext::harness(&mut state, TickBudget::full());
        stage_environment(&mut context, 1000, 0);
        let actor = airborne_hostile(position);
        let mut runtime = fluid_runtime(actor.key);
        runtime.attack_cooldown = 13;
        runtime.hurt_cooldown = 17;
        runtime.oxygen = 211;
        runtime.peak_y = 77.0;
        stage_actors(&mut context, std::slice::from_ref(&actor));
        context.stage(RuleEffect::Runtime(runtime.clone())).unwrap();
        let marker = mornlea_domain::BlockPos::new(0, 0, 0);
        observe(&mut context, marker, STONE);
        let actors = context.read().actors().to_vec();
        let blocks = context.changed_blocks();
        let observed = context.read().observation(Dimension::OVERWORLD, marker);
        let environment = context.read().environment().cloned();
        let events = context.events().to_vec();
        let result = provider::run(&mut context, motion_call());
        assert!(
            matches!(result, Err(ServerError::InvalidInput { field: "actor" })),
            "axis {axis}, result {result:?}"
        );
        assert_eq!(context.read().actors(), actors);
        assert_eq!(context.read().runtime(actor.key), Some(&runtime));
        assert_eq!(context.changed_blocks(), blocks);
        assert_eq!(
            context.read().observation(Dimension::OVERWORLD, marker),
            observed
        );
        assert_eq!(context.read().environment(), environment.as_ref());
        assert_eq!(context.events(), events);
    }
}

#[test]
fn minimum_fluid_coordinates_refuse_without_effect() {
    assert_fluid_coordinate_refusal(i32::MIN as f32);
}

#[test]
fn maximum_fluid_coordinates_refuse_without_effect() {
    assert_fluid_coordinate_refusal(i32::MAX as f32);
}

// -----------------------------------------------------------------------
// Candidate math mirrors (`hostileSpawnColumn` and `HostileCandidateHash`
// in the Go rows cited above). The scenes use them only to know which
// column to load and which tick passes the hash gate; the provider must
// derive the same values from authority state.
// -----------------------------------------------------------------------

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn candidate_column(seed: i64, world_time: u64, anchor: [i32; 2]) -> (i32, i32) {
    let base = splitmix64((seed as u64) ^ world_time);
    let radius = 24 + base % 25;
    let axis = ((base >> 32) & 3) as usize;
    let dx = [1i32, -1, 0, 0][axis];
    let dz = [0i32, 0, 1, -1][axis];
    (
        anchor[0] + dx * radius as i32,
        anchor[1] + dz * radius as i32,
    )
}

fn candidate_hash(seed: i64, world_time: u64, x: i32, y: i32, z: i32) -> u64 {
    let hash = splitmix64((seed as u64) ^ world_time);
    let hash = splitmix64(hash ^ (x as u32 as u64) ^ (z as u32 as u64));
    splitmix64(hash ^ (y as u32 as u64))
}

/// Probes the boundary ticks for one whose candidate passes the hash gate at
/// exactly the wanted display phase (`findSpawningTick`'s probe, resolved
/// from the frozen pure math instead of engine calls).
fn boundary_tick(phase: u64) -> (u64, u32, (i32, i32), u64) {
    for k in 0..4000u64 {
        let world_time = phase + 24_000 * k;
        let (x, z) = candidate_column(SEED, world_time, [0, 0]);
        let hash = candidate_hash(SEED, world_time, x, 1, z);
        if hash & 0xFF < SPAWN_GATE {
            return (world_time, equinox_offset(world_time), (x, z), hash);
        }
    }
    panic!("no gate-passing tick at phase {phase}");
}

fn hostile_count(context: &TickContext<'_>) -> usize {
    context
        .read()
        .actors()
        .iter()
        .filter(|actor| matches!(actor.key, ActorKey::Hostile(_)))
        .count()
}

fn find_hostile<'a>(context: &'a TickContext<'_>, id: u64) -> &'a ActorRecord {
    context
        .read()
        .actors()
        .iter()
        .find(|actor| matches!(actor.key, ActorKey::Hostile(key) if key.get() == id))
        .expect("hostile record")
}

/// The frozen named case: the night window is inclusive at its end, the
/// global resident cap refuses the 65th, the equal-distance target tie picks
/// the stable smaller player id, and the distant counter retains at 599
/// within range and removes at 600 beyond it.
#[test]
fn night_tie_capacity_distance() {
    // --- Night end is inclusive (effective phase exactly 23000 spawns). ---
    let (tick, season, (x, z), _hash) = boundary_tick(NIGHT_END);
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, tick, 0);
    stage_actors(&mut context, &[player_actor(anchor, 1, [0.5, 1.0, 0.5])]);
    preload_flat_column(&mut context, x, z);
    let report = provider::run(&mut context, motion_call()).expect("boundary spawn");
    assert_eq!(hostile_count(&context), 1, "phase 23000 admits the spawn");
    assert_eq!(report.applied, 1, "exactly one candidate per tick");
    let spawned = context
        .read()
        .actors()
        .iter()
        .find(|actor| matches!(actor.key, ActorKey::Hostile(_)))
        .expect("spawned hostile")
        .clone();
    let ActorBody::Hostile(body) = &spawned.body else {
        panic!("hostile body");
    };
    assert_eq!(body.position, [x as f32 + 0.5, 1.0, z as f32 + 0.5]);
    assert_eq!(body.health, 20, "spawned at full health");
    assert_eq!(
        body.kind,
        if body.id % 3 == 0 {
            HURLER
        } else {
            NIGHTWALKER
        }
    );
    // Transients: the counter starts at 0 and the fresh flag was already
    // consumed by this tick's movement pass, because spawn order runs
    // before physics inside the same phase call (`advanceHostiles` order in
    // `hostile.go` with the `advanceHostileMovement` fresh rule).
    let runtime = context.read().runtime(spawned.key).expect("runtime");
    let ActorAux::Hostile {
        distant_ticks,
        shoot_cooldown,
        fresh,
    } = runtime.aux
    else {
        panic!("hostile aux");
    };
    assert!(!fresh, "the spawn-tick movement pass consumed the flag");
    assert_eq!(distant_ticks, 0);
    assert_eq!(shoot_cooldown, 0);
    let _ = season;

    // --- One tick past the window end refuses even a gate-passing
    // candidate (23001 is daytime). ---
    let (late_tick, _, (lx, lz), _) = boundary_tick(NIGHT_END + 1);
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, late_tick, 0);
    stage_actors(&mut context, &[player_actor(anchor, 1, [0.5, 1.0, 0.5])]);
    preload_flat_column(&mut context, lx, lz);
    provider::run(&mut context, motion_call()).expect("refused tick");
    assert_eq!(hostile_count(&context), 0, "phase 23001 refuses the spawn");

    // --- The global resident cap: 64 residents refuse the 65th
    // (`TestHostileSpawnRejectsAtGlobalCap`). ---
    let mut residents = Vec::new();
    for id in 1..=64u64 {
        residents.push(hostile_actor(
            id,
            [10_000.0 + f32::from(id as u8), 40.0, 10_000.0],
            NIGHTWALKER,
        ));
    }
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, tick, 0);
    stage_actors(&mut context, &[player_actor(anchor, 1, [0.5, 1.0, 0.5])]);
    stage_actors(&mut context, &residents);
    preload_flat_column(&mut context, x, z);
    provider::run(&mut context, motion_call()).expect("capped tick");
    assert_eq!(hostile_count(&context), 64, "the 65th resident is refused");

    // --- Equal-distance targeting picks the stable smaller player id
    // (`nearestTarget` tie by `PlayerID` bytes). Session 2 carries the
    // smaller uuid, so a session-key tiebreak would pick the other player.
    // Daytime keeps spawn admission out of the scene. ---
    let mut state = authority();
    let (first, second) = two_sessions(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, 1000, 0);
    // Flat 33x9x33 path window around the hostile: stone below y=40, air
    // at and above it, with actual Ready ownership.
    for x in 84..=116 {
        for z in 84..=116 {
            for y in 36..=44 {
                observe(
                    &mut context,
                    mornlea_domain::BlockPos::new(x, y, z),
                    if y < 40 { STONE } else { AIR },
                );
            }
        }
    }
    preload_ready_window(&mut context, 100, 100);
    stage_actors(
        &mut context,
        &[
            player_actor(first, 9, [90.5, 40.0, 100.5]),
            player_actor(second, 3, [110.5, 40.0, 100.5]),
            hostile_actor(21, [100.5, 40.0, 100.5], NIGHTWALKER),
        ],
    );
    provider::run(&mut context, motion_call()).expect("tie tick");
    let runtime = context
        .read()
        .runtime(ActorKey::Hostile(
            mornlea_domain::HostileId::try_new(21).unwrap(),
        ))
        .expect("runtime");
    let path = runtime.path.as_ref().expect("chase path");
    assert_eq!(
        path.target,
        mornlea_domain::BlockPos::new(110, 40, 100),
        "the uuid-3 player wins the equal-distance tie"
    );
    assert_eq!(path.generation, 1, "first target selection bumps once");
    // The runner consumes an arrived waypoint before stepping
    // (`advanceRunners` current-cell ruling), so the cursor may already sit
    // past the start cell; it must stay inside the path.
    assert!(path.cursor < path.waypoints.len(), "a live path remains");
    // Go `applyPathOutcome` schedules on WorldTime, independently of the
    // executing tick: this scene starts at calendar time 1000.
    assert_eq!(
        path.next_repath_tick, 1020,
        "repath cadence is 20 calendar ticks"
    );
    assert!(!path.waypoints.is_empty(), "a path was found");
    // The hostile stepped toward the chosen target (+x) on this same tick.
    let moved = find_hostile(&context, 21).motion.position().get();
    assert!(moved[0] > 100.5, "moved toward the +x target");
    assert!((moved[2] - 100.5).abs() < 1e-4, "no z drift");

    // --- Distant counter: 599 within range resets to 0 and retains;
    // the 600th tick beyond 64 removes with no drop
    // (`TestHostileDistantCounterResetsWithinRange`,
    // `TestHostileDistantDespawnAfterSixHundredActiveTicks`). ---
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, 1000, 0);
    stage_actors(
        &mut context,
        &[
            player_actor(anchor, 1, [0.5, 1.0, 0.5]),
            hostile_actor_with(
                25,
                [64.5, 1.0, 0.5],
                NIGHTWALKER,
                20,
                599,
                false,
                StoredPlayerId::from_bytes([0u8; 16]),
            ),
        ],
    );
    provider::run(&mut context, burn_call()).expect("within-range tick");
    let kept = find_hostile(&context, 25);
    assert_eq!(kept.lifecycle, ActorLifecycle::Active, "599 retains");
    let ActorBody::Hostile(kept_body) = &kept.body else {
        panic!("hostile body");
    };
    assert_eq!(kept_body.distant_ticks, 0, "within 64 resets the counter");
    let runtime = context
        .read()
        .runtime(ActorKey::Hostile(
            mornlea_domain::HostileId::try_new(25).unwrap(),
        ))
        .expect("runtime");
    let ActorAux::Hostile { distant_ticks, .. } = runtime.aux else {
        panic!("hostile aux");
    };
    assert_eq!(distant_ticks, 0);

    // Exactly 65 blocks is beyond the 64 radius: the counter starts again
    // from 0 and reads 1 after the tick.
    let mut shifted = kept.clone();
    if let ActorBody::Hostile(body) = &mut shifted.body {
        body.position = [65.5, 1.0, 0.5];
    }
    shifted.motion = MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new([65.5, 1.0, 0.5]).expect("position"),
        velocity: FiniteVec3::try_new([0.0; 3]).expect("velocity"),
        on_ground: true,
    });
    context.stage(RuleEffect::Actor(shifted)).expect("shift");
    provider::run(&mut context, burn_call()).expect("beyond-range tick");
    let ActorBody::Hostile(moved_body) = &find_hostile(&context, 25).body else {
        panic!("hostile body");
    };
    assert_eq!(moved_body.distant_ticks, 1, "beyond 64 accumulates");

    // A counter already at 599 beyond range crosses 600 and is removed
    // with no drop.
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, 1000, 0);
    stage_actors(
        &mut context,
        &[
            player_actor(anchor, 1, [0.5, 1.0, 0.5]),
            hostile_actor_with(
                24,
                [80.5, 1.0, 0.5],
                NIGHTWALKER,
                20,
                599,
                false,
                StoredPlayerId::from_bytes([0u8; 16]),
            ),
        ],
    );
    provider::run(&mut context, burn_call()).expect("despawn tick");
    assert_eq!(
        find_hostile(&context, 24).lifecycle,
        ActorLifecycle::Dead,
        "600 distant ticks remove the hostile"
    );
    assert!(
        context.events().is_empty(),
        "a distant removal drops nothing"
    );
}

/// The spawn anchor follows session order, not player-uuid order
/// (`sortedActiveSessions` in `packages/server/sim/entity/drop.go:212-222`
/// feeding the `worldtime % len` pick): session 1 carries uuid 9 and
/// session 2 uuid 3, so the two orders disagree. Both plausible candidate
/// columns are loaded per tick, and the landed position names which anchor
/// the provider used. The uuid tie in target selection is unaffected.
#[test]
fn anchor_follows_session_order() {
    let players_with = |state: &mut AuthorityState| {
        let first = admit_session(state, 1, "anchor-one");
        let second = admit_session(state, 2, "anchor-two");
        (
            player_actor(first, 9, [0.5, 1.0, 0.5]),
            player_actor(second, 3, [200.5, 1.0, 0.5]),
        )
    };
    // Probes one parity: `anchor` is the block anchor of the session-order
    // player at this parity, `other` the uuid-order player's anchor. The
    // scene passes only when the spawn lands on the session-order column.
    let probe = |even: bool, anchor: [i32; 2], other: [i32; 2]| -> [f32; 3] {
        for k in 0..4000u64 {
            let world_time = NIGHT_START + u64::from(!even) + 24_000 * k;
            if world_time.is_multiple_of(2) != even {
                continue;
            }
            let column = candidate_column(SEED, world_time, anchor);
            let hash = candidate_hash(SEED, world_time, column.0, 1, column.1);
            if hash & 0xFF >= SPAWN_GATE {
                continue;
            }
            let decoy = candidate_column(SEED, world_time, other);
            let mut state = authority();
            let (first, second) = players_with(&mut state);
            let mut context = TickContext::harness(&mut state, TickBudget::full());
            stage_environment(&mut context, world_time, 0);
            stage_actors(&mut context, &[first, second]);
            preload_flat_column(&mut context, column.0, column.1);
            preload_flat_column(&mut context, decoy.0, decoy.1);
            provider::run(&mut context, motion_call()).expect("probe tick");
            let landed = context
                .read()
                .actors()
                .iter()
                .find_map(|actor| match &actor.body {
                    ActorBody::Hostile(body) => Some(body.position),
                    _ => None,
                })
                .expect("spawned hostile");
            assert_eq!(
                (landed[0].floor() as i32, landed[2].floor() as i32),
                column,
                "anchor at worldtime {world_time} followed the wrong order"
            );
            return landed;
        }
        panic!("no gate-passing anchor probe at parity even={even}");
    };

    // Even ticks index the session-order head (session 1 at the origin).
    probe(true, [0, 0], [200, 0]);
    // Odd ticks index the session-order tail (session 2 at x=200).
    probe(false, [200, 0], [0, 0]);
}

/// Peaceful difficulty gates at the entry (the very tick that spawns under
/// normal difficulty), and every admitted id satisfies the hash gate and the
/// `hash % 3` kind dispatch
/// (`TestHostileSpawnPeacefulGatesAtEntry`,
/// `TestHostileSpawnGateMatchesHashLowByte`,
/// `TestHostileSpawnKindFollowsCandidateHashRule`).
#[test]
fn spawn_gates_peaceful_hash_and_kind() {
    let (tick, _season, (x, z), _hash) = boundary_tick(NIGHT_START + 24_000);

    // Peaceful refuses the very candidate that normal admits.
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, tick, 1);
    stage_actors(&mut context, &[player_actor(anchor, 1, [0.5, 1.0, 0.5])]);
    preload_flat_column(&mut context, x, z);
    provider::run(&mut context, motion_call()).expect("peaceful tick");
    assert_eq!(hostile_count(&context), 0, "peaceful never spawns");

    // Normal admits it; the observable id form carries the gate: low byte
    // < 13, and the kind follows `id % 3`.
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, tick, 0);
    stage_actors(&mut context, &[player_actor(anchor, 1, [0.5, 1.0, 0.5])]);
    preload_flat_column(&mut context, x, z);
    provider::run(&mut context, motion_call()).expect("normal tick");
    assert_eq!(hostile_count(&context), 1);
    let spawned = context
        .read()
        .actors()
        .iter()
        .find(|actor| matches!(actor.key, ActorKey::Hostile(_)))
        .expect("spawned")
        .clone();
    let ActorBody::Hostile(body) = &spawned.body else {
        panic!("hostile body");
    };
    assert!(body.id != 0, "spawned id is nonzero");
    assert!(body.id & 0xFF < SPAWN_GATE, "id low byte passes the gate");
    assert_eq!(
        body.kind,
        if body.id % 3 == 0 {
            HURLER
        } else {
            NIGHTWALKER
        }
    );

    // Wrong shapes are refused without effect.
    let mut state = authority();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, tick, 0);
    assert!(
        provider::run(
            &mut context,
            RuleCall {
                phase: RulePhase::Combat,
                actor: None,
                command: None,
                internal: None,
            },
        )
        .is_err()
    );
    assert!(
        provider::run(
            &mut context,
            RuleCall {
                phase: RulePhase::HostileMotion,
                actor: Some(ActorKey::Hostile(
                    mornlea_domain::HostileId::try_new(1).unwrap()
                )),
                command: None,
                internal: None,
            },
        )
        .is_err()
    );
}

/// The dark-spawn light boundary: a torch 6 cells away lifts the candidate
/// block light to 8 and refuses, at 7 cells it reads 7 and admits
/// (`TestHostileSpawnRejectsBrightCandidate`).
#[test]
fn bright_light_boundary() {
    let (tick, _season, (x, z), _hash) = boundary_tick(NIGHT_START + 24_000);

    // Torch at distance 6 along +x: observed air corridor between, so the
    // light reaches the candidate at 14 - 6 = 8 > 7.
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, tick, 0);
    stage_actors(&mut context, &[player_actor(anchor, 1, [0.5, 1.0, 0.5])]);
    preload_flat_column(&mut context, x, z);
    for dx in 1..=5 {
        observe(
            &mut context,
            mornlea_domain::BlockPos::new(x + dx, 1, z),
            AIR,
        );
    }
    observe(
        &mut context,
        mornlea_domain::BlockPos::new(x + 6, 1, z),
        TORCH_STANDING,
    );
    provider::run(&mut context, motion_call()).expect("bright tick");
    assert_eq!(hostile_count(&context), 0, "light 8 refuses the candidate");

    // Same torch one cell farther: candidate light 7 <= 7 admits.
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, tick, 0);
    stage_actors(&mut context, &[player_actor(anchor, 1, [0.5, 1.0, 0.5])]);
    preload_flat_column(&mut context, x, z);
    for dx in 1..=6 {
        observe(
            &mut context,
            mornlea_domain::BlockPos::new(x + dx, 1, z),
            AIR,
        );
    }
    observe(
        &mut context,
        mornlea_domain::BlockPos::new(x + 7, 1, z),
        TORCH_STANDING,
    );
    provider::run(&mut context, motion_call()).expect("dim tick");
    assert_eq!(hostile_count(&context), 1, "light 7 admits the candidate");
}

/// Movement transients and the burn pass: a fresh spawn skips its first
/// movement tick and the next neutral tick holds it in place; daylight burns
/// one health every 20th exposed tick while night resets the timer
/// (`TestHostileMovementReusesPlayerPhysicsStep` neutral row,
/// `TestHostileBurnDamagesEveryTwentyTicksWhenExposed`,
/// `TestHostileBurnTimerResetsAtNight`).
#[test]
fn fresh_skip_neutral_move_and_burn() {
    // Spawn under night, then observe the fresh skip and the neutral step.
    let (tick, _season, (x, z), _hash) = boundary_tick(NIGHT_START + 24_000);
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, tick, 0);
    stage_actors(&mut context, &[player_actor(anchor, 1, [0.5, 1.0, 0.5])]);
    preload_flat_column(&mut context, x, z);
    provider::run(&mut context, motion_call()).expect("spawn tick");
    let spawned_id = context
        .read()
        .actors()
        .iter()
        .find_map(|actor| match actor.key {
            ActorKey::Hostile(key) => Some(key.get()),
            _ => None,
        })
        .expect("spawned id");
    let spawn_position = find_hostile(&context, spawned_id).motion.position().get();
    // The fresh flag was consumed by the skip, and no physics ran on the
    // spawn tick.
    let runtime = context
        .read()
        .runtime(ActorKey::Hostile(
            mornlea_domain::HostileId::try_new(spawned_id).unwrap(),
        ))
        .expect("runtime");
    let ActorAux::Hostile { fresh, .. } = runtime.aux else {
        panic!("hostile aux");
    };
    assert!(!fresh, "the fresh flag clears on the skip tick");

    // Second tick: no path is possible (unobserved window), so the hostile
    // advances neutrally and stays at its spawn cell.
    provider::run(&mut context, motion_call()).expect("neutral tick");
    let settled = find_hostile(&context, spawned_id).motion.position().get();
    assert_eq!(settled, spawn_position, "neutral input moves nothing");

    // --- Burn: daylight exposure damages every 20th tick. ---
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, 1000, 0);
    stage_actors(
        &mut context,
        &[
            player_actor(anchor, 1, [0.5, 1.0, 0.5]),
            hostile_actor(21, [2.5, 1.0, 2.5], NIGHTWALKER),
        ],
    );
    // Exposed sky: every cell from above the body to the world top is
    // observed air (`hostileSkyExposed` reads straight up to MaxY).
    for y in 2..MAX_Y {
        observe(&mut context, mornlea_domain::BlockPos::new(2, y, 2), AIR);
    }
    observe(&mut context, mornlea_domain::BlockPos::new(2, 0, 2), GRASS);
    observe(&mut context, mornlea_domain::BlockPos::new(2, 1, 2), AIR);
    for _ in 0..19 {
        provider::run(&mut context, burn_call()).expect("burn tick");
    }
    let ActorBody::Hostile(body) = &find_hostile(&context, 21).body else {
        panic!("hostile body");
    };
    assert_eq!(body.health, 20, "19 exposed ticks do not burn yet");
    let runtime = context
        .read()
        .runtime(ActorKey::Hostile(
            mornlea_domain::HostileId::try_new(21).unwrap(),
        ))
        .expect("runtime");
    assert_eq!(runtime.burn_cooldown, 1, "the timer walked 19 of 20 ticks");
    provider::run(&mut context, burn_call()).expect("20th tick");
    let ActorBody::Hostile(body) = &find_hostile(&context, 21).body else {
        panic!("hostile body");
    };
    assert_eq!(body.health, 19, "the 20th exposed tick burns one health");
    let runtime = context
        .read()
        .runtime(ActorKey::Hostile(
            mornlea_domain::HostileId::try_new(21).unwrap(),
        ))
        .expect("runtime");
    assert_eq!(runtime.burn_cooldown, 20, "the timer reset to the period");

    // Night resets the timer without damage
    // (`TestHostileBurnTimerResetsAtNight`).
    let night = 15000;
    context
        .stage(RuleEffect::Environment(environment(night, 0)))
        .expect("night environment");
    let mut partial = find_hostile(&context, 21).clone();
    if let ActorBody::Hostile(body) = &mut partial.body {
        body.burn_cooldown = 10;
    }
    context.stage(RuleEffect::Actor(partial)).expect("timer");
    context
        .stage(RuleEffect::Runtime(ActorRuntime {
            key: ActorKey::Hostile(mornlea_domain::HostileId::try_new(21).unwrap()),
            controls: None,
            has_view: false,
            reset: false,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 10,
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
            aux: ActorAux::Hostile {
                distant_ticks: 0,
                shoot_cooldown: 0,
                fresh: false,
            },
        }))
        .expect("timer runtime");
    provider::run(&mut context, burn_call()).expect("night tick");
    let runtime = context
        .read()
        .runtime(ActorKey::Hostile(
            mornlea_domain::HostileId::try_new(21).unwrap(),
        ))
        .expect("runtime");
    assert_eq!(runtime.burn_cooldown, 20, "night resets the timer");
    let ActorBody::Hostile(body) = &find_hostile(&context, 21).body else {
        panic!("hostile body");
    };
    assert_eq!(body.health, 19, "night deals no burn damage");
}

// -----------------------------------------------------------------------
// Hurler distance bands (`advanceHurlerBand` in
// `packages/server/server/hostile_manager.go`): beyond 14 blocks the hurler
// approaches through the path machinery, 6..=14 holds position with no path,
// below 6 retreats along the straight target-minus-hostile line without
// pathfinding. Walkers keep the 1.8 attack stop. These cases assert
// positions only, never intents.
// -----------------------------------------------------------------------

fn ready_air_chunk() -> mornlea_storage::Chunk {
    mornlea_storage::Chunk {
        sections: vec![
            mornlea_storage::ContainerSnapshot {
                kind: mornlea_storage::StorageKind::Single,
                bits: 0,
                single: AIR,
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

fn set_ready_cell(chunk: &mut mornlea_storage::Chunk, pos: mornlea_domain::BlockPos, block: u16) {
    let section = &mut chunk.sections[((pos.y() + 64) / 16) as usize];
    if section.kind == mornlea_storage::StorageKind::Single {
        let single = u64::from(section.single);
        let packed = single | single << 15 | single << 30 | single << 45;
        *section = mornlea_storage::ContainerSnapshot {
            kind: mornlea_storage::StorageKind::Direct,
            bits: 15,
            single: 0,
            palette: vec![],
            packed: vec![packed; 1024],
        };
    }
    assert_eq!(section.kind, mornlea_storage::StorageKind::Direct);
    let index = (((pos.y() + 64) % 16) * 256 + (pos.z() & 15) * 16 + (pos.x() & 15)) as usize;
    let shift = (index % 4) * 15;
    section.packed[index / 4] =
        (section.packed[index / 4] & !(0x7fff << shift)) | u64::from(block) << shift;
}

fn ready_window_chunk(context: &TickContext<'_>, key: ChunkKey) -> mornlea_storage::Chunk {
    let mut chunk = ready_air_chunk();
    for x in key.pos.x() * 16..=key.pos.x() * 16 + 15 {
        for z in key.pos.z() * 16..=key.pos.z() * 16 + 15 {
            for y in 36..=44 {
                let pos = mornlea_domain::BlockPos::new(x, y, z);
                if let Some(observed) = context.read().observation(key.dimension, pos) {
                    set_ready_cell(&mut chunk, pos, observed.block);
                }
            }
        }
    }
    chunk
}

/// Prepare validated compact bases from exact sparse terrain off-tick.
fn preload_ready_window(context: &mut TickContext<'_>, x: i32, z: i32) {
    for dx in -1..=1 {
        for dz in -1..=1 {
            let key = ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new((x >> 4) + dx, (z >> 4) + dz),
            };
            context.preload_ready_chunk(
                mornlea_server::core::world::ReadyChunk::try_new(
                    key,
                    1,
                    (50 + (dx + 1) * 3 + dz + 1) as u64,
                    ready_window_chunk(context, key),
                )
                .unwrap(),
            );
        }
    }
}

/// Flat 33x9x33 walk volume around the scene: stone below y=40, air above.
fn preload_band_world(context: &mut TickContext<'_>) {
    preload_sparse_band_world(context);
    preload_ready_window(context, 100, 100);
}

fn preload_sparse_band_world(context: &mut TickContext<'_>) {
    for x in 84..=116 {
        for z in 84..=116 {
            for y in 36..=44 {
                observe(
                    context,
                    mornlea_domain::BlockPos::new(x, y, z),
                    if y < 40 { STONE } else { AIR },
                );
            }
        }
    }
}

#[test]
fn motion_selects_live_path_target_past_nearest_dead_player() {
    let mut state = authority();
    let dead_session = admit_session(&mut state, 1, "motion-dead");
    let live_session = admit_session(&mut state, 9, "motion-live");
    let foreign_session = admit_session(&mut state, 2, "motion-foreign");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, 1000, 0);
    let mut dead = player_actor(dead_session, 1, [101.0, 40.0, 100.5]);
    dead.survival = SurvivalState::try_new(SurvivalStateParts {
        health: 0,
        oxygen: 300,
        hunger: 20,
        saturation_zero: false,
        armor_points: 0,
    })
    .unwrap();
    let ActorBody::Player(body) = &mut dead.body else {
        unreachable!();
    };
    body.health = 0;
    let mut foreign = player_actor(foreign_session, 2, [100.5, 40.0, 100.5]);
    foreign.dimension = Dimension::DEPTHS;
    let ActorBody::Player(body) = &mut foreign.body else {
        unreachable!();
    };
    body.current.dimension = i32::from(Dimension::DEPTHS.get());
    stage_actors(
        &mut context,
        &[
            dead.clone(),
            player_actor(live_session, 9, [105.5, 40.0, 100.5]),
            foreign,
            hostile_actor(21, [100.5, 40.0, 100.5], NIGHTWALKER),
        ],
    );
    let mut dead_runtime = fluid_runtime(dead.key);
    dead_runtime.aux = ActorAux::Player {
        respawn: None,
        workbench: None,
    };
    dead_runtime.attack_cooldown = 13;
    dead_runtime.oxygen = 211;
    context
        .stage(RuleEffect::Runtime(dead_runtime.clone()))
        .unwrap();
    preload_band_world(&mut context);

    provider::run(&mut context, motion_call()).expect("live-target motion");

    let actor = find_hostile(&context, 21);
    let ActorBody::Hostile(body) = &actor.body else {
        unreachable!();
    };
    assert!(body.has_target);
    assert_eq!(body.player_id.to_bytes(), uuid_bytes(9));
    let path = context
        .read()
        .runtime(actor.key)
        .unwrap()
        .path
        .as_ref()
        .expect("live chase path");
    assert_eq!(path.target, mornlea_domain::BlockPos::new(105, 40, 100));
    assert!(actor.motion.position().get()[0] > 100.5);
    assert_eq!(context.read().actor(dead.key), Some(&dead));
    assert_eq!(context.read().runtime(dead.key), Some(&dead_runtime));
}

fn hurler_position(context: &TickContext<'_>, id: u64) -> [f32; 3] {
    let ActorBody::Hostile(body) = &find_hostile(context, id).body else {
        panic!("hostile body");
    };
    body.position
}

fn hurler_runtime_path(context: &TickContext<'_>, id: u64) -> bool {
    context
        .read()
        .runtime(ActorKey::Hostile(
            mornlea_domain::HostileId::try_new(id).expect("hostile id"),
        ))
        .is_some_and(|runtime| runtime.path.is_some())
}

/// One hostile motion tick over a borrowed foot cell, through the real
/// provider only (no NativePhysics double). The source oracle pins dt 0.05,
/// walk speed 4.3 and ground acceleration 40, so a full-speed -z approach
/// settles at walk 3.01 over thick Snow (cells 87 and 88, which carry zero
/// collision) and keeps 4.3 over AIR: displacement 0.1505 thick / 0.215 AIR
/// from [100.5, 40, 100.5].
fn assert_motion_snow_native_displacement(kind: u8, foot: u16, want_z: f32, want_vz: f32) {
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, 1000, 0);
    preload_band_world(&mut context);
    // Repair the hostile fixture: stage the foot block through a live Ready
    // transaction before the first chase/path search consumes it.
    let foot_cell = mornlea_domain::BlockPos::new(100, 40, 100);
    if foot != mornlea_domain::Block::AIR {
        let observed = context
            .read()
            .observation(Dimension::OVERWORLD, foot_cell)
            .expect("ready foot observation");
        let write = BlockWrite::try_new(observed, foot).expect("foot block write");
        let outcome = context
            .transaction()
            .try_system(SystemRule::Support, vec![write])
            .expect("support system write");
        assert_eq!(outcome.changed.len(), 1);
    }
    stage_actors(
        &mut context,
        &[
            player_actor(anchor, 1, [100.5, 40.0, 84.5]),
            hostile_actor(21, [100.5, 40.0, 100.5], kind),
        ],
    );
    provider::run(&mut context, motion_call()).expect("live chase tick");
    // The chase must point exactly -z before any pinned snow number applies.
    let chased = find_hostile(&context, 21).motion.position().get();
    assert!(
        (chased[0] - 100.5).abs() < 1e-6 && chased[2] < 100.5,
        "fixture chase must point exactly -z, got {chased:?}"
    );
    // Restage the chased hostile at the start pose with full -z speed; the
    // established target, repath deadline and runtime path stay live.
    let mut prepared = find_hostile(&context, 21).clone();
    prepared.motion = MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new([100.5, 40.0, 100.5]).expect("position"),
        velocity: FiniteVec3::try_new([0.0, 0.0, -4.3]).expect("velocity"),
        on_ground: true,
    });
    prepared.look = LookAngles::try_new(0.0, 0.0).expect("look");
    {
        let ActorBody::Hostile(body) = &mut prepared.body else {
            unreachable!();
        };
        body.position = [100.5, 40.0, 100.5];
        body.velocity = [0.0, 0.0, -4.3];
        body.on_ground = true;
        body.yaw = 0.0;
    }
    stage_actors(&mut context, std::slice::from_ref(&prepared));
    let key = prepared.key;
    let runtime = context.read().runtime(key).cloned();
    let environment = context.read().environment().cloned();
    let player = context
        .read()
        .actors()
        .iter()
        .find(|actor| matches!(actor.key, ActorKey::Player(_)))
        .cloned()
        .expect("player record");
    let support_cell = mornlea_domain::BlockPos::new(100, 39, 100);
    let foot_observed = context.read().observation(Dimension::OVERWORLD, foot_cell);
    let support_observed = context.read().observation(Dimension::OVERWORLD, support_cell);
    let events = context.events().to_vec();

    provider::run(&mut context, motion_call()).expect("snow motion tick");

    let actor = find_hostile(&context, 21);
    let position = actor.motion.position().get();
    let velocity = actor.motion.velocity().get();
    assert_eq!(position[0], 100.5, "kind {kind}, foot {foot}");
    assert_eq!(position[1], 40.0, "kind {kind}, foot {foot}");
    assert!(
        (position[2] - want_z).abs() < 1e-5,
        "kind {kind}, foot {foot}: z {}",
        position[2]
    );
    assert!(
        (velocity[2] - want_vz).abs() < 1e-6,
        "kind {kind}, foot {foot}: vz {}",
        velocity[2]
    );
    assert!(actor.motion.on_ground());
    assert_eq!(actor.look.yaw(), 0.0);
    assert_eq!(actor.key, key);
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    assert_eq!(actor.survival, prepared.survival);
    let ActorBody::Hostile(body) = &actor.body else {
        unreachable!();
    };
    let ActorBody::Hostile(want) = &prepared.body else {
        unreachable!();
    };
    assert_eq!(body.id, want.id);
    assert_eq!(body.health, want.health);
    assert_eq!(body.position, position);
    assert_eq!(body.velocity, velocity);
    assert_eq!(body.on_ground, actor.motion.on_ground());
    assert_eq!(body.yaw, 0.0);
    assert_eq!(body.has_target, want.has_target);
    assert_eq!(body.player_id, want.player_id);
    assert_eq!(body.next_repath_ticks, want.next_repath_ticks);
    assert_eq!(body.attack_cooldown, want.attack_cooldown);
    assert_eq!(body.hurt_cooldown, want.hurt_cooldown);
    assert_eq!(body.burn_cooldown, want.burn_cooldown);
    assert_eq!(body.distant_ticks, want.distant_ticks);
    assert_eq!(context.read().runtime(key), runtime.as_ref());
    assert_eq!(
        context
            .read()
            .actors()
            .iter()
            .find(|actor| matches!(actor.key, ActorKey::Player(_)))
            .cloned(),
        Some(player),
        "the unrelated player record is unchanged"
    );
    assert_eq!(context.read().environment(), environment.as_ref());
    assert_eq!(
        context.read().observation(Dimension::OVERWORLD, foot_cell),
        foot_observed,
        "the borrowed foot cell is unchanged"
    );
    assert_eq!(
        context.read().observation(Dimension::OVERWORLD, support_cell),
        support_observed,
        "the support cell is unchanged"
    );
    assert_eq!(context.events(), events);
}

/// Thick Snow (87 and 88) under the foot cell consumes the shared snow
/// tuning for both hostile kinds inside the actual native motion pass; the
/// AIR control scene keeps the uncut walk speed.
#[test]
fn motion_snow_thick_native_displacement() {
    for kind in [NIGHTWALKER, HURLER] {
        for (foot, want_z, want_vz) in [
            (87, 100.349_5, -3.01),
            (88, 100.349_5, -3.01),
            (AIR, 100.285, -4.3),
        ] {
            assert_motion_snow_native_displacement(kind, foot, want_z, want_vz);
        }
    }
}

/// Sixteen blocks out, the hurler dispatches a path and steps toward the
/// target on the same tick.
#[test]
fn hurler_far_approaches_along_path() {
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, 1000, 0);
    preload_band_world(&mut context);
    stage_actors(
        &mut context,
        &[
            player_actor(anchor, 1, [116.5, 40.0, 100.5]),
            hostile_actor(11, [100.5, 40.0, 100.5], HURLER),
        ],
    );
    provider::run(&mut context, motion_call()).expect("approach tick");
    assert!(
        hurler_runtime_path(&context, 11),
        "the approach band dispatches a path"
    );
    let moved = hurler_position(&context, 11);
    assert!(moved[0] > 100.5, "the hurler stepped toward the +x target");
    assert!((moved[2] - 100.5).abs() < 1e-4, "no z drift");
}

/// Ten blocks out, the hurler holds: no path and no displacement.
#[test]
fn hurler_mid_band_holds() {
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, 1000, 0);
    preload_band_world(&mut context);
    stage_actors(
        &mut context,
        &[
            player_actor(anchor, 1, [110.5, 40.0, 100.5]),
            hostile_actor(11, [100.5, 40.0, 100.5], HURLER),
        ],
    );
    provider::run(&mut context, motion_call()).expect("hold tick");
    assert!(
        !hurler_runtime_path(&context, 11),
        "the hold band dispatches no path"
    );
    assert_eq!(
        hurler_position(&context, 11),
        [100.5, 40.0, 100.5],
        "the hold band keeps position"
    );
}

/// Band edges hold: exactly 14 dispatches no path, exactly 6 does not move.
#[test]
fn hurler_band_edges_hold() {
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, 1000, 0);
    preload_band_world(&mut context);
    stage_actors(
        &mut context,
        &[
            player_actor(anchor, 1, [114.5, 40.0, 100.5]),
            hostile_actor(11, [100.5, 40.0, 100.5], HURLER),
        ],
    );
    provider::run(&mut context, motion_call()).expect("edge tick");
    assert!(
        !hurler_runtime_path(&context, 11),
        "exactly 14 blocks holds, never approaches"
    );

    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, 1000, 0);
    preload_band_world(&mut context);
    stage_actors(
        &mut context,
        &[
            player_actor(anchor, 1, [106.5, 40.0, 100.5]),
            hostile_actor(11, [100.5, 40.0, 100.5], HURLER),
        ],
    );
    provider::run(&mut context, motion_call()).expect("edge tick");
    assert_eq!(
        hurler_position(&context, 11),
        [100.5, 40.0, 100.5],
        "exactly 6 blocks holds, never retreats"
    );
}

/// Three blocks out, the hurler backs away along the straight hostile-minus
/// target line with no pathfinding: +x away, z unchanged.
#[test]
fn hurler_close_retreats_straight_line() {
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, 1000, 0);
    preload_band_world(&mut context);
    stage_actors(
        &mut context,
        &[
            player_actor(anchor, 1, [100.5, 40.0, 100.5]),
            hostile_actor(11, [103.5, 40.0, 100.5], HURLER),
        ],
    );
    provider::run(&mut context, motion_call()).expect("retreat tick");
    assert!(
        !hurler_runtime_path(&context, 11),
        "the retreat band uses no pathfinding"
    );
    let moved = hurler_position(&context, 11);
    assert!(moved[0] > 103.5, "the hurler backed away from the target");
    assert!((moved[2] - 100.5).abs() < 1e-4, "retreat holds the line");
}

/// Walkers keep the 1.8 attack stop: in range they hold with no path.
#[test]
fn walker_attack_stop_unchanged() {
    let mut state = authority();
    let anchor = anchor_session(&mut state);
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, 1000, 0);
    preload_band_world(&mut context);
    stage_actors(
        &mut context,
        &[
            player_actor(anchor, 1, [100.5, 40.0, 100.5]),
            hostile_actor(11, [102.0, 40.0, 100.5], NIGHTWALKER),
        ],
    );
    provider::run(&mut context, motion_call()).expect("walker tick");
    assert_eq!(
        hurler_position(&context, 11),
        [102.0, 40.0, 100.5],
        "walkers still stop inside 1.8 blocks"
    );
}

fn burn_health_actor(health: u8) -> ActorRecord {
    let mut record = hostile_actor_with(
        21,
        [2.5, 1.0, 2.5],
        NIGHTWALKER,
        1,
        17,
        false,
        StoredPlayerId::from_bytes([0; 16]),
    );
    record.survival = SurvivalState::try_new(SurvivalStateParts {
        health,
        oxygen: 17,
        hunger: 3,
        saturation_zero: true,
        armor_points: 4,
    })
    .expect("survival");
    let ActorBody::Hostile(body) = &mut record.body else {
        unreachable!();
    };
    body.health = health;
    record
}

fn observe_burn_sky(context: &mut TickContext<'_>) {
    for y in 2..MAX_Y {
        observe(context, mornlea_domain::BlockPos::new(2, y, 2), AIR);
    }
}

#[test]
fn daylight_burn_updates_authoritative_health_without_resetting_survival() {
    let mut state = authority();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut context, 1000, 0);
    stage_actors(&mut context, &[burn_health_actor(20)]);
    observe_burn_sky(&mut context);

    provider::run(&mut context, burn_call()).expect("exposed burn");

    let actor = find_hostile(&context, 21);
    assert_eq!(
        actor.survival,
        SurvivalState::try_new(SurvivalStateParts {
            health: 19,
            oxygen: 17,
            hunger: 3,
            saturation_zero: true,
            armor_points: 4,
        })
        .expect("expected survival")
    );
    let ActorBody::Hostile(body) = &actor.body else {
        unreachable!();
    };
    assert_eq!(body.health, 19);
    assert_eq!(body.burn_cooldown, 20);
    assert_eq!(actor.lifecycle, ActorLifecycle::Active);
}

#[test]
fn lethal_daylight_burn_reaches_real_death_settlement_once() {
    for ready_loot in [true, false] {
        let mut state = authority();
        let mut context = TickContext::harness(&mut state, TickBudget::full());
        stage_environment(&mut context, 1000, 0);
        stage_actors(&mut context, &[burn_health_actor(1)]);
        let chunk_key = overworld_cell(mornlea_domain::BlockPos::new(2, 1, 2));
        if ready_loot {
            // A real Ready chunk supplies both sky observations and physical
            // loot slots; sparse observations alone cannot accept death loot.
            context.preload_ready_chunk(
                mornlea_server::core::world::ReadyChunk::try_new(
                    chunk_key,
                    1,
                    1,
                    mornlea_storage::Chunk {
                        sections: vec![
                            mornlea_storage::ContainerSnapshot {
                                kind: mornlea_storage::StorageKind::Single,
                                bits: 0,
                                single: AIR,
                                palette: vec![],
                                packed: vec![],
                            };
                            24
                        ],
                        drops: vec![Default::default(); 32],
                        furnaces: vec![Default::default(); 32],
                        chests: vec![Default::default(); 16],
                    },
                )
                .expect("Ready loot chunk"),
            );
        } else {
            observe_burn_sky(&mut context);
        }

        provider::run(&mut context, burn_call()).expect("lethal burn");
        let burned = find_hostile(&context, 21);
        assert_eq!(burned.survival.health(), 0, "ready loot: {ready_loot}");
        let ActorBody::Hostile(body) = &burned.body else {
            unreachable!();
        };
        assert_eq!(body.health, 0);
        assert_eq!(body.distant_ticks, 17, "dying actors skip distant removal");
        assert_eq!(burned.lifecycle, ActorLifecycle::Active);

        let death_call = RuleCall {
            phase: RulePhase::HostilePlayerDeaths,
            actor: None,
            command: None,
            internal: None,
        };
        let report = mornlea_server::rules::hostile_outcomes::run(&mut context, death_call)
            .expect("real death settlement");
        assert_eq!(report.applied, 1);
        assert_eq!(find_hostile(&context, 21).lifecycle, ActorLifecycle::Dead);
        let drops = context.read().drops(chunk_key).to_vec();
        if ready_loot {
            assert_eq!(drops.len(), 1);
            assert_eq!(
                drops[0].stack,
                ItemStack {
                    item: 45,
                    count: 1,
                    durability: 0,
                }
            );
        } else {
            assert!(drops.is_empty());
        }
        let events = context.events().to_vec();
        let repeated = mornlea_server::rules::hostile_outcomes::run(&mut context, death_call)
            .expect("repeated death settlement");
        assert_eq!(repeated.examined, 0);
        assert_eq!(repeated.applied, 0);
        assert_eq!(context.read().drops(chunk_key), drops);
        assert_eq!(context.events(), events);
    }
}

#[test]
fn night_and_roof_preserve_authoritative_burn_health() {
    for (world_time, roofed) in [(15000, false), (1000, true)] {
        let mut state = authority();
        let mut context = TickContext::harness(&mut state, TickBudget::full());
        stage_environment(&mut context, world_time, 0);
        let initial = burn_health_actor(1);
        stage_actors(&mut context, std::slice::from_ref(&initial));
        observe_burn_sky(&mut context);
        if roofed {
            observe(&mut context, mornlea_domain::BlockPos::new(2, 3, 2), STONE);
        }

        provider::run(&mut context, burn_call()).expect("protected burn");

        let actor = find_hostile(&context, 21);
        assert_eq!(actor.survival, initial.survival);
        let ActorBody::Hostile(body) = &actor.body else {
            unreachable!();
        };
        assert_eq!(body.health, 1);
        assert_eq!(body.burn_cooldown, 20);
        assert_eq!(actor.lifecycle, ActorLifecycle::Active);
    }
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

fn assert_geometry_refusal(ctx: &mut TickContext<'_>, call: RuleCall<'_>) {
    let before = geometry_snapshot(ctx);
    let events = ctx.events().to_vec();
    let cells: Vec<_> = (-4..=8)
        .flat_map(|x| {
            (-4..=4)
                .flat_map(move |z| (-64..320).map(move |y| mornlea_domain::BlockPos::new(x, y, z)))
        })
        .chain(
            (-48..=48)
                .flat_map(|z| (0..3).map(move |y| mornlea_domain::BlockPos::new(i32::MIN, y, z))),
        )
        .collect();
    let blocks: Vec<_> = cells
        .iter()
        .map(|cell| ctx.read().observation(Dimension::OVERWORLD, *cell))
        .collect();
    assert!(matches!(
        provider::run(ctx, call),
        Err(ServerError::InvalidInput { field: "actor" })
    ));
    assert_eq!(geometry_snapshot(ctx), before);
    assert_eq!(ctx.events(), events);
    let after: Vec<_> = cells
        .iter()
        .map(|cell| ctx.read().observation(Dimension::OVERWORLD, *cell))
        .collect();
    assert_eq!(after, blocks);
}

#[test]
fn geometry_motion_extreme_cells_refuse_without_effect() {
    for value in [i32::MIN as f32, i32::MAX as f32, f32::MAX, -f32::MAX] {
        for axis in [0, 2] {
            let mut state = authority();
            let session = anchor_session(&mut state);
            let mut ctx = TickContext::harness(&mut state, TickBudget::full());
            stage_environment(&mut ctx, 1000, 1);
            let mut pos = [0.5, 40.0, 0.5];
            pos[axis] = value;
            stage_actors(
                &mut ctx,
                &[
                    player_actor(session, 1, [0.5, 40.0, 0.5]),
                    hostile_actor(21, pos, NIGHTWALKER),
                ],
            );
            assert_geometry_refusal(&mut ctx, motion_call());
        }
    }
}

#[test]
fn geometry_motion_refusal_preserves_earlier_and_fresh_entries() {
    for invalid_first in [false, true] {
        for fresh in [false, true] {
            let mut state = authority();
            let mut ctx = TickContext::harness(&mut state, TickBudget::full());
            stage_environment(&mut ctx, 1000, 1);
            let mut good = hostile_actor(
                if invalid_first { 22 } else { 21 },
                [0.5, 40.0, 0.5],
                NIGHTWALKER,
            );
            let mut bad = hostile_actor(
                if invalid_first { 21 } else { 22 },
                [2.5, 40.0, 0.5],
                NIGHTWALKER,
            );
            let ActorBody::Hostile(body) = &mut bad.body else {
                unreachable!()
            };
            body.velocity = [f32::MAX, 0.0, 0.0];
            bad.motion = MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new(body.position).unwrap(),
                velocity: FiniteVec3::try_new(body.velocity).unwrap(),
                on_ground: true,
            });
            let ActorBody::Hostile(body) = &mut good.body else {
                unreachable!()
            };
            body.velocity = [1.0, 0.0, 0.0];
            good.motion = MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new(body.position).unwrap(),
                velocity: FiniteVec3::try_new(body.velocity).unwrap(),
                on_ground: true,
            });
            let mut runtime = fluid_runtime(good.key);
            if let ActorAux::Hostile { fresh: flag, .. } = &mut runtime.aux {
                *flag = fresh;
            }
            stage_actors(&mut ctx, &[good, bad]);
            ctx.stage(RuleEffect::Runtime(runtime)).unwrap();
            for x in -4..=8 {
                for z in -4..=4 {
                    for y in 38..=43 {
                        observe(
                            &mut ctx,
                            mornlea_domain::BlockPos::new(x, y, z),
                            if y < 40 { STONE } else { AIR },
                        );
                    }
                }
            }
            assert_geometry_refusal(&mut ctx, motion_call());
        }
    }
}

#[test]
fn geometry_spawn_candidate_offsets_refuse() {
    for axis in [1, 3] {
        let seed = (0..100i64)
            .find(|seed| ((splitmix64(*seed as u64 ^ NIGHT_START) >> 32) & 3) == axis)
            .unwrap();
        let mut state = authority();
        let session = anchor_session(&mut state);
        let mut ctx = TickContext::harness(&mut state, TickBudget::full());
        let mut env = environment(NIGHT_START, 0);
        env.seed = seed;
        ctx.stage(RuleEffect::Environment(env)).unwrap();
        let pos = if axis == 1 {
            [i32::MIN as f32, 40.0, 0.5]
        } else {
            [0.5, 40.0, i32::MIN as f32]
        };
        stage_actors(&mut ctx, &[player_actor(session, 1, pos)]);
        assert_geometry_refusal(&mut ctx, motion_call());
    }
}

#[test]
fn geometry_spawn_light_window_refuses() {
    let (seed, z) = (0..10000i64)
        .find_map(|seed| {
            let base = splitmix64(seed as u64 ^ NIGHT_START);
            if (base >> 32) & 3 != 2 {
                return None;
            }
            let z = 24 + (base % 25) as i32;
            (candidate_hash(seed, NIGHT_START, i32::MIN, 1, z) & 0xff < SPAWN_GATE)
                .then_some((seed, z))
        })
        .unwrap();
    let mut state = authority();
    let session = anchor_session(&mut state);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    let mut env = environment(NIGHT_START, 0);
    env.seed = seed;
    ctx.stage(RuleEffect::Environment(env)).unwrap();
    stage_actors(
        &mut ctx,
        &[player_actor(session, 1, [i32::MIN as f32, 40.0, 0.5])],
    );
    preload_flat_column(&mut ctx, i32::MIN, z);
    assert_geometry_refusal(&mut ctx, motion_call());
}

#[test]
fn geometry_burn_refusal_preserves_entire_batch() {
    for axis in [0, 2] {
        let mut state = authority();
        let mut ctx = TickContext::harness(&mut state, TickBudget::full());
        stage_environment(&mut ctx, 1000, 1);
        let mut pos = [0.5, 40.0, 0.5];
        pos[axis] = i32::MAX as f32;
        stage_actors(
            &mut ctx,
            &[
                hostile_actor_with(
                    21,
                    [0.5, 40.0, 0.5],
                    NIGHTWALKER,
                    1,
                    7,
                    false,
                    StoredPlayerId::from_bytes([0; 16]),
                ),
                hostile_actor_with(
                    22,
                    pos,
                    NIGHTWALKER,
                    1,
                    599,
                    false,
                    StoredPlayerId::from_bytes([0; 16]),
                ),
            ],
        );
        for y in 41..320 {
            observe(&mut ctx, mornlea_domain::BlockPos::new(0, y, 0), AIR);
        }
        assert_geometry_refusal(&mut ctx, burn_call());
    }
}

#[test]
fn geometry_representable_edges_defer_and_negative_motion_succeeds() {
    for edge in [
        i32::MIN as f32 + 128.0,
        f32::from_bits((i32::MAX as f32).to_bits() - 1),
    ] {
        let mut state = authority();
        let session = anchor_session(&mut state);
        let mut ctx = TickContext::harness(&mut state, TickBudget::full());
        stage_environment(&mut ctx, 1000, 1);
        stage_actors(
            &mut ctx,
            &[
                player_actor(session, 1, [0.5, 40.0, 0.5]),
                hostile_actor(21, [edge, 40.0, edge], NIGHTWALKER),
            ],
        );
        provider::run(&mut ctx, motion_call()).expect("representable unloaded edge");
        assert!(
            ctx.read()
                .runtime(find_hostile(&ctx, 21).key)
                .unwrap()
                .path
                .is_none()
        );
    }
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut ctx, 1000, 1);
    stage_actors(
        &mut ctx,
        &[hostile_actor(21, [-2.5, 40.0, -2.5], NIGHTWALKER)],
    );
    for x in -4..=0 {
        for z in -4..=0 {
            for y in 38..=43 {
                observe(
                    &mut ctx,
                    mornlea_domain::BlockPos::new(x, y, z),
                    if y < 40 { STONE } else { AIR },
                );
            }
        }
    }
    provider::run(&mut ctx, motion_call()).expect("negative motion");
    assert_eq!(
        find_hostile(&ctx, 21).motion.position().get(),
        [-2.5, 40.0, -2.5]
    );
}

#[test]
fn geometry_loaded_edge_windows_preserve_exact_cells() {
    for edge in [
        i32::MIN as f32 + 128.0,
        f32::from_bits((i32::MAX as f32).to_bits() - 1),
    ] {
        let center = edge as i32;
        let mut state = authority();
        let session = anchor_session(&mut state);
        let mut ctx = TickContext::harness(&mut state, TickBudget::full());
        stage_environment(&mut ctx, 1000, 1);
        stage_actors(
            &mut ctx,
            &[
                player_actor(session, 1, [0.5, 40.0, 110.5]),
                hostile_actor(21, [edge, 40.0, 100.5], NIGHTWALKER),
            ],
        );
        for x in center - 16..=center + 16 {
            for z in 84..=116 {
                for y in 36..=44 {
                    observe(
                        &mut ctx,
                        mornlea_domain::BlockPos::new(x, y, z),
                        if y < 40 { STONE } else { AIR },
                    );
                }
            }
        }
        preload_ready_window(&mut ctx, center, 100);
        provider::run(&mut ctx, motion_call()).expect("loaded signed edge");
        let runtime = ctx.read().runtime(find_hostile(&ctx, 21).key).unwrap();
        let path = runtime.path.as_ref().expect("exact edge chase window");
        let goal = if center < 0 { center + 16 } else { center - 16 };
        assert_eq!(path.target, mornlea_domain::BlockPos::new(goal, 40, 110));
        assert!(path.cursor < path.waypoints.len());
    }
}

#[test]
fn geometry_goal_y_distance_widens_before_subtraction() {
    let mut state = authority();
    let session = anchor_session(&mut state);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut ctx, 1000, 1);
    stage_actors(
        &mut ctx,
        &[
            player_actor(session, 1, [105.5, i32::MIN as f32, 100.5]),
            hostile_actor(21, [100.5, 40.0, 100.5], NIGHTWALKER),
        ],
    );
    preload_band_world(&mut ctx);
    provider::run(&mut ctx, motion_call()).expect("finite admitted target Y");
    let runtime = ctx.read().runtime(find_hostile(&ctx, 21).key).unwrap();
    assert_eq!(
        runtime.path.as_ref().unwrap().target,
        mornlea_domain::BlockPos::new(105, 40, 100)
    );
}

fn cadence_seed(target: [f32; 3], now: u64, observed: bool) -> (AuthorityState, SessionKey) {
    let mut state = authority();
    let session = anchor_session(&mut state);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut ctx, now, 1);
    stage_actors(
        &mut ctx,
        &[
            player_actor(session, 1, target),
            hostile_actor(21, [100.5, 40.0, 100.5], NIGHTWALKER),
        ],
    );
    ctx.preload_inventory(
        ActorKey::Player(session),
        mornlea_server::contracts::InventoryRecord::empty(),
    );
    if observed {
        preload_band_world(&mut ctx);
    }
    let residents = ctx.resident_snapshot();
    drop(ctx);
    state.commit_residents(residents);
    (state, session)
}

fn cadence_body(state: &AuthorityState) -> HostileMob {
    state
        .residents()
        .actors
        .iter()
        .find_map(|actor| match &actor.body {
            ActorBody::Hostile(body) if body.id == 21 => Some(body.clone()),
            _ => None,
        })
        .expect("carried hostile")
}

fn cadence_path(state: &AuthorityState) -> Option<mornlea_server::contracts::PathState> {
    let key = ActorKey::Hostile(mornlea_domain::HostileId::try_new(21).unwrap());
    state
        .residents()
        .runtimes
        .get(&key)
        .and_then(|runtime| runtime.path.clone())
}

fn cadence_tick(state: &mut AuthorityState, expected_time: u64) {
    let tick = state.next_tick();
    assert_eq!(
        state.residents().environment.as_ref().unwrap().world_time,
        expected_time
    );
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(publication.counters.executed_tick, tick);
    assert_eq!(state.next_tick(), tick + 1);
    assert_eq!(
        state.residents().environment.as_ref().unwrap().world_time,
        expected_time + 1,
        "the real reducer reached end-of-tick environment publication"
    );
}

fn cadence_move_player(state: &mut AuthorityState, session: SessionKey, position: [f32; 3]) {
    let mut residents = state.residents();
    let actor = residents
        .actors
        .iter_mut()
        .find(|actor| actor.key == ActorKey::Player(session))
        .unwrap();
    actor.motion = MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new(position).unwrap(),
        velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
        on_ground: true,
    });
    let ActorBody::Player(body) = &mut actor.body else {
        unreachable!()
    };
    body.current.position = position;
    state.commit_residents(residents);
}

#[test]
fn cadence_clamped_and_adjusted_goals_survive_real_carries_until_due() {
    for target in [[130.5, 40.0, 100.5], [110.5, 43.5, 100.5]] {
        let (mut state, session) = cadence_seed(target, 1000, true);
        cadence_tick(&mut state, 1000);
        let first = cadence_path(&state).expect("successful initial path");
        let successful_deadline = first.next_repath_tick;
        assert_ne!(
            first.target,
            mornlea_domain::BlockPos::new(target[0] as i32, target[1] as i32, target[2] as i32)
        );
        for now in 1001..=1003 {
            cadence_tick(&mut state, now);
            let path = cadence_path(&state).unwrap();
            assert_eq!(
                (path.generation, path.target, path.next_repath_tick),
                (first.generation, first.target, successful_deadline)
            );
            assert_eq!(cadence_body(&state).next_repath_ticks, successful_deadline);
        }
        cadence_move_player(&mut state, session, [112.5, 40.0, 100.5]);
        for now in 1004..1020 {
            cadence_tick(&mut state, now);
            let path = cadence_path(&state).unwrap();
            assert_eq!(
                (path.generation, path.target, path.next_repath_tick),
                (first.generation, first.target, successful_deadline)
            );
        }
        cadence_tick(&mut state, 1020);
        let due = cadence_path(&state).expect("current goal refreshed when due");
        assert_eq!(due.target, mornlea_domain::BlockPos::new(112, 40, 100));
        assert_eq!(due.generation, first.generation);
        assert_eq!(due.next_repath_tick, 1040);
        assert_eq!(cadence_body(&state).next_repath_ticks, 1040);
    }
}

#[test]
fn cadence_restored_future_deadline_waits_without_transient_path() {
    let (mut state, session) = cadence_seed([110.5, 40.0, 100.5], 4095, true);
    let mut residents = state.residents();
    let actor = residents
        .actors
        .iter_mut()
        .find(|actor| matches!(actor.key, ActorKey::Hostile(_)))
        .unwrap();
    let ActorBody::Hostile(body) = &mut actor.body else {
        unreachable!()
    };
    body.has_target = true;
    body.player_id = StoredPlayerId::from_bytes(uuid_bytes(1));
    body.next_repath_ticks = 4096;
    state.commit_residents(residents);
    cadence_tick(&mut state, 4095);
    assert!(cadence_path(&state).is_none());
    assert_eq!(cadence_body(&state).position, [100.5, 40.0, 100.5]);
    assert_eq!(cadence_body(&state).next_repath_ticks, 4096);
    assert_eq!(
        cadence_body(&state).player_id,
        StoredPlayerId::from_bytes(uuid_bytes(1))
    );
    cadence_tick(&mut state, 4096);
    assert_eq!(cadence_path(&state).unwrap().next_repath_tick, 4116);
    assert_eq!(cadence_body(&state).next_repath_ticks, 4116);
    assert!(
        state
            .residents()
            .actors
            .iter()
            .any(|actor| actor.key == ActorKey::Player(session))
    );
}

#[test]
fn cadence_new_nearest_waits_and_changed_uuid_preserves_generation() {
    let (mut state, _) = cadence_seed([130.5, 40.0, 100.5], 1000, true);
    cadence_tick(&mut state, 1000);
    let newcomer = admit_session(&mut state, 2, "cadence-new");
    let mut residents = state.residents();
    residents
        .actors
        .push(player_actor(newcomer, 2, [110.5, 40.0, 100.5]));
    residents.inventories.insert(
        ActorKey::Player(newcomer),
        mornlea_server::contracts::InventoryRecord::empty(),
    );
    let key = ActorKey::Hostile(mornlea_domain::HostileId::try_new(21).unwrap());
    residents
        .runtimes
        .get_mut(&key)
        .unwrap()
        .path
        .as_mut()
        .unwrap()
        .generation = 7;
    state.commit_residents(residents);
    for now in 1001..1020 {
        cadence_tick(&mut state, now);
        assert_eq!(
            cadence_body(&state).player_id,
            StoredPlayerId::from_bytes(uuid_bytes(1))
        );
        assert_eq!(cadence_path(&state).unwrap().generation, 7);
    }
    cadence_tick(&mut state, 1020);
    assert_eq!(
        cadence_body(&state).player_id,
        StoredPlayerId::from_bytes(uuid_bytes(2))
    );
    assert_eq!(cadence_path(&state).unwrap().generation, 8);
    assert_eq!(cadence_body(&state).next_repath_ticks, 1040);
}

#[test]
fn cadence_target_loss_waits_until_dispatch_and_keeps_existing_movement() {
    let (mut state, session) = cadence_seed([130.5, 40.0, 100.5], 1000, true);
    cadence_tick(&mut state, 1000);
    let mut residents = state.residents();
    residents
        .actors
        .iter_mut()
        .find(|actor| actor.key == ActorKey::Player(session))
        .unwrap()
        .lifecycle = ActorLifecycle::Dead;
    state.commit_residents(residents);
    let previous_x = cadence_body(&state).position[0];
    cadence_tick(&mut state, 1001);
    assert!(cadence_body(&state).position[0] > previous_x);
    assert_eq!(
        cadence_body(&state).player_id,
        StoredPlayerId::from_bytes(uuid_bytes(1))
    );
    assert_eq!(cadence_body(&state).next_repath_ticks, 1020);
    for now in 1002..=1020 {
        cadence_tick(&mut state, now);
    }
    let body = cadence_body(&state);
    assert!(!body.has_target);
    assert_eq!(body.player_id, StoredPlayerId::from_bytes([0; 16]));
    assert_eq!(body.next_repath_ticks, 1021);
    assert!(cadence_path(&state).is_none());
}

#[test]
fn cadence_exhaustion_clears_path_and_retains_next_calendar_tick() {
    let (mut state, _) = cadence_seed([130.5, 40.0, 100.5], 1000, true);
    cadence_tick(&mut state, 1000);
    let mut residents = state.residents();
    let pos = cadence_body(&state).position;
    let key = ActorKey::Hostile(mornlea_domain::HostileId::try_new(21).unwrap());
    let path = residents
        .runtimes
        .get_mut(&key)
        .unwrap()
        .path
        .as_mut()
        .unwrap();
    path.waypoints = vec![mornlea_domain::BlockPos::new(
        pos[0].floor() as i32,
        40,
        pos[2].floor() as i32,
    )];
    path.cursor = 0;
    path.next_repath_tick = 1020;
    state.commit_residents(residents);
    cadence_tick(&mut state, 1001);
    assert!(cadence_path(&state).is_none());
    assert_eq!(cadence_body(&state).next_repath_ticks, 1002);
}

#[test]
fn cadence_uncovered_refresh_retries_next_calendar_tick() {
    let (mut state, _) = cadence_seed([110.5, 40.0, 100.5], 1000, false);
    cadence_tick(&mut state, 1000);
    assert!(cadence_path(&state).is_none());
    assert_eq!(cadence_body(&state).next_repath_ticks, 1001);
    cadence_tick(&mut state, 1001);
    assert!(cadence_path(&state).is_none());
    assert_eq!(cadence_body(&state).next_repath_ticks, 1002);
}

#[test]
fn cadence_success_deadline_uses_frozen_calendar_time() {
    let (mut state, _) = cadence_seed([110.5, 40.0, 100.5], 1000, true);
    assert_eq!(state.next_tick(), 0);
    cadence_tick(&mut state, 1000);
    assert_eq!(cadence_body(&state).next_repath_ticks, 1020);
    assert_eq!(cadence_path(&state).unwrap().next_repath_tick, 1020);
}

#[test]
fn cadence_hurler_bands_follow_owned_uuid_live_position_between_dispatches() {
    let (mut state, owned) = cadence_seed([130.5, 40.0, 100.5], 1000, true);
    let mut residents = state.residents();
    let ActorBody::Hostile(body) = &mut residents
        .actors
        .iter_mut()
        .find(|actor| matches!(actor.key, ActorKey::Hostile(_)))
        .unwrap()
        .body
    else {
        unreachable!()
    };
    body.kind = HURLER;
    state.commit_residents(residents);
    cadence_tick(&mut state, 1000);
    let initial_x = cadence_body(&state).position[0];
    let newcomer = admit_session(&mut state, 2, "band-new");
    let mut residents = state.residents();
    residents
        .actors
        .push(player_actor(newcomer, 2, [105.5, 40.0, 100.5]));
    residents.inventories.insert(
        ActorKey::Player(newcomer),
        mornlea_server::contracts::InventoryRecord::empty(),
    );
    state.commit_residents(residents);
    cadence_tick(&mut state, 1001);
    let approaching = cadence_body(&state);
    assert!(
        approaching.position[0] > initial_x,
        "a nearer retreat-band candidate cannot replace the owned approaching target"
    );
    assert_eq!(
        approaching.player_id,
        StoredPlayerId::from_bytes(uuid_bytes(1))
    );
    let path = cadence_path(&state).unwrap();
    cadence_move_player(&mut state, owned, [108.5, 40.0, 100.5]);
    cadence_tick(&mut state, 1002);
    let holding = cadence_body(&state);
    assert_eq!(holding.player_id, approaching.player_id);
    assert_eq!(
        holding.yaw, approaching.yaw,
        "the owned player's new hold-band position suppresses retreat steering"
    );
    let retained = cadence_path(&state).unwrap();
    assert_eq!(
        (
            retained.generation,
            retained.target,
            retained.next_repath_tick
        ),
        (path.generation, path.target, path.next_repath_tick)
    );
}

#[test]
fn cadence_failed_search_retries_next_calendar_tick() {
    let (mut state, _) = cadence_seed([110.5, 40.0, 100.5], 1000, true);
    let mut residents = state.residents();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ready_hydrate(&mut ctx, &residents, None);
    for (key, generation, revision, mut chunk) in residents.ready_snapshot() {
        if key.pos.x() == 105 >> 4 {
            for z in 84..=116 {
                if z >> 4 == key.pos.z() {
                    for y in 40..=44 {
                        set_ready_cell(&mut chunk, mornlea_domain::BlockPos::new(105, y, z), STONE);
                    }
                }
            }
            ctx.preload_ready_chunk(
                mornlea_server::core::world::ReadyChunk::try_new(key, generation, revision, chunk)
                    .unwrap(),
            );
        }
    }
    residents = ctx.resident_snapshot();
    drop(ctx);
    state.commit_residents(residents);
    cadence_tick(&mut state, 1000);
    assert!(cadence_path(&state).is_none());
    assert_eq!(cadence_body(&state).next_repath_ticks, 1001);
}

fn cadence_changed_identity_context(
    ctx: &mut TickContext<'_>,
    first: SessionKey,
    second: SessionKey,
    generation: u64,
) {
    stage_environment(ctx, 1000, 1);
    stage_actors(
        ctx,
        &[
            player_actor(first, 1, [130.5, 40.0, 100.5]),
            hostile_actor(21, [100.5, 40.0, 100.5], NIGHTWALKER),
        ],
    );
    preload_band_world(ctx);
    provider::run(ctx, motion_call()).unwrap();
    let key = find_hostile(ctx, 21).key;
    let mut runtime = ctx.read().runtime(key).unwrap().clone();
    runtime.path.as_mut().unwrap().generation = generation;
    ctx.stage(RuleEffect::Runtime(runtime)).unwrap();
    stage_actors(ctx, &[player_actor(second, 2, [110.5, 40.0, 100.5])]);
    stage_environment(ctx, 1020, 1);
}

#[test]
fn cadence_generation_advances_from_the_existing_path_on_uuid_change() {
    let mut state = authority();
    let (first, second) = two_sessions(&mut state);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    cadence_changed_identity_context(&mut ctx, first, second, 7);
    provider::run(&mut ctx, motion_call()).unwrap();
    let actor = find_hostile(&ctx, 21);
    let ActorBody::Hostile(body) = &actor.body else {
        unreachable!()
    };
    assert_eq!(body.player_id, StoredPlayerId::from_bytes(uuid_bytes(2)));
    assert_eq!(
        ctx.read()
            .runtime(actor.key)
            .unwrap()
            .path
            .as_ref()
            .unwrap()
            .generation,
        8
    );
}

#[test]
fn cadence_generation_exhaustion_refuses_without_partial_effects() {
    let mut state = authority();
    let (first, second) = two_sessions(&mut state);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    cadence_changed_identity_context(&mut ctx, first, second, u64::MAX);
    let before = geometry_snapshot(&ctx);
    let events = ctx.events().to_vec();
    assert!(matches!(
        provider::run(&mut ctx, motion_call()),
        Err(ServerError::InvalidInput { field: "actor" })
    ));
    assert_eq!(geometry_snapshot(&ctx), before);
    assert_eq!(ctx.events(), events);
}

#[test]
fn geometry_both_edge_axes_complete_at_the_exhaustion_retry() {
    for edge in [
        i32::MIN as f32 + 128.0,
        f32::from_bits((i32::MAX as f32).to_bits() - 1),
    ] {
        let center = edge as i32;
        let mut state = authority();
        let session = anchor_session(&mut state);
        let mut ctx = TickContext::harness(&mut state, TickBudget::full());
        stage_environment(&mut ctx, 1000, 1);
        stage_actors(
            &mut ctx,
            &[
                player_actor(session, 1, [0.5, 40.0, 0.5]),
                hostile_actor(21, [edge, 40.0, edge], NIGHTWALKER),
            ],
        );
        for x in center - 16..=center + 16 {
            for z in center - 16..=center + 16 {
                for y in 36..=44 {
                    observe(
                        &mut ctx,
                        mornlea_domain::BlockPos::new(x, y, z),
                        if y < 40 { STONE } else { AIR },
                    );
                }
            }
        }
        preload_ready_window(&mut ctx, center, center);
        // Float32 projects every horizontal cell center in this window to
        // the same position, so all arrived waypoints clear in this call.
        provider::run(&mut ctx, motion_call()).expect("representable signed axes");
        let actor = find_hostile(&ctx, 21);
        assert!(ctx.read().runtime(actor.key).unwrap().path.is_none());
        let ActorBody::Hostile(body) = &actor.body else {
            unreachable!()
        };
        assert_eq!(body.position[0], edge);
        assert_eq!(body.position[2], edge);
        assert_eq!(body.next_repath_ticks, 1001);
        assert!(body.has_target);
        assert_eq!(body.player_id, StoredPlayerId::from_bytes(uuid_bytes(1)));
    }
}

/// Public replay hydration materializes carried values before Ready install.
/// No sparse preload can override Ready height or revision ownership.
fn ready_hydrate(
    ctx: &mut TickContext<'_>,
    residents: &mornlea_server::state::ResidentTickState,
    missing: Option<ChunkKey>,
) {
    for (key, generation, revision, chunk) in residents.ready_snapshot() {
        if Some(key) != missing {
            ctx.preload_ready_chunk(
                mornlea_server::core::world::ReadyChunk::try_new(key, generation, revision, chunk)
                    .unwrap(),
            );
        }
    }
    for observed in residents.blocks.values() {
        if ctx.read().ready_chunk_revision(observed.key).is_none() {
            ctx.preload_block(*observed);
        }
    }
    stage_actors(ctx, &residents.actors);
    for runtime in residents.runtimes.values() {
        ctx.stage(RuleEffect::Runtime(runtime.clone())).unwrap();
    }
    for (key, inventory) in &residents.inventories {
        ctx.preload_inventory(*key, *inventory);
    }
    ctx.stage(RuleEffect::Environment(
        residents.environment.as_ref().unwrap().clone(),
    ))
    .unwrap();
    assert!(residents.mining.is_empty());
    assert!(residents.projectiles.is_empty());
    assert!(residents.sleeping.is_empty());
}

fn ready_commit_hydrated(
    ctx: &TickContext<'_>,
    before: &mornlea_server::state::ResidentTickState,
) -> mornlea_server::state::ResidentTickState {
    let mut next = ctx.resident_snapshot();
    let changed = ctx.changed_blocks();
    // Materialization preserves values; restore untouched carried CAS lanes.
    // Changed cells retain their newly admitted transaction observations.
    for (key, observed) in &before.blocks {
        if !changed.iter().any(|cell| (cell.key, cell.pos) == *key) {
            next.blocks.insert(*key, *observed);
        }
    }
    next
}

fn assert_ready_cache_retained(
    actual: &mornlea_server::contracts::PathState,
    expected: &mornlea_server::contracts::PathState,
) {
    assert_eq!(actual.generation, expected.generation);
    assert_eq!(actual.target, expected.target);
    assert_eq!(actual.revisions, expected.revisions);
    assert_eq!(actual.waypoints, expected.waypoints);
    assert_eq!(actual.next_repath_tick, expected.next_repath_tick);
    assert!(actual.cursor >= expected.cursor);
    assert!(actual.cursor < actual.waypoints.len());
}

fn ready_saved_revision(state: &AuthorityState, key: ChunkKey) -> u64 {
    state
        .residents()
        .ready_snapshot()
        .into_iter()
        .find(|chunk| chunk.0 == key)
        .unwrap()
        .2
}

/// Admit real writes through the public transaction/commit boundary, then
/// let actual reducer ticks consume the carried state. This is not network
/// command admission; changed cell CAS may rebase when materialized here.
fn ready_admit_writes(
    state: &mut AuthorityState,
    pos: mornlea_domain::BlockPos,
    replacements: &[u16],
) -> u64 {
    let before = state.residents();
    let key = overworld_cell(pos);
    let previous = ready_saved_revision(state, key);
    let mut ctx = TickContext::harness(state, TickBudget::full());
    ready_hydrate(&mut ctx, &before, None);
    assert_eq!(ctx.read().ready_chunk_revision(key), Some(previous));
    for block in replacements {
        let observed = ctx.read().observation(key.dimension, pos).unwrap();
        let outcome = ctx
            .transaction()
            .try_system(
                mornlea_server::contracts::SystemRule::Support,
                vec![mornlea_server::contracts::BlockWrite::try_new(observed, *block).unwrap()],
            )
            .unwrap();
        assert_eq!(outcome.changed.len(), 1);
        assert_eq!(ctx.read().ready_chunk_revision(key), Some(previous + 1));
    }
    let next = ready_commit_hydrated(&ctx, &before);
    drop(ctx);
    state.commit_residents(next);
    assert_eq!(ready_saved_revision(state, key), previous + 1);
    previous + 1
}

#[test]
fn ready_sparse_chunk_samples_defer_without_ready_ownership() {
    let mut state = authority();
    let session = anchor_session(&mut state);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    stage_environment(&mut ctx, 1000, 1);
    stage_actors(
        &mut ctx,
        &[
            player_actor(session, 1, [110.5, 40.0, 100.5]),
            hostile_actor(21, [100.5, 40.0, 100.5], NIGHTWALKER),
        ],
    );
    for x in [84, 100, 116] {
        for z in [84, 100, 116] {
            observe(&mut ctx, mornlea_domain::BlockPos::new(x, 36, z), AIR);
        }
    }
    // A valid sparse corridor makes the old search succeed without Ready data.
    for x in 100..=110 {
        for y in 39..=41 {
            observe(
                &mut ctx,
                mornlea_domain::BlockPos::new(x, y, 100),
                if y == 39 { STONE } else { AIR },
            );
        }
    }
    provider::run(&mut ctx, motion_call()).unwrap();
    let actor = find_hostile(&ctx, 21);
    assert!(ctx.read().runtime(actor.key).unwrap().path.is_none());
    let ActorBody::Hostile(body) = &actor.body else {
        unreachable!()
    };
    assert_eq!(body.next_repath_ticks, 1001);
}

#[test]
fn ready_grid_captures_exact_chunk_revisions_instead_of_cell_cas() {
    let (mut state, _) = cadence_seed([110.5, 40.0, 100.5], 1000, true);
    let before = state.residents();
    let first = mornlea_domain::BlockPos::new(84, 36, 84);
    let key = overworld_cell(first);
    let previous = ready_saved_revision(&state, key);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ready_hydrate(&mut ctx, &before, None);
    for block in [13, STONE] {
        let observed = ctx.read().observation(key.dimension, first).unwrap();
        ctx.transaction()
            .try_system(
                mornlea_server::contracts::SystemRule::Support,
                vec![mornlea_server::contracts::BlockWrite::try_new(observed, block).unwrap()],
            )
            .unwrap();
    }
    let cas = ctx
        .read()
        .observation(key.dimension, first)
        .unwrap()
        .revision;
    assert_eq!(cas, previous + 2);
    assert_eq!(ctx.read().ready_chunk_revision(key), Some(previous + 1));
    provider::run(&mut ctx, motion_call()).unwrap();
    let actor = find_hostile(&ctx, 21);
    let path = ctx
        .read()
        .runtime(actor.key)
        .unwrap()
        .path
        .as_ref()
        .unwrap()
        .clone();
    assert_eq!(path.revisions.len(), 9);
    for (key, revision) in path.revisions {
        assert_eq!(Some(revision), ctx.read().ready_chunk_revision(key));
    }
    let ActorBody::Hostile(body) = &actor.body else {
        unreachable!()
    };
    assert!(body.position[0] > 100.5, "a freshly built Ready path moves");
    assert_eq!(body.next_repath_ticks, 1020);
}

#[test]
fn ready_missing_one_covered_key_defers_despite_sparse_sample() {
    let (mut state, _) = cadence_seed([110.5, 40.0, 100.5], 1000, true);
    let residents = state.residents();
    let missing = overworld_cell(mornlea_domain::BlockPos::new(116, 36, 116));
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ready_hydrate(&mut ctx, &residents, Some(missing));
    observe(&mut ctx, mornlea_domain::BlockPos::new(116, 36, 116), AIR);
    assert_eq!(ctx.read().ready_chunk_revision(missing), None);
    provider::run(&mut ctx, motion_call()).unwrap();
    let actor = find_hostile(&ctx, 21);
    assert!(ctx.read().runtime(actor.key).unwrap().path.is_none());
    let ActorBody::Hostile(body) = &actor.body else {
        unreachable!()
    };
    assert_eq!(body.next_repath_ticks, 1001);
}

#[test]
fn ready_two_later_committed_writes_invalidate_real_carried_paths() {
    let (mut state, _) = cadence_seed([130.5, 40.0, 100.5], 1000, true);
    cadence_tick(&mut state, 1000);
    let first = cadence_path(&state).unwrap();
    cadence_tick(&mut state, 1001);
    assert_eq!(cadence_path(&state).unwrap(), first);
    let pos = mornlea_domain::BlockPos::new(84, 36, 84);
    let key = overworld_cell(pos);
    let revision = ready_admit_writes(&mut state, pos, &[13]);
    cadence_tick(&mut state, 1002);
    assert!(cadence_path(&state).is_none());
    assert_eq!(
        cadence_body(&state).player_id,
        StoredPlayerId::from_bytes(uuid_bytes(1))
    );
    assert_eq!(cadence_body(&state).next_repath_ticks, 1003);
    cadence_tick(&mut state, 1003);
    let refreshed = cadence_path(&state).unwrap();
    assert!(refreshed.revisions.contains(&(key, revision)));
    assert_eq!(refreshed.next_repath_tick, 1023);
    cadence_tick(&mut state, 1004);
    assert_ready_cache_retained(&cadence_path(&state).unwrap(), &refreshed);
    let second_revision = ready_admit_writes(&mut state, pos, &[STONE]);
    assert_eq!(second_revision, revision + 1);
    cadence_tick(&mut state, 1005);
    assert!(cadence_path(&state).is_none());
    assert_eq!(
        cadence_body(&state).player_id,
        StoredPlayerId::from_bytes(uuid_bytes(1))
    );
    assert_eq!(cadence_body(&state).next_repath_ticks, 1006);
    cadence_tick(&mut state, 1006);
    let second = cadence_path(&state).unwrap();
    assert!(second.revisions.contains(&(key, second_revision)));
    assert_eq!(second.next_repath_tick, 1026);
    cadence_tick(&mut state, 1007);
    assert_ready_cache_retained(&cadence_path(&state).unwrap(), &second);
}

#[test]
fn ready_unrelated_chunk_write_preserves_carried_cache_and_deadline() {
    let (mut state, _) = cadence_seed([130.5, 40.0, 100.5], 1000, true);
    let before = state.residents();
    let pos = mornlea_domain::BlockPos::new(1000, 60, 1000);
    let key = overworld_cell(pos);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ready_hydrate(&mut ctx, &before, None);
    ctx.preload_ready_chunk(
        mornlea_server::core::world::ReadyChunk::try_new(key, 1, 77, ready_air_chunk()).unwrap(),
    );
    let next = ctx.resident_snapshot();
    drop(ctx);
    state.commit_residents(next);
    cadence_tick(&mut state, 1000);
    let first = cadence_path(&state).unwrap();
    assert!(!first.revisions.iter().any(|saved| saved.0 == key));
    assert_eq!(ready_admit_writes(&mut state, pos, &[STONE]), 78);
    cadence_tick(&mut state, 1001);
    let retained = cadence_path(&state).unwrap();
    assert_eq!(retained.generation, first.generation);
    assert_eq!(retained.target, first.target);
    assert_eq!(retained.revisions, first.revisions);
    assert_eq!(retained.next_repath_tick, first.next_repath_tick);
    assert_eq!(cadence_body(&state).next_repath_ticks, 1020);
}

fn ready_corrupt_cache(case: &str) {
    let (mut state, _) = cadence_seed([130.5, 40.0, 100.5], 1000, true);
    cadence_tick(&mut state, 1000);
    let residents = state.residents();
    let key = ActorKey::Hostile(mornlea_domain::HostileId::try_new(21).unwrap());
    let mut runtime = residents.runtimes[&key].clone();
    let path = runtime.path.as_mut().unwrap();
    let missing = if case == "missing" {
        Some(path.revisions[0].0)
    } else {
        None
    };
    match case {
        "missing" => {}
        "changed" => path.revisions[0].1 += 1,
        "dimension" => path.revisions[0].0.dimension = Dimension::new(1).unwrap(),
        "outside" => path.revisions[0].0.pos = ChunkPos::new(30, 30),
        "oversized" => path.revisions.push(path.revisions[0]),
        "exhausted" => path.cursor = path.waypoints.len(),
        _ => unreachable!(),
    }
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ready_hydrate(&mut ctx, &residents, missing);
    if let Some(missing) = missing {
        let pos = mornlea_domain::BlockPos::new(missing.pos.x() * 16, 36, missing.pos.z() * 16);
        observe(&mut ctx, pos, AIR);
        assert_eq!(ctx.read().ready_chunk_revision(missing), None);
    }
    ctx.stage(RuleEffect::Runtime(runtime)).unwrap();
    provider::run(&mut ctx, motion_call()).unwrap();
    let actor = find_hostile(&ctx, 21);
    assert!(
        ctx.read().runtime(actor.key).unwrap().path.is_none(),
        "{case}"
    );
    let ActorBody::Hostile(body) = &actor.body else {
        unreachable!()
    };
    assert!(body.has_target);
    assert_eq!(body.player_id, StoredPlayerId::from_bytes(uuid_bytes(1)));
    assert_eq!(body.next_repath_ticks, 1002);
}

#[test]
fn ready_cached_missing_or_sparse_only_key_refuses_reuse() {
    ready_corrupt_cache("missing");
}
#[test]
fn ready_cached_changed_revision_refuses_reuse() {
    ready_corrupt_cache("changed");
}
#[test]
fn ready_cached_wrong_dimension_refuses_reuse() {
    ready_corrupt_cache("dimension");
}
#[test]
fn ready_cached_outside_current_view_refuses_reuse() {
    ready_corrupt_cache("outside");
}
#[test]
fn ready_cached_oversized_revision_set_refuses_reuse() {
    ready_corrupt_cache("oversized");
}
#[test]
fn ready_cached_exhausted_cursor_refuses_reuse() {
    ready_corrupt_cache("exhausted");
}

#[test]
fn ready_walker_hold_and_hurler_hold_or_retreat_bypass_stale_cache() {
    for (kind, distance) in [(NIGHTWALKER, 1.0), (HURLER, 8.0), (HURLER, 3.0)] {
        let (mut state, session) = cadence_seed([130.5, 40.0, 100.5], 1000, true);
        cadence_tick(&mut state, 1000);
        let mut residents = state.residents();
        let key = ActorKey::Hostile(mornlea_domain::HostileId::try_new(21).unwrap());
        let ActorBody::Hostile(body) = &mut residents
            .actors
            .iter_mut()
            .find(|a| a.key == key)
            .unwrap()
            .body
        else {
            unreachable!()
        };
        body.kind = kind;
        residents
            .runtimes
            .get_mut(&key)
            .unwrap()
            .path
            .as_mut()
            .unwrap()
            .revisions[0]
            .1 += 1;
        state.commit_residents(residents);
        let position = cadence_body(&state).position;
        cadence_move_player(
            &mut state,
            session,
            [position[0] + distance, 40.0, position[2]],
        );
        let stale = cadence_path(&state).unwrap();
        let before_yaw = cadence_body(&state).yaw;
        cadence_tick(&mut state, 1001);
        assert_eq!(cadence_path(&state).unwrap(), stale);
        assert_eq!(cadence_body(&state).next_repath_ticks, 1020);
        if kind == HURLER && distance < 6.0 {
            assert_ne!(cadence_body(&state).yaw, before_yaw);
        }
    }
}

#[test]
fn ready_actual_drop_change_invalidates_carried_path() {
    let (mut state, _) = cadence_seed([130.5, 40.0, 100.5], 1000, true);
    cadence_tick(&mut state, 1000);
    let before = state.residents();
    let pos = mornlea_domain::BlockPos::new(84, 60, 84);
    let key = overworld_cell(pos);
    let revision = ready_saved_revision(&state, key);
    let mutation_tick = state.next_tick();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ready_hydrate(&mut ctx, &before, None);
    let batch = mornlea_server::contracts::DropBatch::try_new(
        mornlea_server::contracts::DropSource::System {
            rule: mornlea_server::contracts::SystemRule::Support,
            tick: mutation_tick,
            target: pos,
        },
        Dimension::OVERWORLD,
        FiniteVec3::try_new([84.5, 60.5, 84.5]).unwrap(),
        vec![ItemStack {
            item: 2,
            count: 1,
            durability: 0,
        }],
        5,
    )
    .unwrap();
    ctx.transaction()
        .try_system_with_drops(
            mornlea_server::contracts::SystemRule::Support,
            vec![],
            batch,
        )
        .unwrap();
    assert_eq!(ctx.read().ready_chunk_revision(key), Some(revision + 1));
    let next = ready_commit_hydrated(&ctx, &before);
    drop(ctx);
    state.commit_residents(next);
    assert_eq!(ready_saved_revision(&state, key), revision + 1);
    cadence_tick(&mut state, 1001);
    assert!(cadence_path(&state).is_none());
    assert_eq!(cadence_body(&state).next_repath_ticks, 1002);
}

#[test]
fn ready_actual_container_change_invalidates_carried_path() {
    let (mut state, _) = cadence_seed([130.5, 40.0, 100.5], 1000, true);
    let pos = mornlea_domain::BlockPos::new(84, 60, 84);
    let key = overworld_cell(pos);
    // Establish the physical chest before the first path; this is setup,
    // separate from the later accepted container mutation being measured.
    ready_admit_writes(&mut state, pos, &[11]);
    cadence_tick(&mut state, 1000);
    let residents = state.residents();
    let revision = ready_saved_revision(&state, key);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ready_hydrate(&mut ctx, &residents, None);
    let before = ctx
        .read()
        .container_at(
            Dimension::OVERWORLD,
            pos,
            mornlea_domain::ContainerKind::Chest,
        )
        .unwrap();
    let mut after = before.clone();
    let mornlea_server::contracts::ContainerSlots::Chest(items) = &mut after.slots else {
        unreachable!()
    };
    items[0] = ItemStack {
        item: 2,
        count: 1,
        durability: 0,
    };
    ctx.stage(RuleEffect::WorldContainer {
        dimension: Dimension::OVERWORLD,
        before,
        after,
    })
    .unwrap();
    assert_eq!(ctx.read().ready_chunk_revision(key), Some(revision + 1));
    let next = ready_commit_hydrated(&ctx, &residents);
    drop(ctx);
    state.commit_residents(next);
    assert_eq!(ready_saved_revision(&state, key), revision + 1);
    cadence_tick(&mut state, 1001);
    assert!(cadence_path(&state).is_none());
    assert_eq!(
        cadence_body(&state).player_id,
        StoredPlayerId::from_bytes(uuid_bytes(1))
    );
    assert_eq!(cadence_body(&state).next_repath_ticks, 1002);
}
