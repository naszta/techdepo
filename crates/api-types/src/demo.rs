//! The demo controls drawer on the order page: valid next actions, fault toggles, the
//! per-order simulated clock, the hidden truth, and scenario presets.
//!
//! Everything here is a simulation lever, not shop behaviour. The routes are scoped like the
//! order page: the caller's own orders (an `org:admin` may use any order of the org).

use serde::{Deserialize, Serialize};
use techdepo_domain::{
    ClockAdvance, FaultToggles, LostOutcome, OrderAction, OrderStatus, labeled_enum,
};
use time::{Date, OffsetDateTime};

use crate::common::OrderRef;
use crate::error::{ApiError, FieldError, Validate, check_client_key};
use crate::vocab::TraceStatus;

/// A button in the "valid next actions" list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoActionView {
    /// The action to send back in [`RunDemoActionRequest`].
    pub action: OrderAction,
    /// Button text, e.g. `Simulate payment`.
    pub label: String,
    /// The fault that will fire if this step runs now, e.g. `Charge twice`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub armed_fault: Option<String>,
}

/// The order's simulated clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockView {
    /// Offset from real time in seconds (never negative, never decreases).
    pub offset_secs: i64,
    /// Real now.
    #[serde(with = "time::serde::rfc3339")]
    pub real_now: OffsetDateTime,
    /// Simulated now ("Simulated now: Mon 2026-10-19 09:00").
    #[serde(with = "time::serde::rfc3339")]
    pub simulated_now: OffsetDateTime,
    /// Ship date, once shipped.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_fmt::date::option"
    )]
    pub shipped_on: Option<Date>,
    /// Full workdays since shipment on the simulated calendar (drives the shipping policy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdays_since_ship: Option<u32>,
    /// Carrier ETA, once shipped.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_fmt::date::option"
    )]
    pub eta: Option<Date>,
}

/// Something true in the simulation that the customer cannot see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HiddenTruth {
    /// "Parcel is lost: outcome *gone*".
    ParcelLost {
        /// What a trace will reveal.
        outcome: LostOutcome,
    },
    /// A carrier trace is running or concluded.
    Trace {
        /// Its state.
        status: TraceStatus,
    },
    /// Email sends fail until this instant (provider outage fault fired).
    EmailOutage {
        /// End of the outage.
        #[serde(with = "time::serde::rfc3339")]
        until: OffsetDateTime,
    },
    /// The carrier API is "down" for this order's calls.
    CarrierApiDown,
}

/// `GET /api/orders/{number}/demo` and the response of every demo mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoPanel {
    /// The order and its current status.
    pub order: OrderRef,
    /// Valid next demo actions only (invalid ones are hidden, not disabled).
    pub actions: Vec<DemoActionView>,
    /// Faults armed for the next steps.
    pub faults: FaultToggles,
    /// The simulated clock.
    pub clock: ClockView,
    /// Which [`ClockStep`]s apply now (`ship_last_friday` only once shipped and not delivered).
    pub clock_steps: Vec<ClockStep>,
    /// Hidden truth.
    pub hidden_truth: Vec<HiddenTruth>,
}

/// `POST /api/orders/{number}/demo/actions`: run one step of the order lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunDemoActionRequest {
    /// The action; must be in [`DemoPanel::actions`], otherwise `invalid_transition`.
    pub action: OrderAction,
}

impl Validate for RunDemoActionRequest {
    fn validate(&self) -> Result<(), ApiError> {
        if self.action.is_demo_control() {
            Ok(())
        } else {
            Err(ApiError::invalid_field("action", "Not a demo control"))
        }
    }
}

/// `PUT /api/orders/{number}/demo/faults`: arm faults for the next steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetFaultsRequest {
    /// The full set of toggles (replaces the current ones).
    pub faults: FaultToggles,
}

/// Longest email outage the demo allows, in minutes.
pub use techdepo_domain::MAX_EMAIL_OUTAGE_MINUTES;

impl Validate for SetFaultsRequest {
    fn validate(&self) -> Result<(), ApiError> {
        self.faults
            .validate()
            .map_err(|e| ApiError::invalid_field(format!("faults.{}", e.field()), e.to_string()))
    }
}

