//! Host-layer tick pacing at 20 TPS.
//!
//! Core exposes only [`crate::state::AuthorityState::advance_tick`] and knows
//! nothing about clocks. This module owns the replaceable host clock and the
//! overload policy from design.md (task 4.3): average rate ≤ 20 ticks/s; at
//! most one missed tick is carried over and run immediately; further missed
//! ticks are dropped; world time advances only on executed ticks (callers
//! step only when this scheduler says so).

use std::time::{Duration, Instant};

/// 20 ticks per second (matches Go `time.NewTicker(50 * time.Millisecond)`).
pub const TICK_PERIOD: Duration = Duration::from_millis(50);

/// Host-layer clock. Tests inject a manual clock; production uses wall time.
pub trait HostClock {
    fn now(&self) -> Instant;
}

/// Wall-clock host clock.
#[derive(Debug, Default)]
pub struct WallClock;

impl HostClock for WallClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Manually advanced clock for the 4.3 host-loop proofs.
#[derive(Debug)]
pub struct ManualClock {
    now: Instant,
}

impl ManualClock {
    pub fn new(start: Instant) -> Self {
        Self { now: start }
    }

    pub fn advance(&mut self, by: Duration) {
        self.now += by;
    }
}

impl HostClock for ManualClock {
    fn now(&self) -> Instant {
        self.now
    }
}

/// Decides how many production ticks to run for the current host instant.
#[derive(Debug)]
pub struct HostTickScheduler<C> {
    clock: C,
    /// Instant the next tick is due.
    next_due: Instant,
}

impl<C: HostClock> HostTickScheduler<C> {
    /// Arms the first tick one period after `clock.now()`, matching Go's
    /// `time.Ticker` (first fire after the interval, not immediately).
    pub fn new(clock: C) -> Self {
        let next_due = clock.now() + TICK_PERIOD;
        Self { clock, next_due }
    }

    pub fn clock(&self) -> &C {
        &self.clock
    }

    pub fn clock_mut(&mut self) -> &mut C {
        &mut self.clock
    }

    /// How many ticks to execute now (0, 1, or 2). Updates the schedule.
    ///
    /// Policy: on time → 1; one period late → 2 (due + one catch-up);
    /// further lateness drops extra periods by snapping `next_due` forward
    /// without running them.
    pub fn poll(&mut self) -> u8 {
        let now = self.clock.now();
        if now < self.next_due {
            return 0;
        }
        let late = now.duration_since(self.next_due);
        let periods_behind = (late.as_nanos() / TICK_PERIOD.as_nanos()) as u64;
        let to_run: u8 = if periods_behind == 0 { 1 } else { 2 };
        let advance_periods = periods_behind + 1;
        self.next_due += TICK_PERIOD * advance_periods as u32;
        to_run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> Instant {
        Instant::now()
    }

    #[test]
    fn quiet_until_first_period_then_one_tick() {
        let mut sched = HostTickScheduler::new(ManualClock::new(origin()));
        assert_eq!(sched.poll(), 0);
        sched.clock_mut().advance(TICK_PERIOD);
        assert_eq!(sched.poll(), 1);
        assert_eq!(sched.poll(), 0, "same instant does not double-fire");
    }

    #[test]
    fn average_rate_does_not_exceed_twenty_per_second() {
        let mut sched = HostTickScheduler::new(ManualClock::new(origin()));
        let mut executed = 0u64;
        for _ in 0..1000 {
            sched.clock_mut().advance(Duration::from_millis(1));
            executed += u64::from(sched.poll());
        }
        assert!(
            executed <= 20,
            "executed {executed} ticks in 1s, exceeds 20 TPS"
        );
        assert!(
            executed >= 19,
            "executed {executed} ticks in 1s, expected ~20"
        );
    }

    #[test]
    fn one_missed_tick_is_carried_over_further_are_dropped() {
        let mut sched = HostTickScheduler::new(ManualClock::new(origin()));
        sched.clock_mut().advance(TICK_PERIOD);
        assert_eq!(sched.poll(), 1);
        // Jump five periods late without polling.
        sched.clock_mut().advance(TICK_PERIOD * 5);
        assert_eq!(sched.poll(), 2, "at most one catch-up plus the due tick");
        assert_eq!(sched.poll(), 0);
    }

    #[test]
    fn dropped_ticks_are_not_caught_up_later() {
        let mut sched = HostTickScheduler::new(ManualClock::new(origin()));
        sched.clock_mut().advance(TICK_PERIOD);
        assert_eq!(sched.poll(), 1);
        sched.clock_mut().advance(TICK_PERIOD * 10);
        assert_eq!(sched.poll(), 2);
        sched.clock_mut().advance(TICK_PERIOD);
        assert_eq!(sched.poll(), 1, "no backlog remains after the drop");
    }
}
