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
/// `core.yearIndex`.
fn year_index(world_time: u64, season_offset: u32) -> u64 {
    (world_time % YEAR_TICKS + u64::from(season_offset) % YEAR_TICKS) % YEAR_TICKS
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
