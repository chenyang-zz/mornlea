//! Passive lifecycle replay: spawn admission, the priority chain, grazing,
//! temptation and transient discipline.
//!
//! Every scene below mirrors a frozen Go oracle row, cited at each case:
//!
//! - Spawn admission from `advancePassiveSpawn`
//!   (`packages/server/sim/entity/passive_spawn.go`): the global cap 32 is the
//!   cheapest precondition and short-circuits before any candidate derivation,
//!   one candidate is derived per tick, the anchor is an overworld daytime
//!   player, the local cap refuses the seventh passive within a player's
//!   48-block radius, the candidate column needs a grass support with two air
//!   cells on loaded ground, and the derived candidate id follows the
//!   `HostileCandidateHash` chain with the 64-rehash budget
//!   (`TestPassiveSpawnRejectsAtGlobalCap`,
//!   `TestPassiveSpawnValidatesAtMostOneCandidatePerTick`).
//! - The newborn skips its first movement tick: the `fresh` convention in
//!   `advancePassiveMovement` clears the flag the same batch without
//!   integrating, so the spawned pose is exact.
//! - Priority chain from `passiveStepInput`
//!   (`packages/server/sim/entity/passive.go`): submerged dry-turn, shoreline
//!   avoidance, flee, graze freeze, wheat temptation, idle look, wander.
//! - Temptation from `passiveTemptTarget` and `passiveTempt.go`: wheat in the
//!   authoritative selected hotbar slot within 8 blocks inclusive, the nearest
//!   holder first with the ascending-session tie, the 2.5 stop distance and
//!   the 0.2 bounded turn
//!   (`TestPassiveTemptRadiusBoundary`,
//!   `TestPassiveTemptStopsExactlyAtTwoAndHalf`,
//!   `TestPassiveTemptStoppedCowTurnsToFacePlayer`).
//! - Grazing from `advancePassiveGrazeOne`
//!   (`packages/server/sim/entity/passive_graze.go`) with the sampler row
//!   `PassiveGrazeHit` (`packages/server/updates/sampler.go`): the 1-in-600
//!   roll under salt `0x51ab3e4d07c3f291`, the 20-tick event counting the
//!   trigger tick, the frozen pose, and the single-cell grass-to-dirt
//!   settlement through the observed-basis system transaction exactly 19 ticks
//!   after the trigger (`TestPassiveGrazeTurnsGrassToDirtAfterTwentyTicks`).
//! - An unready trigger cell discards the settlement without writing
//!   (`TestPassiveGrazeUnloadedChunkSettlesNothing`).
//! - Transient discipline from `RestorePassive` (`passive.go`): the save body
//!   owns no transient lane, so a restored resident is admitted with the
//!   frozen zeroed event defaults re-anchored at the loaded position
//!   (`TestPassiveGrazeTransientAcrossRestart`).
//!
//! The fixture integer mirrors (`splitmix64`, `graze_hit`, `candidate_hash`,
//! `spawn_column`) cite the Go sampler and spawn-column rows; each test first
//! replays the known-answer vectors from `sampler_entity_test.go` so a drifted
//! mirror cannot pick fixture values.
//!
//! No case chooses a value the oracle does not pin.

use mornlea_domain::{
    BlockPos, ChunkPos, Dimension, FiniteVec3, LookAngles, MotionState, MotionStateParts,
    PassiveId, SurvivalState, SurvivalStateParts, Weather,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::*;
use mornlea_server::core::world::ReadyChunk;
use mornlea_server::rules::passives as provider;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{
    Chunk, ContainerSnapshot, ItemStack, PassiveMob, PlayerLocation, PlayerSave, StorageKind,
};

// Stable block numbers, mirrored from the frozen const block in
// `packages/shared/core/block.go`: `AirID` 0, `DirtID` 3, `GrassID` 4.
const AIR: u16 = 0;
const DIRT: u16 = 3;
const GRASS: u16 = 4;
// Mature wheat (`core.ItemWheat`, the 36th entry of the frozen item table in
// `packages/shared/core/item.go`).
const ITEM_WHEAT: u16 = 35;

// The frozen graze-roll constants (`PassiveGrazeRollSalt` and
// `PassiveGrazePeriodTicks`, `packages/server/updates/sampler.go`).
const GRAZE_SALT: u64 = 0x51ab_3e4d_07c3_f291;
const GRAZE_PERIOD: u64 = 600;

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        0,
    )
    .expect("authority")
}

fn admitted(tag: u8, name: &str) -> mornlea_protocol::AdmittedLogin {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = mornlea_domain::PlayerId::try_from_bytes(bytes).expect("player id");
    let start = LoginStart::new(id, name, 8).expect("login start");
    let inbound = LoginStart::decode_inbound(&start.encode().expect("encoded")).expect("inbound");
    admit_login(inbound).expect("admitted login")
}

fn harness_context(authority: &mut AuthorityState) -> TickContext<'_> {
    TickContext::harness(authority, TickBudget::full())
}

fn environment(world_time: u64) -> EnvironmentState {
    EnvironmentState {
        seed: 0,
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

fn overworld_key(pos: BlockPos) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
    }
}

fn observation(pos: BlockPos, block: u16) -> BlockObservation {
    BlockObservation::try_new(overworld_key(pos), 1, 1, pos, block).expect("block observation")
}

/// One movement-flat room: staged air above a grass floor, the fixture shape
/// of `movementFlatChunk` in `packages/server/sim/entity/movement_test.go`.
fn stage_room(context: &mut TickContext<'_>, x0: i32, x1: i32, z0: i32, z1: i32, top: i32) {
    for x in x0..=x1 {
        for z in z0..=z1 {
            for y in 1..=top {
                context.preload_block(observation(BlockPos::new(x, y, z), AIR));
            }
        }
    }
    for x in x0..=x1 {
        for z in z0..=z1 {
            context.preload_block(observation(BlockPos::new(x, 0, z), GRASS));
        }
    }
}

/// The spawn candidate triple: one grass support with two air cells above.
fn stage_candidate_triple(context: &mut TickContext<'_>, x: i32, z: i32) {
    context.preload_block(observation(BlockPos::new(x, 0, z), GRASS));
    context.preload_block(observation(BlockPos::new(x, 1, z), AIR));
    context.preload_block(observation(BlockPos::new(x, 2, z), AIR));
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn player_actor(session: SessionKey, position: [f32; 3]) -> ActorRecord {
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
            display_name: "Tester".to_owned(),
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
            respawn_position: [0.0, 0.0, 0.0],
            respawn_dimension: 0,
            armor: [ItemStack::default(); 4],
        }),
    )
    .expect("player actor")
}

