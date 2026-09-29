//! Pre-physics hostile action production: target facts, walker melee intent
//! batches, and hurler ranged shots, staged before hostile motion.
//!
//! This provider owns one batch call, the `HostileActions` phase: read-only
//! planning from the pre-step overlay followed by staging exactly what
//! planning admitted. Any other call shape is refused without effect. The
//! hostile set is small and bounded (64 residents), so planning examines
//! every hostile in ascending-id order and stays deterministic end to end.
//!
//! Mirrored Go rows, each cited at its site:
//!
//! - `packages/server/sim/entity/hostile_action.go`: the shoot-cooldown
//!   gate, the kind and liveness checks, the eye-plus-spread velocity, and
//!   the 40-tick refire period (`settleHostileRangedShot`,
//!   `hostileShardVelocity`).
//! - `packages/server/server/hostile_manager.go`: the deterministic target
//!   selection, the every-tick walker melee submission regardless of attack
//!   cooldown, the hurler band strategy, and the eye-to-eye shot decision
//!   (`nearestTarget`, `advanceRunners`, `considerHurlerShot`,
//!   `hostileRangedLineOfSight`).
//! - `packages/server/updates/sampler.go`: the `SplitMix64` mixer and the
//!   shot-spread derivation (`HostileShotSpread` with the `SHOTSPRD` salt
//!   and the fixed 0.06 rad single-axis bound).
//! - `packages/server/sim/contract/contract.go`: the 1.8 attack range.
//! - `packages/server/sim/entity/projectile.go`: the shard damage 3 and
//!   speed 22.
//!
//! Deliberate boundaries (later phases own them): hit settlement, damage,
//! death, and reducer acceptance. The admitted melee batch travels inside
//! the returned plan for the future reducer to hand to melee settlement;
//! the overlay itself carries no melee channel, so applying a plan stages
//! only shard spawns and shoot-cooldown runtimes. The producer never reads
//! or writes attack cooldowns and never damages.

use mornlea_domain::{BlockPos, Dimension, FiniteVec3, ProjectileKind};
use mornlea_engine::native::contracts::raycast::{Ray, RayCursor, RaycastOp};
use mornlea_engine::native::raycast::NativeRaycast;

use crate::core::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRuntime, HostileMeleeAttack,
    HostileMeleeBatch, PhaseReport, ProjectileRecord, RuleCall, RuleEffect, RulePhase, ServerError,
};
use crate::core::interaction::{look_direction, normalized_direction, target_block};
use crate::core::state::{AuthorityReadView, TickContext};
use crate::rules::hostile_actors::{PlayerFact, nearest_target};
use crate::rules::projectiles;

// ---------------------------------------------------------------------------
// Frozen numeric contract. Each mirrors its Go row exactly.
// ---------------------------------------------------------------------------

/// Walker melee reach (`HostileAttackRange` in
/// `packages/server/sim/contract/contract.go`); comparisons stay in the
/// squared domain.
const MELEE_RANGE: f32 = 1.8;
/// Shard damage (`projectileShardDamage` in
/// `packages/server/sim/entity/projectile.go`).
const SHOT_DAMAGE: u8 = 3;
/// Shard speed in blocks per second (`projectileShardSpeed`).
const SHOT_SPEED: f32 = 22.0;
/// Refire period after a shot (`hostileShootCooldownTicks` in
/// `packages/server/sim/entity/hostile.go`).
const SHOT_COOLDOWN: u32 = 40;
/// Hostile kind byte for the ranged kind (`HostileKindBoneThrower`).
const HURLER: u8 = 1;
/// Shot-spread salt (`HostileShotSpreadSalt`, ASCII `SHOTSPRD`).
const SPREAD_SALT: u64 = 0x5348_4F54_5350_5244;
/// Single-axis spread bound in radians (`HostileShotSpreadMaxRadians`).
const SPREAD_MAX_RADIANS: f32 = 0.06;
/// Spread sampling quantum (`hostileShotSpreadQuantum`, 2^20).
const SPREAD_QUANTUM: u64 = 1 << 20;
/// Half-pi pitch clamp (`quarterPi` in `hostileShardVelocity`).
const QUARTER_PI: f32 = std::f32::consts::FRAC_PI_2;
/// Line-of-sight walk bound: an eye-to-eye segment crossing more cells
/// reads as blocked, matching the manager's small-chunk-view conservatism
/// while keeping the tick bounded.
const MAX_SIGHT_CELLS: usize = 512;

