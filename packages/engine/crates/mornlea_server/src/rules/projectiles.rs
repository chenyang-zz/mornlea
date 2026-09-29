//! Transient projectile flight and player bow draw.
//!
//! Go's `projectile.go` owns gravity, segment collision, lifetime and the
//! sorted bounded projectile set. `bow.go` owns draw interruption and the
//! debit-before-spawn order. Each hit settles before the next projectile;
//! later phases own death, drops and respawn.
//! Ready subscription squares and hostile ranged decisions come from the
//! serial reducer, never from a client or from this provider's own state.

use crate::core::interaction::target_block;
use mornlea_domain::{
    BlockPos, ChunkPos, CombatHit, CombatTarget, Dimension, Event, EventRecipient, FiniteVec3,
    MotionState, MotionStateParts, ProjectileId, ProjectileKind, RejectReason, RoutedEvent,
    SurvivalState, SurvivalStateParts,
};
use mornlea_engine::native::contracts::raycast::{Ray, RayCursor, RaycastOp};
use mornlea_engine::native::raycast::NativeRaycast;
use mornlea_storage::ItemStack;

use crate::core::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, BowProgress, DamageCause, InventoryPatch,
    PhaseReport, ProjectileRecord, Resource, RuleCall, RuleEffect, RulePhase, RuleReject,
    ServerError,
};
use crate::core::state::{AuthorityReadView, TickContext};
use crate::rules::inventory;

const MAX_PROJECTILES: usize = 128;
const MAX_SCOPES: usize = 8;
const MAX_RAY_BATCHES: usize = 8;
const MAX_RAY_CELLS: usize = MAX_RAY_BATCHES * 64;
const GRAVITY_STEP: f32 = 18.0 * 0.05;
const DELTA_SECONDS: f32 = 0.05;
const MAX_AGE: u32 = 100;
const MIN_Y: f32 = -64.0;
const MAX_Y: f32 = 320.0;
const BOW: u16 = 62;
const ARROW: u16 = 63;
const BROKEN_BOW: u16 = 65;
const MIN_DRAW: u16 = 6;
const FULL_DRAW: u16 = 20;
const PROJECTILE_SALT: u64 = 0x5052_4f4a_4543_5449;

/// One reducer supplied Ready session subscription square. The reducer
/// preserves the session's actual dimension, center and effective radius;
/// an unloaded cell inside this square remains eligible for flight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectileScope {
    pub dimension: Dimension,
    pub center: ChunkPos,
    pub radius: u64,
}

fn report(examined: usize, applied: usize) -> PhaseReport {
    PhaseReport {
        examined,
        applied,
        carried: 0,
        rejected: 0,
    }
}

struct FlightStep {
    effect: RuleEffect,
    event: Option<RoutedEvent>,
}

fn flight_step(effect: RuleEffect) -> Option<FlightStep> {
    Some(FlightStep {
        effect,
        event: None,
    })
}

/// The per-player draw entry and the empty batch shape of projectile flight.
/// The reducer calls [`advance`] with the Ready scope snapshot for the batch.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.command.is_some() || call.internal.is_some() {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    match call.phase {
        RulePhase::BowDraw => {
            let Some(actor @ ActorKey::Player(_)) = call.actor else {
                return Err(ServerError::InvalidInput { field: "actor" });
            };
            bow_draw(ctx, actor)
        }
        RulePhase::ProjectileStep if call.actor.is_none() => Ok(report(0, 0)),
        _ => Err(ServerError::InvalidInput { field: "phase" }),
    }
}

/// Advances the bounded, ID-ordered flight set once. Scope validation is
/// complete before any projectile mutation; each flight record then settles
/// its own remove, update or hit as an exact-before staged effect.
pub fn advance(
    ctx: &mut TickContext<'_>,
    scopes: &[ProjectileScope],
) -> Result<PhaseReport, ServerError> {
    if scopes.len() > MAX_SCOPES || scopes.iter().any(|scope| scope.radius > i64::MAX as u64) {
        return Err(ServerError::InvalidInput {
            field: "projectile_scopes",
        });
    }
    let mut records = ctx.read().projectiles().to_vec();
    records.sort_by_key(|record| record.id);
    let mut applied = 0;
    for before in &records {
        let step = flight_effect(&ctx.read(), before, scopes)?;
        if let Some(step) = step {
            ctx.stage(step.effect).map_err(|_| ServerError::Internal {
                invariant: "projectile flight stage",
            })?;
            if let Some(event) = step.event {
                ctx.emit(event)?;
            }
            applied += 1;
        }
    }
    Ok(report(records.len(), applied))
}

