//! Fluid rescan and update scheduling over a caller-owned schedule.
//!
//! This provider owns the fluid phases: boundary rescans that seed the update
//! queues, bounded per-dimension updates with deterministic carry, and the
//! requeue that keeps flowing water moving. The pending queues live in the
//! caller-owned [`FluidSchedule`], because the frozen [`RuleCall`] record
//! cannot carry queue contents and [`TickContext`] exposes no queue port; the
//! serial tick reducer owns the schedule value and calls [`rescan`] on
//! [`RulePhase::FluidRescan`] then [`update`] on [`RulePhase::FluidUpdate`],
//! exactly like the acquisition provider's caller-owned want ledger feeds its
//! batch body after the mailbox drain.
//!
//! Mirrored Go rows, each cited at its site:
//!
//! - `packages/server/fluid/queue.go` (`Queue.Advance`, `lessPos`,
//!   `Enqueue`): due order `(due, chunkX, chunkZ, y, z, x)` with earliest-due
//!   dedup, the tick-start snapshot, per-dimension budgets, unchanged due
//!   ticks on carry, and requeue of each changed cell plus its six neighbors.
//! - `packages/server/fluid/rules.go` (`strongerWrite`, `Replaceable`): fluid
//!   beats air on conflict and the stronger (smaller) level wins between
//!   fluids, independent of enumeration order.
//! - `packages/server/sim/realm/environment.go` (`AdvanceFluids`,
//!   `runFluidRescans`, `rescanChunkFluids`, `dropOutOfScope`): rescans precede
//!   updates, out-of-scope pending rescans drop their cursor while carried
//!   work resumes next tick, and section scans charge whole.
//! - `packages/server/fluid/rescan_native.go` and the engine rescan kernel:
//!   uniform sections shortcut at a cost of one cell while dense sections
//!   charge per cell; sections are the atomic unit, so a scope exit never
//!   leaves a half-scanned section behind.
//!
//! Work accounting: every selected update item charges one
//! [`WorkKind::FluidUpdates`] unit through [`TickContext::charge`], stale
//! skips included, so the frozen per-dimension ceiling bounds evaluated items
//! directly. Each rescan section charges one [`WorkKind::RescanCells`] call of
//! up to 4096 cells through the frozen section-aware ceiling, which authorizes
//! a section only below the target and permits the final section to overshoot
//! by at most 4095 cells. A refused charge never fails the tick: the unstarted
//! item or section carries with its original due tick or cursor, so carried
//! work is never lost and never double-processed. Per-target commits use the
//! accepted system transaction, with a complete drop batch for displaced plants.

use std::collections::{BTreeMap, BTreeSet};

use mornlea_domain::{BlockPos, ChunkPos, Dimension, FiniteVec3, RejectReason};
use mornlea_engine::native::contracts::fluid::{FluidEvalOp, FluidWrites, NeighborSlot};
use mornlea_engine::native::fluid_eval::NativeFluidEval;

use super::crops::environment_plant_outputs;
use crate::core::contracts::{
    BlockObservation, BlockWrite, ChunkKey, DropBatch, DropSource, PhaseReport, RescanWork,
    RuleCall, RulePhase, RuleReject, ServerError, SystemRule, WorkKind,
};
use crate::core::state::TickContext;

/// Air cell, the only block a stale queue item or an undiscovered neighbor can
/// hold while still mattering to the schedule (`core.AirID`,
/// `packages/shared/core/block.go`).
const AIR: u16 = 0;

/// Unobserved or out-of-range world read, the sealed stand-in the Go rescan
/// side reads as `Barrier` (`packages/server/sim/realm/environment.go`,
/// `encodeRescanSkirtColumn`): never replaceable, never enqueued.
const BARRIER: u16 = 1;

/// First fluid block number: the still-water source (`core.WaterSourceID`).
const WATER_SOURCE: u16 = 27;

/// Last fluid block number: the weakest flow (`core.WaterLevel7ID`). Fluid is
/// the eight continuous numbers source through level seven, mirroring Go
/// `core.IsFluid` in `packages/shared/core/fluid.go`.
const WATER_LEVEL_7: u16 = 34;