fn passive_key(id: u64) -> ActorKey {
    ActorKey::Passive(PassiveId::try_new(id).expect("passive id"))
}

fn passive_actor(
    id: u64,
    position: [f32; 3],
    yaw: f32,
    health: u8,
    lifecycle: ActorLifecycle,
) -> ActorRecord {
    ActorRecord::try_new(
        passive_key(id),
        lifecycle,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("position"),
            velocity: FiniteVec3::try_new([0.0; 3]).expect("velocity"),
            on_ground: true,
        }),
        LookAngles::try_new(yaw, 0.0).expect("look"),
        SurvivalState::try_new(SurvivalStateParts {
            health,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .expect("survival"),
        ActorBody::Passive(PassiveMob {
            id,
            dimension: 0,
            position,
            velocity: [0.0; 3],
            on_ground: true,
            yaw,
            health,
        }),
    )
    .expect("passive actor")
}

fn passive_runtime(id: u64, aux: ActorAux) -> ActorRuntime {
    ActorRuntime {
        key: passive_key(id),
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
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
        aux,
    }
}

fn passive_aux(
    home: BlockPos,
    graze_ticks: u16,
    graze_at: Option<BlockPos>,
    fresh: bool,
) -> ActorAux {
    ActorAux::Passive {
        home,
        flee_ticks: 0,
        flee_from: None,
        graze_ticks,
        graze_at,
        fresh,
    }
}

fn wheat_inventory(count: u8) -> InventoryRecord {
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = ItemStack {
        item: ITEM_WHEAT,
        count,
        durability: 0,
    };
    inventory
}

fn passive_call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::PassiveStepDeaths,
        actor: None,
        command: None,
        internal: None,
    }
}

fn aux_of(context: &TickContext<'_>, id: u64) -> ActorAux {
    context
        .read()
        .runtime(passive_key(id))
        .expect("runtime")
        .aux
        .clone()
}

fn active_passive_ids(context: &TickContext<'_>) -> Vec<u64> {
    context
        .read()
        .actors()
        .iter()
        .filter(|actor| {
            matches!(actor.key, ActorKey::Passive(_)) && actor.lifecycle == ActorLifecycle::Active
        })
        .map(|actor| match actor.key {
            ActorKey::Passive(id) => id.get(),
            _ => unreachable!("filtered above"),
        })
        .collect()
}

fn horizontal_dist_sq(from: [f32; 3], to: [f32; 3]) -> f32 {
    let dx = to[0] - from[0];
    let dz = to[2] - from[2];
    dx * dx + dz * dz
}

// ---------------------------------------------------------------------
// Cited integer mirrors. Each replays a Go row verbatim; the known-answer
// assertions inside the tests pin them before any scene leans on them.
// ---------------------------------------------------------------------

/// `Sampler.SplitMix64` (`packages/server/updates/sampler.go`).
fn splitmix64(x: u64) -> u64 {
    let mut x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// `Sampler.PassiveGrazeHit` (`packages/server/updates/sampler.go`).
fn graze_hit(seed: i64, tick: u64, id: u64) -> bool {
    let hash = splitmix64(splitmix64((seed as u64) ^ GRAZE_SALT) ^ tick);
    splitmix64(hash ^ id).is_multiple_of(GRAZE_PERIOD)
}

/// `Sampler.HostileCandidateHash` (`packages/server/updates/sampler.go`), the
/// candidate-id chain the passive spawn reuses wholesale.
fn candidate_hash(seed: i64, tick: u64, x: i32, y: i32, z: i32) -> u64 {
    let hash = splitmix64(splitmix64((seed as u64) ^ tick) ^ (x as u32 as u64) ^ (z as u32 as u64));
    splitmix64(hash ^ (y as u32 as u64))
}

/// `hostileSpawnColumn` (`packages/server/sim/entity/hostile_spawn.go`): the
/// integer-derived radius 24..48 and axis the passive spawn shares.
fn spawn_column(base: u64, anchor_x: i32, anchor_z: i32) -> (i32, i32) {
    let radius = 24i64 + (base % 25) as i64;
    let axis = ((base >> 32) & 3) as usize;
    let deltas = [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)];
    let (dx, dz) = deltas[axis];
    (
        anchor_x + (dx * radius) as i32,
        anchor_z + (dz * radius) as i32,
    )
}

/// `normalizeYaw` (`packages/server/sim/entity/placement.go`).
fn normalize_yaw(yaw: f32) -> f32 {
    let mut normalized = (f64::from(yaw) + std::f64::consts::PI) % (2.0 * std::f64::consts::PI);
    if normalized < 0.0 {
        normalized += 2.0 * std::f64::consts::PI;
    }
    (normalized - std::f64::consts::PI) as f32
}

/// `turnYawToward` (`packages/server/sim/entity/passive.go`).
fn turn_yaw_toward(current: f32, want: f32, max_step: f32) -> f32 {
    let delta = normalize_yaw(want - current);
    if delta > max_step {
        return normalize_yaw(current + max_step);
    }
    if delta < -max_step {
        return normalize_yaw(current - max_step);
    }
    want
}

/// The chase yaw toward a point: `atan2(-dx, -dz)` narrowed exactly like the
/// Go rows (`passiveStepInput`, `passiveTemptTarget`).
fn yaw_to_point(dx: f32, dz: f32) -> f32 {
    normalize_yaw((f64::from(-dx)).atan2(f64::from(-dz)) as f32)
}

/// The segment wander heading (`passiveStepInput` wander arm): the splitmix
/// fold over (seed, segment, id) narrowed through the 24-bit angle table.
fn wander_want_yaw(seed: i64, segment: u64, id: u64) -> f32 {
    let base = splitmix64((seed as u64) ^ segment ^ id);
    normalize_yaw((base & 0xFF_FFFF) as f32 * ((2.0 * std::f64::consts::PI / 16_777_216.0) as f32))
}