fn flight_effect(
    view: &AuthorityReadView<'_>,
    before: &ProjectileRecord,
    scopes: &[ProjectileScope],
) -> Result<Option<FlightStep>, ServerError> {
    if before.age >= MAX_AGE {
        return Ok(flight_step(remove(before)));
    }
    let previous = before.position.get();
    let old_velocity = before.velocity.get();
    let velocity = [
        old_velocity[0],
        old_velocity[1] - GRAVITY_STEP,
        old_velocity[2],
    ];
    let delta = velocity.map(|component| component * DELTA_SECONDS);
    let next = [
        previous[0] + delta[0],
        previous[1] + delta[1],
        previous[2] + delta[2],
    ];
    if !velocity
        .iter()
        .chain(next.iter())
        .all(|part| part.is_finite())
    {
        return Ok(flight_step(remove(before)));
    }
    let block_t = block_hit_t(view, before.dimension, previous, delta)?;
    let entity = entity_hit(view, before, previous, delta);
    if let Some((target, entity_t)) = entity
        && block_t.is_none_or(|wall_t| entity_t < wall_t)
    {
        return Ok(Some(impact_effect(view, before, target, velocity)?));
    }
    if block_t.is_some() {
        return Ok(flight_step(remove(before)));
    }
    if !(MIN_Y..MAX_Y).contains(&next[1]) || !subscribed(before.dimension, next, scopes) {
        return Ok(flight_step(remove(before)));
    }
    let mut after = before.clone();
    after.velocity = FiniteVec3::try_new(velocity).map_err(|_| ServerError::Internal {
        invariant: "projectile finite velocity",
    })?;
    after.position = FiniteVec3::try_new(next).map_err(|_| ServerError::Internal {
        invariant: "projectile finite position",
    })?;
    after.age += 1;
    Ok(flight_step(RuleEffect::Projectile {
        before: Some(before.clone()),
        after: Some(after),
    }))
}

fn remove(before: &ProjectileRecord) -> RuleEffect {
    RuleEffect::Projectile {
        before: Some(before.clone()),
        after: None,
    }
}

fn target_error() -> ServerError {
    ServerError::InvalidInput {
        field: "projectile_target",
    }
}

/// Settle one hit against the latest overlay before the next projectile can
/// choose a target. All required target lanes and the owner confirmation are
/// constructed before the compound can remove the projectile.
fn impact_effect(
    view: &AuthorityReadView<'_>,
    before: &ProjectileRecord,
    target: ActorKey,
    velocity: [f32; 3],
) -> Result<FlightStep, ServerError> {
    let mut actor = view.actor(target).cloned().ok_or_else(target_error)?;
    let impulse = projectile_impulse(velocity);
    let old_motion = actor.motion;
    let old_velocity = old_motion.velocity().get();
    let moved = [
        old_velocity[0] + impulse[0],
        old_velocity[1],
        old_velocity[2] + impulse[2],
    ];
    actor.motion = MotionState::new(MotionStateParts {
        position: old_motion.position(),
        velocity: FiniteVec3::try_new(moved).map_err(|_| target_error())?,
        on_ground: old_motion.on_ground(),
    });

    let mut parts = vec![remove(before)];
    let target_kind = match (&target, &mut actor.body) {
        (ActorKey::Player(_), ActorBody::Player(body)) => {
            let current = view.inventory(target).copied().ok_or_else(target_error)?;
            let mut runtime = view.runtime(target).cloned().ok_or_else(target_error)?;
            if runtime.key != target {
                return Err(target_error());
            }
            let mut updated = current;
            let points = inventory::armor_points(&current.armor);
            let effective = inventory::settle_damage(
                DamageCause::Projectile,
                i32::from(before.damage),
                points,
                &mut updated.armor,
            );
            let health = actor.survival.health().saturating_sub(effective as u8);
            actor.survival = survival_with_health(
                actor.survival,
                health,
                inventory::armor_points(&updated.armor),
            )?;
            body.health = health;
            body.armor = updated.armor;
            runtime.since_damage_ticks = 0;
            runtime.eating = None;
            runtime.bow = None;
            parts.push(RuleEffect::Actor(actor));
            parts.push(RuleEffect::Inventory(InventoryPatch {
                actor: target,
                before: current,
                after: updated,
            }));
            parts.push(RuleEffect::Runtime(runtime));
            CombatTarget::Player
        }
        (ActorKey::Hostile(_), ActorBody::Hostile(body)) => {
            let health = actor.survival.health().saturating_sub(before.damage);
            actor.survival =
                survival_with_health(actor.survival, health, actor.survival.armor_points())?;
            body.health = health;
            body.velocity = moved;
            parts.push(RuleEffect::Actor(actor));
            CombatTarget::Hostile
        }
        (ActorKey::Passive(_), ActorBody::Passive(body)) => {
            let mut runtime = view.runtime(target).cloned().ok_or_else(target_error)?;
            if runtime.key != target {
                return Err(target_error());
            }
            let ActorAux::Passive {
                flee_ticks,
                flee_from,
                graze_ticks,
                ..
            } = &mut runtime.aux
            else {
                return Err(target_error());
            };
            *flee_ticks = 60;
            *flee_from = Some(before.position);
            *graze_ticks = 0;
            let health = actor.survival.health().saturating_sub(before.damage);
            actor.survival =
                survival_with_health(actor.survival, health, actor.survival.armor_points())?;
            body.health = health;
            body.velocity = moved;
            parts.push(RuleEffect::Actor(actor));
            parts.push(RuleEffect::Runtime(runtime));
            CombatTarget::Passive
        }
        _ => return Err(target_error()),
    };

    let event = if before.kind == ProjectileKind::Arrow && view.tick() != 0 {
        let ActorKey::Player(owner) = before.owner else {
            return Err(target_error());
        };
        let hit = CombatHit::try_new(view.tick(), before.damage, target_kind)
            .map_err(|_| target_error())?;
        Some(RoutedEvent::new(
            EventRecipient::Session(owner.get()),
            Event::CombatHit(hit),
        ))
    } else {
        None
    };
    Ok(FlightStep {
        effect: RuleEffect::Compound(parts),
        event,
    })
}

