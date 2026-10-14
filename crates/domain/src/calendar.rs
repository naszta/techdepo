//! Workday calendar: Monday–Friday, minus a configurable holiday list (empty by default; set
//! per org in [`OrgSettings::holidays`](crate::OrgSettings::holidays)).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use time::macros::time;
use time::{Date, Duration, OffsetDateTime, Time, Weekday};

/// Which days count as workdays. Serializes as its holiday list (`["2026-12-25", …]`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkCalendar {
    #[serde(with = "iso_dates")]
    holidays: BTreeSet<Date>,
}

impl WorkCalendar {
    /// Monday–Friday with no holidays.
    pub fn new() -> Self {
        Self::default()
    }

    /// Monday–Friday minus `holidays`.
    pub fn with_holidays(holidays: impl IntoIterator<Item = Date>) -> Self {
        Self {
            holidays: holidays.into_iter().collect(),
        }
    }

    /// The holidays, in date order.
    pub fn holidays(&self) -> impl Iterator<Item = Date> + '_ {
        self.holidays.iter().copied()
    }

    /// Whether `date` is a workday.
    pub fn is_workday(&self, date: Date) -> bool {
        !matches!(date.weekday(), Weekday::Saturday | Weekday::Sunday)
            && !self.holidays.contains(&date)
    }

    /// Number of workdays after `from`, up to and including `to`.
    ///
    /// This is "full workdays since": shipped on a Friday and asked on the next Monday gives 1.
    /// Returns 0 when `to` is not after `from`.
    pub fn workdays_between(&self, from: Date, to: Date) -> u32 {
        let mut count = 0;
        let mut day = from;
        while day < to {
            let Some(next) = day.next_day() else { break };
            day = next;
            if self.is_workday(day) {
                count += 1;
            }
        }
        count
    }

    /// [`workdays_between`](Self::workdays_between) for two instants, compared by calendar date
    /// in their own offsets. Pass both in the same offset.
    pub fn workdays_since(&self, since: OffsetDateTime, now: OffsetDateTime) -> u32 {
        self.workdays_between(since.date(), now.date())
    }

    /// The `n`-th workday after `from` (`n = 0` returns `from` unchanged).
    ///
    /// Saturates at the last representable date.
    pub fn add_workdays(&self, from: Date, n: u32) -> Date {
        let mut day = from;
        let mut remaining = n;
        while remaining > 0 {
            let Some(next) = day.next_day() else { break };
            day = next;
            if self.is_workday(day) {
                remaining -= 1;
            }
        }
        day
    }
}

/// serde for a collection of dates as `YYYY-MM-DD` strings (`time`'s own serde writes tuples).
pub mod iso_dates {
    use serde::de::Error as _;
    use serde::ser::SerializeSeq as _;
    use serde::{Deserialize, Deserializer, Serializer};
    use time::Date;
    use time::format_description::BorrowedFormatItem;
    use time::macros::format_description;

    const FORMAT: &[BorrowedFormatItem<'_>] = format_description!("[year]-[month]-[day]");

    /// Writes the dates as a list of `YYYY-MM-DD` strings.
    pub fn serialize<'a, S, I>(dates: I, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        I: IntoIterator<Item = &'a Date>,
    {
        let mut seq = serializer.serialize_seq(None)?;
        for date in dates {
            let text = date.format(FORMAT).map_err(serde::ser::Error::custom)?;
            seq.serialize_element(&text)?;
        }
        seq.end()
    }

    /// Reads a list of `YYYY-MM-DD` strings.
    pub fn deserialize<'de, D, C>(deserializer: D) -> Result<C, D::Error>
    where
        D: Deserializer<'de>,
        C: FromIterator<Date>,
    {
        Vec::<String>::deserialize(deserializer)?
            .iter()
            .map(|text| Date::parse(text, FORMAT).map_err(D::Error::custom))
            .collect()
    }
}

/// The first Monday 09:00 strictly after `now`, in `now`'s offset.
pub fn next_monday_9am(now: OffsetDateTime) -> OffsetDateTime {
    const NINE: Time = time!(9:00);

    let today = now.date();
    let days_ahead = i64::from(today.weekday().number_days_from_monday());
    let this_week = today - Duration::days(days_ahead);
    let candidate = this_week.with_time(NINE).assume_offset(now.offset());
    if candidate > now {
        candidate
    } else {
        candidate + Duration::weeks(1)
    }
}

/// The most recent `weekday` strictly before `date` (e.g. "last Friday").
pub fn previous_weekday(date: Date, weekday: Weekday) -> Date {
    let current = i64::from(date.weekday().number_days_from_monday());
    let target = i64::from(weekday.number_days_from_monday());
    let back = match (current - target).rem_euclid(7) {
        0 => 7,
        d => d,
    };
    date - Duration::days(back)
}

#[cfg(test)]
mod tests {
    use time::macros::{date, datetime};