#[test]
fn wander_matches_go_heading_and_carried_shortest_arc() {
    // Independently executed Go `Sampler.SplitMix64` and `passiveStepInput`
    // scalar known answers at seed 0, id 41. These expectations do not use
    // the replay mirror above; each later call carries the provider's output.
    let mut state = authority();
    let mut actor = passive_actor(
        41,
        [0.5, 1.0, 0.5],
        f32::from_bits(0x3fbb_d2f9),
        20,
        ActorLifecycle::Active,
    );
    let mut runtime = passive_runtime(41, passive_aux(BlockPos::new(0, 1, 0), 0, None, false));
    for (tick, yaw_bits) in [(0, 0x3fbb_d2f9), (40, 0x3fd5_6c93), (80, 0x3fef_062d)] {
        while state.next_tick() < tick {
            state.advance_tick(TickBudget::full()).expect("empty tick");
        }
        let mut context = harness_context(&mut state);
        context
            .stage(RuleEffect::Environment(environment(0)))
            .expect("environment");
        stage_room(&mut context, -4, 4, -4, 4, 4);
        // Dirt excludes grazing; no players excludes temptation and idle look.
        for x in -4..=4 {
            for z in -4..=4 {
                context.preload_block(observation(BlockPos::new(x, 0, z), DIRT));
            }
        }
        context
            .stage(RuleEffect::Actor(actor.clone()))
            .expect("carried actor");
        context
            .stage(RuleEffect::Runtime(runtime.clone()))
            .expect("carried runtime");
        provider::run(&mut context, passive_call()).expect("wander");
        let next_actor = context
            .read()
            .actor(passive_key(41))
            .expect("actor")
            .clone();
        let next_runtime = context
            .read()
            .runtime(passive_key(41))
            .expect("runtime")
            .clone();
        assert_eq!(next_actor.look.yaw().to_bits(), yaw_bits, "tick {tick}");
        let ActorBody::Passive(body) = &next_actor.body else {
            panic!("passive body");
        };
        assert_eq!(body.yaw.to_bits(), yaw_bits, "body tick {tick}");
        assert_eq!(body.position, next_actor.motion.position().get());
        assert_eq!(body.velocity, next_actor.motion.velocity().get());
        assert_eq!(body.on_ground, next_actor.motion.on_ground());
        assert_eq!(body.health, actor.survival.health());
        assert_eq!(next_actor.key, actor.key);
        assert_eq!(next_actor.lifecycle, actor.lifecycle);
        assert_eq!(next_actor.dimension, actor.dimension);
        assert_eq!(next_actor.survival, actor.survival);
        assert_eq!(next_actor.look.pitch(), actor.look.pitch());
        let distance = horizontal_dist_sq(body.position, actor.motion.position().get());
        assert!(distance > 0.0 && distance < 1.0, "bounded real motion");
        assert_eq!(next_runtime, runtime, "neutral runtime tick {tick}");
        actor = next_actor;
        runtime = next_runtime;
    }
}

fn assert_home_step(
    home: BlockPos,
    position: [f32; 3],
    axis: usize,
    direction: i32,
    chunk: Option<i32>,
) {
    let yaw = match (axis, direction) {
        (0, 1) => -1.5,
        (0, -1) => 1.5,
        (2, 1) => 3.0,
        (2, -1) => -0.1,
        _ => panic!("horizontal direction"),
    };
    let actor = passive_actor(41, position, yaw, 20, ActorLifecycle::Active);
    let runtime = passive_runtime(41, passive_aux(home, 0, None, false));
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(0)))
        .expect("environment");
    let x = position[0].floor() as i32;
    let z = position[2].floor() as i32;
    stage_room(&mut context, x - 4, x + 4, z - 4, z + 4, 4);
    for x in x - 4..=x + 4 {
        for z in z - 4..=z + 4 {
            context.preload_block(observation(BlockPos::new(x, 0, z), DIRT));
        }
    }
    context
        .stage(RuleEffect::Actor(actor.clone()))
        .expect("actor");
    context
        .stage(RuleEffect::Runtime(runtime.clone()))
        .expect("runtime");
    provider::run(&mut context, passive_call()).expect("home movement");
    let next = context
        .read()
        .actor(passive_key(41))
        .expect("actor")
        .clone();
    assert_eq!(context.read().runtime(passive_key(41)), Some(&runtime));
    if let Some(chunk) = chunk {
        let next_position = next.motion.position().get();
        assert!(
            (next_position[axis] - position[axis]) * direction as f32 > 0.0,
            "accepted motion from {position:?}, home {home:?}"
        );
        assert!(horizontal_dist_sq(next_position, position) < 1.0);
        assert_eq!((next_position[axis].floor() as i32) >> 4, chunk);
        assert_eq!(next.key, actor.key);
        assert_eq!(next.lifecycle, actor.lifecycle);
        assert_eq!(next.dimension, actor.dimension);
        assert_eq!(next.survival, actor.survival);
        assert_eq!(next.look.pitch(), actor.look.pitch());
        let ActorBody::Passive(body) = &next.body else {
            panic!("passive body");
        };
        assert_eq!(body.position, next_position);
        assert_eq!(body.velocity, next.motion.velocity().get());
        assert_eq!(body.on_ground, next.motion.on_ground());
        assert_eq!(body.yaw, next.look.yaw());
        assert_eq!(body.health, actor.survival.health());
    } else {
        assert_eq!(next, actor, "rollback from {position:?}, home {home:?}");
        assert_eq!(next.motion.velocity().get(), [0.0; 3]);
    }
}

fn assert_home_axis(axis: usize) {
    // Go `outsideHomeNeighborhood` permits a Chebyshev chunk distance of
    // one. Signed floor/shift gives birth block -1 the birth chunk -1.
    for (home_axis, current_axis, direction, chunk) in [
        (0, 1.99, 1, Some(0)),
        (0, 15.99, 1, Some(1)),
        (0, 31.99, 1, None),
        (0, -0.01, -1, Some(-1)),
        (0, -15.99, -1, None),
        (-1, -15.99, -1, Some(-2)),
        (-1, -31.99, -1, None),
        (-1, -0.01, 1, Some(0)),
        (-1, 15.99, 1, None),
    ] {
        let mut home = [0, 1, 0];
        home[axis] = home_axis;
        let mut position = [0.5, 1.0, 0.5];
        position[axis] = current_axis;
        assert_home_step(
            BlockPos::new(home[0], home[1], home[2]),
            position,
            axis,
            direction,
            chunk,
        );
    }
}

#[test]
fn home_neighborhood_x_uses_signed_chunks() {
    assert_home_axis(0);
}

#[test]
fn home_neighborhood_z_uses_signed_chunks() {
    assert_home_axis(2);
}

fn assert_far_home_axis(axis: usize, home_axis: i32, current_axis: f32, direction: i32) {
    // Extreme saved home coordinates do not require an extreme physics grid:
    // ordinary resident positions must safely roll back outside their home.
    let mut home = [0, 1, 0];
    home[axis] = home_axis;
    let mut position = [0.5, 1.0, 0.5];
    position[axis] = current_axis;
    assert_home_step(
        BlockPos::new(home[0], home[1], home[2]),
        position,
        axis,
        direction,
        None,
    );
}

