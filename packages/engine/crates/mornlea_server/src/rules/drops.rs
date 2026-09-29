//! Active Ready drop aging and atomic pickup settlement.
//!
//! The reducer owns the bounded active-interest set and item producers. This
//! provider only ages stored slots and transfers their existing quantity into
//! active player inventories through the accepted staging contract.

use crate::core::contracts::{
    ActorKey, ActorLifecycle, ChunkKey, InventoryPatch, PhaseReport, Resource, RuleCall,
    RuleEffect, RulePhase, ServerError, SessionKey,
};
use crate::core::state::TickContext;
use crate::rules::crafting::{add_stack, can_repack};
use mornlea_domain::Dimension;

const MAX_ACTIVE_KEYS: usize = 200;
const MAX_ACTIVE_PLAYERS: usize = 8;

/// The batch call validates its shape; the reducer passes active Ready keys
/// separately because the frozen call record has no interest-set field.
pub fn run(_ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::DropStep
        || call.actor.is_some()
        || call.command.is_some()
        || call.internal.is_some()
    {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    Ok(PhaseReport {
        examined: 0,
        applied: 0,
        carried: 0,
        rejected: 0,
    })
}

/// Advance each initially active physical slot once. Counter changes are
/// deliberately non-durable until a later dirty chunk save includes them.
pub fn advance(ctx: &mut TickContext<'_>, active: &[ChunkKey]) -> Result<PhaseReport, ServerError> {
    if active.len() > MAX_ACTIVE_KEYS {
        return Err(ServerError::Capacity {
            resource: Resource::ChunkResults,
            limit: MAX_ACTIVE_KEYS,
            observed: active.len(),
        });
    }
    if active.is_empty() {
        return Ok(PhaseReport {
            examined: 0,
            applied: 0,
            carried: 0,
            rejected: 0,
        });
    }
    let (lifetime, range, mut players) = {
        let view = ctx.read();
        let environment = view
            .environment()
            .ok_or(ServerError::InvalidInput { field: "drop_step" })?;
        let players: Vec<(SessionKey, Dimension, [f32; 3])> = view
            .actors()
            .iter()
            .filter_map(|actor| match (actor.key, actor.lifecycle) {
                (ActorKey::Player(session), ActorLifecycle::Active) => {
                    Some((session, actor.dimension, actor.motion.position().get()))
                }
                _ => None,
            })
            .collect();
        (
            environment.tunables.drop_lifetime_ticks(),
            environment.tunables.drop_pickup_range(),
            players,
        )
    };
    if players.len() > MAX_ACTIVE_PLAYERS {
        return Err(ServerError::Capacity {
            resource: Resource::Players,
            limit: MAX_ACTIVE_PLAYERS,
            observed: players.len(),
        });
    }
    players.sort_unstable_by_key(|player| player.0);
    let mut keys = active.to_vec();
    keys.sort_unstable();
    keys.dedup();
    let mut report = PhaseReport {
        examined: 0,
        applied: 0,
        carried: 0,
        rejected: 0,
    };
    for key in keys {
        let records = {
            let view = ctx.read();
            if !view.ready_chunk(key) {
                continue;
            }
            view.drops(key).to_vec()
        };
        for initial in records {
            report.examined += 1;
            let mut aged = initial.clone();
            aged.pickup_delay = aged.pickup_delay.saturating_sub(1);
            aged.age = aged.age.wrapping_add(1);
            if aged.age >= lifetime {
                ctx.stage(RuleEffect::DropPatch {
                    before: initial,
                    after: None,
                })
                .map_err(|_| ServerError::InvalidInput { field: "drop_step" })?;
                report.applied += 1;
                continue;
            }
            ctx.stage(RuleEffect::DropPatch {
                before: initial,
                after: Some(aged.clone()),
            })
            .map_err(|_| ServerError::InvalidInput { field: "drop_step" })?;
            if aged.pickup_delay != 0 {
                continue;
            }
            let center = aged.position.get();
            let mut changed = false;
            for &(session, dimension, position) in &players {
                if dimension != key.dimension {
                    continue;
                }
                let dx = center[0] - position[0];
                let dy = center[1] - position[1];
                let dz = center[2] - position[2];
                if (dx * dx + dy * dy + dz * dz).sqrt() > range {
                    continue;
                }
                let actor = ActorKey::Player(session);
                let before = match ctx.read().inventory(actor) {
                    Some(record) => *record,
                    None => continue,
                };
                let (slots, remainder) = add_stack(&before.slots, aged.stack);
                let taken = aged.stack.count - remainder.count;
                if taken == 0 || !can_repack(&slots, &before.crafting) {
                    continue;
                }
                let mut after = before;
                after.slots = slots;
                let patch = InventoryPatch::try_new(actor, before, after)
                    .map_err(|_| ServerError::InvalidInput { field: "drop_step" })?;
                let next = if remainder.count == 0 {
                    None
                } else {
                    let mut reduced = aged.clone();
                    reduced.stack = remainder;
                    Some(reduced)
                };
                ctx.stage(RuleEffect::Compound(vec![
                    RuleEffect::Inventory(patch),
                    RuleEffect::DropPatch {
                        before: aged.clone(),
                        after: next.clone(),
                    },
                ]))
                .map_err(|_| ServerError::InvalidInput { field: "drop_step" })?;
                changed = true;
                match next {
                    Some(reduced) => aged = reduced,
                    None => break,
                }
            }
            if changed {
                report.applied += 1;
            }
        }
    }
    Ok(report)
}
