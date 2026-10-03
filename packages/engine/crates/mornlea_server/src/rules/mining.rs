//! Continuous mining progression, tool wear and atomic completion.
//!
//! This provider owns the mining-phase progression for one actor per call:
//! held-primary resolution after combat precedence, the target/block/tool
//! progress counters, and completion through the accepted atomic transaction.
//! Human intent arrives as held controls on the motion-owned runtime lane;
//! companion intent arrives as sessionless `MineHold` proposals on the view.
//! Neither path accepts a client-supplied target or revision: the ray, the
//! footprint, the wear and the output preflight are all authority derived.
//!
//! Mirrored Go rows, each cited at its site:
//!
//! - `packages/server/sim/entity/mining.go` (`miningRule`): the required-ticks
//!   and harvestability table. Door and bed 15, crop, wild grass and sapling 1,
//!   snow layers 1, the soil group, leaves, glass, wool, clay and snow block 5,
//!   log, planks and workbench 15, the stone tool tiers 30/15/8, the stonebrick
//!   tool tiers 30/15/8 with failure, and iron block 40/20/10 with failure.
//!   Unlisted blocks carry the zero sentinel and never progress.
//! - The same file (`stepMiningProgress`): the progress key rule — an
//!   unchanged target/block/held key increments once per tick to saturation, a
//!   changed key restarts at 1, and released, suspended, unready or
//!   required-zero input clears. Saturation completes in the same tick.
//! - The same file (`advanceMining`, `advanceCompanionMining`,
//!   `completeCompanionMining`): humans clear their attempt even when the
//!   output is rejected, while a companion with a full inventory keeps
//!   saturated progress for retry; a companion hold persists until release; a
//!   companion ray that does not hit exactly its target clears; crops,
//!   farmland, torches, wild grass, snow layers and fluids are never companion
//!   targets.
//! - The same file (`consumeMiningToolDurability`): successful tool use wears
//!   one point with the crop-plus-intact-hoe, wild grass, sapling and
//!   intact-sword exemptions, and durability one becomes the registered broken
//!   form.
//!
//! Deliberate boundaries. The per-tick ray walk below only tracks the progress
//! key; every completion re-resolves through the accepted authority resolvers and
//! commits exactly once through `MutationTxn::try_mine`, so the walk can never
//! settle a block, mint a drop or wear a tool. Short grass completes through
//! the resolver's position-stable seed roll in one tick. Snow layers settle through the human-only
//! clear-only branch of the accepted resolver; companion snow mining stays
//! refusing. Combat precedence is an active bow draw read through the landed
//! runtime lane; the full suppression matrix belongs to the combat and reducer
//! nodes. Container read-back and slot lifecycle stay with the container and
//! drop providers: this node pins only the atomic commit-or-refusal of the
//! resolver footprint.

use mornlea_domain::{BlockPos, CompanionId, Dimension, HotbarSlot, PlayerControl, RejectReason};
use mornlea_engine::native::contracts::raycast::{Ray, RayCursor, RaycastOp};
use mornlea_engine::native::raycast::NativeRaycast;
use mornlea_storage::ItemStack;

use crate::core::contracts::{
    ActorKey, ActorLifecycle, BlockObservation, CompanionAction, MiningProgress, PhaseReport,
    RuleCall, RuleEffect, RulePhase, RuleReject, ServerError,
};
use crate::core::interaction::{look_direction, normalized_direction, target_block};
use crate::core::mutation::{
    is_crop, is_farmland, is_fluid, is_snow_layer, is_torch, is_wild_grass, mining_rule,
    resolve_companion_mine, resolve_mine,
};
use crate::core::state::{AuthorityReadView, TickContext};

/// Outcome of the lightweight progress-tracking ray walk: the first observed
/// non-air cell, no hit within reach, or an unready basis the authority cannot
/// certify. Every arm clears progress; none settles anything.
enum TargetOutcome {
    Hit(BlockObservation),
    Empty,
    Unready,
}

