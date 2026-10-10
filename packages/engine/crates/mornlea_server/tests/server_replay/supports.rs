//! Support sweep replay against the ordered Go support invalidators.

use mornlea_domain::{BlockPos, ChunkPos, Dimension, Weather};
use mornlea_server::contracts::{
    BlockObservation, BlockWrite, ChunkKey, EnvironmentState, RuleCall, RuleEffect, RulePhase,
    RuleTunables, ServerLimits, SystemRule, TickBudget,
};
use mornlea_server::rules::supports;
use mornlea_server::state::{AuthorityState, TickContext};

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        0,
    )
    .unwrap()
}
fn environment() -> EnvironmentState {
    EnvironmentState {
        seed: 7,
        next_tick: 1,
        world_time: 0,
        day_phase_offset: 0,
        season_offset: 0,
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty: 0,
        tunables: RuleTunables::source_defaults(),
    }
}
fn call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::Support,
        actor: None,
        command: None,
        internal: None,
    }
}
fn observe(dim: Dimension, pos: BlockPos, block: u16) -> BlockObservation {
    BlockObservation::try_new(
        ChunkKey {
            dimension: dim,
            pos: ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
        },
        1,
        1,
        pos,
        block,
    )
    .unwrap()
}
fn write(ctx: &mut TickContext<'_>, dim: Dimension, pos: BlockPos, block: u16) {
    let old = ctx.read().observation(dim, pos).unwrap();
    ctx.transaction()
        .try_system(
            SystemRule::Support,
            vec![BlockWrite::try_new(old, block).unwrap()],
        )
        .unwrap();
}
fn seeded<'a>(
    authority: &'a mut AuthorityState,
    dim: Dimension,
    support: BlockPos,
    plant: BlockPos,
    plant_block: u16,
) -> TickContext<'a> {
    let mut ctx = TickContext::harness(authority, TickBudget::full());
    ctx.preload_block(observe(dim, support, 4));
    ctx.preload_block(observe(dim, plant, plant_block));
    ctx.stage(RuleEffect::Environment(environment())).unwrap();
    ctx
}

#[test]
fn changed_support_removes_short_grass_without_drop() {
    let mut state = authority();
    let support = BlockPos::new(1, 64, 1);
    let above = BlockPos::new(1, 65, 1);
    let mut ctx = seeded(&mut state, Dimension::OVERWORLD, support, above, 84);
    assert_eq!(supports::run(&mut ctx, call()).unwrap().applied, 0);
    write(&mut ctx, Dimension::OVERWORLD, support, 0);
    let report = supports::run(&mut ctx, call()).unwrap();
    assert_eq!(report.examined, 7);
    assert_eq!(report.applied, 1);
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, above), Some(0));
    assert!(
        ctx.read()
            .drops(observe(Dimension::OVERWORLD, above, 84).key)
            .is_empty()
    );
}

#[test]
fn changed_support_removes_sapling_with_one_drop() {
    let mut state = authority();
    let support = BlockPos::new(1, 64, 1);
    let above = BlockPos::new(1, 65, 1);
    let mut ctx = seeded(&mut state, Dimension::OVERWORLD, support, above, 89);
    write(&mut ctx, Dimension::OVERWORLD, support, 0);
    let report = supports::run(&mut ctx, call()).unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, above), Some(0));
    let view = ctx.read();
    let drops = view.drops(observe(Dimension::OVERWORLD, above, 89).key);
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].stack.item, 57);
}

#[test]
fn changed_support_removes_standing_torch_with_one_drop() {
    let mut state = authority();
    let support = BlockPos::new(1, 64, 1);
    let above = BlockPos::new(1, 65, 1);
    let mut ctx = seeded(&mut state, Dimension::OVERWORLD, support, above, 71);
    write(&mut ctx, Dimension::OVERWORLD, support, 0);
    let report = supports::run(&mut ctx, call()).unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, above), Some(0));
    assert_eq!(
        ctx.read()
            .drops(observe(Dimension::OVERWORLD, above, 71).key)[0]
            .stack
            .item,
        44
    );
}

