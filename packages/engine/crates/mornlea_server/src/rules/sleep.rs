//! Sleeping settlement and the seasonal morning transition.
//!
//! Two owned shapes live on the [`RulePhase::SleepSettlement`] phase: one
//! authority bed interaction at a time through [`enter`], and the fixed
//! all-asleep settlement batch through [`settle`]. Both consume the
//! reducer-carried bookkeeping the frozen `RuleCall` cannot bring — the
//! [`SleepState`] record, the sleeping-session set and the tick's active
//! roster — exactly like the accepted furnace batch's interest set, so the
//! bare `run` entry shape-gates and reports zero work while the serial
//! reducer calls the bodies with the carried state. [`run`] refuses every
//! other shape without effect, including the door interaction kind that
//! belongs to the placement provider.
//!
//! Mirrored Go rows, each cited at its site:
//!
//! - `packages/server/sim/entity/sleep.go` (`executeInteractBed`,
//!   `settleSleepThroughNight`): the authority bed ray, the night gate, the
//!   sneak refusal, the silent non-bed no-op, the foot respawn anchor, and
//!   the morning transition with its two linear scans over the active
//!   roster. The eligibility base is active players only: a pending-respawn
//!   player neither triggers nor blocks the morning, and a disconnect
//!   mid-sleep shrinks the eligible count.
//! - `packages/shared/core/day_phase.go` (`IsDisplayNightPhase` 13000..23000
//!   inclusive, `EffectiveDayPhaseAt`, `DayArcTicks`, `EffectiveMorningOffset`)
//!   and `packages/shared/core/season.go` (`YearPhaseAt`, `SeasonAt`,
//!   `SeasonProgressAt`): the seasonized phase and the inverted morning
//!   offset. The transition computes against `world_time + 1`, the absolute
//!   time the environment phase will complete this tick with, so the
//!   seasonized phase lands on the morning arc start the moment the tick
//!   ends; the absolute clock itself never moves here, only the display
//!   offset.
//! - `packages/shared/core/bed.go` (`IsBed`, `IsBedFoot`, `BedDir`,
//!   `BedHeadNeighbor`) with `bedHalfPositions` in
//!   `packages/server/sim/entity/bed.go`: either bed half resolves the shared
//!   foot cell the record stores.
//! - `packages/server/sim/entity/tick.go` (the `CommandPlayerInput` wake)
//!   and `player.go` (`applyDamage`, `beginReset`): a move axis or the jump
//!   bit wakes, look-only input and the sprint bit do not, real damage wakes,
//!   and every wake keeps the respawn record. The damage wake consumes the
//!   accepted survival provider's victim-routed combat-hit observations; a
//!   hit naming a hostile target is an attacker confirmation, not damage.
//! - The transition stages the updated [`SleepState`] record plus the
//!   publication world record exactly like the environment provider's dual
//!   staging: `pending_offset` names the staged morning offset for the
//!   environment phase to consume once at tick end, and the world record
//!   carries the new display offset with the untouched absolute clock.

use mornlea_domain::{
    BlockPos, CombatTarget, Dimension, Event, EventRecipient, WorldState, WorldStateParts,
};
use mornlea_engine::native::contracts::raycast::{Ray, RayCursor, RaycastOp};
use mornlea_engine::native::raycast::NativeRaycast;

use crate::core::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRuntime, AuthorityInteraction,
    BlockObservation, InteractionKind, PhaseReport, RuleCall, RuleEffect, RulePhase, ServerError,
    SessionKey, SleepState,
};
use crate::core::state::{AuthorityReadView, TickContext};

/// Air cell the ray walks through (`core.AirID`,
/// `packages/shared/core/block.go`).
const AIR: u16 = 0;
/// First bed-foot form, south (`core.BedFootSouthID`); the eight bed forms
/// run foot south/west/north/east then head south/west/north/east.
const BED_FOOT_SOUTH: u16 = 76;
const BED_FOOT_EAST: u16 = 79;
const BED_HEAD_SOUTH: u16 = 80;
const BED_HEAD_EAST: u16 = 83;
/// Display day length (`core.DayLengthTicks`).
const DAY_LENGTH_TICKS: u32 = 24_000;
/// Seasonized-phase midpoint (`core` `halfDayTicks`).
const HALF_DAY_TICKS: u32 = DAY_LENGTH_TICKS / 2;
/// Inclusive display night window shared with the hostile spawn gate
/// (`core.DisplayNightBegin`/`End`).
const DISPLAY_NIGHT_BEGIN: u16 = 13_000;
const DISPLAY_NIGHT_END: u16 = 23_000;
/// Four seasons per year (`core.YearTicks`).
const YEAR_TICKS: u64 = 288_000;
/// One season in authority ticks (`core.SeasonLengthTicks`).
const SEASON_LENGTH_TICKS: u64 = 72_000;
/// `core.SeasonProgressAt` quantizes in-season progress to 0..255.
const SEASON_PROGRESS_QUANTUM: u64 = 256;
/// Morning arc start the transition targets (`settleSleepThroughNight`
/// passing the constant 0 morning phase).
const MORNING_PHASE: u16 = 0;

