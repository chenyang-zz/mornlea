//! Deterministic random-world replay against the source sampler.

use mornlea_domain::{BlockPos, ChunkPos, Dimension, Season, Weather, WorldState, WorldStateParts};
use mornlea_server::contracts::{
    ChunkKey, EnvironmentState, FixtureState, PhaseReport, RuleCall, RuleEffect, RulePhase,
    RuleTunables, ServerLimits, TickBudget,
};
use mornlea_server::core::world::ReadyChunk;
use mornlea_server::rules::random_blocks;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};

fn key() -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    }
}
fn neighbor_key() -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(1, 0),
    }
}
fn chunk(cells: &[(BlockPos, u16)]) -> Chunk {
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
    for (pos, block) in cells {
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
        let index = (((pos.y() + 64) % 16) * 256 + (pos.z() & 15) * 16 + (pos.x() & 15)) as usize;
        sections[section].packed[index / 4] |= u64::from(*block) << ((index % 4) * 15);
    }
    Chunk {
        sections,
        drops: vec![Default::default(); 32],
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    }
}
fn tunables(attempts: u8, growth: u8) -> RuleTunables {
    RuleTunables::try_new(
        RuleTunables::source_defaults().physics(),
        100,
        40,
        20,
        80,
        18,
        4000,
        32,
        1600,
        200,
        5,
        attempts,
        growth,
        6.0,
        1.62,
        10,
        40,
        6000,
        1.25,
    )
    .unwrap()
}
fn environment(tick: u64, attempts: u8, growth: u8) -> EnvironmentState {
    EnvironmentState {
        seed: 7,
        next_tick: tick,
        world_time: 0,
        day_phase_offset: 0,
        season_offset: 0,
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty: 0,
        tunables: tunables(attempts, growth),
    }
}
fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        0,
    )
    .unwrap()
}
fn world() -> WorldState {
    WorldState::try_new(WorldStateParts {
        day_phase_offset: 0,
        world_time_ticks: 0,
        weather: Weather::Clear,
        season: Season::Spring,
        season_progress: 0,
        temperature: 0,
    })
    .unwrap()
}
fn snapshot_run(
    chunks: &[(ChunkKey, Vec<(BlockPos, u16)>)],
    env: EnvironmentState,
    active: &[ChunkKey],
) -> (FixtureState, PhaseReport) {
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    for (chunk_key, cells) in chunks {
        ctx.preload_ready_chunk(ReadyChunk::try_new(*chunk_key, 1, 1, chunk(cells)).unwrap());
    }
    ctx.stage(RuleEffect::Environment(env)).unwrap();
    let report = random_blocks::advance(&mut ctx, active).unwrap();
    (ctx.snapshot_state(world()), report)
}
fn sampled_position(tick: u64, section: u8, attempts: u8) -> BlockPos {
    sampled_at_key(tick, key(), section, attempts)
}
fn sampled_at_key(tick: u64, chunk_key: ChunkKey, section: u8, attempts: u8) -> BlockPos {
    let index = random_blocks::sample_cells(7, tick, chunk_key, section, attempts).unwrap()[0];
    BlockPos::new(
        chunk_key.pos.x() * 16 + i32::from(index & 15),
        -64 + i32::from(section) * 16 + i32::from(index >> 8),
        chunk_key.pos.z() * 16 + i32::from((index >> 4) & 15),
    )
}
fn run_at(
    cells: &[(BlockPos, u16)],
    env: EnvironmentState,
    inspect: BlockPos,
) -> (Option<u16>, PhaseReport) {
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ctx.preload_ready_chunk(ReadyChunk::try_new(key(), 1, 1, chunk(cells)).unwrap());
    ctx.stage(RuleEffect::Environment(env)).unwrap();
    let report = random_blocks::advance(&mut ctx, &[key(), key()]).unwrap();
    (ctx.read().block(Dimension::OVERWORLD, inspect), report)
}
fn run_with_neighbor(
    cells: &[(BlockPos, u16)],
    env: EnvironmentState,
    inspect: BlockPos,
) -> (Option<u16>, PhaseReport) {
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ctx.preload_ready_chunk(ReadyChunk::try_new(key(), 1, 1, chunk(cells)).unwrap());
    ctx.preload_ready_chunk(ReadyChunk::try_new(neighbor_key(), 1, 1, chunk(&[])).unwrap());
    ctx.stage(RuleEffect::Environment(env)).unwrap();
    let report = random_blocks::advance(&mut ctx, &[key()]).unwrap();
    (ctx.read().block(Dimension::OVERWORLD, inspect), report)
}

