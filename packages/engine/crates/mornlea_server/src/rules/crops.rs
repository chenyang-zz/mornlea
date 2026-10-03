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
//! Actual source-player landing capture runs after native motion and fall settlement,
//! before Safe and late death. Its private fixed batch travels with the exclusive
//! source player book and settles copied coordinates here in the original write region.
//! The legacy public collector excludes those sessions and retains its fixture contract
//! for other players. Every settlement reads fresh, preserving duplicate idempotence.
//! Actual source players retain an inline travel/cell tracker and at most eight copied
//! Snow candidates in their exclusive book. Capture precedes Safe/death; reset clears
//! only the tracker, and fresh late settlement preserves original coordinates.
//! Legacy collection excludes actual source players. The per-tick generic schedule
//! retains source-disabled fixture behavior; passive Snow integration remains open.
//! Snow settlement follows trample settlement and precedes random sampling, so a
//! reverted cell is no longer farmland when the sampler visits it.
//!
//! Environmental plant removal owns its exact output policy. The accepted
//! world transaction settles the crop write and output batch together; the
//! preceding ground write remains independently committed on a crop fault.

use std::collections::BTreeMap;

use mornlea_domain::{BlockPos, Dimension, FiniteVec3};
use mornlea_storage::ItemStack;

use super::harvest;
use crate::core::contracts::{
    ActorKey, ActorLifecycle, BlockObservation, BlockWrite, DropBatch, DropSource, PhaseReport,
    RuleCall, RulePhase, ServerError, SystemRule,
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

/// Mature wheat (`core.WheatStage7ID`).
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

/// The flood and footprint branches use one source-compatible plant policy.
/// Human mining has different mature root yields and is deliberately separate.
pub(crate) fn environment_plant_outputs(
    seed: i64,
    tick: u64,
    dimension: Dimension,
    pos: BlockPos,
    block: u16,
) -> Option<Vec<ItemStack>> {
    let stack = |item, count| ItemStack {
        item,
        count,
        durability: 0,
    };
    Some(match block {
        37..=43 => vec![stack(34, 1)],
        WHEAT_MATURE => {
            let (wheat, seeds) = harvest::wheat(seed, tick, dimension.get().into(), pos);
            vec![stack(35, wheat), stack(34, seeds)]
        }
        46..=53 => vec![stack(40, 1)],
        54..=61 => vec![stack(41, 1)],
        89 => vec![stack(57, 1)],
        _ => return None,
    })
}

fn plant_drop_batch(
    ctx: &TickContext<'_>,
    observed: BlockObservation,
    stacks: Vec<ItemStack>,
) -> Option<DropBatch> {
    let delay = ctx.read().environment()?.tunables.drop_pickup_delay_ticks();
    let pos = observed.pos;
    DropBatch::try_new(
        DropSource::System {
            rule: SystemRule::Footprint,
            tick: ctx.read().tick(),
            target: pos,
        },
        observed.key.dimension,
        FiniteVec3::try_new([
            pos.x() as f32 + 0.5,
            pos.y() as f32 + 0.5,
            pos.z() as f32 + 0.5,
        ])
        .ok()?,
        stacks,
        delay,
    )
    .ok()
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

/// Caller-owned legacy footprint candidates and per-actor Snow trackers.
/// A caller may retain this value across calls; the actual reducer recreates it each tick.
/// Actual source-player trample coordinates travel in a separate private fixed batch.
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
/// - `Complete`: both writes and the prepared crop output landed.
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
/// Actual source trample candidates arrive through the private source book; legacy
/// footprints use a caller-owned schedule. This entry stages no work, like the other
/// batch entries. Any other shape retains the shared no-effect refusal.
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

/// Legacy trample body: collects landing edges of staged active players outside actual
/// source sessions in slice order and settles the whole pending batch, then clears the
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
    if ctx.read().environment().is_none() {
        return Err(ServerError::InvalidInput {
            field: "environment",
        });
    }
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

/// Collects legacy landing edges in slice order, excluding actual source sessions
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
        let ActorKey::Player(session) = actor.key else {
            // Only players trample farmland; passives graze it (Go collects
            // landing edges in the player advance loop only).
            continue;
        };
        if ctx.source_player_death_deferred(session) {
            continue;
        }
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
        if let ActorKey::Player(session) = actor.key
            && ctx.source_player_death_deferred(session)
        {
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

/// Settles copied source candidates in order through the existing fresh-cell transactions.
/// Environment and capacity refusal precede all reads; actor eligibility belongs to capture.
pub(crate) fn settle_captured_tramples(
    cells: &[FootprintCell],
    ctx: &mut TickContext<'_>,
) -> Result<PhaseReport, ServerError> {
    if ctx.read().environment().is_none() {
        return Err(ServerError::InvalidInput {
            field: "environment",
        });
    }
    if cells.len() > 32 {
        return Err(ServerError::Internal {
            invariant: "source player trample capacity",
        });
    }
    let mut applied = 0;
    for cell in cells {
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
    commit_trample(ctx, ground, Some(crop)) != TrampleCommit::Refused
}

/// Settles original copied cells through the existing fresh Snow transaction.
pub(crate) fn settle_captured_source_snow(
    cells: &[FootprintCell],
    ctx: &mut TickContext<'_>,
) -> Result<PhaseReport, ServerError> {
    if cells.len() > 8 {
        return Err(ServerError::Internal {
            invariant: "source player snow capacity",
        });
    }
    let mut applied = 0;
    for cell in cells {
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
/// exact `commitTrample` shape: the ground revert commits first, then the crop
/// removal and prepared output settle in one bounded transaction.
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
    let crop_batch = if let Some(crop) = crop {
        let Some(environment) = ctx.read().environment() else {
            return TrampleCommit::Refused;
        };
        let Some(stacks) = environment_plant_outputs(
            environment.seed,
            ctx.read().tick(),
            crop.key.dimension,
            crop.pos,
            crop.block,
        ) else {
            return TrampleCommit::Refused;
        };
        let Some(batch) = plant_drop_batch(ctx, crop, stacks) else {
            return TrampleCommit::Refused;
        };
        if ctx.read().check_drop_batch(&batch).is_err() {
            return TrampleCommit::Refused;
        }
        Some(batch)
    } else {
        None
    };
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
        .try_system_with_drops(
            SystemRule::Footprint,
            vec![crop_write],
            crop_batch.expect("crop batch preflighted"),
        )
        .is_err()
    {
        // The exceptional second-write fault row: ground→dirt stands, the
        // crop write and the drop stay uncommitted, nothing surfaces.
        return TrampleCommit::GroundOnly;
    }
    TrampleCommit::Complete
}

#[cfg(test)]
mod source_trample_tests {
    use super::*;
    use crate::core::contracts::{
        ChunkKey, EnvironmentState, FixtureState, RuleEffect, RuleTunables, ServerLimits,
        SleepState, TickBudget, WorkState,
    };
    use crate::core::state::AuthorityState;
    use crate::core::world::ReadyChunk;
    use mornlea_domain::{ChunkPos, Season, Weather, WorldState, WorldStateParts};
    use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};
    fn source_trample_world() -> WorldState {
        WorldState::try_new(WorldStateParts {
            world_time_ticks: 0,
            day_phase_offset: 0,
            weather: Weather::Clear,
            season: Season::Spring,
            season_progress: 0,
            temperature: 0,
        })
        .unwrap()
    }
    fn source_trample_authority() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap()
    }
    fn source_trample_context(a: &mut AuthorityState) -> TickContext<'_> {
        let f = FixtureState {
            runtime: vec![],
            actors: vec![],
            chunks: vec![],
            inventories: vec![],
            containers: vec![],
            work: WorkState::default(),
            sleep: SleepState {
                beds: vec![],
                day_phase_offset: 0,
                pending_offset: None,
            },
            projectiles: vec![],
            drops: vec![],
            world: source_trample_world(),
        };
        let mut c = TickContext::from_fixture(a, &f, TickBudget::full());
        let mut sections = vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: vec![],
                packed: vec![]
            };
            24
        ];
        for (pos, block) in [
            (BlockPos::ORIGIN, 36),
            (BlockPos::new(0, 1, 0), 44),
            (BlockPos::new(5, 0, 0), 35),
        ] {
            let section = ((pos.y() + 64) / 16) as usize;
            if sections[section].kind == StorageKind::Single {
                sections[section] = ContainerSnapshot {
                    kind: StorageKind::Direct,
                    bits: 15,
                    single: 0,
                    palette: vec![],
                    packed: vec![0; 1024],
                };
            }
            let index =
                (((pos.y() + 64) % 16) * 256 + (pos.z() & 15) * 16 + (pos.x() & 15)) as usize;
            sections[section].packed[index / 4] |= u64::from(block as u16) << ((index % 4) * 15);
        }
        c.preload_ready_chunk(
            ReadyChunk::try_new(
                ChunkKey {
                    dimension: Dimension::OVERWORLD,
                    pos: ChunkPos::new(0, 0),
                },
                1,
                1,
                Chunk {
                    sections,
                    drops: vec![Default::default(); 32],
                    furnaces: vec![Default::default(); 32],
                    chests: vec![Default::default(); 16],
                },
            )
            .unwrap(),
        );
        c.stage(RuleEffect::Environment(EnvironmentState {
            seed: 7,
            next_tick: 0,
            world_time: 0,
            day_phase_offset: 0,
            season_offset: 0,
            weather: Weather::Clear,
            weather_remaining: 0,
            difficulty: 0,
            tunables: RuleTunables::source_defaults(),
        }))
        .unwrap();
        c
    }
    fn source_trample_cell(pos: BlockPos) -> FootprintCell {
        FootprintCell {
            dimension: Dimension::OVERWORLD,
            pos,
        }
    }
    fn source_trample_snapshot(c: &TickContext<'_>) -> String {
        format!("{:?}", c.snapshot_state(source_trample_world()))
    }
    #[test]
    fn source_trample_captured_cells_use_fresh_transactions() {
        let mut a = source_trample_authority();
        let mut c = source_trample_context(&mut a);
        assert!(c.read().actors().is_empty());
        assert!(
            c.read()
                .runtime(ActorKey::Player(
                    crate::core::contracts::SessionKey::from_raw(1).unwrap()
                ))
                .is_none()
        );
        let cells = [
            source_trample_cell(BlockPos::ORIGIN),
            source_trample_cell(BlockPos::ORIGIN),
            source_trample_cell(BlockPos::new(5, 0, 0)),
        ];
        let before = cells;
        assert_eq!(
            settle_captured_tramples(&cells, &mut c).unwrap(),
            PhaseReport {
                examined: 3,
                applied: 2,
                carried: 0,
                rejected: 0
            }
        );
        for pos in [BlockPos::ORIGIN, BlockPos::new(5, 0, 0)] {
            assert_eq!(c.read().block(Dimension::OVERWORLD, pos), Some(3));
        }
        assert_eq!(
            c.read().block(Dimension::OVERWORLD, BlockPos::new(0, 1, 0)),
            Some(0)
        );
        let drops = c
            .read()
            .drops(ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, 0),
            })
            .iter()
            .map(|d| d.stack)
            .collect::<Vec<_>>();
        assert_eq!(
            drops,
            vec![
                ItemStack {
                    item: 35,
                    count: 1,
                    durability: 0
                },
                ItemStack {
                    item: 34,
                    count: 1,
                    durability: 0
                }
            ]
        );
        assert_eq!(cells, before);
    }
    #[test]
    fn source_trample_refusal_and_silent_skips() {
        let mut a = source_trample_authority();
        let c = source_trample_context(&mut a);
        let env = c.read().environment().unwrap().clone();
        // A detached context permits an explicit missing-environment fixture.
        let f = c.snapshot_state(source_trample_world());
        drop(c);
        let mut c = TickContext::from_fixture(&mut a, &f, TickBudget::full());
        let cells = [source_trample_cell(BlockPos::ORIGIN)];
        let before = source_trample_snapshot(&c);
        let error = ServerError::InvalidInput {
            field: "environment",
        };
        assert_eq!(settle_captured_tramples(&cells, &mut c), Err(error));
        assert_eq!(settle_captured_tramples(&[], &mut c), Err(error));
        assert_eq!(source_trample_snapshot(&c), before);
        c.stage(RuleEffect::Environment(env)).unwrap();
        let before = source_trample_snapshot(&c);
        let oversized = [cells[0]; 33];
        assert_eq!(
            settle_captured_tramples(&oversized, &mut c),
            Err(ServerError::Internal {
                invariant: "source player trample capacity"
            })
        );
        assert_eq!(source_trample_snapshot(&c), before);
        let quiet = [
            source_trample_cell(BlockPos::new(32, 0, 0)),
            source_trample_cell(BlockPos::new(2, 0, 0)),
        ];
        let copy = quiet;
        assert_eq!(
            settle_captured_tramples(&quiet, &mut c).unwrap(),
            PhaseReport {
                examined: 2,
                applied: 0,
                carried: 0,
                rejected: 0
            }
        );
        assert_eq!(source_trample_snapshot(&c), before);
        assert_eq!(quiet, copy);
        assert_eq!(cells, [source_trample_cell(BlockPos::ORIGIN)]);
    }
}

