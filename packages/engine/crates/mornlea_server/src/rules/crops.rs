//! Actor trampling of farmland and snow-layer footprints.
//!
//! This provider owns the two batch phases that run after movement and before
//! random sampling in the frozen tick order (`Trample` then `SnowFootprint`,
//! the same write region Go reaches through `SettleTramples` and
//! `SettleSnowFootprints` in `packages/server/sim/entity/tick.go`: after
//! subscription reconciliation, before the random tick). Mirrored Go rows,
//! each cited at its site:
//!
//! - `packages/server/sim/entity/trample.go` (`noteTrampleLanding`): a
//!   player's landing edge — airborne the previous authority tick, grounded
//!   this tick, the same judgment as fall damage — collects every support-
//!   layer cell its collision box horizontally covers. The support layer sits
//!   at `floor(y - 1e-4)` (`physics.GroundProbe`, the existing ground-probe
//!   tolerance, reused so trample and physics read the same scale), the half
//!   width is `physics.PlayerWidth`/2 = 0.3, and coverage is strict
//!   (positive-measure) intersection, so at most a 2x2 column overlap is
//!   collected, enumerated X outer then Z with append-only staging.
//! - The same file (`settleTrampleCell`): settlement reads fresh, so a cell
//!   an earlier candidate already reverted reads as non-farmland and passes —
//!   same-cell duplicates settle exactly once regardless of order. A bare
//!   cell reverts farmland to dirt through the accepted transaction and MUST
//!   NOT mint a drop; a crop cell consumes the crop drop-capacity permitting,
//!   and a full drop budget abandons the whole cell silently (the
//!   `RejectDropCapacity`-shaped all-or-nothing of
//!   `TestTrampleCapacityFailureKeepsCellIntact`).
//! - The same file (`commitTrample`): the crop-bearing path commits two
//!   ordered single writes — ground revert first, crop removal second — and
//!   only a fully landed pair lets the prepared drop commit. The second-write
//!   refusal is the frozen exceptional fault row: ground→dirt stands while
//!   the crop and the drop stay uncommitted, silently. That outcome is why
//!   this path is an ordered stage and never one all-or-nothing transaction.
//! - `packages/server/sim/entity/snow_footprint.go` (`noteSnowFootprint`,
//!   `settleSnowFootprints`): players and passives share one mechanism. Each
//!   grounded tick accumulates the actor's horizontal travel; crossing the
//!   0.6-block stride samples the foot cell (`blockPosOf`, floor of each
//!   component — deliberately without the ground-probe subtraction, because
//!   the entity stands inside the snow-layer cell above its support).
//!   Airborne ticks accumulate nothing and reset nothing, the last sampled
//!   cell is remembered until the actor leaves it, and one call registers at
//!   most one cell, so an entity writes at most one cell per tick. Settlement
//!   reads fresh and lowers one tier (tier 1 shatters to air) through the
//!   accepted transaction, silently skipping non-snow, unobserved and
//!   refused cells.
//!
//! Rust adaptation of the two-phase split. The Go source splits collection
//! (physics phase) from settlement (write region) because its stage-order
//! contract forbids block writes before reconciliation; here both phases run
//! inside the write region and the context already carries each actor's
//! pre-step pose beside its post-motion pose, so each body call collects the
//! landing edges (or stride samples) from the staged actors and settles them
//! in the same call. Pending cells and the per-actor snow trackers live in a
//! caller-owned [`FootprintSchedule`] because the frozen [`RuleCall`] record
//! cannot carry queue contents and [`TickContext`] exposes no queue port;
//! the serial tick reducer owns the schedule value across ticks, the same
//! way the fluid and farmland schedules travel. Actors are enumerated in the
//! staged slice order — the frozen session-ascending order Go collects in
//! (`advanceActivePlayers`), which the reducer owns at staging; the provider
//! preserves it. Snow settlement runs after trample settlement in the tick
//! order, and both precede random sampling, so a cell this provider reverts
//! is no longer farmland when the sampler visits it.
//!
//! Boundary: drop minting belongs to the drop node. This provider computes
//! the would-be stack count and enforces the per-chunk capacity preflight
//! (the frozen full-capacity row), but stages no [`crate::core::contracts::RuleEffect::Drops`]
//! and mints no records — the passive-lifecycle provider's terminal records
//! follow the same no-loot discipline.

use std::collections::BTreeMap;

