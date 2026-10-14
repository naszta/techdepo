//! Injectable faults for the demo, one toggle per order step.
//!
//! A toggle is armed before triggering the step that would hit it. Org settings carry default
//! toggles; the demo controls can override them per order.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Fault for the "Pay" step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PaymentFault {
    /// Payment succeeds once.
    #[default]
    None,
    /// Two captured charges for the same order.
    ChargeTwice,
    /// The payment is declined; the order stays unpaid and can be retried.
    Declined,
}

/// Fault for the "Issue invoice" step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InvoiceFault {
    /// The invoice is correct.
    #[default]
    None,
    /// The invoice is issued without the billing address.
    MissingBillingAddress,
}

/// Hidden outcome of a lost parcel, revealed by the carrier's trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LostOutcome {
    /// The carrier finds it; delivery tomorrow.
    Resurfaces,
    /// No information; a person has to decide.
    Gone,
}

/// Fault for the "Ship" step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShipmentFault {
    /// The parcel arrives by its ETA.
    #[default]
    None,
    /// Lost in transit. The customer only ever sees "in transit".
    Lost {
        /// What the carrier trace will reveal.
        outcome: LostOutcome,
    },
}

/// Fault for the confirmation email.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EmailFault {
    /// The provider accepts mail.
    #[default]
    None,
    /// The provider fails for this many minutes; Restate retries until it recovers.
    Outage {
        /// Outage length in minutes.
        minutes: u32,
    },
}

/// Fault for the fake carrier's API.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CarrierFault {
    /// The API answers.
    #[default]
    None,
    /// The API is unavailable; calls fail and are retried.
    ApiDown,
}

/// The armed fault for every step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct FaultToggles {
    /// Payment step.
    pub payment: PaymentFault,
    /// Invoice step.
    pub invoice: InvoiceFault,
    /// Shipping step.
    pub shipment: ShipmentFault,
    /// Confirmation email.
    pub email: EmailFault,
    /// Carrier API.
    pub carrier: CarrierFault,
}

/// Longest email outage the demo allows, in minutes: the confirmation email's retries must
/// outlast it (support.md: "retries until it recovers").
pub const MAX_EMAIL_OUTAGE_MINUTES: u32 = 60;

/// An armed fault is out of range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FaultError {
    /// The email outage is not 1–[`MAX_EMAIL_OUTAGE_MINUTES`] minutes.
    #[error("Between 1 and {MAX_EMAIL_OUTAGE_MINUTES} minutes")]
    EmailOutageMinutes(u32),
}

impl FaultError {
    /// The offending field, relative to the toggles (`email.minutes`).
    pub const fn field(&self) -> &'static str {
        match self {
            Self::EmailOutageMinutes(_) => "email.minutes",
        }
    }
}

impl FaultToggles {
    /// At least one fault is armed.
    pub fn any(&self) -> bool {
        *self != Self::default()
    }

    /// Checks value ranges: the same rules for per-order toggles and the org's defaults.
    pub fn validate(&self) -> Result<(), FaultError> {
        if let EmailFault::Outage { minutes } = self.email
            && !(1..=MAX_EMAIL_OUTAGE_MINUTES).contains(&minutes)
        {
            return Err(FaultError::EmailOutageMinutes(minutes));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_shape() {
        let toggles = FaultToggles {
            shipment: ShipmentFault::Lost {
                outcome: LostOutcome::Gone,
            },
            email: EmailFault::Outage { minutes: 5 },
            ..FaultToggles::default()
        };
        let json = serde_json::to_value(toggles).unwrap();
        assert_eq!(json["payment"]["kind"], "none");
        assert_eq!(json["shipment"]["kind"], "lost");
        assert_eq!(json["shipment"]["outcome"], "gone");
        assert_eq!(json["email"]["minutes"], 5);
        assert!(toggles.any());

        let parsed: FaultToggles =
            serde_json::from_str(r#"{"carrier":{"kind":"api_down"}}"#).unwrap();
        assert_eq!(parsed.carrier, CarrierFault::ApiDown);
        assert_eq!(parsed.payment, PaymentFault::None);
        assert!(!FaultToggles::default().any());
    }
}