#[test]
fn sampler_kat_and_endpoints() {
    let key = ChunkKey {
        dimension: Dimension::DEPTHS,
        pos: ChunkPos::new(5, -7),
    };
    assert_eq!(
        random_blocks::sample_cells(-42, 777, key, 9, 8).unwrap(),
        [3835, 318, 2309, 2970, 832, 2023, 3058, 3225]
    );
    assert!(
        random_blocks::sample_cells(-42, 777, key, 9, 0)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        random_blocks::sample_cells(-42, 777, key, 9, 64)
            .unwrap()
            .len(),
        64
    );
    assert!(random_blocks::sample_cells(-42, 777, key, 9, 65).is_err());
    assert!(random_blocks::sample_cells(-42, 777, key, 24, 1).is_err());
}

#[test]
fn sampled_crop_grows_once_and_mature_stays() {
    let pos = sampled_position(1, 8, 1);
    let below = BlockPos::new(pos.x(), pos.y() - 1, pos.z());
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ctx.preload_ready_chunk(
        ReadyChunk::try_new(key(), 1, 1, chunk(&[(pos, 37), (below, 36)])).unwrap(),
    );
    ctx.stage(RuleEffect::Environment(environment(1, 1, 100)))
        .unwrap();
    let report = random_blocks::advance(&mut ctx, &[key()]).unwrap();
    assert_eq!(report.examined, 24);
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, pos), Some(38));
    assert_eq!(report.applied, 1);
}

#[test]
fn unavailable_and_extreme_keys_are_refused_without_writes() {
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ctx.stage(RuleEffect::Environment(environment(1, 1, 100)))
        .unwrap();
    assert_eq!(
        random_blocks::advance(&mut ctx, &[key(), key()])
            .unwrap()
            .examined,
        0
    );
    let extreme = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(i32::MAX, 0),
    };
    assert!(random_blocks::advance(&mut ctx, &[key(), extreme]).is_err());
}

#[test]
fn all_crop_families_require_wet_support_and_sky() {
    let tick = 1;
    let pos = sampled_position(tick, 8, 1);
    let below = BlockPos::new(pos.x(), pos.y() - 1, pos.z());
    let roof = BlockPos::new(pos.x(), pos.y() + 2, pos.z());
    for (start, mature) in [(37, 44), (46, 53), (54, 61)] {
        assert_eq!(
            run_at(&[(pos, start), (below, 36)], environment(tick, 1, 100), pos).0,
            Some(start + 1)
        );
        assert_eq!(
            run_at(
                &[(pos, mature), (below, 36)],
                environment(tick, 1, 100),
                pos
            )
            .0,
            Some(mature)
        );
        assert_eq!(
            run_at(&[(pos, start), (below, 35)], environment(tick, 1, 100), pos).0,
            Some(start)
        );
        assert_eq!(
            run_at(
                &[(pos, start), (below, 36), (roof, 2)],
                environment(tick, 1, 100),
                pos
            )
            .0,
            Some(start)
        );
        assert_eq!(
            run_at(&[(pos, start), (below, 36)], environment(tick, 1, 0), pos).0,
            Some(start)
        );
    }
}

