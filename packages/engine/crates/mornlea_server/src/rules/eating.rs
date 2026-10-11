//! Atomic eating progression: hold ticks, interrupt precedence and the
//! one-item hunger/saturation settlement.
//!
//! This provider owns the eating phase for one player per call. The hold input
//! arrives as the eating bit of the held controls on the motion-owned runtime
//! lane, and the progress record lives in the same runtime record: eating is
//! transient tick state, never persisted, exactly like the Go `eatingState`
//! that stays out of snapshots and hashes. Settlement stages one atomic
//! compound — the consumed stack, the raised actor hunger and the reset
//! progress — so a refused component leaves the item, the actor and the
//! progress record untouched.
//!
//! Mirrored Go rows, each cited at its site:
//!
//! - `packages/server/sim/entity/eating.go` (`advanceEating`): the release,
//!   reset, non-food, full-hunger and suspended interrupt short-circuit that
//!   clears without consuming; the `(slot, item)` progress key with the
//!   starting tick counting as 1 and a changed key restarting at 1; and the
//!   settlement that consumes one item, raises hunger to `min(20, old+gain)`,
//!   clamps saturation against the updated hunger to
//!   `min(new_hunger*1000, old+gain)` over wide intermediates, and resets
//!   progress in the same step. The interrupt branch precedes the settlement
//!   branch, so an interrupt landing on the settlement tick consumes nothing.
//! - `packages/shared/core/hunger.go` (`FoodValue`): the frozen food table —
//!   bread 5/6000, potato 1/600, carrot 3/3600, poisonous potato 2/1200,
//!   rotten flesh 4/0, raw beef 3/1800, cooked beef 8/12800. The poisonous
//!   potato settles as plain food while the status-effect system is absent,
//!   exactly like the Go row.
//! - `packages/shared/core/item.go` (`Hotbar.Consume`): the one-item
//!   consumption that normalizes an emptied slot back to the zero stack, with
//!   the defensively checked return value.
//! - `packages/shared/tuning/tunables.go` (`EatingTicks` default, also pinned
//!   by `RuleTunables::source_defaults`): the hold length is read once from
//!   the staged tick snapshot, with the source's minimum of one tick.
//! - `packages/server/sim/entity/player.go` (`applyDamage` with the
//!   `advanceActivePlayers` call site): real health loss clears the lane, and
//!   a suspension is an open container view or a view not ready. Both halves
//!   live outside this module: the accepted survival provider clears the lane
//!   on actual damage (resisted damage never reaches it), and this provider
//!   reads the container-view lease and the view-readiness flag from the
//!   runtime lanes. Companions never eat — the phase accepts players only.

use mornlea_domain::{HotbarSlot, SurvivalState, SurvivalStateParts};
use mornlea_storage::ItemStack;

use crate::core::contracts::{
    ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, EatingProgress, InventoryPatch,
    InventoryRecord, PhaseReport, RuleCall, RuleEffect, RulePhase, ServerError, SessionKey,
};
use crate::core::state::TickContext;

/// Authoritative hunger maximum (`core.MaxHunger`,
/// `packages/shared/core/hunger.go`).
const MAX_HUNGER: u8 = 20;

/// One saturation point in milli units (`core.SaturationMilliPerPoint`).
const SATURATION_MILLI_PER_POINT: u32 = 1_000;

/// The absent item number (`core.ItemNone`, `packages/shared/core/item.go`).
const ITEM_NONE: u16 = 0;

/// Stable food item numbers, mirrored from the frozen const block in
/// `packages/shared/core/item.go`.
const ITEM_BREAD: u16 = 36;
const ITEM_POTATO: u16 = 40;
const ITEM_CARROT: u16 = 41;
const ITEM_POISONOUS_POTATO: u16 = 42;
const ITEM_ROTTEN_FLESH: u16 = 45;
const ITEM_RAW_BEEF: u16 = 53;
const ITEM_COOKED_BEEF: u16 = 54;

