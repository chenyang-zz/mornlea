//! Farmland moisture replay.
//!
//! Every gate below mirrors a frozen Go oracle row, cited at each case:
//!
//! - The per-candidate settlement order — check-budget guard, active Ready
//!   scope gate with zero reads, target read, whole-neighborhood reservation,
//!   dy/z/x scan where any Ready fluid hydrates — from
//!   `packages/server/sim/realm/environment.go` (`AdvanceFarmlandMoisture`,
//!   `newFarmlandMoistureHandler`, `farmlandIsWet`).
//! - The shared cross-dimension budgets and the original-due-tick carry from
//!   `TestFarmlandMoistureBudgetIsGlobalAcrossDimensions` in
//!   `packages/server/sim/realm/farmland_moisture_budget_test.go`: an
//!   exhausted budget defers at the guard that refused, with zero reads and
//!   no partial neighborhood state.
//! - The dry worst case of exactly one target read plus 162 neighborhood
//!   reads from `TestFarmlandMoistureDryCandidateUsesWorstCaseReads` in the
//!   same file.
//!
//! No case chooses a value the oracles do not pin. Refusals and carries
//! compare queue leftovers and world cells, not a bare `is_err`.

use std::collections::BTreeSet;

use mornlea_domain::{BlockPos, ChunkPos, Dimension};
use mornlea_server::contracts::{
    BlockObservation, ChunkKey, PhaseReport, RuleCall, RulePhase, ServerLimits, TickBudget,
};
use mornlea_server::rules::farmland::{self as provider, FarmlandSchedule};
use mornlea_server::state::{AuthorityState, TickContext};

// Stable block numbers, mirrored from the frozen const block in
// `packages/shared/core/block.go` and `packages/shared/core/fluid.go`.
const AIR: u16 = 0; // `core.AirID`
const FARMLAND_DRY: u16 = 35; // `core.FarmlandDryID`

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
        .expect("observation");
    ctx.preload_block(observed);
}

/// Preloads the 162-cell hydration neighborhood of one farmland cell in Go
/// `farmlandIsWet` enumeration order (dy 0..=1, dz, dx), leaving the center
/// cell untouched.
fn observe_neighborhood(ctx: &mut TickContext<'_>, dimension: Dimension, center: BlockPos) {
    for dy in 0..=1 {
        for dz in -4..=4 {
            for dx in -4..=4 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                observe(
                    ctx,
                    dimension,
                    BlockPos::new(center.x() + dx, center.y() + dy, center.z() + dz),
                    AIR,
                );
            }
        }
    }
}

fn farmland_call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::Farmland,
        actor: None,
        command: None,
        internal: None,
    }
}

fn zero_report() -> PhaseReport {
    PhaseReport {
        examined: 0,
        applied: 0,
        carried: 0,
        rejected: 0,
    }
}

/// Read-budget reservation: a target read that leaves 161 reads cannot
/// reserve the full 162-cell neighborhood, so the candidate defers at its
/// original due tick with no partial neighbor state; one more read completes
/// the dry worst case of 1 + 162 = 163.
#[test]
fn reads_162_163() {
    let mut authority = authority();
    // The batch entry owns no queue port, so even a well-shaped Farmland call
    // reports zero without effect; a foreign shape refuses the same way.
    {
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        let batch = provider::run(&mut ctx, farmland_call()).expect("batch entry");
        assert_eq!(batch, zero_report());
        let refused = provider::run(
            &mut ctx,
            RuleCall {
                phase: RulePhase::Interaction,
                actor: None,
                command: None,
                internal: None,
            },
        )
        .expect("foreign shape refusal");
        assert_eq!(refused, zero_report());
    }

    // First tick: the read budget covers the target read but not the whole
    // reserved neighborhood (`farmlandMoistureReadsPerTick` boundary of the
    // budget test, narrowed to the 162/163 edge).
    let budget = TickBudget::try_new(4096, 512, 65536, 8, 162).expect("budget");
    let mut ctx = TickContext::harness(&mut authority, budget);
    let mut schedule = FarmlandSchedule::new();
    let pos = BlockPos::new(8, 1, 8);
    let key = chunk_key(Dimension::OVERWORLD, pos);
    observe(&mut ctx, Dimension::OVERWORLD, pos, FARMLAND_DRY);
    schedule.enqueue_candidate(key, pos, 0);
    let scope: BTreeSet<ChunkKey> = [key].into_iter().collect();

    let report = provider::advance(&mut schedule, &mut ctx, &scope, 0).expect("advance");
    assert_eq!(
        report,
        PhaseReport {
            examined: 1,
            applied: 0,
            carried: 1,
            rejected: 0,
        }
    );
    // The deferral retains the original due tick and the dry farmland keeps
    // every cell of its neighborhood: no partial hydration state survives.
    assert_eq!(schedule.candidate_due(Dimension::OVERWORLD, pos), Some(0));
    assert_eq!(
        ctx.read().block(Dimension::OVERWORLD, pos),
        Some(FARMLAND_DRY)
    );

    // Second tick: the rebuilt budget settles the dry worst case. A waterless
    // Ready neighborhood reads as dry and the cell stays dry — the 30%
    // dry-revert roll of the random-rules node is out of scope here; moisture
    // only reports dry.
    let budget = TickBudget::try_new(4096, 512, 65536, 8, 163).expect("budget");
    let mut ctx = TickContext::harness(&mut authority, budget);
    observe(&mut ctx, Dimension::OVERWORLD, pos, FARMLAND_DRY);
    observe_neighborhood(&mut ctx, Dimension::OVERWORLD, pos);
    let report = provider::advance(&mut schedule, &mut ctx, &scope, 0).expect("advance");
    assert_eq!(
        report,
        PhaseReport {
            examined: 1,
            applied: 0,
            carried: 0,
            rejected: 0,
        }
    );
    assert_eq!(schedule.pending_candidates(Dimension::OVERWORLD), 0);
    assert_eq!(
        ctx.read().block(Dimension::OVERWORLD, pos),
        Some(FARMLAND_DRY)
    );
}

