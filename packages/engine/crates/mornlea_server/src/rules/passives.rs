//! Passive lifecycle: spawn admission, the priority chain, grazing,
//! temptation and deathless removal.
//!
//! This provider owns the [`RulePhase::PassiveStepDeaths`] batch. One call
//! advances the whole passive set for one tick in the frozen Go order
//! (`advancePassives` plus the death settle that follows it in
//! `packages/server/sim/entity/entity.go`): resident admission, at most one
//! spawn candidate, movement, graze events, then death settlement.
//!
//! Mirrored Go rows, each cited at its site:
//!
//! - `packages/server/sim/entity/passive_spawn.go` (`advancePassiveSpawn`,
//!   `passiveSpawnColumnSpot`, `passiveNearLimitExceeded`): overworld daytime
//!   anchors only, the global cap 32 as the cheapest precondition, one derived
//!   candidate per tick, the shared integer-derived radius 24..48 column, a
//!   grass-supported two-air column, the local cap of 6 passives inside a
//!   player's 48-block radius, the candidate hash id with the 64-rehash
//!   budget, and the `fresh` newborn that skips its first movement tick.
//! - `packages/server/sim/entity/passive.go` (`passiveStepInput`,
//!   `advancePassiveMovement`, `passiveIdleLookTarget`, `RestorePassive`,
//!   `settlePassiveDeaths`, `turnYawToward`, `outsideHomeNeighborhood`): the
//!   priority chain submerged dry-turn, shoreline avoidance, flee 60, graze
//!   freeze, wheat temptation, idle look, wander; movement through the same F1
//!   physics kernels as players with the home-neighborhood rollback and the
//!   fall-out removal; restore admission that re-anchors home and zeroes every
//!   transient; death settled once with no late resurrection.
//! - `packages/server/sim/entity/passive_tempt.go` (`passiveTemptTarget`):
//!   wheat in the authoritative selected hotbar slot within 8 blocks
//!   inclusive, nearest holder first with the ascending-session tie, the 2.5
//!   stop distance.
//! - `packages/server/sim/entity/passive_graze.go`
//!   (`advancePassiveGrazeOne`, `tryStartPassiveGraze`,
//!   `settlePassiveGraze`): the 20-tick event counting the trigger tick, the
//!   support/ready aborts, and the single-cell grass-to-dirt settlement.
//! - `packages/server/updates/sampler.go` (`PassiveGrazeHit`,
//!   `SplitMix64`, `HostileCandidateHash`): the 1-in-600 roll under salt
//!   `0x51ab3e4d07c3f291` and the candidate hash chain.
//! - `packages/shared/core/day_phase.go` and `season.go`
//!   (`EffectiveDayPhaseAt`, `DayArcTicks`, `YearPhaseAt`): the only seasonal
//!   day-phase path, mirrored wholesale; `phaseIsDay` in
//!   `packages/server/sim/entity/hostile.go` pins the daytime window.
//! - `packages/shared/physics/step.go`, `collision.go`, `submersion.go` and
//!   `types.go` (`StepWithTunables`, `stepSweepBounds`, `stepPrismFor`,
//!   `encodeStepInput`, `SubmersionFlagsWithTunables`,
//!   `BlockCollisionBoxes`): the kernel-facing sweep, prism, grid and
//!   submersion mirrors, identical in shape to the accepted player-motion
//!   plumbing.
//!
//! Deliberate boundaries. Graze settlement commits through the accepted
//! system-transaction entry under [`SystemRule::PassiveGraze`]; the overlay's
//! observation-backed reads stand in for the whole-chunk readiness gate the
//! serial reducer owns together with chunk acquisition. Death settlement owns
//! the fixed beef loot beside the terminal record; fall-out removal stays
//! lootless. Damage entry, flee triggering and melee belong to the combat
//! nodes; this provider consumes the flee lane the aux carries.

use mornlea_domain::{
    BlockPos, ChunkPos, Dimension, FiniteVec3, LookAngles, MotionState, MotionStateParts,
    SurvivalState, SurvivalStateParts,
};
use mornlea_engine::native::contracts::collision::{Aabb, CollisionCell, CollisionGrid};
use mornlea_engine::native::contracts::physics::{
    PhysicsControls, PhysicsOp, PhysicsRequest, PhysicsState, PhysicsTuning, SweepBounds,
};
use mornlea_engine::native::physics::NativePhysics;
use mornlea_storage::{ItemStack, PassiveMob};

use crate::core::actor_snow::apply_snow_slowdown;
use crate::core::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, BlockWrite, ChunkKey,
    DropBatch, DropSource, EnvironmentState, PhaseReport, RuleCall, RuleEffect, RulePhase,
    ServerError, SessionKey, SystemRule,
};
use crate::core::state::{AuthorityReadView, TickContext};
use crate::rules::environment::year_phase_at;

// -- Frozen numeric contract (`passive.go`, `passive_spawn.go`,
// `passive_tempt.go`, `passive_graze.go`, `packages/server/updates/sampler.go`).
// Each value is locked by the Go boundary tests; none scales with world size.

/// Global passive cap (`maxPassives`, `passive.go`).
const MAX_PASSIVES: u64 = 32;
/// Flee duration after one effective hit (`passiveFleeDurationTicks`). The
/// damage entry that arms the lane belongs to the combat nodes; the frozen
/// duration lives beside the chain that consumes and aborts on it.
#[allow(dead_code)]
const FLEE_DURATION_TICKS: u16 = 60;
/// Idle-look horizontal radius (`passiveIdleLookRadius`).
const IDLE_LOOK_RADIUS: f32 = 6.0;
/// Bounded turn per tick (`passiveIdleLookMaxTurn`).
const MAX_TURN: f32 = 0.2;
/// Wander heading segment length (`passiveWanderSegmentTicks`).
const WANDER_SEGMENT_TICKS: u64 = 40;
/// Motion stop before the body enters fluid (`passiveWaterLookaheadBlocks`).
const WATER_LOOKAHEAD: f32 = 1.5;
/// Second-block dry probe (`passiveDryProbeOffsetBlocks`).
const DRY_PROBE_OFFSET_BLOCKS: i32 = 2;
/// Temptation radius, boundary inclusive (`passiveTemptRadius`).
const TEMPT_RADIUS: f32 = 8.0;
/// Squared stop distance (`passiveTemptStopDistance` 2.5, squared in place).
const TEMPT_STOP_SQ: f32 = 2.5 * 2.5;
/// Graze event duration including the trigger tick
/// (`passiveGrazeDurationTicks`).
const GRAZE_DURATION_TICKS: u16 = 20;
/// Graze roll salt (`PassiveGrazeRollSalt`, `packages/server/updates/sampler.go`).
const GRAZE_SALT: u64 = 0x51ab_3e4d_07c3_f291;
/// Graze roll denominator (`PassiveGrazePeriodTicks`).
const GRAZE_PERIOD: u64 = 600;
/// Passives allowed within one active player's near radius
/// (`maxPassivesNearPlayer`, `passive_spawn.go`).
const NEAR_CAP: u64 = 6;
/// Near-radius in blocks (`maxPassivesNearRadius`).
const NEAR_RADIUS: f32 = 48.0;
/// Minimum candidate column distance (`hostileSpawnMinRadius`, the shared
/// window `passive_spawn.go` reuses).
const SPAWN_MIN_RADIUS: i64 = 24;
/// Rehash budget for a conflicting candidate id (`passiveSpawnMaxRehashes`).
const SPAWN_REHASH_BUDGET: u64 = 64;
/// Full health (`core.MaxHealth`, `packages/shared/core/health.go`).
const MAX_HEALTH: u8 = 20;
/// Raw beef (`core.ItemRawBeef`, `packages/shared/core/item.go`): the fixed
/// passive death batch (`dropPassiveLoot`, `passive.go`).
const ITEM_RAW_BEEF: u16 = 53;

// -- World constants mirrored from the frozen Go tables.

/// Lowest world Y, inclusive (`core.MinY`, `packages/shared/core/pos.go`).
const MIN_Y: i32 = -64;
/// Highest world Y, exclusive (`core.MaxY`).
const MAX_Y: i32 = 320;
/// Air (`core.AirID`, `packages/shared/core/block.go`).
const AIR: u16 = 0;
/// Grass support (`core.GrassID`).
const GRASS: u16 = 4;
/// Settlement product (`core.DirtID`).
const DIRT: u16 = 3;
/// Wheat in the selected slot (`core.ItemWheat`,
/// `packages/shared/core/item.go`).
const ITEM_WHEAT: u16 = 35;
/// Fluid range (`core.IsFluid`, `packages/shared/core/fluid.go`).
const FLUID_FIRST: u16 = 27;
const FLUID_LAST: u16 = 34;

// -- Physics mirrors (`packages/shared/physics/types.go`, `collision.go`).

/// Body half width (`PlayerWidth / 2`; passives share the player body shape
/// because the Go passive state is a plain `physics.State`).
const HALF_WIDTH: f32 = 0.3;
/// Body height (`PlayerHeight`).
const PLAYER_HEIGHT: f32 = 1.8;
/// Sweep padding (`CollisionEpsilon`).
const COLLISION_EPSILON: f32 = 1e-5;
/// Ground support probe (`GroundProbe`).
const GROUND_PROBE: f32 = 1e-4;
/// Grid cell cap (`collisionMaxCells`, `packages/shared/physics/collision.go`).
const PRISM_MAX_CELLS: u64 = 4096;
/// Farmland pair, top face at 15/16 (`IsFarmland`, `farmlandCollisionHeight`).
const FARMLAND_FIRST: u16 = 35;
const FARMLAND_LAST: u16 = 36;
const FARMLAND_TOP: f32 = 0.9375;
/// Crop stages, zero collision (`IsCrop`,
/// `packages/shared/core/farming.go`).
const WHEAT_FIRST: u16 = 37;
const WHEAT_LAST: u16 = 44;
const POTATO_FIRST: u16 = 46;
const POTATO_LAST: u16 = 53;
const CARROT_FIRST: u16 = 54;
const CARROT_LAST: u16 = 61;
/// Wild grass, zero collision (`ShortGrassID`).
const SHORT_GRASS: u16 = 84;
/// Snow layers, zero collision (`SnowLayer1BlockID..=SnowLayer4BlockID`).
const SNOW_FIRST: u16 = 85;
const SNOW_LAST: u16 = 88;
/// Sapling, zero collision (`SaplingID`).
const SAPLING: u16 = 89;
/// Door lower range and upper form (`DoorLowerSouthClosed..=DoorUpper`).
const DOOR_LOWER_FIRST: u16 = 62;
const DOOR_LOWER_LAST: u16 = 69;
const DOOR_UPPER: u16 = 70;
const DOOR_THICKNESS: f32 = 3.0 / 16.0;
/// Torch forms, zero collision (`TorchStandingID..=TorchWallNegZID`).
const TORCH_FIRST: u16 = 71;
const TORCH_LAST: u16 = 75;
/// Bed forms, half box at 9/16 (`BedFootSouthID..=BedHeadEastID`).
const BED_FIRST: u16 = 76;
const BED_LAST: u16 = 83;
const BED_TOP: f32 = 0.5625;

/// Probe order fixed to +Z, +X, -Z, -X (`passiveDryProbeOffsets`, `passive.go`);
/// full ordered enumeration keeps selection deterministic.
const DRY_PROBE_OFFSETS: [(i32, i32); 4] = [(0, 1), (1, 0), (0, -1), (-1, 0)];

/// The batch entry: exactly the [`RulePhase::PassiveStepDeaths`] batch shape
/// advances the lifecycle; every other shape refuses without effect. Counting:
/// `examined` adds one derived spawn candidate plus each active resident once
/// per pass (movement, graze); `applied` counts staged effects; `rejected`
/// counts candidates refused after derivation; `carried` is always zero
/// because nothing carries between ticks.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    run_with_snow(ctx, None, call)
}