/// Walks the numerical ray kernel batch by batch and classifies every
/// traversed cell against the authority view, the same walk the accepted
/// resolvers perform: an unobserved cell is unready, cells the shared
/// `target_block` classifier passes over continue, and the first target cell
/// is the hit. No hit within reach is empty.
fn walk_target(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    origin: [f32; 3],
    direction: [f32; 3],
    reach: f32,
) -> TargetOutcome {
    let normalized = match normalized_direction(direction) {
        Some(normalized) => normalized,
        None => return TargetOutcome::Unready,
    };
    let mut cursor = match RayCursor::try_new(Ray {
        origin,
        direction: normalized,
        maximum: reach,
    }) {
        Ok(cursor) => cursor,
        Err(_) => return TargetOutcome::Unready,
    };
    loop {
        let batch = match NativeRaycast.next_batch(&mut cursor) {
            Ok(batch) => batch,
            Err(_) => return TargetOutcome::Unready,
        };
        for record in batch.records() {
            let cell = BlockPos::new(record.cell[0], record.cell[1], record.cell[2]);
            match view.observation(dimension, cell) {
                None => return TargetOutcome::Unready,
                Some(observed) if !target_block(view, dimension, cell, observed.block) => {}
                Some(observed) => return TargetOutcome::Hit(observed),
            }
        }
        if batch.is_done() {
            return TargetOutcome::Empty;
        }
    }
}

/// The frozen authority basis one mining tick needs: an active actor with its
/// pose and dimension, the environment tunables, and the actor inventory the
/// held tool reads from.
struct ActorBasis {
    dimension: Dimension,
    eye: [f32; 3],
    reach: f32,
    inventory: crate::core::contracts::InventoryRecord,
}

/// Loads the authority basis or reports it unavailable. A missing actor,
/// environment or inventory record is unready rather than a wire refusal
/// because it is the authority's own basis that is unavailable.
fn actor_basis(view: &AuthorityReadView<'_>, actor: ActorKey) -> Option<ActorBasis> {
    let record = view.actor(actor)?;
    if record.lifecycle != ActorLifecycle::Active {
        return None;
    }
    let environment = view.environment()?;
    let tunables = environment.tunables;
    let position = record.motion.position().get();
    let inventory = *view.inventory(actor)?;
    Some(ActorBasis {
        dimension: record.dimension,
        eye: [
            position[0],
            position[1] + tunables.eye_height(),
            position[2],
        ],
        reach: tunables.interaction_reach(),
        inventory,
    })
}

/// Stages a cleared progress record and reports one examined actor. The
/// applied count names whether a previous record was actually removed.
fn stage_clear(
    ctx: &mut TickContext<'_>,
    actor: ActorKey,
    prior: Option<MiningProgress>,
) -> Result<PhaseReport, ServerError> {
    const STAGE: ServerError = ServerError::InvalidInput { field: "mining" };
    let applied = usize::from(prior.is_some());
    ctx.stage(RuleEffect::Mining {
        actor,
        progress: None,
    })
    .map_err(|_| STAGE)?;
    Ok(PhaseReport {
        examined: 1,
        applied,
        carried: 0,
        rejected: 0,
    })
}

/// Stages one progress record and reports one examined actor. The applied
/// count names whether the staged record differs from the previous one, so a
/// retained saturated record reports no new work.
fn stage_progress(
    ctx: &mut TickContext<'_>,
    record: MiningProgress,
    prior: Option<MiningProgress>,
) -> Result<PhaseReport, ServerError> {
    const STAGE: ServerError = ServerError::InvalidInput { field: "mining" };
    let applied = usize::from(prior.as_ref() != Some(&record));
    ctx.stage(RuleEffect::Mining {
        actor: record.actor,
        progress: Some(record),
    })
    .map_err(|_| STAGE)?;
    Ok(PhaseReport {
        examined: 1,
        applied,
        carried: 0,
        rejected: 0,
    })
}

