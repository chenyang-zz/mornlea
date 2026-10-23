//! Gameplay host assembly: 20 TPS pacing over the production tick.
//!
//! The binary only parses flags; this module owns the host clock and the run
//! loop. DiskStore enablement, TCP accept, and companion startup are wired as
//! the executable-entry slices land. Configuration-file consumption and fluid
//! rescan parity stay in later PRs.

use std::time::Duration;

use super::host_tick::{HostClock, HostTickScheduler, WallClock};

/// Runs `on_tick` once per scheduled host tick until `stop` returns true.
///
/// Sleeps a short slice when idle so a wall-clock host does not busy-spin.
/// Tests inject [`super::host_tick::ManualClock`] and pass `idle_sleep: None`,
/// advancing the clock themselves between iterations via [`HostTickScheduler::clock_mut`].
pub fn run_paced_loop<C: HostClock>(
    mut scheduler: HostTickScheduler<C>,
    mut stop: impl FnMut(&mut HostTickScheduler<C>) -> bool,
    mut on_tick: impl FnMut(),
    idle_sleep: Option<Duration>,
) {
    while !stop(&mut scheduler) {
        let due = scheduler.poll();
        if due == 0 {
            if let Some(pause) = idle_sleep {
                std::thread::sleep(pause);
            }
            continue;
        }
        for _ in 0..due {
            on_tick();
        }
    }
}

/// Production wall-clock loop at 20 TPS with a 1 ms idle pause.
pub fn run_wall_clock_loop(mut stop: impl FnMut() -> bool, on_tick: impl FnMut()) {
    run_paced_loop(
        HostTickScheduler::new(WallClock),
        |_| stop(),
        on_tick,
        Some(Duration::from_millis(1)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::ServerLimits;
    use crate::runtime::host_tick::{ManualClock, TICK_PERIOD};
    use crate::state::AuthorityState;
    use std::time::Instant;

    fn empty_authority() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap()
    }

    #[test]
    fn paced_loop_advances_world_time_only_on_executed_ticks() {
        let mut authority = empty_authority();
        let mut sched = HostTickScheduler::new(ManualClock::new(Instant::now()));
        let before = authority.next_tick();
        sched.clock_mut().advance(TICK_PERIOD);
        assert_eq!(sched.poll(), 1);
        authority.advance_tick_full().unwrap();
        assert_eq!(authority.next_tick(), before + 1);
        sched.clock_mut().advance(TICK_PERIOD * 5);
        let due = sched.poll();
        assert_eq!(due, 2);
        for _ in 0..due {
            authority.advance_tick_full().unwrap();
        }
        assert_eq!(
            authority.next_tick(),
            before + 3,
            "five late periods execute two ticks; dropped periods skip world time"
        );
    }

    #[test]
    fn run_paced_loop_executes_two_on_time_ticks() {
        use std::cell::Cell;
        let ticks = Cell::new(0u8);
        let periods = Cell::new(0u8);
        run_paced_loop(
            HostTickScheduler::new(ManualClock::new(Instant::now())),
            |sched| {
                if ticks.get() >= 2 {
                    return true;
                }
                periods.set(periods.get() + 1);
                sched.clock_mut().advance(TICK_PERIOD);
                false
            },
            || ticks.set(ticks.get() + 1),
            None,
        );
        assert_eq!(ticks.get(), 2);
        assert!(periods.get() >= 2);
    }

    #[test]
    fn wall_period_constant_is_fifty_ms() {
        assert_eq!(TICK_PERIOD, Duration::from_millis(50));
    }
}
