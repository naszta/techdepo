//! Order status, overlay flags and the order state machine.
//!
//! ```text
//!                  ┌──────────── cancel (unpaid) ───────────► cancelled
//!                  │
//! awaiting_payment ─[pay]─► paid ─(auto)─► processing ─[ship]─► shipped ─[time passes]─► delivered
//!                             │               │                    │                         │
//!                             │               │                    │                         └─► return_requested ─► returned
//!                             └──── refund (human-approved) ───────┴──► refunded ◄────────────────────────────────────┘
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use time::Date;

use crate::id::OrderId;

labeled_enum! {
    /// Where an order is in its lifecycle.
    pub enum OrderStatus {
        AwaitingPayment => ("awaiting_payment", "Awaiting payment"),
        Paid => ("paid", "Paid"),
        Processing => ("processing", "Processing"),
        Shipped => ("shipped", "Shipped"),
        Delivered => ("delivered", "Delivered"),
        Cancelled => ("cancelled", "Cancelled"),
        Refunded => ("refunded", "Refunded"),
        ReturnRequested => ("return_requested", "Return requested"),
        Returned => ("returned", "Returned"),
    }
}

labeled_enum! {
    /// Something that can happen to an order.
    pub enum OrderAction {
        /// A payment was captured.
        Pay => ("pay", "Simulate payment"),
        /// Fulfilment started (automatic after payment).
        StartProcessing => ("start_processing", "Start processing"),
        /// Handed to the carrier.
        Ship => ("ship", "Mark as shipped"),
        /// The carrier delivered the parcel (automatic once simulated time passes the ETA).
        Deliver => ("deliver", "Deliver"),
        /// Cancel an unpaid order.
        Cancel => ("cancel", "Cancel order"),
        /// The customer asked to return a delivered order.
        RequestReturn => ("request_return", "Request return"),
        /// The returned parcel arrived back.
        MarkReturned => ("mark_returned", "Customer sends parcel back"),
        /// A full refund completed (only `RefundWorkflow` does this).
        MarkRefunded => ("mark_refunded", "Mark refunded"),
    }
}

/// An action is not allowed in the order's current status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cannot {action} an order that is {from}", action = .action.as_str(), from = .from.as_str())]
pub struct TransitionError {
    /// Status the order was in.
    pub from: OrderStatus,
    /// The rejected action.
    pub action: OrderAction,
}

impl OrderStatus {
    /// The status after `action`, or an error if the action is not allowed now.
    pub const fn apply(self, action: OrderAction) -> Result<Self, TransitionError> {
        use OrderAction as A;
        use OrderStatus as S;

        let next = match (self, action) {
            (S::AwaitingPayment, A::Pay) => S::Paid,
            (S::AwaitingPayment, A::Cancel) => S::Cancelled,
            (S::Paid, A::StartProcessing) => S::Processing,
            (S::Processing, A::Ship) => S::Shipped,
            (S::Shipped, A::Deliver) => S::Delivered,
            (S::Delivered, A::RequestReturn) => S::ReturnRequested,
            (S::ReturnRequested, A::MarkReturned) => S::Returned,
            (S::Paid | S::Processing | S::Shipped | S::Returned, A::MarkRefunded) => S::Refunded,
            (from, action) => return Err(TransitionError { from, action }),
        };
        Ok(next)
    }

    /// Whether `action` is allowed in this status.
    pub const fn allows(self, action: OrderAction) -> bool {
        self.apply(action).is_ok()
    }

    /// No further transitions are possible.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Refunded)
    }

    /// Money has been captured and not (fully) refunded.
    pub const fn is_paid(self) -> bool {
        !matches!(
            self,
            Self::AwaitingPayment | Self::Cancelled | Self::Refunded
        )
    }

    /// The parcel has left the warehouse.
    pub const fn has_shipped(self) -> bool {
        matches!(
            self,
            Self::Shipped | Self::Delivered | Self::ReturnRequested | Self::Returned
        )
    }

    /// What a cancellation, refund or return request means in this status (support
    /// `CancelOrder` and `RefundRequest` routes).
    pub const fn cancellation(self) -> Cancellation {
        match self {
            Self::AwaitingPayment => Cancellation::CancelNow,
            Self::Paid | Self::Processing => Cancellation::RefundRequired,
            Self::Shipped => Cancellation::InTransit,
            Self::Delivered => Cancellation::ReturnRequired,
            Self::ReturnRequested | Self::Returned => Cancellation::ReturnInProgress,
            Self::Cancelled | Self::Refunded => Cancellation::AlreadyClosed,
        }
    }
}

impl OrderAction {
    /// Whether the demo controls panel offers this action as a button.
    ///
    /// Automatic steps (`StartProcessing`, `Deliver`) happen by themselves and `MarkRefunded`
    /// is reserved for `RefundWorkflow`, so they are not buttons.
    pub const fn is_demo_control(self) -> bool {
        matches!(
            self,
            Self::Pay | Self::Ship | Self::Cancel | Self::RequestReturn | Self::MarkReturned
        )
    }
}

/// Every action allowed in `status`, in declaration order.
pub fn valid_actions(status: OrderStatus) -> Vec<OrderAction> {
    OrderAction::ALL
        .iter()
        .copied()
        .filter(|a| status.allows(*a))
        .collect()
}

/// The actions the demo controls show for `status`: only what's possible, only buttons.
pub fn demo_actions(status: OrderStatus) -> Vec<OrderAction> {
    valid_actions(status)
        .into_iter()
        .filter(|a| a.is_demo_control())
        .collect()
}