/// Cells along one section edge (`core.SectionSize`,
/// `packages/shared/core/pos.go`).
const SECTION_SIZE: i32 = 16;

/// Cells in one chunk section: the largest single rescan charge, matching the
/// frozen section ceiling (`core.SectionSize` cubed).
const SECTION_CELLS: usize = 4096;

/// Dimensions in sorted order, the same order the Go realm advances its
/// per-dimension fluid queues (`sortedFluidDimensions` in
/// `packages/server/sim/realm/environment.go`).
const SORTED_DIMENSIONS: [Dimension; 2] = [Dimension::OVERWORLD, Dimension::DEPTHS];

/// Offsets of the six face neighbors in Go `sixNeighbors` order
/// (`packages/server/fluid/rules.go`): above, below, then the four horizontal
/// directions.
const SIX_NEIGHBORS: [(i32, i32, i32); 6] = [
    (0, 1, 0),
    (0, -1, 0),
    (1, 0, 0),
    (-1, 0, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// Reports whether a block number is any fluid, mirroring Go `core.IsFluid`.
fn is_fluid(block: u16) -> bool {
    (WATER_SOURCE..=WATER_LEVEL_7).contains(&block)
}

/// Fluid strength rank, mirroring Go `core.FluidLevel`: the source reads as
/// zero and each flow reads as its distance from the source, so a smaller rank
/// is stronger water. Only meaningful for blocks where [`is_fluid`] holds.
fn fluid_level(block: u16) -> u8 {
    if block == WATER_SOURCE {
        0
    } else {
        (block - WATER_SOURCE) as u8
    }
}

/// Merges two candidate writes to the same target cell, mirroring Go
/// `strongerWrite` (`packages/server/fluid/rules.go`): a fluid candidate beats
/// air, and between two fluids the stronger (smaller) level wins with ties
/// keeping the earlier candidate. The fold is order-independent, so the merged
/// result never depends on evaluation or decode order.
fn stronger_write(first: u16, second: u16) -> u16 {
    match (is_fluid(first), is_fluid(second)) {
        (true, false) => first,
        (false, true) => second,
        (false, false) => first,
        (true, true) => {
            if fluid_level(first) <= fluid_level(second) {
                first
            } else {
                second
            }
        }
    }
}

/// Maps one queue dimension to its per-dimension budget slot. Only the two
/// authority dimensions exist, so the mapping is total over every dimension a
/// staged key can name.
fn dimension_slot(dimension: Dimension) -> usize {
    match dimension.get() {
        0 => 0,
        _ => 1,
    }
}

/// Chunk column containing a world position, the same arithmetic shift the
/// accepted replay helpers use to derive staged chunk keys.
fn chunk_of(pos: BlockPos) -> ChunkPos {
    ChunkPos::new(pos.x() >> 4, pos.z() >> 4)
}

/// World position of one neighbor slot relative to the evaluated cell,
/// mirroring the kernel slot layout (self, above, below, +x, -x, +z, -z).
fn slot_pos(center: BlockPos, slot: NeighborSlot) -> Option<BlockPos> {
    let (dx, dy, dz) = match slot {
        NeighborSlot::SelfCell => (0, 0, 0),
        NeighborSlot::Above => (0, 1, 0),
        NeighborSlot::Below => (0, -1, 0),
        NeighborSlot::PosX => (1, 0, 0),
        NeighborSlot::NegX => (-1, 0, 0),
        NeighborSlot::PosZ => (0, 0, 1),
        NeighborSlot::NegZ => (0, 0, -1),
    };
    Some(BlockPos::new(
        center.x().checked_add(dx)?,
        center.y().checked_add(dy)?,
        center.z().checked_add(dz)?,
    ))
}

/// Due-order key for one queued fluid cell: `(due, chunkX, chunkZ, y, z, x)`.
/// The single fluid kind needs no tiebreak beyond the position itself.
type DueOrderKey = (u64, i32, i32, i32, i32, i32);

/// Sorted-commit key for one merged write target:
/// `(chunkX, chunkZ, y, z, x)`, mirroring Go `lessPos`.
type SortedTargetKey = (i32, i32, i32, i32, i32);

/// One dimension's due queue: the full due order as the map key with a
/// position index for earliest-due dedup, the same two-sided structure as Go's
/// unified scheduler domain (indexed heap plus per-position dedup in
/// `packages/server/updates`).
#[derive(Clone, Debug, Default)]
struct FluidQueue {
    by_due: BTreeMap<DueOrderKey, (ChunkKey, BlockPos)>,
    due_by_pos: BTreeMap<(ChunkKey, BlockPos), u64>,
}

impl FluidQueue {
    /// Enqueues one cell, keeping the earlier due tick when the cell is
    /// already queued, mirroring Go `Enqueue` ("only earlier, never later").
    /// Reports whether the queue changed.
    fn insert(&mut self, key: ChunkKey, pos: BlockPos, due: u64) -> bool {
        if let Some(&held) = self.due_by_pos.get(&(key, pos)) {
            if held <= due {
                return false;
            }
            self.by_due.remove(&order_key(held, key, pos));
        }
        self.by_due.insert(order_key(due, key, pos), (key, pos));
        self.due_by_pos.insert((key, pos), due);
        true
    }

    /// Removes and returns the earliest-due item at or before `now`, or
    /// nothing when no item is due. Removal on pop means a popped item can
    /// never be processed twice.
    fn pop_due(&mut self, now: u64) -> Option<(ChunkKey, BlockPos, u64)> {
        let (&order, &(key, pos)) = self.by_due.first_key_value()?;
        if order.0 > now {
            return None;
        }
        self.by_due.remove(&order);
        self.due_by_pos.remove(&(key, pos));
        Some((key, pos, order.0))
    }

    /// Counts the queued items still due at `now` without disturbing them.
    fn count_due(&self, now: u64) -> usize {
        self.by_due
            .keys()
            .take_while(|order| order.0 <= now)
            .count()
    }
}

/// Builds the due-order key for one queued cell.
fn order_key(due: u64, key: ChunkKey, pos: BlockPos) -> DueOrderKey {
    (due, key.pos.x(), key.pos.z(), pos.y(), pos.z(), pos.x())
}

/// Caller-owned fluid schedule: the pending update queues plus the pending
/// rescan cursors. The serial tick reducer holds this value across ticks;
/// each provider call removes the work it starts and leaves the rest
/// untouched, so carried work resumes next tick with its original due tick or
/// cursor.
#[derive(Clone, Debug, Default)]
pub struct FluidSchedule {
    fluid: [FluidQueue; 2],
    rescans: Vec<RescanWork>,
}

impl FluidSchedule {
    /// Creates an empty schedule with no pending work in either dimension.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueues one fluid update, keeping the earlier due tick on duplicates.
    pub fn enqueue_fluid(&mut self, key: ChunkKey, pos: BlockPos, due: u64) {
        self.fluid[dimension_slot(key.dimension)].insert(key, pos, due);
    }

    /// Enqueues one section rescan. A chunk already awaiting rescan keeps its
    /// recorded cursor, mirroring Go `enqueueChunk` first-wins dedup.
    pub fn enqueue_rescan(&mut self, work: RescanWork) {
        if !self.rescans.iter().any(|held| held.key == work.key) {
            self.rescans.push(work);
        }
    }

    /// Counts the queued fluid items of one dimension, due or not.
    pub fn pending_fluid(&self, dimension: Dimension) -> usize {
        self.fluid[dimension_slot(dimension)].due_by_pos.len()
    }

    /// Reports the queued due tick of one cell, if it is queued.
    pub fn fluid_due(&self, dimension: Dimension, pos: BlockPos) -> Option<u64> {
        let key = ChunkKey {
            dimension,
            pos: chunk_of(pos),
        };
        self.fluid[dimension_slot(dimension)]
            .due_by_pos
            .get(&(key, pos))
            .copied()
    }

    /// Counts the pending section rescans.
    pub fn pending_rescans(&self) -> usize {
        self.rescans.len()
    }

    /// Views the pending section rescans in scan order.
    pub fn rescan_queue(&self) -> &[RescanWork] {
        &self.rescans
    }
}

/// The frozen batch entry for both fluid phases.
///
/// The schedule value travels with the serial reducer rather than the tick
/// context, so this entry stages nothing and reports zero work for the two
/// well-shaped batch calls, exactly like the acquisition entry whose
/// completions arrive through its own drain body. Any other shape is the
/// shared no-effect refusal: no charge, no staging, no commit.
pub fn run(_ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::FluidRescan && call.phase != RulePhase::FluidUpdate
        || call.actor.is_some()
        || call.command.is_some()
        || call.internal.is_some()
    {
        return Ok(PhaseReport {
            examined: 0,
            applied: 0,
            carried: 0,
            rejected: 0,
        });
    }
    Ok(PhaseReport {
        examined: 0,
        applied: 0,
        carried: 0,
        rejected: 0,
    })
}

/// Boundary rescan: drops out-of-scope pending sections, then scans each
/// pending section whole within the frozen per-dimension rescan target.
///
/// A section whose staged cells are uniformly non-fluid (unobserved cells
/// count as sealed, never fluid) shortcuts at a cost of one cell, mirroring
/// the kernel's fixed-point shortcut charge; any other section charges its
/// full remaining cell count and enqueues each discovered fluid cell plus its
/// observed-air neighbors at `now + delay`, mirroring the Go recovery rule
/// that re-enqueues every fluid cell with its air neighbors (`queue.go`).
/// Sections are atomic: a refused charge carries the section with its cursor
/// unmoved and stops the scan, so carried sections resume whole next tick.
/// `examined` counts completed sections, `applied` counts newly queued fluid
/// items, and `carried` counts the sections left pending.
pub fn rescan(
    schedule: &mut FluidSchedule,
    ctx: &mut TickContext<'_>,
    scope: &BTreeSet<ChunkKey>,
    now: u64,
    delay: u64,
) -> Result<PhaseReport, ServerError> {
    // A scope exit drops the cursor: pending sections outside the active set
    // leave without touching the world, while everything in scope carries on.
    // This mirrors Go `dropOutOfScope`, including the head rule — dropping the
    // queue head resets the shared cursor there; here each entry carries its
    // own cursor, so dropping entries is the whole reset.
    schedule.rescans.retain(|work| scope.contains(&work.key));
    let mut examined = 0usize;
    let mut applied = 0usize;
    // Entries complete head-first: each finished section leaves the queue, so
    // the head always names the next unstarted section and whatever remains at
    // a refusal break is exactly the carried set.
    while !schedule.rescans.is_empty() {
        let work = schedule.rescans[0].clone();
        if work.next_cell as usize >= SECTION_CELLS {
            schedule.rescans.remove(0);
            examined += 1;
            continue;
        }
        let start = work.next_cell as usize;
        if is_uniform_section(&ctx.read(), &work) {
            if ctx
                .charge(WorkKind::RescanCells(work.key.dimension), 1)
                .is_err()
            {
                break;
            }
            schedule.rescans.remove(0);
            examined += 1;
            continue;
        }
        let cells = SECTION_CELLS - start;
        if ctx
            .charge(WorkKind::RescanCells(work.key.dimension), cells)
            .is_err()
        {
            break;
        }
        applied += scan_section(schedule, &ctx.read(), &work, start, now, delay);
        schedule.rescans.remove(0);
        examined += 1;
    }
    let carried = schedule.rescans.len();
    Ok(PhaseReport {
        examined,
        applied,
        carried,
        rejected: 0,
    })
}

/// Reports whether every staged cell of a section reads the same, with
/// unobserved cells counting as sealed. Only a section scanned from its first
/// cell can shortcut: a resumed cursor cannot certify the cells before it.
fn is_uniform_section(view: &crate::core::state::AuthorityReadView<'_>, work: &RescanWork) -> bool {
    if work.next_cell != 0 {
        return false;
    }
    let first = section_cell(view, work, 0);
    (1..SECTION_CELLS).all(|cell| section_cell(view, work, cell) == first)
        && !matches!(first, Some(block) if is_fluid(block))
}

/// Reads one section cell by cursor, following Go's dense `blockIndex` order
/// (`x + z * 16 + y * 256` in `encodeRescanBox`). Out-of-range arithmetic
/// reads as unobserved rather than wrapping into another chunk.
fn section_cell(
    view: &crate::core::state::AuthorityReadView<'_>,
    work: &RescanWork,
    cell: usize,
) -> Option<u16> {
    let local_x = (cell % SECTION_SIZE as usize) as i32;
    let local_z = ((cell / SECTION_SIZE as usize) % SECTION_SIZE as usize) as i32;
    let local_y = (cell / (SECTION_SIZE as usize * SECTION_SIZE as usize)) as i32;
    let x = work
        .key
        .pos
        .x()
        .checked_mul(SECTION_SIZE)?
        .checked_add(local_x)?;
    let y = work.section_y.checked_add(local_y)?;
    let z = work
        .key
        .pos
        .z()
        .checked_mul(SECTION_SIZE)?
        .checked_add(local_z)?;
    view.block(work.key.dimension, BlockPos::new(x, y, z))
}

/// Scans one authorized section from `start`, enqueuing each discovered fluid
/// cell plus its observed-air neighbors at `now + delay`. Returns the number
/// of newly queued fluid items.
fn scan_section(
    schedule: &mut FluidSchedule,
    view: &crate::core::state::AuthorityReadView<'_>,
    work: &RescanWork,
    start: usize,
    now: u64,
    delay: u64,
) -> usize {
    let due = now.saturating_add(delay);
    let mut enqueued = 0usize;
    for cell in start..SECTION_CELLS {
        let Some(block) = section_cell(view, work, cell) else {
            continue;
        };
        if !is_fluid(block) {
            continue;
        }
        let local_x = (cell % SECTION_SIZE as usize) as i32;
        let local_z = ((cell / SECTION_SIZE as usize) % SECTION_SIZE as usize) as i32;
        let local_y = (cell / (SECTION_SIZE as usize * SECTION_SIZE as usize)) as i32;
        let Some(pos) = section_pos(work, local_x, local_y, local_z) else {
            continue;
        };
        if schedule.enqueue_fluid_checked(work.key, pos, due) {
            enqueued += 1;
        }
        for (dx, dy, dz) in SIX_NEIGHBORS {
            let Some(neighbor) = checked_add_pos(pos, dx, dy, dz) else {
                continue;
            };
            if view.block(work.key.dimension, neighbor) != Some(AIR) {
                continue;
            }
            let key = ChunkKey {
                dimension: work.key.dimension,
                pos: chunk_of(neighbor),
            };
            if schedule.enqueue_fluid_checked(key, neighbor, due) {
                enqueued += 1;
            }
        }
    }
    enqueued
}

/// World position of one section-local cell, or nothing on arithmetic
/// overflow.
fn section_pos(work: &RescanWork, x: i32, y: i32, z: i32) -> Option<BlockPos> {
    Some(BlockPos::new(
        work.key.pos.x().checked_mul(SECTION_SIZE)?.checked_add(x)?,
        work.section_y.checked_add(y)?,
        work.key.pos.z().checked_mul(SECTION_SIZE)?.checked_add(z)?,
    ))
}

/// Adds one offset to a world position, or nothing on arithmetic overflow.
fn checked_add_pos(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> Option<BlockPos> {
    Some(BlockPos::new(
        pos.x().checked_add(dx)?,
        pos.y().checked_add(dy)?,
        pos.z().checked_add(dz)?,
    ))
}

/// Bounded update: selects the due items of each sorted dimension, evaluates
/// their tick-start snapshots through the engine kernel, merges overlapping
/// outputs strongest-first, settles each target in sorted order, and requeues.
///
/// Selection pops at most the frozen per-dimension ceiling can charge: each
/// popped item charges one [`WorkKind::FluidUpdates`] unit, stale skips
/// included, and the first refusal restores that item and carries every still-
/// due remainder with unchanged due ticks. `examined` counts evaluated items,
/// `applied` counts cells whose value actually changed, `carried` counts
/// due-but-unevaluated leftovers, and `rejected` stays zero — fluid has no
/// refusal class, only skips and carries.
pub fn update(
    schedule: &mut FluidSchedule,
    ctx: &mut TickContext<'_>,
    now: u64,
    delay: u64,
) -> Result<PhaseReport, ServerError> {
    const REFUSAL: ServerError = ServerError::InvalidInput { field: "fluid" };
    // The environment is a tick-wide authority input. Refusing before the
    // first pop preserves the caller-owned schedule for a later valid tick.
    let Some(environment) = ctx.read().environment().cloned() else {
        return Err(ServerError::InvalidInput {
            field: "environment",
        });
    };
    let mut report = PhaseReport {
        examined: 0,
        applied: 0,
        carried: 0,
        rejected: 0,
    };
    for dimension in SORTED_DIMENSIONS {
        let queue = &mut schedule.fluid[dimension_slot(dimension)];
        let mut batch: Vec<(ChunkKey, BlockPos)> = Vec::new();
        while let Some((key, pos, due)) = queue.pop_due(now) {
            if ctx.charge(WorkKind::FluidUpdates(dimension), 1).is_err() {
                // Exhaustion is deterministic carry, never a fatal tick error:
                // the unstarted item returns to its exact due slot, and the
                // carried count covers it with every still-due remainder.
                queue.insert(key, pos, due);
                report.carried += queue.count_due(now);
                break;
            }
            batch.push((key, pos));
        }
        if batch.is_empty() {
            continue;
        }
        report.examined += batch.len();
        // Snapshot every selected neighborhood before writing anything, so no
        // evaluation can observe another item's output of this same tick.
        // Unobserved cells read as the sealed stand-in, mirroring the Go
        // rescan side's `Barrier` for reads outside ready data.
        let mut items: Vec<[u16; 7]> = Vec::with_capacity(batch.len());
        {
            let view = ctx.read();
            for (_, pos) in &batch {
                items.push(snapshot_neighborhood(&view, dimension, *pos));
            }
        }
        let mut dst: Vec<FluidWrites> = vec![FluidWrites::default(); items.len()];
        NativeFluidEval
            .evaluate(&items, &mut dst)
            .map_err(|_| REFUSAL)?;
        // Merge overlapping outputs strongest-first over the sorted target
        // order, so the commit order never depends on evaluation order.
        let mut merged: BTreeMap<SortedTargetKey, (BlockObservation, u16)> = BTreeMap::new();
        {
            let view = ctx.read();
            for ((_, pos), writes) in batch.iter().zip(dst.iter()) {
                for change in writes.changes() {
                    let Some(target) = slot_pos(*pos, change.slot) else {
                        continue;
                    };
                    let Some(observed) = view.observation(dimension, target) else {
                        // Unreachable: the kernel only targets cells the snapshot
                        // read as replaceable, and replaceable cells are observed.
                        // Skipping keeps a basis gap from failing the tick.
                        continue;
                    };
                    let key = (
                        observed.key.pos.x(),
                        observed.key.pos.z(),
                        observed.pos.y(),
                        observed.pos.z(),
                        observed.pos.x(),
                    );
                    merged
                        .entry(key)
                        .and_modify(|(_, held)| *held = stronger_write(*held, change.block))
                        .or_insert((observed, change.block));
                }
            }
        }
        // Each target is one source SetBlock decision. A plant capacity retry
        // cannot discard an independent water write from the same evaluation.
        let mut changed: Vec<(ChunkKey, BlockPos)> = Vec::new();
        let mut retries: Vec<(ChunkKey, BlockPos)> = Vec::new();
        for (observed, block) in merged.values() {
            if observed.block == *block {
                continue;
            }
            let write = BlockWrite::try_new(*observed, *block).map_err(|_| REFUSAL)?;
            if let Some(stacks) = environment_plant_outputs(
                environment.seed,
                now,
                dimension,
                observed.pos,
                observed.block,
            ) {
                let pos = observed.pos;
                let drops = DropBatch::try_new(
                    DropSource::System {
                        rule: SystemRule::Fluid,
                        tick: ctx.read().tick(),
                        target: pos,
                    },
                    dimension,
                    FiniteVec3::try_new([
                        pos.x() as f32 + 0.5,
                        pos.y() as f32 + 0.5,
                        pos.z() as f32 + 0.5,
                    ])
                    .map_err(|_| REFUSAL)?,
                    stacks,
                    environment.tunables.drop_pickup_delay_ticks(),
                )
                .map_err(|_| REFUSAL)?;
                match ctx.read().check_drop_batch(&drops) {
                    Ok(()) => {}
                    Err(RuleReject::Wire(RejectReason::DropCapacity)) => {
                        retries.push((observed.key, pos));
                        continue;
                    }
                    Err(_) => return Err(REFUSAL),
                }
                match ctx
                    .transaction()
                    .try_system_with_drops(SystemRule::Fluid, vec![write], drops)
                {
                    Ok(_) => {}
                    Err(RuleReject::Wire(RejectReason::DropCapacity)) => {
                        retries.push((observed.key, pos));
                        continue;
                    }
                    Err(_) => return Err(REFUSAL),
                }
            } else {
                ctx.transaction()
                    .try_system(SystemRule::Fluid, vec![write])
                    .map_err(|_| REFUSAL)?;
            }
            changed.push((observed.key, observed.pos));
        }
        report.applied += changed.len();
        // Each changed cell plus its six neighbors requeues at `now + delay`,
        // with earliest-due dedup folding overlaps, mirroring Go's requeue of
        // the changed set after commit.
        let due = now.saturating_add(delay);
        for (key, pos) in changed.into_iter().chain(retries) {
            schedule.enqueue_fluid(key, pos, due);
            for (dx, dy, dz) in SIX_NEIGHBORS {
                let Some(neighbor) = checked_add_pos(pos, dx, dy, dz) else {
                    continue;
                };
                schedule.enqueue_fluid(
                    ChunkKey {
                        dimension,
                        pos: chunk_of(neighbor),
                    },
                    neighbor,
                    due,
                );
            }
        }
    }
    Ok(report)
}

/// Reads the seven-cell evaluation neighborhood of one queued cell in kernel
/// slot order (self, above, below, +x, -x, +z, -z), with unobserved cells
/// standing in as sealed.
fn snapshot_neighborhood(
    view: &crate::core::state::AuthorityReadView<'_>,
    dimension: Dimension,
    pos: BlockPos,
) -> [u16; 7] {
    const OFFSETS: [(i32, i32, i32); 7] = [
        (0, 0, 0),
        (0, 1, 0),
        (0, -1, 0),
        (1, 0, 0),
        (-1, 0, 0),
        (0, 0, 1),
        (0, 0, -1),
    ];
    let mut cells = [BARRIER; 7];
    for (slot, (dx, dy, dz)) in OFFSETS.iter().enumerate() {
        let Some(cell) = checked_add_pos(pos, *dx, *dy, *dz) else {
            continue;
        };
        cells[slot] = view.block(dimension, cell).unwrap_or(BARRIER);
    }
    cells
}

impl FluidSchedule {
    /// Inserts one fluid item, reporting whether the queue changed. The
    /// section scanner uses the checked form to count genuine discoveries.
    fn enqueue_fluid_checked(&mut self, key: ChunkKey, pos: BlockPos, due: u64) -> bool {
        self.fluid[dimension_slot(key.dimension)].insert(key, pos, due)
    }
}