#[test]
fn changed_support_removes_bed_pair_with_one_drop() {
    let mut state = authority();
    let support = BlockPos::new(1, 64, 1);
    let foot = BlockPos::new(1, 65, 1);
    let head = BlockPos::new(1, 65, 2);
    let mut ctx = seeded(&mut state, Dimension::OVERWORLD, support, foot, 76);
    ctx.preload_block(observe(Dimension::OVERWORLD, head, 80));
    write(&mut ctx, Dimension::OVERWORLD, support, 0);
    let report = supports::run(&mut ctx, call()).unwrap();
    assert_eq!(report.applied, 2);
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, foot), Some(0));
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, head), Some(0));
    assert_eq!(
        ctx.read()
            .drops(observe(Dimension::OVERWORLD, foot, 76).key)[0]
            .stack
            .item,
        46
    );
}

fn ready_chunk(
    cells: &[(BlockPos, u16)],
    drops: Vec<mornlea_storage::DropSlot>,
) -> mornlea_storage::Chunk {
    use mornlea_storage::{ContainerSnapshot, StorageKind};
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
    mornlea_storage::Chunk {
        sections,
        drops,
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    }
}

fn ready(
    ctx: &mut TickContext<'_>,
    dim: Dimension,
    pos: BlockPos,
    cells: &[(BlockPos, u16)],
    drops: Vec<mornlea_storage::DropSlot>,
    revision: u64,
) {
    use mornlea_server::core::world::ReadyChunk;
    let key = observe(dim, pos, 0).key;
    ctx.preload_ready_chunk(
        ReadyChunk::try_new(key, 1, revision, ready_chunk(cells, drops)).unwrap(),
    );
}

fn slots() -> Vec<mornlea_storage::DropSlot> {
    vec![Default::default(); 32]
}

#[test]
fn changed_ledger_dedupes_sorts_and_restores_final_values() {
    use mornlea_domain::chunk_block_index;
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    let a = BlockPos::new(15, 64, 1);
    let b = BlockPos::new(1, 65, 1);
    let c = BlockPos::new(16, 64, 1);
    for pos in [c, b, a] {
        ctx.preload_block(observe(Dimension::OVERWORLD, pos, 4));
    }
    assert!(ctx.changed_blocks().is_empty());
    write(&mut ctx, Dimension::OVERWORLD, c, 0);
    write(&mut ctx, Dimension::OVERWORLD, b, 0);
    write(&mut ctx, Dimension::OVERWORLD, a, 0);
    write(&mut ctx, Dimension::OVERWORLD, a, 4);
    assert_eq!(ctx.changed_blocks().len(), 3);
    let ordered = ctx.changed_blocks();
    assert_eq!(
        ordered.iter().map(|o| o.pos).collect::<Vec<_>>(),
        vec![a, b, c]
    );
    assert!(chunk_block_index(a) < chunk_block_index(b));
    assert_eq!(ordered[0].block, 4);
    let stale = BlockWrite::try_new(observe(Dimension::OVERWORLD, a, 4), 0).unwrap();
    assert!(
        ctx.transaction()
            .try_system(SystemRule::Support, vec![stale])
            .is_err()
    );
    assert_eq!(ctx.changed_blocks(), ordered);
    let now = ctx.read().observation(Dimension::OVERWORLD, a).unwrap();
    ctx.transaction()
        .try_system(
            SystemRule::Support,
            vec![BlockWrite::try_new(now, now.block).unwrap()],
        )
        .unwrap();
    assert_eq!(ctx.changed_blocks(), ordered);
}

#[test]
fn restored_support_holds_all_four_targets() {
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    for (x, block) in [(1, 84), (3, 89), (5, 71), (7, 76)] {
        let support = BlockPos::new(x, 64, 1);
        let target = BlockPos::new(x, 65, 1);
        ctx.preload_block(observe(Dimension::OVERWORLD, support, 4));
        ctx.preload_block(observe(Dimension::OVERWORLD, target, block));
        if block == 76 {
            ctx.preload_block(observe(Dimension::OVERWORLD, BlockPos::new(x, 65, 2), 80));
        }
        write(&mut ctx, Dimension::OVERWORLD, support, 0);
        write(&mut ctx, Dimension::OVERWORLD, support, 4);
    }
    ctx.stage(RuleEffect::Environment(environment())).unwrap();
    let report = supports::run(&mut ctx, call()).unwrap();
    assert_eq!(report.applied, 0);
    assert_eq!(report.examined, 16);
    for (x, block) in [(1, 84), (3, 89), (5, 71), (7, 76)] {
        assert_eq!(
            ctx.read()
                .block(Dimension::OVERWORLD, BlockPos::new(x, 65, 1)),
            Some(block)
        );
    }
}

