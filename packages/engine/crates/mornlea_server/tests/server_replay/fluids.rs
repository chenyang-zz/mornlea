//! Fluid rescan and update scheduling replay.
//!
//! Every gate below mirrors a frozen Go oracle row, cited at each case:
//!
//! - Due order `(due, chunkX, chunkZ, y, z, x)` with earliest-due dedup, the
//!   tick-start snapshot, strongest-level merge of overlapping outputs, sorted
//!   commit, and requeue of each changed cell plus its six neighbors at
//!   `now + delay` from `packages/server/fluid/queue.go` (`Queue.Advance`,
//!   `lessPos`, `strongerWrite`, `sixNeighbors`, `Enqueue`).
//! - The per-dimension update budget and the rescan target gating whole
//!   sections from `packages/server/sim/realm/environment.go`
//!   (`AdvanceFluids`, `runFluidRescans`, `rescanChunkFluids`): rescan phases
//!   run before update phases, out-of-scope pending rescans drop their cursor
//!   while carried work resumes next tick, and a section scan charges whole.
//! - Single-cell rule evaluation through the accepted engine kernel, whose
//!   table mirrors `packages/server/fluid/rules.go` (`evalCell`,
//!   `flowingSurvives`, `Replaceable`).
//! - The conflicting-writers terrain (two strengths racing for one cell with
//!   the stronger winning regardless of enqueue order) mirrors
//!   `TestAdvance_ConflictingWritesResolveToStrongest` in
//!   `packages/server/fluid/advance_test.go`.
//!
//! No case chooses a value the oracles do not pin. Refusals and carries compare
//! queue leftovers and world cells, not a bare `is_err`.

use std::collections::BTreeSet;

use mornlea_domain::{BlockPos, ChunkPos, Dimension};
use mornlea_server::contracts::{
    BlockObservation, ChunkKey, PhaseReport, RescanWork, RuleCall, RulePhase, ServerLimits,
    TickBudget,
};
use mornlea_server::rules::fluids::{self as provider, FluidSchedule};
use mornlea_server::state::{AuthorityState, TickContext};

// Stable block numbers, mirrored from the frozen const block in
// `packages/shared/core/block.go`.
const AIR: u16 = 0; // `core.AirID`
const STONE: u16 = 2; // `core.StoneID`
const SOURCE: u16 = 27; // `core.WaterSourceID`
const LEVEL_1: u16 = 28; // `core.WaterLevel1ID`
const LEVEL_2: u16 = 29; // `core.WaterLevel2ID`

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        7,
    )
    .expect("authority")
}

fn chunk_key(dimension: Dimension, pos: BlockPos) -> ChunkKey {
    ChunkKey {
        dimension,
        pos: ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
    }
}

fn observe(ctx: &mut TickContext<'_>, dimension: Dimension, pos: BlockPos, block: u16) {
    let observed = BlockObservation::try_new(chunk_key(dimension, pos), 1, 1, pos, block)
        .expect("block observation");
    ctx.preload_block(observed);
}

fn zero_report() -> PhaseReport {
    PhaseReport {
        examined: 0,
        applied: 0,
        carried: 0,
        rejected: 0,
    }
}

/// Budget and carry across two sorted dimensions: 513 due items per dimension
/// against the frozen 512-updates-per-dimension ceiling leaves exactly one
/// carry per dimension, and every changed cell requeues with its six neighbors
/// at `now + delay` without overlap loss.
#[test]
fn two_dimensions_513() {
    let mut authority = authority();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    // The batch entry owns no queue port, so even a well-shaped fluid call
    // reports zero without effect; a foreign phase refuses the same way.
    for phase in [RulePhase::FluidRescan, RulePhase::FluidUpdate] {
        let report = provider::run(
            &mut ctx,
            RuleCall {
                phase,
                actor: None,
                command: None,
                internal: None,
            },
        )
        .expect("fluid batch entry");
        assert_eq!(report, zero_report());
    }
    let refused = provider::run(
        &mut ctx,
        RuleCall {
            phase: RulePhase::Interaction,
            actor: None,
            command: None,
            internal: None,
        },
    )
    .expect("foreign phase refusal");
    assert_eq!(refused, zero_report());

    let mut schedule = FluidSchedule::new();
    for dimension in [Dimension::OVERWORLD, Dimension::DEPTHS] {
        for index in 0..513 {
            // Eight cells apart so no two requeue neighborhoods overlap; due
            // order then follows the index and the last item carries.
            let pos = BlockPos::new(index * 8, 10, 0);
            schedule.enqueue_fluid(chunk_key(dimension, pos), pos, 0);
            // A lone source over air with sealed sides spreads straight down,
            // mirroring the vertical-priority oracle row.
            observe(&mut ctx, dimension, pos, SOURCE);
            observe(&mut ctx, dimension, BlockPos::new(pos.x(), 9, 0), AIR);
        }
    }

    let report = provider::update(&mut schedule, &mut ctx, 0, 5).expect("update");
    assert_eq!(
        report,
        PhaseReport {
            examined: 1024,
            applied: 1024,
            carried: 2,
            rejected: 0,
        }
    );
    for dimension in [Dimension::OVERWORLD, Dimension::DEPTHS] {
        // One due-zero carry plus seven requeues (changed cell plus six
        // neighbors) for each of the 512 processed items.
        assert_eq!(schedule.pending_fluid(dimension), 1 + 512 * 7);
        // Due order is `(due, chunkX, chunkZ, y, z, x)`, so the item with the
        // greatest x is the one left behind in each dimension.
        let carried = BlockPos::new(512 * 8, 10, 0);
        assert_eq!(schedule.fluid_due(dimension, carried), Some(0));
        assert_eq!(
            ctx.read().block(dimension, BlockPos::new(512 * 8, 9, 0)),
            Some(AIR)
        );
        assert_eq!(ctx.read().block(dimension, carried), Some(SOURCE));
        for index in 0..512 {
            let pos = BlockPos::new(index * 8, 10, 0);
            assert_eq!(ctx.read().block(dimension, pos), Some(SOURCE));
            assert_eq!(
                ctx.read().block(dimension, BlockPos::new(pos.x(), 9, 0)),
                Some(LEVEL_1)
            );
        }
    }
}

