//! The single snapshot validation entry for an arriving companion plan.
//!
//! Pure functions over values only (no IO, no clock, no authority access):
//! the frozen planning snapshot the plan was asked for, the plan, and the
//! companion's current world. The frozen half replays the Go decode-time
//! rules (`validatePlanStepsAgainstSnapshot` on the frozen snapshot), whose
//! violations are invalid plans; the current half replays the Go
//! install-time revalidation (`plannerOutcomeMatchesCurrentAuthority`),
//! whose violations are world changes. Callers map the outcome to their own
//! rejection type.

use mornlea_domain::{BlockPos, ChunkPos};

use super::contracts::{AgentPlan, CurrentWorld, PlanBlock, PlanStep, PlanningSnapshot};

/// Why an arriving plan failed snapshot validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanCheckFailure {
    /// A frozen-snapshot (decode-time) rule refused the plan.
    InvalidPlan,
    /// The current world no longer supports the plan.
    WorldChanged,
}

/// Validates one plan against its frozen snapshot first and then against
/// the companion's current world. A missing current world is a world
/// change; it is only consulted once the frozen half has passed, the order
/// in which Go decodes the plan before the outcome reaches the tick.
pub fn validate_plan_against_snapshots(
    frozen: &PlanningSnapshot,
    plan: &AgentPlan,
    current: Option<&CurrentWorld>,
) -> Result<(), PlanCheckFailure> {
    validate_against_frozen(frozen, plan)?;
    let current = current.ok_or(PlanCheckFailure::WorldChanged)?;
    validate_against_current(frozen, plan, current)
}

/// Reports whether a block id is a planner-legal mine target.
///
/// The rule mirrors the Go `planMineableBlock`: crops, farmland, torches, and
/// wild grass are explicitly refused, and every other block needs a single
/// registered drop, so air, barrier, bedrock, fluids, snow layers, and any
/// unregistered id stay refused.
fn plan_mineable(block: u16) -> bool {
    if (35..=36).contains(&block)
        || (37..=44).contains(&block)
        || (46..=53).contains(&block)
        || (54..=61).contains(&block)
        || (71..=75).contains(&block)
        || block == 84
    {
        return false;
    }
    matches!(
        block,
        2 | 3 | 4 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 | 21
            | 22 | 23 | 24 | 25 | 26 | 45 | 62..=70 | 76..=83 | 89
    )
}

/// Maps a plan block to the item id the companion must hold.
///
/// The table mirrors the Go `planPlaceBlocks` reverse index over
/// `ItemPlacement`: each placeable block names its placing item.
fn plan_block_item(block: PlanBlock) -> u16 {
    match block {
        PlanBlock::Brick => 24,
        PlanBlock::Chest => 14,
        PlanBlock::Clay => 27,
        PlanBlock::Cobblestone => 16,
        PlanBlock::Dirt => 2,
        PlanBlock::Furnace => 8,
        PlanBlock::Glass => 23,
        PlanBlock::Grass => 3,
        PlanBlock::Gravel => 19,
        PlanBlock::IronBlock => 9,
        PlanBlock::Leaves => 22,
        PlanBlock::LightBlock => 15,
        PlanBlock::MossyCobblestone => 29,
        PlanBlock::OakLog => 20,
        PlanBlock::OakPlanks => 21,
        PlanBlock::RoofTile => 26,
        PlanBlock::Sand => 18,
        PlanBlock::SmoothStone => 17,
        PlanBlock::SnowBlock => 28,
        PlanBlock::Stone => 1,
        PlanBlock::StoneBrick => 4,
        PlanBlock::WhiteWool => 25,
        PlanBlock::Workbench => 38,
    }
}

/// Derives the chunk column of a block position with arithmetic shift,
/// mirroring the Go `BlockPos.Chunk` rule for negative coordinates.
fn target_chunk(target: BlockPos) -> ChunkPos {
    ChunkPos::new(target.x() >> 4, target.z() >> 4)
}

/// Reads one dense terrain cell: the position must sit inside the frozen
/// `[33, 17, 33]` projection and its column must be marked ready.
fn terrain_lookup(
    origin: BlockPos,
    ready_columns: &[u8],
    blocks: &[u16],
    target: BlockPos,
) -> Option<u16> {
    let dx = target.x().checked_sub(origin.x())?;
    let dy = target.y().checked_sub(origin.y())?;
    let dz = target.z().checked_sub(origin.z())?;
    if !(0..33).contains(&dx) || !(0..17).contains(&dy) || !(0..33).contains(&dz) {
        return None;
    }
    let column = (dx * 33 + dz) as usize;
    if ready_columns.get(column / 8)? & (1 << (column % 8)) == 0 {
        return None;
    }
    blocks.get(((dx * 17 + dy) * 33 + dz) as usize).copied()
}

