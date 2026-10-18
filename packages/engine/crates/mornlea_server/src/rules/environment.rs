//! Environment rule provider: the authoritative climate clock.
//!
//! One batch call at `RulePhase::EnvironmentEnd` completes a tick's
//! environment. The provider consumes the tick-start `EnvironmentState`
//! snapshot the reducer froze, advances the absolute world time exactly one
//! tick, advances the weather clock exactly once, derives the season from the
//! advanced absolute time, and stages the advanced record plus the
//! publication world record the final publication copies. It owns no world
//! mutation, fluid, or farming work: the frozen snapshot's display
//! `day_phase_offset` passes through untouched, because a sleep settlement
//! writes only the offset and never the absolute clock
//! (`packages/shared/core/day_phase.go`).
//!
//! Numeric sources are mirrored, not chosen here:
//!
//! - Weather clock constants, dice salts, and the fixed segment distribution:
//!   `packages/server/sim/runtime/weather.go` (`advanceWeatherClock`,
//!   `rollWeatherSegment`, `rollWeatherDuration`, `weatherHash`). One minute
//!   is exactly 1200 authority ticks; clear segments span 10..150 minutes,
//!   rain and thunder 3..13 minutes; one roll in four lands rain and one
//!   rain roll in eight upgrades to thunder.
//! - The restore-zero rule: `packages/server/sim/runtime/engine.go`
//!   (`RestoreWeather`) — a zero remaining clock is a legacy save that never
//!   recorded weather; it keeps the restored kind and regenerates a default
//!   duration at completed tick 0, so a migrated world behaves like a fresh
//!   world of the same seed.
//! - Season derivation: `packages/shared/core/season.go` (`SeasonAt`,
//!   `SeasonProgressAt`, `yearIndex`) with 72000 ticks per season and 288000
//!   per year. The season offset itself is a seed-derived assembly quantity
//!   carried on the snapshot, not re-derived here.
//! - Day arc and seasonized day phase: `packages/shared/core/day_phase.go`
//!   (`DayArcTicks`, `EffectiveDayPhase`, `EffectiveDayPhaseAt`) over the
//!   24000-tick display day; the day arc follows the year phase, and sleep,
//!   hostiles, passives, player publication and random blocks read it here.
//! - The absolute clock saturates instead of wrapping: the frozen acceptance
//!   row `environment::step_restore_boundary` requires a non-wrapping clock,
//!   which deliberately strengthens the Go `worldTime.Add(1)` wrap at the
//!   `u64` edge.

use mornlea_domain::{Season, Weather, WorldState, WorldStateParts};

use crate::core::contracts::{
    EnvironmentState, PhaseReport, RuleCall, RuleEffect, RulePhase, ServerError,
};
use crate::core::state::TickContext;

/// One season in authority ticks: three display days, from the Go
/// `core.SeasonLengthTicks`.
const SEASON_LENGTH_TICKS: u64 = 72_000;
/// Four seasons per year, from the Go `core.YearTicks` (4 x 72000).
const YEAR_TICKS: u64 = 288_000;
/// `core.SeasonProgressAt` quantizes in-season progress to 0..255.
const SEASON_PROGRESS_QUANTUM: u64 = 256;
/// One display day in authority ticks, from the Go `core.DayLengthTicks`.
const DAY_LENGTH_TICKS: u32 = 24_000;
/// Seasonized-phase midpoint: the warp maps the day arc onto the first half
/// of the cycle and the night arc onto the second (`core` `halfDayTicks`).
const HALF_DAY_TICKS: u32 = DAY_LENGTH_TICKS / 2;