use mornlea_domain::{BlockPos, ChunkPos, Dimension};

use crate::core::contracts::{
    ActorKey, ActorLifecycle, BlockObservation, BlockWrite, ChunkKey, PhaseReport, RuleCall,
    RulePhase, ServerError, SystemRule,
};
use crate::core::state::TickContext;

/// Air cell (`core.AirID`, `packages/shared/core/block.go`).
const AIR: u16 = 0;

/// Dirt, the trampled farmland's revert target (`core.DirtID`).
const DIRT: u16 = 3;

/// Dry farmland (`core.FarmlandDryID`).
const FARMLAND_DRY: u16 = 35;

/// Wet farmland (`core.FarmlandWetID`).
const FARMLAND_WET: u16 = 36;

/// Mature wheat (`core.WheatStage7ID`), the only crop whose trample yields
/// two stacks (wheat plus seeds, `CropYieldRolls`).
const WHEAT_MATURE: u16 = 44;

/// First snow-layer form (`core.SnowLayer1BlockID`).
const SNOW_LAYER_FIRST: u16 = 85;

/// Last snow-layer form (`core.SnowLayer4BlockID`).
const SNOW_LAYER_LAST: u16 = 88;

/// Ground-support probe tolerance (`physics.GroundProbe`,
/// `packages/shared/physics/types.go`): keeps a foot at an exact block top on
/// the support block below.
const GROUND_PROBE: f32 = 1e-4;

/// Player half width (`physics.PlayerWidth`/2 = 0.3): at most two columns per
/// axis are strictly covered.
const HALF_WIDTH: f32 = 0.3;

/// Horizontal travel that triggers one footprint sample
/// (`snowFootprintStride`, `packages/server/sim/entity/snow_footprint.go`).
const SNOW_STRIDE: f32 = 0.6;

/// Fixed drop slots one chunk holds (`core.DropsPerChunk`,
/// `packages/shared/core/drop.go`), the same budget the human mining
/// preflight enforces in `crate::core::mutation`.
const DROPS_PER_CHUNK: usize = 32;

/// Reports whether a block number is dry or wet farmland (`core.IsFarmland`,
/// `packages/shared/core/farming.go`).
fn is_farmland(block: u16) -> bool {
    (FARMLAND_DRY..=FARMLAND_WET).contains(&block)
}

/// Reports whether a block number is any crop stage — wheat, potato or
/// carrot (`core.IsCrop`, `packages/shared/core/farming.go`; 45 is the solid
/// workbench between the ranges).
fn is_crop(block: u16) -> bool {
    (37..=44).contains(&block) || (46..=53).contains(&block) || (54..=61).contains(&block)
}

/// Snow-layer tier of one block number, `core.SnowLayerTier`
/// (`packages/shared/core/block.go`): tier 1..=4, or nothing for non-layers.
fn snow_layer_tier(block: u16) -> Option<u8> {
    if (SNOW_LAYER_FIRST..=SNOW_LAYER_LAST).contains(&block) {
        Some((block - SNOW_LAYER_FIRST + 1) as u8)
    } else {
        None
    }
}

/// Chunk column of a world position (`core.BlockPos.Chunk`,
/// `packages/shared/core/pos.go`; the arithmetic shift floors negatives).
fn chunk_of(pos: BlockPos) -> ChunkPos {
    ChunkPos::new(pos.x() >> 4, pos.z() >> 4)
}

/// One collected footprint candidate cell. Collection records geometry only
/// (`tramplePendingCell` / `snowFootprintCell`): whether the cell is
/// farmland or snow is decided by the settlement's fresh reads, and the
/// dimension travels with the position because a bare `BlockPos` cannot name
/// one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FootprintCell {
    pub dimension: Dimension,
    pub pos: BlockPos,
}

/// One entity's transient snow-footprint accumulator
/// (`snowFootprintTracker`): travel since the last sample plus the last
/// sampled cell, remembered (whether or not that sample wrote) so the same
/// cell is never re-sampled before the entity leaves it. Purely in-tick
/// serial state, never saved.
#[derive(Clone, Copy, Debug)]
struct SnowTracker {
    travel: f32,
    cell: BlockPos,
    cell_valid: bool,
}

impl SnowTracker {
    fn new() -> Self {
        Self {
            travel: 0.0,
            cell: BlockPos::ORIGIN,
            cell_valid: false,
        }
    }
}