#[cfg(test)]
mod source_snow_tests {
    use super::*;
    use crate::core::contracts::{
        ChunkKey, FixtureState, ServerLimits, SleepState, TickBudget, WorkState,
    };
    use crate::core::state::AuthorityState;
    use crate::core::world::ReadyChunk;
    use mornlea_domain::{ChunkPos, Season, Weather, WorldState, WorldStateParts};
    use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};
    fn source_snow_world() -> WorldState {
        WorldState::try_new(WorldStateParts {
            world_time_ticks: 0,
            day_phase_offset: 0,
            weather: Weather::Clear,
            season: Season::Spring,
            season_progress: 0,
            temperature: 0,
        })
        .unwrap()
    }
    fn source_snow_authority() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap()
    }
    fn source_snow_context(a: &mut AuthorityState) -> TickContext<'_> {
        let f = FixtureState {
            runtime: vec![],
            actors: vec![],
            chunks: vec![],
            inventories: vec![],
            containers: vec![],
            work: WorkState::default(),
            sleep: SleepState {
                beds: vec![],
                day_phase_offset: 0,
                pending_offset: None,
            },
            projectiles: vec![],
            drops: vec![],
            world: source_snow_world(),
        };
        let mut c = TickContext::from_fixture(a, &f, TickBudget::full());
        let mut sections = vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: vec![],
                packed: vec![]
            };
            24
        ];
        for (pos, block) in [
            (BlockPos::ORIGIN, 1),
            (BlockPos::new(0, 1, 0), 87),
            (BlockPos::new(5, 1, 0), 1),
        ] {
            let section = ((pos.y() + 64) / 16) as usize;
            if sections[section].kind == StorageKind::Single {
                sections[section] = ContainerSnapshot {
                    kind: StorageKind::Direct,
                    bits: 15,
                    single: 0,
                    palette: vec![],
                    packed: vec![0; 1024],
                };
            }
            let index =
                (((pos.y() + 64) % 16) * 256 + (pos.z() & 15) * 16 + (pos.x() & 15)) as usize;
            sections[section].packed[index / 4] |= u64::from(block as u16) << ((index % 4) * 15);
        }
        c.preload_ready_chunk(
            ReadyChunk::try_new(
                ChunkKey {
                    dimension: Dimension::OVERWORLD,
                    pos: ChunkPos::new(0, 0),
                },
                1,
                1,
                Chunk {
                    sections,
                    drops: vec![Default::default(); 32],
                    furnaces: vec![Default::default(); 32],
                    chests: vec![Default::default(); 16],
                },
            )
            .unwrap(),
        );
        c
    }
    fn source_snow_cell(pos: BlockPos) -> FootprintCell {
        FootprintCell {
            dimension: Dimension::OVERWORLD,
            pos,
        }
    }
    fn source_snow_snapshot(c: &TickContext<'_>) -> String {
        format!(
            "{:?}/{:?}/{:?}",
            c.snapshot_state(source_snow_world()),
            c.events(),
            c.resident_snapshot().ready_snapshot()
        )
    }

    #[test]
    fn source_snow_captured_cells_are_fresh_and_bounded() {
        let mut a = source_snow_authority();
        let mut c = source_snow_context(&mut a);
        assert!(c.read().actors().is_empty());
        assert!(c.read().environment().is_none());
        let pos = BlockPos::new(0, 1, 0);
        let cells = [source_snow_cell(pos); 3];
        let copy = cells;
        assert_eq!(
            settle_captured_source_snow(&cells, &mut c).unwrap(),
            PhaseReport {
                examined: 3,
                applied: 3,
                carried: 0,
                rejected: 0
            }
        );
        assert_eq!(c.read().block(Dimension::OVERWORLD, pos), Some(0));
        assert_eq!(cells, copy);
        assert!(
            c.read()
                .drops(ChunkKey {
                    dimension: Dimension::OVERWORLD,
                    pos: ChunkPos::new(0, 0)
                })
                .is_empty()
        );
        let before = source_snow_snapshot(&c);
        let quiet = [
            source_snow_cell(BlockPos::new(32, 1, 0)),
            source_snow_cell(BlockPos::new(5, 1, 0)),
        ];
        let copy = quiet;
        assert_eq!(
            settle_captured_source_snow(&quiet, &mut c).unwrap(),
            PhaseReport {
                examined: 2,
                applied: 0,
                carried: 0,
                rejected: 0
            }
        );
        assert_eq!(quiet, copy);
        assert_eq!(source_snow_snapshot(&c), before);
        assert_eq!(
            settle_captured_source_snow(&[cells[0]; 9], &mut c),
            Err(ServerError::Internal {
                invariant: "source player snow capacity"
            })
        );
        assert_eq!(source_snow_snapshot(&c), before);
    }
}