/// Hunger gain and saturation recovery of one food, the exact `FoodValue`
/// table (`packages/shared/core/hunger.go`); `None` is every non-food item.
/// The poisonous potato carries no status effect yet, mirroring the deferred
/// Go row.
fn food_value(item: u16) -> Option<(u8, u16)> {
    match item {
        ITEM_BREAD => Some((5, 6_000)),
        ITEM_POTATO => Some((1, 600)),
        ITEM_CARROT => Some((3, 3_600)),
        ITEM_POISONOUS_POTATO => Some((2, 1_200)),
        ITEM_ROTTEN_FLESH => Some((4, 0)),
        ITEM_RAW_BEEF => Some((3, 1_800)),
        ITEM_COOKED_BEEF => Some((8, 12_800)),
        _ => None,
    }
}

/// One reported phase step: exactly one actor is examined, and the applied
/// count names whether the staged state moved.
fn step_report(applied: bool) -> PhaseReport {
    PhaseReport {
        examined: 1,
        applied: usize::from(applied),
        carried: 0,
        rejected: 0,
    }
}

/// The eating-phase entry: exactly one player actor settles per call; every
/// other shape refuses without effect.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::Eating {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    if call.command.is_some() || call.internal.is_some() {
        return Err(ServerError::InvalidInput { field: "command" });
    }
    let actor = call
        .actor
        .ok_or(ServerError::InvalidInput { field: "actor" })?;
    let ActorKey::Player(session) = actor else {
        return Err(ServerError::InvalidInput { field: "actor" });
    };
    settle_actor(ctx, actor, session)
}

/// One player eating tick, mirroring `advanceEating` branch for branch: the
/// interrupt short-circuit first, then the `(slot, item)` key, then the
/// atomic settlement.
fn settle_actor(
    ctx: &mut TickContext<'_>,
    actor: ActorKey,
    session: SessionKey,
) -> Result<PhaseReport, ServerError> {
    let view = ctx.read();
    // No staged runtime means no held input and no progress to advance or
    // clear: the empty observation, not a refusal.
    let Some(runtime) = view.runtime(actor).cloned() else {
        return Ok(step_report(false));
    };
    let held = runtime
        .controls
        .is_some_and(|control| control.actions().eating);
    let suspended = view.viewer(session).is_some() || !runtime.has_view;
    // The basis below only feeds advancement, so a missing or inactive actor
    // record or inventory clears progress instead of refusing a wire shape:
    // the reference never sees an absent basis, and the mining provider's
    // unready arm is the established mirror.
    let basis = if held && !runtime.reset && !suspended {
        match (view.actor(actor), view.inventory(actor)) {
            (Some(record), Some(inventory)) if record.lifecycle == ActorLifecycle::Active => {
                Some((record.clone(), *inventory))
            }
            _ => None,
        }
    } else {
        None
    };
    let Some((record, inventory)) = basis else {
        return stage_progress(ctx, runtime, None);
    };
    let slot = inventory.selected;
    let stack = inventory.slots[usize::from(slot.get())];
    let Some((hunger_gain, saturation_gain)) = food_value(stack.item) else {
        return stage_progress(ctx, runtime, None);
    };
    let hunger = record.survival.hunger();
    if hunger >= MAX_HUNGER {
        return stage_progress(ctx, runtime, None);
    }
    // Interrupts clear without advancing. An eligible hold requires the
    // same immutable timing snapshot as the other rule providers.
    let eating_ticks = view
        .environment()
        .ok_or(ServerError::Internal {
            invariant: "eating snapshot",
        })?
        .tunables
        .eating_ticks()
        .max(1);
    // Progress key: a recorded `(slot, item)` pair continuing from a nonzero
    // count increments by exactly one, and anything else restarts at 1 — the
    // starting tick itself. The nonzero guard is the Go empty-state sentinel.
    let ticks = match runtime.eating {
        Some(progress)
            if progress.slot == slot && progress.item == stack.item && progress.ticks != 0 =>
        {
            progress.ticks.saturating_add(1)
        }
        _ => 1,
    };
    if ticks < eating_ticks {
        return stage_progress(
            ctx,
            runtime,
            Some(EatingProgress {
                slot,
                item: stack.item,
                ticks,
            }),
        );
    }
    settle_completion(
        ctx,
        runtime,
        record,
        inventory,
        slot,
        stack,
        hunger,
        hunger_gain,
        saturation_gain,
    )
}

