//! Bounded melee freeze and settlement over the authoritative tick overlay.
//!
//! Hostile choices are supplied by the pre-physics producer. This provider
//! freezes post-motion geometry, builds all intents before any cooldown write,
//! then validates live identity and stages each successful hit atomically.

use mornlea_domain::{
    BlockPos, CombatHit, CombatTarget, Dimension, Event, EventRecipient, FiniteVec3, MotionState,
    MotionStateParts, RoutedEvent, SurvivalState, SurvivalStateParts,
};
use mornlea_engine::native::contracts::raycast::{Ray, RayCursor, RaycastOp};
use mornlea_engine::native::raycast::NativeRaycast;
use mornlea_storage::ItemStack;

use crate::core::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, DamageCause, HostileMeleeBatch, InventoryPatch,
    PhaseReport, Resource, RuleCall, RuleEffect, RulePhase, ServerError, SessionKey,
};
use crate::core::interaction::{look_direction, normalized_direction, target_block};
use crate::core::state::{AuthorityReadView, TickContext};
use crate::rules::{inventory, player_survival};

const MAX_ACTORS: usize = 104;
const MAX_INTENTS: usize = 72;
const REACH: f32 = 3.0;

/// Melee-capable hostile kind. Only the Go `HostileKindNightwalker` (`0` in
/// `packages/shared/network/protocol/message_hostile.go`) closes to melee;
/// `HostileKindBoneThrower` (`1`) hurls from range and never melees.
const WALKER_KIND: u8 = 0;

#[derive(Clone)]
struct FrozenActor {
    key: ActorKey,
    body_identity: BodyIdentity,
    dimension: Dimension,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
    health: u8,
    attack_cooldown: u32,
    hurt_cooldown: u32,
    primary: bool,
    hostile_kind: u8,
    selected_slot: usize,
    selected: ItemStack,
    armor_points: u8,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum BodyIdentity {
    Player(mornlea_storage::PlayerId),
    Mob(u64),
}

struct Intent {
    attacker: FrozenActor,
    victim: FrozenActor,
    damage: u8,
    player_attack: bool,
}

/// One bounded, single-use tick snapshot. Only freeze constructs its private
/// records; settlement consumes it so stale choices cannot be replayed.
pub struct CombatFrame {
    tick: u64,
    actors: Vec<FrozenActor>,
    intents: Vec<Intent>,
}

pub struct CombatOutcome {
    pub report: PhaseReport,
    pub damaged_players: Vec<SessionKey>,
}

pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::Combat
        || call.actor.is_some()
        || call.command.is_some()
        || call.internal.is_some()
    {
        return Err(ServerError::InvalidInput {
            field: "combat_call",
        });
    }
    let empty = HostileMeleeBatch::try_new(ctx.read().tick(), &[])?;
    Ok(advance(ctx, &empty)?.report)
}

pub fn advance(
    ctx: &mut TickContext<'_>,
    attacks: &HostileMeleeBatch,
) -> Result<CombatOutcome, ServerError> {
    let frame = freeze(ctx, attacks)?;
    settle_frame(ctx, frame)
}

pub fn freeze(
    ctx: &TickContext<'_>,
    attacks: &HostileMeleeBatch,
) -> Result<CombatFrame, ServerError> {
    freeze_with_limits(ctx, attacks, MAX_ACTORS, MAX_INTENTS)
}