fn survival_with_health(
    previous: SurvivalState,
    health: u8,
    armor_points: u8,
) -> Result<SurvivalState, ServerError> {
    SurvivalState::try_new(SurvivalStateParts {
        health,
        oxygen: previous.oxygen(),
        hunger: previous.hunger(),
        saturation_zero: previous.saturation_zero(),
        armor_points,
    })
    .map_err(|_| target_error())
}

fn projectile_impulse(velocity: [f32; 3]) -> [f32; 3] {
    let horizontal = [velocity[0], 0.0, velocity[2]];
    let squared = horizontal[0] * horizontal[0] + horizontal[2] * horizontal[2];
    if squared == 0.0 {
        return [0.0; 3];
    }
    // mgl32.Normalize multiplies by the f32 inverse length before the
    // shared f32 knockback speed, preserving source rounding order.
    let length = f64::from(squared).sqrt() as f32;
    let inverse = 1.0 / length;
    [
        horizontal[0] * inverse * 0.35,
        0.0,
        horizontal[2] * inverse * 0.35,
    ]
}

fn subscribed(dimension: Dimension, position: [f32; 3], scopes: &[ProjectileScope]) -> bool {
    let x = (position[0] as f64).floor() / 16.0;
    let z = (position[2] as f64).floor() / 16.0;
    if x < i32::MIN as f64 || x > i32::MAX as f64 || z < i32::MIN as f64 || z > i32::MAX as f64 {
        return false;
    }
    let x = x.floor() as i64;
    let z = z.floor() as i64;
    scopes.iter().any(|scope| {
        scope.dimension == dimension
            && (x - i64::from(scope.center.x())).unsigned_abs() <= scope.radius
            && (z - i64::from(scope.center.z())).unsigned_abs() <= scope.radius
    })
}