#[test]
fn later_pass_observes_removed_grass_but_grass_pass_does_not_recurse() {
    let mut state = authority();
    let support = BlockPos::new(1, 64, 1);
    let grass = BlockPos::new(1, 65, 1);
    let torch = BlockPos::new(1, 66, 1);
    let mut ctx = seeded(&mut state, Dimension::OVERWORLD, support, grass, 84);
    ctx.preload_block(observe(Dimension::OVERWORLD, torch, 71));
    write(&mut ctx, Dimension::OVERWORLD, support, 0);
    let report = supports::run(&mut ctx, call()).unwrap();
    assert_eq!(report.applied, 2);
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, grass), Some(0));
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, torch), Some(0));
}

#[test]
fn torch_orientations_and_glass_support_policy() {
    let mut state = authority();
    let center = BlockPos::new(5, 64, 5);
    let targets = [
        (BlockPos::new(5, 65, 5), 71),
        (BlockPos::new(6, 64, 5), 72),
        (BlockPos::new(4, 64, 5), 73),
        (BlockPos::new(5, 64, 6), 74),
        (BlockPos::new(5, 64, 4), 75),
    ];
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ctx.preload_block(observe(Dimension::OVERWORLD, center, 4));
    for (pos, block) in targets {
        ctx.preload_block(observe(Dimension::OVERWORLD, pos, block));
    }
    ctx.stage(RuleEffect::Environment(environment())).unwrap();
    write(&mut ctx, Dimension::OVERWORLD, center, 20);
    assert_eq!(supports::run(&mut ctx, call()).unwrap().applied, 0);
    write(&mut ctx, Dimension::OVERWORLD, center, 0);
    assert_eq!(supports::run(&mut ctx, call()).unwrap().applied, 5);
    for (pos, _) in targets {
        assert_eq!(ctx.read().block(Dimension::OVERWORLD, pos), Some(0));
    }
}

#[test]
fn full_slots_clear_grass_but_preserve_drop_bearing_targets() {
    use mornlea_storage::{DropSlot, ItemStack};
    for (block, item) in [(84, None), (89, Some(57)), (71, Some(44)), (76, Some(46))] {
        let mut state = authority();
        let support = BlockPos::new(1, 64, 1);
        let target = BlockPos::new(1, 65, 1);
        let mut occupied = slots();
        occupied.fill(DropSlot {
            generation: 1,
            active: true,
            stack: ItemStack {
                item: 5,
                count: 64,
                durability: 0,
            },
            block_index: 0,
            age_ticks: 0,
            pickup_delay_ticks: 0,
        });
        let mut cells = vec![(support, 4), (target, block)];
        if block == 76 {
            cells.push((BlockPos::new(1, 65, 2), 80));
        }
        let mut ctx = TickContext::harness(&mut state, TickBudget::full());
        ready(&mut ctx, Dimension::OVERWORLD, support, &cells, occupied, 1);
        ctx.stage(RuleEffect::Environment(environment())).unwrap();
        write(&mut ctx, Dimension::OVERWORLD, support, 0);
        let report = supports::run(&mut ctx, call()).unwrap();
        assert_eq!(
            ctx.read().block(Dimension::OVERWORLD, target),
            Some(if item.is_some() { block } else { 0 })
        );
        assert_eq!(report.rejected, usize::from(item.is_some()));
    }
}

fn world() -> mornlea_domain::WorldState {
    mornlea_domain::WorldState::try_new(mornlea_domain::WorldStateParts {
        day_phase_offset: 0,
        world_time_ticks: 0,
        weather: Weather::Clear,
        season: mornlea_domain::Season::Spring,
        season_progress: 0,
        temperature: 0,
    })
    .unwrap()
}

#[test]
fn ready_reload_clears_changed_ledger_and_wrong_calls_leave_state() {
    let mut state = authority();
    let pos = BlockPos::new(1, 64, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ready(&mut ctx, Dimension::OVERWORLD, pos, &[(pos, 4)], slots(), 1);
    write(&mut ctx, Dimension::OVERWORLD, pos, 0);
    assert_eq!(ctx.changed_blocks().len(), 1);
    ready(&mut ctx, Dimension::OVERWORLD, pos, &[(pos, 4)], slots(), 2);
    assert!(ctx.changed_blocks().is_empty());
    assert!(matches!(
        supports::run(&mut ctx, call()),
        Err(mornlea_server::contracts::ServerError::InvalidInput {
            field: "environment"
        })
    ));
    ctx.stage(RuleEffect::Environment(environment())).unwrap();
    let wrong = RuleCall {
        phase: RulePhase::RandomBlock,
        ..call()
    };
    assert!(matches!(
        supports::run(&mut ctx, wrong),
        Err(mornlea_server::contracts::ServerError::InvalidInput { field: "support" })
    ));
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, pos), Some(4));
    assert_eq!(supports::run(&mut ctx, call()).unwrap().examined, 0);
}

