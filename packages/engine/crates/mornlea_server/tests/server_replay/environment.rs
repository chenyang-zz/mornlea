//! Environment transition replay: world time, season, and the weather clock.
//!
//! Every numeric literal below is computed by the accepted Go runtime, not
//! chosen here. Oracle sources:
//!
//! - Weather clock constants, dice salts, and the fixed segment distribution:
//!   `packages/server/sim/runtime/weather.go` (`advanceWeatherClock`,
//!   `rollWeatherSegment`, `rollWeatherDuration`, `weatherHash`). The expiry
//!   and restore values were read off a real `runtime.Engine` driven at the
//!   same seed, world time, and completed tick as each case.
//! - Restore-zero rule: `packages/server/sim/runtime/engine.go`
//!   (`RestoreWeather`) with `weather_restore_test.go`
//!   (`TestRestoreWeatherZeroRemainingPreservesKind`).
//! - Season derivation and the seed-0 golden anchor: `packages/shared/core/
//!   season.go` (`SeasonAt`, `SeasonProgressAt`) and `packages/server/sim/
//!   runtime/season_test.go` (`testSeasonOffset` = 158435,
//!   `testSolsticeWorldTime` = 201565, `testSolsticeDayPhaseOffset` = 22235).
//! - The absolute clock saturates instead of wrapping per the frozen
//!   `environment::step_restore_boundary` row, which strengthens the Go
//!   `worldTime.Add(1)` wrap at the `u64` edge.

use std::panic::{AssertUnwindSafe, catch_unwind, set_hook, take_hook};

use mornlea_domain::{
    Command, CommandEnvelope, CommandEnvelopeParts, PlayerId, Season, Weather, WorldState,
    WorldStateParts,
};
use mornlea_protocol::{LoginStart, PlayIntent, admit_login};
use mornlea_server::contracts::{
    EnvironmentState, Expected, Fixture, FixtureInput, FixtureState, Observed, PhaseReport,
    RuleCall, RuleEffect, RulePhase, RuleReject, RuleTunables, ScheduledInput, ServerError,
    SleepState, TickBudget, TickCounters, TransportKind, WorkState,
};
use mornlea_server::rules::environment;
use mornlea_server::state::{AuthorityState, TickContext};

use super::assert_expected;

/// Season start offset for seed 0: the `core` golden anchor pinned by the Go
/// season test fixture (`testSeasonOffset`).
const SEED_ZERO_SEASON_OFFSET: u32 = 158_435;
/// Display offset a settled sleep leaves behind, the Go season test's
/// `testSolsticeDayPhaseOffset`, used here as the pass-through sleep offset.
const SETTLED_SLEEP_OFFSET: u16 = 22_235;
/// Go-deterministic weather segment rolled for seed 0 at completed tick 1:
/// `rollWeatherSegment(0, 1)` returns `(Clear, 146779)`; 146779 is inside the
/// clear interval `[12000, 180000]`.
const EXPIRY_SUCCESSOR_KIND: Weather = Weather::Clear;
const EXPIRY_SUCCESSOR_REMAINING: u32 = 146_779;
/// `RestoreWeather(42, (Rain, 0))` regenerates 9025 ticks and the first tick
/// decrements once, mirroring `TestRestoreWeatherZeroRemainingPreservesKind`.
const RESTORE_RAIN_SEED_42_REMAINING: u32 = 9_024;
/// `RestoreWeather(42, (Clear, 0))` regenerates 34233 ticks, then the first
/// tick decrements to 34232.
const RESTORE_CLEAR_SEED_42_REMAINING: u32 = 34_232;
/// Clear segment interval, `weatherClearMinTicks`..`weatherClearMaxTicks`.
const CLEAR_MIN_TICKS: u32 = 12_000;
const CLEAR_MAX_TICKS: u32 = 180_000;
/// Rain and thunder segment interval from the same Go constants.
const PRECIPITATION_MIN_TICKS: u32 = 3_600;
const PRECIPITATION_MAX_TICKS: u32 = 15_600;