/// Weather minutes convert at exactly 1200 authority ticks: one tick is
/// 50ms at 20tps, from `runtime/weather.go` (`weatherTicksPerMinute`).
const WEATHER_TICKS_PER_MINUTE: u64 = 1_200;
/// Clear segment spans 10..150 minutes (`weatherClearMinTicks`/`Max`).
const WEATHER_CLEAR_MIN_TICKS: u64 = 10 * WEATHER_TICKS_PER_MINUTE;
const WEATHER_CLEAR_MAX_TICKS: u64 = 150 * WEATHER_TICKS_PER_MINUTE;
/// Rain and thunder segments span 3..13 minutes (`weatherRainMinTicks`/`Max`);
/// thunder reuses the rain interval because it is an in-rain upgrade whose
/// segment end returns to the same dice.
const WEATHER_RAIN_MIN_TICKS: u64 = 3 * WEATHER_TICKS_PER_MINUTE;
const WEATHER_RAIN_MAX_TICKS: u64 = 13 * WEATHER_TICKS_PER_MINUTE;
/// Fixed dice ratios: one roll in four lands rain, one rain roll in eight
/// upgrades to thunder (`weatherRainOneIn`, `weatherThunderOneIn`).
const WEATHER_RAIN_ONE_IN: u64 = 4;
const WEATHER_THUNDER_ONE_IN: u64 = 8;
/// Dice salts isolate the weather hash stream from terrain, crop, and season
/// hash streams, from `runtime/weather.go`.
const WEATHER_KIND_SALT: u64 = 0x8bad_f00d_51ab_0001;
const WEATHER_THUNDER_SALT: u64 = 0x8bad_f00d_51ab_0002;
const WEATHER_DURATION_SALT: u64 = 0x8bad_f00d_51ab_0003;

/// The environment phase: one batch update of time, weather, and season.
///
/// Refusal policy: a call whose shape does not name this batch phase (wrong
/// `RulePhase`, or any actor, command, or internal payload) is rejected with
/// a zero-count [`PhaseReport`] and no effect — the seam's no-effect rule.
/// A missing tick-start snapshot is a reducer sequencing bug and returns
/// [`ServerError::Internal`] with nothing staged.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::EnvironmentEnd
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
    let baseline = ctx
        .read()
        .environment()
        .cloned()
        .ok_or(ServerError::Internal {
            invariant: "environment snapshot",
        })?;
    // The snapshot's `next_tick` names the tick in progress; completing that
    // tick is `next_tick + 1`, mirroring `runtime/engine_step.go` feeding
    // `result.Tick = currentTick + 1` into the weather dice.
    let completed_tick = baseline.next_tick.saturating_add(1);
    let world_time = baseline.world_time.saturating_add(1);
    let (weather, weather_remaining) = advance_weather(
        baseline.weather,
        baseline.weather_remaining,
        baseline.seed,
        completed_tick,
    );
    let advanced = EnvironmentState {
        seed: baseline.seed,
        next_tick: completed_tick,
        world_time,
        day_phase_offset: baseline.day_phase_offset,
        season_offset: baseline.season_offset,
        weather,
        weather_remaining,
        difficulty: baseline.difficulty,
        tunables: baseline.tunables,
    };
    // Season and progress derive from the advanced absolute time alone; the
    // temperature scalar is per-player publication work keyed on each body's
    // height, so the world record carries the prior value through.
    let temperature = ctx
        .read()
        .world()
        .map(|world| world.temperature())
        .unwrap_or(0);
    let published = WorldState::try_new(WorldStateParts {
        day_phase_offset: advanced.day_phase_offset,
        world_time_ticks: world_time,
        weather: advanced.weather,
        season: season_at(world_time, advanced.season_offset),
        season_progress: season_progress_at(world_time, advanced.season_offset),
        temperature,
    })
    .map_err(|_| ServerError::InvalidInput {
        field: "day_phase_offset",
    })?;
    // The staged record is authoritative climate state; the world record is
    // the publication mirror the final publication copies from.
    ctx.stage(RuleEffect::Environment(advanced))
        .map_err(|_| ServerError::Internal {
            invariant: "environment staging",
        })?;
    ctx.stage(RuleEffect::World(published))
        .map_err(|_| ServerError::Internal {
            invariant: "environment publication staging",
        })?;
    Ok(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    })
}