#[test]
fn snow_grows_caps_and_melts_under_roof() {
    let tick = 2;
    let ground = sampled_position(tick, 8, 1);
    let layer = BlockPos::new(ground.x(), ground.y() + 1, ground.z());
    let roof = BlockPos::new(ground.x(), ground.y() + 2, ground.z());
    let mut cold = environment(tick, 1, 50);
    cold.weather = Weather::Rain;
    cold.season_offset = 216_000;
    cold.world_time = 6_000;
    assert_eq!(run_at(&[(ground, 4)], cold.clone(), layer).0, Some(85));
    assert_eq!(
        run_at(&[(ground, 4), (layer, 85)], cold.clone(), layer).0,
        Some(86)
    );
    assert_eq!(
        run_at(&[(ground, 4), (layer, 88)], cold.clone(), layer).0,
        Some(88)
    );
    assert_eq!(
        run_at(&[(ground, 4), (roof, 2)], cold.clone(), layer).0,
        Some(0)
    );
    let mut hot = environment(tick, 1, 50);
    hot.world_time = 6_000;
    hot.season_offset = 66_000;
    assert_eq!(
        run_at(&[(ground, 4), (layer, 86), (roof, 2)], hot, layer).0,
        Some(85)
    );
}

#[test]
fn snow_holds_inside_temperature_band() {
    let (tick, ground) = (0..128)
        .find_map(|tick| {
            let pos = sampled_position(tick, 8, 1);
            (pos.y() == 68).then_some((tick, pos))
        })
        .expect("sample must reach the hold-band height");
    let layer = BlockPos::new(ground.x(), 69, ground.z());
    let mut hold = environment(tick, 1, 50);
    hold.world_time = 6_000;
    hold.season_offset = 282_000;
    hold.weather = Weather::Rain;
    assert_eq!(run_at(&[(ground, 4), (layer, 86)], hold, layer).0, Some(86));
}

#[test]
fn repeated_sample_reads_new_layer_and_height() {
    let (tick, ground) = (0..128)
        .find_map(|tick| {
            let cells = random_blocks::sample_cells(7, tick, key(), 8, 64).unwrap();
            let repeated = cells
                .iter()
                .copied()
                .find(|cell| cells.iter().filter(|other| *other == cell).count() == 2)?;
            Some((
                tick,
                BlockPos::new(
                    i32::from(repeated & 15),
                    64 + i32::from(repeated >> 8),
                    i32::from((repeated >> 4) & 15),
                ),
            ))
        })
        .expect("64 samples should repeat a cell twice");
    let layer = BlockPos::new(ground.x(), ground.y() + 1, ground.z());
    let mut cold = environment(tick, 64, 50);
    cold.weather = Weather::Rain;
    cold.season_offset = 216_000;
    cold.world_time = 6_000;
    let (block, report) = run_at(&[(ground, 4)], cold, layer);
    assert_eq!(block, Some(86));
    assert_eq!(report.applied, 2);
}

#[test]
fn dry_farmland_reverts_only_on_its_roll_and_clear_overhead() {
    let (tick, pos) = (0..128)
        .find_map(|tick| {
            let pos = sampled_position(tick, 8, 1);
            (run_at(&[(pos, 35)], environment(tick, 1, 50), pos).0 == Some(3))
                .then_some((tick, pos))
        })
        .expect("source dry roll must hit in bounded search");
    assert_eq!(
        run_at(&[(pos, 35)], environment(tick, 1, 50), pos).0,
        Some(3)
    );
    let above = BlockPos::new(pos.x(), pos.y() + 1, pos.z());
    assert_eq!(
        run_at(&[(pos, 35), (above, 37)], environment(tick, 1, 50), pos).0,
        Some(35)
    );
}