/// The per-tick progress key and table output one advancement consumes: the
/// ray target, its observed block, the selected slot and stack, and the
/// required ticks the table assigns that block/tool pair.
struct KeyTick {
    target: BlockPos,
    block: u16,
    slot: HotbarSlot,
    stack: ItemStack,
    required: u16,
}

/// Next staged progress and whether this tick completes through the atomic
/// transaction: the frozen key rule (`stepMiningProgress`) with saturation
/// clamped at the required value, so an already-saturated record stays
/// saturated instead of overflowing.
fn advance_progress(
    actor: ActorKey,
    dimension: Dimension,
    key: KeyTick,
    prior: Option<MiningProgress>,
    tick: u64,
) -> (MiningProgress, bool) {
    let held = key.stack.item;
    let elapsed = match &prior {
        Some(previous)
            if previous.target == key.target
                && previous.observed_block == key.block
                && previous.tool.item == held
                && previous.required == u32::from(key.required)
                && previous.required != 0 =>
        {
            if previous.elapsed < previous.required {
                previous.elapsed + 1
            } else {
                previous.required
            }
        }
        _ => 1,
    };
    let record = MiningProgress {
        actor,
        dimension,
        target: key.target,
        observed_block: key.block,
        tool_slot: key.slot,
        tool: key.stack,
        elapsed,
        required: u32::from(key.required),
        last_tick: tick,
    };
    (record, elapsed >= u32::from(key.required))
}

/// Settles one human actor tick: held primary after combat precedence drives
/// the progress key, and saturation completes exactly once through the
/// accepted resolver and transaction. Transaction refusal clears the human
/// record before the collapsed error shape surfaces; receipt capacity refusal
/// precedes the transaction and retains the current attempt.
fn run_human(
    ctx: &mut TickContext<'_>,
    actor: ActorKey,
    control: PlayerControl,
    bow_drawn: bool,
) -> Result<PhaseReport, ServerError> {
    const REFUSAL: ServerError = ServerError::InvalidInput { field: "mining" };
    // Combat precedence: an active bow draw owns the held primary this tick,
    // so mining neither progresses nor completes.
    if bow_drawn || !control.actions().primary {
        let prior = ctx.read().mining(actor).cloned();
        return stage_clear(ctx, actor, prior);
    }
    let view = ctx.read();
    let basis = match actor_basis(&view, actor) {
        Some(basis) => basis,
        None => {
            let prior = view.mining(actor).cloned();
            return stage_clear(ctx, actor, prior);
        }
    };
    let look = control.look();
    let outcome = walk_target(
        &view,
        basis.dimension,
        basis.eye,
        look_direction(look.yaw(), look.pitch()),
        basis.reach,
    );
    let observed = match outcome {
        TargetOutcome::Hit(observed) => observed,
        TargetOutcome::Empty | TargetOutcome::Unready => {
            let prior = view.mining(actor).cloned();
            return stage_clear(ctx, actor, prior);
        }
    };
    let slot = basis.inventory.selected;
    let stack = basis.inventory.slots[usize::from(slot.get())];
    let (required, _harvestable) = mining_rule(observed.block, stack.item);
    let prior = view.mining(actor).cloned();
    let tick = view.tick();
    if required == 0 {
        return stage_clear(ctx, actor, prior);
    }
    let (record, completable) = advance_progress(
        actor,
        basis.dimension,
        KeyTick {
            target: observed.pos,
            block: observed.block,
            slot,
            stack,
            required,
        },
        prior.clone(),
        tick,
    );
    if !completable {
        return stage_progress(ctx, record, prior);
    }
    let outcome = resolve_mine(actor, &control, &ctx.read());
    let resolved = match outcome {
        Ok(Some(resolved)) => resolved,
        Ok(None) => return stage_clear(ctx, actor, prior),
        Err(_) => {
            stage_clear(ctx, actor, prior)?;
            return Err(REFUSAL);
        }
    };
    // Capacity refusal preserves the completed attempt before any transaction.
    ctx.check_charge_capacity()?;
    match ctx.transaction().try_mine(resolved) {
        Ok(_) => {
            stage_clear(ctx, actor, prior)?;
            ctx.note_charge(actor, crate::core::state::ActionKind::Mining)?;
            Ok(PhaseReport {
                examined: 1,
                applied: 1,
                carried: 0,
                rejected: 0,
            })
        }
        Err(_) => {
            stage_clear(ctx, actor, prior)?;
            Err(REFUSAL)
        }
    }
}