/// One settled batch's outcome: the phase report, the record the reducer
/// carries into the next tick, and the sessions still asleep after wake
/// processing.
pub struct SettledSleep {
    pub report: PhaseReport,
    pub record: SleepState,
    pub sleeping: Vec<SessionKey>,
}

/// The frozen phase entry for the sleeping settlement.
///
/// The two owned shapes are the bare batch and the internal bed interaction;
/// both bodies consume reducer-carried bookkeeping (the [`SleepState`]
/// record, the sleeping set, the active roster) that the frozen `RuleCall`
/// cannot carry, so this entry shape-gates and reports zero work with
/// nothing staged, mirroring the accepted furnace batch entry. The serial
/// reducer calls [`enter`] per bed interaction and [`settle`] once per tick.
/// Every other shape — a wrong phase, an actor or command payload, or the
/// door interaction kind that belongs to the placement provider — refuses
/// with nothing staged.
pub fn run(_ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::SleepSettlement
        || call.actor.is_some()
        || call.command.is_some()
        || call
            .internal
            .is_some_and(|interaction| interaction.kind != InteractionKind::Bed)
    {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    Ok(PhaseReport {
        examined: 0,
        applied: 0,
        carried: 0,
        rejected: 0,
    })
}

/// Settles one authority bed interaction (`executeInteractBed` in
/// `packages/server/sim/entity/sleep.go`): the authority look ray from the
/// eye along the held look finds the first observed cell, a non-bed target
/// settles silently with no state change, a sneaking player refuses, a phase
/// outside the night window refuses, and either bed half records the foot
/// cell as the respawn anchor on both the returned record and the runtime
/// lane the survival death path reads. Refusals leave the carried record and
/// the context untouched.
///
/// The interaction order is the Go order: no target within reach refuses
/// before the bed check, the non-bed check settles silently before the sneak
/// gate, and the sneak gate refuses before the night gate.
pub fn enter(
    ctx: &mut TickContext<'_>,
    record: &SleepState,
    interaction: &AuthorityInteraction,
) -> Result<(PhaseReport, SleepState), ServerError> {
    const REFUSAL: ServerError = ServerError::InvalidInput { field: "sleep" };
    let actor = ActorKey::Player(interaction.session);
    let environment = ctx
        .read()
        .environment()
        .cloned()
        .ok_or(ServerError::Internal {
            invariant: "sleep settlement snapshot",
        })?;
    let view_record = ctx
        .read()
        .actor(actor)
        .ok_or(ServerError::InvalidInput { field: "actor" })?;
    if view_record.lifecycle != ActorLifecycle::Active {
        return Err(ServerError::InvalidInput { field: "actor" });
    }
    let dimension = view_record.dimension;
    let position = view_record.motion.position().get();
    let eye = [
        position[0],
        position[1] + environment.tunables.eye_height(),
        position[2],
    ];
    let direction = look_direction(interaction.look.yaw(), interaction.look.pitch());
    let Some(hit) = cast_ray(
        &ctx.read(),
        dimension,
        eye,
        direction,
        environment.tunables.interaction_reach(),
    )?
    else {
        return Err(REFUSAL);
    };
    if !is_bed(hit.block) {
        // A non-bed target is the door-precedent silent success: the client
        // waits for no result and nothing changes.
        return Ok((
            PhaseReport {
                examined: 1,
                applied: 0,
                carried: 0,
                rejected: 0,
            },
            record.clone(),
        ));
    }
    if ctx
        .read()
        .runtime(actor)
        .and_then(|runtime| runtime.controls)
        .is_some_and(|control| control.actions().sneaking)
    {
        // Sneak-held entry refuses: the placement path owns sneaking edits.
        return Err(REFUSAL);
    }
    if !is_display_night_phase(effective_day_phase_at(
        environment.world_time,
        environment.day_phase_offset,
        environment.season_offset,
    )) {
        // Daytime bed use refuses with zero state change, reusing the frozen
        // refusal shape for a block that rejects the interaction.
        return Err(REFUSAL);
    }
    let Some((foot, _head)) = bed_half_positions(hit.pos, hit.block) else {
        // Defensive: `is_bed` already guarantees the half resolution.
        return Err(REFUSAL);
    };
    // The respawn anchor lands on the runtime lane the survival death path
    // carries forward, replacing any previous anchor for the session.
    let mut runtime = entry_runtime(&ctx.read(), actor)?;
    runtime.aux = ActorAux::Player {
        respawn: Some((dimension, foot)),
    };
    ctx.stage(RuleEffect::Runtime(runtime))
        .map_err(|_| ServerError::Internal {
            invariant: "sleep respawn staging",
        })?;
    // The record stores the respawn anchor keyed by session, replacing the
    // session's own row and never touching the other players' anchors.
    let mut beds = record.beds.clone();
    match beds
        .iter()
        .position(|(listed, _, _)| *listed == interaction.session)
    {
        Some(index) => beds[index] = (interaction.session, dimension, foot),
        None => beds.push((interaction.session, dimension, foot)),
    }
    let updated = SleepState::try_new(beds, record.day_phase_offset, record.pending_offset)
        .map_err(|_| REFUSAL)?;
    Ok((
        PhaseReport {
            examined: 1,
            applied: 1,
            carried: 0,
            rejected: 0,
        },
        updated,
    ))
}

/// Settles the fixed all-asleep batch (`settleSleepThroughNight` in
/// `packages/server/sim/entity/sleep.go`).
///
/// The wake pass first removes every sleeper with real movement intent
/// (a move axis or the jump bit on the staged held controls — look-only
/// input and the sprint bit are neutral) or a routed damage observation from
/// the survival provider; every wake keeps the bed record. The transition
/// then fires only when at least one player is active and every active
/// player is still asleep: a disconnect mid-sleep shrinks the eligible set,
/// and a pending-respawn player is absent from the active roster so it
/// neither triggers nor blocks. The morning transition inverts the display
/// offset to `EffectiveMorningOffset(completed, DayArcTicks(
/// YearPhaseAt(completed, season_offset)), 0)` with `completed` the absolute
/// time the environment phase will finish this tick at, stages the updated
/// record plus the publication world record with the untouched absolute
/// clock, and wakes every sleeper — including disconnected ones, so a stale
/// flag cannot leak into the next all-asleep decision.
///
/// `sleeping` and `active` are ascending session sets the reducer owns;
/// `examined` counts both linear scans (the wake pass and the eligibility
/// scan) and `applied` counts the staged transition.
pub fn settle(
    ctx: &mut TickContext<'_>,
    record: &SleepState,
    sleeping: &[SessionKey],
    active: &[SessionKey],
) -> Result<SettledSleep, ServerError> {
    let environment = ctx
        .read()
        .environment()
        .cloned()
        .ok_or(ServerError::Internal {
            invariant: "sleep settlement snapshot",
        })?;
    let damaged = damaged_sessions(ctx);
    let mut remaining = Vec::with_capacity(sleeping.len());
    for session in sleeping {
        let actor = ActorKey::Player(*session);
        let moves = ctx
            .read()
            .runtime(actor)
            .and_then(|runtime| runtime.controls)
            .is_some_and(|control| {
                let movement = control.movement();
                movement.move_x != 0 || movement.move_z != 0 || movement.jump
            });
        if moves || damaged.contains(session) {
            continue;
        }
        remaining.push(*session);
    }
    let all_asleep = !active.is_empty() && active.iter().all(|session| remaining.contains(session));
    let report = PhaseReport {
        examined: sleeping.len() + active.len(),
        applied: usize::from(all_asleep),
        carried: 0,
        rejected: 0,
    };
    if !all_asleep {
        return Ok(SettledSleep {
            report,
            record: record.clone(),
            sleeping: remaining,
        });
    }
    // The completed absolute time this tick ends with: the environment phase
    // advances the clock by exactly one, so the seasonized phase lands on
    // the morning arc start the moment the tick completes.
    let completed = environment.world_time.saturating_add(1);
    let day_arc = day_arc_ticks(year_phase_at(completed, environment.season_offset));
    let offset = effective_morning_offset(completed, day_arc, MORNING_PHASE);
    let updated = SleepState::try_new(
        record.beds.clone(),
        u64::from(offset),
        Some(u64::from(offset)),
    )
    .map_err(|_| ServerError::Internal {
        invariant: "sleep record staging",
    })?;
    let temperature = ctx
        .read()
        .world()
        .map(|world| world.temperature())
        .unwrap_or(0);
    let published = WorldState::try_new(WorldStateParts {
        day_phase_offset: offset,
        world_time_ticks: environment.world_time,
        weather: environment.weather,
        season: season_at(environment.world_time, environment.season_offset),
        season_progress: season_progress_at(environment.world_time, environment.season_offset),
        temperature,
    })
    .map_err(|_| ServerError::InvalidInput {
        field: "day_phase_offset",
    })?;
    ctx.stage(RuleEffect::Sleep(updated.clone()))
        .map_err(|_| ServerError::Internal {
            invariant: "sleep record staging",
        })?;
    ctx.stage(RuleEffect::World(published))
        .map_err(|_| ServerError::Internal {
            invariant: "sleep publication staging",
        })?;
    Ok(SettledSleep {
        report,
        record: updated,
        sleeping: Vec::new(),
    })
}

/// Sessions that took real damage this tick, read from the survival
/// provider's victim-routed player-target combat hits. An attacker
/// confirmation naming a hostile or passive target does not wake its
/// recipient.
fn damaged_sessions(ctx: &TickContext<'_>) -> Vec<SessionKey> {
    let mut damaged = Vec::new();
    for routed in ctx.events() {
        let EventRecipient::Session(raw) = routed.recipient() else {
            continue;
        };
        let Event::CombatHit(hit) = routed.event() else {
            continue;
        };
        if hit.target() != CombatTarget::Player {
            continue;
        }
        if let Some(session) = SessionKey::from_raw(raw) {
            damaged.push(session);
        }
    }
    damaged
}

/// The entry's runtime lane: the staged record when one exists, else the
/// same save-derived defaults the survival provider's merge builds, so the
/// staged respawn anchor never resurrects dropped transients.
fn entry_runtime(
    view: &AuthorityReadView<'_>,
    actor: ActorKey,
) -> Result<ActorRuntime, ServerError> {
    if let Some(staged) = view.runtime(actor) {
        return Ok(staged.clone());
    }
    let record = view
        .actor(actor)
        .ok_or(ServerError::InvalidInput { field: "actor" })?;
    let ActorBody::Player(save) = &record.body else {
        return Err(ServerError::Internal {
            invariant: "sleep entry body",
        });
    };
    Ok(ActorRuntime {
        key: record.key,
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: record.survival.oxygen(),
        peak_y: record.motion.position().get()[1],
        exhaustion_milli: u32::from(save.exhaustion_milli),
        saturation_milli: u32::from(save.saturation_milli),
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Player { respawn: None },
    })
}

/// Unit look direction of a rotation, the exact `LookDirection` formula
/// (`packages/server/sim/entity/command.go`): yaw zero faces north (`-Z`),
/// positive pitch looks up.
fn look_direction(yaw: f32, pitch: f32) -> [f32; 3] {
    let cos_pitch = pitch.cos();
    [-yaw.sin() * cos_pitch, pitch.sin(), -yaw.cos() * cos_pitch]
}

/// Walks the numerical ray kernel batch by batch and returns the full
/// observation of the first non-air cell, the same walk the accepted
/// placement geometry performs: an unobserved cell refuses because the
/// authority cannot certify geometry it has not observed, observed air
/// continues the walk, and no hit within reach reports `None`.
fn cast_ray(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    origin: [f32; 3],
    direction: [f32; 3],
    reach: f32,
) -> Result<Option<BlockObservation>, ServerError> {
    const REFUSAL: ServerError = ServerError::InvalidInput {
        field: "interaction",
    };
    let length =
        (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2])
            .sqrt();
    if !length.is_finite() || length < 1e-6 {
        return Err(REFUSAL);
    }
    let normalized = [
        direction[0] / length,
        direction[1] / length,
        direction[2] / length,
    ];
    let mut cursor = RayCursor::try_new(Ray {
        origin,
        direction: normalized,
        maximum: reach,
    })
    .map_err(|_| REFUSAL)?;
    loop {
        let batch = NativeRaycast.next_batch(&mut cursor).map_err(|_| REFUSAL)?;
        for record in batch.records() {
            let cell = BlockPos::new(record.cell[0], record.cell[1], record.cell[2]);
            match view.observation(dimension, cell) {
                None => return Err(REFUSAL),
                Some(observed) if observed.block == AIR => {}
                Some(observed) => return Ok(Some(observed)),
            }
        }
        if batch.is_done() {
            return Ok(None);
        }
    }
}