#[test]
fn minimum_home_x_rolls_back_without_overflow() {
    assert_far_home_axis(0, i32::MIN, 2.5, 1);
}

#[test]
fn maximum_home_x_rolls_back_without_overflow() {
    assert_far_home_axis(0, i32::MAX, -2.5, -1);
}

#[test]
fn minimum_home_z_rolls_back_without_overflow() {
    assert_far_home_axis(2, i32::MIN, 2.5, 1);
}

#[test]
fn maximum_home_z_rolls_back_without_overflow() {
    assert_far_home_axis(2, i32::MAX, -2.5, -1);
}

/// Scene configuration for one native motion guard: the starting ground
/// contact, a stationary variant, the support block under the room floor and
/// the foot block present before the pursuit runs.
struct SnowScene {
    grounded: bool,
    stationary: bool,
    support: u16,
    initial_foot: u16,
}

/// Thick Snow (87 and 88) under the cow's foot cell consumes the shared snow
/// tuning inside the actual native motion pass while a real wheat holder
/// holds the pursuit heading; the scene controls vary the ground contact,
/// holder distance, support and staged foot blocks.
fn assert_motion_snow_case(scene: SnowScene, foot: u16, want_z: f32, want_vz: f32) {
    // The fixture cow id misses the tick graze roll, so the scene stays free
    // of a grazing freeze and the pursued heading is temptation alone.
    let cow = first_non_hit_id(0, 0, 21);
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(1)))
        .expect("environment");
    stage_room(&mut context, 0, 5, -7, 5, 3);
    let foot_cell = BlockPos::new(2, 1, 2);
    let support_cell = BlockPos::new(2, 0, 2);
    if scene.support != GRASS {
        context.preload_block(observation(support_cell, scene.support));
    }
    if scene.initial_foot != AIR {
        context.preload_block(observation(foot_cell, scene.initial_foot));
    }
    // A requested foot that differs from the staged one rewrites the same
    // cell through the live support transaction.
    if foot != scene.initial_foot {
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
    // The cow starts at the scene's ground contact and speed; the motion
    // state and the mirrored passive body stay consistent before staging.
    let speed = if scene.stationary { 0.0 } else { -4.3 };
    let mut prepared = passive_actor(cow, [2.5, 1.0, 2.5], 0.0, 20, ActorLifecycle::Active);
    prepared.motion = MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new([2.5, 1.0, 2.5]).expect("position"),
        velocity: FiniteVec3::try_new([0.0, 0.0, speed]).expect("velocity"),
        on_ground: scene.grounded,
    });
    {
        let ActorBody::Passive(body) = &mut prepared.body else {
            panic!("passive body");
        };
        body.velocity = [0.0, 0.0, speed];
        body.on_ground = scene.grounded;
    }
    context
        .stage(RuleEffect::Actor(prepared.clone()))
        .expect("cow");
    context
        .stage(RuleEffect::Runtime(passive_runtime(
            cow,
            passive_aux(BlockPos::new(2, 1, 2), 0, None, false),
        )))
        .expect("cow runtime");
    // Wheat held exactly 8 blocks ahead on -z tempts the cow straight at the
    // holder (move_z 1, yaw 0), so the tick is real temptation plus native
    // physics with no double step; the stationary guard parks the holder
    // inside the 2.5 stop distance for a genuine zero intent.
    let holder = if scene.stationary {
        [2.5, 1.0, 1.5]
    } else {
        [2.5, 1.0, -5.5]
    };
    context
        .stage(RuleEffect::Actor(player_actor(session, holder)))
        .expect("player");
    context.preload_inventory(ActorKey::Player(session), wheat_inventory(5));
    let runtime = context.read().runtime(passive_key(cow)).cloned();
    let environment = context.read().environment().cloned();
    let player = context
        .read()
        .actors()
        .iter()
        .find(|actor| matches!(actor.key, ActorKey::Player(_)))
        .cloned()
        .expect("player record");
    let inventory = context.read().inventory(ActorKey::Player(session)).cloned();
    let foot_observed = context.read().observation(Dimension::OVERWORLD, foot_cell);
    let support_observed = context
        .read()
        .observation(Dimension::OVERWORLD, support_cell);
    let events = context.events().to_vec();

    provider::run(&mut context, passive_call()).expect("snow motion tick");

    let next = context.read().actor(passive_key(cow)).expect("cow").clone();
    let position = next.motion.position().get();
    let velocity = next.motion.velocity().get();
    assert_eq!(position[0], 2.5, "foot {foot}");
    // The support variations replace only the center floor cell; the
    // surrounding grass floor keeps the body in contact (y stays 1), so the
    // decisive pin is the unslowed horizontal step over the AIR foot cell.
    assert_eq!(position[1], 1.0, "foot {foot}");
    assert!(next.motion.on_ground());
    assert!(
        (position[2] - want_z).abs() < 1e-5,
        "foot {foot}: z {}",
        position[2]
    );
    assert!(
        (velocity[2] - want_vz).abs() < 1e-6,
        "foot {foot}: vz {}",
        velocity[2]
    );
    assert_eq!(next.look.yaw(), 0.0);
    assert_eq!(next.key, prepared.key);
    assert_eq!(next.lifecycle, ActorLifecycle::Active);
    assert_eq!(next.survival, prepared.survival);
    let ActorBody::Passive(body) = &next.body else {
        panic!("passive body");
    };
    let ActorBody::Passive(want) = &prepared.body else {
        panic!("passive body");
    };
    assert_eq!(body.id, want.id);
    assert_eq!(body.health, want.health);
    assert_eq!(body.position, position);
    assert_eq!(body.velocity, velocity);
    assert_eq!(body.on_ground, next.motion.on_ground());
    assert_eq!(body.yaw, 0.0);
    assert_eq!(context.read().runtime(passive_key(cow)), runtime.as_ref());
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
    assert_eq!(
        context.read().inventory(ActorKey::Player(session)).cloned(),
        inventory,
        "temptation consumes no wheat"
    );
    assert_eq!(context.read().environment(), environment.as_ref());
    assert_eq!(
        context.read().observation(Dimension::OVERWORLD, foot_cell),
        foot_observed,
        "the borrowed foot cell is unchanged"
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, support_cell),
        support_observed,
        "the support cell is unchanged"
    );
    assert_eq!(context.events(), events);
}