#[test]
fn partial_slot_merges_with_delay_and_f1_reload() {
    use mornlea_domain::chunk_block_index;
    use mornlea_storage::{ChunkSave, DropSlot, ItemStack};
    let mut state = authority();
    let support = BlockPos::new(1, 64, 1);
    let sapling = BlockPos::new(1, 65, 1);
    let mut occupied = slots();
    occupied.fill(DropSlot {
        generation: 1,
        active: true,
        stack: ItemStack {
            item: 5,
            count: 64,
            durability: 0,
        },
        block_index: 0,
        age_ticks: 0,
        pickup_delay_ticks: 0,
    });
    occupied[5] = DropSlot {
        generation: 7,
        active: true,
        stack: ItemStack {
            item: 57,
            count: 63,
            durability: 0,
        },
        block_index: chunk_block_index(sapling),
        age_ticks: 8,
        pickup_delay_ticks: 0,
    };
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ready(
        &mut ctx,
        Dimension::OVERWORLD,
        support,
        &[(support, 4), (sapling, 89)],
        occupied,
        1,
    );
    let env = environment();
    let delay = env.tunables.drop_pickup_delay_ticks();
    ctx.stage(RuleEffect::Environment(env)).unwrap();
    write(&mut ctx, Dimension::OVERWORLD, support, 0);
    assert_eq!(supports::run(&mut ctx, call()).unwrap().applied, 1);
    let snapshot = ctx.snapshot_state(world());
    let (key, _, revision, chunk) = &snapshot.chunks[0];
    assert_eq!(chunk.drops[5].stack.count, 64);
    assert_eq!(chunk.drops[5].generation, 7);
    assert_eq!(chunk.drops[5].pickup_delay_ticks, delay);
    assert_eq!(ctx.read().drops(*key).len(), 32);
    let save = ChunkSave {
        key: mornlea_storage::ChunkKey {
            dimension: i32::from(key.dimension.get()),
            x: key.pos.x(),
            z: key.pos.z(),
        },
        revision: *revision,
        chunk: chunk.clone(),
    };
    let bytes = mornlea_storage::encode_chunk(&save).unwrap();
    let decoded = mornlea_storage::decode_chunk(save.key, save.revision, &bytes).unwrap();
    assert_eq!(decoded.chunk.drops[5], chunk.drops[5]);
    let mut restored_state = authority();
    let mut restored = TickContext::harness(&mut restored_state, TickBudget::full());
    restored.preload_ready_chunk(
        mornlea_server::core::world::ReadyChunk::try_new(*key, 1, *revision, decoded.chunk)
            .unwrap(),
    );
    assert_eq!(
        restored.read().block(Dimension::OVERWORLD, sapling),
        Some(0)
    );
    assert_eq!(restored.read().drops(*key)[5].stack.count, 64);
    assert_eq!(restored.read().drops(*key)[5].pickup_delay, delay);
}

#[test]
fn cross_chunk_bed_requires_counterpart_then_clears_both_once() {
    let mut state = authority();
    let support = BlockPos::new(15, 64, 1);
    let foot = BlockPos::new(15, 65, 1);
    let head = BlockPos::new(16, 65, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ready(
        &mut ctx,
        Dimension::OVERWORLD,
        support,
        &[(support, 4), (foot, 79)],
        slots(),
        1,
    );
    ctx.stage(RuleEffect::Environment(environment())).unwrap();
    write(&mut ctx, Dimension::OVERWORLD, support, 0);
    assert_eq!(supports::run(&mut ctx, call()).unwrap().applied, 0);
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, foot), Some(79));
    ready(
        &mut ctx,
        Dimension::OVERWORLD,
        head,
        &[(head, 83)],
        slots(),
        1,
    );
    let report = supports::run(&mut ctx, call()).unwrap();
    assert_eq!(report.applied, 2);
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, foot), Some(0));
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, head), Some(0));
    assert_eq!(
        ctx.read()
            .drops(observe(Dimension::OVERWORLD, foot, 79).key)
            .len(),
        1
    );
    assert_eq!(supports::run(&mut ctx, call()).unwrap().applied, 0);
}

