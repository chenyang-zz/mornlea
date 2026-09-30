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
    // at and above it.
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
    assert_eq!(path.next_repath_tick, 20, "repath cadence is 20 ticks");
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

/// Flat 33x9x33 walk volume around the scene: stone below y=40, air above.
fn preload_band_world(context: &mut TickContext<'_>) {
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