/// Reports whether a block number is any bed half (`core.IsBed`,
/// `packages/shared/core/bed.go`).
fn is_bed(block: u16) -> bool {
    (BED_FOOT_SOUTH..=BED_HEAD_EAST).contains(&block)
}

/// Reports whether a block number is a bed-foot form (`core.IsBedFoot`).
fn is_bed_foot(block: u16) -> bool {
    (BED_FOOT_SOUTH..=BED_FOOT_EAST).contains(&block)
}

/// Direction encoding of a bed form, south 0 / west 1 / north 2 / east 3
/// (`core.BedDir`); the head segment translates the foot segment by four, so
/// paired halves share the code.
fn bed_dir(block: u16) -> Option<u8> {
    match block {
        BED_FOOT_SOUTH | BED_HEAD_SOUTH => Some(0),
        x if x == BED_FOOT_SOUTH + 1 || x == BED_HEAD_SOUTH + 1 => Some(1),
        x if x == BED_FOOT_SOUTH + 2 || x == BED_HEAD_SOUTH + 2 => Some(2),
        x if x == BED_FOOT_EAST || x == BED_HEAD_EAST => Some(3),
        _ => None,
    }
}

/// Head-cell neighbor of a foot cell in the bed's facing
/// (`core.BedHeadNeighbor`): south +Z, west -X, north -Z, east +X.
fn bed_head_neighbor(foot: BlockPos, dir: u8) -> BlockPos {
    let (dx, dz) = match dir {
        0 => (0, 1),
        1 => (-1, 0),
        2 => (0, -1),
        _ => (1, 0),
    };
    BlockPos::new(foot.x() + dx, foot.y(), foot.z() + dz)
}