/// One completed environment tick: the baseline the reducer freezes at tick
/// start, the call, and everything the harness observes afterwards.
struct EnvironmentRun {
    observed: Observed,
    report: Result<PhaseReport, ServerError>,
    final_environment: Option<EnvironmentState>,
    published_world: WorldState,
}

/// Builds the authority and context exactly like the harness `run_phase`,
/// stages the tick-start climate snapshot the reducer owns, then invokes the
/// environment provider once. Scheduled fixture inputs queue on the same
/// authority first, mirroring the harness `replay` loop.
fn run_environment(
    fixture: &Fixture,
    baseline: Option<EnvironmentState>,
    call: RuleCall<'_>,
) -> EnvironmentRun {
    let mut authority =
        AuthorityState::try_new(super::limits(), fixture.seed as i64).expect("authority");
    for input in &fixture.schedule {
        match input {
            FixtureInput::Human(scheduled) => {
                let _ = authority.submit(scheduled.session, scheduled.intent.clone());
            }
            FixtureInput::Companion { value, .. } => {
                let _ = authority.submit_companion(value.clone());
            }
            FixtureInput::Internal { .. } => {}
        }
    }
    let budget = fixture
        .budgets
        .first()
        .map(|(_, budget)| *budget)
        .unwrap_or_else(TickBudget::full);
    let mut context = TickContext::from_fixture(&mut authority, &fixture.initial, budget);
    if let Some(baseline) = baseline {
        context
            .stage(RuleEffect::Environment(baseline))
            .expect("tick-start environment snapshot stages");
    }
    let report = environment::run(&mut context, call);
    let failed = report.is_err();
    let final_environment = context.read().environment().cloned();
    let state = context.snapshot_state(fixture.initial.world);
    EnvironmentRun {
        published_world: state.world,
        observed: Observed {
            events: context.events().to_vec(),
            counters: TickCounters {
                executed_tick: context.read().tick(),
                ..TickCounters::default()
            },
            state_sha256: super::canonical_state_sha256(&state),
            class: if failed {
                Some(RuleReject::StaleObservation)
            } else {
                None
            },
        },
        report,
        final_environment,
    }
}

/// Canonical fixture state for one world record: the hash folds only the
/// world scalars, so this is the exact oracle shape the harness hashes.
fn initial_state(world: WorldState) -> FixtureState {
    FixtureState {
        runtime: Vec::new(),
        actors: Vec::new(),
        chunks: Vec::new(),
        inventories: Vec::new(),
        containers: Vec::new(),
        work: WorkState::default(),
        sleep: SleepState {
            beds: Vec::new(),
            day_phase_offset: 0,
            pending_offset: None,
        },
        projectiles: Vec::new(),
        drops: Vec::new(),
        world,
    }
}

/// Publication world record at sea level; the temperature is a publication
/// scalar this phase never writes.
fn world(
    world_time_ticks: u64,
    day_phase_offset: u16,
    weather: Weather,
    season: Season,
    season_progress: u8,
) -> WorldState {
    WorldState::try_new(WorldStateParts {
        day_phase_offset,
        world_time_ticks,
        weather,
        season,
        season_progress,
        temperature: 0,
    })
    .expect("fixture world")
}

/// Frozen tick-start climate snapshot for seed 0 with the golden season
/// offset; per-case fields override the clock and weather.
fn baseline(
    world_time: u64,
    day_phase_offset: u16,
    weather: Weather,
    weather_remaining: u32,
) -> EnvironmentState {
    EnvironmentState {
        seed: 0,
        next_tick: 0,
        world_time,
        day_phase_offset,
        season_offset: SEED_ZERO_SEASON_OFFSET,
        weather,
        weather_remaining,
        difficulty: 0,
        tunables: RuleTunables::source_defaults(),
    }
}

/// The seed-42 restore fixture: its weather regeneration is pinned by the Go
/// restore tests at this seed. The clock sits at the same boundary tick as
/// the seed-0 cases; only the seed and weather differ.
fn restored_baseline(weather: Weather, weather_remaining: u32) -> EnvironmentState {
    EnvironmentState {
        seed: 42,
        ..baseline(23999, 0, weather, weather_remaining)
    }
}