/// Advances one tick of the weather clock.
///
/// Mirrors `runtime/weather.go` `advanceWeatherClock`: more than one tick of
/// remaining segment only decrements, and expiry rolls a fresh segment whose
/// kind never consults the expiring one. The Go illegal-kind normalization is
/// unrepresentable here because the domain `Weather` enum has no illegal
/// values. A zero remaining is the restored legacy-save clock: it keeps the
/// restored kind and regenerates a default duration at completed tick 0
/// (`RestoreWeather` zero branch), after which this tick's single decrement
/// applies — regenerated segments are always above one tick, so the decrement
/// cannot underflow.
fn advance_weather(
    kind: Weather,
    remaining: u32,
    seed: i64,
    completed_tick: u64,
) -> (Weather, u32) {
    if remaining > 1 {
        return (kind, remaining - 1);
    }
    if remaining == 1 {
        return roll_weather_segment(seed, completed_tick);
    }
    let restored = roll_weather_duration(seed, 0, kind);
    (kind, restored - 1)
}

/// Rolls one fresh weather segment: one-in-four rain, one-in-eight thunder
/// inside rain, then a duration from the kind's interval. Mirrors
/// `runtime/weather.go` `rollWeatherSegment`.
fn roll_weather_segment(seed: i64, completed_tick: u64) -> (Weather, u32) {
    let mut kind = Weather::Clear;
    if weather_hash(seed, completed_tick, WEATHER_KIND_SALT).is_multiple_of(WEATHER_RAIN_ONE_IN) {
        kind = Weather::Rain;
        if weather_hash(seed, completed_tick, WEATHER_THUNDER_SALT)
            .is_multiple_of(WEATHER_THUNDER_ONE_IN)
        {
            kind = Weather::Thunder;
        }
    }
    (kind, roll_weather_duration(seed, completed_tick, kind))
}

/// Draws one segment duration uniformly from the kind's fixed interval.
/// Mirrors `runtime/weather.go` `rollWeatherDuration`.
fn roll_weather_duration(seed: i64, completed_tick: u64, kind: Weather) -> u32 {
    let (min, max) = if kind == Weather::Clear {
        (WEATHER_CLEAR_MIN_TICKS, WEATHER_CLEAR_MAX_TICKS)
    } else {
        (WEATHER_RAIN_MIN_TICKS, WEATHER_RAIN_MAX_TICKS)
    };
    let span = max - min + 1;
    (min + weather_hash(seed, completed_tick, WEATHER_DURATION_SALT) % span) as u32
}

/// Folds (world seed, completed tick, dice salt) into one uniform roll.
/// Mirrors `runtime/weather.go` `weatherHash`: one `splitmix64` on the salted
/// seed, one on the tick-xored value, and a final terminator pass. A negative
/// seed takes the same two's-complement bit pattern the Go `uint64(seed)`
/// conversion produces.
fn weather_hash(seed: i64, completed_tick: u64, salt: u64) -> u64 {
    splitmix64(splitmix64(
        splitmix64((seed as u64) ^ salt) ^ completed_tick,
    ))
}

/// The repository's shared SplitMix64 terminator, copied verbatim from the Go
/// dice (`splitmix64` in `runtime/weather.go` and `shared/core/season.go`).
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Folds the in-year tick index: each side takes its modulo before the sum,
/// so no absolute time or offset can overflow the addition. Mirrors
/// `core.yearIndex`. This is the single Rust source for the in-year index;
/// sleep, hostiles, passives, player publication and random blocks call it
/// instead of keeping copies.
pub(crate) fn year_index(world_time: u64, season_offset: u32) -> u64 {
    (world_time % YEAR_TICKS + u64::from(season_offset) % YEAR_TICKS) % YEAR_TICKS
}

/// Year phase 0..1 at an absolute time (`core.YearPhaseAt`): the in-year
/// index divided by `YEAR_TICKS`. This is the single Rust source for year
/// phase; callers that need the phase go through here rather than dividing
/// a local year-index copy.
pub(crate) fn year_phase_at(world_time: u64, season_offset: u32) -> f64 {
    year_index(world_time, season_offset) as f64 / YEAR_TICKS as f64
}