#[test]
fn cross_chunk_bed_max_revision_refuses_atomically() {
    let mut state = authority();
    let support = BlockPos::new(15, 64, 1);
    let foot = BlockPos::new(15, 65, 1);
    let head = BlockPos::new(16, 65, 1);
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ready(
        &mut ctx,
        Dimension::OVERWORLD,
        support,
        &[(support, 4), (foot, 79)],
        slots(),
        1,
    );
    ready(
        &mut ctx,
        Dimension::OVERWORLD,
        head,
        &[(head, 83)],
        slots(),
        u64::MAX,
    );
    ctx.stage(RuleEffect::Environment(environment())).unwrap();
    write(&mut ctx, Dimension::OVERWORLD, support, 0);
    let report = supports::run(&mut ctx, call()).unwrap();
    assert_eq!(report.rejected, 1);
    assert_eq!(report.applied, 0);
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, foot), Some(79));
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, head), Some(83));
    assert!(
        ctx.read()
            .drops(observe(Dimension::OVERWORLD, foot, 79).key)
            .is_empty()
    );
}

#[test]
fn depths_support_removal_is_dimension_scoped() {
    let mut state = authority();
    let support = BlockPos::new(1, 64, 1);
    let target = BlockPos::new(1, 65, 1);
    let mut ctx = seeded(&mut state, Dimension::DEPTHS, support, target, 89);
    ctx.preload_block(observe(Dimension::OVERWORLD, target, 89));
    write(&mut ctx, Dimension::DEPTHS, support, 0);
    assert_eq!(supports::run(&mut ctx, call()).unwrap().applied, 1);
    assert_eq!(ctx.read().block(Dimension::DEPTHS, target), Some(0));
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, target), Some(89));
    assert_eq!(
        ctx.read().drops(observe(Dimension::DEPTHS, target, 89).key)[0]
            .stack
            .item,
        57
    );
}

#[test]
fn grass_pass_does_not_recursively_clear_newly_exposed_grass() {
    let mut state = authority();
    let support = BlockPos::new(1, 64, 1);
    let first = BlockPos::new(1, 65, 1);
    let second = BlockPos::new(1, 66, 1);
    let mut ctx = seeded(&mut state, Dimension::OVERWORLD, support, first, 84);
    ctx.preload_block(observe(Dimension::OVERWORLD, second, 84));
    write(&mut ctx, Dimension::OVERWORLD, support, 0);
    let report = supports::run(&mut ctx, call()).unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, first), Some(0));
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, second), Some(84));
}

#[test]
fn bed_rejects_glass_support_that_keeps_torches() {
    let mut state = authority();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    let glass = BlockPos::new(1, 64, 1);
    let bed = BlockPos::new(1, 65, 1);
    let head = BlockPos::new(1, 65, 2);
    let torch_support = BlockPos::new(5, 64, 1);
    let torch = BlockPos::new(5, 65, 1);
    for (pos, block) in [
        (glass, 4),
        (bed, 76),
        (head, 80),
        (torch_support, 4),
        (torch, 71),
    ] {
        ctx.preload_block(observe(Dimension::OVERWORLD, pos, block));
    }
    ctx.stage(RuleEffect::Environment(environment())).unwrap();
    write(&mut ctx, Dimension::OVERWORLD, glass, 20);
    write(&mut ctx, Dimension::OVERWORLD, torch_support, 20);
    assert_eq!(supports::run(&mut ctx, call()).unwrap().applied, 2);
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, bed), Some(0));
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, head), Some(0));
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, torch), Some(71));
}

#[test]
fn loaded_air_counterpart_is_cleared_without_pair_form_check() {
    let mut state = authority();
    let support = BlockPos::new(1, 64, 1);
    let foot = BlockPos::new(1, 65, 1);
    let head = BlockPos::new(1, 65, 2);
    let mut ctx = seeded(&mut state, Dimension::OVERWORLD, support, foot, 76);
    ctx.preload_block(observe(Dimension::OVERWORLD, head, 0));
    write(&mut ctx, Dimension::OVERWORLD, support, 0);
    let report = supports::run(&mut ctx, call()).unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(report.rejected, 0);
    assert_eq!(ctx.read().block(Dimension::OVERWORLD, foot), Some(0));
    assert_eq!(
        ctx.read()
            .drops(observe(Dimension::OVERWORLD, foot, 76).key)
            .len(),
        1
    );
}