fn call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::EnvironmentEnd,
        actor: None,
        command: None,
        internal: None,
    }
}

/// The expected outcome for one tick ending on `world`: no events, no
/// rejection, harness-default counters.
fn expected(world: WorldState) -> Expected {
    Expected {
        events: Vec::new(),
        state_sha256: super::canonical_state_sha256(&initial_state(world)),
        class: None,
        counters: TickCounters::default(),
    }
}

fn fixture(world: WorldState, schedule: Vec<FixtureInput>) -> Fixture {
    Fixture {
        seed: 0,
        initial: initial_state(world),
        schedule,
        expected: expected(world),
        budgets: Vec::new(),
    }
}

fn restored_fixture(world: WorldState) -> Fixture {
    Fixture {
        seed: 42,
        initial: initial_state(world),
        schedule: Vec::new(),
        expected: expected(world),
        budgets: Vec::new(),
    }
}

/// Runs the assertion and reports whether it panicked, with the panic hook
/// silenced so the expected oracle failure prints no backtrace.
fn panics(assert: impl FnOnce()) -> bool {
    let previous = take_hook();
    set_hook(Box::new(|_| {}));
    let panicked = catch_unwind(AssertUnwindSafe(assert)).is_err();
    set_hook(previous);
    panicked
}

/// The frozen row end to end: world time 23999 advances to 24000 exactly once
/// (second tick to 24001), the absolute clock never wraps at the `u64` edge,
/// a settled sleep offset moves only `day_phase_offset`, weather with
/// remaining 1 expires into the Go-deterministic successor, and a
/// restore-to-zero weather preserves its kind and regenerates duration.
#[test]
fn step_restore_boundary() {
    // Day-boundary tick with a settled sleep display offset: the offset
    // passes through and the absolute clock advances exactly one tick.
    let day = run_environment(
        &fixture(
            world(
                23999,
                SETTLED_SLEEP_OFFSET,
                Weather::Rain,
                Season::Autumn,
                136,
            ),
            Vec::new(),
        ),
        Some(baseline(23999, SETTLED_SLEEP_OFFSET, Weather::Rain, 5000)),
        call(),
    );
    assert_eq!(
        day.report,
        Ok(PhaseReport {
            examined: 1,
            applied: 1,
            carried: 0,
            rejected: 0,
        })
    );
    let advanced = day.final_environment.as_ref().expect("advanced record");
    assert_eq!(advanced.next_tick, 1);
    assert_eq!(advanced.world_time, 24000);
    assert_eq!(advanced.day_phase_offset, SETTLED_SLEEP_OFFSET);
    assert_eq!(advanced.weather, Weather::Rain);
    assert_eq!(advanced.weather_remaining, 4999);
    assert_eq!(
        day.published_world,
        world(
            24000,
            SETTLED_SLEEP_OFFSET,
            Weather::Rain,
            Season::Autumn,
            136
        )
    );
    assert_expected(
        &day.observed,
        &expected(world(
            24000,
            SETTLED_SLEEP_OFFSET,
            Weather::Rain,
            Season::Autumn,
            136,
        )),
    );

    // The second tick advances exactly once more: 24000 to 24001, continuing
    // the carried environment record from the first tick.
    let second = run_environment(
        &fixture(
            world(
                24000,
                SETTLED_SLEEP_OFFSET,
                Weather::Rain,
                Season::Autumn,
                136,
            ),
            Vec::new(),
        ),
        day.final_environment.clone(),
        call(),
    );
    let continued = second.final_environment.as_ref().expect("continued record");
    assert_eq!(continued.next_tick, 2);
    assert_eq!(continued.world_time, 24001);
    assert_eq!(continued.day_phase_offset, SETTLED_SLEEP_OFFSET);
    assert_eq!(continued.weather_remaining, 4998);
    assert_eq!(
        second.published_world,
        world(
            24001,
            SETTLED_SLEEP_OFFSET,
            Weather::Rain,
            Season::Autumn,
            136
        )
    );
    assert_expected(
        &second.observed,
        &expected(world(
            24001,
            SETTLED_SLEEP_OFFSET,
            Weather::Rain,
            Season::Autumn,
            136,
        )),
    );

    // The absolute clock saturates instead of wrapping: one more tick at the
    // `u64` maximum keeps the maximum and never folds back to zero. Season
    // derivation at the maximum is Winter 192 (Go `SeasonAt`/`SeasonProgressAt`
    // at `u64::MAX` with the golden offset).
    let edge = run_environment(
        &fixture(
            world(u64::MAX, 0, Weather::Rain, Season::Winter, 192),
            Vec::new(),
        ),
        Some(baseline(u64::MAX, 0, Weather::Rain, 5000)),
        call(),
    );
    let saturated = edge.final_environment.as_ref().expect("saturated record");
    assert_eq!(saturated.world_time, u64::MAX);
    assert_eq!(saturated.next_tick, 1);
    assert_eq!(
        edge.published_world,
        world(u64::MAX, 0, Weather::Rain, Season::Winter, 192)
    );
    assert_expected(
        &edge.observed,
        &expected(world(u64::MAX, 0, Weather::Rain, Season::Winter, 192)),
    );

    // Weather with remaining 1 expires: every expiring kind rolls the same
    // Go-deterministic successor for seed 0 at completed tick 1, because the
    // Go dice re-roll the whole segment and never consult the old kind. The
    // successor stays inside the clear interval, mirroring
    // `TestWeatherExpiryRollsLegalSegment`.
    for kind in [Weather::Clear, Weather::Rain, Weather::Thunder] {
        let expired = run_environment(
            &fixture(world(23999, 0, kind, Season::Autumn, 136), Vec::new()),
            Some(baseline(23999, 0, kind, 1)),
            call(),
        );
        let rolled = expired.final_environment.as_ref().expect("rolled record");
        assert_eq!(rolled.weather, EXPIRY_SUCCESSOR_KIND);
        assert_eq!(rolled.weather_remaining, EXPIRY_SUCCESSOR_REMAINING);
        assert!(
            rolled.weather_remaining >= CLEAR_MIN_TICKS
                && rolled.weather_remaining <= CLEAR_MAX_TICKS,
            "successor clear segment outside the Go interval"
        );
        assert_eq!(
            expired.published_world,
            world(24000, 0, EXPIRY_SUCCESSOR_KIND, Season::Autumn, 136)
        );
        assert_expected(
            &expired.observed,
            &expected(world(24000, 0, EXPIRY_SUCCESSOR_KIND, Season::Autumn, 136)),
        );
    }

    // Restore of zero preserves the kind and regenerates the duration per the
    // Go restore rule, then this tick's single decrement applies: (Rain, 0)
    // at seed 42 lands on 9024, (Clear, 0) on 34232, both inside their kind's
    // interval.
    for (kind, regenerated, min, max) in [
        (
            Weather::Rain,
            RESTORE_RAIN_SEED_42_REMAINING,
            PRECIPITATION_MIN_TICKS,
            PRECIPITATION_MAX_TICKS,
        ),
        (
            Weather::Clear,
            RESTORE_CLEAR_SEED_42_REMAINING,
            CLEAR_MIN_TICKS,
            CLEAR_MAX_TICKS,
        ),
    ] {
        let restored = run_environment(
            &restored_fixture(world(23999, 0, kind, Season::Autumn, 136)),
            Some(restored_baseline(kind, 0)),
            call(),
        );
        let migrated = restored
            .final_environment
            .as_ref()
            .expect("restored record");
        assert_eq!(migrated.weather, kind, "restore of zero preserves the kind");
        assert_eq!(migrated.weather_remaining, regenerated);
        assert!(
            migrated.weather_remaining >= min && migrated.weather_remaining <= max,
            "regenerated segment outside the Go interval"
        );
        assert_eq!(
            restored.published_world,
            world(24000, 0, kind, Season::Autumn, 136)
        );
        assert_expected(
            &restored.observed,
            &expected(world(24000, 0, kind, Season::Autumn, 136)),
        );
    }
}