/// Season at an absolute time: the quarter of the year the in-year index
/// falls in, boundaries landing on the first tick of each new season.
/// Mirrors `core.SeasonAt`. This is the single Rust source for the season;
/// sleep settlement and player publication call it instead of keeping copies.
pub(crate) fn season_at(world_time: u64, season_offset: u32) -> Season {
    match year_index(world_time, season_offset) / SEASON_LENGTH_TICKS {
        0 => Season::Spring,
        1 => Season::Summer,
        2 => Season::Autumn,
        _ => Season::Winter,
    }
}

/// Quantized in-season progress 0..255: season start is 0, the last tick
/// before the boundary is 255, and the boundary wraps back to 0. Mirrors
/// `core.SeasonProgressAt`. This is the single Rust source for season
/// progress; sleep settlement and player publication call it.
pub(crate) fn season_progress_at(world_time: u64, season_offset: u32) -> u8 {
    let in_season = year_index(world_time, season_offset) % SEASON_LENGTH_TICKS;
    (in_season * SEASON_PROGRESS_QUANTUM / SEASON_LENGTH_TICKS) as u8
}

/// Day arc in ticks at one year phase (`core.DayArcTicks`): the sinusoidal
/// day fraction scaled to the 24000-tick day, rounded to nearest and floored
/// to even so both warp branches divide symmetrically. Anchors: winter
/// solstice 8400, equinox 12000, summer solstice 15600. This is the single
/// Rust source for the day arc; sleep settlement derives it here instead of
/// keeping a copy, and the seasonized phase below derives it internally.
pub(crate) fn day_arc_ticks(year_phase: f64) -> u16 {
    let fraction = 0.5 + 0.15 * (2.0 * std::f64::consts::PI * year_phase).sin();
    let ticks = (fraction * f64::from(DAY_LENGTH_TICKS)).round() as i64;
    let ticks = if ticks % 2 == 1 { ticks - 1 } else { ticks };
    ticks as u16
}

