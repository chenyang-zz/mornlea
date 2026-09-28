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
//! settle a block, mint a drop or wear a tool. Wild grass progress runs to
//! saturation and is retained there without completing: the position-stable
//! seed roll has no Rust kernel yet and belongs to the random-rules node, which
//! also owns grass completion. Snow layers settle through the human-only
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
use crate::core::mutation::{resolve_companion_mine, resolve_mine};
use crate::core::state::{AuthorityReadView, TickContext};

/// Air cell the ray walks through (`core.AirID`,
/// `packages/shared/core/block.go`).
const AIR: u16 = 0;

/// Empty held item (`core.ItemNone`, `packages/shared/core/item.go`).
const ITEM_NONE: u16 = 0;

/// Stone pick (`core.ItemStonePickaxe`).
const ITEM_STONE_PICKAXE: u16 = 10;

/// Iron pick (`core.ItemIronPickaxe`).
const ITEM_IRON_PICKAXE: u16 = 11;

/// Spent stone pick (`core.ItemBrokenStonePickaxe`).
const ITEM_BROKEN_STONE_PICKAXE: u16 = 12;

/// Spent iron pick (`core.ItemBrokenIronPickaxe`).
const ITEM_BROKEN_IRON_PICKAXE: u16 = 13;

/// First door form through the single upper form (`core.IsDoor`,
/// `packages/shared/core/block.go`).
const DOOR_FIRST: u16 = 62;

/// Last door form (`core.DoorUpper`).
const DOOR_LAST: u16 = 70;

/// First bed form through the last (`core.IsBed`, `packages/shared/core/bed.go`).
const BED_FIRST: u16 = 76;

/// Last bed form.
const BED_LAST: u16 = 83;

/// Dry and wet farmland (`core.IsFarmland`, `packages/shared/core/farming.go`).
const FARMLAND_FIRST: u16 = 35;

/// Last farmland form.
const FARMLAND_LAST: u16 = 36;

/// First wheat stage through the last carrot stage (`core.IsCrop`,
/// `packages/shared/core/farming.go`).
const CROP_FIRST: u16 = 37;

/// Last crop stage. Potato (46..=53) and carrot (54..=61) sit inside the span.
const CROP_LAST: u16 = 61;

/// Wild short grass (`core.ShortGrassID`).
const WILD_GRASS: u16 = 84;

/// First snow layer through the last (`core.IsSnowLayer`).
const SNOW_FIRST: u16 = 85;

/// Last snow layer form.
const SNOW_LAST: u16 = 88;

/// Sapling (`core.SaplingID`).
const SAPLING: u16 = 89;

/// First fluid form through the last (`core.IsFluid`,
/// `packages/shared/core/fluid.go`).
const FLUID_FIRST: u16 = 27;

/// Last fluid form.
const FLUID_LAST: u16 = 34;

/// First torch form through the last (`core.IsTorch`,
/// `packages/shared/core/block_properties.go`).
const TORCH_FIRST: u16 = 71;

/// Last torch form.
const TORCH_LAST: u16 = 75;

/// Soil-group and related blocks at 5 ticks (`miningRule`,
/// `packages/server/sim/entity/mining.go`): dirt, grass, sand, gravel,
/// leaves, glass, wool, clay and snow block.
const FIVE_TICK_BLOCKS: [u16; 9] = [3, 4, 15, 16, 19, 20, 22, 24, 25];

/// Wooden blocks at 15 ticks: log, planks and workbench.
const WOOD_BLOCKS: [u16; 3] = [17, 18, 45];

/// Stone-tier blocks whose tool row is 30/15/8: stone, cobblestone, smooth
/// stone, brick, roof tile and mossy cobblestone.
const STONE_TIER: [u16; 6] = [2, 13, 14, 21, 23, 26];

/// Stonebrick-tier blocks whose tool row is 30-failed/15/8: stonebrick,
/// furnace, chest, light block, coal ore and iron ore.
const STONEBRICK_TIER: [u16; 6] = [6, 9, 11, 12, 7, 8];

/// Iron block (`core.IronBlockID`): stone pick 20 failed, iron pick 10.
const IRON_BLOCK: u16 = 10;

/// Reports whether a block number is any door half (`core.IsDoor`).
fn is_door(block: u16) -> bool {
    (DOOR_FIRST..=DOOR_LAST).contains(&block)
}

/// Reports whether a block number is any bed form (`core.IsBed`).
fn is_bed(block: u16) -> bool {
    (BED_FIRST..=BED_LAST).contains(&block)
}

/// Reports whether a block number is any crop stage (`core.IsCrop`).
fn is_crop(block: u16) -> bool {
    (CROP_FIRST..=CROP_LAST).contains(&block)
}

/// Reports whether a block number is dry or wet farmland (`core.IsFarmland`).
fn is_farmland(block: u16) -> bool {
    (FARMLAND_FIRST..=FARMLAND_LAST).contains(&block)
}

/// Reports whether a block number is the wild short grass
/// (`core.IsWildGrass`).
fn is_wild_grass(block: u16) -> bool {
    block == WILD_GRASS
}

/// Reports whether a block number is the sapling (`core.IsSapling`).
fn is_sapling(block: u16) -> bool {
    block == SAPLING
}