/// The serial reducer's batch entry: the body is identical to the public
/// legacy entry, plus the retained Snow book's three touch points — resident
/// registration after spawn and before movement, per-resident capture after
/// each accepted step, and the death-side tracker forget. The public `run`
/// keeps the exact book-less observable behavior.
pub(crate) fn run_with_snow(
    ctx: &mut TickContext<'_>,
    mut snow: Option<&mut PassiveSnowBook>,
    call: RuleCall<'_>,
) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::PassiveStepDeaths
        || call.actor.is_some()
        || call.command.is_some()
        || call.internal.is_some()
    {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    // The tick-start environment snapshot names the seed, clock and tunables
    // this batch consumes; a missing snapshot is a reducer sequencing bug,
    // refused before any staging so a failed batch leaves nothing behind.
    let environment = ctx
        .read()
        .environment()
        .cloned()
        .ok_or(ServerError::Internal {
            invariant: "passive lifecycle snapshot",
        })?;
    let mut report = PhaseReport {
        examined: 0,
        applied: 0,
        carried: 0,
        rejected: 0,
    };
    admit_residents(ctx, &mut report, snow.as_deref_mut())?;
    advance_spawn(ctx, &environment, &mut report, snow.as_deref_mut())?;
    // Every current resident registers after spawn and before movement —
    // fresh newborns and graze-frozen cows included, so the retained book owns
    // the whole resident set from its first tick. The movement loop reuses the
    // same resident snapshot with no second collection.
    let keys = resident_keys(ctx);
    if let Some(book) = snow.as_deref_mut() {
        register_snow_residents(ctx, book, &keys)?;
    }
    for key in keys {
        advance_movement(ctx, key, &environment, &mut report, snow.as_deref_mut())?;
        report.examined += 1;
    }
    for key in resident_keys(ctx) {
        advance_graze(ctx, key, &environment, &mut report)?;
        report.examined += 1;
    }
    settle_deaths(ctx, &mut report, snow)?;
    Ok(report)
}

// ---------------------------------------------------------------------
// Retained passive Snow capture (the retained mirror of the source row)
// ---------------------------------------------------------------------

/// Retained stride threshold: a crossing resets the accumulator and discards
/// the overshoot exactly like the source tracker row.
const PASSIVE_SNOW_STRIDE: f32 = 0.6;
/// Tracker slots: one per resident under the global passive cap.
const PASSIVE_SNOW_SLOTS: usize = 32;
/// Pending copied-candidate bound; the late settle drains it in slices.
const PASSIVE_SNOW_PENDING: usize = 32;
/// The existing fresh-cell consumer settles at most eight cells per call.
const PASSIVE_SNOW_SETTLE_SLICE: usize = 8;

/// One resident's retained Snow accumulator: travel since the last sample
/// plus the last sampled cell, remembered even when the sample wrote nothing,
/// so the same cell is never re-sampled before the resident leaves it. The
/// cell is remembered whatever it holds, snow or not.
#[derive(Copy, Clone)]
struct PassiveSnowTracker {
    travel: f32,
    cell: BlockPos,
    cell_valid: bool,
}

impl PassiveSnowTracker {
    fn new() -> Self {
        Self {
            travel: 0.0,
            cell: BlockPos::ORIGIN,
            cell_valid: false,
        }
    }

    /// Pure preview geometry, byte-identical to the accepted source tracker:
    /// source-f32 squares widened through the square root, the 0.6 threshold
    /// discarding overshoot, same-cell suppression, the checked foot cell.
    fn preview(
        &self,
        before: [f32; 3],
        after: [f32; 3],
    ) -> Result<(Self, Option<BlockPos>), ServerError> {
        let mut next = *self;
        let dx = after[0] - before[0];
        let dz = after[2] - before[2];
        let distance = (f64::from(dx * dx + dz * dz).sqrt()) as f32;
        next.travel += distance;
        if next.travel < PASSIVE_SNOW_STRIDE {
            return Ok((next, None));
        }
        next.travel = 0.0;
        let cell = crate::core::actor_placement::snow_cell(after)?;
        if next.cell_valid && next.cell == cell {
            return Ok((next, None));
        }
        next.cell = cell;
        next.cell_valid = true;
        Ok((next, Some(cell)))
    }
}

/// The reducer-owned retained passive Snow book: fixed tracker slots for
/// current residents plus the fixed pending batch of copied candidates.
/// Trackers persist across ticks; the pending batch drains each tick through
/// the existing fresh-cell consumer. Reconciliation forgets only tracker
/// slots, so queued coordinates always survive retirement and death.
pub(crate) struct PassiveSnowBook {
    trackers: [Option<(mornlea_domain::PassiveId, PassiveSnowTracker)>; PASSIVE_SNOW_SLOTS],
    pending: [crate::rules::crops::FootprintCell; PASSIVE_SNOW_PENDING],
    len: usize,
}

impl Default for PassiveSnowBook {
    fn default() -> Self {
        Self {
            trackers: [None; PASSIVE_SNOW_SLOTS],
            pending: [crate::rules::crops::FootprintCell {
                dimension: Dimension::OVERWORLD,
                pos: BlockPos::ORIGIN,
            }; PASSIVE_SNOW_PENDING],
            len: 0,
        }
    }
}

impl PassiveSnowBook {
    #[cfg(test)]
    fn snow_test_state(&self, id: mornlea_domain::PassiveId) -> Option<(f32, BlockPos, bool)> {
        self.trackers
            .iter()
            .find(|slot| slot.is_some_and(|(held, _)| held == id))
            .map(|slot| match slot {
                Some((_, tracker)) => (tracker.travel, tracker.cell, tracker.cell_valid),
                None => unreachable!("matched slot holds a tracker"),
            })
    }

    /// Exact owned-id qualification: only a registered resident's tracker is
    /// visible to capture.
    fn tracker_mut(&mut self, id: mornlea_domain::PassiveId) -> Option<&mut PassiveSnowTracker> {
        self.trackers.iter_mut().find_map(|slot| match slot {
            Some((held, tracker)) if *held == id => Some(tracker),
            _ => None,
        })
    }

    fn owns(&self, id: mornlea_domain::PassiveId) -> bool {
        self.trackers
            .iter()
            .any(|slot| slot.is_some_and(|(held, _)| held == id))
    }

    fn commit_tracker(&mut self, id: mornlea_domain::PassiveId, tracker: PassiveSnowTracker) {
        if let Some(held) = self.tracker_mut(id) {
            *held = tracker;
        }
    }

    /// Death-side forget: clears only the tracker slot, never the pending
    /// batch, so a dying resident's queued coordinates still settle.
    fn forget(&mut self, id: mornlea_domain::PassiveId) {
        for slot in &mut self.trackers {
            if slot.is_some_and(|(held, _)| held == id) {
                *slot = None;
            }
        }
    }

    /// Lookup-or-insert reconciliation over the borrowed resident keys:
    /// retired identities free their slots first, so a stale entry can never
    /// refuse a new resident's admission; matched ids keep their accumulated
    /// travel and remembered cell, and the pending prefix is untouched.
    fn reconcile(&mut self, current: &[ActorKey]) {
        for slot in &mut self.trackers {
            if slot
                .as_ref()
                .is_some_and(|(held, _)| !current_holds(current, *held))
            {
                *slot = None;
            }
        }
        for key in current {
            let ActorKey::Passive(id) = key else {
                continue;
            };
            if self.owns(*id) {
                continue;
            }
            if let Some(free) = self.trackers.iter_mut().find(|slot| slot.is_none()) {
                *free = Some((*id, PassiveSnowTracker::new()));
            }
        }
    }

    fn append(&mut self, dimension: Dimension, pos: BlockPos) -> Result<(), ServerError> {
        if self.len >= PASSIVE_SNOW_PENDING {
            return Err(ServerError::Internal {
                invariant: "passive snow capacity",
            });
        }
        self.pending[self.len] = crate::rules::crops::FootprintCell { dimension, pos };
        self.len += 1;
        Ok(())
    }

    fn pending_len(&self) -> usize {
        self.len
    }

    fn pending_slice(&self, start: usize, count: usize) -> &[crate::rules::crops::FootprintCell] {
        &self.pending[start..start + count]
    }

    fn clear_pending(&mut self) {
        self.len = 0;
    }
}

/// Whether the borrowed resident slice holds this passive identity.
fn current_holds(current: &[ActorKey], id: mornlea_domain::PassiveId) -> bool {
    current
        .iter()
        .any(|key| matches!(key, ActorKey::Passive(held) if *held == id))
}

/// Registers every current resident into the retained book and marks the
/// tick-local ownership lane the legacy single-tick collector excludes. The
/// bound preflight runs before any tracker change, so an oversize set refuses
/// instead of silently leaving a resident untracked.
fn register_snow_residents(
    ctx: &mut TickContext<'_>,
    book: &mut PassiveSnowBook,
    residents: &[ActorKey],
) -> Result<(), ServerError> {
    let count = residents
        .iter()
        .filter(|key| matches!(key, ActorKey::Passive(_)))
        .count();
    if count > PASSIVE_SNOW_SLOTS {
        return Err(ServerError::Internal {
            invariant: "passive snow capacity",
        });
    }
    book.reconcile(residents);
    for key in residents {
        if let ActorKey::Passive(id) = key {
            ctx.record_passive_snow_owned(*id)?;
        }
    }
    Ok(())
}

/// Captures one resident's accepted step into the retained book. The preview
/// geometry and the append preflight both complete before the tracker
/// commits, so a refused append leaves the previous accumulator untouched. An
/// airborne step preserves travel without sampling or resetting; the home
/// rollback arrives as a zero-distance step.
fn capture_passive_snow(
    book: &mut PassiveSnowBook,
    dimension: Dimension,
    id: mornlea_domain::PassiveId,
    before: [f32; 3],
    after: [f32; 3],
    grounded: bool,
) -> Result<(), ServerError> {
    if !grounded {
        return Ok(());
    }
    let Some(tracker) = book.tracker_mut(id) else {
        return Ok(());
    };
    let (next, candidate) = tracker.preview(before, after)?;
    if let Some(pos) = candidate {
        book.append(dimension, pos)?;
    }
    book.commit_tracker(id, next);
    Ok(())
}

/// Late settle: settles the copied candidates through the existing fresh-cell
/// consumer in bounded in-order slices, reusing its transactions unchanged.
/// Only a fully settled batch clears the prefix; a refusal leaves the whole
/// prefix charged and untouched.
pub(crate) fn settle_snow(
    book: &mut PassiveSnowBook,
    ctx: &mut TickContext<'_>,
) -> Result<PhaseReport, ServerError> {
    let len = book.pending_len();
    if len > PASSIVE_SNOW_PENDING {
        return Err(ServerError::Internal {
            invariant: "passive snow capacity",
        });
    }
    let mut report = PhaseReport {
        examined: 0,
        applied: 0,
        carried: 0,
        rejected: 0,
    };
    let mut settled_count = 0;
    while settled_count < len {
        let take = (len - settled_count).min(PASSIVE_SNOW_SETTLE_SLICE);
        let settled = crate::rules::crops::settle_captured_source_snow(
            book.pending_slice(settled_count, take),
            ctx,
        )?;
        report.examined += settled.examined;
        report.applied += settled.applied;
        settled_count += take;
    }
    book.clear_pending();
    Ok(report)
}

#[cfg(test)]
mod passive_snow_tests {
    use super::*;

    fn id(value: u64) -> mornlea_domain::PassiveId {
        mornlea_domain::PassiveId::try_new(value).unwrap()
    }

    #[test]
    fn preview_accumulates_resets_and_discards_overshoot() {
        let tracker = PassiveSnowTracker::new();
        let (tracker, none) = tracker.preview([0.0, 1.0, 0.0], [0.3, 1.0, 0.0]).unwrap();
        assert!(none.is_none());
        assert!((tracker.travel - 0.3).abs() < 1e-6);
        // Crossing resets exactly at the threshold: overshoot is discarded and
        // the landing cell is remembered.
        let (tracker, cell) = tracker.preview([0.3, 1.0, 0.0], [1.0, 1.0, 0.0]).unwrap();
        assert_eq!(cell, Some(BlockPos::new(1, 1, 0)));
        assert_eq!(tracker.travel, 0.0);
        assert_eq!(tracker.cell, BlockPos::new(1, 1, 0));
        assert!(tracker.cell_valid);
        // A later crossing inside the same cell suppresses the candidate but
        // still clears the travel.
        let (tracker, none) = tracker.preview([1.2, 1.0, 0.0], [1.9, 1.0, 0.0]).unwrap();
        assert!(none.is_none());
        assert_eq!(tracker.travel, 0.0);
        // A zero-distance step (the home rollback form) preserves travel.
        let (tracker, none) = tracker.preview([1.9, 1.0, 0.0], [1.9, 1.0, 0.4]).unwrap();
        assert!(none.is_none());
        assert_eq!(tracker.travel, 0.4);
    }