/// The grounded wrapper: a grass floor and an air foot cell before the final
/// foot write.
fn assert_motion_snow_native_displacement(foot: u16, want_z: f32, want_vz: f32) {
    assert_motion_snow_case(
        SnowScene {
            grounded: true,
            stationary: false,
            support: GRASS,
            initial_foot: AIR,
        },
        foot,
        want_z,
        want_vz,
    );
}

/// Thick Snow (87 and 88) under the cow's foot consumes the shared snow
/// tuning inside the actual native motion pass; the thin Snow controls
/// (85 and 86), the AIR control scene, airborne starts, stationary intents,
/// removed supports and same-tick foot rewrites keep the uncut walk speed or
/// hold the start pose.
#[test]
fn motion_snow_thick_native_displacement() {
    for (foot, want_z, want_vz) in [
        (85, 2.285, -4.3),
        (86, 2.285, -4.3),
        (87, 2.349_5, -3.01),
        (88, 2.349_5, -3.01),
        (AIR, 2.285, -4.3),
    ] {
        assert_motion_snow_native_displacement(foot, want_z, want_vz);
    }
    // An airborne start ignores the foot block until it lands, and a holder
    // inside the stop distance holds the start pose.
    for foot in [AIR, 87, 88] {
        assert_motion_snow_case(
            SnowScene {
                grounded: false,
                stationary: false,
                support: GRASS,
                initial_foot: AIR,
            },
            foot,
            2.285,
            -4.3,
        );
        assert_motion_snow_case(
            SnowScene {
                grounded: true,
                stationary: true,
                support: GRASS,
                initial_foot: AIR,
            },
            foot,
            2.5,
            0.0,
        );
    }
    // Center-only support variations (air or walk-through snow) keep the
    // surrounding floor in contact; the unslowed horizontal pin over the AIR
    // foot cell stays decisive.
    for support in [AIR, 87, 88] {
        assert_motion_snow_case(
            SnowScene {
                grounded: true,
                stationary: false,
                support,
                initial_foot: AIR,
            },
            AIR,
            2.285,
            -4.3,
        );
    }
    // Same-tick foot rewrites through the support transaction: clearing thick
    // Snow keeps the uncut speed, writing it mid-tick consumes the tuning.
    assert_motion_snow_case(
        SnowScene {
            grounded: true,
            stationary: false,
            support: GRASS,
            initial_foot: 87,
        },
        AIR,
        2.285,
        -4.3,
    );
    assert_motion_snow_case(
        SnowScene {
            grounded: true,
            stationary: false,
            support: GRASS,
            initial_foot: AIR,
        },
        88,
        2.349_5,
        -3.01,
    );
}

// The isolated tests each qualify one foot cell directly, so a negative
// control shows the snow scaling before the first failure stops the
// composite fixture.
#[test]
fn motion_snow_isolated_87() {
    assert_motion_snow_native_displacement(87, 2.349_5, -3.01);
}

#[test]
fn motion_snow_isolated_88() {
    assert_motion_snow_native_displacement(88, 2.349_5, -3.01);
}

fn first_hit_id(seed: i64, tick: u64, from: u64) -> u64 {
    (from..)
        .find(|&id| graze_hit(seed, tick, id))
        .expect("a graze hit exists within the scan window")
}

fn first_non_hit_id(seed: i64, tick: u64, from: u64) -> u64 {
    (from..)
        .find(|&id| !graze_hit(seed, tick, id))
        .expect("a non-hit id exists within the scan window")
}