/// Section-aware rescan accounting: a mixed section scans whole at a cost of
/// 4096 cells, a small target overshoots by at most one section, rescans run
/// before updates, and a scope exit drops the cursor while carried sections
/// resume next tick.
#[test]
fn rescan_section_overshoot() {
    let mut authority = authority();
    let budget = TickBudget::try_new(4096, 512, 4097, 65536, 65536).expect("budget");
    let mut ctx = TickContext::harness(&mut authority, budget);
    let mut schedule = FluidSchedule::new();
    // Three in-scope sections with one observed source each, plus a pending
    // section whose chunk leaves the active scope before the scan.
    let chunks = [
        ChunkPos::new(0, 0),
        ChunkPos::new(1, 0),
        ChunkPos::new(2, 0),
        ChunkPos::new(3, 0),
    ];
    let sources = [
        BlockPos::new(2, 4, 3),
        BlockPos::new(18, 5, 6),
        BlockPos::new(34, 2, 1),
    ];
    for (chunk, source) in chunks.iter().zip(sources.iter()).take(3) {
        schedule.enqueue_rescan(RescanWork {
            key: ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: *chunk,
            },
            section_y: 0,
            next_cell: 0,
        });
        observe(&mut ctx, Dimension::OVERWORLD, *source, SOURCE);
        observe(
            &mut ctx,
            Dimension::OVERWORLD,
            BlockPos::new(source.x(), source.y() - 1, source.z()),
            AIR,
        );
    }
    schedule.enqueue_rescan(RescanWork {
        key: ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: chunks[3],
        },
        section_y: 0,
        next_cell: 0,
    });
    let scope: BTreeSet<ChunkKey> = chunks
        .iter()
        .take(3)
        .map(|chunk| ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: *chunk,
        })
        .collect();

    // Target 4097 authorizes two whole 4096-cell sections (spent reaches
    // target + 4095) and refuses the third, which carries with its cursor.
    let report = provider::rescan(&mut schedule, &mut ctx, &scope, 0, 5).expect("rescan");
    assert_eq!(
        report,
        PhaseReport {
            examined: 2,
            applied: 4,
            carried: 1,
            rejected: 0,
        }
    );
    // The out-of-scope section dropped its cursor; the refused section stays
    // queued from its unmoved cursor, never lost and never double-processed.
    assert_eq!(schedule.pending_rescans(), 1);
    assert_eq!(
        schedule.rescan_queue(),
        &[RescanWork {
            key: ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: chunks[2],
            },
            section_y: 0,
            next_cell: 0,
        }]
    );
    // Each mixed section enqueued its source plus the observed air below at
    // `now + delay`; nothing is due yet, so the update phase that follows the
    // rescan phase finds no work and resamples nothing.
    assert_eq!(schedule.pending_fluid(Dimension::OVERWORLD), 4);
    for source in sources.iter().take(2) {
        assert_eq!(
            schedule.fluid_due(
                Dimension::OVERWORLD,
                BlockPos::new(source.x(), source.y() - 1, source.z())
            ),
            Some(5)
        );
    }
    let update = provider::update(&mut schedule, &mut ctx, 0, 5).expect("update");
    assert_eq!(update, zero_report());
    assert_eq!(schedule.pending_fluid(Dimension::OVERWORLD), 4);
}