#[test]
fn grass_spreads_from_ready_neighbor_and_skips_roof() {
    let (tick, pos) = (0..256)
        .find_map(|tick| {
            let pos = sampled_position(tick, 8, 1);
            if !(1..15).contains(&pos.x()) || !(1..15).contains(&pos.z()) {
                return None;
            }
            let neighbor = BlockPos::new(pos.x() + 1, pos.y(), pos.z());
            (run_at(&[(pos, 3), (neighbor, 4)], environment(tick, 1, 50), pos).0 == Some(4))
                .then_some((tick, pos))
        })
        .expect("source grass roll must hit in bounded search");
    let east = BlockPos::new(pos.x() + 1, pos.y(), pos.z());
    let west = BlockPos::new(pos.x() - 1, pos.y(), pos.z());
    let roof = BlockPos::new(pos.x(), pos.y() + 1, pos.z());
    assert_eq!(
        run_at(
            &[(pos, 3), (east, 4), (west, 4)],
            environment(tick, 1, 50),
            pos
        )
        .0,
        Some(4)
    );
    assert_eq!(
        run_at(
            &[(pos, 3), (east, 4), (roof, 2)],
            environment(tick, 1, 50),
            pos
        )
        .0,
        Some(3)
    );
}

#[test]
fn grass_ignores_unready_first_neighbor() {
    let (tick, pos) = (0..4096)
        .find_map(|tick| {
            let pos = sampled_position(tick, 8, 1);
            if pos.x() != 15 || !(1..15).contains(&pos.z()) {
                return None;
            }
            let west = BlockPos::new(14, pos.y(), pos.z());
            (run_at(&[(pos, 3), (west, 4)], environment(tick, 1, 50), pos).0 == Some(4))
                .then_some((tick, pos))
        })
        .expect("grass roll should hit at an east boundary");
    let west = BlockPos::new(14, pos.y(), pos.z());
    assert_eq!(
        run_at(&[(pos, 3), (west, 4)], environment(tick, 1, 50), pos).0,
        Some(4)
    );
    assert_eq!(
        run_at(&[(pos, 3)], environment(tick, 1, 50), pos).0,
        Some(3)
    );
}

#[test]
fn zero_attempts_leave_sampled_world_unchanged() {
    let pos = sampled_position(1, 8, 1);
    let below = BlockPos::new(pos.x(), pos.y() - 1, pos.z());
    let (block, report) = run_at(&[(pos, 37), (below, 36)], environment(1, 0, 100), pos);
    assert_eq!(block, Some(37));
    assert_eq!(report.examined, 0);
    assert_eq!(report.applied, 0);
}

#[test]
fn same_fixture_seed_tick_and_active_set_repeat_full_snapshot() {
    let tick = 17;
    let crop = sampled_position(tick, 8, 1);
    let support = BlockPos::new(crop.x(), crop.y() - 1, crop.z());
    let fixtures = vec![(key(), vec![(crop, 37), (support, 36)])];
    let env = environment(tick, 1, 100);
    let first = snapshot_run(&fixtures, env.clone(), &[key()]);
    let second = snapshot_run(&fixtures, env, &[key()]);
    assert_eq!(first.1.applied, 1);
    assert_eq!(first.1, second.1);
    assert_eq!(first.0, second.0);
}

#[test]
fn two_ready_keys_permutation_and_duplicates_repeat_full_snapshot() {
    let tick = 31;
    let first_crop = sampled_at_key(tick, key(), 8, 1);
    let second_crop = sampled_at_key(tick, neighbor_key(), 8, 1);
    let first_support = BlockPos::new(first_crop.x(), first_crop.y() - 1, first_crop.z());
    let second_support = BlockPos::new(second_crop.x(), second_crop.y() - 1, second_crop.z());
    let fixtures = vec![
        (key(), vec![(first_crop, 37), (first_support, 36)]),
        (
            neighbor_key(),
            vec![(second_crop, 46), (second_support, 36)],
        ),
    ];
    let env = environment(tick, 1, 100);
    let sorted = snapshot_run(&fixtures, env.clone(), &[key(), neighbor_key()]);
    let permuted = snapshot_run(
        &fixtures,
        env,
        &[neighbor_key(), key(), neighbor_key(), key()],
    );
    assert_eq!(sorted.1.examined, 48);
    assert_eq!(sorted.1.applied, 2);
    assert_eq!(sorted.1, permuted.1);
    assert_eq!(sorted.0, permuted.0);
}

