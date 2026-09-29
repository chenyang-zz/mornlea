//! Bounded deterministic random rules over compact Ready chunks.
//!
//! The reducer owns active-set admission. This provider observes each prior
//! staged write, including the height cache, and uses checked transactions.

use std::{collections::BTreeSet, f64::consts::PI};

use crate::core::{
    contracts::{
        BlockObservation, BlockWrite, ChunkKey, EnvironmentState, PhaseReport, RuleCall, RulePhase,
        ServerError, SystemRule,
    },
    state::TickContext,
};
use mornlea_domain::{BlockPos, Dimension, Weather};
use mornlea_engine::native::{
    contracts::world::{TreeOp, TreeRequest},
    tree::NativeTree,
};

const AIR: u16 = 0;
const STONE: u16 = 2;
const DIRT: u16 = 3;
const GRASS: u16 = 4;
const SAND: u16 = 15;
const GRAVEL: u16 = 16;
const SNOW_BLOCK: u16 = 25;
const DRY: u16 = 35;
const WET: u16 = 36;
const SHORT_GRASS: u16 = 84;
const SNOW_FIRST: u16 = 85;
const SNOW_LAST: u16 = 88;
const SAPLING: u16 = 89;

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

/// Samples with replacement using the source section hash chain and bound.
pub fn sample_cells(
    seed: i64,
    tick: u64,
    key: ChunkKey,
    section: u8,
    attempts: u8,
) -> Result<Vec<u16>, ServerError> {
    if section >= 24 {
        return Err(ServerError::InvalidInput { field: "section" });
    }
    if attempts > 64 {
        return Err(ServerError::InvalidInput {
            field: "random_attempts",
        });
    }
    let mut hash = mix(seed as u64);
    for part in [
        tick,
        u64::from(key.dimension.get()),
        u64::from(key.pos.x() as u32),
        u64::from(key.pos.z() as u32),
        u64::from(section),
    ] {
        hash = mix(hash ^ part);
    }
    Ok((0..attempts)
        .map(|i| (mix(hash ^ u64::from(i)) % 4096) as u16)
        .collect())
}

fn roll(seed: i64, tick: u64, dim: Dimension, pos: BlockPos, salt: u64) -> u64 {
    let mut hash = mix(seed as u64 ^ salt);
    for part in [
        tick,
        u64::from(dim.get()),
        u64::from(pos.x() as u32),
        u64::from(pos.y() as u32),
        u64::from(pos.z() as u32),
    ] {
        hash = mix(hash ^ part);
    }
    hash
}

fn climate(env: &EnvironmentState, y: i32) -> f32 {
    let year = ((env.world_time % 288_000 + u64::from(env.season_offset) % 288_000) % 288_000)
        as f64
        / 288_000.0;
    let mut arc = ((0.5 + 0.15 * (2.0 * PI * year).sin()) * 24_000.0).round() as u32;
    arc -= arc % 2;
    let linear = ((env.world_time % 24_000 + u64::from(env.day_phase_offset)) % 24_000) as u32;
    let phase = if linear < arc {
        linear * 12_000 / arc
    } else {
        12_000 + (linear - arc) * 12_000 / (24_000 - arc)
    };
    let precip = if matches!(env.weather, Weather::Rain | Weather::Thunder) {
        -4.0
    } else {
        0.0
    };
    let lapse = if y > 64 {
        -1.25 * f64::from(y - 64)
    } else {
        0.0
    };
    (11.0
        + 19.0 * (2.0 * PI * year).sin()
        + 5.0 * (2.0 * PI * (f64::from(phase) - 6000.0) / 24_000.0).sin()
        + precip
        + lapse)
        .clamp(-40.0, 45.0) as f32
}

fn write(ctx: &mut TickContext<'_>, old: BlockObservation, new: u16) -> bool {
    let Ok(write) = BlockWrite::try_new(old, new) else {
        return false;
    };
    ctx.transaction()
        .try_system(SystemRule::RandomBlock, vec![write])
        .is_ok()
}

fn sky(ctx: &TickContext<'_>, dim: Dimension, pos: BlockPos) -> bool {
    ctx.read()
        .highest_non_air(dim, pos.x(), pos.z())
        .is_some_and(|top| top <= pos.y())
}

