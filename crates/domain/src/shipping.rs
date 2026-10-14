//! Shipping policy and the fake carrier's ETA.

use std::ops::RangeInclusive;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::Date;

use crate::calendar::WorkCalendar;
use crate::id::TrackingNumber;

/// Countries we ship to (ISO 3166-1 alpha-2, upper case): the 27 EU member states.
pub const SHIPPING_COUNTRIES: [&str; 27] = [
    "AT", "BE", "BG", "CY", "CZ", "DE", "DK", "EE", "ES", "FI", "FR", "GR", "HR", "HU", "IE", "IT",
    "LT", "LU", "LV", "MT", "NL", "PL", "PT", "RO", "SE", "SI", "SK",
];

/// Whether we ship to `country` (an upper-case ISO 3166-1 alpha-2 code).
pub fn ships_to(country: &str) -> bool {
    SHIPPING_COUNTRIES.contains(&country)
}

/// The standard delivery estimate in workdays.
pub const STANDARD_TRANSIT_WORKDAYS: RangeInclusive<u32> = 1..=3;

/// What support answers to "where is my order?" for a shipped order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShippingPolicyOutcome {
    /// ≤ 3 workdays: within the standard 1–3 workday estimate; reply with the policy and ETA.
    WithinStandard,
    /// 4 workdays: the carrier is overloaded; ask the customer to wait a little longer.
    AskToWait,
    /// ≥ 5 workdays: acknowledge the problem and open an investigation.
    OpenInvestigation,
}

/// The shipping policy, given the number of full workdays since shipment
/// (see [`WorkCalendar::workdays_between`]).
pub const fn shipping_policy(workdays_since_ship: u32) -> ShippingPolicyOutcome {
    match workdays_since_ship {
        0..=3 => ShippingPolicyOutcome::WithinStandard,
        4 => ShippingPolicyOutcome::AskToWait,
        _ => ShippingPolicyOutcome::OpenInvestigation,
    }
}

/// Transit time for a shipment: 1–3 workdays, derived deterministically from its tracking
/// number so replays and reloads agree.
pub fn transit_workdays(tracking: &TrackingNumber) -> u32 {
    let span = STANDARD_TRANSIT_WORKDAYS.end() - STANDARD_TRANSIT_WORKDAYS.start() + 1;
    // `span` is 3, so the remainder always fits in a u32.
    let offset = u32::try_from(tracking.stable_hash() % u64::from(span)).unwrap_or(0);
    STANDARD_TRANSIT_WORKDAYS.start() + offset
}

/// The carrier's ETA: ship date plus [`transit_workdays`].
pub fn carrier_eta(calendar: &WorkCalendar, shipped_on: Date, tracking: &TrackingNumber) -> Date {
    calendar.add_workdays(shipped_on, transit_workdays(tracking))
}

#[cfg(test)]
mod tests {
    use time::macros::date;

    use super::*;

    #[test]
    fn ships_within_the_eu() {
        assert!(ships_to("HU") && ships_to("AT") && ships_to("GR"));
        assert!(!ships_to("CH") && !ships_to("GB") && !ships_to("hu"));
        assert!(SHIPPING_COUNTRIES.is_sorted());
    }

    #[test]
    fn policy_table() {
        for n in 0..=3 {
            assert_eq!(shipping_policy(n), ShippingPolicyOutcome::WithinStandard);
        }
        assert_eq!(shipping_policy(4), ShippingPolicyOutcome::AskToWait);
        assert_eq!(shipping_policy(5), ShippingPolicyOutcome::OpenInvestigation);
        assert_eq!(
            shipping_policy(30),
            ShippingPolicyOutcome::OpenInvestigation
        );
    }

    #[test]
    fn friday_shipment_asked_on_monday_gets_standard_policy() {
        let cal = WorkCalendar::new();
        let n = cal.workdays_between(date!(2026 - 10 - 16), date!(2026 - 10 - 19));
        assert_eq!(n, 1);
        assert_eq!(shipping_policy(n), ShippingPolicyOutcome::WithinStandard);
    }

    #[test]
    fn transit_is_deterministic_and_in_range() {
        let mut seen = [false; 3];
        for i in 0..100 {
            let tracking = TrackingNumber::new(format!("KP{i:08}"));
            let days = transit_workdays(&tracking);
            assert!(STANDARD_TRANSIT_WORKDAYS.contains(&days));
            assert_eq!(days, transit_workdays(&tracking));
            seen[(days - 1) as usize] = true;
        }
        assert_eq!(seen, [true; 3], "every transit time occurs");
    }

    #[test]
    fn eta_skips_weekend() {
        let cal = WorkCalendar::new();
        let tracking = TrackingNumber::new("KP00000001");
        let eta = carrier_eta(&cal, date!(2026 - 10 - 16), &tracking);
        assert_eq!(
            cal.workdays_between(date!(2026 - 10 - 16), eta),
            transit_workdays(&tracking)
        );
        assert!(cal.is_workday(eta));
    }
}