/// Tick-local hostile choices admitted from the pre-step overlay. Only
/// planning constructs it; the future reducer owns the one invocation per
/// tick and hands the melee batch to settlement.
#[derive(Clone, Debug)]
pub struct HostileActionPlan {
    tick: u64,
    melee: HostileMeleeBatch,
    shots: Vec<ShotStaging>,
}

impl HostileActionPlan {
    /// The authority tick the plan was admitted for.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// The admitted walker melee batch, in hostile-ID order.
    pub fn melee_batch(&self) -> &HostileMeleeBatch {
        &self.melee
    }
}

/// One admitted hurler shot: the normalized aim and both eye positions are
/// frozen at planning; the spawn ID, velocity and cooldown stage at apply.
#[derive(Clone, Copy, Debug)]
struct ShotStaging {
    key: ActorKey,
    hostile_id: u64,
    dimension: Dimension,
    eye: [f32; 3],
    aim: [f32; 3],
}

/// Settles one hostile action batch: only an empty `HostileActions` call
/// runs, planning then staging in one step.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if !matches!(call.phase, RulePhase::HostileActions) {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    if call.actor.is_some() || call.command.is_some() || call.internal.is_some() {
        return Err(ServerError::InvalidInput {
            field: "hostile_action_call",
        });
    }
    let plan = plan(ctx)?;
    apply(ctx, plan)
}

/// Admits melee intents and hurler shots from the pre-step overlay. Pure
/// read-only planning: selects targets, admits walker entries in
/// hostile-ID order, and freezes hurler shot stagings. No staging, no
/// mutation.
pub fn plan(ctx: &TickContext<'_>) -> Result<HostileActionPlan, ServerError> {
    let view = ctx.read();
    let environment = view.environment().cloned().ok_or(ServerError::Internal {
        invariant: "hostile action environment",
    })?;
    let tick = view.tick();
    let eye_height = environment.tunables.eye_height();

    let mut bodies: Vec<(ActorKey, &crate::core::contracts::ActorRecord)> = Vec::new();
    for record in view.actors() {
        if matches!(record.key, ActorKey::Hostile(_)) && record.lifecycle == ActorLifecycle::Active
        {
            bodies.push((record.key, record));
        }
    }
    bodies.sort_by_key(|(_, record)| hostile_id_of(record));

    let mut melee = Vec::new();
    let mut shots = Vec::new();
    for (key, record) in bodies {
        let ActorBody::Hostile(body) = &record.body else {
            continue;
        };
        if body.health == 0 {
            // Zero-health hostiles neither attack nor shoot.
            continue;
        }
        if hostile_fresh(&view, key) {
            // Fresh spawns produce no intent on their spawn tick.
            continue;
        }
        let Some(target) = live_target(&view, record.dimension, body.position)? else {
            continue;
        };
        if body.kind == HURLER {
            // Hurlers never receive melee entries, even within reach.
            if let Some(staging) = stage_shot(
                &view,
                key,
                body.id,
                record.dimension,
                body.position,
                &target,
                eye_height,
            )? {
                shots.push(staging);
            }
        } else if horizontal_distance_sq(body.position, target.position)
            <= MELEE_RANGE * MELEE_RANGE
        {
            // Walkers submit every tick within reach, regardless of attack
            // cooldown: cooldown gating belongs to settlement.
            let attacker =
                mornlea_domain::HostileId::try_new(body.id).map_err(|_| ServerError::Internal {
                    invariant: "hostile melee attacker",
                })?;
            melee.push(HostileMeleeAttack::new(attacker, target.session));
        }
    }

    // Overflow is impossible by construction (one entry per resident, and
    // residents are capped at 64 upstream); the constructor still guards.
    let melee = HostileMeleeBatch::try_new(tick, &melee)?;
    Ok(HostileActionPlan { tick, melee, shots })
}