/// Temptation and grazing: the wheat radius includes exactly 8 blocks and one
/// step closes distance, the 2.5 stop distance freezes displacement while the
/// 0.2 turn converges, and the graze run settles its trigger cell to dirt
/// through the transaction on the event's twentieth tick while an unready cell
/// never writes.
#[test]
fn tempt_8_stop_2_5_and_graze20() {
    // Known-answer pins for the cited roll mirror
    // (`TestSamplerPassiveGrazeHitKAT`).
    assert!(!graze_hit(0, 0, 41));
    assert!(!graze_hit(7, 600, 0));
    assert!(graze_hit(0, 103, 41));
    assert!(graze_hit(0, 180, 777));

    // Radius boundary: wheat held at exactly 8 blocks is inside the radius
    // and the first step closes distance (`TestPassiveTemptRadiusBoundary`).
    // The fixture cow id neither hits the tick graze roll (the scene stays
    // free of a grazing freeze) nor wanders toward the holder (its segment
    // heading has a negative x component), so only genuine pursuit closes
    // distance and the boundary stays discriminating.
    let cow = (21u64..)
        .find(|&id| !graze_hit(0, 0, id) && f64::from(wander_want_yaw(0, 0, id)).sin() > 0.0)
        .expect("a fixture id exists");
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(1)))
        .expect("environment");
    stage_room(&mut context, 0, 13, 0, 5, 3);
    context
        .stage(RuleEffect::Actor(passive_actor(
            cow,
            [2.5, 1.0, 2.5],
            0.0,
            20,
            ActorLifecycle::Active,
        )))
        .expect("cow");
    context
        .stage(RuleEffect::Runtime(passive_runtime(
            cow,
            passive_aux(BlockPos::new(2, 1, 2), 0, None, false),
        )))
        .expect("cow runtime");
    context
        .stage(RuleEffect::Actor(player_actor(session, [10.5, 1.0, 2.5])))
        .expect("player");
    context.preload_inventory(ActorKey::Player(session), wheat_inventory(5));
    let call = passive_call();
    provider::run(&mut context, call).expect("tempt advance");
    let view = context.read();
    let cow_record = view.actor(passive_key(cow)).expect("cow");
    let after = horizontal_dist_sq(cow_record.motion.position().get(), [10.5, 1.0, 2.5]);
    assert!(after < 64.0, "wheat at exactly 8 blocks closes distance");
    // Pursuit keeps closing on the holder over sustained ticks
    // (`TestPassiveTemptFollowsWheatHolderAndStops`); the fixture's wander
    // heading points away, so sustained closure is temptation alone.
    for _ in 0..10 {
        provider::run(&mut context, call).expect("pursuit advance");
    }
    let view = context.read();
    let cow_record = view.actor(passive_key(cow)).expect("cow");
    let settled = horizontal_dist_sq(cow_record.motion.position().get(), [10.5, 1.0, 2.5]);
    assert!(
        settled < 64.0 - 4.0,
        "sustained pursuit at the inclusive boundary closes at least two blocks"
    );
    assert_eq!(
        view.inventory(ActorKey::Player(session))
            .expect("inventory")
            .slots[0]
            .count,
        5,
        "temptation consumes no wheat"
    );
    assert_eq!(
        active_passive_ids(&context),
        vec![cow],
        "temptation does not multiply residents"
    );

    // Stop distance: with the holder at exactly 2.5 blocks the cow freezes in
    // place while the bounded turn converges on the holder
    // (`TestPassiveTemptStopsExactlyAtTwoAndHalf`,
    // `TestPassiveTemptStoppedCowTurnsToFacePlayer`). The white-box restage
    // mirrors `placeSessionPlayer` and the restored cow fixture.
    context
        .stage(RuleEffect::Actor(passive_actor(
            cow,
            [2.5, 1.0, 2.5],
            0.0,
            20,
            ActorLifecycle::Active,
        )))
        .expect("cow reset");
    context
        .stage(RuleEffect::Runtime(passive_runtime(
            cow,
            passive_aux(BlockPos::new(2, 1, 2), 0, None, false),
        )))
        .expect("cow runtime reset");
    context
        .stage(RuleEffect::Actor(player_actor(session, [5.0, 1.0, 2.5])))
        .expect("holder moved to exactly 2.5");
    let want = yaw_to_point(2.5, 0.0);
    provider::run(&mut context, call).expect("stop advance");
    let view = context.read();
    let cow_record = view.actor(passive_key(cow)).expect("cow");
    assert_eq!(
        cow_record.motion.position().get(),
        [2.5, 1.0, 2.5],
        "the stop distance freezes displacement at zero approach velocity"
    );
    let turned = turn_yaw_toward(0.0, want, 0.2);
    assert_ne!(turned, 0.0, "the stopped cow still turns toward the holder");
    assert_eq!(cow_record.look.yaw(), turned, "the turn is bounded by 0.2");
    let ActorBody::Passive(body) = &cow_record.body else {
        panic!("passive body")
    };
    assert_eq!(body.yaw, turned, "body yaw mirrors the facing");
    for _ in 0..7 {
        provider::run(&mut context, call).expect("turn advance");
    }
    let view = context.read();
    let cow_record = view.actor(passive_key(cow)).expect("cow");
    assert_eq!(
        cow_record.look.yaw(),
        want,
        "the bounded turn converges exactly on the holder"
    );
    assert_eq!(
        cow_record.motion.position().get(),
        [2.5, 1.0, 2.5],
        "convergence never displaces the stopped cow"
    );
    for _ in 0..2 {
        provider::run(&mut context, call).expect("idle advance");
    }
    let view = context.read();
    assert_eq!(
        view.actor(passive_key(cow))
            .expect("cow")
            .motion
            .position()
            .get(),
        [2.5, 1.0, 2.5],
        "the stopped cow stays put"
    );

    // Grazing: the frozen 1-in-600 roll opens a 20-tick event on the trigger
    // tick with the cell recorded, the pose frozen and the cell untouched
    // until the twentieth tick settles grass to dirt through the
    // observed-basis transaction
    // (`TestPassiveGrazeTurnsGrassToDirtAfterTwentyTicks`).
    let grazer = first_hit_id(0, 0, 11);
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(1)))
        .expect("environment");
    stage_room(&mut context, 0, 5, 0, 5, 2);
    context
        .stage(RuleEffect::Actor(passive_actor(
            grazer,
            [2.5, 1.0, 2.5],
            0.0,
            20,
            ActorLifecycle::Active,
        )))
        .expect("grazer");
    context
        .stage(RuleEffect::Runtime(passive_runtime(
            grazer,
            passive_aux(BlockPos::new(2, 1, 2), 0, None, false),
        )))
        .expect("grazer runtime");
    provider::run(&mut context, call).expect("trigger advance");
    let ActorAux::Passive {
        graze_ticks,
        graze_at,
        ..
    } = aux_of(&context, grazer)
    else {
        panic!("passive aux")
    };
    assert_eq!(graze_ticks, 19, "the trigger tick counts as event tick one");
    let trigger = graze_at.expect("trigger cell");
    let view = context.read();
    let observed = view
        .observation(Dimension::OVERWORLD, trigger)
        .expect("ready trigger cell");
    assert_eq!(observed.block, GRASS, "the event opens on standing grass");
    let frozen = view.actor(passive_key(grazer)).expect("grazer").motion;
    for remaining in (1..=18).rev() {
        provider::run(&mut context, call).expect("event advance");
        let ActorAux::Passive {
            graze_ticks,
            graze_at,
            ..
        } = aux_of(&context, grazer)
        else {
            panic!("passive aux")
        };
        assert_eq!(
            graze_ticks, remaining,
            "the event counts down once per tick"
        );
        assert_eq!(graze_at, Some(trigger), "the trigger cell stays recorded");
        let view = context.read();
        assert_eq!(
            view.actor(passive_key(grazer)).expect("grazer").motion,
            frozen,
            "grazing freezes the pose"
        );
        assert_eq!(
            view.observation(Dimension::OVERWORLD, trigger)
                .expect("ready cell")
                .block,
            GRASS,
            "settlement writes nothing before the twentieth tick"
        );
    }
    // Tick twenty settles: snapshot the staged floor first so the write is
    // proven to be exactly one cell through the transaction.
    let mut before = Vec::new();
    let view = context.read();
    for x in 0..=5 {
        for z in 0..=5 {
            for y in 0..=2 {
                before.push((
                    BlockPos::new(x, y, z),
                    view.observation(Dimension::OVERWORLD, BlockPos::new(x, y, z)),
                ));
            }
        }
    }
    provider::run(&mut context, call).expect("settle advance");
    let ActorAux::Passive {
        graze_ticks,
        graze_at,
        ..
    } = aux_of(&context, grazer)
    else {
        panic!("passive aux")
    };
    assert_eq!(
        (graze_ticks, graze_at),
        (0, None),
        "settlement clears the transient event"
    );
    let view = context.read();
    let settled = view
        .observation(Dimension::OVERWORLD, trigger)
        .expect("settled cell");
    assert_eq!(settled.block, DIRT, "tick twenty settles grass to dirt");
    assert_eq!(
        settled.revision,
        observed.revision + 1,
        "the write advances the observed basis exactly once"
    );
    let mut writes = Vec::new();
    for (pos, old) in before {
        let now = view.observation(Dimension::OVERWORLD, pos);
        if now != old {
            writes.push(pos);
        }
    }
    assert_eq!(
        writes,
        vec![trigger],
        "the event writes exactly its trigger cell"
    );
    assert!(
        matches!(
            view.actor(passive_key(grazer)).expect("grazer").lifecycle,
            ActorLifecycle::Active
        ),
        "grazing does not remove the cow"
    );

    // An unready trigger cell ends the event without writing
    // (`TestPassiveGrazeUnloadedChunkSettlesNothing`): the white-box aux
    // mirrors the Go fixture that stages the event's final tick over an
    // unloaded column.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(1)))
        .expect("environment");
    let far = BlockPos::new(1000, 0, 1000);
    context
        .stage(RuleEffect::Actor(passive_actor(
            16,
            [1000.5, 1.0, 1000.5],
            0.0,
            20,
            ActorLifecycle::Active,
        )))
        .expect("cow over an unloaded column");
    context
        .stage(RuleEffect::Runtime(passive_runtime(
            16,
            passive_aux(BlockPos::new(1000, 1, 1000), 1, Some(far), false),
        )))
        .expect("event tail");
    provider::run(&mut context, call).expect("discard advance");
    let ActorAux::Passive {
        graze_ticks,
        graze_at,
        ..
    } = aux_of(&context, 16)
    else {
        panic!("passive aux")
    };
    assert_eq!(
        (graze_ticks, graze_at),
        (0, None),
        "the unready cell ends the event"
    );
    assert_eq!(
        context.read().observation(Dimension::OVERWORLD, far),
        None,
        "an unready cell never writes"
    );
    assert!(
        matches!(
            context
                .read()
                .actor(passive_key(16))
                .expect("cow")
                .lifecycle,
            ActorLifecycle::Active
        ),
        "the discarded settlement keeps the cow resident"
    );
}