/// Uses the accepted F1 DDA kernel. An unavailable cell invalidates the
/// ray's hit observation, so it never becomes an invented solid block. A
/// traversal beyond 512 cells reports capacity before this projectile stages
/// a removal or hit; earlier projectiles in the ordered batch remain staged.
fn block_hit_t(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    origin: [f32; 3],
    delta: [f32; 3],
) -> Result<Option<f32>, ServerError> {
    // Go's Vec3.Len rounds the sum of f32 squares before its f64 square
    // root. Retain that ordinary distance for block/entity ordering.
    let squared = delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2];
    let source_length = f64::from(squared).sqrt() as f32;
    if source_length < 1e-6 {
        return Ok(None);
    }
    let length64 = f64::from(delta[0])
        .hypot(f64::from(delta[1]))
        .hypot(f64::from(delta[2]));
    // An extreme finite delta can overflow f32 square accumulation. The
    // f64 fallback keeps traversal bounded rather than treating it as no hit.
    let maximum = if source_length.is_finite() {
        source_length
    } else {
        length64.min(f64::from(f32::MAX)) as f32
    };
    let normalize = 1.0 / length64;
    let ray = Ray {
        origin,
        direction: delta.map(|component| component * normalize as f32),
        maximum,
    };
    let mut cursor = RayCursor::try_new(ray).map_err(|_| ServerError::InvalidInput {
        field: "projectile_ray",
    })?;
    // Production arrows and shards cross at most a few cells per step. A
    // malformed resolved record cannot turn one tick into an unbounded walk.
    for _ in 0..MAX_RAY_BATCHES {
        let batch =
            NativeRaycast
                .next_batch(&mut cursor)
                .map_err(|_| ServerError::InvalidInput {
                    field: "projectile_ray",
                })?;
        for record in batch.records() {
            let pos = BlockPos::new(record.cell[0], record.cell[1], record.cell[2]);
            let Some(observed) = view.observation(dimension, pos) else {
                return Ok(None);
            };
            if target_block(view, dimension, pos, observed.block) {
                let t = if source_length.is_finite() {
                    record.distance / source_length
                } else {
                    (f64::from(record.distance) / length64) as f32
                };
                return Ok(Some(t));
            }
        }
        if batch.is_done() {
            return Ok(None);
        }
    }
    // A full final batch does not check the next crossing. One bounded
    // numerical lookahead distinguishes completion at cell 512 from a 513th
    // visit, without observing any additional world cell.
    let lookahead =
        NativeRaycast
            .next_batch(&mut cursor)
            .map_err(|_| ServerError::InvalidInput {
                field: "projectile_ray",
            })?;
    if lookahead.records().is_empty() && lookahead.is_done() {
        Ok(None)
    } else {
        Err(ray_capacity())
    }
}

fn ray_capacity() -> ServerError {
    ServerError::Capacity {
        resource: Resource::RuleEffects,
        limit: MAX_RAY_CELLS,
        observed: MAX_RAY_CELLS + 1,
    }
}

/// The earliest current AABB along the segment wins; target-kind and stable
/// ID break an exact-distance tie independently of actor iteration order.
fn entity_hit(
    view: &AuthorityReadView<'_>,
    projectile: &ProjectileRecord,
    origin: [f32; 3],
    delta: [f32; 3],
) -> Option<(ActorKey, f32)> {
    let mut best: Option<(ActorKey, f32, u8, u64)> = None;
    for actor in view.actors() {
        if actor.lifecycle != ActorLifecycle::Active
            || actor.dimension != projectile.dimension
            || actor.survival.health() == 0
        {
            continue;
        }
        let (kind_rank, id) = match actor.key {
            ActorKey::Player(session) if projectile.kind == ProjectileKind::Arrow => {
                if projectile.owner == actor.key {
                    continue;
                }
                (1, session.get())
            }
            ActorKey::Player(session) => (1, session.get()),
            ActorKey::Hostile(id) if projectile.kind == ProjectileKind::Arrow => (2, id.get()),
            ActorKey::Passive(id) if projectile.kind == ProjectileKind::Arrow => (3, id.get()),
            _ => continue,
        };
        let Some(t) = segment_aabb(origin, delta, actor.motion.position().get()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, old_t, old_rank, old_id)| {
            t < *old_t || (t == *old_t && (kind_rank, id) < (*old_rank, *old_id))
        }) {
            best = Some((actor.key, t, kind_rank, id));
        }
    }
    best.map(|(key, t, _, _)| (key, t))
}

fn segment_aabb(origin: [f32; 3], delta: [f32; 3], feet: [f32; 3]) -> Option<f32> {
    let minimum = [feet[0] - 0.3, feet[1], feet[2] - 0.3];
    let maximum = [feet[0] + 0.3, feet[1] + 1.8, feet[2] + 0.3];
    let mut near: f32 = 0.0;
    let mut far: f32 = 1.0;
    for axis in 0..3 {
        if delta[axis].abs() < 1e-6 {
            if origin[axis] < minimum[axis] || origin[axis] > maximum[axis] {
                return None;
            }
            continue;
        }
        let mut entry = (minimum[axis] - origin[axis]) / delta[axis];
        let mut exit = (maximum[axis] - origin[axis]) / delta[axis];
        if entry > exit {
            (entry, exit) = (exit, entry);
        }
        near = near.max(entry);
        far = far.min(exit);
        if near > far {
            return None;
        }
    }
    Some(near)
}