/// Caller-owned footprint schedule: the pending candidate cells of both
/// phases plus the per-actor snow trackers. The serial tick reducer holds
/// this value across ticks; each body call removes the work it settles and
/// leaves the rest untouched, mirroring the Go engine-owned pending buffers
/// that clear every settlement.
#[derive(Clone, Debug, Default)]
pub struct FootprintSchedule {
    trample_pending: Vec<FootprintCell>,
    snow_pending: Vec<FootprintCell>,
    trackers: BTreeMap<ActorKey, SnowTracker>,
}

impl FootprintSchedule {
    /// Creates an empty schedule with no pending work and no trackers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts the pending trample candidates not yet settled.
    pub fn trample_pending(&self) -> usize {
        self.trample_pending.len()
    }

    /// Counts the pending snow candidates not yet settled.
    pub fn snow_pending(&self) -> usize {
        self.snow_pending.len()
    }
}

/// Settlement outcome of one trample candidate's [`commit_trample`], the
/// three-way shape of the Go `commitTrample` bool plus its ground-first
/// side effect:
///
/// - `Complete`: both writes landed; the drop lane may proceed.
/// - `GroundOnly`: the ground revert landed but the crop write was refused —
///   the frozen second-write fault row. Ground→dirt stands; the crop and the
///   drop stay uncommitted; nothing surfaces to the caller (Go returns
///   `false` and discards the prepared drop batch).
/// - `Refused`: nothing committed; the whole cell is abandoned silently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrampleCommit {
    Complete,
    GroundOnly,
    Refused,
}