/// Checks every plan step against the frozen snapshot the plan was asked
/// for, in step order: a follow target must be in the frozen online set,
/// a mine target must read from the frozen projection and be mineable
/// there, and the placing item of a place block must be held in the
/// frozen inventory. These are the Go decode-time rules
/// (`validatePlanStepsAgainstSnapshot`); a violation is an invalid plan,
/// never a world change. The place block registry is enforced by the
/// `PlanBlock` type itself.
fn validate_against_frozen(
    frozen: &PlanningSnapshot,
    plan: &AgentPlan,
) -> Result<(), PlanCheckFailure> {
    for step in &plan.steps {
        match step {
            PlanStep::GoTo { .. } => {}
            PlanStep::Follow { player_id } => {
                if !frozen
                    .online_players
                    .iter()
                    .any(|player| player.player_id == *player_id)
                {
                    return Err(PlanCheckFailure::InvalidPlan);
                }
            }
            PlanStep::Mine { x, y, z } => {
                let block = terrain_lookup(
                    frozen.terrain.origin,
                    &frozen.terrain.ready_columns,
                    &frozen.terrain.blocks,
                    BlockPos::new(*x, *y, *z),
                )
                .ok_or(PlanCheckFailure::InvalidPlan)?;
                if !plan_mineable(block) {
                    return Err(PlanCheckFailure::InvalidPlan);
                }
            }
            PlanStep::Place { block, .. } => {
                let item = plan_block_item(*block);
                if !frozen
                    .companion
                    .inventory
                    .iter()
                    .any(|held| held.item == item && held.count >= 1)
                {
                    return Err(PlanCheckFailure::InvalidPlan);
                }
            }
        }
    }
    Ok(())
}

/// Revalidates every plan step against the current world.
///
/// Mine and place targets must read the same dense value frozen and now,
/// mine targets must stay minable, place blocks must stay held, follow
/// targets must stay online at the same position, and every mine or place
/// chunk revision must be present and unchanged on both sides. A missing
/// cell or revision fails closed, mirroring the Go authority rebuild.
fn validate_against_current(
    frozen_snapshot: &PlanningSnapshot,
    plan: &AgentPlan,
    world: &CurrentWorld,
) -> Result<(), PlanCheckFailure> {
    let terrain = &frozen_snapshot.terrain;
    for step in &plan.steps {
        match step {
            PlanStep::GoTo { .. } => {}
            PlanStep::Mine { x, y, z } => {
                let target = BlockPos::new(*x, *y, *z);
                let frozen = terrain_lookup(
                    terrain.origin,
                    &terrain.ready_columns,
                    &terrain.blocks,
                    target,
                )
                .ok_or(PlanCheckFailure::WorldChanged)?;
                let current = world
                    .blocks
                    .get(&target)
                    .copied()
                    .ok_or(PlanCheckFailure::WorldChanged)?;
                if frozen != current || !plan_mineable(current) {
                    return Err(PlanCheckFailure::WorldChanged);
                }
                check_revision(frozen_snapshot, world, target)?;
            }
            PlanStep::Place { x, y, z, block } => {
                let target = BlockPos::new(*x, *y, *z);
                let frozen = terrain_lookup(
                    terrain.origin,
                    &terrain.ready_columns,
                    &terrain.blocks,
                    target,
                )
                .ok_or(PlanCheckFailure::WorldChanged)?;
                let current = world
                    .blocks
                    .get(&target)
                    .copied()
                    .ok_or(PlanCheckFailure::WorldChanged)?;
                if frozen != current {
                    return Err(PlanCheckFailure::WorldChanged);
                }
                let item = plan_block_item(*block);
                if !world
                    .inventory
                    .iter()
                    .any(|held| held.item == item && held.count >= 1)
                {
                    return Err(PlanCheckFailure::WorldChanged);
                }
                check_revision(frozen_snapshot, world, target)?;
            }
            PlanStep::Follow { player_id } => {
                let frozen = frozen_snapshot
                    .online_players
                    .iter()
                    .find(|player| player.player_id == *player_id)
                    .map(|player| player.position.get())
                    .ok_or(PlanCheckFailure::WorldChanged)?;
                let current = world
                    .online_players
                    .get(player_id)
                    .copied()
                    .ok_or(PlanCheckFailure::WorldChanged)?;
                if frozen != current {
                    return Err(PlanCheckFailure::WorldChanged);
                }
            }
        }
    }
    Ok(())
}

/// Checks the planned target chunk revision is present and unchanged on both
/// sides; a missing entry fails closed.
fn check_revision(
    frozen_snapshot: &PlanningSnapshot,
    world: &CurrentWorld,
    target: BlockPos,
) -> Result<(), PlanCheckFailure> {
    let chunk = target_chunk(target);
    let frozen = frozen_snapshot
        .chunk_revisions
        .iter()
        .find(|revision| revision.pos == chunk)
        .map(|revision| revision.revision)
        .ok_or(PlanCheckFailure::WorldChanged)?;
    let current = world
        .chunk_revisions
        .get(&chunk)
        .copied()
        .ok_or(PlanCheckFailure::WorldChanged)?;
    if frozen != current {
        return Err(PlanCheckFailure::WorldChanged);
    }
    Ok(())
}