/// Stages exactly what planning admitted: no re-selection, no new
/// admission. A plan from another tick refuses before any effect.
pub fn apply(
    ctx: &mut TickContext<'_>,
    plan: HostileActionPlan,
) -> Result<PhaseReport, ServerError> {
    if plan.tick != ctx.read().tick() {
        return Err(ServerError::InvalidInput {
            field: "hostile_action_tick",
        });
    }
    let examined = plan.melee.entries().len() + plan.shots.len();
    let mut applied = plan.melee.entries().len();
    let mut rejected = 0usize;
    for shot in &plan.shots {
        if settle_shot(ctx, shot)? {
            applied += 1;
        } else {
            rejected += 1;
        }
    }
    Ok(PhaseReport {
        examined,
        applied,
        carried: 0,
        rejected,
    })
}

/// Selects the nearest live target through the shared motion selector: the
/// nearest active same-dimension player, exact ties by smaller `PlayerID`
/// bytes. Zero-health players are never targets.
fn live_target(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    position: [f32; 3],
) -> Result<Option<PlayerFact>, ServerError> {
    let Some(fact) = nearest_target(view, dimension, position)? else {
        return Ok(None);
    };
    let live = view
        .actor(ActorKey::Player(fact.session))
        .is_some_and(|record| {
            record.lifecycle == ActorLifecycle::Active && record.survival.health() != 0
        });
    Ok(live.then_some(fact))
}

/// Freezes one hurler shot when the pre-step cooldown is ready, the aim is
/// normalizable, and the eye-to-eye segment is unobstructed. Returns `None`
/// without effect otherwise.
#[allow(clippy::too_many_arguments)]
fn stage_shot(
    view: &AuthorityReadView<'_>,
    key: ActorKey,
    hostile_id: u64,
    dimension: Dimension,
    position: [f32; 3],
    target: &PlayerFact,
    eye_height: f32,
) -> Result<Option<ShotStaging>, ServerError> {
    if hostile_cooldown(view, key) != 0 {
        // Cooldown 1 shoots nothing this tick even though motion later
        // decrements it; the pre-step value rules here.
        return Ok(None);
    }
    let eye = [position[0], position[1] + eye_height, position[2]];
    let target_eye = [
        target.position[0],
        target.position[1] + eye_height,
        target.position[2],
    ];
    let delta = [
        target_eye[0] - eye[0],
        target_eye[1] - eye[1],
        target_eye[2] - eye[2],
    ];
    let Some(aim) = normalized_direction(delta) else {
        return Ok(None);
    };
    if !line_of_sight(view, dimension, eye, aim, target_eye)? {
        return Ok(None);
    }
    Ok(Some(ShotStaging {
        key,
        hostile_id,
        dimension,
        eye,
        aim,
    }))
}