/// Days after delivery during which the customer can return an order (EU right of withdrawal).
pub const RETURN_WINDOW_DAYS: i64 = 14;

/// Whether an order delivered on `delivered_on` can still be returned on `today` (both on the
/// order's simulated calendar).
pub fn within_return_window(delivered_on: Date, today: Date) -> bool {
    (today - delivered_on).whole_days() <= RETURN_WINDOW_DAYS
}

/// The outcome of "cancel my order", decided purely by status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Cancellation {
    /// Unpaid: cancel immediately.
    CancelNow,
    /// Paid but not shipped: needs a (human-approved) refund.
    RefundRequired,
    /// Shipped, not delivered yet: it can be returned once it arrives.
    InTransit,
    /// Delivered: the route requests the return ([`OrderAction::RequestReturn`]) if the order is
    /// within [`RETURN_WINDOW_DAYS`] of delivery, and the refund starts once it is back.
    ReturnRequired,
    /// A return is already under way.
    ReturnInProgress,
    /// Already cancelled or refunded.
    AlreadyClosed,
}

/// Overlay flags shown next to the status.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct OrderFlags {
    /// A refund has been requested and is awaiting approval or execution.
    pub refund_pending: bool,
    /// A carrier investigation is open.
    pub investigation_open: bool,
    /// This order is a replacement for another (lost) order.
    pub replacement_for: Option<OrderId>,
}

impl OrderFlags {
    /// No flag is set.
    pub fn is_empty(&self) -> bool {
        !self.refund_pending && !self.investigation_open && self.replacement_for.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use OrderAction as A;
    use OrderStatus as S;

    #[test]
    fn happy_path() {
        let mut status = S::AwaitingPayment;
        for action in [A::Pay, A::StartProcessing, A::Ship, A::Deliver] {
            status = status.apply(action).unwrap();
        }
        assert_eq!(status, S::Delivered);
        status = status.apply(A::RequestReturn).unwrap();
        status = status.apply(A::MarkReturned).unwrap();
        assert_eq!(status.apply(A::MarkRefunded), Ok(S::Refunded));
    }

    #[test]
    fn cancel_only_when_unpaid() {
        assert_eq!(S::AwaitingPayment.apply(A::Cancel), Ok(S::Cancelled));
        for status in [S::Paid, S::Processing, S::Shipped, S::Delivered] {
            let err = status.apply(A::Cancel).unwrap_err();
            assert_eq!(
                err,
                TransitionError {
                    from: status,
                    action: A::Cancel
                }
            );
        }
        assert_eq!(
            S::Shipped.apply(A::Cancel).unwrap_err().to_string(),
            "cannot cancel an order that is shipped"
        );
    }

    #[test]
    fn refund_paths() {
        for status in [S::Paid, S::Processing, S::Shipped, S::Returned] {
            assert_eq!(status.apply(A::MarkRefunded), Ok(S::Refunded), "{status:?}");
        }
        for status in [
            S::AwaitingPayment,
            S::Delivered,
            S::ReturnRequested,
            S::Cancelled,
        ] {
            assert!(status.apply(A::MarkRefunded).is_err(), "{status:?}");
        }
    }

    #[test]
    fn terminal_states_allow_nothing() {
        for status in S::ALL.iter().copied().filter(|s| s.is_terminal()) {
            assert!(valid_actions(status).is_empty(), "{status:?}");
        }
        for status in S::ALL.iter().copied().filter(|s| !s.is_terminal()) {
            assert!(!valid_actions(status).is_empty(), "{status:?}");
        }
    }

    #[test]
    fn valid_and_demo_actions() {
        assert_eq!(valid_actions(S::AwaitingPayment), vec![A::Pay, A::Cancel]);
        assert_eq!(
            valid_actions(S::Paid),
            vec![A::StartProcessing, A::MarkRefunded]
        );
        assert!(demo_actions(S::Paid).is_empty());
        assert_eq!(demo_actions(S::Processing), vec![A::Ship]);
        assert_eq!(demo_actions(S::Shipped), Vec::<OrderAction>::new());
        assert_eq!(demo_actions(S::Delivered), vec![A::RequestReturn]);
        assert_eq!(demo_actions(S::ReturnRequested), vec![A::MarkReturned]);
    }

    #[test]
    fn return_window_is_fourteen_days() {
        use time::macros::date;
        let delivered = date!(2026 - 10 - 01);
        assert!(within_return_window(delivered, date!(2026 - 10 - 15)));
        assert!(!within_return_window(delivered, date!(2026 - 10 - 16)));
    }

    #[test]
    fn cancellation_table() {
        assert_eq!(S::AwaitingPayment.cancellation(), Cancellation::CancelNow);
        assert_eq!(S::Paid.cancellation(), Cancellation::RefundRequired);
        assert_eq!(S::Processing.cancellation(), Cancellation::RefundRequired);
        assert_eq!(S::Shipped.cancellation(), Cancellation::InTransit);
        assert_eq!(S::Delivered.cancellation(), Cancellation::ReturnRequired);
        assert_eq!(S::Cancelled.cancellation(), Cancellation::AlreadyClosed);
        assert_eq!(S::Refunded.cancellation(), Cancellation::AlreadyClosed);
    }

    #[test]
    fn serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&S::AwaitingPayment).unwrap(),
            "\"awaiting_payment\""
        );
        assert_eq!("return_requested".parse::<S>().unwrap(), S::ReturnRequested);
        assert_eq!(S::ReturnRequested.to_string(), "Return requested");
        let flags: OrderFlags = serde_json::from_str("{}").unwrap();
        assert!(flags.is_empty());
    }
}