/// Seasonized display phase for one linear phase and day arc
/// (`core.EffectiveDayPhase`): the day arc maps linearly onto the first half
/// of the seasonized cycle and the night arc onto the second. An illegal arc
/// (zero, odd, or over one day) returns zero rather than dividing by it.
/// This is the single Rust source for the seasonized phase; sleep, hostiles
/// and passives call it through `effective_day_phase_at` instead of keeping
/// copies.
pub(crate) fn effective_day_phase(world_time: u64, offset: u16, day_arc: u16) -> u16 {
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
/// always derived here, never supplied by a caller. This is the single Rust
/// source for the combined phase; sleep, hostiles, passives, player
/// publication and random blocks call it instead of assembling their own
/// warp from a local year-phase or day-arc copy.
pub(crate) fn effective_day_phase_at(world_time: u64, offset: u16, season_offset: u32) -> u16 {
    effective_day_phase(
        world_time,
        offset,
        day_arc_ticks(year_phase_at(world_time, season_offset)),
    )
}

#[cfg(test)]
mod season_tests {
    use super::*;

    /// `(world_time, season_offset, season, progress)` pins taken from the Go
    /// `core.SeasonAt` / `core.SeasonProgressAt` arithmetic: the first progress
    /// step (282 ticks), the last tick of each season, every season boundary,
    /// the year wrap, the seed-42 offset step at tick 89, and the `u64`/`u32`
    /// extremes that the per-term modulo keeps overflow-free.
    const SEASON_PINS: [(u64, u32, Season, u8); 17] = [
        (0, 0, Season::Spring, 0),
        (281, 0, Season::Spring, 0),
        (282, 0, Season::Spring, 1),
        (71_999, 0, Season::Spring, 255),
        (72_000, 0, Season::Summer, 0),
        (143_999, 0, Season::Summer, 255),
        (144_000, 0, Season::Autumn, 0),
        (216_000, 0, Season::Winter, 0),
        (287_999, 0, Season::Winter, 255),
        (288_000, 0, Season::Spring, 0),
        (0, 14_818, Season::Spring, 52),
        (88, 14_818, Season::Spring, 52),
        (89, 14_818, Season::Spring, 53),
        (0, 287_999, Season::Winter, 255),
        (1, 287_999, Season::Spring, 0),
        (u64::MAX, 0, Season::Summer, 140),
        (u64::MAX, u32::MAX, Season::Summer, 223),
    ];

    #[test]
    fn season_derivation_matches_go_pins() {
        for (world_time, offset, season, progress) in SEASON_PINS {
            assert_eq!(
                (
                    season_at(world_time, offset),
                    season_progress_at(world_time, offset)
                ),
                (season, progress),
                "world_time={world_time} season_offset={offset}"
            );
        }
    }
}

#[cfg(test)]
mod year_phase_tests {
    use super::*;

    /// `(world_time, season_offset, year_index)` pins taken from the Go
    /// `core.yearIndex` / `core.YearPhaseAt` arithmetic in
    /// `packages/shared/core/season.go`: season-quarter anchors (0, 0.25, 0.5),
    /// the year wrap, the seed-42 offset, and the `u64`/`u32` extremes. Year
    /// phase is the index divided by 288000; exact float anchors 0, 0.25 and
    /// 0.5 match `TestYearPhaseAtAnchors`.
    const YEAR_INDEX_PINS: [(u64, u32, u64); 14] = [
        (0, 0, 0),
        (1, 0, 1),
        (72_000, 0, 72_000),
        (143_999, 0, 143_999),
        (144_000, 0, 144_000),
        (216_000, 0, 216_000),
        (287_999, 0, 287_999),
        (288_000, 0, 0),
        (0, 14_818, 14_818),
        (89, 14_818, 14_907),
        (0, 287_999, 287_999),
        (1, 287_999, 0),
        (u64::MAX, 0, 111_615),
        (u64::MAX, u32::MAX, 134_910),
    ];

    #[test]
    fn year_phase_derivation_matches_go_pins() {
        for (world_time, offset, index) in YEAR_INDEX_PINS {
            assert_eq!(
                year_index(world_time, offset),
                index,
                "year_index({world_time}, {offset})"
            );
            let phase = year_phase_at(world_time, offset);
            assert_eq!(
                phase,
                index as f64 / YEAR_TICKS as f64,
                "year_phase_at({world_time}, {offset})"
            );
        }
        assert_eq!(year_phase_at(0, 0), 0.0);
        assert_eq!(year_phase_at(72_000, 0), 0.25);
        assert_eq!(year_phase_at(144_000, 0), 0.5);
        assert_eq!(year_phase_at(288_000, 0), 0.0);
    }
}

#[cfg(test)]
mod day_arc_tests {
    use super::*;

    /// `(year_phase, day_arc)` pins taken from the Go `core.DayArcTicks`
    /// arithmetic in `packages/shared/core/day_phase.go`: the solstice and
    /// equinox anchors, the quarter-year points, and the wrap past one full
    /// year (including a negative phase) that the sinusoid folds back.
    const DAY_ARC_PINS: [(f64, u16); 13] = [
        (0.0, 12_000),
        (0.125, 14_546),
        (0.25, 15_600),
        (0.375, 14_546),
        (0.5, 12_000),
        (0.625, 9_454),
        (0.75, 8_400),
        (0.875, 9_454),
        (1.0, 12_000),
        (-0.25, 8_400),
        (1.25, 15_600),
        (0.1, 14_116),
        (1.0 / 3.0, 15_118),
    ];

    /// `(world_time, offset, day_arc, phase)` pins taken from the Go
    /// `core.EffectiveDayPhase` arithmetic: the equinox identity (including
    /// the day wrap and the `u64` extreme), the summer and winter branch
    /// edges on either side of the arc, the extreme legal arcs, and every
    /// illegal arc shape returning zero.
    const EFFECTIVE_PHASE_PINS: [(u64, u16, u16, u16); 38] = [
        (0, 0, 12_000, 0),
        (1, 0, 12_000, 1),
        (12_999, 0, 12_000, 12_999),
        (13_000, 0, 12_000, 13_000),
        (23_999, 0, 12_000, 23_999),
        (24_000, 0, 12_000, 0),
        (u64::MAX, 0, 12_000, 15_615),
        (u64::MAX, 23_999, 12_000, 15_614),
        (23_999, 1, 12_000, 0),
        (0, 23_999, 12_000, 23_999),
        (0, 0, 15_600, 0),
        (7_800, 0, 15_600, 6_000),
        (15_599, 0, 15_600, 11_999),
        (15_600, 0, 15_600, 12_000),
        (15_601, 0, 15_600, 12_001),
        (19_800, 0, 15_600, 18_000),
        (23_999, 0, 15_600, 23_998),
        (0, 0, 8_400, 0),
        (4_200, 0, 8_400, 6_000),
        (8_399, 0, 8_400, 11_998),
        (8_400, 0, 8_400, 12_000),
        (8_401, 0, 8_400, 12_000),
        (12_000, 0, 8_400, 14_769),
        (23_999, 0, 8_400, 23_999),
        (0, 0, 2, 0),
        (1, 0, 2, 6_000),
        (2, 0, 2, 12_000),
        (23_999, 0, 2, 23_999),
        (0, 0, 24_000, 0),
        (12_000, 0, 24_000, 6_000),
        (23_999, 0, 24_000, 11_999),
        (0, 0, 23_998, 0),
        (23_997, 0, 23_998, 11_999),
        (23_998, 0, 23_998, 12_000),
        (23_999, 0, 23_998, 18_000),
        (12_345, 6_000, 0, 0),
        (12_345, 6_000, 1, 0),
        (12_345, 6_000, 12_001, 0),
    ];

    /// Illegal day arcs beyond the table above share the zero contract.
    const ILLEGAL_ARCS: [u16; 4] = [3, 23_999, 24_001, u16::MAX];

    /// `(world_time, offset, season_offset, phase)` pins taken from the Go
    /// `core.EffectiveDayPhaseAt` arithmetic: the summer and winter solstice
    /// anchors, the equinox identity at the year wrap, a seed-offset point
    /// whose arc moves between adjacent ticks, and the `u64`/`u32` extremes.
    const EFFECTIVE_PHASE_AT_PINS: [(u64, u16, u32, u16); 19] = [
        (0, 0, 0, 0),
        (72_000, 0, 0, 0),
        (72_000, 7_800, 0, 6_000),
        (72_000, 13_000, 0, 10_000),
        (72_000, 15_600, 0, 12_000),
        (72_000, 23_999, 0, 23_998),
        (216_000, 0, 0, 0),
        (216_000, 4_200, 0, 6_000),
        (216_000, 8_399, 0, 11_998),
        (216_000, 8_400, 0, 12_000),
        (216_000, 23_999, 0, 23_999),
        (0, 0, 14_818, 0),
        (89, 0, 14_818, 81),
        (0, 23_999, 0, 23_999),
        (24_000, 0, 0, 0),
        (u64::MAX, 0, 0, 13_588),
        (u64::MAX, 23_999, u32::MAX, 15_088),
        (1, 0, 287_999, 1),
        (288_000, 6_000, 0, 6_000),
    ];

    #[test]
    fn day_arc_derivation_matches_go_pins() {
        for (year_phase, day_arc) in DAY_ARC_PINS {
            assert_eq!(
                day_arc_ticks(year_phase),
                day_arc,
                "day_arc_ticks({year_phase})"
            );
        }
        for (world_time, offset, day_arc, phase) in EFFECTIVE_PHASE_PINS {
            assert_eq!(
                effective_day_phase(world_time, offset, day_arc),
                phase,
                "effective_day_phase({world_time}, {offset}, {day_arc})"
            );
        }
        for day_arc in ILLEGAL_ARCS {
            assert_eq!(
                effective_day_phase(12_345, 6_000, day_arc),
                0,
                "effective_day_phase(12345, 6000, {day_arc})"
            );
        }
        for (world_time, offset, season_offset, phase) in EFFECTIVE_PHASE_AT_PINS {
            assert_eq!(
                effective_day_phase_at(world_time, offset, season_offset),
                phase,
                "effective_day_phase_at({world_time}, {offset}, {season_offset})"
            );
        }
        // The extra combined pin below exercises a winter arc reached
        // through a season offset rather than a solstice world time.
        assert_eq!(effective_day_phase_at(144_000, 12_000, 72_000), 14_769);
    }
}