fn settle(
    ctx: &mut TickContext<'_>,
    env: &EnvironmentState,
    dim: Dimension,
    pos: BlockPos,
) -> bool {
    let Some(old) = ctx.read().observation(dim, pos) else {
        return false;
    };
    let block = old.block;
    let below = BlockPos::new(pos.x(), pos.y() - 1, pos.z());
    let above = BlockPos::new(pos.x(), pos.y() + 1, pos.z());
    let (seed, tick) = (env.seed, env.next_tick);
    let crop = [(37, 44), (46, 53), (54, 61)]
        .iter()
        .find(|(start, end)| (*start..=*end).contains(&block));
    if let Some((_, mature)) = crop {
        return block != *mature
            && ctx.read().block(dim, below) == Some(WET)
            && sky(ctx, dim, pos)
            && env.tunables.crop_growth_percent() > 0
            && roll(seed, tick, dim, pos, 0xc0ffee5eedca11ed) % 100
                < u64::from(env.tunables.crop_growth_percent())
            && write(ctx, old, block + 1);
    }
    if block == DRY {
        return ctx.read().block(dim, above) == Some(AIR)
            && roll(seed, tick, dim, pos, 0xfa1abb1edeadc0de) % 100 < 30
            && write(ctx, old, DIRT);
    }
    if block == SAPLING {
        if pos.y() > 311
            || !matches!(ctx.read().block(dim, below), Some(DIRT | GRASS))
            || !sky(ctx, dim, pos)
            || roll(seed, tick, dim, pos, 0x5341504c47524f57) & 7 != 0
        {
            return false;
        }
        let Ok(tree) = NativeTree.tree_blocks(&TreeRequest {
            seed,
            root: [pos.x(), pos.y(), pos.z()],
        }) else {
            return false;
        };
        let mut writes = Vec::with_capacity(tree.records().len());
        for record in tree.records() {
            let (Some(x), Some(z)) = (
                pos.x().checked_add(i32::from(record.offset[0])),
                pos.z().checked_add(i32::from(record.offset[2])),
            ) else {
                return false;
            };
            let target = BlockPos::new(x, pos.y() + i32::from(record.offset[1]), z);
            let Some(existing) = ctx.read().observation(dim, target) else {
                return false;
            };
            if target != pos && existing.block != AIR && existing.block != SHORT_GRASS {
                return false;
            }
            let Ok(write) = BlockWrite::try_new(existing, record.block) else {
                return false;
            };
            writes.push(write);
        }
        return ctx
            .transaction()
            .try_system(SystemRule::RandomBlock, writes)
            .is_ok();
    }
    if block == DIRT && roll(seed, tick, dim, pos, 0x4752535052454144) & 3 == 0 {
        let cover = ctx.read().block(dim, above);
        if cover == Some(AIR) || cover.is_some_and(|id| (SNOW_FIRST..=SNOW_LAST).contains(&id)) {
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (Some(x), Some(z)) = (pos.x().checked_add(dx), pos.z().checked_add(dz)) else {
                    continue;
                };
                if ctx.read().block(dim, BlockPos::new(x, pos.y(), z)) == Some(GRASS) {
                    if write(ctx, old, GRASS) {
                        return true;
                    }
                    break;
                }
            }
        }
    }
    if ![GRASS, DIRT, STONE, SAND, GRAVEL, SNOW_BLOCK].contains(&block) || pos.y() == 319 {
        return false;
    }
    let Some(layer) = ctx.read().observation(dim, above) else {
        return false;
    };
    let temp = climate(env, above.y());
    let precip = matches!(env.weather, Weather::Rain | Weather::Thunder);
    if (SNOW_FIRST..=SNOW_LAST).contains(&layer.block) {
        if temp > 2.0 {
            return write(
                ctx,
                layer,
                if layer.block == SNOW_FIRST {
                    AIR
                } else {
                    layer.block - 1
                },
            );
        }
        return temp <= 0.0
            && precip
            && layer.block < SNOW_LAST
            && sky(ctx, dim, above)
            && write(ctx, layer, layer.block + 1);
    }
    layer.block == AIR
        && temp <= 0.0
        && precip
        && ctx.read().highest_non_air(dim, pos.x(), pos.z()) == Some(pos.y())
        && write(ctx, layer, SNOW_FIRST)
}

/// Validates the frozen batch shape. Its active set is supplied to `advance`.
pub fn run(_ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::RandomBlock
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

/// Samples each unique Ready key in sorted section order. All key spans are
/// checked before mutation so an extreme coordinate cannot leave a prefix.
pub fn advance(ctx: &mut TickContext<'_>, active: &[ChunkKey]) -> Result<PhaseReport, ServerError> {
    let env = ctx
        .read()
        .environment()
        .cloned()
        .ok_or(ServerError::InvalidInput {
            field: "environment",
        })?;
    let keys: BTreeSet<_> = active.iter().copied().collect();
    for key in &keys {
        for coordinate in [key.pos.x(), key.pos.z()] {
            let base = i64::from(coordinate) * 16;
            if base < i64::from(i32::MIN) || base + 15 > i64::from(i32::MAX) {
                return Err(ServerError::InvalidInput { field: "chunk_key" });
            }
        }
    }
    let mut report = PhaseReport {
        examined: 0,
        applied: 0,
        carried: 0,
        rejected: 0,
    };
    for key in keys {
        if !ctx.read().ready_chunk(key) {
            continue;
        }
        let (base_x, base_z) = (key.pos.x() * 16, key.pos.z() * 16);
        for section in 0..24 {
            for cell in sample_cells(
                env.seed,
                env.next_tick,
                key,
                section,
                env.tunables.random_attempts(),
            )? {
                let pos = BlockPos::new(
                    base_x + i32::from(cell & 15),
                    -64 + i32::from(section) * 16 + i32::from(cell >> 8),
                    base_z + i32::from((cell >> 4) & 15),
                );
                report.examined += 1;
                if settle(ctx, &env, key.dimension, pos) {
                    report.applied += 1;
                }
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contracts::RuleTunables;

    #[test]
    fn climate_source_anchors() {
        let mut env = EnvironmentState {
            seed: 7,
            next_tick: 0,
            world_time: 7_800,
            day_phase_offset: 0,
            season_offset: 64_200,
            weather: Weather::Clear,
            weather_remaining: 0,
            difficulty: 0,
            tunables: RuleTunables::source_defaults(),
        };
        assert_eq!(climate(&env, 64), 30.0);
        assert_eq!(climate(&env, 88), 0.0);
        env.world_time = 4_200;
        env.season_offset = 211_800;
        assert_eq!(climate(&env, 64), -8.0);
        env.world_time = 6_000;
        env.season_offset = 282_000;
        env.weather = Weather::Rain;
        assert_eq!(climate(&env, 69), 0.75);
    }
}