/// Spawn admission and transient discipline: a clean set spawns the derived
/// candidate whose newborn skips its first movement tick, the thirty-third
/// resident is refused at the global cap while a dead record stays dead, and a
/// restored save body is admitted with the frozen zeroed transient defaults.
///
/// The case keeps the brief's token sequence as a suffix because a Rust
/// identifier cannot begin with a digit.
#[test]
fn resident_32_33_and_restore_transient() {
    // Known-answer pins for the cited candidate chain
    // (`TestSamplerHostileCandidateHashKAT`).
    assert_eq!(candidate_hash(0, 13001, 24, 1, 0), 0x8bcd_14a8_d5cf_3d91);
    assert_eq!(
        candidate_hash(-42, 23000, -24, 64, -48),
        0xedf4_e302_35ad_740a
    );

    // A clean resident set spawns the derived candidate
    // (`TestPassiveSpawnReplayIsDeterministic` fixture shape): the candidate
    // column follows the shared integer derivation and the newborn's exact
    // pose pins the fresh-skip convention.
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(1)))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(session, [0.5, 1.0, 0.5])))
        .expect("anchor");
    let (column_x, column_z) = spawn_column(splitmix64(1), 0, 0);
    stage_candidate_triple(&mut context, column_x, column_z);
    let expected_id = candidate_hash(0, 1, column_x, 1, column_z);
    assert_ne!(expected_id, 0, "the derived candidate id is nonzero");
    let call = passive_call();
    let report = provider::run(&mut context, call).expect("spawn advance");
    assert_eq!(
        active_passive_ids(&context),
        vec![expected_id],
        "one candidate spawns with the derived id"
    );
    let view = context.read();
    let spawned = view.actor(passive_key(expected_id)).expect("spawned");
    assert_eq!(
        spawned.motion.position().get(),
        [column_x as f32 + 0.5, 1.0, column_z as f32 + 0.5],
        "the newborn skipped its first movement tick"
    );
    let ActorAux::Passive { fresh, .. } = aux_of(&context, expected_id) else {
        panic!("passive aux")
    };
    assert!(!fresh, "the skip flag clears with the batch");
    assert_eq!(
        report.examined, 3,
        "candidate plus both passes over one resident"
    );
    assert_eq!(report.rejected, 0, "a clean set refuses nothing");

    // The thirty-third resident is refused at the global cap while a dead
    // record stays dead and unexamined
    // (`TestPassiveSpawnRejectsAtGlobalCap`). The residents sit far beyond the
    // 48-block near radius, so only the global cap can refuse.
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(1)))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(session, [0.5, 1.0, 0.5])))
        .expect("anchor");
    let (column_x, column_z) = spawn_column(splitmix64(1), 0, 0);
    stage_candidate_triple(&mut context, column_x, column_z);
    for id in 1..=32u64 {
        let position = [1000.0 + id as f32, 1.0, 0.5];
        context
            .stage(RuleEffect::Actor(passive_actor(
                id,
                position,
                0.0,
                20,
                ActorLifecycle::Active,
            )))
            .expect("resident");
        context
            .stage(RuleEffect::Runtime(passive_runtime(
                id,
                passive_aux(BlockPos::new(1000 + id as i32, 1, 0), 0, None, false),
            )))
            .expect("resident runtime");
    }
    context
        .stage(RuleEffect::Actor(passive_actor(
            40,
            [2000.5, 1.0, 0.5],
            0.0,
            0,
            ActorLifecycle::Dead,
        )))
        .expect("dead record");
    let dead_before = context.read().actor(passive_key(40)).cloned();
    let report = provider::run(&mut context, call).expect("capped advance");
    assert_eq!(
        active_passive_ids(&context),
        (1..=32).collect::<Vec<u64>>(),
        "the 33rd resident is refused at the global cap"
    );
    assert!(
        !active_passive_ids(&context).contains(&expected_id),
        "no candidate spawns at the cap"
    );
    let view = context.read();
    assert_eq!(
        view.actor(passive_key(40)),
        dead_before.as_ref(),
        "a dead record is not resurrected, moved or re-settled"
    );
    assert_eq!(
        report.examined, 64,
        "thirty-two residents examined by both passes, no candidate derived"
    );
    assert_eq!(
        report.rejected, 0,
        "the cap short-circuits before derivation"
    );

    // Restore: a save-body-only resident is admitted with the frozen zeroed
    // transient defaults re-anchored at the loaded position
    // (`RestorePassive`, `TestPassiveGrazeTransientAcrossRestart`). The save
    // body owns no transient lane, so nothing can resurrect an event.
    let restored = first_non_hit_id(0, 0, 31);
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(1)))
        .expect("environment");
    stage_room(&mut context, 1, 4, 1, 4, 2);
    context
        .stage(RuleEffect::Actor(passive_actor(
            restored,
            [2.5, 1.0, 2.5],
            0.0,
            20,
            ActorLifecycle::Active,
        )))
        .expect("restored body");
    let report = provider::run(&mut context, call).expect("restore advance");
    let ActorAux::Passive {
        home,
        flee_ticks,
        flee_from,
        graze_ticks,
        graze_at,
        fresh,
    } = aux_of(&context, restored)
    else {
        panic!("passive aux")
    };
    assert_eq!(
        (flee_ticks, flee_from, graze_ticks, graze_at, fresh),
        (0, None, 0, None, false),
        "restore resets the transients to the frozen defaults"
    );
    assert_eq!(
        home,
        BlockPos::new(2, 1, 2),
        "restore re-anchors home at the loaded position"
    );
    assert!(
        matches!(
            context
                .read()
                .actor(passive_key(restored))
                .expect("cow")
                .lifecycle,
            ActorLifecycle::Active
        ),
        "a restored resident stays resident"
    );
    assert_ne!(
        context
            .read()
            .actor(passive_key(restored))
            .expect("cow")
            .motion
            .position()
            .get(),
        [2.5, 1.0, 2.5],
        "restore seeds no fresh skip: the restored resident integrates at once"
    );
    assert_eq!(report.examined, 2, "one resident examined by both passes");
    assert_eq!(report.rejected, 0, "restore refuses nothing");
    let view = context.read();
    let ActorBody::Passive(body) = &view.actor(passive_key(restored)).expect("body").body else {
        panic!("passive body")
    };
    assert_eq!(
        (body.id, body.health),
        (restored, 20),
        "the save body keeps only persisted facts"
    );
}

