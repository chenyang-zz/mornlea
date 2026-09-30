//! Bounded farmland moisture: wet/dry settlement and deterministic hydration
//! over a read budget.
//!
//! This provider owns the moisture domain that runs after the fluid phases in
//! the frozen tick order. Mirrored Go rows, each cited at its site:
//!
//! - `packages/server/sim/realm/environment.go` (`AdvanceFarmlandMoisture`,
//!   `newFarmlandMoistureHandler`, `farmlandIsWet`): each due candidate
//!   settles through the fixed guard order — global check budget, active
//!   Ready scope gate that consumes with zero block reads, one target read,
//!   then a whole-neighborhood reservation — and a candidate whose budget
//!   guard refuses is deferred at its original due tick while the domain
//!   pauses, so a judgment is never split across ticks and no partial
//!   neighbor state survives the pause.
//! - `packages/server/sim/realm/environment.go` (`farmlandIsWet`): the
//!   162-cell scan enumerates dy `0..=1`, dz and dx `-4..=4`; any Ready fluid
//!   cell hydrates and unready cells never qualify, so the result is a
//!   boolean independent of scan order.
//! - `packages/server/sim/realm/environment.go` (`enqueueFarmlandMoisture`,
//!   `enqueueFarmlandMoistureAroundFluid`): fresh candidates are due the tick
//!   they are enqueued, out-of-world Y positions are dropped at enqueue, and
//!   a fluid-membership change wakes the reverse window one layer below
//!   through radius four. The cited sources carry no periodic re-check: a
//!   settled candidate leaves the domain, and re-wake comes only from block
//!   -write events or full-chunk rescans. A deferred candidate also keeps its
//!   original due tick, which is why a budget-deferred judgment completes on
//!   the very next tick.
//! - `packages/server/sim/realm/environment.go` (`runFarmlandMoistureRescans`,
//!   `farmlandMoistureRescanPosition`, `dropOutOfScope`): the rescan tail runs
//!   only with the reads left over from the candidate pass, scans the full
//!   -height chunk halo with the x-fastest, then z, then y cursor, charges one
//!   read per cell, and re-seeds in-scope Ready farmland as candidates due
//!   now; out-of-scope pending chunks drop without reads and a dropped head
//!   resets the shared cursor.
//! - `packages/server/updates/queue.go` (`lessItem`, `lessPos`, `Enqueue`):
//!   candidate order is `(due, chunkX, chunkZ, y, z, x)` with per-position
//!   dedup that only ever moves a queued cell earlier, never later.
//!
//! Work accounting goes through the frozen charge surfaces only: one
//! [`WorkKind::FarmlandChecks`] unit per settled-or-deferred-by-work
//! candidate and [`WorkKind::FarmlandReads`] units for the target read plus
//! the full 162-read neighborhood reservation. The two budgets are global
//! across dimensions, exactly like the Go counters the per-dimension handler
//! guards against. A refused charge defers whole and pauses; it never fails
//! the tick and never leaves a half-judged neighborhood. Hydration and drying
//! transitions stage through the accepted atomic transaction under
//! [`SystemRule::Farmland`] before any rescan work runs.
//!
//! Boundary: a dry farmland cell with air above reverts through the 30%
//! probability roll of the deterministic random-rules provider. Moisture
//! reports dry only — it writes [`FARMLAND_DRY`] and never rolls.
//!
//! Like the fluid and acquisition providers, the pending queues live in the
//! caller-owned [`FarmlandSchedule`], because the frozen [`RuleCall`] record
//! cannot carry queue contents and [`TickContext`] exposes no queue port; the
//! serial tick reducer owns the schedule value across ticks and calls
//! [`run`] on the `RulePhase::Farmland` batch (which stages nothing) and
//! [`advance`] for the bounded settlement body.

use std::collections::{BTreeMap, BTreeSet};

use mornlea_domain::{BlockPos, ChunkPos, Dimension};