fn freeze_with_limits(
    ctx: &TickContext<'_>,
    attacks: &HostileMeleeBatch,
    actor_limit: usize,
    intent_limit: usize,
) -> Result<CombatFrame, ServerError> {
    let view = ctx.read();
    if attacks.tick() != view.tick() {
        return Err(ServerError::InvalidInput {
            field: "combat_tick",
        });
    }
    let environment = view.environment().ok_or(ServerError::Internal {
        invariant: "combat environment",
    })?;
    let mut hostiles = Vec::new();
    let mut players = Vec::new();
    let mut passives = Vec::new();
    for actor in view.actors() {
        match actor.key {
            ActorKey::Hostile(_) if actor.lifecycle == ActorLifecycle::Active => {
                hostiles.push(actor)
            }
            ActorKey::Passive(_) if actor.lifecycle == ActorLifecycle::Active => {
                passives.push(actor)
            }
            ActorKey::Player(_) if actor.lifecycle == ActorLifecycle::Active => players.push(actor),
            _ => {}
        }
    }
    for (count, ceiling, resource) in [
        (players.len(), 8, Resource::Players),
        (hostiles.len(), 64, Resource::RuleEffects),
        (passives.len(), 32, Resource::RuleEffects),
    ] {
        if count > ceiling {
            return Err(ServerError::Capacity {
                resource,
                limit: ceiling,
                observed: count,
            });
        }
    }
    let ceiling = actor_limit.min(MAX_ACTORS);
    let count = players.len() + hostiles.len() + passives.len();
    if count > ceiling {
        return Err(ServerError::Capacity {
            resource: Resource::RuleEffects,
            limit: ceiling,
            observed: count,
        });
    }
    hostiles.sort_by_key(|actor| actor.key);
    players.sort_by_key(|actor| actor.key);
    passives.sort_by_key(|actor| actor.key);
    let mut actors = Vec::with_capacity(count);
    for actor in hostiles.into_iter().chain(players).chain(passives) {
        let position = actor.motion.position().get();
        let eye = f64::from(position[1]) + f64::from(environment.tunables.eye_height());
        if position
            .iter()
            .any(|value| !in_floor_bounds(f64::from(*value)))
            || !in_floor_bounds(eye)
        {
            return Err(ServerError::InvalidInput {
                field: "combat_actor",
            });
        }
        let key = actor.key;
        let runtime = view.runtime(key).ok_or(ServerError::Internal {
            invariant: "combat actor runtime",
        })?;
        if runtime.key != key {
            return Err(ServerError::Internal {
                invariant: "combat actor runtime",
            });
        }
        let (
            body_identity,
            attack_cooldown,
            hurt_cooldown,
            primary,
            hostile_kind,
            selected_slot,
            selected,
            armor_points,
        ) = match (&actor.body, key) {
            (ActorBody::Hostile(body), ActorKey::Hostile(_)) => {
                if !matches!(runtime.aux, ActorAux::Hostile { .. }) {
                    return Err(ServerError::Internal {
                        invariant: "combat hostile runtime",
                    });
                }
                (
                    BodyIdentity::Mob(body.id),
                    u32::from(body.attack_cooldown),
                    u32::from(body.hurt_cooldown),
                    false,
                    body.kind,
                    0,
                    ItemStack::default(),
                    0,
                )
            }
            (ActorBody::Player(_), ActorKey::Player(_)) => {
                if runtime.key != key || !matches!(runtime.aux, ActorAux::Player { .. }) {
                    return Err(ServerError::Internal {
                        invariant: "combat player runtime",
                    });
                }
                let inventory = view.inventory(key).ok_or(ServerError::Internal {
                    invariant: "combat player inventory",
                })?;
                let slot = usize::from(inventory.selected.get());
                let ActorBody::Player(body) = &actor.body else {
                    unreachable!()
                };
                (
                    BodyIdentity::Player(body.player_id),
                    runtime.attack_cooldown,
                    runtime.hurt_cooldown,
                    runtime
                        .controls
                        .is_some_and(|control| control.actions().primary),
                    0,
                    slot,
                    inventory.slots[slot],
                    inventory::armor_points(&inventory.armor),
                )
            }
            (ActorBody::Passive(_), ActorKey::Passive(_)) => {
                if !matches!(runtime.aux, ActorAux::Passive { .. }) {
                    return Err(ServerError::Internal {
                        invariant: "combat passive runtime",
                    });
                }
                let ActorBody::Passive(body) = &actor.body else {
                    unreachable!()
                };
                (
                    BodyIdentity::Mob(body.id),
                    0,
                    0,
                    false,
                    0,
                    0,
                    ItemStack::default(),
                    0,
                )
            }
            _ => {
                return Err(ServerError::Internal {
                    invariant: "combat actor body",
                });
            }
        };
        actors.push(FrozenActor {
            key,
            body_identity,
            dimension: actor.dimension,
            position,
            yaw: actor.look.yaw(),
            pitch: actor.look.pitch(),
            health: actor.survival.health(),
            attack_cooldown: attack_cooldown.saturating_sub(1),
            hurt_cooldown: hurt_cooldown.saturating_sub(1),
            primary,
            hostile_kind,
            selected_slot,
            selected,
            armor_points,
        });
    }
    let mut intents = Vec::new();
    let ceiling = intent_limit.min(MAX_INTENTS);
    for attack in attacks.entries() {
        let source = actors
            .iter()
            .find(|actor| actor.key == ActorKey::Hostile(attack.attacker()));
        let target = actors
            .iter()
            .find(|actor| actor.key == ActorKey::Player(attack.target()));
        if let (Some(source), Some(target)) = (source, target) {
            let delta = [
                source.position[0] - target.position[0],
                source.position[2] - target.position[2],
            ];
            if source.hostile_kind == WALKER_KIND
                && source.health > 0
                && source.attack_cooldown == 0
                && target.health > 0
                && target.hurt_cooldown == 0
                && source.dimension == target.dimension
                && delta[0] * delta[0] + delta[1] * delta[1] <= 1.8 * 1.8
            {
                push_intent(
                    &mut intents,
                    ceiling,
                    Intent {
                        attacker: source.clone(),
                        victim: target.clone(),
                        damage: 3,
                        player_attack: false,
                    },
                )?;
            }
        }
    }
    for source in actors
        .iter()
        .filter(|actor| matches!(actor.key, ActorKey::Player(_)))
    {
        if !source.primary
            || source.health == 0
            || source.attack_cooldown != 0
            || matches!(source.selected.item, 62 | 65)
        {
            continue;
        }
        let eye = [
            source.position[0],
            source.position[1] + environment.tunables.eye_height(),
            source.position[2],
        ];
        let direction = look_direction(source.yaw, source.pitch);
        let mut chosen: Option<(&FrozenActor, f32)> = None;
        for target in &actors {
            if target.key == source.key
                || target.dimension != source.dimension
                || target.health == 0
            {
                continue;
            }
            if let Some(distance) = aabb_distance(eye, direction, target.position)
                && chosen.is_none_or(|(old, current)| {
                    distance < current
                        || distance == current && key_rank(target.key) < key_rank(old.key)
                })
            {
                chosen = Some((target, distance));
            }
        }
        if let Some((target, distance)) = chosen {
            if target.hurt_cooldown != 0
                || !clear_ray(&view, source.dimension, eye, direction, distance)?
            {
                continue;
            }
            let damage = match source.selected.item {
                47 => 4,
                48 => 5,
                49 => 6,
                _ => 2,
            };
            push_intent(
                &mut intents,
                ceiling,
                Intent {
                    attacker: source.clone(),
                    victim: target.clone(),
                    damage,
                    player_attack: true,
                },
            )?;
        }
    }
    for intent in &intents {
        if intent.player_attack {
            ctx.check_mining_suppression(intent.attacker.key)?;
        }
    }
    Ok(CombatFrame {
        tick: view.tick(),
        actors,
        intents,
    })
}