// ---------------------------------------------------------------------
// Passive death loot (`dropPassiveLoot`, `packages/server/sim/entity/passive.go`).
// ---------------------------------------------------------------------

/// Raw beef (`core.ItemRawBeef`, `packages/shared/core/item.go`): the fixed
/// passive death batch.
const ITEM_RAW_BEEF: u16 = 53;

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

fn preload_empty(context: &mut TickContext<'_>, x: i32, z: i32) {
    context.preload_ready_chunk(
        ReadyChunk::try_new(
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(x, z),
            },
            1,
            1,
            empty_chunk_data(),
        )
        .expect("ready chunk"),
    );
}

fn drop_stacks(context: &TickContext<'_>, key: ChunkKey) -> Vec<ItemStack> {
    context
        .read()
        .drops(key)
        .iter()
        .map(|record| record.stack)
        .collect()
}

/// Fills every physical drop slot of one ready chunk with single stones at
/// distinct cells, so later rehearsals refuse with exhausted capacity.
fn fill_chunk(context: &mut TickContext<'_>, key: ChunkKey) {
    let tick = context.read().tick();
    for cell in 0..32 {
        let batch = DropBatch::try_new(
            DropSource::Death {
                actor: passive_key(1),
                tick,
            },
            key.dimension,
            FiniteVec3::try_new([
                (key.pos.x() * 16 + (cell % 8)) as f32 + 0.5,
                70.5,
                (key.pos.z() * 16 + (cell / 8)) as f32 + 0.5,
            ])
            .expect("cell center"),
            vec![ItemStack {
                item: 1,
                count: 1,
                durability: 0,
            }],
            5,
        )
        .expect("fill batch");
        context.stage(RuleEffect::Drops(batch)).expect("fill stage");
    }
    assert_eq!(drop_stacks(context, key).len(), 32, "the chunk must fill");
}

fn dying_cow(context: &mut TickContext<'_>, id: u64, position: [f32; 3]) {
    context
        .stage(RuleEffect::Actor(passive_actor(
            id,
            position,
            0.0,
            0,
            ActorLifecycle::Active,
        )))
        .expect("dying cow");
    // The newborn skip flag freezes movement without integrating, and the id
    // misses the graze roll, so the cow reaches death settlement unmoved.
    context
        .stage(RuleEffect::Runtime(passive_runtime(
            id,
            passive_aux(BlockPos::new(0, 1, 0), 0, None, true),
        )))
        .expect("dying runtime");
}

#[test]
fn passive_death_stages_beef_and_dead_atomically() {
    // A zero-health cow drops exactly one raw beef at its death chunk beside
    // the terminal record: never a looted-but-present or removed-but-unlooted
    // actor (`settlePassiveDeaths`).
    let id = first_non_hit_id(0, 0, 31);
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(1)))
        .expect("environment");
    preload_empty(&mut context, 0, 0);
    dying_cow(&mut context, id, [0.5, 1.0, 0.5]);
    let call = passive_call();
    let report = provider::run(&mut context, call).expect("death advance");
    let view = context.read();
    let record = view.actor(passive_key(id)).expect("dead cow");
    assert_eq!(record.lifecycle, ActorLifecycle::Dead);
    assert_eq!(record.survival.health(), 0);
    assert_eq!(
        drop_stacks(
            &context,
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, 0),
            }
        ),
        vec![ItemStack {
            item: ITEM_RAW_BEEF,
            count: 1,
            durability: 0,
        }]
    );
    assert_eq!(report.rejected, 0, "the death refuses nothing");
    assert!(context.events().is_empty());
}

#[test]
fn passive_all_chunks_full_omits_loot_but_completes_death() {
    // No ready chunk with room omits the beef deterministically while the
    // death still completes.
    let id = first_non_hit_id(0, 0, 31);
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(1)))
        .expect("environment");
    let home = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    };
    preload_empty(&mut context, 0, 0);
    fill_chunk(&mut context, home);
    dying_cow(&mut context, id, [0.5, 1.0, 0.5]);
    let call = passive_call();
    let report = provider::run(&mut context, call).expect("death advance");
    let view = context.read();
    assert_eq!(
        view.actor(passive_key(id)).expect("dead cow").lifecycle,
        ActorLifecycle::Dead
    );
    assert_eq!(drop_stacks(&context, home).len(), 32);
    assert_eq!(report.rejected, 0, "the omission is not a rejection");
}

#[test]
fn passive_below_min_death_stays_lootless() {
    // A death below the world floor has no valid drop column: the removal
    // stays lootless exactly like the movement fall-out path.
    let id = first_non_hit_id(0, 0, 31);
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment(1)))
        .expect("environment");
    preload_empty(&mut context, 0, 0);
    dying_cow(&mut context, id, [0.5, -100.0, 0.5]);
    let call = passive_call();
    provider::run(&mut context, call).expect("death advance");
    let view = context.read();
    assert_eq!(
        view.actor(passive_key(id)).expect("dead cow").lifecycle,
        ActorLifecycle::Dead
    );
    assert!(
        drop_stacks(
            &context,
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, 0),
            }
        )
        .is_empty()
    );
}
