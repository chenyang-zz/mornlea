//! Checked player drop commands, Ready drop aging and atomic pickup settlement.
//!
//! The reducer owns the bounded active-interest set and item producers. This
//! provider debits authoritative player stacks and transfers existing world
//! quantities into active inventories through the accepted staging contract.

use crate::core::command_outcome::{CommandDisposition, CommandResult};
use crate::core::contracts::{
    ActorKey, ActorLifecycle, ChunkKey, DropBatch, DropSource, InventoryPatch, PhaseReport,
    Resource, RuleCall, RuleEffect, RulePhase, RuleReject, ServerError, SessionKey,
};
use crate::core::state::TickContext;
use crate::rules::crafting::{add_stack, can_repack, grid_extent, set_view_slot, view_slot};
use mornlea_domain::{ChunkPos, Command, CommandEnvelope, Dimension, RejectReason, StackView};
use mornlea_storage::ItemStack;

const MAX_ACTIVE_KEYS: usize = 200;
const MAX_ACTIVE_PLAYERS: usize = 8;

/// The batch call validates its shape; the reducer passes active Ready keys
/// separately because the frozen call record has no interest-set field.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.actor.is_some() || call.internal.is_some() {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    match (call.phase, call.command) {
        (RulePhase::Interaction, Some(command)) => {
            settle_command(ctx, command).map_err(|_| ServerError::InvalidInput {
                field: "drop_command",
            })
        }
        (RulePhase::DropStep, None) => Ok(PhaseReport {
            examined: 0,
            applied: 0,
            carried: 0,
            rejected: 0,
        }),
        _ => Err(ServerError::InvalidInput { field: "phase" }),
    }
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
                // Publication observes the final inventory even if an earlier bite cancels its diff.
                ctx.record_inventory_publication_dirty(session);
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

/// Keep physical capacity refusals distinct from invalid trusted preparation.
/// Raw callers recover their original typed error; live callers retain provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayerDropFailure {
    Refused(RejectReason),
    Trusted(RuleReject),
}

impl PlayerDropFailure {
    fn into_raw(self) -> RuleReject {
        match self {
            Self::Refused(reason) => RuleReject::Wire(reason),
            Self::Trusted(error) => error,
        }
    }
}

/// Prepare one foot-position output without debiting its source. Container
/// settlement reuses this check, then atomically stages its own source debit.
pub(crate) fn prepare_player_drop_checked(
    ctx: &TickContext<'_>,
    session: SessionKey,
    sequence: u64,
    stack: ItemStack,
) -> Result<DropBatch, PlayerDropFailure> {
    prepare_player_drop_inner(ctx, session, sequence, stack, true)
}

// Panel settlement uses current state after admission; recovery does not repeat its Active gate.
fn prepare_player_drop_inner(
    ctx: &TickContext<'_>,
    session: SessionKey,
    sequence: u64,
    stack: ItemStack,
    require_active: bool,
) -> Result<DropBatch, PlayerDropFailure> {
    let view = ctx.read();
    let actor = view
        .actor(ActorKey::Player(session))
        .filter(|actor| !require_active || actor.lifecycle == ActorLifecycle::Active)
        .ok_or(PlayerDropFailure::Refused(RejectReason::PlayerNotReady))?;
    let origin = actor.motion.position();
    // Check in f64 before narrowing: the f32 representation of i32::MAX
    // rounds outside the integer range and must never saturate into a chunk.
    let feet = origin.get().map(|value| f64::from(value).floor());
    if feet
        .iter()
        .any(|value| *value < f64::from(i32::MIN) || *value > f64::from(i32::MAX))
        || !(-64.0..320.0).contains(&feet[1])
    {
        return Err(PlayerDropFailure::Refused(RejectReason::ChunkNotReady));
    }
    let key = ChunkKey {
        dimension: actor.dimension,
        pos: ChunkPos::new((feet[0] as i32) >> 4, (feet[2] as i32) >> 4),
    };
    if !view.ready_chunk(key) {
        return Err(PlayerDropFailure::Refused(RejectReason::ChunkNotReady));
    }
    let environment = view
        .environment()
        .ok_or(PlayerDropFailure::Trusted(RuleReject::Wire(
            RejectReason::InvalidInput,
        )))?;
    let batch = DropBatch::try_new(
        DropSource::Panel { session, sequence },
        actor.dimension,
        origin,
        vec![stack],
        environment.tunables.player_drop_pickup_delay_ticks(),
    )
    .map_err(PlayerDropFailure::Trusted)?;
    view.check_drop_batch(&batch).map_err(|error| match error {
        RuleReject::Wire(RejectReason::DropCapacity) => {
            PlayerDropFailure::Refused(RejectReason::DropCapacity)
        }
        other => PlayerDropFailure::Trusted(other),
    })?;
    Ok(batch)
}