labeled_enum! {
    /// A simulated-clock control.
    pub enum ClockStep {
        /// `+1 day`.
        Day => ("day", "+1 day"),
        /// `+1 workday`.
        Workday => ("workday", "+1 workday"),
        /// `→ next Monday 09:00`.
        NextMonday9am => ("next_monday_9am", "→ next Monday 09:00"),
        /// Move the ship date (and the shipment's events and ETA) back to the most recent
        /// Friday before the simulated today. Only for shipped, undelivered orders.
        ShipLastFriday => ("ship_last_friday", "Set ship date to last Friday"),
    }
}

impl ClockStep {
    /// The clock advance this step performs (`None` for [`ClockStep::ShipLastFriday`], which
    /// moves the shipment rather than the clock).
    pub const fn advance(self) -> Option<ClockAdvance> {
        match self {
            Self::Day => Some(ClockAdvance::Day),
            Self::Workday => Some(ClockAdvance::Workday),
            Self::NextMonday9am => Some(ClockAdvance::NextMonday9am),
            Self::ShipLastFriday => None,
        }
    }

    /// The steps that apply to an order in `status`.
    pub fn available(status: OrderStatus) -> Vec<Self> {
        let mut steps = vec![Self::Day, Self::Workday, Self::NextMonday9am];
        if status == OrderStatus::Shipped {
            steps.push(Self::ShipLastFriday);
        }
        steps
    }
}

/// `POST /api/orders/{number}/demo/clock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvanceClockRequest {
    /// The step.
    pub step: ClockStep,
}

labeled_enum! {
    /// One-click scenario presets (M7): an order already in an interesting state.
    pub enum DemoScenario {
        /// Placed, awaiting payment.
        Unpaid => ("unpaid", "Unpaid order"),
        /// Paid with the "charge twice" fault (scenario 1).
        DoubleCharged => ("double_charged", "Double-charged"),
        /// Paid; the invoice has no billing address (scenario 2).
        InvoiceWithoutAddress => ("invoice_without_address", "Invoice without address"),
        /// Shipped last Friday, will arrive normally (scenario 3, standard policy).
        ShippedLastFriday => ("shipped_last_friday", "Shipped last Friday"),
        /// Shipped last Friday, lost; the trace finds it (scenario 3).
        LostResurfaces => ("lost_resurfaces", "Lost, resurfaces"),
        /// Shipped last Friday, lost for good (scenarios 4 and 5).
        LostGone => ("lost_gone", "Lost, gone"),
        /// Delivered.
        Delivered => ("delivered", "Delivered"),
    }
}

/// `POST /api/demo/scenarios`: create an order for the caller in a preset state, using the
/// cheapest complete in-stock build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateScenarioRequest {
    /// Client-generated key.
    pub idempotency_key: String,
    /// The preset.
    pub scenario: DemoScenario,
}

impl Validate for CreateScenarioRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors: Vec<FieldError> = Vec::new();
        check_client_key(&mut errors, "idempotency_key", &self.idempotency_key);
        ApiError::check(errors)
    }
}

/// Response of `POST /api/demo/scenarios`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateScenarioResponse {
    /// The new order.
    pub order: OrderRef,
}

#[cfg(test)]
mod tests {
    use techdepo_domain::EmailFault;

    use super::*;

    #[test]
    fn faults_validation() {
        let mut req = SetFaultsRequest {
            faults: FaultToggles::default(),
        };
        assert!(req.validate().is_ok());
        req.faults.email = EmailFault::Outage { minutes: 0 };
        assert!(req.validate().is_err());
        req.faults.email = EmailFault::Outage { minutes: 5 };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn clock_steps() {
        assert_eq!(ClockStep::available(OrderStatus::Paid).len(), 3);
        assert!(ClockStep::available(OrderStatus::Shipped).contains(&ClockStep::ShipLastFriday));
        assert_eq!(ClockStep::Workday.advance(), Some(ClockAdvance::Workday));
        let truth = HiddenTruth::ParcelLost {
            outcome: LostOutcome::Gone,
        };
        assert_eq!(
            serde_json::to_value(truth).unwrap(),
            serde_json::json!({ "kind": "parcel_lost", "outcome": "gone" })
        );
    }
}