/// Snapshot merge: stale items skip at a uniform cost of one update unit each,
/// overlapping outputs merge to the strongest level, and the commit lands in
/// sorted order regardless of enqueue order.
#[test]
fn strongest_snapshot_merge() {
    let layout = |schedule: &mut FluidSchedule, ctx: &mut TickContext<'_>, order: &[BlockPos]| {
        for pos in order {
            schedule.enqueue_fluid(chunk_key(Dimension::OVERWORLD, *pos), *pos, 0);
        }
        // The conflicting-writers terrain: a source and a level-2 flow race
        // for the air between them over a stone floor.
        observe(ctx, Dimension::OVERWORLD, BlockPos::new(0, 10, 0), SOURCE);
        observe(ctx, Dimension::OVERWORLD, BlockPos::new(1, 10, 0), AIR);
        observe(ctx, Dimension::OVERWORLD, BlockPos::new(2, 10, 0), LEVEL_2);
        observe(ctx, Dimension::OVERWORLD, BlockPos::new(3, 10, 0), LEVEL_1);
        observe(ctx, Dimension::OVERWORLD, BlockPos::new(4, 10, 0), SOURCE);
        // A second independent source spreading into open air pins the sorted
        // commit across more than one write.
        observe(ctx, Dimension::OVERWORLD, BlockPos::new(10, 10, 0), SOURCE);
        observe(ctx, Dimension::OVERWORLD, BlockPos::new(11, 10, 0), AIR);
        // Stale queue items: already air, skipped at unit cost.
        observe(ctx, Dimension::OVERWORLD, BlockPos::new(100, 10, 0), AIR);
        observe(ctx, Dimension::OVERWORLD, BlockPos::new(101, 10, 0), AIR);
        for x in [0, 1, 2, 3, 4, 10, 11] {
            observe(ctx, Dimension::OVERWORLD, BlockPos::new(x, 9, 0), STONE);
        }
    };
    let queued = [
        BlockPos::new(0, 10, 0),
        BlockPos::new(2, 10, 0),
        BlockPos::new(3, 10, 0),
        BlockPos::new(4, 10, 0),
        BlockPos::new(10, 10, 0),
        BlockPos::new(100, 10, 0),
        BlockPos::new(101, 10, 0),
    ];

    let mut authority_main = authority();
    let mut ctx = TickContext::harness(&mut authority_main, TickBudget::full());
    let mut schedule = FluidSchedule::new();
    layout(&mut schedule, &mut ctx, &queued);
    let report = provider::update(&mut schedule, &mut ctx, 0, 5).expect("update");
    // Seven selected items (five fluid, two uniform stale skips) cost one unit
    // each; two cells actually change.
    assert_eq!(
        report,
        PhaseReport {
            examined: 7,
            applied: 2,
            carried: 0,
            rejected: 0,
        }
    );
    // The level-1 spread from the source beats the level-3 spread from the
    // level-2 flow; the level-2 flow itself survives on the tick-start
    // snapshot where its stronger level-1 neighbor is still present.
    assert_eq!(
        ctx.read()
            .block(Dimension::OVERWORLD, BlockPos::new(1, 10, 0)),
        Some(LEVEL_1)
    );
    assert_eq!(
        ctx.read()
            .block(Dimension::OVERWORLD, BlockPos::new(11, 10, 0)),
        Some(LEVEL_1)
    );
    assert_eq!(
        ctx.read()
            .block(Dimension::OVERWORLD, BlockPos::new(2, 10, 0)),
        Some(LEVEL_2)
    );
    assert_eq!(
        ctx.read()
            .block(Dimension::OVERWORLD, BlockPos::new(0, 10, 0)),
        Some(SOURCE)
    );

    // Reversed enqueue order reaches the identical state: the merge does not
    // depend on processing order.
    let mut authority_reversed = authority();
    let mut ctx_reversed = TickContext::harness(&mut authority_reversed, TickBudget::full());
    let mut schedule_reversed = FluidSchedule::new();
    let mut reversed = queued;
    reversed.reverse();
    layout(&mut schedule_reversed, &mut ctx_reversed, &reversed);
    let rerun = provider::update(&mut schedule_reversed, &mut ctx_reversed, 0, 5).expect("update");
    assert_eq!(rerun, report);
    assert_eq!(
        ctx_reversed
            .read()
            .block(Dimension::OVERWORLD, BlockPos::new(1, 10, 0)),
        Some(LEVEL_1)
    );
    assert_eq!(
        ctx_reversed
            .read()
            .block(Dimension::OVERWORLD, BlockPos::new(11, 10, 0)),
        Some(LEVEL_1)
    );

    // A budget of five against seven unit-cost items processes exactly five in
    // due order and carries two, pinning the uniform per-item cost.
    let mut authority_tight = authority();
    let budget = TickBudget::try_new(4096, 5, 65536, 65536, 65536).expect("budget");
    let mut ctx_tight = TickContext::harness(&mut authority_tight, budget);
    let mut schedule_tight = FluidSchedule::new();
    layout(&mut schedule_tight, &mut ctx_tight, &queued);
    let tight = provider::update(&mut schedule_tight, &mut ctx_tight, 0, 5).expect("update");
    assert_eq!(
        tight,
        PhaseReport {
            examined: 5,
            applied: 2,
            carried: 2,
            rejected: 0,
        }
    );
    assert_eq!(
        schedule_tight.fluid_due(Dimension::OVERWORLD, BlockPos::new(100, 10, 0)),
        Some(0)
    );
}