/// Preserve the typed raw drop contract while the live adapter retains provenance.
pub fn settle_command(
    ctx: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
) -> Result<PhaseReport, RuleReject> {
    settle_owned(ctx, envelope, true).map_err(PlayerDropFailure::into_raw)
}

fn owns_inline(command: Command) -> bool {
    matches!(command, Command::DropSelectedItem)
        || matches!(command, Command::DropStack(source)
            if matches!(source.view(), StackView::Inventory | StackView::Crafting))
}

/// Live owner absence is an invariant; lifecycle refusal remains ordinary.
fn check_inline_owner(
    ctx: &TickContext<'_>,
    envelope: &CommandEnvelope,
    require_active: bool,
) -> Result<bool, ServerError> {
    let session = SessionKey::from_raw(envelope.session()).ok_or(ServerError::Internal {
        invariant: "inline drop session",
    })?;
    let actor = ActorKey::Player(session);
    let view = ctx.read();
    let Some(record) = view.actor(actor) else {
        return if require_active {
            Ok(false)
        } else {
            Err(ServerError::Internal {
                invariant: "inline drop owner",
            })
        };
    };
    if require_active && record.lifecycle != ActorLifecycle::Active {
        return Ok(false);
    }
    if view.inventory(actor).is_none() {
        return Err(ServerError::Internal {
            invariant: "inline drop owner",
        });
    }
    Ok(true)
}

/// Cheap grid extent validation precedes actors; debits wait for Interaction.
pub(crate) fn admit_inline(ctx: &mut TickContext<'_>, envelope: &CommandEnvelope) -> CommandResult {
    if !owns_inline(envelope.command()) {
        return Ok(CommandDisposition::Unowned);
    }
    if !check_inline_owner(ctx, envelope, true)? {
        return Ok(CommandDisposition::Refused(RejectReason::PlayerNotReady));
    }
    if let Command::DropStack(source) = envelope.command()
        && source.view() == StackView::Crafting
        && source.slot() < 9
    {
        let session = SessionKey::from_raw(envelope.session()).expect("checked session");
        let extent = grid_extent(
            ctx.read()
                .inventory(ActorKey::Player(session))
                .expect("checked owner")
                .crafting_size,
        );
        if source.slot() >= extent * extent {
            return Ok(CommandDisposition::Refused(RejectReason::InvalidSlot));
        }
    }
    ctx.defer(*envelope, RulePhase::Interaction)?;
    Ok(CommandDisposition::Settled(PhaseReport {
        examined: 1,
        applied: 0,
        carried: 1,
        rejected: 0,
    }))
}

/// Only this provider owns inline drops; trusted preparation never becomes refusal.
pub(crate) fn settle_inline_command(
    ctx: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
) -> CommandResult {
    if !owns_inline(envelope.command()) {
        return Ok(CommandDisposition::Unowned);
    }
    let require_active = matches!(envelope.command(), Command::DropSelectedItem);
    if !check_inline_owner(ctx, envelope, require_active)? {
        return Ok(CommandDisposition::Refused(RejectReason::PlayerNotReady));
    }
    match settle_owned(ctx, envelope, require_active) {
        Ok(report) => Ok(CommandDisposition::Settled(report)),
        Err(PlayerDropFailure::Refused(reason)) => Ok(CommandDisposition::Refused(reason)),
        Err(PlayerDropFailure::Trusted(_)) => Err(ServerError::Internal {
            invariant: "inline drop staging",
        }),
    }
}

