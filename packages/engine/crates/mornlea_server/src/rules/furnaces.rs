//! Tick-driven furnace advancement: fuel burn, smelt progress and the output
//! transition, one authoritative tick at a time.
//!
//! The durable furnace state is exactly the container record
//! (`ContainerSlots::Furnace`): the three material cells plus the burn and
//! smelt counters. No wall-clock timer exists anywhere in this provider, so a
//! restored record resumes from its stored values and downtime cannot be
//! replaced by elapsed real time
//! (`TestFurnaceRestartRestoresTimersWithoutCatchUp` in
//! `packages/server/server/furnace_publication_test.go`).
//!
//! Mirrored Go rows, each cited at its site:
//!
//! - `packages/server/sim/entity/furnace.go` (`advanceFurnace`, `canSmelt`):
//!   the per-tick step. An input with no product, an empty input, or an
//!   output that cannot accept the product pauses both timers with nothing
//!   consumed; a zero burn with a valid input consumes one coal and sets the
//!   configured burn before the same-tick burn-minus-one and progress-plus-one;
//!   progress reaching the configured smelt interval resets and moves one
//!   input into the output in that same tick.
//! - `packages/shared/core/furnace.go` (`FurnaceBurnTicks` 1600,
//!   `FurnaceSmeltTicks` 200) and the checked caps in `core/contracts.rs`
//!   (`MAX_FURNACE_BURN`, `MAX_FURNACE_SMELT`), which pin the same pair as
//!   the source defaults.
//! - `packages/shared/core/smelting.go` (`SmeltingOutput`): the fixed
//!   input-to-product rows 6->7, 18->23, 27->24 and 53->54, read through the
//!   single domain copy (`mornlea_domain::smelting_output`) so the rows and
//!   the output whitelist cannot drift apart.
//! - `packages/server/sim/entity/drop.go` (`activeInterestKeys`) together
//!   with `advanceFurnaces` in `furnace.go`: the sorted-unique chunk interest
//!   set visits each furnace slot at most once per tick, and a furnace
//!   outside the interest (no Ready chunk, no player interest) is never
//!   visited, which pauses it without resetting anything.
//!
//! Burn and smelt timing come from one immutable environment snapshot per
//! active batch. Zero values normalize to one before ignition or settlement,
//! preserving the source tuning invariant without silently using defaults.
//!
//! Deliberate boundaries. The frozen `RuleCall` record cannot carry the
//! interest set and the read view exposes no container enumeration, so — as
//! with the accepted chunk-acquisition batch — the serial reducer owns the
//! enumeration: it stages the interest furnaces into the context overlay,
//! derives the tick's interest the way `activeInterestKeys` does (active
//! players, Ready chunks), and calls [`advance`] once per tick with the
//! sorted-unique references. A bare batch-shape `run` call therefore reports
//! zero work with nothing staged. Overlay presence is the active gate: a
//! mined furnace loses its record through the accepted atomic transaction and
//! is simply never named again. The input-kind progress reset is the
//! container view provider's material rule and is consumed, never restated
//! here. The per-chunk revision barrier (`engine.touchChunk`, once per
//! changed chunk per tick) and the furnace-state publication to open viewers
//! (`engine.publishContainers`) stay reducer-owned at the overlay-commit and
//! publish legs, exactly as the container view provider left them.

use std::collections::BTreeSet;

use mornlea_domain::ContainerRef;
use mornlea_storage::ItemStack;

use crate::core::contracts::{
    ContainerRecord, ContainerSlots, PhaseReport, RuleCall, RuleEffect, RulePhase, ServerError,
};
use crate::core::state::TickContext;

/// The absent item number (`core.ItemNone`,
/// `packages/shared/core/item.go`).
const ITEM_NONE: u16 = 0;

/// The only fuel the fuel cell accepts (`core.ItemCoal`).
const ITEM_COAL: u16 = 5;

/// Furnace stack ceiling (`core.MaxStackCount`,
/// `packages/shared/core/item.go`).
const MAX_STACK_COUNT: u8 = 64;

/// Material cell positions inside the furnace record: input, fuel cell and
/// output. The unified view slots `36..38` belong to the container view
/// provider; this provider reads the record cells directly.
const INPUT_CELL: usize = 0;
const FUEL_CELL: usize = 1;
const OUTPUT_CELL: usize = 2;