/// Settles one companion actor tick: the first valid selected `MineHold`
/// holds until a selected `MineRelease`, the authority ray must hit exactly the
/// proposed cell, and the companion target registry gates accumulation.
/// Saturation completes exactly once through the accepted companion resolver;
/// only a full inventory keeps the saturated record for retry.
fn run_companion(
    ctx: &mut TickContext<'_>,
    actor: ActorKey,
    id: CompanionId,
) -> Result<PhaseReport, ServerError> {
    const REFUSAL: ServerError = ServerError::InvalidInput { field: "mining" };
    let Some(target) = companion_mining_target(ctx, actor, id)? else {
        let prior = ctx.read().mining(actor).cloned();
        return stage_clear(ctx, actor, prior);
    };
    let view = ctx.read();
    let basis = match actor_basis(&view, actor) {
        Some(basis) => basis,
        None => {
            let prior = view.mining(actor).cloned();
            return stage_clear(ctx, actor, prior);
        }
    };
    let prior = view.mining(actor).cloned();
    let tick = view.tick();
    let center = [
        target.x() as f32 + 0.5,
        target.y() as f32 + 0.5,
        target.z() as f32 + 0.5,
    ];
    let outcome = walk_target(
        &view,
        basis.dimension,
        basis.eye,
        [
            center[0] - basis.eye[0],
            center[1] - basis.eye[1],
            center[2] - basis.eye[2],
        ],
        basis.reach,
    );
    let observed = match outcome {
        // A blocked, out-of-reach or superseded ray clears: only the exact
        // proposed cell may accumulate.
        TargetOutcome::Hit(observed) if observed.pos == target => observed,
        TargetOutcome::Hit(_) | TargetOutcome::Empty | TargetOutcome::Unready => {
            return stage_clear(ctx, actor, prior);
        }
    };
    // The companion target registry (`companionMineableBlock`): crops,
    // farmland, torches, wild grass, snow layers and fluids are explicitly
    // refused before any progress accumulates. The drop-table half of that
    // registry is subsumed here because every block with required ticks above
    // zero outside this list carries a registered drop.
    if is_crop(observed.block)
        || is_farmland(observed.block)
        || is_torch(observed.block)
        || is_wild_grass(observed.block)
        || is_snow_layer(observed.block)
        || is_fluid(observed.block)
    {
        return stage_clear(ctx, actor, prior);
    }
    let slot = basis.inventory.selected;
    let stack = basis.inventory.slots[usize::from(slot.get())];
    let (required, _harvestable) = mining_rule(observed.block, stack.item);
    if required == 0 {
        return stage_clear(ctx, actor, prior);
    }
    let (record, completable) = advance_progress(
        actor,
        basis.dimension,
        KeyTick {
            target: observed.pos,
            block: observed.block,
            slot,
            stack,
            required,
        },
        prior.clone(),
        tick,
    );
    if !completable {
        return stage_progress(ctx, record, prior);
    }
    let outcome = resolve_companion_mine(id, target, &ctx.read());
    let resolved = match outcome {
        Ok(resolved) => resolved,
        Err(RuleReject::Wire(RejectReason::HotbarFull)) => {
            // A full companion inventory keeps saturated progress for retry:
            // the block, the wear and the backpack are all untouched. The
            // output preflight lives in the resolver, so its refusal surfaces
            // here rather than at commit.
            stage_progress(ctx, record, prior)?;
            return Err(REFUSAL);
        }
        Err(_) => {
            stage_clear(ctx, actor, prior)?;
            return Err(REFUSAL);
        }
    };
    match ctx.transaction().try_mine(resolved) {
        Ok(_) => {
            stage_clear(ctx, actor, prior)?;
            Ok(PhaseReport {
                examined: 1,
                applied: 1,
                carried: 0,
                rejected: 0,
            })
        }
        Err(_) => {
            stage_clear(ctx, actor, prior)?;
            Err(REFUSAL)
        }
    }
}