    #[test]
    fn reconcile_forgoes_retired_and_reuses_slots_without_losing_pending() {
        let mut book = PassiveSnowBook::default();
        let one = id(1);
        let two = id(2);
        book.reconcile(&[ActorKey::Passive(one), ActorKey::Passive(two)]);
        assert!(book.owns(one) && book.owns(two));
        book.append(Dimension::OVERWORLD, BlockPos::new(4, 1, 4))
            .unwrap();
        // A retired id frees only its tracker slot; queued coordinates survive.
        book.reconcile(&[ActorKey::Passive(two)]);
        assert!(!book.owns(one));
        assert!(book.owns(two));
        assert_eq!(book.pending_len(), 1);
        // A new neighbor id takes the freed slot; matched ids keep theirs.
        let three = id(3);
        book.reconcile(&[ActorKey::Passive(two), ActorKey::Passive(three)]);
        assert!(book.owns(two) && book.owns(three));
        assert_eq!(book.pending_len(), 1);
        // Death forget clears only the tracker, never the pending batch.
        book.forget(two);
        assert!(!book.owns(two));
        assert_eq!(book.pending_len(), 1);
    }

    #[test]
    fn capture_preflights_capacity_and_preserves_travel() {
        let mut book = PassiveSnowBook::default();
        let one = id(1);
        book.reconcile(&[ActorKey::Passive(one)]);
        for offset in 0..PASSIVE_SNOW_PENDING {
            book.append(Dimension::OVERWORLD, BlockPos::new(offset as i32, 1, 0))
                .unwrap();
        }
        // A short step commits partial travel with no candidate.
        capture_passive_snow(
            &mut book,
            Dimension::OVERWORLD,
            one,
            [0.0, 1.0, 0.0],
            [0.3, 1.0, 0.0],
            true,
        )
        .unwrap();
        // A full pending batch refuses the append before the tracker commits,
        // so the crossing leaves the retained accumulator untouched and the
        // prefix ordered.
        assert_eq!(
            capture_passive_snow(
                &mut book,
                Dimension::OVERWORLD,
                one,
                [0.3, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                true,
            ),
            Err(ServerError::Internal {
                invariant: "passive snow capacity"
            })
        );
        let (travel, _, _) = book.snow_test_state(one).unwrap();
        assert!((travel - 0.3).abs() < 1e-6);
        assert_eq!(
            book.pending_slice(0, PASSIVE_SNOW_PENDING)[0].pos,
            BlockPos::new(0, 1, 0)
        );
    }

    #[test]
    fn airborne_capture_preserves_travel_and_samples_nothing() {
        let mut book = PassiveSnowBook::default();
        let one = id(1);
        book.reconcile(&[ActorKey::Passive(one)]);
        // Seed grounded positive subthreshold travel through a real capture.
        capture_passive_snow(
            &mut book,
            Dimension::OVERWORLD,
            one,
            [0.0, 1.0, 0.0],
            [0.3, 1.0, 0.0],
            true,
        )
        .unwrap();
        let seeded = book.snow_test_state(one).unwrap();
        assert!((seeded.0 - 0.3).abs() < 1e-6);
        assert!(!seeded.2);
        // The airborne step preserves the exact nonzero tracker and samples
        // nothing: cell, prefix and length are unchanged.
        capture_passive_snow(
            &mut book,
            Dimension::OVERWORLD,
            one,
            [0.3, 1.0, 0.0],
            [2.0, 3.0, 0.0],
            false,
        )
        .unwrap();
        assert_eq!(book.snow_test_state(one).unwrap(), seeded);
        assert_eq!(book.pending_len(), 0);
        // The later grounded threshold crossing yields the exact destination
        // cell: overshoot discarded, accumulator reset, cell remembered.
        capture_passive_snow(
            &mut book,
            Dimension::OVERWORLD,
            one,
            [2.0, 3.0, 0.0],
            [2.4, 3.0, 0.0],
            true,
        )
        .unwrap();
        let (travel, cell, valid) = book.snow_test_state(one).unwrap();
        assert_eq!(travel, 0.0);
        assert_eq!(cell, BlockPos::new(2, 3, 0));
        assert!(valid);
        assert_eq!(book.pending_len(), 1);
        assert_eq!(
            book.pending_slice(0, PASSIVE_SNOW_PENDING)[0].pos,
            BlockPos::new(2, 3, 0)
        );
    }

    #[test]
    fn capture_rejects_out_of_range_destination_without_mutation() {
        let mut book = PassiveSnowBook::default();
        let one = id(1);
        book.reconcile(&[ActorKey::Passive(one)]);
        book.append(Dimension::OVERWORLD, BlockPos::new(7, 1, 7))
            .unwrap();
        capture_passive_snow(
            &mut book,
            Dimension::OVERWORLD,
            one,
            [0.0, 1.0, 0.0],
            [0.3, 1.0, 0.0],
            true,
        )
        .unwrap();
        let seeded = book.snow_test_state(one).unwrap();
        let prefix: Vec<(Dimension, BlockPos)> = book
            .pending_slice(0, book.pending_len())
            .iter()
            .map(|cell| (cell.dimension, cell.pos))
            .collect();
        // A finite destination past the checked cell bound still crosses the
        // stride, but the geometry preflight refuses before the tracker
        // commits or the prefix grows.
        assert_eq!(
            capture_passive_snow(
                &mut book,
                Dimension::OVERWORLD,
                one,
                [0.3, 1.0, 0.0],
                [3.0e9, 1.0, 0.0],
                true,
            ),
            Err(ServerError::InvalidInput {
                field: "actor_geometry"
            })
        );
        assert_eq!(book.snow_test_state(one).unwrap(), seeded);
        let observed: Vec<(Dimension, BlockPos)> = book
            .pending_slice(0, book.pending_len())
            .iter()
            .map(|cell| (cell.dimension, cell.pos))
            .collect();
        assert_eq!(observed, prefix);
        assert_eq!(book.pending_len(), prefix.len());
    }

    #[test]
    fn home_rollback_zero_distance_preserves_travel() {
        let mut book = PassiveSnowBook::default();
        let one = id(1);
        book.reconcile(&[ActorKey::Passive(one)]);
        capture_passive_snow(
            &mut book,
            Dimension::OVERWORLD,
            one,
            [0.0, 1.0, 0.0],
            [0.5, 1.0, 0.0],
            true,
        )
        .unwrap();
        capture_passive_snow(
            &mut book,
            Dimension::OVERWORLD,
            one,
            [0.5, 1.0, 0.0],
            [0.5, 1.0, 0.0],
            true,
        )
        .unwrap();
        let (travel, _, valid) = book.snow_test_state(one).unwrap();
        assert!((travel - 0.5).abs() < 1e-6);
        assert!(!valid);
        assert_eq!(book.pending_len(), 0);
    }

    #[test]
    fn pending_slice_views_prefix_in_order_and_clear_is_whole_batch() {
        let mut book = PassiveSnowBook::default();
        for x in 0..10i32 {
            book.append(Dimension::OVERWORLD, BlockPos::new(x, 1, 0))
                .unwrap();
        }
        assert_eq!(book.pending_slice(0, 8)[0].pos, BlockPos::new(0, 1, 0));
        assert_eq!(book.pending_slice(8, 2)[0].pos, BlockPos::new(8, 1, 0));
        book.clear_pending();
        assert_eq!(book.pending_len(), 0);
    }

    #[test]
    fn runtimeless_admission_resets_tracker_and_keeps_pending() {
        use crate::core::contracts::{
            FixtureState, ServerLimits, SleepState, TickBudget, WorkState,
        };
        use crate::core::state::AuthorityState;
        let mut authority = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap();
        let nine = id(9);
        let record = ActorRecord::try_new(
            ActorKey::Passive(nine),
            ActorLifecycle::Active,
            Dimension::OVERWORLD,
            MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new([0.0, 1.0, 0.0]).unwrap(),
                velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
                on_ground: true,
            }),
            LookAngles::try_new(0.0, 0.0).unwrap(),
            SurvivalState::try_new(SurvivalStateParts {
                health: MAX_HEALTH,
                oxygen: 300,
                hunger: 20,
                saturation_zero: false,
                armor_points: 0,
            })
            .unwrap(),
            ActorBody::Passive(PassiveMob {
                id: nine.get(),
                dimension: 0,
                position: [0.0, 1.0, 0.0],
                velocity: [0.0; 3],
                on_ground: true,
                yaw: 0.0,
                health: MAX_HEALTH,
            }),
        )
        .unwrap();
        let fixture = FixtureState {
            runtime: vec![],
            actors: vec![record],
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
            world: mornlea_domain::WorldState::try_new(mornlea_domain::WorldStateParts {
                world_time_ticks: 0,
                day_phase_offset: 0,
                weather: mornlea_domain::Weather::Clear,
                season: mornlea_domain::Season::Spring,
                season_progress: 0,
                temperature: 0,
            })
            .unwrap(),
        };
        let mut c = TickContext::from_fixture(&mut authority, &fixture, TickBudget::full());
        let mut book = PassiveSnowBook::default();
        // Seed stale state: a remembered cell with a queued candidate plus
        // retained travel the restore must not inherit.
        book.reconcile(&[ActorKey::Passive(nine)]);
        capture_passive_snow(
            &mut book,
            Dimension::OVERWORLD,
            nine,
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            true,
        )
        .unwrap();
        capture_passive_snow(
            &mut book,
            Dimension::OVERWORLD,
            nine,
            [1.0, 1.0, 0.0],
            [1.4, 1.0, 0.0],
            true,
        )
        .unwrap();
        let mut report = PhaseReport {
            examined: 0,
            applied: 0,
            carried: 0,
            rejected: 0,
        };
        admit_residents(&mut c, &mut report, Some(&mut book)).unwrap();
        // The admission-side reset only forgets the stale tracker slot: the
        // tracker is absent immediately after admission, the runtime staged,
        // and the queued coordinate survived with its prefix intact.
        assert_eq!(book.snow_test_state(nine), None);
        assert_eq!(report.applied, 1);
        assert!(c.read().runtime(ActorKey::Passive(nine)).is_some());
        assert_eq!(book.pending_len(), 1);
        assert_eq!(
            book.pending_slice(0, book.pending_len())[0].pos,
            BlockPos::new(1, 1, 0)
        );
        // The actual flow reconciles the current resident keys after
        // admission: the fresh slot starts at zero travel with no remembered
        // cell, and the pending batch is untouched.
        let keys = resident_keys(&c);
        book.reconcile(&keys);
        let (travel, cell, valid) = book.snow_test_state(nine).unwrap();
        assert_eq!(travel, 0.0);
        assert_eq!(cell, BlockPos::ORIGIN);
        assert!(!valid);
        assert_eq!(book.pending_len(), 1);
        assert_eq!(
            book.pending_slice(0, book.pending_len())[0].pos,
            BlockPos::new(1, 1, 0)
        );
    }
}

// ---------------------------------------------------------------------
// Residents and admission
// ---------------------------------------------------------------------

/// Active passive keys in ascending stable-id order, the deterministic
/// collection shape the Go set maintains (`passiveSet`, `passive.go`).
fn resident_keys(ctx: &TickContext<'_>) -> Vec<ActorKey> {
    let mut keys: Vec<ActorKey> = ctx
        .read()
        .actors()
        .iter()
        .filter(|actor| {
            matches!(actor.key, ActorKey::Passive(_)) && actor.lifecycle == ActorLifecycle::Active
        })
        .map(|actor| actor.key)
        .collect();
    keys.sort();
    keys
}