#[test]
fn malformed_batch_does_not_advance() {
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ctx.stage(RuleEffect::Environment(environment(1, 64, 100)))
        .unwrap();
    let report = random_blocks::run(
        &mut ctx,
        RuleCall {
            phase: RulePhase::Support,
            actor: None,
            command: None,
            internal: None,
        },
    )
    .unwrap();
    assert_eq!(report.examined, 0);
    assert_eq!(report.applied, 0);
}

#[test]
fn sapling_uses_native_tree_and_aborts_on_obstruction() {
    let (tick, pos) = (0..512)
        .find_map(|tick| {
            let pos = sampled_position(tick, 8, 1);
            if !(3..13).contains(&pos.x()) || !(3..13).contains(&pos.z()) {
                return None;
            }
            let below = BlockPos::new(pos.x(), pos.y() - 1, pos.z());
            let result = run_at(&[(pos, 89), (below, 3)], environment(tick, 1, 50), pos).0;
            (result == Some(17)).then_some((tick, pos))
        })
        .expect("source sapling roll must hit in bounded search");
    let below = BlockPos::new(pos.x(), pos.y() - 1, pos.z());
    let trunk = BlockPos::new(pos.x(), pos.y() + 1, pos.z());
    assert_eq!(
        run_at(
            &[(pos, 89), (below, 3), (trunk, 2)],
            environment(tick, 1, 50),
            pos
        )
        .0,
        Some(89)
    );
}

#[test]
fn sapling_root_height_bound_is_exclusive() {
    let (tick, root) = (0..4096)
        .find_map(|tick| {
            let pos = sampled_position(tick, 23, 1);
            if pos.y() != 311 || !(3..13).contains(&pos.x()) || !(3..13).contains(&pos.z()) {
                return None;
            }
            let below = BlockPos::new(pos.x(), 310, pos.z());
            (run_at(&[(pos, 89), (below, 3)], environment(tick, 1, 50), pos).0 == Some(17))
                .then_some((tick, pos))
        })
        .expect("root at the maximum allowed height should grow on a source roll");
    assert_eq!(
        run_at(
            &[(root, 89), (BlockPos::new(root.x(), 310, root.z()), 3)],
            environment(tick, 1, 50),
            root
        )
        .0,
        Some(17)
    );
    let (tick, too_high) = (0..512)
        .find_map(|tick| {
            let pos = sampled_position(tick, 23, 1);
            (pos.y() == 312 && (3..13).contains(&pos.x()) && (3..13).contains(&pos.z()))
                .then_some((tick, pos))
        })
        .expect("sample should reach the rejected root height");
    assert_eq!(
        run_at(
            &[
                (too_high, 89),
                (BlockPos::new(too_high.x(), 311, too_high.z()), 3)
            ],
            environment(tick, 1, 50),
            too_high
        )
        .0,
        Some(89)
    );
}

#[test]
fn tree_cross_chunk_requires_all_targets_ready() {
    let (tick, root) = (0..4096)
        .find_map(|tick| {
            let pos = sampled_position(tick, 8, 1);
            if pos.x() != 15 || !(3..13).contains(&pos.z()) {
                return None;
            }
            let below = BlockPos::new(pos.x(), pos.y() - 1, pos.z());
            (run_with_neighbor(&[(pos, 89), (below, 3)], environment(tick, 1, 50), pos).0
                == Some(17))
            .then_some((tick, pos))
        })
        .expect("source roll should grow a boundary tree with both chunks ready");
    let below = BlockPos::new(root.x(), root.y() - 1, root.z());
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ctx.preload_ready_chunk(
        ReadyChunk::try_new(key(), 1, 1, chunk(&[(root, 89), (below, 3)])).unwrap(),
    );
    ctx.stage(RuleEffect::Environment(environment(tick, 1, 50)))
        .unwrap();
    let before = ctx.snapshot_state(world());
    let report = random_blocks::advance(&mut ctx, &[key()]).unwrap();
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, root), Some(89));
    assert_eq!(report.applied, 0);
    assert_eq!(ctx.snapshot_state(world()), before);
}