/// Crossing the day boundary and the season boundary updates the publication
/// fields exactly once; a mutated expectation (time advanced twice) fails the
/// harness hash check, proving the oracle bites.
#[test]
fn day_and_season_boundary_once() {
    // Day boundary: one tick publishes the wrapped day exactly once.
    let day = run_environment(
        &fixture(
            world(
                23999,
                SETTLED_SLEEP_OFFSET,
                Weather::Rain,
                Season::Autumn,
                136,
            ),
            Vec::new(),
        ),
        Some(baseline(23999, SETTLED_SLEEP_OFFSET, Weather::Rain, 5000)),
        call(),
    );
    let day_expected = expected(world(
        24000,
        SETTLED_SLEEP_OFFSET,
        Weather::Rain,
        Season::Autumn,
        136,
    ));
    assert_expected(&day.observed, &day_expected);
    // The oracle bites: a wrong intermediate publication whose time advanced
    // twice must fail the hash check.
    let doubled = Expected {
        state_sha256: expected(world(
            24001,
            SETTLED_SLEEP_OFFSET,
            Weather::Rain,
            Season::Autumn,
            136,
        ))
        .state_sha256,
        ..day_expected.clone()
    };
    assert!(panics(|| assert_expected(&day.observed, &doubled)));

    // Season boundary at the core golden anchor: seed 0 offset 158435 crosses
    // Spring 255 into Summer 0 at world time 201565, the Go season fixture's
    // solstice tick with its display offset 22235 and rain 5000. The season
    // offset itself is untouched, and the weather decrements once.
    let season = run_environment(
        &fixture(
            world(
                201564,
                SETTLED_SLEEP_OFFSET,
                Weather::Rain,
                Season::Spring,
                255,
            ),
            Vec::new(),
        ),
        Some(baseline(201564, SETTLED_SLEEP_OFFSET, Weather::Rain, 5000)),
        call(),
    );
    let advanced = season.final_environment.as_ref().expect("advanced record");
    assert_eq!(advanced.world_time, 201565);
    assert_eq!(advanced.season_offset, SEED_ZERO_SEASON_OFFSET);
    assert_eq!(advanced.day_phase_offset, SETTLED_SLEEP_OFFSET);
    assert_eq!(advanced.weather_remaining, 4999);
    let season_expected = expected(world(
        201565,
        SETTLED_SLEEP_OFFSET,
        Weather::Rain,
        Season::Summer,
        0,
    ));
    assert_eq!(
        season.published_world,
        world(
            201565,
            SETTLED_SLEEP_OFFSET,
            Weather::Rain,
            Season::Summer,
            0
        )
    );
    assert_expected(&season.observed, &season_expected);
    // A single call updated the season once: an expectation for a second
    // season-advance tick fails the hash.
    let twice = Expected {
        state_sha256: expected(world(
            201566,
            SETTLED_SLEEP_OFFSET,
            Weather::Rain,
            Season::Summer,
            0,
        ))
        .state_sha256,
        ..season_expected.clone()
    };
    assert!(panics(|| assert_expected(&season.observed, &twice)));
}