/// Restores residents whose runtime lane is absent: a save body owns no
/// transient lane (`PassiveMob`, `mornlea_storage`), so the admission builds
/// the frozen zeroed defaults and re-anchors home at the loaded position,
/// exactly the transient discipline of `RestorePassive` (`passive.go`): no
/// restart can resurrect a flee or graze event.
fn admit_residents(
    ctx: &mut TickContext<'_>,
    report: &mut PhaseReport,
    mut snow: Option<&mut PassiveSnowBook>,
) -> Result<(), ServerError> {
    for key in resident_keys(ctx) {
        if ctx.read().runtime(key).is_some() {
            continue;
        }
        let Some(record) = ctx.read().actor(key).cloned() else {
            continue;
        };
        let position = record.motion.position().get();
        stage_runtime(
            ctx,
            key,
            ActorAux::Passive {
                home: block_of(position),
                flee_ticks: 0,
                flee_from: None,
                graze_ticks: 0,
                graze_at: None,
                fresh: false,
            },
        )?;
        // A runtime-less restore is a fresh admission: the same id cannot
        // inherit a stale tracker. The reset lands only after the runtime
        // staged successfully, and the pending coordinates always survive.
        if let Some(book) = snow.as_deref_mut()
            && let ActorKey::Passive(id) = key
        {
            book.forget(id);
        }
        report.applied += 1;
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Spawn admission (`advancePassiveSpawn`, `passive_spawn.go`)
// ---------------------------------------------------------------------

/// One candidate per tick, every necessary condition ordered cheap-first. A
/// cap, daytime or anchor short-circuit happens before any derivation and
/// stays silent exactly like the Go row; only a derived candidate that fails a
/// later gate counts as rejected.
fn advance_spawn(
    ctx: &mut TickContext<'_>,
    environment: &EnvironmentState,
    report: &mut PhaseReport,
    snow: Option<&mut PassiveSnowBook>,
) -> Result<(), ServerError> {
    let residents = resident_keys(ctx).len() as u64;
    let plan = {
        let view = ctx.read();
        plan_spawn(&view, environment, residents)
    };
    match plan {
        SpawnPlan::Silent => Ok(()),
        SpawnPlan::Rejected => {
            report.rejected += 1;
            Ok(())
        }
        SpawnPlan::Derived {
            id,
            dimension,
            candidate,
            home,
        } => insert_spawn(ctx, id, dimension, candidate, home, report, snow),
    }
}

/// One spawn attempt planned against a stable read view, then staged: no
/// candidate, a derived-but-refused candidate, or a derived candidate ready to
/// insert.
enum SpawnPlan {
    Silent,
    Rejected,
    Derived {
        id: mornlea_domain::PassiveId,
        dimension: Dimension,
        candidate: [f32; 3],
        home: BlockPos,
    },
}

/// The read-only half of `advancePassiveSpawn` (`passive_spawn.go`): daytime,
/// anchor set, global cap, anchor dimension, the shared candidate column, the
/// grass spot, the near limit, then the hashed id with its rehash budget.
fn plan_spawn(
    view: &AuthorityReadView<'_>,
    environment: &EnvironmentState,
    residents: u64,
) -> SpawnPlan {
    let now = environment.world_time;
    if !phase_is_day(effective_day_phase_at(
        now,
        environment.day_phase_offset,
        environment.season_offset,
    )) {
        return SpawnPlan::Silent;
    }
    let players = active_players(view);
    if players.is_empty() {
        return SpawnPlan::Silent;
    }
    // The global cap is the cheapest precondition after the anchor set: a full
    // world derives no candidate at all.
    if residents >= MAX_PASSIVES {
        return SpawnPlan::Silent;
    }
    let (_, anchor_dimension, anchor_position) = players[(now % players.len() as u64) as usize];
    if anchor_dimension != Dimension::OVERWORLD {
        // Protocol, storage and the passive spawn all require overworld-only
        // cattle (`passive_spawn.go`).
        return SpawnPlan::Rejected;
    }
    let anchor = block_of(anchor_position);
    let base = splitmix64((environment.seed as u64) ^ now);
    let (column_x, column_z) = spawn_column(base, anchor.x(), anchor.z());
    // The observation-backed spot scan stands in for the whole-chunk
    // readiness gate; an unstaged column yields no spot, so no spawn ever
    // invents ground or triggers loading.
    let Some(spot_y) = spawn_column_spot(view, anchor_dimension, column_x, column_z) else {
        return SpawnPlan::Rejected;
    };
    let candidate = [column_x as f32 + 0.5, spot_y as f32, column_z as f32 + 0.5];
    if near_limit_exceeded(view, anchor_dimension, candidate) {
        return SpawnPlan::Rejected;
    }
    // The id is the candidate hash (nonzero); a conflict rehashes along the
    // chain with a bounded budget and never truncates or overwrites the
    // resident set (`passive_spawn.go`).
    let mut id = candidate_hash(environment.seed, now, column_x, spot_y, column_z);
    for _ in 0..SPAWN_REHASH_BUDGET {
        if let Ok(candidate_id) = mornlea_domain::PassiveId::try_new(id)
            && view.actor(ActorKey::Passive(candidate_id)).is_none()
        {
            return SpawnPlan::Derived {
                id: candidate_id,
                dimension: anchor_dimension,
                candidate,
                home: BlockPos::new(column_x, spot_y, column_z),
            };
        }
        id = splitmix64(id);
    }
    SpawnPlan::Rejected
}

/// Stages the derived candidate with the `fresh` skip flag set: generation
/// happens at the tick boundary before physics, and the newborn integrates
/// from the next tick (`advancePassiveSpawn` fresh convention).
fn insert_spawn(
    ctx: &mut TickContext<'_>,
    id: mornlea_domain::PassiveId,
    dimension: Dimension,
    candidate: [f32; 3],
    home: BlockPos,
    report: &mut PhaseReport,
    snow: Option<&mut PassiveSnowBook>,
) -> Result<(), ServerError> {
    let record = ActorRecord::try_new(
        ActorKey::Passive(id),
        ActorLifecycle::Active,
        dimension,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(candidate).map_err(|_| ServerError::Internal {
                invariant: "passive spawn staging",
            })?,
            velocity: FiniteVec3::try_new([0.0; 3]).map_err(|_| ServerError::Internal {
                invariant: "passive spawn staging",
            })?,
            on_ground: true,
        }),
        LookAngles::try_new(0.0, 0.0).map_err(|_| ServerError::Internal {
            invariant: "passive spawn staging",
        })?,
        SurvivalState::try_new(SurvivalStateParts {
            health: MAX_HEALTH,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .map_err(|_| ServerError::Internal {
            invariant: "passive spawn staging",
        })?,
        ActorBody::Passive(PassiveMob {
            id: id.get(),
            dimension: i32::from(dimension.get()),
            position: candidate,
            velocity: [0.0; 3],
            on_ground: true,
            yaw: 0.0,
            health: MAX_HEALTH,
        }),
    )
    .map_err(|_| ServerError::Internal {
        invariant: "passive spawn staging",
    })?;
    ctx.stage(RuleEffect::Actor(record))
        .map_err(|_| ServerError::Internal {
            invariant: "passive spawn staging",
        })?;
    stage_runtime(
        ctx,
        ActorKey::Passive(id),
        ActorAux::Passive {
            home,
            flee_ticks: 0,
            flee_from: None,
            graze_ticks: 0,
            graze_at: None,
            fresh: true,
        },
    )?;
    // A newborn never inherits a retired same-id tracker; its pending
    // coordinates survive. The reset lands only after both stagings succeed.
    if let Some(book) = snow {
        book.forget(id);
    }
    report.applied += 2;
    report.examined += 1;
    Ok(())
}

/// Topmost-first scan for the first column cell with two air cells above a
/// grass support (`passiveSpawnColumnSpot`); the grass row additionally
/// carries a full collision box, satisfying the shared solid-support probe.
fn spawn_column_spot(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    x: i32,
    z: i32,
) -> Option<i32> {
    let mut y = MAX_Y - 2;
    while y > MIN_Y {
        let lower = view.observation(dimension, BlockPos::new(x, y, z));
        if !lower.is_some_and(|observed| observed.block == AIR) {
            y -= 1;
            continue;
        }
        let upper = view.observation(dimension, BlockPos::new(x, y + 1, z));
        if !upper.is_some_and(|observed| observed.block == AIR) {
            y -= 1;
            continue;
        }
        let support = view.observation(dimension, BlockPos::new(x, y - 1, z));
        if !support.is_some_and(|observed| observed.block == GRASS) {
            y -= 1;
            continue;
        }
        return Some(y);
    }
    None
}

/// Refuses the candidate when any active same-dimension player within the
/// 48-block radius already counts 6 passives nearby; all distances compare in
/// the squared domain (`passiveNearLimitExceeded`).
fn near_limit_exceeded(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    candidate: [f32; 3],
) -> bool {
    let near_sq = NEAR_RADIUS * NEAR_RADIUS;
    for (_, player_dimension, player_position) in active_players(view) {
        if player_dimension != dimension
            || horizontal_distance_sq(player_position, candidate) > near_sq
        {
            continue;
        }
        let mut count = 0u64;
        for actor in view.actors() {
            if !matches!(actor.key, ActorKey::Passive(_))
                || actor.lifecycle != ActorLifecycle::Active
                || actor.dimension != dimension
            {
                continue;
            }
            if horizontal_distance_sq(actor.motion.position().get(), player_position) <= near_sq {
                count += 1;
            }
        }
        if count >= NEAR_CAP {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------
// Movement (`advancePassiveMovement` and `passiveStepInput`, `passive.go`)
// ---------------------------------------------------------------------

/// One resident's live pose; the body yaw rides beside the physics state the
/// same way `passiveState` carries `yaw` next to `state`.
#[derive(Clone, Copy)]
struct Pose {
    position: [f32; 3],
    velocity: [f32; 3],
    on_ground: bool,
    yaw: f32,
}

/// The transient lanes the priority chain reads and mutates.
struct ChainLane {
    flee_ticks: u16,
    flee_from: Option<FiniteVec3>,
    graze_ticks: u16,
    yaw: f32,
}

/// One movement pass. The newborn clears its skip flag without integrating; a
/// submerged cow cancels the graze event like effective damage; a grazing cow
/// freezes; everyone else derives the chain input and advances once through
/// the F1 physics kernel, with the home-neighborhood rollback and the
/// deterministic fall-out removal (`advancePassiveMovement`).
fn advance_movement(
    ctx: &mut TickContext<'_>,
    key: ActorKey,
    environment: &EnvironmentState,
    report: &mut PhaseReport,
    snow: Option<&mut PassiveSnowBook>,
) -> Result<(), ServerError> {
    let Some(record) = ctx.read().actor(key).cloned() else {
        return Ok(());
    };
    let Some(runtime) = ctx.read().runtime(key).cloned() else {
        return Ok(());
    };
    let ActorAux::Passive {
        home,
        flee_ticks,
        flee_from,
        graze_ticks: runtime_graze_ticks,
        graze_at: runtime_graze_at,
        fresh,
    } = runtime.aux
    else {
        return Err(ServerError::Internal {
            invariant: "passive lifecycle aux",
        });
    };
    if fresh {
        stage_runtime(
            ctx,
            key,
            ActorAux::Passive {
                home,
                flee_ticks,
                flee_from,
                graze_ticks: runtime_graze_ticks,
                graze_at: runtime_graze_at,
                fresh: false,
            },
        )?;
        report.applied += 1;
        return Ok(());
    }
    let tuning = environment.tunables.physics();
    let pose = Pose {
        position: record.motion.position().get(),
        velocity: record.motion.velocity().get(),
        on_ground: record.motion.on_ground(),
        yaw: record.look.yaw(),
    };
    let (body_in_fluid, _eye_in_fluid) = {
        let view = ctx.read();
        submersion_flags(
            &view,
            record.dimension,
            pose.position,
            environment.tunables.eye_height(),
        )?
    };
    let mut graze_ticks = runtime_graze_ticks;
    let mut graze_at = runtime_graze_at;
    if body_in_fluid && graze_ticks > 0 {
        // Body submersion cancels the event like effective damage; only the
        // transient lane changes, no block is written, and the cow integrates
        // this same tick into the submerged escape (`advancePassiveMovement`).
        graze_ticks = 0;
        graze_at = None;
    }
    if graze_ticks > 0 {
        // Frozen with the head down: neutral input, no integration, no facing
        // update; effective damage and submersion have already cancelled.
        return Ok(());
    }
    let mut lane = ChainLane {
        flee_ticks,
        flee_from,
        graze_ticks,
        yaw: pose.yaw,
    };
    let (move_z, jump) = step_input(
        ctx,
        key,
        record.dimension,
        pose.position,
        &mut lane,
        body_in_fluid,
        environment,
    )?;
    let step = StepVector {
        move_x: 0,
        move_z,
        jump,
        sprinting: false,
        sneaking: false,
        yaw_sin: f64::from(lane.yaw).sin() as f32,
        yaw_cos: f64::from(lane.yaw).cos() as f32,
    };
    // Thick Snow under the foot cell consumes the shared snow tuning; the
    // sweep, prism and native step all run against the snow-adjusted copy.
    let tuning = apply_snow_slowdown(
        &ctx.read(),
        record.dimension,
        pose.position,
        pose.on_ground,
        [step.move_x, step.move_z],
        tuning,
    )?;
    let (sweep_min, sweep_max) =
        sweep_bounds(pose.velocity, pose.on_ground, step, body_in_fluid, tuning);
    let (origin, dimensions) = step_prism(pose.position, sweep_min, sweep_max, tuning.step_height)?;
    let cells = {
        let view = ctx.read();
        prism_cells(&view, record.dimension, origin, dimensions)?
    };
    let grid = CollisionGrid::try_new(origin, dimensions, &cells)
        .map_err(|_| ServerError::InvalidInput { field: "actor" })?;
    let stepped = NativePhysics
        .step(&PhysicsRequest {
            state: PhysicsState {
                position: pose.position,
                velocity: pose.velocity,
                on_ground: pose.on_ground,
            },
            controls: PhysicsControls {
                move_x: step.move_x,
                move_z: step.move_z,
                jump: step.jump,
                yaw_sin: step.yaw_sin,
                yaw_cos: step.yaw_cos,
                body_in_fluid,
                sprinting: false,
                sneaking: false,
            },
            tuning,
            sweep: SweepBounds {
                minimum: sweep_min,
                maximum: sweep_max,
            },
            grid,
        })
        .map_err(|_| ServerError::Internal {
            invariant: "passive motion step",
        })?;
    let next = stepped.state;
    let finite = next
        .position
        .iter()
        .chain(next.velocity.iter())
        .all(|component| component.is_finite());
    if !finite || next.position[1] < MIN_Y as f32 {
        // A cow that fell out of the world or state-distorted is removed
        // deterministically with no drops and no half-removed state
        // (`advancePassiveMovement` removal threshold). This quiet
        // termination is not death settlement, so it records the tick-local
        // quiet marker for the vanished reason after staging succeeds.
        stage_dead(ctx, &record)?;
        if let ActorKey::Passive(id) = key {
            ctx.record_passive_quiet_removal(id);
        }
        report.applied += 1;
        return Ok(());
    }
    let mut settled = Pose {
        position: next.position,
        velocity: next.velocity,
        on_ground: next.on_ground,
        yaw: lane.yaw,
    };
    if outside_home_neighborhood(home, settled.position) {
        // No path leads out of the birth neighborhood: roll back to the
        // step-start pose, the deterministic stop form (`passive.go`).
        settled = pose;
    }
    let changed = settled.position != pose.position
        || settled.velocity != pose.velocity
        || settled.on_ground != pose.on_ground
        || settled.yaw != pose.yaw
        || lane.flee_ticks != flee_ticks
        || graze_ticks != runtime_graze_ticks
        || graze_at != runtime_graze_at;
    if changed {
        stage_passive(
            ctx,
            &record,
            settled,
            ActorAux::Passive {
                home,
                flee_ticks: lane.flee_ticks,
                flee_from: lane.flee_from,
                graze_ticks,
                graze_at,
                fresh: false,
            },
        )?;
        report.applied += 1;
    }
    // Capture runs after the valid native step, the home rollback and the
    // staging, and before the batch's death settlement. Quiet invalid and
    // fall-out removals returned above, so every pose here is a live step.
    if let Some(book) = snow
        && let ActorKey::Passive(id) = key
    {
        capture_passive_snow(
            book,
            record.dimension,
            id,
            pose.position,
            settled.position,
            settled.on_ground,
        )?;
    }
    Ok(())
}

/// The frozen priority chain: submerged dry-turn, shoreline avoidance, flee,
/// graze, temptation, idle look, wander (`passiveStepInput`). Inputs stay pure
/// derivations over the staged world with no global randomness or map order.
fn step_input(
    ctx: &TickContext<'_>,
    key: ActorKey,
    dimension: Dimension,
    position: [f32; 3],
    lane: &mut ChainLane,
    body_in_fluid: bool,
    environment: &EnvironmentState,
) -> Result<(i8, bool), ServerError> {
    if body_in_fluid {
        // In water continuous lift plus a bounded turn toward the first dry
        // second-block probe; an all-wet probe window keeps the current
        // heading so the cow drifts toward shore instead of floating forever.
        if let Some((dx, dz)) = first_dry_direction(ctx, dimension, position) {
            lane.yaw = turn_yaw_toward(lane.yaw, yaw_toward(dx as f32, dz as f32), MAX_TURN);
        }
        return Ok((1, true));
    }
    let near_fluid = adjacent_fluid_flags(ctx, dimension, position);
    let stops = heading_enters_fluid(ctx, dimension, position, lane.yaw)
        || near_fluid
            .iter()
            .zip(DRY_PROBE_OFFSETS)
            .any(|(fluid, (ox, oz))| {
                let (forward_x, forward_z) = forward(lane.yaw);
                *fluid && forward_x * ox as f32 + forward_z * oz as f32 >= 0.0
            });
    if stops {
        return Ok(shore_input(ctx, dimension, position, lane, near_fluid));
    }
    if lane.flee_ticks > 0 {
        lane.flee_ticks -= 1;
        if let Some(from) = lane.flee_from {
            let origin = from.get();
            let dx = position[0] - origin[0];
            let dz = position[2] - origin[2];
            if dx != 0.0 || dz != 0.0 {
                lane.yaw = yaw_toward(dx, dz);
                return Ok((1, false));
            }
        }
    }
    if lane.graze_ticks > 0 {
        // Defensive neutral statement: movement is already frozen upstream.
        return Ok((0, false));
    }
    let view = ctx.read();
    if let Some(target) = tempt_target(&view, dimension, position) {
        let dx = target[0] - position[0];
        let dz = target[2] - position[2];
        let want = yaw_toward(dx, dz);
        if dx * dx + dz * dz > TEMPT_STOP_SQ {
            lane.yaw = turn_yaw_toward(lane.yaw, want, MAX_TURN);
            return Ok((1, false));
        }
        // The stop distance freezes displacement but never the facing.
        lane.yaw = turn_yaw_toward(lane.yaw, want, MAX_TURN);
        return Ok((0, false));
    }
    if let Some(target) = idle_target(&view, dimension, position) {
        let dx = target[0] - position[0];
        let dz = target[2] - position[2];
        if dx != 0.0 || dz != 0.0 {
            lane.yaw = turn_yaw_toward(lane.yaw, yaw_toward(dx, dz), MAX_TURN);
        }
        // Idle look never closes distance; only wheat does.
        return Ok((0, false));
    }
    let ActorKey::Passive(id) = key else {
        return Err(ServerError::Internal {
            invariant: "passive lifecycle aux",
        });
    };
    let segment = ctx.read().tick() / WANDER_SEGMENT_TICKS;
    let base = splitmix64((environment.seed as u64) ^ segment ^ id.get());
    let want = normalize_yaw(
        (base & 0xFF_FFFF) as f32 * ((2.0 * std::f64::consts::PI / 16_777_216.0) as f32),
    );
    lane.yaw = turn_yaw_toward(lane.yaw, want, MAX_TURN);
    Ok((1, false))
}

// -- Temptation and idle look (`passive_tempt.go`, `passive.go`).

/// Nearest same-dimension active player holding wheat in the authoritative
/// selected hotbar slot within 8 blocks inclusive; equal distance keeps the
/// smaller session id because the scan walks ascending sessions and only a
/// strictly nearer player replaces the best (`passiveTemptTarget`).
fn tempt_target(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    position: [f32; 3],
) -> Option<[f32; 3]> {
    let radius_sq = TEMPT_RADIUS * TEMPT_RADIUS;
    let mut best = None;
    let mut best_sq = radius_sq;
    for (session, player_dimension, player_position) in active_players(view) {
        if player_dimension != dimension {
            continue;
        }
        let Some(inventory) = view.inventory(ActorKey::Player(session)) else {
            continue;
        };
        if inventory.slots[usize::from(inventory.selected.get())].item != ITEM_WHEAT {
            continue;
        }
        let dist_sq = horizontal_distance_sq(player_position, position);
        if dist_sq > radius_sq || (best.is_some() && dist_sq >= best_sq) {
            continue;
        }
        best_sq = dist_sq;
        best = Some(player_position);
    }
    best
}

/// Same scan without the wheat lane and inside the 6-block idle radius
/// (`passiveIdleLookTarget`); the chain order keeps flee, graze and temptation
/// ahead of it.
fn idle_target(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    position: [f32; 3],
) -> Option<[f32; 3]> {
    let radius_sq = IDLE_LOOK_RADIUS * IDLE_LOOK_RADIUS;
    let mut best = None;
    let mut best_sq = radius_sq;
    for (_, player_dimension, player_position) in active_players(view) {
        if player_dimension != dimension {
            continue;
        }
        let dist_sq = horizontal_distance_sq(player_position, position);
        if dist_sq > radius_sq || (best.is_some() && dist_sq >= best_sq) {
            continue;
        }
        best_sq = dist_sq;
        best = Some(player_position);
    }
    best
}

/// Active players in ascending session order, the deterministic scan shape of
/// `sortedActiveSessions`.
fn active_players(view: &AuthorityReadView<'_>) -> Vec<(SessionKey, Dimension, [f32; 3])> {
    let mut players: Vec<(SessionKey, Dimension, [f32; 3])> = view
        .online_players()
        .filter_map(|actor| match actor.key {
            ActorKey::Player(session) => {
                Some((session, actor.dimension, actor.motion.position().get()))
            }
            _ => None,
        })
        .collect();
    players.sort_by_key(|(session, _, _)| *session);
    players
}

// -- Water safety probes (`passive.go`, cited per row).

/// First dry second-block probe in the fixed offset order; unready cells count
/// as dry so the authority neither invents water nor triggers loading
/// (`passiveFirstDryDirection`).
fn first_dry_direction(
    ctx: &TickContext<'_>,
    dimension: Dimension,
    position: [f32; 3],
) -> Option<(i32, i32)> {
    let foot = block_of(position);
    for (ox, oz) in DRY_PROBE_OFFSETS {
        let probe = BlockPos::new(
            foot.x() + DRY_PROBE_OFFSET_BLOCKS * ox,
            foot.y(),
            foot.z() + DRY_PROBE_OFFSET_BLOCKS * oz,
        );
        if let Some(observed) = ctx.read().observation(dimension, probe)
            && is_fluid(observed.block)
        {
            continue;
        }
        return Some((ox, oz));
    }
    None
}

/// Adjacent first-block fluid flags in the probe order; unready counts as dry
/// (`passiveAdjacentFluidFlags`).
fn adjacent_fluid_flags(
    ctx: &TickContext<'_>,
    dimension: Dimension,
    position: [f32; 3],
) -> [bool; 4] {
    let foot = block_of(position);
    let mut flags = [false; 4];
    for (index, (ox, oz)) in DRY_PROBE_OFFSETS.iter().enumerate() {
        let probe = BlockPos::new(foot.x() + ox, foot.y(), foot.z() + oz);
        flags[index] = ctx
            .read()
            .observation(dimension, probe)
            .is_some_and(|observed| is_fluid(observed.block));
    }
    flags
}

/// First ordered direction whose adjacent and second blocks are both dry
/// (`passiveWalkableDirection`); far probes only run for dry near points.
fn walkable_direction(
    ctx: &TickContext<'_>,
    dimension: Dimension,
    position: [f32; 3],
    near_fluid: [bool; 4],
) -> Option<(i32, i32)> {
    let foot = block_of(position);
    for (index, (ox, oz)) in DRY_PROBE_OFFSETS.iter().enumerate() {
        if near_fluid[index] {
            continue;
        }
        let probe = BlockPos::new(
            foot.x() + DRY_PROBE_OFFSET_BLOCKS * ox,
            foot.y(),
            foot.z() + DRY_PROBE_OFFSET_BLOCKS * oz,
        );
        if let Some(observed) = ctx.read().observation(dimension, probe)
            && is_fluid(observed.block)
        {
            continue;
        }
        return Some((*ox, *oz));
    }
    None
}

/// One read on the foot block about 1.5 blocks ahead; unready counts as dry
/// (`passiveHeadingEntersFluid`).
fn heading_enters_fluid(
    ctx: &TickContext<'_>,
    dimension: Dimension,
    position: [f32; 3],
    yaw: f32,
) -> bool {
    let (forward_x, forward_z) = forward(yaw);
    let probe = BlockPos::new(
        (position[0] + WATER_LOOKAHEAD * forward_x).floor() as i32,
        position[1].floor() as i32,
        (position[2] + WATER_LOOKAHEAD * forward_z).floor() as i32,
    );
    ctx.read()
        .observation(dimension, probe)
        .is_some_and(|observed| is_fluid(observed.block))
}

/// Shore avoidance input: turn toward the first direction whose near and far
/// probes are both dry, advance while the candidate is in the closed forward
/// half-plane, turn in place otherwise, and reverse only when every direction
/// fails (`passiveShoreInput`).
fn shore_input(
    ctx: &TickContext<'_>,
    dimension: Dimension,
    position: [f32; 3],
    lane: &mut ChainLane,
    near_fluid: [bool; 4],
) -> (i8, bool) {
    if let Some((dx, dz)) = walkable_direction(ctx, dimension, position, near_fluid) {
        let want = yaw_toward(dx as f32, dz as f32);
        let (forward_x, forward_z) = forward(lane.yaw);
        let advancing = forward_x * dx as f32 + forward_z * dz as f32 >= 0.0;
        lane.yaw = turn_yaw_toward(lane.yaw, want, MAX_TURN);
        return if advancing { (1, false) } else { (0, false) };
    }
    lane.yaw = normalize_yaw(lane.yaw + std::f32::consts::PI);
    (1, false)
}

// ---------------------------------------------------------------------
// Grazing (`advancePassiveGrazeOne`, `passive_graze.go`)
// ---------------------------------------------------------------------

/// The support cell under the feet: floor of X/Z with the Y one ground probe
/// below, the same geometry the trample collectors use so grazing reads the
/// same standing block physics settles on (`passiveGrazeSupport`).
fn graze_support(position: [f32; 3]) -> BlockPos {
    BlockPos::new(
        position[0].floor() as i32,
        (position[1] - GROUND_PROBE).floor() as i32,
        position[2].floor() as i32,
    )
}

/// One graze pass: trigger under the frozen 1-in-600 roll, then the in-event
/// path on the same tick; aborts clear the transient without writing, and the
/// twentieth tick settles the trigger cell through the system transaction
/// (`advancePassiveGrazeOne`, `tryStartPassiveGraze`, `settlePassiveGraze`).
fn advance_graze(
    ctx: &mut TickContext<'_>,
    key: ActorKey,
    environment: &EnvironmentState,
    report: &mut PhaseReport,
) -> Result<(), ServerError> {
    let ActorKey::Passive(id) = key else {
        return Ok(());
    };
    let Some(record) = ctx.read().actor(key).cloned() else {
        return Ok(());
    };
    let Some(runtime) = ctx.read().runtime(key).cloned() else {
        return Ok(());
    };
    let ActorAux::Passive {
        home,
        flee_ticks,
        flee_from,
        graze_ticks,
        graze_at,
        fresh,
    } = runtime.aux
    else {
        return Err(ServerError::Internal {
            invariant: "passive lifecycle aux",
        });
    };
    let position = record.motion.position().get();
    let mut graze_ticks = graze_ticks;
    let mut graze_at = graze_at;
    if graze_ticks == 0 {
        // Cheap-first trigger gates: no flee, then the deterministic roll,
        // then standing grass on loaded ground.
        if flee_ticks > 0 {
            return Ok(());
        }
        if !graze_hit(environment.seed, ctx.read().tick(), id.get()) {
            return Ok(());
        }
        let support = graze_support(position);
        match ctx.read().observation(record.dimension, support) {
            Some(observed) if observed.block == GRASS => {}
            _ => return Ok(()),
        }
        graze_at = Some(support);
        graze_ticks = GRAZE_DURATION_TICKS;
    }
    // In-event path, entered on the trigger tick too. Abort checks run before
    // the countdown: a fleeing cow, a moved support, or a trigger cell that is
    // no longer standing grass on ready ground ends the event with no write.
    let aborted = match graze_at {
        // A live event always names its trigger cell; the checks mirror
        // `advancePassiveGrazeOne`: flee, support change, cell no longer
        // standing grass on ready ground.
        Some(cell) => {
            flee_ticks > 0
                || graze_support(position) != cell
                || !ctx
                    .read()
                    .observation(record.dimension, cell)
                    .is_some_and(|observed| observed.block == GRASS)
        }
        None => true,
    };
    if aborted {
        if graze_at.is_some() {
            stage_runtime(
                ctx,
                key,
                ActorAux::Passive {
                    home,
                    flee_ticks,
                    flee_from,
                    graze_ticks: 0,
                    graze_at: None,
                    fresh,
                },
            )?;
            report.applied += 1;
        }
        return Ok(());
    }
    graze_ticks -= 1;
    if graze_ticks == 0 {
        // Settlement: the cell is re-verified as standing grass and the write
        // commits through the accepted system transaction; a refused
        // transaction discards the settlement without writing, the idempotent
        // drop guard of `settlePassiveGraze`.
        let settled = match graze_at {
            Some(cell) => match ctx.read().observation(record.dimension, cell) {
                Some(observed) if observed.block == GRASS => BlockWrite::try_new(observed, DIRT)
                    .is_ok_and(|write| {
                        ctx.transaction()
                            .try_system(SystemRule::PassiveGraze(id), vec![write])
                            .is_ok()
                    }),
                _ => false,
            },
            None => false,
        };
        if settled {
            report.applied += 1;
        }
        graze_at = None;
    }
    stage_runtime(
        ctx,
        key,
        ActorAux::Passive {
            home,
            flee_ticks,
            flee_from,
            graze_ticks,
            graze_at,
            fresh,
        },
    )?;
    report.applied += 1;
    Ok(())
}

// ---------------------------------------------------------------------
// Death settlement (`settlePassiveDeaths`, `passive.go`)
// ---------------------------------------------------------------------

/// Terminates every resident whose health reached zero alongside its fixed
/// loot (`settlePassiveDeaths` with `dropPassiveLoot`). The lifecycle is
/// terminal: a dead record is excluded from every later pass, so a removed
/// cow can never resurrect late. The loot stages in the same `Compound` as
/// the `Dead` record; an all-full ring or an unlocatable death stays lootless
/// with the death still completing, exactly like the movement fall-out path.
fn settle_deaths(
    ctx: &mut TickContext<'_>,
    report: &mut PhaseReport,
    mut snow: Option<&mut PassiveSnowBook>,
) -> Result<(), ServerError> {
    for key in resident_keys(ctx) {
        let Some(record) = ctx.read().actor(key).cloned() else {
            continue;
        };
        if record.survival.health() != 0 {
            continue;
        }
        settle_dead_with_loot(ctx, &record)?;
        if let Some(book) = snow.as_deref_mut()
            && let ActorKey::Passive(id) = record.key
        {
            // Only the tracker slot clears; the coordinates this resident
            // queued earlier in the tick still settle after death.
            book.forget(id);
        }
        report.applied += 1;
    }
    Ok(())
}

/// Stages one terminal record with its fixed beef batch. The first ready ring
/// chunk whose rehearsal succeeds takes the batch beside the `Dead` staging;
/// lootless deaths reuse the plain termination below, so fall-out
/// (`advancePassiveMovement` removal threshold) and death settlement share one
/// terminal shape.
fn settle_dead_with_loot(
    ctx: &mut TickContext<'_>,
    record: &ActorRecord,
) -> Result<(), ServerError> {
    if let Some(batch) = rehearse_beef(ctx, record) {
        let dead = dead_record(record)?;
        ctx.stage(RuleEffect::Compound(vec![
            RuleEffect::Actor(dead),
            RuleEffect::Drops(batch),
        ]))
        .map_err(|_| ServerError::Internal {
            invariant: "passive death staging",
        })?;
        return Ok(());
    }
    stage_dead(ctx, record)
}

/// First-fit rehearsal of the fixed beef batch over the death-dimension ready
/// chunks in ring order (`dropPassiveLoot` over `deathDropChunks`): the death
/// chunk first, then outward rings. `None` skips the rehearsal (invalid pose)
/// or omits the loot (below-floor column, all-full ring) with the death still
/// completing.
fn rehearse_beef(ctx: &TickContext<'_>, record: &ActorRecord) -> Option<DropBatch> {
    let view = ctx.read();
    let delay = view
        .environment()
        .map(|environment| environment.tunables.drop_pickup_delay_ticks())?;
    let tick = view.tick();
    let block = death_block(record.motion.position().get())?;
    let center = ChunkPos::new(block.x() >> 4, block.z() >> 4);
    let mut candidates: Vec<ChunkKey> = view
        .ready_chunk_keys()
        .into_iter()
        .filter(|key| key.dimension == record.dimension)
        .collect();
    candidates.sort_by_key(|key| ring_order_key(center, key.pos));
    for key in candidates {
        let origin = death_origin(block, key.pos)?;
        let Ok(batch) = DropBatch::try_new(
            DropSource::Death {
                actor: record.key,
                tick,
            },
            record.dimension,
            origin,
            vec![ItemStack {
                item: ITEM_RAW_BEEF,
                count: 1,
                durability: 0,
            }],
            delay,
        ) else {
            continue;
        };
        if view.check_drop_batch(&batch).is_ok() {
            return Some(batch);
        }
    }
    None
}

/// Foot block of a death pose (`blockPosOf`,
/// `packages/server/sim/entity/hostile.go`): each component floors in float64
/// before narrowing, so an out-of-span pose refuses instead of saturating
/// silently through the cast. Mirrors the hostile death rule one dimension at
/// a time.
fn death_block(position: [f32; 3]) -> Option<BlockPos> {
    let mut block = [0i32; 3];
    for (index, value) in position.iter().enumerate() {
        let floored = f64::from(*value).floor();
        if !floored.is_finite() || floored < f64::from(i32::MIN) || floored > f64::from(i32::MAX) {
            return None;
        }
        block[index] = floored as i32;
    }
    Some(BlockPos::new(block[0], block[1], block[2]))
}

/// Sort key placing nearer rings first with (`x`, `z`) breaking ties inside
/// one ring (`deathDropChunks` over `sortChunkKeys`,
/// `packages/server/sim/entity/death.go`). Mirrors the hostile death rule.
fn ring_order_key(center: ChunkPos, pos: ChunkPos) -> (i64, i32, i32) {
    let dx = i64::from(pos.x()) - i64::from(center.x());
    let dz = i64::from(pos.z()) - i64::from(center.z());
    (dx.abs().max(dz.abs()), pos.x(), pos.z())
}

/// Nearest column of a death block inside one candidate chunk
/// (`clampBlockToChunk`, `packages/server/sim/entity/death.go`): overflow
/// lands on the side facing the death point. Mirrors the hostile death rule.
fn clamp_block_to_chunk(block: BlockPos, chunk: ChunkPos) -> BlockPos {
    let min_x = chunk.x().wrapping_shl(4);
    let min_z = chunk.z().wrapping_shl(4);
    BlockPos::new(
        block.x().clamp(min_x, min_x + 15),
        block.y(),
        block.z().clamp(min_z, min_z + 15),
    )
}

/// Rehearsal origin steering one placement at the clamped column: the block
/// center floors back onto the intended chunk cell through the shared drop
/// location rule. Mirrors the hostile death rule.
fn death_origin(block: BlockPos, chunk: ChunkPos) -> Option<FiniteVec3> {
    let clamped = clamp_block_to_chunk(block, chunk);
    FiniteVec3::try_new([
        clamped.x() as f32 + 0.5,
        clamped.y() as f32 + 0.5,
        clamped.z() as f32 + 0.5,
    ])
    .ok()
}

/// Stages the terminal record for one resident.
fn stage_dead(ctx: &mut TickContext<'_>, record: &ActorRecord) -> Result<(), ServerError> {
    let dead = dead_record(record)?;
    ctx.stage(RuleEffect::Actor(dead))
        .map_err(|_| ServerError::Internal {
            invariant: "passive lifecycle staging",
        })?;
    Ok(())
}

/// Builds the terminal record for one resident without staging it, so the
/// loot path can share the exact termination the lootless paths stage.
fn dead_record(record: &ActorRecord) -> Result<ActorRecord, ServerError> {
    let mut body = match &record.body {
        ActorBody::Passive(body) => body.clone(),
        _ => {
            return Err(ServerError::Internal {
                invariant: "passive lifecycle body",
            });
        }
    };
    body.health = 0;
    ActorRecord::try_new(
        record.key,
        ActorLifecycle::Dead,
        record.dimension,
        record.motion,
        record.look,
        record.survival,
        ActorBody::Passive(body),
    )
    .map_err(|_| ServerError::Internal {
        invariant: "passive lifecycle staging",
    })
}

// ---------------------------------------------------------------------
// Staging helpers
// ---------------------------------------------------------------------

/// Stages the passive runtime lane this provider owns with zeroed bookkeeping
/// lanes.
fn stage_runtime(
    ctx: &mut TickContext<'_>,
    key: ActorKey,
    aux: ActorAux,
) -> Result<(), ServerError> {
    ctx.stage(RuleEffect::Runtime(ActorRuntime {
        key,
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 0,
        peak_y: 0.0,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux,
    }))
    .map_err(|_| ServerError::Internal {
        invariant: "passive lifecycle staging",
    })
}

/// Stages the advanced pose on both lanes the passive owns: the actor record
/// (motion, facing and the mirrored save body) and the runtime aux.
fn stage_passive(
    ctx: &mut TickContext<'_>,
    record: &ActorRecord,
    pose: Pose,
    aux: ActorAux,
) -> Result<(), ServerError> {
    let mut body = match &record.body {
        ActorBody::Passive(body) => body.clone(),
        _ => {
            return Err(ServerError::Internal {
                invariant: "passive lifecycle body",
            });
        }
    };
    body.position = pose.position;
    body.velocity = pose.velocity;
    body.on_ground = pose.on_ground;
    body.yaw = pose.yaw;
    let advanced = ActorRecord::try_new(
        record.key,
        record.lifecycle,
        record.dimension,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(pose.position).map_err(|_| ServerError::Internal {
                invariant: "passive lifecycle staging",
            })?,
            velocity: FiniteVec3::try_new(pose.velocity).map_err(|_| ServerError::Internal {
                invariant: "passive lifecycle staging",
            })?,
            on_ground: pose.on_ground,
        }),
        LookAngles::try_new(pose.yaw, 0.0).map_err(|_| ServerError::Internal {
            invariant: "passive lifecycle staging",
        })?,
        record.survival,
        ActorBody::Passive(body),
    )
    .map_err(|_| ServerError::Internal {
        invariant: "passive lifecycle staging",
    })?;
    ctx.stage(RuleEffect::Actor(advanced))
        .map_err(|_| ServerError::Internal {
            invariant: "passive lifecycle staging",
        })?;
    stage_runtime(ctx, record.key, aux)
}

// ---------------------------------------------------------------------
// Pure mirrors, each citing its Go row.
// ---------------------------------------------------------------------

/// `Sampler.SplitMix64` (`packages/server/updates/sampler.go`), the shared
/// integer hash primitive of every random face.
fn splitmix64(x: u64) -> u64 {
    let mut x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// `Sampler.PassiveGrazeHit`: a pure integer hash over (world seed, tick, id)
/// against the 1-in-600 denominator; stateless, so an aborted event can
/// re-open on the next hit with no appetite state.
fn graze_hit(seed: i64, tick: u64, id: u64) -> bool {
    let hash = splitmix64(splitmix64((seed as u64) ^ GRAZE_SALT) ^ tick);
    splitmix64(hash ^ id).is_multiple_of(GRAZE_PERIOD)
}

/// `Sampler.HostileCandidateHash`: the passive spawn reuses the candidate hash
/// chain wholesale; the full hash is the id with no hostile-side threshold.
fn candidate_hash(seed: i64, tick: u64, x: i32, y: i32, z: i32) -> u64 {
    let hash = splitmix64(splitmix64((seed as u64) ^ tick) ^ (x as u32 as u64) ^ (z as u32 as u64));
    splitmix64(hash ^ (y as u32 as u64))
}

/// `hostileSpawnColumn` (`packages/server/sim/entity/hostile_spawn.go`): the
/// integer-derived radius 24..48 and axis the passive window shares.
fn spawn_column(base: u64, anchor_x: i32, anchor_z: i32) -> (i32, i32) {
    let radius = SPAWN_MIN_RADIUS + (base % 25) as i64;
    let axis = ((base >> 32) & 3) as usize;
    let deltas = [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)];
    let (dx, dz) = deltas[axis];
    (
        anchor_x + (dx * radius) as i32,
        anchor_z + (dz * radius) as i32,
    )
}

/// Display phase of the absolute clock plus the display offset
/// (`DisplayDayPhase`, `packages/shared/core/day_phase.go`): modulo before the
/// add so any absolute time stays overflow-free.
fn display_day_phase(world_time: u64, offset: u16) -> u16 {
    ((world_time % 24000 + u64::from(offset)) % 24000) as u16
}

/// Day-arc ratio of a year phase (`DayFractionAt`): 0.5 + 0.15*sin(2pi*p).
fn day_fraction_at(year_phase: f64) -> f64 {
    0.5 + 0.15 * (2.0 * std::f64::consts::PI * year_phase).sin()
}

/// Day arc in ticks (`DayArcTicks`): rounded, then forced even so the warp
/// divisions stay symmetric across both branches.
fn day_arc_ticks(year_phase: f64) -> u16 {
    let mut ticks = (day_fraction_at(year_phase) * 24000.0).round() as i64;
    if ticks % 2 == 1 {
        ticks -= 1;
    }
    ticks as u16
}

/// Seasonal display phase (`EffectiveDayPhase`): the day arc maps linearly
/// onto the half period; an invalid arc returns zero by contract.
fn effective_day_phase(world_time: u64, offset: u16, day_arc: u16) -> u16 {
    if day_arc == 0 || day_arc > 24000 || !day_arc.is_multiple_of(2) {
        return 0;
    }
    let phase = u32::from(display_day_phase(world_time, offset));
    let arc = u32::from(day_arc);
    if phase < arc {
        (phase * 12000 / arc) as u16
    } else {
        (12000 + (phase - arc) * 12000 / (24000 - arc)) as u16
    }
}

/// The only day-phase path (`EffectiveDayPhaseAt`): season offset to year
/// phase to day arc to the warped phase. Consumers must not assemble their own
/// warp.
fn effective_day_phase_at(world_time: u64, offset: u16, season_offset: u32) -> u16 {
    effective_day_phase(
        world_time,
        offset,
        day_arc_ticks(year_phase_at(world_time, season_offset)),
    )
}

/// Daytime window for spawn admission (`phaseIsDay`,
/// `packages/server/sim/entity/hostile.go`).
fn phase_is_day(phase: u16) -> bool {
    (1..=11999).contains(&phase)
}

/// Foot block of a body position (`blockPosOf`, `packages/server/sim/entity/hostile.go`).
fn block_of(position: [f32; 3]) -> BlockPos {
    BlockPos::new(
        position[0].floor() as i32,
        position[1].floor() as i32,
        position[2].floor() as i32,
    )
}

/// Squared horizontal distance (`horizontalDistanceSq`), always in the squared
/// domain so no sqrt enters a comparison.
fn horizontal_distance_sq(from: [f32; 3], to: [f32; 3]) -> f32 {
    let dx = to[0] - from[0];
    let dz = to[2] - from[2];
    dx * dx + dz * dz
}

/// Facing yaw toward a world offset: `atan2(-dx, -dz)` computed in float64 and
/// narrowed exactly like the Go rows.
fn yaw_toward(dx: f32, dz: f32) -> f32 {
    normalize_yaw((f64::from(-dx)).atan2(f64::from(-dz)) as f32)
}

/// Facing unit vector with the shared world-axis conversion
/// (`passiveForward`): `dx = -sin(yaw)`, `dz = -cos(yaw)`.
fn forward(yaw: f32) -> (f32, f32) {
    (
        -(f64::from(yaw).sin() as f32),
        -(f64::from(yaw).cos() as f32),
    )
}

/// Yaw normalization (`normalizeYaw`,
/// `packages/server/sim/entity/placement.go`): float64 remainder into
/// [-pi, pi).
fn normalize_yaw(yaw: f32) -> f32 {
    let mut normalized = (f64::from(yaw) + std::f64::consts::PI) % (2.0 * std::f64::consts::PI);
    if normalized < 0.0 {
        normalized += 2.0 * std::f64::consts::PI;
    }
    (normalized - std::f64::consts::PI) as f32
}

/// Bounded shortest-arc turn (`turnYawToward`, `passive.go`): land exactly
/// when the step reaches the target, never overshoot, never wrap the long way.
fn turn_yaw_toward(current: f32, want: f32, max_step: f32) -> f32 {
    let delta = normalize_yaw(want - current);
    if delta > max_step {
        return normalize_yaw(current + max_step);
    }
    if delta < -max_step {
        return normalize_yaw(current - max_step);
    }
    want
}

/// Outside the birth-chunk neighborhood when the Chebyshev chunk distance
/// exceeds one (`outsideHomeNeighborhood`, `passive.go`).
fn outside_home_neighborhood(home: BlockPos, position: [f32; 3]) -> bool {
    let current_block = block_of(position);
    let dx = (i64::from(current_block.x() >> 4) - i64::from(home.x() >> 4)).abs();
    let dz = (i64::from(current_block.z() >> 4) - i64::from(home.z() >> 4)).abs();
    dx > 1 || dz > 1
}

fn is_fluid(block: u16) -> bool {
    (FLUID_FIRST..=FLUID_LAST).contains(&block)
}

// -- Kernel-facing mirrors (`packages/shared/physics/step.go`,
// `collision.go`, `submersion.go`), identical in shape to the accepted
// player-motion plumbing because the Go passive advances through the same
// `StepWithTunables`.

/// One step's resolved intent (`StepWithTunables` input projection).
#[derive(Clone, Copy)]
struct StepVector {
    move_x: i8,
    move_z: i8,
    jump: bool,
    sprinting: bool,
    sneaking: bool,
    yaw_sin: f32,
    yaw_cos: f32,
}

/// Body immersion mirror (`SubmersionFlagsWithTunables`): the feet-centered
/// box over staged blocks with the snapshot eye height; unobserved cells read
/// as non-fluid, never invented.
fn submersion_flags(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    position: [f32; 3],
    eye_height: f32,
) -> Result<(bool, bool), ServerError> {
    let eye_in_fluid = is_fluid_at(
        view,
        dimension,
        BlockPos::new(
            checked_floor(position[0])?,
            checked_floor(position[1] + eye_height)?,
            checked_floor(position[2])?,
        ),
    );
    let minimum = [
        checked_floor(position[0] - HALF_WIDTH)?,
        checked_floor(position[1])?,
        checked_floor(position[2] - HALF_WIDTH)?,
    ];
    let maximum = [
        fluid_upper(position[0] + HALF_WIDTH, minimum[0])?,
        fluid_upper(position[1] + PLAYER_HEIGHT, minimum[1])?,
        fluid_upper(position[2] + HALF_WIDTH, minimum[2])?,
    ];
    for y in minimum[1]..=maximum[1] {
        for x in minimum[0]..=maximum[0] {
            for z in minimum[2]..=maximum[2] {
                if is_fluid_at(view, dimension, BlockPos::new(x, y, z)) {
                    return Ok((true, eye_in_fluid));
                }
            }
        }
    }
    Ok((false, eye_in_fluid))
}

fn is_fluid_at(view: &AuthorityReadView<'_>, dimension: Dimension, pos: BlockPos) -> bool {
    view.observation(dimension, pos)
        .is_some_and(|observed| is_fluid(observed.block))
}

/// AABB upper-cell mirror (`fluidCellUpperBound`): ceil minus one so a box
/// merely touching a boundary claims no neighbor, clamped to at least one cell.
fn fluid_upper(maximum: f32, lower: i32) -> Result<i32, ServerError> {
    Ok((checked_ceil(maximum)? - 1).max(lower))
}

/// int32 span of the checked floor/ceil domain (`collisionCheckedFloor`).
const COORD_MIN: f64 = i32::MIN as f64;
const COORD_MAX: f64 = i32::MAX as f64;

fn checked_floor(value: f32) -> Result<i32, ServerError> {
    let floored = f64::from(value).floor();
    if !floored.is_finite() || !(COORD_MIN..=COORD_MAX).contains(&floored) {
        return Err(ServerError::InvalidInput { field: "actor" });
    }
    Ok(floored as i32)
}

fn checked_ceil(value: f32) -> Result<i32, ServerError> {
    let ceiling = f64::from(value).ceil();
    if !ceiling.is_finite() || !(COORD_MIN..=COORD_MAX).contains(&ceiling) {
        return Err(ServerError::InvalidInput { field: "actor" });
    }
    Ok(ceiling as i32)
}

/// Sweep hull mirror (`stepSweepBounds`): the integrated displacement's convex
/// bound with the exact Go operation order, including the fused vector length
/// and the two-stage target scaling.
fn sweep_bounds(
    velocity: [f32; 3],
    on_ground: bool,
    step: StepVector,
    body_in_fluid: bool,
    tuning: PhysicsTuning,
) -> ([f32; 3], [f32; 3]) {
    let dt = tuning.fixed_delta_seconds;
    let mut walk_speed = tuning.walk_speed;
    if step.sneaking && on_ground && !body_in_fluid {
        walk_speed *= tuning.sneak_speed_multiplier;
    } else if step.sprinting && step.move_z > 0 && on_ground && !body_in_fluid {
        walk_speed *= tuning.sprint_speed_multiplier;
    }
    let target = movement_target(
        step.move_x,
        step.move_z,
        walk_speed,
        step.yaw_sin,
        step.yaw_cos,
    );
    let mut horizontal = [velocity[0], 0.0, velocity[2]];
    if on_ground {
        if step_vector_length(target) == 0.0 {
            horizontal = move_toward(horizontal, [0.0; 3], tuning.ground_deceleration * dt);
        } else {
            horizontal = move_toward(horizontal, target, tuning.ground_acceleration * dt);
        }
    } else {
        horizontal = move_toward(horizontal, target, tuning.air_acceleration * dt);
        let length = step_vector_length(horizontal);
        if length > tuning.walk_speed {
            let inverse = 1.0 / length;
            horizontal = [
                horizontal[0] * inverse * tuning.walk_speed,
                horizontal[1] * inverse * tuning.walk_speed,
                horizontal[2] * inverse * tuning.walk_speed,
            ];
        }
    }
    if body_in_fluid {
        horizontal = [
            horizontal[0] * tuning.fluid_horizontal_drag,
            horizontal[1] * tuning.fluid_horizontal_drag,
            horizontal[2] * tuning.fluid_horizontal_drag,
        ];
    }
    let (tx, tz) = (horizontal[0], horizontal[2]);
    let minimum = [
        min3(0.0, velocity[0], tx) * dt,
        sweep_vertical_min(velocity[1], step.jump, on_ground, body_in_fluid, tuning, dt),
        min3(0.0, velocity[2], tz) * dt,
    ];
    let maximum = [
        max3(0.0, velocity[0], tx) * dt,
        sweep_vertical_max(velocity[1], step.jump, on_ground, body_in_fluid, tuning, dt),
        max3(0.0, velocity[2], tz) * dt,
    ];
    (minimum, maximum)
}

fn sweep_vertical_min(
    velocity_y: f32,
    jump: bool,
    on_ground: bool,
    body_in_fluid: bool,
    tuning: PhysicsTuning,
    dt: f32,
) -> f32 {
    if body_in_fluid && jump {
        return 0.0f32.min(tuning.fluid_ascend_speed * dt);
    }
    if on_ground && jump {
        return 0.0;
    }
    let (gravity, terminal) = if body_in_fluid {
        (tuning.fluid_gravity, tuning.fluid_sink_speed)
    } else {
        (tuning.gravity, tuning.terminal_fall_speed)
    };
    let fallen = velocity_y - gravity * dt;
    if fallen >= -terminal {
        min3(0.0, velocity_y, fallen) * dt
    } else {
        min3(0.0, velocity_y, -terminal) * dt
    }
}

fn sweep_vertical_max(
    velocity_y: f32,
    jump: bool,
    on_ground: bool,
    body_in_fluid: bool,
    tuning: PhysicsTuning,
    dt: f32,
) -> f32 {
    if body_in_fluid && jump {
        return 0.0f32.max(tuning.fluid_ascend_speed * dt);
    }
    if on_ground && jump {
        return tuning.jump_speed * dt;
    }
    let (gravity, terminal) = if body_in_fluid {
        (tuning.fluid_gravity, tuning.fluid_sink_speed)
    } else {
        (tuning.gravity, tuning.terminal_fall_speed)
    };
    let fallen = velocity_y - gravity * dt;
    if fallen >= -terminal {
        max3(0.0, velocity_y, fallen) * dt
    } else {
        max3(0.0, velocity_y, -terminal) * dt
    }
}

/// Yaw-relative movement target (`movementTargetFromYaw`) with the two-stage
/// normalization the Go row performs.
fn movement_target(
    move_x: i8,
    move_z: i8,
    walk_speed: f32,
    yaw_sin: f32,
    yaw_cos: f32,
) -> [f32; 3] {
    let forward = [-yaw_sin, 0.0, -yaw_cos];
    let right = [yaw_cos, 0.0, -yaw_sin];
    let intent = [
        right[0] * f32::from(move_x) + forward[0] * f32::from(move_z),
        right[1] * f32::from(move_x) + forward[1] * f32::from(move_z),
        right[2] * f32::from(move_x) + forward[2] * f32::from(move_z),
    ];
    let length = step_vector_length(intent);
    if length == 0.0 {
        return [0.0; 3];
    }
    let inverse = 1.0 / length;
    [
        intent[0] * inverse * walk_speed,
        intent[1] * inverse * walk_speed,
        intent[2] * inverse * walk_speed,
    ]
}

/// Fused vector length (`stepVectorLength`).
fn step_vector_length(v: [f32; 3]) -> f32 {
    let inner = f64::mul_add(f64::from(v[0]), f64::from(v[0]), f64::from(v[1] * v[1])) as f32;
    let sum = f64::mul_add(f64::from(v[2]), f64::from(v[2]), f64::from(inner)) as f32;
    f64::from(sum).sqrt() as f32
}

/// Approach mirror (`moveToward`, `packages/shared/physics/motion.go`).
fn move_toward(current: [f32; 3], target: [f32; 3], maximum_delta: f32) -> [f32; 3] {
    let delta = [
        target[0] - current[0],
        target[1] - current[1],
        target[2] - current[2],
    ];
    let length = step_vector_length(delta);
    if length <= maximum_delta {
        return target;
    }
    let scale = maximum_delta / length;
    [
        current[0] + delta[0] * scale,
        current[1] + delta[1] * scale,
        current[2] + delta[2] * scale,
    ]
}

fn min3(a: f32, b: f32, c: f32) -> f32 {
    a.min(b.min(c))
}

fn max3(a: f32, b: f32, c: f32) -> f32 {
    a.max(b.max(c))
}

/// Prism mirror (`stepPrismFor`): the sweep hull padded by the body box, probe
/// and epsilon, floored to cells; an over-cap prism refuses without effect.
fn step_prism(
    position: [f32; 3],
    sweep_min: [f32; 3],
    sweep_max: [f32; 3],
    step_height: f32,
) -> Result<([i32; 3], [u32; 3]), ServerError> {
    let minimum = [
        position[0] + sweep_min[0] - HALF_WIDTH - COLLISION_EPSILON,
        position[1] + 0.0f32.min(sweep_min[1]).min(step_height) - GROUND_PROBE - COLLISION_EPSILON,
        position[2] + sweep_min[2] - HALF_WIDTH - COLLISION_EPSILON,
    ];
    let maximum = [
        position[0] + sweep_max[0] + HALF_WIDTH + COLLISION_EPSILON,
        position[1] + 0.0f32.max(sweep_max[1]).max(step_height) + PLAYER_HEIGHT + COLLISION_EPSILON,
        position[2] + sweep_max[2] + HALF_WIDTH + COLLISION_EPSILON,
    ];
    let origin = [
        checked_floor(minimum[0])?,
        checked_floor(minimum[1])?,
        checked_floor(minimum[2])?,
    ];
    let end = [
        checked_floor(maximum[0])?,
        checked_floor(maximum[1])?,
        checked_floor(maximum[2])?,
    ];
    let mut dimensions = [0u32; 3];
    let mut cells: u64 = 1;
    for axis in 0..3 {
        let span = i64::from(end[axis]) - i64::from(origin[axis]) + 1;
        if span <= 0 || span > u64::from(u32::MAX) as i64 {
            return Err(ServerError::InvalidInput { field: "actor" });
        }
        if i64::from(origin[axis]) + span - 1 > i64::from(i32::MAX) {
            return Err(ServerError::InvalidInput { field: "actor" });
        }
        dimensions[axis] = span as u32;
        cells *= span as u64;
        if cells > PRISM_MAX_CELLS {
            return Err(ServerError::InvalidInput { field: "actor" });
        }
    }
    Ok((origin, dimensions))
}

/// Grid mirror (`encodeStepInput` cell loop, y/x/z order): staged blocks map
/// through the collision shapes while unobserved cells stay unloaded, the
/// kernel's unknown-as-blocking policy.
/// Managed height planes are air; unavailable in-height and disabled cells stay blocking.
fn prism_cells(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    origin: [i32; 3],
    dimensions: [u32; 3],
) -> Result<Vec<CollisionCell>, ServerError> {
    let total = u64::from(dimensions[0]) * u64::from(dimensions[1]) * u64::from(dimensions[2]);
    let mut cells = Vec::new();
    cells
        .try_reserve_exact(total as usize)
        .map_err(|_| ServerError::InvalidInput { field: "actor" })?;
    for y in 0..dimensions[1] {
        for x in 0..dimensions[0] {
            for z in 0..dimensions[2] {
                let pos = BlockPos::new(
                    origin[0] + x as i32,
                    origin[1] + y as i32,
                    origin[2] + z as i32,
                );
                match view.live_collision_block(dimension, pos) {
                    None => cells.push(CollisionCell::default()),
                    Some(block) => cells.push(collision_cell(block)?),
                }
            }
        }
    }
    Ok(cells)
}

/// Per-block shape mirror (`BlockCollisionBoxes`,
/// `packages/shared/physics/types.go`): air, fluids, plants, torches, snow and
/// door uppers carry no box; beds, doors and farmland carry their reduced
/// boxes; everything else is a full cube.
fn collision_cell(block: u16) -> Result<CollisionCell, ServerError> {
    if block == AIR
        || is_fluid(block)
        || (WHEAT_FIRST..=WHEAT_LAST).contains(&block)
        || (POTATO_FIRST..=POTATO_LAST).contains(&block)
        || (CARROT_FIRST..=CARROT_LAST).contains(&block)
        || block == SHORT_GRASS
        || block == SAPLING
        || (TORCH_FIRST..=TORCH_LAST).contains(&block)
        || (SNOW_FIRST..=SNOW_LAST).contains(&block)
        || block == DOOR_UPPER
    {
        return empty_cell();
    }
    if (BED_FIRST..=BED_LAST).contains(&block) {
        return boxed_cell([[[0.0, 0.0, 0.0], [1.0, BED_TOP, 1.0]]]);
    }
    if (DOOR_LOWER_FIRST..=DOOR_LOWER_LAST).contains(&block) {
        let index = block - DOOR_LOWER_FIRST;
        let direction = index / 2;
        let open = index % 2 == 1;
        let thin = DOOR_THICKNESS;
        let wide = 1.0 - DOOR_THICKNESS;
        let shape = match (direction, open) {
            (0, false) | (1, true) => [[0.0, 0.0, wide], [1.0, 1.0, 1.0]],
            (1, false) | (2, true) => [[0.0, 0.0, 0.0], [thin, 1.0, 1.0]],
            (2, false) | (3, true) => [[0.0, 0.0, 0.0], [1.0, 1.0, thin]],
            _ => [[wide, 0.0, 0.0], [1.0, 1.0, 1.0]],
        };
        return boxed_cell([[shape[0], shape[1]]]);
    }
    if (FARMLAND_FIRST..=FARMLAND_LAST).contains(&block) {
        return boxed_cell([[[0.0, 0.0, 0.0], [1.0, FARMLAND_TOP, 1.0]]]);
    }
    boxed_cell([[[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]])
}

const EMPTY_BOX: Aabb = Aabb {
    minimum: [0.0; 3],
    maximum: [0.0; 3],
};

fn empty_cell() -> Result<CollisionCell, ServerError> {
    CollisionCell::try_new(true, [EMPTY_BOX; 8], 0).map_err(|_| ServerError::Internal {
        invariant: "passive motion grid",
    })
}

fn boxed_cell(local: [[[f32; 3]; 2]; 1]) -> Result<CollisionCell, ServerError> {
    let mut boxes = [EMPTY_BOX; 8];
    boxes[0] = Aabb {
        minimum: local[0][0],
        maximum: local[0][1],
    };
    CollisionCell::try_new(true, boxes, 1).map_err(|_| ServerError::Internal {
        invariant: "passive motion grid",
    })
}