use crate::core::contracts::{
    BlockWrite, ChunkKey, PhaseReport, RuleCall, RulePhase, ServerError, SystemRule, WorkKind,
};
use crate::core::state::TickContext;

/// First fluid block number: the still-water source (`core.WaterSourceID`).
const WATER_SOURCE: u16 = 27;

/// Last fluid block number: the weakest flow (`core.WaterLevel7ID`). Fluid is
/// the eight continuous numbers source through level seven, mirroring Go
/// `core.IsFluid` in `packages/shared/core/fluid.go`.
const WATER_LEVEL_7: u16 = 34;

/// Dry farmland (`core.FarmlandDryID`, `packages/shared/core/block.go`).
const FARMLAND_DRY: u16 = 35;

/// Wet farmland (`core.FarmlandWetID`), the only hydrated form.
const FARMLAND_WET: u16 = 36;

/// Cells along one section edge (`core.SectionSize`,
/// `packages/shared/core/pos.go`).
const SECTION_SIZE: i32 = 16;

/// Lowest world block layer, inclusive (`core.MinY`).
const WORLD_MIN_Y: i32 = -64;

/// Exclusive world height bound (`core.MaxY`); the enqueue guard drops
/// candidates at or above it.
const WORLD_MAX_Y: i32 = 320;

/// Horizontal hydration radius in cells (`farmlandWetRadius`,
/// `packages/server/sim/realm/environment.go`).
const WET_RADIUS: i32 = 4;

/// Layers above the candidate its neighborhood scan covers
/// (`farmlandWetLayersAbove`): the scan reads the candidate layer and one
/// above, mirroring the reverse wake window.
const WET_LAYERS_ABOVE: i32 = 1;

/// Worst-case neighborhood reads per judgment (`farmlandWetNeighborReads`):
/// nine by nine columns times two layers, reserved whole before scanning.
const NEIGHBOR_READS: usize = 162;

/// One side of the full-height rescan halo: the section plus the radius on
/// both sides (`farmlandMoistureRescanSide`).
const RESCAN_SIDE: usize = 24;

/// Cells in one rescan halo: side squared times the full world height of 384
/// cells (`farmlandMoistureRescanCells`, `core.SectionsPerChunk` times
/// `core.SectionSize`).
const RESCAN_CELLS: usize = 221_184;

/// Dimensions in ascending identifier order, the order the Go realm advances
/// its per-dimension scheduler instances (`sortedFluidDimensions` in
/// `packages/server/sim/realm/environment.go`).
const SORTED_DIMENSIONS: [Dimension; 2] = [Dimension::OVERWORLD, Dimension::DEPTHS];

/// Coarse stand-in for the unreachable in-tick transaction refusal, the same
/// one-shape policy the fluid provider documents for non-budget failures.
const REFUSAL: ServerError = ServerError::InvalidInput { field: "farmland" };

/// Reports whether a block number is any fluid, mirroring Go `core.IsFluid`.
fn is_fluid(block: u16) -> bool {
    (WATER_SOURCE..=WATER_LEVEL_7).contains(&block)
}

/// Reports whether a block number is dry or wet farmland, mirroring Go
/// `core.IsFarmland` (`packages/shared/core/farming.go`).
fn is_farmland(block: u16) -> bool {
    (FARMLAND_DRY..=FARMLAND_WET).contains(&block)
}

/// Maps one dimension to its queue slot. Only the two authority dimensions
/// exist, so the mapping is total over every dimension a staged key can name.
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