/// Shares arrival-order selection with motion and placement while retaining
/// mining intent separately from the fallible progress/settlement lane.
fn companion_mining_target(
    ctx: &mut TickContext<'_>,
    actor: ActorKey,
    id: CompanionId,
) -> Result<Option<BlockPos>, ServerError> {
    let view = ctx.read();
    let Some(record) = view
        .actor(actor)
        .filter(|record| record.lifecycle == ActorLifecycle::Active)
    else {
        return Ok(None);
    };
    let selected = crate::rules::companions::select(&view)
        .0
        .into_iter()
        .find(|(selected_id, _)| *selected_id == id)
        .map(|(_, action)| action);
    let previous = view.runtime(actor);
    let held = match previous.map(|runtime| &runtime.aux) {
        Some(crate::core::contracts::ActorAux::Companion { mining_target, .. }) => *mining_target,
        Some(_) => {
            return Err(ServerError::Internal {
                invariant: "companion mining runtime",
            });
        }
        None => None,
    };
    let target = match selected {
        Some(CompanionAction::MineHold { target }) => Some(target),
        Some(CompanionAction::MineRelease) => None,
        _ => return Ok(held),
    };
    if target == held {
        return Ok(target);
    }
    let mut runtime = previous
        .cloned()
        .unwrap_or_else(|| crate::core::contracts::ActorRuntime {
            key: actor,
            controls: None,
            has_view: false,
            reset: false,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 0,
            oxygen: record.survival.oxygen(),
            peak_y: record.motion.position().get()[1],
            exhaustion_milli: 0,
            saturation_milli: 0,
            since_damage_ticks: 0,
            drown_ticks: 0,
            starvation_ticks: 0,
            eating: None,
            bow: None,
            path: None,
            aux: crate::core::contracts::ActorAux::Companion {
                generation: 0,
                attempt: 0,
                task: Default::default(),
                mining_target: None,
            },
        });
    if let crate::core::contracts::ActorAux::Companion { mining_target, .. } = &mut runtime.aux {
        *mining_target = target;
    }
    ctx.stage(RuleEffect::Runtime(runtime))
        .map_err(|_| ServerError::Internal {
            invariant: "companion mining runtime staging",
        })?;
    Ok(target)
}

/// The mining-phase entry: exactly one ordered human or companion actor
/// settles per call; every other shape refuses without effect.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::MiningStep {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    if call.command.is_some() || call.internal.is_some() {
        return Err(ServerError::InvalidInput { field: "command" });
    }
    let actor = call
        .actor
        .ok_or(ServerError::InvalidInput { field: "actor" })?;
    match actor {
        ActorKey::Player(_) => {
            // Successful bucket use owns this tick's interaction. The receipt
            // lives only in the tick context, so held mining resumes next tick.
            if ctx.mining_suppressed(actor) {
                let prior = ctx.read().mining(actor).cloned();
                return stage_clear(ctx, actor, prior);
            }
            let view = ctx.read();
            let held = view.runtime(actor).and_then(|record| record.controls);
            let bow_drawn = view
                .runtime(actor)
                .is_some_and(|record| record.bow.is_some());
            let control = match held {
                Some(control) => control,
                None => {
                    let prior = ctx.read().mining(actor).cloned();
                    return stage_clear(ctx, actor, prior);
                }
            };
            run_human(ctx, actor, control, bow_drawn)
        }
        ActorKey::Companion(id) => run_companion(ctx, actor, id),
        ActorKey::Hostile(_) | ActorKey::Passive(_) => {
            Err(ServerError::InvalidInput { field: "actor" })
        }
    }
}