/// A queued command arriving at the boundary tick does not perturb the
/// environment transition: the batch phase reads no commands, refuses
/// command-shaped and wrong-phase calls without effect, and refuses to act
/// without the reducer's tick-start snapshot.
#[test]
fn command_at_boundary_unchanged() {
    // One real session with one queued sequenced command, allocated on a
    // scratch authority; fresh authorities number sessions from 1, so the
    // session identity is valid on every run's own authority.
    let mut scratch = AuthorityState::try_new(super::limits(), 0).expect("scratch authority");
    let mut bytes = [0u8; 16];
    bytes[0] = 9;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = PlayerId::try_from_bytes(bytes).expect("player id");
    let start = LoginStart::new(id, "environment", 8).expect("login start");
    let inbound =
        LoginStart::decode_inbound(&start.encode().expect("login encode")).expect("login decode");
    let session = scratch
        .allocate(
            admit_login(inbound).expect("admit login"),
            TransportKind::Memory,
        )
        .expect("session");
    let scheduled = FixtureInput::Human(ScheduledInput {
        earliest_tick: 0,
        session,
        intent: PlayIntent::Sequenced {
            sequence: 1,
            command: Command::CloseContainer,
        },
    });
    let boundary_world = world(
        23999,
        SETTLED_SLEEP_OFFSET,
        Weather::Rain,
        Season::Autumn,
        136,
    );
    let with_command = fixture(boundary_world, vec![scheduled]);
    let without_command = fixture(boundary_world, Vec::new());

    let with = run_environment(
        &with_command,
        Some(baseline(23999, SETTLED_SLEEP_OFFSET, Weather::Rain, 5000)),
        call(),
    );
    let without = run_environment(
        &without_command,
        Some(baseline(23999, SETTLED_SLEEP_OFFSET, Weather::Rain, 5000)),
        call(),
    );
    assert_eq!(
        with.report,
        Ok(PhaseReport {
            examined: 1,
            applied: 1,
            carried: 0,
            rejected: 0,
        })
    );
    assert_eq!(with.observed, without.observed);
    assert_eq!(with.report, without.report);
    assert_eq!(with.final_environment, without.final_environment);
    assert_eq!(
        with.final_environment
            .as_ref()
            .expect("advanced record")
            .world_time,
        24000
    );
    assert_eq!(
        with.published_world,
        world(
            24000,
            SETTLED_SLEEP_OFFSET,
            Weather::Rain,
            Season::Autumn,
            136
        )
    );
    assert_expected(
        &with.observed,
        &expected(world(
            24000,
            SETTLED_SLEEP_OFFSET,
            Weather::Rain,
            Season::Autumn,
            136,
        )),
    );

    // The batch phase refuses a command-shaped call without any effect: the
    // staged baseline survives untouched and nothing publishes.
    let envelope = CommandEnvelope::try_new(CommandEnvelopeParts {
        tick: 0,
        session: session.get(),
        sequence: 1,
        arrival_index: 0,
        command: Command::CloseContainer,
    })
    .expect("command envelope");
    let staged = baseline(23999, SETTLED_SLEEP_OFFSET, Weather::Rain, 5000);
    let refused = run_environment(
        &with_command,
        Some(staged.clone()),
        RuleCall {
            phase: RulePhase::EnvironmentEnd,
            actor: None,
            command: Some(&envelope),
            internal: None,
        },
    );
    assert_eq!(
        refused.report,
        Ok(PhaseReport {
            examined: 0,
            applied: 0,
            carried: 0,
            rejected: 0
        })
    );
    assert_eq!(refused.final_environment.as_ref(), Some(&staged));
    assert_eq!(refused.published_world, boundary_world);
    assert_expected(&refused.observed, &expected(boundary_world));

    // A wrong phase is likewise refused without effect.
    let wrong_phase = run_environment(
        &with_command,
        Some(staged.clone()),
        RuleCall {
            phase: RulePhase::Publish,
            actor: None,
            command: None,
            internal: None,
        },
    );
    assert_eq!(
        wrong_phase.report,
        Ok(PhaseReport {
            examined: 0,
            applied: 0,
            carried: 0,
            rejected: 0
        })
    );
    assert_eq!(wrong_phase.final_environment.as_ref(), Some(&staged));
    assert_eq!(wrong_phase.published_world, boundary_world);
    assert_expected(&wrong_phase.observed, &expected(boundary_world));

    // Without the reducer's tick-start climate snapshot the provider refuses
    // to act at all: no advanced record, no publication, typed error, and the
    // harness classifies the failed call as `StaleObservation`.
    let unsnapshotted = run_environment(&with_command, None, call());
    assert_eq!(
        unsnapshotted.report,
        Err(ServerError::Internal {
            invariant: "environment snapshot",
        })
    );
    assert_eq!(unsnapshotted.final_environment, None);
    assert_eq!(unsnapshotted.published_world, boundary_world);
    let refused_expected = Expected {
        class: Some(RuleReject::StaleObservation),
        ..expected(boundary_world)
    };
    assert_expected(&unsnapshotted.observed, &refused_expected);
}