/// Resolves the (foot, head) pair of a bed hit (`bedHalfPositions`,
/// `packages/server/sim/entity/bed.go`): a foot hit names the pair directly,
/// a head hit derives the foot by stepping against the facing.
fn bed_half_positions(target: BlockPos, block: u16) -> Option<(BlockPos, BlockPos)> {
    let dir = bed_dir(block)?;
    if is_bed_foot(block) {
        return Some((target, bed_head_neighbor(target, dir)));
    }
    let (dx, dz) = match dir {
        0 => (0, -1),
        1 => (1, 0),
        2 => (0, 1),
        _ => (-1, 0),
    };
    let foot = BlockPos::new(target.x() + dx, target.y(), target.z() + dz);
    Some((foot, target))
}

/// Folds the in-year tick index: each side takes its modulo before the sum,
/// so no absolute time or offset can overflow the addition. Mirrors
/// `core.yearIndex` (`packages/shared/core/season.go`).
fn year_index(world_time: u64, season_offset: u32) -> u64 {
    (world_time % YEAR_TICKS + u64::from(season_offset) % YEAR_TICKS) % YEAR_TICKS
}

/// Year phase 0..1 at an absolute time (`core.YearPhaseAt`).
fn year_phase_at(world_time: u64, season_offset: u32) -> f64 {
    year_index(world_time, season_offset) as f64 / YEAR_TICKS as f64
}

