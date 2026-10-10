//! Ordered support sweeps over this tick's successful block mutations.
//!
//! Each pass snapshots the committed changed-cell ledger on entry. Its own
//! removals cannot recurse within the pass, while later passes see them.

use mornlea_domain::{BlockPos, FiniteVec3, registered_block};
use mornlea_storage::ItemStack;

use crate::core::{
    contracts::{
        BlockObservation, BlockWrite, DropBatch, DropSource, PhaseReport, RuleCall, RulePhase,
        ServerError, SystemRule,
    },
    state::TickContext,
};

const AIR: u16 = 0;
const DIRT: u16 = 3;
const GRASS: u16 = 4;
const LEAVES: u16 = 19;
const GLASS: u16 = 20;
const SHORT_GRASS: u16 = 84;
const SAPLING: u16 = 89;
const TORCH_FIRST: u16 = 71;
const TORCH_LAST: u16 = 75;
const BED_FIRST: u16 = 76;
const BED_LAST: u16 = 83;
const ITEM_SAPLING: u16 = 57;
const ITEM_TORCH: u16 = 44;
const ITEM_BED: u16 = 46;

fn offset(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> Option<BlockPos> {
    let y = pos.y().checked_add(dy)?;
    (-64..320)
        .contains(&y)
        .then(|| BlockPos::new(pos.x().wrapping_add(dx), y, pos.z().wrapping_add(dz)))
}

fn plant(block: u16) -> bool {
    (37..=44).contains(&block)
        || (46..=61).contains(&block)
        || block == SHORT_GRASS
        || block == SAPLING
}

fn torch_solid(block: u16) -> bool {
    registered_block(block)
        && block != AIR
        && !(27..=34).contains(&block)
        && !plant(block)
        && !(TORCH_FIRST..=TORCH_LAST).contains(&block)
        && block != 70
}

fn bed_solid(block: u16) -> bool {
    (35..=36).contains(&block)
        || registered_block(block)
            && block != AIR
            && block != GLASS
            && block != LEAVES
            && !(27..=34).contains(&block)
            && !plant(block)
            && !(62..=70).contains(&block)
}

fn drop_batch(
    ctx: &TickContext<'_>,
    env: &crate::core::contracts::EnvironmentState,
    target: BlockObservation,
    item: u16,
) -> DropBatch {
    DropBatch {
        source: DropSource::System {
            rule: SystemRule::Support,
            tick: ctx.read().tick(),
            target: target.pos,
        },
        dimension: target.key.dimension,
        origin: FiniteVec3::try_new([
            target.pos.x() as f32 + 0.5,
            target.pos.y() as f32 + 0.5,
            target.pos.z() as f32 + 0.5,
        ])
        .expect("finite cell center"),
        stacks: vec![ItemStack {
            item,
            count: 1,
            durability: 0,
        }],
        pickup_delay: env.tunables.drop_pickup_delay_ticks(),
    }
}

fn write_air(
    ctx: &mut TickContext<'_>,
    observed: BlockObservation,
    item: Option<u16>,
    env: &crate::core::contracts::EnvironmentState,
    report: &mut PhaseReport,
) {
    let write = BlockWrite::try_new(observed, AIR).expect("air is registered");
    let result = if let Some(item) = item {
        let batch = drop_batch(ctx, env, observed, item);
        ctx.transaction()
            .try_system_with_drops(SystemRule::Support, vec![write], batch)
    } else {
        ctx.transaction()
            .try_system(SystemRule::Support, vec![write])
    };
    match result {
        Ok(outcome) => report.applied += outcome.changed.len(),
        Err(_) => report.rejected += 1,
    }
}

fn torch_support(pos: BlockPos, block: u16) -> Option<BlockPos> {
    let (dx, dy, dz) = match block {
        71 => (0, -1, 0),
        72 => (-1, 0, 0),
        73 => (1, 0, 0),
        74 => (0, 0, -1),
        75 => (0, 0, 1),
        _ => return None,
    };
    offset(pos, dx, dy, dz)
}

fn bed_pair(pos: BlockPos, block: u16) -> Option<(BlockPos, BlockPos)> {
    if !(BED_FIRST..=BED_LAST).contains(&block) {
        return None;
    }
    let dir = (block - BED_FIRST) % 4;
    let (dx, dz) = match dir {
        0 => (0, 1),
        1 => (-1, 0),
        2 => (0, -1),
        _ => (1, 0),
    };
    if block < 80 {
        Some((pos, offset(pos, dx, 0, dz)?))
    } else {
        Some((offset(pos, -dx, 0, -dz)?, pos))
    }
}

/// Settles unsupported plants, torches, and beds after earlier block writers.
/// The four snapshots mirror the fixed Go order; only attempted atomic write
/// refusals enter `rejected`, and no gameplay event is published here.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::Support
        || call.actor.is_some()
        || call.command.is_some()
        || call.internal.is_some()
    {
        return Err(ServerError::InvalidInput { field: "support" });
    }
    let env = ctx
        .read()
        .environment()
        .cloned()
        .ok_or(ServerError::InvalidInput {
            field: "environment",
        })?;
    let mut report = PhaseReport {
        examined: 0,
        applied: 0,
        carried: 0,
        rejected: 0,
    };
    for pass in 0..4 {
        let changes = ctx.changed_blocks();
        report.examined += changes.len();
        for changed in changes {
            let dim = changed.key.dimension;
            let pos = changed.pos;
            match pass {
                0 => {
                    let Some(above) = offset(pos, 0, 1, 0) else {
                        continue;
                    };
                    let Some(target) = ctx.read().observation(dim, above) else {
                        continue;
                    };
                    if target.block != SHORT_GRASS {
                        continue;
                    }
                    if ctx
                        .read()
                        .block(dim, pos)
                        .is_some_and(|block| block != GRASS)
                    {
                        write_air(ctx, target, None, &env, &mut report);
                    }
                }
                1 => {
                    let Some(above) = offset(pos, 0, 1, 0) else {
                        continue;
                    };
                    let Some(target) = ctx.read().observation(dim, above) else {
                        continue;
                    };
                    if target.block != SAPLING {
                        continue;
                    }
                    if ctx
                        .read()
                        .block(dim, pos)
                        .is_some_and(|block| block != DIRT && block != GRASS)
                    {
                        write_air(ctx, target, Some(ITEM_SAPLING), &env, &mut report);
                    }
                }
                2 => {
                    for (dx, dy, dz) in [
                        (1, 0, 0),
                        (-1, 0, 0),
                        (0, 1, 0),
                        (0, -1, 0),
                        (0, 0, 1),
                        (0, 0, -1),
                    ] {
                        let Some(neighbor) = offset(pos, dx, dy, dz) else {
                            continue;
                        };
                        let Some(target) = ctx.read().observation(dim, neighbor) else {
                            continue;
                        };
                        if torch_support(neighbor, target.block) != Some(pos) {
                            continue;
                        }
                        if ctx.read().block(dim, pos).is_some_and(torch_solid) {
                            continue;
                        }
                        write_air(ctx, target, Some(ITEM_TORCH), &env, &mut report);
                    }
                }
                _ => {
                    let Some(above) = offset(pos, 0, 1, 0) else {
                        continue;
                    };
                    let Some(target) = ctx.read().observation(dim, above) else {
                        continue;
                    };
                    if !(BED_FIRST..=BED_LAST).contains(&target.block) {
                        continue;
                    }
                    if !ctx
                        .read()
                        .block(dim, pos)
                        .is_some_and(|block| !bed_solid(block))
                    {
                        continue;
                    }
                    let Some((foot, head)) = bed_pair(above, target.block) else {
                        continue;
                    };
                    let (Some(foot), Some(head)) = (
                        ctx.read().observation(dim, foot),
                        ctx.read().observation(dim, head),
                    ) else {
                        continue;
                    };
                    let writes = vec![
                        BlockWrite::try_new(foot, AIR).expect("air"),
                        BlockWrite::try_new(head, AIR).expect("air"),
                    ];
                    let batch = drop_batch(ctx, &env, target, ITEM_BED);
                    match ctx.transaction().try_system_with_drops(
                        SystemRule::Support,
                        writes,
                        batch,
                    ) {
                        Ok(outcome) => report.applied += outcome.changed.len(),
                        Err(_) => report.rejected += 1,
                    }
                }
            }
        }
    }
    Ok(report)
}
