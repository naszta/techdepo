//! Per-order simulated clock.
//!
//! Each order has a virtual clock: an offset from real time. One order can sit at day 2 while
//! another sits at day 6 in the same demo. The clock only moves forward.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

use crate::calendar::{WorkCalendar, next_monday_9am};

/// Offset of an order's simulated time from real time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct VirtualClock {
    offset_secs: i64,
}

/// A demo control that moves an order's simulated time forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClockAdvance {
    /// `+1 day`.
    Day,
    /// `+1 workday`: the same time of day on the next workday.
    Workday,
    /// `→ next Monday 09:00`.
    NextMonday9am,
}

impl VirtualClock {
    /// A clock running this many seconds ahead of real time.
    pub const fn from_offset_secs(offset_secs: i64) -> Self {
        Self { offset_secs }
    }

    /// Seconds ahead of real time (persisted as `virtual_clock_offset`).
    pub const fn offset_secs(self) -> i64 {
        self.offset_secs
    }

    /// The offset as a duration.
    pub fn offset(self) -> Duration {
        Duration::seconds(self.offset_secs)
    }

    /// Simulated now, given the real (journaled) now.
    pub fn now(self, real_now: OffsetDateTime) -> OffsetDateTime {
        real_now.saturating_add(self.offset())
    }

    /// Moves simulated time forward by `by`. Negative durations are ignored.
    pub fn advance_by(&mut self, by: Duration) {
        if by.is_positive() {
            self.offset_secs = self.offset_secs.saturating_add(by.whole_seconds());
        }
    }

    /// Moves simulated time forward to `target`. Targets in the simulated past are ignored.
    pub fn advance_to(&mut self, real_now: OffsetDateTime, target: OffsetDateTime) {
        self.advance_by(target - self.now(real_now));
    }

    /// Applies a demo control and returns the new simulated now.
    pub fn apply(
        &mut self,
        real_now: OffsetDateTime,
        advance: ClockAdvance,
        calendar: &WorkCalendar,
    ) -> OffsetDateTime {
        let now = self.now(real_now);
        let target = match advance {
            ClockAdvance::Day => now.saturating_add(Duration::days(1)),
            ClockAdvance::Workday => now.replace_date(calendar.add_workdays(now.date(), 1)),
            ClockAdvance::NextMonday9am => next_monday_9am(now),
        };
        self.advance_to(real_now, target);
        self.now(real_now)
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    #[test]
    fn controls_move_forward() {
        let real = datetime!(2026-10-16 14:00 UTC); // Friday
        let cal = WorkCalendar::new();
        let mut clock = VirtualClock::default();
        assert_eq!(clock.now(real), real);

        assert_eq!(
            clock.apply(real, ClockAdvance::Day, &cal),
            datetime!(2026-10-17 14:00 UTC)
        );
        assert_eq!(
            clock.apply(real, ClockAdvance::Workday, &cal),
            datetime!(2026-10-19 14:00 UTC)
        );
        assert_eq!(
            clock.apply(real, ClockAdvance::NextMonday9am, &cal),
            datetime!(2026-10-26 09:00 UTC)
        );
        assert_eq!(
            clock.offset_secs(),
            (datetime!(2026-10-26 09:00 UTC) - real).whole_seconds()
        );
    }

    #[test]
    fn never_goes_backwards() {
        let real = datetime!(2026-10-16 14:00 UTC);
        let mut clock = VirtualClock::from_offset_secs(3600);
        clock.advance_to(real, real);
        clock.advance_by(Duration::hours(-5));
        assert_eq!(clock.offset_secs(), 3600);
    }
}