/// Stages the runtime record with one eating lane change. No change reports no
/// work, so a clear over an already empty lane stays silent.
fn stage_progress(
    ctx: &mut TickContext<'_>,
    mut runtime: ActorRuntime,
    eating: Option<EatingProgress>,
) -> Result<PhaseReport, ServerError> {
    const STAGE: ServerError = ServerError::InvalidInput { field: "eating" };
    if runtime.eating == eating {
        return Ok(step_report(false));
    }
    runtime.eating = eating;
    ctx.stage(RuleEffect::Runtime(runtime)).map_err(|_| STAGE)?;
    Ok(step_report(true))
}

/// The atomic settlement at the end of the hold: one item consumed through the
/// `Consume` normalization, hunger and saturation raised over wide
/// intermediates with saturation clamped against the updated hunger, and the
/// progress reset — all staged as one compound effect so any refused
/// component leaves the inventory, the actor and the progress untouched.
#[allow(clippy::too_many_arguments)]
fn settle_completion(
    ctx: &mut TickContext<'_>,
    mut runtime: ActorRuntime,
    record: ActorRecord,
    inventory: InventoryRecord,
    slot: HotbarSlot,
    stack: ItemStack,
    hunger: u8,
    hunger_gain: u8,
    saturation_gain: u16,
) -> Result<PhaseReport, ServerError> {
    const STAGE: ServerError = ServerError::InvalidInput { field: "eating" };
    let index = usize::from(slot.get());
    // `Consume` fails only on an empty or out-of-range slot (`Hotbar.Consume`,
    // `packages/shared/core/item.go`); the food gate above already excludes
    // both, but the return value stays checked exactly like the reference.
    if stack.item == ITEM_NONE || stack.count == 0 {
        return stage_progress(ctx, runtime, None);
    }
    let mut after = inventory;
    let mut consumed = stack;
    consumed.count -= 1;
    if consumed.count == 0 {
        consumed = ItemStack::default();
    }
    after.slots[index] = consumed;
    // Wide intermediates: both sums fit the narrow types only because today's
    // table is small, so the arithmetic widens before clamping, matching the
    // Go comment about silent wraparound on future rows.
    let new_hunger = (u32::from(hunger) + u32::from(hunger_gain)).min(u32::from(MAX_HUNGER));
    let new_hunger = new_hunger as u8;
    let old_saturation = runtime.saturation_milli.min(u32::from(u16::MAX)) as u16;
    let new_saturation = (u32::from(old_saturation) + u32::from(saturation_gain))
        .min(u32::from(new_hunger) * SATURATION_MILLI_PER_POINT) as u16;
    // Hunger first, then the clamp against the updated hunger: clamping
    // against the pre-bite hunger would silently withhold saturation.
    let survival = SurvivalState::try_new(SurvivalStateParts {
        health: record.survival.health(),
        hunger: new_hunger,
        oxygen: record.survival.oxygen(),
        saturation_zero: new_saturation == 0,
        armor_points: record.survival.armor_points(),
    })
    .map_err(|_| STAGE)?;
    let ActorBody::Player(mut save) = record.body.clone() else {
        return Err(ServerError::Internal {
            invariant: "eating settlement body",
        });
    };
    save.hunger = new_hunger;
    save.saturation_milli = new_saturation;
    let settled = ActorRecord::try_new(
        record.key,
        record.lifecycle,
        record.dimension,
        record.motion,
        record.look,
        survival,
        ActorBody::Player(save),
    )
    .map_err(|_| STAGE)?;
    let ActorKey::Player(session) = record.key else {
        return Err(ServerError::Internal {
            invariant: "eating player identity",
        });
    };
    runtime.saturation_milli = u32::from(new_saturation);
    runtime.eating = None;
    let patch = InventoryPatch::try_new(record.key, inventory, after).map_err(|_| STAGE)?;
    ctx.stage(RuleEffect::Compound(vec![
        RuleEffect::Inventory(patch),
        RuleEffect::Actor(settled),
        RuleEffect::Runtime(runtime),
    ]))
    .map_err(|_| STAGE)?;
    // Later pickups can restore these slots; the accepted bite still publishes once.
    ctx.record_inventory_publication_dirty(session);
    Ok(step_report(true))
}