/// The frozen phase entry for the furnace batch.
///
/// Only the exact batch shape is the provider's: the furnace phase with no
/// actor, command or internal payload. Every other shape is refused without
/// effect. The batch itself cannot run from a bare call — the frozen
/// `RuleCall` cannot carry the interest set and the read view exposes no
/// container enumeration — so the batch body is [`advance`], called by the
/// serial reducer once per tick after it enumerates the interest. This entry
/// reports zero work with nothing staged, mirroring the accepted
/// chunk-acquisition entry whose drained batch also reaches the rule through
/// a reducer-owned drain.
pub fn run(_ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::FurnaceStep
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

/// Advances every furnace named by the tick's sorted-unique interest set one
/// tick, reporting `examined` unique references and `applied` changed
/// records.
///
/// The set form guarantees the Go overlapping-viewers rule: duplicate
/// references in the caller's slice still advance one furnace exactly once
/// (`TestFurnaceAdvancesOnceWithOverlappingViewers`). A reference with no
/// staged record is interest churn — outside the interest, unready, or
/// already retired — and pauses without resetting, the way the Go walk skips
/// a missing Ready chunk. A chest reference in the interest set is a reducer
/// sequencing bug and refuses the whole batch before anything is staged.
pub fn advance(
    ctx: &mut TickContext<'_>,
    interest: &[ContainerRef],
) -> Result<PhaseReport, ServerError> {
    let ordered: BTreeSet<ContainerRef> = interest.iter().copied().collect();
    let view = ctx.read();
    let mut present: Vec<ContainerRecord> = Vec::with_capacity(ordered.len());
    for reference in &ordered {
        let Some(record) = view.container(*reference) else {
            continue;
        };
        match &record.slots {
            ContainerSlots::Furnace { .. } => present.push(record),
            ContainerSlots::Chest(_) => {
                return Err(ServerError::InvalidInput { field: "interest" });
            }
        }
    }
    if present.is_empty() {
        return Ok(PhaseReport {
            examined: ordered.len(),
            applied: 0,
            carried: 0,
            rejected: 0,
        });
    }
    let tunables = view
        .environment()
        .ok_or(ServerError::Internal {
            invariant: "furnace snapshot",
        })?
        .tunables;
    let burn_ticks = tunables.furnace_burn_ticks().max(1);
    let smelt_ticks = tunables.furnace_smelt_ticks().max(1);
    let mut applied = 0usize;
    for before in present {
        if let Some(after) = advance_furnace(&before, burn_ticks, smelt_ticks)? {
            ctx.stage(RuleEffect::Container { before, after })
                .map_err(|_| ServerError::InvalidInput { field: "furnace" })?;
            applied += 1;
        }
    }
    Ok(PhaseReport {
        examined: ordered.len(),
        applied,
        carried: 0,
        rejected: 0,
    })
}

/// One tick of the Go `advanceFurnace` row
/// (`packages/server/sim/entity/furnace.go`), returning the next record or
/// `None` when the tick pauses without consuming anything.
///
/// The pause rows come first, exactly as in `canSmelt`: no smelting row for
/// the input item, an empty input, or an output that is nonempty with a
/// different product or a full same-product stack freezes both timers, so a
/// burning furnace never loses fuel to an unusable input
/// (`TestFurnaceMaterialsPauseWithoutWastingFuel`). Ignition consumes one
/// coal and sets the configured burn before the same-tick decrement and
/// progress increment; the source default first step has burn 1599 and progress 1
/// (`TestFurnaceMaterialsLightFuelAndAdvanceSameTick`). Completion resets the
/// progress, consumes one input and mints or increments the product in that
/// same tick (`TestFurnaceProducesMaterialsAtTwoHundredTicks`).
fn advance_furnace(
    record: &ContainerRecord,
    burn_ticks: u16,
    smelt_ticks: u8,
) -> Result<Option<ContainerRecord>, ServerError> {
    let ContainerSlots::Furnace {
        slots: stored,
        fuel: burn,
        progress,
    } = &record.slots
    else {
        return Err(ServerError::InvalidInput { field: "interest" });
    };
    let [input, fuel_cell, output] = *stored;
    let mut slots = *stored;
    let mut burn = *burn;
    let mut progress = *progress;
    // `canSmelt`: the fixed product must exist, the input must hold items and
    // the output cell must be empty or hold the same product below the stack
    // ceiling; anything else pauses both timers with nothing consumed, so a
    // burning furnace never loses fuel to an unusable input.
    let Some(product) = mornlea_domain::smelting_output(input.item) else {
        return Ok(None);
    };
    if input.count == 0 {
        return Ok(None);
    }
    if output.item != ITEM_NONE && (output.item != product || output.count >= MAX_STACK_COUNT) {
        return Ok(None);
    }
    if burn == 0 {
        // Ignition row: one coal goes out and the burn time is set before the
        // same-tick decrement, so the first tick after lighting reads
        // configured burn minus one with progress 1.
        if fuel_cell.item != ITEM_COAL || fuel_cell.count == 0 {
            return Ok(None);
        }
        let mut fuel = fuel_cell;
        fuel.count -= 1;
        if fuel.count == 0 {
            fuel = ItemStack::default();
        }
        slots[FUEL_CELL] = fuel;
        burn = u32::from(burn_ticks);
    }
    burn -= 1;
    progress = progress
        .checked_add(1)
        .ok_or(ServerError::InvalidInput { field: "progress" })?;
    if progress >= u32::from(smelt_ticks) {
        // Completion row: the same tick resets the progress, consumes one
        // input and mints or increments the product.
        progress = 0;
        let mut consumed = input;
        consumed.count -= 1;
        if consumed.count == 0 {
            consumed = ItemStack::default();
        }
        slots[INPUT_CELL] = consumed;
        if output.item == ITEM_NONE {
            slots[OUTPUT_CELL] = ItemStack {
                item: product,
                count: 1,
                durability: 0,
            };
        } else {
            let mut filled = output;
            filled.count += 1;
            slots[OUTPUT_CELL] = filled;
        }
    }
    Ok(Some(ContainerRecord {
        reference: record.reference,
        revision: record.revision,
        slots: ContainerSlots::Furnace {
            slots,
            fuel: burn,
            progress,
        },
    }))
}