fn push_intent(
    intents: &mut Vec<Intent>,
    ceiling: usize,
    intent: Intent,
) -> Result<(), ServerError> {
    if intents.len() >= ceiling {
        return Err(ServerError::Capacity {
            resource: Resource::RuleEffects,
            limit: ceiling,
            observed: intents.len() + 1,
        });
    }
    intents.push(intent);
    Ok(())
}

fn in_floor_bounds(value: f64) -> bool {
    value.is_finite() && value >= f64::from(i32::MIN) && value < f64::from(i32::MAX) + 1.0
}

fn key_rank(key: ActorKey) -> (u8, u64) {
    match key {
        ActorKey::Player(id) => (1, id.get()),
        ActorKey::Hostile(id) => (2, id.get()),
        ActorKey::Passive(id) => (3, id.get()),
        ActorKey::Companion(_) => (4, 0),
    }
}

fn aabb_distance(origin: [f32; 3], direction: [f32; 3], position: [f32; 3]) -> Option<f32> {
    let min = [position[0] - 0.3, position[1], position[2] - 0.3];
    let max = [position[0] + 0.3, position[1] + 1.8, position[2] + 0.3];
    let (mut near, mut far) = (-f32::MAX, f32::MAX);
    for axis in 0..3 {
        if direction[axis].abs() < 1e-6 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
            continue;
        }
        let mut entry = (min[axis] - origin[axis]) / direction[axis];
        let mut exit = (max[axis] - origin[axis]) / direction[axis];
        if entry > exit {
            std::mem::swap(&mut entry, &mut exit);
        }
        near = near.max(entry);
        far = far.min(exit);
        if near > far {
            return None;
        }
    }
    if far < 0.0 {
        return None;
    }
    near = near.max(0.0);
    (near <= REACH).then_some(near)
}