/// Reports whether a block number is a snow layer (`core.IsSnowLayer`).
fn is_snow_layer(block: u16) -> bool {
    (SNOW_FIRST..=SNOW_LAST).contains(&block)
}

/// Reports whether a block number is any fluid form (`core.IsFluid`).
fn is_fluid(block: u16) -> bool {
    (FLUID_FIRST..=FLUID_LAST).contains(&block)
}

/// Reports whether a block number is any torch form (`core.IsTorch`).
fn is_torch(block: u16) -> bool {
    (TORCH_FIRST..=TORCH_LAST).contains(&block)
}

/// Required ticks and harvestability of one block under one held item, the
/// exact `miningRule` table (`packages/server/sim/entity/mining.go`). The zero
/// sentinel means unmineable and always pairs with `false`; the native ray
/// never decides mineability, so this table is the only gate.
fn mining_rule(block: u16, held: u16) -> (u16, bool) {
    if is_door(block) || is_bed(block) {
        return (15, true);
    }
    if is_crop(block) || is_wild_grass(block) || is_sapling(block) {
        return (1, true);
    }
    if is_snow_layer(block) {
        return (1, false);
    }
    if is_farmland(block) || FIVE_TICK_BLOCKS.contains(&block) {
        return (5, true);
    }
    if WOOD_BLOCKS.contains(&block) {
        return (15, true);
    }
    if STONE_TIER.contains(&block) {
        return match held {
            ITEM_NONE | ITEM_BROKEN_STONE_PICKAXE | ITEM_BROKEN_IRON_PICKAXE => (30, true),
            ITEM_STONE_PICKAXE => (15, true),
            ITEM_IRON_PICKAXE => (8, true),
            _ => (30, false),
        };
    }
    if STONEBRICK_TIER.contains(&block) {
        return match held {
            ITEM_STONE_PICKAXE => (15, true),
            ITEM_IRON_PICKAXE => (8, true),
            _ => (30, false),
        };
    }
    if block == IRON_BLOCK {
        return match held {
            ITEM_STONE_PICKAXE => (20, false),
            ITEM_IRON_PICKAXE => (10, true),
            _ => (40, false),
        };
    }
    (0, false)
}

/// Unit look direction of a rotation, the exact `LookDirection` formula
/// (`packages/server/sim/entity/command.go`): yaw zero faces north (`-Z`),
/// positive pitch looks up. The accepted resolvers own the settlement ray;
/// this copy only tracks the progress key.
fn look_direction(yaw: f32, pitch: f32) -> [f32; 3] {
    let cos_pitch = pitch.cos();
    [-yaw.sin() * cos_pitch, pitch.sin(), -yaw.cos() * cos_pitch]
}

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
/// resolvers perform: an unobserved cell is unready, observed air continues,
/// and any other block is the hit. No hit within reach is empty.
fn walk_target(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    origin: [f32; 3],
    direction: [f32; 3],
    reach: f32,
) -> TargetOutcome {
    let length =
        (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2])
            .sqrt();
    if !length.is_finite() || length < 1e-6 {
        return TargetOutcome::Unready;
    }
    let normalized = [
        direction[0] / length,
        direction[1] / length,
        direction[2] / length,
    ];
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
                Some(observed) if observed.block == AIR => {}
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
/// accepted resolver and transaction. Any refusal after the attempt clears the
/// human record before the collapsed error shape surfaces.
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
    // Wild grass has no sampler kernel yet: progress runs to saturation and is
    // retained there without completing. The seed roll and grass completion
    // belong to the random-rules node.
    if is_wild_grass(observed.block) {
        let (record, _) = advance_progress(
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
        return stage_progress(ctx, record, prior);
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

/// Settles one companion actor tick: the latest `MineHold` proposal in the
/// view holds until a `MineRelease`, the authority ray must hit exactly the
/// proposed cell, and the companion target registry gates accumulation.
/// Saturation completes exactly once through the accepted companion resolver;
/// only a full inventory keeps the saturated record for retry.
fn run_companion(
    ctx: &mut TickContext<'_>,
    actor: ActorKey,
    id: CompanionId,
) -> Result<PhaseReport, ServerError> {
    const REFUSAL: ServerError = ServerError::InvalidInput { field: "mining" };
    let view = ctx.read();
    let basis = match actor_basis(&view, actor) {
        Some(basis) => basis,
        None => {
            let prior = view.mining(actor).cloned();
            return stage_clear(ctx, actor, prior);
        }
    };
    // Latest proposal per companion wins; movement and placement proposals do
    // not touch the mining hold, which persists until an explicit release.
    let mut held: Option<BlockPos> = None;
    let mut released = false;
    for envelope in view.companion_actions() {
        if envelope.companion_id != id {
            continue;
        }
        match envelope.action {
            CompanionAction::MineHold { target } => {
                held = Some(target);
                released = false;
            }
            CompanionAction::MineRelease => {
                held = None;
                released = true;
            }
            _ => {}
        }
    }
    let prior = view.mining(actor).cloned();
    let tick = view.tick();
    let target = match (held, released, &prior) {
        (Some(target), _, _) => target,
        (None, true, _) | (None, false, None) => {
            return stage_clear(ctx, actor, prior);
        }
        (None, false, Some(previous)) => previous.target,
    };
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