/// Adds one offset to a world position, or nothing on arithmetic overflow.
fn checked_add_pos(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> Option<BlockPos> {
    Some(BlockPos::new(
        pos.x().checked_add(dx)?,
        pos.y().checked_add(dy)?,
        pos.z().checked_add(dz)?,
    ))
}

/// Due-order key for one queued candidate: `(due, chunkX, chunkZ, y, z, x)`,
/// mirroring Go `lessItem` plus `lessPos` in `packages/server/updates/queue.go`.
type DueOrderKey = (u64, i32, i32, i32, i32, i32);

/// Builds the due-order key for one queued candidate.
fn order_key(due: u64, key: ChunkKey, pos: BlockPos) -> DueOrderKey {
    (due, key.pos.x(), key.pos.z(), pos.y(), pos.z(), pos.x())
}

/// One dimension's candidate queue: the full due order as the map key with a
/// per-position index for earliest-due dedup, the two-sided structure of the
/// Go unified scheduler domain (indexed heap plus per-position dedup in
/// `packages/server/updates`).
#[derive(Clone, Debug, Default)]
struct CandidateQueue {
    by_due: BTreeMap<DueOrderKey, (ChunkKey, BlockPos)>,
    due_by_pos: BTreeMap<(ChunkKey, BlockPos), u64>,
}

impl CandidateQueue {
    /// Enqueues one candidate, keeping the earlier due tick when the position
    /// is already queued, mirroring the Go scheduler's "only earlier, never
    /// later" dedup.
    fn insert(&mut self, key: ChunkKey, pos: BlockPos, due: u64) {
        if let Some(&held) = self.due_by_pos.get(&(key, pos)) {
            if held <= due {
                return;
            }
            self.by_due.remove(&order_key(held, key, pos));
        }
        self.by_due.insert(order_key(due, key, pos), (key, pos));
        self.due_by_pos.insert((key, pos), due);
    }

    /// Removes and returns the earliest-due candidate at or before `now`, or
    /// nothing when no candidate is due. Removal on pop means a popped
    /// candidate is re-inserted exactly once when a guard defers it.
    fn pop_due(&mut self, now: u64) -> Option<(ChunkKey, BlockPos, u64)> {
        let (&order, &(key, pos)) = self.by_due.first_key_value()?;
        if order.0 > now {
            return None;
        }
        self.by_due.remove(&order);
        self.due_by_pos.remove(&(key, pos));
        Some((key, pos, order.0))
    }

    /// Counts the queued candidates still due at `now` without disturbing
    /// them.
    fn count_due(&self, now: u64) -> usize {
        self.by_due
            .keys()
            .take_while(|order| order.0 <= now)
            .count()
    }
}

/// Full-chunk rescan state over the shared head cursor, mirroring the Go
/// rescan machine (`farmlandMoistureRescanState`): a first-wins dedup set, a
/// pending FIFO, and one cursor that belongs to the head chunk.
#[derive(Clone, Debug, Default)]
struct RescanState {
    pending: Vec<ChunkKey>,
    queued: BTreeSet<ChunkKey>,
    cursor: usize,
}

impl RescanState {
    /// Enqueues one chunk for a full-halo rescan; a chunk already awaiting
    /// rescan keeps its cursor, mirroring Go `enqueueChunk` first-wins dedup.
    fn enqueue_chunk(&mut self, key: ChunkKey) {
        if self.queued.insert(key) {
            self.pending.push(key);
        }
    }

    /// Drops pending chunks that left the active Ready scope without reads
    /// and resets the shared cursor when the head changed or nothing is kept,
    /// mirroring Go `dropOutOfScope` exactly.
    fn drop_out_of_scope(&mut self, scope: &BTreeSet<ChunkKey>) {
        if self.pending.is_empty() {
            return;
        }
        let head = self.pending[0];
        let mut kept = Vec::with_capacity(self.pending.len());
        for key in std::mem::take(&mut self.pending) {
            if scope.contains(&key) {
                kept.push(key);
            } else {
                self.queued.remove(&key);
            }
        }
        self.pending = kept;
        if self.pending.is_empty() || self.pending[0] != head {
            self.cursor = 0;
        }
    }

    /// Retires the finished head and resets the cursor for the next chunk.
    fn pop(&mut self) {
        let key = self.pending.remove(0);
        self.queued.remove(&key);
        self.cursor = 0;
    }
}

/// Caller-owned farmland schedule: the per-dimension candidate queues plus
/// the pending full-chunk rescans. The serial tick reducer holds this value
/// across ticks; each provider call removes the work it starts and leaves
/// the rest untouched, so carried candidates resume next tick with their
/// original due tick and carried rescans resume at their cursor.
#[derive(Clone, Debug, Default)]
pub struct FarmlandSchedule {
    candidates: [CandidateQueue; 2],
    rescans: RescanState,
}

impl FarmlandSchedule {
    /// Creates an empty schedule with no pending work in either dimension.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueues one moisture candidate due at `due`, dropping positions at or
    /// above the exclusive world height bound or below the world floor, the
    /// guard Go `enqueueFarmlandMoisture` applies before its scheduler enqueue.
    /// Duplicates keep the earlier due tick. The candidate's chunk key is
    /// derived from the position, the same derivation the Go handler applies
    /// at settlement time, so the scope verdict cannot depend on the caller's
    /// key.
    pub fn enqueue_candidate(&mut self, key: ChunkKey, pos: BlockPos, due: u64) {
        if pos.y() < WORLD_MIN_Y || pos.y() >= WORLD_MAX_Y {
            return;
        }
        let key = ChunkKey {
            dimension: key.dimension,
            pos: chunk_of(pos),
        };
        self.candidates[dimension_slot(key.dimension)].insert(key, pos, due);
    }

    /// Wakes the reverse hydration window of one fluid-membership change: dy
    /// `-1..=0`, dz and dx `-4..=4` around the changed cell
    /// (`enqueueFarmlandMoistureAroundFluid`). The per-candidate world guard
    /// clips rows outside the world, which is how the world-floor window
    /// keeps only its valid layer. This is the event-wake surface the block
    /// -write enqueuers call; the schedule accepts the wake due from the
    /// caller the same way the fluid schedule does.
    pub fn enqueue_candidate_window(&mut self, key: ChunkKey, center: BlockPos, due: u64) {
        for dy in -WET_LAYERS_ABOVE..=0 {
            for dz in -WET_RADIUS..=WET_RADIUS {
                for dx in -WET_RADIUS..=WET_RADIUS {
                    let Some(pos) = checked_add_pos(center, dx, dy, dz) else {
                        continue;
                    };
                    self.enqueue_candidate(key, pos, due);
                }
            }
        }
    }

    /// Enqueues one full-chunk rescan, the scope-entry seeding Go
    /// `updateEnvironmentScope` performs for every newly active Ready chunk.
    pub fn enqueue_rescan(&mut self, key: ChunkKey) {
        self.rescans.enqueue_chunk(key);
    }

    /// Counts the queued candidates of one dimension, due or not.
    pub fn pending_candidates(&self, dimension: Dimension) -> usize {
        self.candidates[dimension_slot(dimension)].due_by_pos.len()
    }

    /// Reports the queued due tick of one candidate, if it is queued.
    pub fn candidate_due(&self, dimension: Dimension, pos: BlockPos) -> Option<u64> {
        let key = ChunkKey {
            dimension,
            pos: chunk_of(pos),
        };
        self.candidates[dimension_slot(dimension)]
            .due_by_pos
            .get(&(key, pos))
            .copied()
    }
}

/// The frozen batch entry for the moisture phase.
///
/// The schedule value travels with the serial reducer rather than the tick
/// context, so this entry stages nothing and reports zero work for the
/// well-shaped batch call, exactly like the fluid and acquisition entries
/// whose pending work arrives through caller-owned values. Any other shape is
/// the shared no-effect refusal: no charge, no staging, no commit.
pub fn run(_ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::Farmland
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

/// Bounded moisture settlement: candidates settle in due order per sorted
/// dimension against the two global budgets, then the rescan tail runs with
/// the reads left over.
///
/// Each popped candidate charges one [`WorkKind::FarmlandChecks`] unit; a
/// refusal defers it at its original due tick and pauses that dimension's
/// domain without charging, mirroring the Go inspection guard. An out-of-
/// scope candidate is consumed on the spot with zero block reads. The target
/// read charges one [`WorkKind::FarmlandReads`] unit before the cell is
/// read; an unready or non-farmland target is consumed there. A farmland
/// target reserves all [`NEIGHBOR_READS`] units before scanning — the target
/// read stays charged when the reservation refuses, exactly like the Go
/// meter — and the scan then hydrates on the first Ready fluid cell or
/// reports dry. Transitions stage through [`SystemRule::Farmland`] before
/// any rescan work runs. `examined` counts charged candidates, `applied`
/// counts staged transitions, `carried` counts the candidates left due when
/// the pass stopped, and `rejected` stays zero — moisture has no refusal
/// class, only skips and carries.
pub fn advance(
    schedule: &mut FarmlandSchedule,
    ctx: &mut TickContext<'_>,
    scope: &BTreeSet<ChunkKey>,
    now: u64,
) -> Result<PhaseReport, ServerError> {
    let mut examined = 0usize;
    let mut applied = 0usize;
    for dimension in SORTED_DIMENSIONS {
        let queue = &mut schedule.candidates[dimension_slot(dimension)];
        while let Some((key, pos, due)) = queue.pop_due(now) {
            // Global check budget exhausted: the candidate is deferred
            // without charging it and this dimension's domain pauses, the
            // same flattened pause the Go handler produces per dimension.
            if ctx.charge(WorkKind::FarmlandChecks, 1).is_err() {
                queue.insert(key, pos, due);
                break;
            }
            examined += 1;
            // A candidate whose chunk left the active Ready scope is
            // consumed on inspection with zero block reads; re-entering
            // scope rebuilds its moisture through the full-chunk rescan.
            if !scope.contains(&key) {
                continue;
            }
            // Read budget cannot pay even the target read: defer whole,
            // keeping the original due tick.
            if ctx.charge(WorkKind::FarmlandReads, 1).is_err() {
                queue.insert(key, pos, due);
                break;
            }
            let observed = {
                let view = ctx.read();
                view.observation(key.dimension, pos)
            };
            let Some(observed) = observed else {
                // Unready target: the read is charged and the candidate is
                // consumed, the Go `!ready` arm.
                continue;
            };
            if !is_farmland(observed.block) {
                // Stale candidate for a cell that is no longer farmland.
                continue;
            }
            // A neighborhood judgment is never split across ticks: reserve
            // the full worst case before scanning. The target read above
            // stays charged on refusal, mirroring the Go meter.
            if ctx.charge(WorkKind::FarmlandReads, NEIGHBOR_READS).is_err() {
                queue.insert(key, pos, due);
                break;
            }
            let wet = {
                let view = ctx.read();
                neighborhood_is_wet(&view, key.dimension, pos)
            };
            // Dry reports dry: the 30% air-above revert roll belongs to the
            // deterministic random-rules provider, not to moisture.
            let next = if wet { FARMLAND_WET } else { FARMLAND_DRY };
            if next != observed.block {
                let write = BlockWrite::try_new(observed, next).map_err(|_| REFUSAL)?;
                ctx.transaction()
                    .try_system(SystemRule::Farmland, vec![write])
                    .map_err(|_| REFUSAL)?;
                applied += 1;
            }
        }
    }
    let carried = schedule.candidates[0].count_due(now) + schedule.candidates[1].count_due(now);
    run_rescans(schedule, ctx, scope, now);
    Ok(PhaseReport {
        examined,
        applied,
        carried,
        rejected: 0,
    })
}

/// Scans the 162-cell hydration neighborhood in Go `farmlandIsWet` order (dy
/// outer, then dz, then dx) and reports whether any Ready fluid cell
/// hydrates the candidate. Unready cells never qualify, so the result does
/// not depend on the enumeration order.
fn neighborhood_is_wet(
    view: &crate::core::state::AuthorityReadView<'_>,
    dimension: Dimension,
    pos: BlockPos,
) -> bool {
    for dy in 0..=WET_LAYERS_ABOVE {
        for dz in -WET_RADIUS..=WET_RADIUS {
            for dx in -WET_RADIUS..=WET_RADIUS {
                let Some(cell) = checked_add_pos(pos, dx, dy, dz) else {
                    continue;
                };
                if view.block(dimension, cell).is_some_and(is_fluid) {
                    return true;
                }
            }
        }
    }
    false
}

/// Full-chunk rescan tail: drops out-of-scope pending chunks without reads,
/// then scans each pending halo head-first with the shared cursor while the
/// read budget left over from the candidate pass lasts.
///
/// Every scanned cell charges one [`WorkKind::FarmlandReads`] unit; a refused
/// charge pauses with the cursor unmoved, so the halo resumes whole next
/// tick. A discovered in-scope Ready farmland cell re-seeds as a candidate
/// due now, which the next tick's candidate pass settles — never the tick
/// that discovered it. A finished halo leaves the queue with the cursor
/// reset, mirroring Go `runFarmlandMoistureRescans` and `pop`.
fn run_rescans(
    schedule: &mut FarmlandSchedule,
    ctx: &mut TickContext<'_>,
    scope: &BTreeSet<ChunkKey>,
    now: u64,
) {
    schedule.rescans.drop_out_of_scope(scope);
    while !schedule.rescans.pending.is_empty() {
        let key = schedule.rescans.pending[0];
        while schedule.rescans.cursor < RESCAN_CELLS {
            // The read meter gates the cursor: a refused charge leaves the
            // halo at its cursor for the next tick.
            if ctx.charge(WorkKind::FarmlandReads, 1).is_err() {
                return;
            }
            let Some(pos) = rescan_position(key, schedule.rescans.cursor) else {
                // A position the cursor arithmetic cannot represent is
                // skipped rather than wrapped into another chunk; the cell
                // cannot be read, so it charges nothing.
                schedule.rescans.cursor += 1;
                continue;
            };
            schedule.rescans.cursor += 1;
            let block = ctx.read().block(key.dimension, pos);
            let Some(block) = block else {
                continue;
            };
            if !is_farmland(block) {
                continue;
            }
            let farmland_key = ChunkKey {
                dimension: key.dimension,
                pos: chunk_of(pos),
            };
            if scope.contains(&farmland_key) {
                schedule.candidates[dimension_slot(key.dimension)].insert(farmland_key, pos, now);
            }
        }
        schedule.rescans.pop();
    }
}

/// World position of one halo cell by cursor, mirroring Go
/// `farmlandMoistureRescanPosition`: x fastest, then z, then y from the world
/// floor, with the halo starting radius cells before the chunk origin.
/// Overflowing arithmetic reads as no position rather than wrapping into
/// another chunk.
fn rescan_position(key: ChunkKey, cursor: usize) -> Option<BlockPos> {
    let x = (cursor % RESCAN_SIDE) as i32;
    let z = ((cursor / RESCAN_SIDE) % RESCAN_SIDE) as i32;
    let y = (cursor / (RESCAN_SIDE * RESCAN_SIDE)) as i32;
    Some(BlockPos::new(
        key.pos
            .x()
            .checked_mul(SECTION_SIZE)?
            .checked_sub(WET_RADIUS)?
            .checked_add(x)?,
        WORLD_MIN_Y.checked_add(y)?,
        key.pos
            .z()
            .checked_mul(SECTION_SIZE)?
            .checked_sub(WET_RADIUS)?
            .checked_add(z)?,
    ))
}