    use super::*;

    // 2026-10-16 is a Friday.
    const FRI: Date = date!(2026 - 10 - 16);

    #[test]
    fn friday_to_monday_is_one_workday() {
        let cal = WorkCalendar::new();
        assert_eq!(cal.workdays_between(FRI, date!(2026 - 10 - 17)), 0); // Sat
        assert_eq!(cal.workdays_between(FRI, date!(2026 - 10 - 18)), 0); // Sun
        assert_eq!(cal.workdays_between(FRI, date!(2026 - 10 - 19)), 1); // Mon
        assert_eq!(cal.workdays_between(FRI, date!(2026 - 10 - 22)), 4); // Thu
        assert_eq!(cal.workdays_between(FRI, date!(2026 - 10 - 23)), 5); // Fri
    }

    #[test]
    fn same_day_and_reversed_ranges_are_zero() {
        let cal = WorkCalendar::new();
        assert_eq!(cal.workdays_between(FRI, FRI), 0);
        assert_eq!(cal.workdays_between(FRI, date!(2026 - 10 - 12)), 0);
    }

    #[test]
    fn workdays_since_uses_dates_not_hours() {
        let cal = WorkCalendar::new();
        let shipped = datetime!(2026-10-16 17:30 UTC);
        let asked = datetime!(2026-10-19 08:00 UTC);
        assert_eq!(cal.workdays_since(shipped, asked), 1);
    }

    #[test]
    fn holidays_are_skipped() {
        let cal = WorkCalendar::with_holidays([date!(2026 - 10 - 19)]);
        assert!(!cal.is_workday(date!(2026 - 10 - 19)));
        assert_eq!(cal.workdays_between(FRI, date!(2026 - 10 - 19)), 0);
        assert_eq!(cal.add_workdays(FRI, 1), date!(2026 - 10 - 20));
    }

    #[test]
    fn serializes_as_iso_dates() {
        let cal = WorkCalendar::with_holidays([date!(2026 - 12 - 25), date!(2026 - 10 - 23)]);
        let json = serde_json::to_string(&cal).unwrap();
        assert_eq!(json, r#"["2026-10-23","2026-12-25"]"#);
        assert_eq!(serde_json::from_str::<WorkCalendar>(&json).unwrap(), cal);
        assert!(serde_json::from_str::<WorkCalendar>(r#"["23/10/2026"]"#).is_err());
    }

    #[test]
    fn add_workdays_skips_weekends() {
        let cal = WorkCalendar::new();
        assert_eq!(cal.add_workdays(FRI, 0), FRI);
        assert_eq!(cal.add_workdays(FRI, 1), date!(2026 - 10 - 19));
        assert_eq!(cal.add_workdays(FRI, 3), date!(2026 - 10 - 21));
        assert_eq!(
            cal.add_workdays(date!(2026 - 10 - 17), 1),
            date!(2026 - 10 - 19)
        );
        assert_eq!(cal.add_workdays(date!(2026 - 10 - 12), 4), FRI);
    }

    #[test]
    fn next_monday_nine() {
        assert_eq!(
            next_monday_9am(datetime!(2026-10-16 14:00 UTC)),
            datetime!(2026-10-19 09:00 UTC)
        );
        assert_eq!(
            next_monday_9am(datetime!(2026-10-19 08:59 UTC)),
            datetime!(2026-10-19 09:00 UTC)
        );
        assert_eq!(
            next_monday_9am(datetime!(2026-10-19 09:00 UTC)),
            datetime!(2026-10-26 09:00 UTC)
        );
        assert_eq!(
            next_monday_9am(datetime!(2026-10-18 23:00 +02:00)),
            datetime!(2026-10-19 09:00 +02:00)
        );
    }

    #[test]
    fn last_friday() {
        assert_eq!(
            previous_weekday(date!(2026 - 10 - 19), Weekday::Friday),
            FRI
        );
        assert_eq!(
            previous_weekday(FRI, Weekday::Friday),
            date!(2026 - 10 - 09)
        );
        assert_eq!(
            previous_weekday(date!(2026 - 10 - 17), Weekday::Friday),
            FRI
        );
    }
}