/// Settle one admitted player's selected or inline panel drop. The current
/// source is read at settlement, so preceding commands cannot mint stale items.
fn settle_owned(
    ctx: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
    require_active: bool,
) -> Result<PhaseReport, PlayerDropFailure> {
    let session = SessionKey::from_raw(envelope.session()).ok_or(PlayerDropFailure::Trusted(
        RuleReject::Wire(RejectReason::InvalidInput),
    ))?;
    let actor = ActorKey::Player(session);
    let view = ctx.read();
    if !view
        .actor(actor)
        .is_some_and(|record| !require_active || record.lifecycle == ActorLifecycle::Active)
    {
        return Err(PlayerDropFailure::Refused(RejectReason::PlayerNotReady));
    }
    let before = *view
        .inventory(actor)
        .ok_or(PlayerDropFailure::Trusted(RuleReject::Wire(
            RejectReason::PlayerNotReady,
        )))?;
    let mut after = before;
    let source = match envelope.command() {
        Command::DropSelectedItem => {
            let slot = usize::from(before.selected.get());
            let mut source = before.slots[slot];
            if source.count == 0 {
                return Err(PlayerDropFailure::Refused(RejectReason::InvalidSlot));
            }
            after.slots[slot].count -= 1;
            if after.slots[slot].count == 0 {
                after.slots[slot] = ItemStack::default();
            }
            source.count = 1;
            source
        }
        Command::DropStack(source) => {
            let slot = usize::from(source.slot());
            match source.view() {
                StackView::Inventory => {
                    let stack = before.slots[slot];
                    after.slots[slot] = ItemStack::default();
                    stack
                }
                StackView::Crafting => {
                    let extent = usize::from(grid_extent(before.crafting_size));
                    if slot < 9 && slot >= extent * extent {
                        return Err(PlayerDropFailure::Refused(RejectReason::InvalidSlot));
                    }
                    let stack = view_slot(&before, slot);
                    set_view_slot(&mut after, slot, ItemStack::default());
                    stack
                }
                StackView::Container(_) => {
                    return Err(PlayerDropFailure::Refused(RejectReason::InvalidInput));
                }
            }
        }
        _ => return Err(PlayerDropFailure::Refused(RejectReason::InvalidInput)),
    };
    if source.count == 0 {
        return Err(PlayerDropFailure::Refused(RejectReason::InvalidSlot));
    }
    if matches!(envelope.command(), Command::DropStack(_))
        && !after.slots.iter().all(ItemStack::is_valid)
    {
        return Err(PlayerDropFailure::Refused(RejectReason::InvalidInput));
    }
    let batch =
        prepare_player_drop_inner(ctx, session, envelope.sequence(), source, require_active)?;
    // A debit frees capacity; repacking crafting inputs would add a credit-only
    // veto absent from the source command contract.
    ctx.stage(RuleEffect::Compound(vec![
        RuleEffect::Inventory(
            InventoryPatch::try_new(actor, before, after).map_err(PlayerDropFailure::Trusted)?,
        ),
        RuleEffect::Drops(batch),
    ]))
    .map_err(PlayerDropFailure::Trusted)?;
    // A grid debit still publishes inventory intent even when the pack is equal.
    ctx.record_inventory_publication_dirty(session);
    if matches!(envelope.command(), Command::DropStack(source)
        if source.view() == StackView::Crafting && source.slot() < 9)
    {
        ctx.record_crafting_publication_dirty(session);
    }
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::command_outcome::CommandDisposition;
    use crate::core::contracts::{ServerLimits, TickBudget};
    use crate::core::state::AuthorityState;
    use mornlea_domain::{CommandEnvelopeParts, ContainerKind, ContainerRef, StackSource};

    #[test]
    fn inline_drop_adapter_ownership_and_readiness() {
        let mut authority = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap();
        let mut context = TickContext::harness(&mut authority, TickBudget::full());
        let make = |command| {
            CommandEnvelope::try_new(CommandEnvelopeParts {
                tick: 0,
                session: 1,
                sequence: 1,
                arrival_index: 1,
                command,
            })
            .unwrap()
        };
        for command in [
            Command::DropSelectedItem,
            Command::DropStack(StackSource::try_new(StackView::Inventory, 0).unwrap()),
            Command::DropStack(StackSource::try_new(StackView::Crafting, 0).unwrap()),
        ] {
            assert_eq!(
                admit_inline(&mut context, &make(command)),
                Ok(CommandDisposition::Refused(RejectReason::PlayerNotReady))
            );
            assert_eq!(
                settle_inline_command(&mut context, &make(command)),
                if matches!(command, Command::DropSelectedItem) {
                    Ok(CommandDisposition::Refused(RejectReason::PlayerNotReady))
                } else {
                    Err(ServerError::Internal {
                        invariant: "inline drop owner",
                    })
                }
            );
        }
        let reference =
            ContainerRef::try_new(ChunkPos::new(0, -1), ContainerKind::Chest, 0, 1).unwrap();
        for command in [
            Command::CloseContainer,
            Command::DropStack(StackSource::try_new(StackView::Container(reference), 36).unwrap()),
        ] {
            assert_eq!(
                admit_inline(&mut context, &make(command)),
                Ok(CommandDisposition::Unowned)
            );
            assert_eq!(
                settle_inline_command(&mut context, &make(command)),
                Ok(CommandDisposition::Unowned)
            );
        }
        assert!(context.events().is_empty());
        assert!(context.deferred(RulePhase::Interaction).is_empty());
    }
}