/// Reports whether the eye-to-eye segment is unobstructed: any blocking
/// cell strictly before the target eye blocks, while the target cell
/// itself never does. Unobserved cells read as blocking, so a shot never
/// fires on world the authority was not handed.
fn line_of_sight(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    origin: [f32; 3],
    direction: [f32; 3],
    target_eye: [f32; 3],
) -> Result<bool, ServerError> {
    let maximum = f64::from(target_eye[0] - origin[0])
        .hypot(f64::from(target_eye[1] - origin[1]))
        .hypot(f64::from(target_eye[2] - origin[2])) as f32;
    let target = BlockPos::new(
        floor_to_i32(target_eye[0]),
        floor_to_i32(target_eye[1]),
        floor_to_i32(target_eye[2]),
    );
    let mut cursor = RayCursor::try_new(Ray {
        origin,
        direction,
        maximum,
    })
    .map_err(|_| ServerError::Internal {
        invariant: "hostile shot ray",
    })?;
    let mut visited = 0usize;
    loop {
        let batch = NativeRaycast
            .next_batch(&mut cursor)
            .map_err(|_| ServerError::Internal {
                invariant: "hostile shot ray",
            })?;
        for record in batch.records() {
            visited += 1;
            if visited > MAX_SIGHT_CELLS {
                return Ok(false);
            }
            let cell = BlockPos::new(record.cell[0], record.cell[1], record.cell[2]);
            if cell == target {
                return Ok(true);
            }
            let Some(observed) = view.observation(dimension, cell) else {
                return Ok(false);
            };
            if target_block(view, dimension, cell, observed.block) {
                return Ok(false);
            }
        }
        if batch.is_done() {
            return Ok(true);
        }
    }
}

/// Settles one admitted shot through the accepted ports: the shard spawns
/// with an ID from the derive port, and on success the 40-cooldown stages.
/// A stale candidate (kind, liveness or cooldown changed) or a refused
/// spawn stages nothing, consumes no cooldown, and never blocks the other
/// intents.
fn settle_shot(ctx: &mut TickContext<'_>, shot: &ShotStaging) -> Result<bool, ServerError> {
    let (kind, health, distant_ticks, burn_cooldown) = {
        let view = ctx.read();
        let Some(record) = view.actor(shot.key) else {
            return Ok(false);
        };
        if record.lifecycle != ActorLifecycle::Active {
            return Ok(false);
        }
        let ActorBody::Hostile(body) = &record.body else {
            return Ok(false);
        };
        (
            body.kind,
            body.health,
            body.distant_ticks,
            body.burn_cooldown,
        )
    };
    if kind != HURLER || health == 0 {
        return Ok(false);
    }
    if hostile_cooldown(&ctx.read(), shot.key) != 0 {
        return Ok(false);
    }
    let environment = ctx
        .read()
        .environment()
        .cloned()
        .ok_or(ServerError::Internal {
            invariant: "hostile action environment",
        })?;
    let eye = FiniteVec3::try_new(shot.eye).map_err(|_| ServerError::Internal {
        invariant: "hostile shot eye",
    })?;
    let velocity = FiniteVec3::try_new(shard_velocity(
        environment.seed,
        environment.world_time,
        shot.hostile_id,
        shot.aim,
    ))
    .map_err(|_| ServerError::Internal {
        invariant: "hostile shot velocity",
    })?;
    let id = match projectiles::derive_spawn_id(
        ctx,
        ProjectileKind::Shard,
        shot.dimension,
        shot.key,
        eye,
    ) {
        Ok(id) => id,
        Err(_) => return Ok(false),
    };
    let record = ProjectileRecord {
        id,
        owner: shot.key,
        dimension: shot.dimension,
        position: eye,
        velocity,
        kind: ProjectileKind::Shard,
        damage: SHOT_DAMAGE,
        age: 0,
    };
    if projectiles::spawn(ctx, record).is_err() {
        return Ok(false);
    }
    let runtime = match ctx.read().runtime(shot.key) {
        Some(current) => {
            let mut staged = current.clone();
            if let ActorAux::Hostile { shoot_cooldown, .. } = &mut staged.aux {
                *shoot_cooldown = SHOT_COOLDOWN;
            }
            staged
        }
        None => ActorRuntime {
            key: shot.key,
            controls: None,
            has_view: false,
            reset: false,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: u32::from(burn_cooldown),
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
                distant_ticks,
                shoot_cooldown: SHOT_COOLDOWN,
                fresh: false,
            },
        },
    };
    ctx.stage(RuleEffect::Runtime(runtime))
        .map_err(|_| ServerError::Internal {
            invariant: "hostile shot staging",
        })?;
    Ok(true)
}