/// Day arc in ticks at one year phase (`core.DayArcTicks`): the sinusoidal
/// day fraction scaled to the 24000-tick day, rounded to nearest and floored
/// to even so both warp branches divide symmetrically. Anchors: winter
/// solstice 8400, equinox 12000, summer solstice 15600.
fn day_arc_ticks(year_phase: f64) -> u16 {
    let fraction = 0.5 + 0.15 * (2.0 * std::f64::consts::PI * year_phase).sin();
    let ticks = (fraction * f64::from(DAY_LENGTH_TICKS)).round() as i64;
    let ticks = if ticks % 2 == 1 { ticks - 1 } else { ticks };
    ticks as u16
}

/// Seasonized display phase for one linear phase and day arc
/// (`core.EffectiveDayPhase`): the day arc maps linearly onto the first half
/// of the seasonized cycle and the night arc onto the second. An illegal arc
/// (zero, odd, or over one day) returns zero rather than dividing by it.
fn effective_day_phase(world_time: u64, offset: u16, day_arc: u16) -> u16 {
    let arc_valid = day_arc != 0 && day_arc <= DAY_LENGTH_TICKS as u16 && day_arc.is_multiple_of(2);
    if !arc_valid {
        return 0;
    }
    let p = ((world_time % u64::from(DAY_LENGTH_TICKS) + u64::from(offset))
        % u64::from(DAY_LENGTH_TICKS)) as u32;
    let arc = u32::from(day_arc);
    if p < arc {
        return (p * HALF_DAY_TICKS / arc) as u16;
    }
    (HALF_DAY_TICKS + (p - arc) * HALF_DAY_TICKS / (DAY_LENGTH_TICKS - arc)) as u16
}

