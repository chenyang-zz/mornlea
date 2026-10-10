//! A mixed-dimension rescan FIFO owns one quota with attributed consumption.

use super::*;
use mornlea_server::contracts::{Resource, ServerError, WorkKind};

fn budget(target: usize) -> TickBudget {
    TickBudget::try_new(4096, 512, target, 65536, 65536).unwrap()
}

// Sparse observations supply the cause; the actual scanner owns every queue output.
fn queue_sections(
    ctx: &mut TickContext<'_>,
    schedule: &mut FluidSchedule,
    rows: &[(Dimension, ChunkPos)],
) -> (BTreeSet<ChunkKey>, Vec<RescanWork>, Vec<BlockPos>) {
    let mut scope = BTreeSet::new();
    let mut work = Vec::new();
    let mut sources = Vec::new();
    for &(dimension, pos) in rows {
        let key = ChunkKey { dimension, pos };
        let source = BlockPos::new(pos.x() * 16 + 2, 4, pos.z() * 16 + 3);
        observe(ctx, dimension, source, SOURCE);
        observe(
            ctx,
            dimension,
            BlockPos::new(source.x(), source.y() - 1, source.z()),
            AIR,
        );
        let entry = RescanWork {
            key,
            section_y: 0,
            next_cell: 0,
        };
        schedule.enqueue_rescan(entry.clone());
        scope.insert(key);
        work.push(entry);
        sources.push(source);
    }
    (scope, work, sources)
}

fn report(examined: usize, applied: usize, carried: usize) -> PhaseReport {
    PhaseReport {
        examined,
        applied,
        carried,
        rejected: 0,
    }
}

#[test]
fn mixed_dimensions_share_section_overshoot() {
    let mut owner = authority();
    let mut ctx = TickContext::harness(&mut owner, budget(4097));
    stage_environment(&mut ctx);
    let mut schedule = FluidSchedule::new();
    let rows = [
        (Dimension::OVERWORLD, ChunkPos::new(0, 0)),
        (Dimension::DEPTHS, ChunkPos::new(0, 0)),
        (Dimension::OVERWORLD, ChunkPos::new(1, 0)),
    ];
    let (scope, work, sources) = queue_sections(&mut ctx, &mut schedule, &rows);
    assert_eq!(
        provider::rescan(&mut schedule, &mut ctx, &scope, 0, 5).unwrap(),
        report(2, 4, 1)
    );
    assert_eq!(schedule.rescan_queue(), &work[2..]);
    assert_eq!(ctx.spent_rescan(Dimension::OVERWORLD), 4096);
    assert_eq!(ctx.spent_rescan(Dimension::DEPTHS), 4096);
    for dimension in [Dimension::OVERWORLD, Dimension::DEPTHS] {
        assert_eq!(schedule.pending_fluid(dimension), 2);
    }
    for (i, &(dimension, _)) in rows.iter().take(2).enumerate() {
        let source = sources[i];
        assert_eq!(schedule.fluid_due(dimension, source), Some(5));
        assert_eq!(
            schedule.fluid_due(
                dimension,
                BlockPos::new(source.x(), source.y() - 1, source.z())
            ),
            Some(5)
        );
    }
    assert_eq!(schedule.fluid_due(Dimension::OVERWORLD, sources[2]), None);
}

fn assert_first_dimension_owns_target(first: Dimension, second: Dimension) {
    let mut owner = authority();
    let mut ctx = TickContext::harness(&mut owner, budget(4096));
    stage_environment(&mut ctx);
    let mut schedule = FluidSchedule::new();
    let rows = [(first, ChunkPos::new(0, 0)), (second, ChunkPos::new(0, 0))];
    let (scope, work, sources) = queue_sections(&mut ctx, &mut schedule, &rows);
    assert_eq!(
        provider::rescan(&mut schedule, &mut ctx, &scope, 0, 5).unwrap(),
        report(1, 2, 1)
    );
    assert_eq!(schedule.rescan_queue(), &work[1..]);
    assert_eq!(ctx.spent_rescan(first), 4096);
    assert_eq!(ctx.spent_rescan(second), 0);
    assert_eq!(schedule.pending_fluid(first), 2);
    assert_eq!(schedule.pending_fluid(second), 0);
    assert_eq!(schedule.fluid_due(first, sources[0]), Some(5));
    assert_eq!(schedule.fluid_due(second, sources[1]), None);
}

#[test]
fn exhausted_target_preserves_other_dimension() {
    assert_first_dimension_owns_target(Dimension::OVERWORLD, Dimension::DEPTHS);
}

#[test]
fn reverse_dimension_order_keeps_fifo() {
    assert_first_dimension_owns_target(Dimension::DEPTHS, Dimension::OVERWORLD);
}

#[test]
fn rejected_global_charge_keeps_attribution() {
    let mut owner = authority();
    let mut ctx = TickContext::harness(&mut owner, budget(4096));
    let refused = Err(ServerError::Capacity {
        resource: Resource::Commands,
        limit: 8191,
        observed: 4097,
    });
    assert_eq!(
        ctx.charge(WorkKind::RescanCells(Dimension::OVERWORLD), 4097),
        refused
    );
    assert_eq!(ctx.spent_rescan(Dimension::OVERWORLD), 0);
    assert_eq!(ctx.spent_rescan(Dimension::DEPTHS), 0);
    ctx.charge(WorkKind::RescanCells(Dimension::OVERWORLD), 4096)
        .unwrap();
    for _ in 0..2 {
        assert_eq!(
            ctx.charge(WorkKind::RescanCells(Dimension::DEPTHS), 1),
            refused
        );
        assert_eq!(ctx.spent_rescan(Dimension::OVERWORLD), 4096);
        assert_eq!(ctx.spent_rescan(Dimension::DEPTHS), 0);
    }
}

#[test]
fn zero_global_target_refuses_both_dimensions() {
    let mut owner = authority();
    let mut ctx = TickContext::harness(&mut owner, budget(0));
    for dimension in [Dimension::OVERWORLD, Dimension::DEPTHS] {
        assert_eq!(
            ctx.charge(WorkKind::RescanCells(dimension), 1),
            Err(ServerError::Capacity {
                resource: Resource::Commands,
                limit: 4095,
                observed: 1,
            })
        );
    }
    assert_eq!(ctx.spent_rescan(Dimension::OVERWORLD), 0);
    assert_eq!(ctx.spent_rescan(Dimension::DEPTHS), 0);
}

#[test]
fn update_quota_remains_per_dimension() {
    let mut owner = authority();
    let configured = TickBudget::try_new(4096, 1, 4096, 65536, 65536).unwrap();
    let mut ctx = TickContext::harness(&mut owner, configured);
    for dimension in [Dimension::OVERWORLD, Dimension::DEPTHS] {
        ctx.charge(WorkKind::FluidUpdates(dimension), 1).unwrap();
    }
    assert_eq!(ctx.spent_fluid(Dimension::OVERWORLD), 1);
    assert_eq!(ctx.spent_fluid(Dimension::DEPTHS), 1);
    assert_eq!(
        ctx.charge(WorkKind::FluidUpdates(Dimension::OVERWORLD), 1),
        Err(ServerError::Capacity {
            resource: Resource::Commands,
            limit: 1,
            observed: 2,
        })
    );
    assert_eq!(ctx.spent_fluid(Dimension::OVERWORLD), 1);
    assert_eq!(ctx.spent_fluid(Dimension::DEPTHS), 1);
    assert_eq!(TickBudget::full().fluid_updates_per_dimension(), 512);
    assert_eq!(
        TickBudget::full().fluid_rescan_target_per_dimension(),
        65536
    );
}
