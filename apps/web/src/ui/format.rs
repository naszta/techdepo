//! Dates and times for display.
//!
//! Instants are shown in UTC, the zone the order's simulated calendar runs in (the demo's
//! "next Monday 09:00" is 09:00 UTC), so real and simulated times read the same way.

use time::macros::format_description;
use time::{Date, OffsetDateTime};

/// `13 Oct 2026`.
pub fn date(at: OffsetDateTime) -> String {
    at.format(format_description!(
        "[day padding:none] [month repr:short] [year]"
    ))
    .unwrap_or_default()
}

/// `Mon 12 Oct, 14:31`.
pub fn datetime(at: OffsetDateTime) -> String {
    at.format(format_description!(
        "[weekday repr:short] [day padding:none] [month repr:short], [hour]:[minute]"
    ))
    .unwrap_or_default()
}

/// `Mon 19 Oct 2026, 09:00`.
pub fn datetime_long(at: OffsetDateTime) -> String {
    at.format(format_description!(
        "[weekday repr:short] [day padding:none] [month repr:short] [year], [hour]:[minute]"
    ))
    .unwrap_or_default()
}

/// `Fri 16 Oct` (a calendar date: ship date, ETA, invoice date).
pub fn day(date: Date) -> String {
    date.format(format_description!(
        "[weekday repr:short] [day padding:none] [month repr:short]"
    ))
    .unwrap_or_default()
}

/// `2 days 3 h` for an offset in seconds (the simulated clock's lead over real time).
pub fn duration(secs: i64) -> String {
    let secs = secs.max(0);
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    match (days, hours) {
        (0, 0) => format!("{minutes} min"),
        (0, h) => format!("{h} h {minutes} min"),
        (1, 0) => "1 day".to_owned(),
        (d, 0) => format!("{d} days"),
        (1, h) => format!("1 day {h} h"),
        (d, h) => format!("{d} days {h} h"),
    }
}

/// The browser's current time (for "ago" labels; never used for business decisions).
pub fn now() -> OffsetDateTime {
    let nanos = (js_sys::Date::now() * 1_000_000.0) as i128;
    OffsetDateTime::from_unix_timestamp_nanos(nanos).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// `5 min ago`, `3 h ago`, `2 days ago` (real time; task ages and SLAs).
pub fn ago(at: OffsetDateTime) -> String {
    let secs = (now() - at).whole_seconds();
    if secs < 60 {
        "just now".to_owned()
    } else {
        format!("{} ago", duration(secs))
    }
}

/// `in 12 min` or `12 min ago` for a deadline.
pub fn relative(at: OffsetDateTime) -> String {
    let secs = (at - now()).whole_seconds();
    if secs >= 0 {
        format!("in {}", duration(secs))
    } else {
        ago(at)
    }
}