/// The frozen batch entry for both footprint phases.
///
/// The schedule value travels with the serial reducer rather than the tick
/// context, so this entry stages nothing and reports zero work for the
/// well-shaped batch calls, exactly like the fluid, acquisition and farmland
/// entries whose pending work arrives through caller-owned values. Any other
/// shape is the shared no-effect refusal: no staging, no commit, no charge.
pub fn run(_ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if (!matches!(call.phase, RulePhase::Trample | RulePhase::SnowFootprint))
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

/// Trample-phase body: collects the landing edges of every staged active
/// player in slice order and settles the whole pending batch, then clears the
/// pending buffer (never carried across ticks, like Go `settleTramples`).
///
/// Each candidate is examined with fresh reads, so a candidate whose ground
/// an earlier candidate in the same batch already reverted reads as
/// non-farmland and passes — the dual-landing idempotence. `examined` counts
/// collected candidates, `applied` counts candidates whose ground revert
/// committed (including the second-write fault row, where the ground revert
/// is the part that stands), `carried` stays zero because the batch never
/// splits, and `rejected` stays zero: trampling has no refusal channel, only
/// silent skips (`settleTrampleCell`).
pub fn settle_tramples(
    schedule: &mut FootprintSchedule,
    ctx: &mut TickContext<'_>,
) -> Result<PhaseReport, ServerError> {
    collect_trample_landings(schedule, ctx);
    let cells = std::mem::take(&mut schedule.trample_pending);
    let mut applied = 0usize;
    for cell in &cells {
        if settle_trample_cell(ctx, cell) {
            applied += 1;
        }
    }
    Ok(PhaseReport {
        examined: cells.len(),
        applied,
        carried: 0,
        rejected: 0,
    })
}

/// Snow-footprint-phase body: accumulates each staged active player's and
/// passive's horizontal travel, samples foot cells at stride crossings, then
/// settles the whole pending batch and clears it (Go `settleSnowFootprints`).
/// Trackers persist in the schedule across ticks; a dead actor simply stops
/// being enumerated and its stale entry is inert.
///
/// `examined` counts sampled candidates, `applied` counts tier reductions
/// that committed, `carried` stays zero and `rejected` stays zero — footprints
/// have no refusal channel either.
pub fn settle_snow_footprints(
    schedule: &mut FootprintSchedule,
    ctx: &mut TickContext<'_>,
) -> Result<PhaseReport, ServerError> {
    collect_snow_samples(schedule, ctx);
    let cells = std::mem::take(&mut schedule.snow_pending);
    let mut applied = 0usize;
    for cell in &cells {
        if settle_snow_cell(ctx, cell) {
            applied += 1;
        }
    }
    Ok(PhaseReport {
        examined: cells.len(),
        applied,
        carried: 0,
        rejected: 0,
    })
}

/// Collects the landing edges of every staged active player in slice order
/// (`noteTrampleLanding`, called on the same edge as fall damage). The
/// post-motion pose supplies the landed position; the pre-step snapshot
/// supplies the airborne prior tick. Candidates append X outer then Z inner,
/// at most 2x2 cells per actor.
fn collect_trample_landings(schedule: &mut FootprintSchedule, ctx: &TickContext<'_>) {
    let view = ctx.read();
    for actor in view.actors() {
        if actor.lifecycle != ActorLifecycle::Active || !actor.motion.on_ground() {
            continue;
        }
        let ActorKey::Player(_) = actor.key else {
            // Only players trample farmland; passives graze it (Go collects
            // landing edges in the player advance loop only).
            continue;
        };
        let Some(pre) = view.pre_step_motion(actor.key) else {
            // No certified pre-step pose: the landing edge cannot be judged.
            continue;
        };
        if pre.on_ground() {
            continue;
        }
        append_covered_cells(
            &mut schedule.trample_pending,
            actor.dimension,
            actor.motion.position().get(),
        );
    }
}

/// Appends every strictly covered support-layer cell of one landed position,
/// the `noteTrampleLanding` geometry: support Y at `floor(y - 1e-4)`, column
/// overlap `floor(x - 0.3) ..= ceil(x + 0.3) - 1` (zero-measure edge contact
/// excluded), X outer then Z inner.
fn append_covered_cells(out: &mut Vec<FootprintCell>, dimension: Dimension, position: [f32; 3]) {
    let support_y = (position[1] - GROUND_PROBE).floor() as i32;
    let first_x = (position[0] - HALF_WIDTH).floor() as i32;
    let last_x = (position[0] + HALF_WIDTH).ceil() as i32 - 1;
    let first_z = (position[2] - HALF_WIDTH).floor() as i32;
    let last_z = (position[2] + HALF_WIDTH).ceil() as i32 - 1;
    for x in first_x..=last_x {
        for z in first_z..=last_z {
            out.push(FootprintCell {
                dimension,
                pos: BlockPos::new(x, support_y, z),
            });
        }
    }
}

/// Accumulates each staged active actor's grounded horizontal travel and
/// samples the foot cell at stride crossings (`noteSnowFootprint`). An
/// airborne tick accumulates nothing and resets nothing; a sample inside the
/// already-remembered cell clears the travel but registers nothing.
fn collect_snow_samples(schedule: &mut FootprintSchedule, ctx: &TickContext<'_>) {
    let view = ctx.read();
    for actor in view.actors() {
        if actor.lifecycle != ActorLifecycle::Active || !actor.motion.on_ground() {
            continue;
        }
        let Some(pre) = view.pre_step_motion(actor.key) else {
            continue;
        };
        let before = pre.position().get();
        let after = actor.motion.position().get();
        let tracker = schedule
            .trackers
            .entry(actor.key)
            .or_insert_with(SnowTracker::new);
        let dx = after[0] - before[0];
        let dz = after[2] - before[2];
        tracker.travel += (dx * dx + dz * dz).sqrt();
        if tracker.travel < SNOW_STRIDE {
            continue;
        }
        tracker.travel = 0.0;
        let cell = BlockPos::new(
            after[0].floor() as i32,
            after[1].floor() as i32,
            after[2].floor() as i32,
        );
        if tracker.cell_valid && tracker.cell == cell {
            continue;
        }
        tracker.cell = cell;
        tracker.cell_valid = true;
        schedule.snow_pending.push(FootprintCell {
            dimension: actor.dimension,
            pos: cell,
        });
    }
}

/// Settles one trample candidate with fresh reads (`settleTrampleCell`).
/// Returns whether the ground revert committed — the part of the settlement
/// that stands even on the second-write fault row.
fn settle_trample_cell(ctx: &mut TickContext<'_>, cell: &FootprintCell) -> bool {
    let Some(ground) = ctx.read().observation(cell.dimension, cell.pos) else {
        // Unobserved (unloaded or unready) cell: silently skipped, the Go
        // `!ready` arm.
        return false;
    };
    if !is_farmland(ground.block) {
        // Already settled this tick, or never farmland: the idempotence that
        // makes same-cell duplicates settle exactly once.
        return false;
    }
    let Some(crop_pos) = cell
        .pos
        .y()
        .checked_add(1)
        .map(|y| BlockPos::new(cell.pos.x(), y, cell.pos.z()))
    else {
        return false;
    };
    let crop = ctx.read().observation(cell.dimension, crop_pos);
    let Some(crop) = crop.filter(|observed| is_crop(observed.block)) else {
        // Bare farmland: revert to dirt through the accepted transaction; the
        // conversion itself MUST NOT mint a drop.
        return commit_trample(ctx, ground, None) == TrampleCommit::Complete;
    };
    // Every crop number carries a registered drop in the frozen
    // `core.BlockDrop` table (`packages/shared/core/item.go`: wheat seeds or
    // wheat, potato, carrot), so the Go `!ok` guard has no reachable arm
    // here. The mature wheat batch is two stacks (`CropYieldRolls`: one
    // wheat, one seeds); every other crop is one.
    let stacks = if crop.block == WHEAT_MATURE {
        2usize
    } else {
        1usize
    };
    let key = ChunkKey {
        dimension: cell.dimension,
        pos: chunk_of(crop_pos),
    };
    // Capacity preflight before any write (`PrepareDropBatch`): a full drop
    // budget abandons the whole cell silently — no revert, no crop removal,
    // no partial drop.
    if ctx.read().drops(key).len() + stacks > DROPS_PER_CHUNK {
        return false;
    }
    commit_trample(ctx, ground, Some(crop)) != TrampleCommit::Refused
}

/// Settles one snow candidate with a fresh read (`settleSnowFootprintCell`):
/// lowers one tier, tier 1 shatters to air, through the accepted transaction.
/// Non-snow, unobserved and refused cells are silently skipped — there is no
/// refusal channel, and the skip itself is observable (the layer stays).
fn settle_snow_cell(ctx: &mut TickContext<'_>, cell: &FootprintCell) -> bool {
    let Some(observed) = ctx.read().observation(cell.dimension, cell.pos) else {
        return false;
    };
    let Some(tier) = snow_layer_tier(observed.block) else {
        return false;
    };
    let next = if tier > 1 { observed.block - 1 } else { AIR };
    let Ok(write) = BlockWrite::try_new(observed, next) else {
        return false;
    };
    ctx.transaction()
        .try_system(SystemRule::Footprint, vec![write])
        .is_ok()
}

/// Commits one trample candidate's block writes as an ordered stage, the
/// exact `commitTrample` shape (two sequential single-write commits plus the
/// drop gate): the ground revert first through the accepted transaction, then
/// the crop removal, and only `Complete` lets the caller's drop lane proceed.
///
/// The crop-bearing path deliberately does not use one all-or-nothing
/// two-write transaction: the frozen second-write fault row requires that a
/// refused crop write leave the ground→dirt revert standing with the crop and
/// the drop uncommitted, which is exactly what the Go source's sequential
/// `SetBlock` pair produces (`commitTrample` returns `false` after the ground
/// `recordChange`, discarding the prepared drop batch). The batch body calls
/// this right after its fresh reads, so the stage runs atomically per write;
/// the fault row is reached when a crop basis captured before an interleaving
/// same-tick write fails validation at commit — the exposure window the
/// source's collect-then-settle split exists for.
///
/// A ground-write refusal abandons the whole cell (`Refused`, Go's ground arm
/// with nothing recorded); an unregistered replacement is unreachable for the
/// frozen constants and maps to the same silent abandonment.
pub fn commit_trample(
    ctx: &mut TickContext<'_>,
    ground: BlockObservation,
    crop: Option<BlockObservation>,
) -> TrampleCommit {
    let Ok(ground_write) = BlockWrite::try_new(ground, DIRT) else {
        return TrampleCommit::Refused;
    };
    if ctx
        .transaction()
        .try_system(SystemRule::Footprint, vec![ground_write])
        .is_err()
    {
        return TrampleCommit::Refused;
    }
    let Some(crop) = crop else {
        return TrampleCommit::Complete;
    };
    let Ok(crop_write) = BlockWrite::try_new(crop, AIR) else {
        return TrampleCommit::GroundOnly;
    };
    if ctx
        .transaction()
        .try_system(SystemRule::Footprint, vec![crop_write])
        .is_err()
    {
        // The exceptional second-write fault row: ground→dirt stands, the
        // crop write and the drop stay uncommitted, nothing surfaces.
        return TrampleCommit::GroundOnly;
    }
    TrampleCommit::Complete
}