fn clear_ray(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    eye: [f32; 3],
    direction: [f32; 3],
    target_distance: f32,
) -> Result<bool, ServerError> {
    let Some(unit) = normalized_direction(direction) else {
        return Err(ServerError::InvalidInput {
            field: "combat_actor",
        });
    };
    let mut cursor = RayCursor::try_new(Ray {
        origin: eye,
        direction: unit,
        maximum: REACH,
    })
    .map_err(|_| ServerError::InvalidInput {
        field: "combat_actor",
    })?;
    for _ in 0..8 {
        let batch =
            NativeRaycast
                .next_batch(&mut cursor)
                .map_err(|_| ServerError::InvalidInput {
                    field: "combat_actor",
                })?;
        for cell in batch.records() {
            let pos = BlockPos::new(cell.cell[0], cell.cell[1], cell.cell[2]);
            let Some(observed) = view.observation(dimension, pos) else {
                return Ok(false);
            };
            if target_block(view, dimension, pos, observed.block) {
                return Ok(cell.distance >= target_distance);
            }
        }
        if batch.is_done() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn settle_frame(
    ctx: &mut TickContext<'_>,
    frame: CombatFrame,
) -> Result<CombatOutcome, ServerError> {
    if frame.tick != ctx.read().tick() {
        return Err(ServerError::InvalidInput {
            field: "combat_tick",
        });
    }
    for actor in &frame.actors {
        let Some(mut live) = ctx.read().actor(actor.key).cloned() else {
            continue;
        };
        if live.dimension != actor.dimension {
            continue;
        }
        match (&mut live.body, actor.key) {
            (ActorBody::Hostile(body), ActorKey::Hostile(_)) => {
                body.attack_cooldown = actor.attack_cooldown as u8;
                body.hurt_cooldown = actor.hurt_cooldown as u8;
                ctx.stage(RuleEffect::Actor(live))
                    .map_err(|_| ServerError::Internal {
                        invariant: "combat cooldown stage",
                    })?;
            }
            (ActorBody::Player(_), ActorKey::Player(_))
                if live.lifecycle == ActorLifecycle::Active =>
            {
                if let Some(mut runtime) = ctx.read().runtime(actor.key).cloned() {
                    runtime.attack_cooldown = actor.attack_cooldown;
                    runtime.hurt_cooldown = actor.hurt_cooldown;
                    ctx.stage(RuleEffect::Runtime(runtime))
                        .map_err(|_| ServerError::Internal {
                            invariant: "combat cooldown stage",
                        })?;
                }
            }
            _ => {}
        }
    }
    let mut reserved = Vec::with_capacity(frame.intents.len());
    let mut damaged_players = Vec::new();
    let mut applied = 0;
    let mut rejected = 0;
    for intent in frame.intents {
        if reserved.contains(&intent.victim.key) {
            rejected += 1;
            continue;
        }
        reserved.push(intent.victim.key);
        if settle_intent(ctx, &intent)? {
            applied += 1;
            if let ActorKey::Player(key) = intent.victim.key {
                damaged_players.push(key);
            }
        } else {
            rejected += 1;
        }
    }
    Ok(CombatOutcome {
        report: PhaseReport {
            examined: frame.actors.len(),
            applied,
            carried: 0,
            rejected,
        },
        damaged_players,
    })
}

fn settle_intent(ctx: &mut TickContext<'_>, intent: &Intent) -> Result<bool, ServerError> {
    let view = ctx.read();
    let Some(mut attacker) = view.actor(intent.attacker.key).cloned() else {
        return Ok(false);
    };
    let Some(mut victim) = view.actor(intent.victim.key).cloned() else {
        return Ok(false);
    };
    if !same_body_identity(&attacker.body, intent.attacker.body_identity)
        || !same_body_identity(&victim.body, intent.victim.body_identity)
    {
        return Ok(false);
    }
    if attacker.dimension != intent.attacker.dimension
        || victim.dimension != intent.victim.dimension
        || attacker.dimension != victim.dimension
    {
        return Ok(false);
    }
    if matches!(attacker.key, ActorKey::Player(_)) && attacker.lifecycle != ActorLifecycle::Active
        || matches!(victim.key, ActorKey::Player(_)) && victim.lifecycle != ActorLifecycle::Active
    {
        return Ok(false);
    }
    if !matches!(
        (&attacker.body, attacker.key),
        (ActorBody::Player(_), ActorKey::Player(_)) | (ActorBody::Hostile(_), ActorKey::Hostile(_))
    ) || !matches!(
        (&victim.body, victim.key),
        (ActorBody::Player(_), ActorKey::Player(_))
            | (ActorBody::Hostile(_), ActorKey::Hostile(_))
            | (ActorBody::Passive(_), ActorKey::Passive(_))
    ) {
        return Ok(false);
    }
    let mut parts = Vec::with_capacity(6);
    let mut attacker_runtime = None;
    let mut attacker_inventory = None;
    if intent.player_attack {
        let Some(inventory) = view.inventory(attacker.key).copied() else {
            return Ok(false);
        };
        let slot = usize::from(inventory.selected.get());
        let stack = inventory.slots[slot];
        if slot != intent.attacker.selected_slot
            || stack.item != intent.attacker.selected.item
            || stack.count != intent.attacker.selected.count
        {
            return Ok(false);
        }
        let Some(runtime) = view.runtime(attacker.key).cloned() else {
            return Ok(false);
        };
        attacker_runtime = Some(runtime);
        attacker_inventory = Some(inventory);
    }
    let target_kind = match victim.key {
        ActorKey::Player(_) => CombatTarget::Player,
        ActorKey::Hostile(_) => CombatTarget::Hostile,
        ActorKey::Passive(_) => CombatTarget::Passive,
        ActorKey::Companion(_) => return Ok(false),
    };
    let mut target_inventory = None;
    let mut target_runtime = None;
    let impulse = knockback(
        intent.attacker.position,
        intent.victim.position,
        intent.attacker.yaw,
    );
    let old = victim.motion;
    let velocity = old.velocity().get();
    let moved = [
        velocity[0] + impulse[0],
        velocity[1],
        velocity[2] + impulse[2],
    ];
    victim.motion = MotionState::new(MotionStateParts {
        position: old.position(),
        velocity: FiniteVec3::try_new(moved).map_err(|_| ServerError::Internal {
            invariant: "combat knockback",
        })?,
        on_ground: old.on_ground(),
    });
    let health = match &mut victim.body {
        ActorBody::Player(body) => {
            let Some(before) = view.inventory(victim.key).copied() else {
                return Ok(false);
            };
            let Some(mut runtime) = view.runtime(victim.key).cloned() else {
                return Ok(false);
            };
            let mut after = before;
            let damage = inventory::settle_damage(
                DamageCause::Melee,
                i32::from(intent.damage),
                intent.victim.armor_points,
                &mut after.armor,
            ) as u8;
            let health = victim.survival.health().saturating_sub(damage);
            body.health = health;
            body.armor = after.armor;
            runtime.since_damage_ticks = 0;
            runtime.eating = None;
            runtime.bow = None;
            if intent.player_attack {
                runtime.hurt_cooldown = 10;
            } else {
                runtime.hurt_cooldown = 20;
            }
            target_inventory = Some((before, after));
            target_runtime = Some(runtime);
            health
        }
        ActorBody::Hostile(body) => {
            let health = victim.survival.health().saturating_sub(intent.damage);
            body.health = health;
            body.velocity = moved;
            body.hurt_cooldown = 10;
            health
        }
        ActorBody::Passive(body) => {
            let Some(mut runtime) = view.runtime(victim.key).cloned() else {
                return Ok(false);
            };
            let ActorAux::Passive {
                flee_ticks,
                flee_from,
                graze_ticks,
                ..
            } = &mut runtime.aux
            else {
                return Ok(false);
            };
            *flee_ticks = 60;
            *flee_from = Some(FiniteVec3::try_new(intent.attacker.position).map_err(|_| {
                ServerError::Internal {
                    invariant: "combat flee source",
                }
            })?);
            *graze_ticks = 0;
            let health = victim.survival.health().saturating_sub(intent.damage);
            body.health = health;
            body.velocity = moved;
            target_runtime = Some(runtime);
            health
        }
        ActorBody::Companion(_) => return Ok(false),
    };
    victim.survival = survival(
        victim.survival,
        health,
        target_inventory
            .as_ref()
            .map_or(victim.survival.armor_points(), |(_, after)| {
                inventory::armor_points(&after.armor)
            }),
    )?;
    parts.push(RuleEffect::Actor(victim));
    if let Some((before, after)) = target_inventory {
        parts.push(RuleEffect::Inventory(InventoryPatch {
            actor: intent.victim.key,
            before,
            after,
        }));
    }
    if let Some(runtime) = target_runtime {
        parts.push(RuleEffect::Runtime(runtime));
    }
    match (&mut attacker.body, intent.attacker.key) {
        (ActorBody::Player(body), ActorKey::Player(_)) => {
            let mut runtime = attacker_runtime.ok_or(ServerError::Internal {
                invariant: "combat attacker runtime",
            })?;
            let before = attacker_inventory.ok_or(ServerError::Internal {
                invariant: "combat attacker inventory",
            })?;
            let mut after = before;
            runtime.attack_cooldown = 10;
            let (hunger, saturation, exhaustion) = player_survival::exhausted_state(
                attacker.survival.hunger(),
                runtime.saturation_milli.min(u32::from(u16::MAX)) as u16,
                runtime.exhaustion_milli.min(u32::from(u16::MAX)) as u16,
                100,
                view.environment()
                    .ok_or(ServerError::Internal {
                        invariant: "combat environment",
                    })?
                    .tunables
                    .exhaustion_threshold_milli(),
            );
            runtime.exhaustion_milli = u32::from(exhaustion);
            runtime.saturation_milli = u32::from(saturation);
            body.hunger = hunger;
            body.saturation_milli = saturation;
            body.exhaustion_milli = exhaustion;
            attacker.survival = SurvivalState::try_new(SurvivalStateParts {
                health: attacker.survival.health(),
                oxygen: attacker.survival.oxygen(),
                hunger,
                saturation_zero: saturation == 0,
                armor_points: attacker.survival.armor_points(),
            })
            .map_err(|_| ServerError::Internal {
                invariant: "combat exhaustion",
            })?;
            if matches!(intent.attacker.selected.item, 47..=49)
                && after.slots[intent.attacker.selected_slot].count == 1
            {
                let sword = &mut after.slots[intent.attacker.selected_slot];
                if sword.durability > 1 {
                    sword.durability -= 1;
                } else {
                    sword.item += 3;
                    sword.durability = 0;
                }
            }
            parts.push(RuleEffect::Actor(attacker));
            parts.push(RuleEffect::Inventory(InventoryPatch {
                actor: intent.attacker.key,
                before,
                after,
            }));
            parts.push(RuleEffect::Runtime(runtime));
        }
        (ActorBody::Hostile(body), ActorKey::Hostile(_)) => {
            body.attack_cooldown = 20;
            parts.push(RuleEffect::Actor(attacker));
        }
        _ => return Ok(false),
    }
    ctx.stage(RuleEffect::Compound(parts))
        .map_err(|_| ServerError::Internal {
            invariant: "combat compound stage",
        })?;
    if intent.player_attack {
        ctx.suppress_mining(intent.attacker.key)?;
        if ctx.read().tick() != 0 {
            let ActorKey::Player(session) = intent.attacker.key else {
                unreachable!()
            };
            let hit = CombatHit::try_new(ctx.read().tick(), intent.damage, target_kind).map_err(
                |_| ServerError::Internal {
                    invariant: "combat hit event",
                },
            )?;
            ctx.emit(RoutedEvent::new(
                EventRecipient::Session(session.get()),
                Event::CombatHit(hit),
            ))?;
        }
    }
    Ok(true)
}

fn same_body_identity(body: &ActorBody, frozen: BodyIdentity) -> bool {
    matches!((body, frozen),
        (ActorBody::Player(body), BodyIdentity::Player(id)) if body.player_id == id
    ) || matches!((body, frozen),
        (ActorBody::Hostile(body), BodyIdentity::Mob(id)) if body.id == id
    ) || matches!((body, frozen),
        (ActorBody::Passive(body), BodyIdentity::Mob(id)) if body.id == id
    )
}

fn survival(
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
    .map_err(|_| ServerError::Internal {
        invariant: "combat survival",
    })
}

fn knockback(from: [f32; 3], to: [f32; 3], yaw: f32) -> [f32; 3] {
    let mut delta = [to[0] - from[0], to[2] - from[2]];
    if delta[0] == 0.0 && delta[1] == 0.0 {
        let look = look_direction(yaw, 0.0);
        delta = [look[0], look[2]];
    }
    let length = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
    [delta[0] / length * 0.35, 0.0, delta[1] / length * 0.35]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contracts::{
        ActorRecord, ActorRuntime, EnvironmentState, HostileMeleeAttack, InventoryRecord,
        RuleTunables, ServerLimits, TickBudget,
    };
    use crate::core::state::AuthorityState;
    use mornlea_domain::{HostileId, LookAngles, Weather};
    use mornlea_storage::{HostileMob, Inventory, PlayerLocation, PlayerSave};

    fn unit_authority() -> AuthorityState {
        let mut state = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
            7,
        )
        .expect("authority");
        state.advance_tick(TickBudget::full()).expect("advance");
        state
    }

    fn unit_motion(position: [f32; 3]) -> MotionState {
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("position"),
            velocity: FiniteVec3::try_new([0.0; 3]).expect("velocity"),
            on_ground: true,
        })
    }

    fn unit_survival(health: u8) -> SurvivalState {
        SurvivalState::try_new(SurvivalStateParts {
            health,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .expect("survival")
    }

    fn unit_hostile(id: u64, position: [f32; 3]) -> ActorRecord {
        ActorRecord::try_new(
            ActorKey::Hostile(HostileId::try_new(id).expect("hostile id")),
            ActorLifecycle::Active,
            Dimension::OVERWORLD,
            unit_motion(position),
            LookAngles::try_new(0.0, 0.0).expect("look"),
            unit_survival(20),
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
                kind: WALKER_KIND,
            }),
        )
        .expect("hostile actor")
    }

    fn unit_player(key: SessionKey, position: [f32; 3]) -> ActorRecord {
        ActorRecord::try_new(
            ActorKey::Player(key),
            ActorLifecycle::Active,
            Dimension::OVERWORLD,
            unit_motion(position),
            LookAngles::try_new(0.0, 0.0).expect("look"),
            unit_survival(20),
            ActorBody::Player(PlayerSave {
                player_id: mornlea_storage::PlayerId::from_bytes([1; 16]),
                revision: 1,
                display_name: "Melee".into(),
                current: PlayerLocation {
                    dimension: 0,
                    position,
                },
                yaw: 0.0,
                pitch: 0.0,
                safe: None,
                inventory: Inventory::default(),
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
        .expect("player actor")
    }

    fn unit_runtime(key: ActorKey, aux: ActorAux) -> ActorRuntime {
        ActorRuntime {
            key,
            controls: None,
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
            aux,
        }
    }

    fn unit_environment(ctx: &mut TickContext<'_>) {
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
    }

    #[test]
    fn reach_and_parallel_box() {
        assert_eq!(
            aabb_distance([0.5, 2.62, 0.5], [0.0, 0.0, -1.0], [0.5, 1.0, -2.8]),
            Some(3.0)
        );
        assert_eq!(
            aabb_distance([0.5, 2.62, 0.5], [0.0, 0.0, -1.0], [1.5, 1.0, -2.8]),
            None
        );
    }

    /// Lowered actor ceiling refuses before any snapshot work: two staged
    /// actors against a ceiling of one report the selected ceiling and leave
    /// cooldowns, inventory, events and suppression untouched.
    #[test]
    fn actor_ceiling_one_refuses_two_actors() {
        let mut state = unit_authority();
        let mut ctx = TickContext::harness(&mut state, TickBudget::full());
        unit_environment(&mut ctx);
        let attacker = HostileId::try_new(7).expect("hostile id");
        let victim = SessionKey::from_raw(9).expect("session");
        ctx.stage(RuleEffect::Actor(unit_hostile(7, [0.5, 1.0, 0.5])))
            .expect("hostile");
        ctx.stage(RuleEffect::Runtime(unit_runtime(
            ActorKey::Hostile(attacker),
            ActorAux::Hostile {
                distant_ticks: 0,
                shoot_cooldown: 0,
                fresh: false,
            },
        )))
        .expect("hostile runtime");
        ctx.stage(RuleEffect::Actor(unit_player(victim, [1.5, 1.0, 0.5])))
            .expect("player");
        ctx.stage(RuleEffect::Runtime(unit_runtime(
            ActorKey::Player(victim),
            ActorAux::Player {
                respawn: None,
                workbench: None,
            },
        )))
        .expect("player runtime");
        ctx.preload_inventory(ActorKey::Player(victim), InventoryRecord::empty());
        let batch = HostileMeleeBatch::try_new(ctx.read().tick(), &[]).expect("batch");
        let before_attack = ctx
            .read()
            .actor(ActorKey::Hostile(attacker))
            .expect("hostile")
            .clone();
        let before_runtime = ctx
            .read()
            .runtime(ActorKey::Player(victim))
            .expect("runtime")
            .clone();
        let Err(error) = freeze_with_limits(&ctx, &batch, 1, MAX_INTENTS) else {
            panic!("actor ceiling must refuse");
        };
        assert_eq!(
            error,
            ServerError::Capacity {
                resource: Resource::RuleEffects,
                limit: 1,
                observed: 2,
            }
        );
        assert_eq!(
            ctx.read()
                .actor(ActorKey::Hostile(attacker))
                .expect("hostile"),
            &before_attack
        );
        assert_eq!(
            ctx.read()
                .runtime(ActorKey::Player(victim))
                .expect("runtime"),
            &before_runtime
        );
        assert!(ctx.events().is_empty());
        assert!(!ctx.mining_suppressed(ActorKey::Player(victim)));
    }

    /// Lowered intent ceiling refuses a valid attack before any commit: the
    /// single hostile intent against a zero ceiling reports the selected
    /// ceiling and leaves cooldowns, inventory, events and suppression
    /// untouched.
    #[test]
    fn intent_ceiling_zero_refuses_one_attack() {
        let mut state = unit_authority();
        let mut ctx = TickContext::harness(&mut state, TickBudget::full());
        unit_environment(&mut ctx);
        let attacker = HostileId::try_new(7).expect("hostile id");
        let victim = SessionKey::from_raw(9).expect("session");
        ctx.stage(RuleEffect::Actor(unit_hostile(7, [0.5, 1.0, 0.5])))
            .expect("hostile");
        ctx.stage(RuleEffect::Runtime(unit_runtime(
            ActorKey::Hostile(attacker),
            ActorAux::Hostile {
                distant_ticks: 0,
                shoot_cooldown: 0,
                fresh: false,
            },
        )))
        .expect("hostile runtime");
        ctx.stage(RuleEffect::Actor(unit_player(victim, [1.5, 1.0, 0.5])))
            .expect("player");
        ctx.stage(RuleEffect::Runtime(unit_runtime(
            ActorKey::Player(victim),
            ActorAux::Player {
                respawn: None,
                workbench: None,
            },
        )))
        .expect("player runtime");
        ctx.preload_inventory(ActorKey::Player(victim), InventoryRecord::empty());
        let batch = HostileMeleeBatch::try_new(
            ctx.read().tick(),
            &[HostileMeleeAttack::new(attacker, victim)],
        )
        .expect("batch");
        let Err(error) = freeze_with_limits(&ctx, &batch, MAX_ACTORS, 0) else {
            panic!("intent ceiling must refuse");
        };
        assert_eq!(
            error,
            ServerError::Capacity {
                resource: Resource::RuleEffects,
                limit: 0,
                observed: 1,
            }
        );
        let ActorBody::Hostile(body) = &ctx
            .read()
            .actor(ActorKey::Hostile(attacker))
            .expect("hostile")
            .body
        else {
            unreachable!()
        };
        assert_eq!((body.attack_cooldown, body.hurt_cooldown), (0, 0));
        let runtime = ctx
            .read()
            .runtime(ActorKey::Player(victim))
            .expect("runtime");
        assert_eq!((runtime.attack_cooldown, runtime.hurt_cooldown), (0, 0));
        assert_eq!(
            ctx.read()
                .actor(ActorKey::Player(victim))
                .expect("player")
                .survival
                .health(),
            20
        );
        assert!(ctx.events().is_empty());
        assert!(!ctx.mining_suppressed(ActorKey::Player(victim)));
    }
}