/// Global budget across dimensions with original-due carry: an exhausted
/// budget pauses the domain at the guard that refused, the deferred candidate
/// keeps its original due tick with zero reads and no partial hydration
/// state, and the rebuilt budget settles it whole the next tick.
#[test]
fn global_budget_and_original_due() {
    // First tick: a one-check, one-read global budget. The Overworld
    // candidate (air, non-farmland) consumes both units; the Depths candidate
    // then defers at the check guard without charging it.
    let mut authority_a = authority();
    let budget = TickBudget::try_new(4096, 512, 65536, 1, 1).expect("budget");
    let mut ctx = TickContext::harness(&mut authority_a, budget);
    let mut schedule = FarmlandSchedule::new();
    let pos = BlockPos::new(8, 1, 8);
    let overworld = chunk_key(Dimension::OVERWORLD, pos);
    let depths = chunk_key(Dimension::DEPTHS, pos);
    observe(&mut ctx, Dimension::OVERWORLD, pos, AIR);
    schedule.enqueue_candidate(overworld, pos, 0);
    observe(&mut ctx, Dimension::DEPTHS, pos, FARMLAND_DRY);
    schedule.enqueue_candidate(depths, pos, 0);
    let scope: BTreeSet<ChunkKey> = [overworld, depths].into_iter().collect();

    let report = provider::advance(&mut schedule, &mut ctx, &scope, 0).expect("advance");
    assert_eq!(
        report,
        PhaseReport {
            examined: 1,
            applied: 0,
            carried: 1,
            rejected: 0,
        }
    );
    assert_eq!(schedule.candidate_due(Dimension::DEPTHS, pos), Some(0));
    assert_eq!(ctx.read().block(Dimension::DEPTHS, pos), Some(FARMLAND_DRY));

    // Second tick: budgets rebuilt, the carried candidate settles whole with
    // the dry worst case (1 + 162 reads) and the waterless neighborhood keeps
    // the farmland dry, mirroring the Go dual-dimension budget oracle.
    let mut authority_b = authority();
    let mut ctx = TickContext::harness(&mut authority_b, TickBudget::full());
    observe(&mut ctx, Dimension::DEPTHS, pos, FARMLAND_DRY);
    observe_neighborhood(&mut ctx, Dimension::DEPTHS, pos);
    let scope: BTreeSet<ChunkKey> = [depths].into_iter().collect();
    let report = provider::advance(&mut schedule, &mut ctx, &scope, 0).expect("advance");
    assert_eq!(
        report,
        PhaseReport {
            examined: 1,
            applied: 0,
            carried: 0,
            rejected: 0,
        }
    );
    assert_eq!(schedule.pending_candidates(Dimension::DEPTHS), 0);
    assert_eq!(ctx.read().block(Dimension::DEPTHS, pos), Some(FARMLAND_DRY));
}