/// Combined seasonized phase (`core.EffectiveDayPhaseAt`): the day arc is
/// always derived here, never supplied by a caller.
fn effective_day_phase_at(world_time: u64, offset: u16, season_offset: u32) -> u16 {
    effective_day_phase(
        world_time,
        offset,
        day_arc_ticks(year_phase_at(world_time, season_offset)),
    )
}

/// Inclusive display night window shared with the hostile spawn gate
/// (`core.IsDisplayNightPhase`).
fn is_display_night_phase(phase: u16) -> bool {
    (DISPLAY_NIGHT_BEGIN..=DISPLAY_NIGHT_END).contains(&phase)
}

/// Inverts the warp for one target morning phase (`core.EffectiveMorningOffset`
/// in `packages/shared/core/day_phase.go`): each branch takes the ceiling of
/// its exact inverse (the `(a + b - 1) / b` form), the degenerate top-of-cycle
/// case falls back to the last tick, and the result translates the current
/// linear phase onto the target without ever touching the absolute clock.
/// Illegal arcs and out-of-range targets return zero.
fn effective_morning_offset(world_time: u64, day_arc: u16, morning_phase: u16) -> u16 {
    let arc_valid = day_arc != 0 && day_arc <= DAY_LENGTH_TICKS as u16 && day_arc.is_multiple_of(2);
    if !arc_valid || u32::from(morning_phase) >= DAY_LENGTH_TICKS {
        return 0;
    }
    let arc = u32::from(day_arc);
    let target = u32::from(morning_phase);
    // Each branch is the Go `(a + b - 1) / b` ceiling; unsigned `div_ceil`
    // is that same ceiling.
    let mut linear = if target < HALF_DAY_TICKS {
        (target * arc).div_ceil(HALF_DAY_TICKS)
    } else {
        let night = DAY_LENGTH_TICKS - arc;
        arc + ((target - HALF_DAY_TICKS) * night).div_ceil(HALF_DAY_TICKS)
    };
    if linear >= DAY_LENGTH_TICKS {
        linear = DAY_LENGTH_TICKS - 1;
    }
    ((linear + DAY_LENGTH_TICKS - (world_time % u64::from(DAY_LENGTH_TICKS)) as u32)
        % DAY_LENGTH_TICKS) as u16
}

/// Season at an absolute time: the quarter of the year the in-year index
/// falls in, boundaries landing on the first tick of each new season.
/// Mirrors `core.SeasonAt` (`packages/shared/core/season.go`).
fn season_at(world_time: u64, season_offset: u32) -> mornlea_domain::Season {
    match year_index(world_time, season_offset) / SEASON_LENGTH_TICKS {
        0 => mornlea_domain::Season::Spring,
        1 => mornlea_domain::Season::Summer,
        2 => mornlea_domain::Season::Autumn,
        _ => mornlea_domain::Season::Winter,
    }
}

/// Quantized in-season progress 0..255: season start is 0, the last tick
/// before the boundary is 255, and the boundary wraps back to 0. Mirrors
/// `core.SeasonProgressAt`.
fn season_progress_at(world_time: u64, season_offset: u32) -> u8 {
    let in_season = year_index(world_time, season_offset) % SEASON_LENGTH_TICKS;
    (in_season * SEASON_PROGRESS_QUANTUM / SEASON_LENGTH_TICKS) as u8
}