/// Production insertion validates the complete resolved record before
/// evicting the smallest ID. The defensive direct staging cap remains strict.
pub fn spawn(ctx: &mut TickContext<'_>, record: ProjectileRecord) -> Result<(), RuleReject> {
    if !(1..=20).contains(&record.damage)
        || record.age != 0
        || !legal_owner(record.kind, record.owner)
    {
        return Err(RuleReject::Wire(RejectReason::InvalidInput));
    }
    let resident = ctx.read().projectiles().to_vec();
    if resident.iter().any(|current| current.id == record.id) {
        return Err(RuleReject::StaleObservation);
    }
    let mut parts = Vec::with_capacity(2);
    if resident.len() >= MAX_PROJECTILES {
        let first = resident
            .iter()
            .min_by_key(|value| value.id)
            .expect("full projectile set has a minimum");
        parts.push(remove(first));
    }
    parts.push(RuleEffect::Projectile {
        before: None,
        after: Some(record),
    });
    if parts.len() == 1 {
        ctx.stage(parts.pop().expect("one insertion"))
    } else {
        ctx.stage(RuleEffect::Compound(parts))
    }
}

/// Derives the source hash from seed, tick, kind, owner and exact f32 birth
/// bits. The bounded rehash chain observes the current resident IDs before
/// spawn's capacity eviction, matching the Go collision rule.
pub fn derive_spawn_id(
    ctx: &TickContext<'_>,
    kind: ProjectileKind,
    dimension: Dimension,
    owner: ActorKey,
    position: FiniteVec3,
) -> Result<ProjectileId, RuleReject> {
    if !legal_owner(kind, owner) {
        return Err(RuleReject::Wire(RejectReason::InvalidInput));
    }
    let view = ctx.read();
    let environment = view.environment().ok_or(RuleReject::StaleObservation)?;
    let owner_id = match owner {
        ActorKey::Player(session) => session.get(),
        ActorKey::Hostile(id) => id.get(),
        _ => return Err(RuleReject::Wire(RejectReason::InvalidInput)),
    };
    let kind_id = match kind {
        ProjectileKind::Shard => 0,
        ProjectileKind::Arrow => 1,
    };
    let mut id = splitmix64((environment.seed as u64) ^ PROJECTILE_SALT);
    id = splitmix64(id ^ view.tick());
    id = splitmix64(id ^ u64::from(dimension.get()));
    id = splitmix64(id ^ kind_id);
    id = splitmix64(id ^ owner_id);
    for part in position.get() {
        id = splitmix64(id ^ u64::from(part.to_bits()));
    }
    for _ in 0..64 {
        if let Ok(value) = ProjectileId::try_new(id)
            && !view.projectiles().iter().any(|record| record.id == value)
        {
            return Ok(value);
        }
        id = splitmix64(id);
    }
    Err(RuleReject::ResourceFull(Resource::RuleEffects))
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn legal_owner(kind: ProjectileKind, owner: ActorKey) -> bool {
    matches!(
        (kind, owner),
        (ProjectileKind::Arrow, ActorKey::Player(_))
            | (ProjectileKind::Shard, ActorKey::Hostile(_))
    )
}

fn bow_draw(ctx: &mut TickContext<'_>, actor: ActorKey) -> Result<PhaseReport, ServerError> {
    let ActorKey::Player(session) = actor else {
        return Err(ServerError::InvalidInput { field: "actor" });
    };
    let view = ctx.read();
    let Some(mut runtime) = view.runtime(actor).cloned() else {
        return Ok(report(1, 0));
    };
    let held = runtime
        .controls
        .is_some_and(|control| control.actions().primary);
    let suspended = runtime.reset || !runtime.has_view || view.viewer(session).is_some();
    let old = runtime.bow;
    if suspended {
        return clear_bow(ctx, runtime, old);
    }
    let Some(inventory) = view.inventory(actor).copied() else {
        return clear_bow(ctx, runtime, old);
    };
    let Some(player) = view.actor(actor).cloned() else {
        return clear_bow(ctx, runtime, old);
    };
    let slot = inventory.selected;
    let selected = inventory.slots[usize::from(slot.get())];
    if held {
        if selected.item != BOW
            || player.lifecycle != ActorLifecycle::Active
            || player.survival.health() == 0
        {
            return clear_bow(ctx, runtime, old);
        }
        let ticks = match old {
            Some(progress) if progress.slot == slot && progress.ticks != 0 => {
                progress.ticks.saturating_add(1)
            }
            _ if inventory
                .slots
                .iter()
                .any(|stack| stack.item == ARROW && stack.count > 0) =>
            {
                1
            }
            _ => return clear_bow(ctx, runtime, old),
        };
        runtime.bow = Some(BowProgress { slot, ticks });
        ctx.stage(RuleEffect::Runtime(runtime))
            .map_err(|_| ServerError::Internal {
                invariant: "bow progress",
            })?;
        return Ok(report(1, 1));
    }
    let Some(progress) = old else {
        return Ok(report(1, 0));
    };
    runtime.bow = None;
    if progress.slot != slot
        || selected.item != BOW
        || player.lifecycle != ActorLifecycle::Active
        || player.survival.health() == 0
        || progress.ticks < MIN_DRAW
    {
        ctx.stage(RuleEffect::Runtime(runtime))
            .map_err(|_| ServerError::Internal {
                invariant: "bow release",
            })?;
        return Ok(report(1, 1));
    }
    let Some(ammo) = inventory
        .slots
        .iter()
        .position(|stack| stack.item == ARROW && stack.count > 0)
    else {
        ctx.stage(RuleEffect::Runtime(runtime))
            .map_err(|_| ServerError::Internal {
                invariant: "bow no ammo",
            })?;
        return Ok(report(1, 1));
    };
    let mut after = inventory;
    let mut arrow = after.slots[ammo];
    arrow.count -= 1;
    after.slots[ammo] = if arrow.count == 0 {
        ItemStack::default()
    } else {
        arrow
    };
    let mut bow = selected;
    if bow.durability > 1 {
        bow.durability -= 1;
    } else {
        // Historical zero-durability bows take the same broken-form branch
        // as the last durability point in `consumeToolDurabilityAt`.
        bow.item = BROKEN_BOW;
        bow.durability = 0;
    }
    after.slots[usize::from(slot.get())] = bow;
    ctx.stage(RuleEffect::Compound(vec![
        RuleEffect::Inventory(InventoryPatch {
            actor,
            before: inventory,
            after,
        }),
        RuleEffect::Runtime(runtime),
    ]))
    .map_err(|_| ServerError::Internal {
        invariant: "bow debit",
    })?;

    let Some(environment) = ctx.read().environment() else {
        return Ok(report(1, 1));
    };
    let mut eye = player.motion.position().get();
    eye[1] += environment.tunables.eye_height();
    let Ok(position) = FiniteVec3::try_new(eye) else {
        return Ok(report(1, 1));
    };
    let Ok(id) = derive_spawn_id(
        ctx,
        ProjectileKind::Arrow,
        player.dimension,
        actor,
        position,
    ) else {
        return Ok(report(1, 1));
    };
    let (speed, damage) = if progress.ticks >= FULL_DRAW {
        (30.0, 5)
    } else {
        (16.0, 2)
    };
    let yaw = player.look.yaw();
    let pitch = player.look.pitch();
    // Go `LookDirection` evaluates trig in f64, rounds each value to f32,
    // then multiplies by the f32 tier speed.
    let cos_pitch = f64::from(pitch).cos() as f32;
    let direction = [
        -(f64::from(yaw).sin() as f32) * cos_pitch,
        f64::from(pitch).sin() as f32,
        -(f64::from(yaw).cos() as f32) * cos_pitch,
    ];
    let Ok(velocity) = FiniteVec3::try_new(direction.map(|part| part * speed)) else {
        return Ok(report(1, 1));
    };
    let _ = spawn(
        ctx,
        ProjectileRecord {
            id,
            owner: actor,
            dimension: player.dimension,
            position,
            velocity,
            kind: ProjectileKind::Arrow,
            damage,
            age: 0,
        },
    );
    Ok(report(1, 1))
}

fn clear_bow(
    ctx: &mut TickContext<'_>,
    mut runtime: crate::core::contracts::ActorRuntime,
    old: Option<BowProgress>,
) -> Result<PhaseReport, ServerError> {
    if old.is_none() {
        return Ok(report(1, 0));
    }
    runtime.bow = None;
    ctx.stage(RuleEffect::Runtime(runtime))
        .map_err(|_| ServerError::Internal {
            invariant: "bow clear",
        })?;
    Ok(report(1, 1))
}