/// Folds the normalized aim into a shard velocity: yaw/pitch plus the
/// deterministic spread offsets, pitch clamped, speed rebuilt
/// (`hostileShardVelocity`).
fn shard_velocity(seed: i64, world_time: u64, id: u64, aim: [f32; 3]) -> [f32; 3] {
    let yaw = f64::from(-aim[0]).atan2(f64::from(-aim[2])) as f32;
    let pitch = f64::from(aim[1]).clamp(-1.0, 1.0).asin() as f32;
    let (yaw_offset, pitch_offset) = shot_spread(seed, world_time, id);
    let yaw = normalize_yaw(yaw + yaw_offset);
    let pitch = (pitch + pitch_offset).clamp(-QUARTER_PI, QUARTER_PI);
    let direction = look_direction(yaw, pitch);
    [
        direction[0] * SHOT_SPEED,
        direction[1] * SHOT_SPEED,
        direction[2] * SHOT_SPEED,
    ]
}

/// Deterministic shot spread from seed, world time and hostile ID
/// (`HostileShotSpread`): two independent uniform offsets within the fixed
/// bound, no process randomness.
fn shot_spread(seed: i64, world_time: u64, id: u64) -> (f32, f32) {
    let hash = splitmix64(splitmix64(splitmix64((seed as u64) ^ SPREAD_SALT) ^ world_time) ^ id);
    (spread_offset(hash), spread_offset(splitmix64(hash)))
}

/// Maps one draw into the symmetric uniform bound.
fn spread_offset(hash: u64) -> f32 {
    let unit = (hash & (SPREAD_QUANTUM - 1)) as f32 / SPREAD_QUANTUM as f32;
    (unit * 2.0 - 1.0) * SPREAD_MAX_RADIANS
}

/// The pre-step shoot cooldown: the staged runtime transient, or ready when
/// no runtime was staged (restored records refire immediately).
fn hostile_cooldown(view: &AuthorityReadView<'_>, key: ActorKey) -> u32 {
    view.runtime(key).map_or(0, |runtime| match runtime.aux {
        ActorAux::Hostile { shoot_cooldown, .. } => shoot_cooldown,
        _ => 0,
    })
}

/// Whether the pre-step runtime still marks the hostile freshly spawned.
fn hostile_fresh(view: &AuthorityReadView<'_>, key: ActorKey) -> bool {
    view.runtime(key).is_some_and(|runtime| match runtime.aux {
        ActorAux::Hostile { fresh, .. } => fresh,
        _ => false,
    })
}

/// Sort key for ascending-ID planning order.
fn hostile_id_of(record: &crate::core::contracts::ActorRecord) -> u64 {
    match &record.body {
        ActorBody::Hostile(body) => body.id,
        _ => u64::MAX,
    }
}

/// Squared horizontal distance, the only comparison domain the radius rules
/// use (`horizontalDistanceSq` in `hostile.go`).
fn horizontal_distance_sq(from: [f32; 3], to: [f32; 3]) -> f32 {
    let dx = to[0] - from[0];
    let dz = to[2] - from[2];
    dx * dx + dz * dz
}

/// Checked floor into i32; positions outside the int32 domain cannot name a
/// cell and refuse (`collisionCheckedFloor`).
fn floor_to_i32(value: f32) -> i32 {
    let floored = f64::from(value).floor();
    if !floored.is_finite() || !((i32::MIN as f64)..=(i32::MAX as f64)).contains(&floored) {
        0
    } else {
        floored as i32
    }
}

/// Yaw normalization mirror (`normalizeYaw` in
/// `packages/server/sim/entity/placement.go`): into [-pi, pi).
fn normalize_yaw(yaw: f32) -> f32 {
    let mut normalized = (f64::from(yaw) + std::f64::consts::PI) % (2.0 * std::f64::consts::PI);
    if normalized < 0.0 {
        normalized += 2.0 * std::f64::consts::PI;
    }
    (normalized - std::f64::consts::PI) as f32
}

/// The repository's shared SplitMix64 mixer (`sampler.SplitMix64` in
/// `packages/server/updates/sampler.go`).
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}
