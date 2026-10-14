//! Investigations and refunds: records mirrored from their Restate workflows (admin only).

use serde::{Deserialize, Serialize};
use techdepo_domain::{
    Actor, HumanTaskId, InvestigationId, LostOutcome, Money, OrderNumber, RefundId, TicketId,
    UserId,
};
use time::OffsetDateTime;

use crate::common::{CustomerRef, Evidence, OrderRef, TicketRef};
use crate::error::{ApiError, FieldError, Validate, check_client_key, check_text};
use crate::ids::PaymentId;
use crate::orders::{CarrierEvent, InvoiceRef, PaymentEntry, ShipmentView};
use crate::tasks::{HumanTaskSummary, MAX_NOTE_CHARS};
use crate::vocab::{
    InvestigationDecision, InvestigationOutcome, InvestigationStatus, RefundReason, RefundStatus,
    TraceStatus,
};

// ---------------------------------------------------------------------------------------------
// Investigations
// ---------------------------------------------------------------------------------------------

/// `GET /api/admin/investigations` query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvestigationListQuery {
    /// Status filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<InvestigationStatus>,
    /// Only open (or only closed) investigations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
    /// Page cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// An investigation in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvestigationSummary {
    /// Investigation ID.
    pub id: InvestigationId,
    /// State.
    pub status: InvestigationStatus,
    /// The order.
    pub order: OrderRef,
    /// The ticket; absent when an admin opened it from the order page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<TicketRef>,
    /// What the trace revealed, once concluded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carrier_result: Option<LostOutcome>,
    /// The person's offer decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<InvestigationDecision>,
    /// How it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<InvestigationOutcome>,
    /// Conversation rounds with the customer so far (at most 2).
    pub rounds: u32,
    /// Who opened it (`ai`: the WhereIsMyOrder workflow; `human`: an admin).
    pub opened_by: Actor,
    /// Opened.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// Closed.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub closed_at: Option<OffsetDateTime>,
}

/// `GET /api/admin/investigations/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvestigationDetail {
    /// Header.
    pub investigation: InvestigationSummary,
    /// The shipment, with every carrier event including internal trace events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shipment: Option<ShipmentView>,
    /// Trace state (hidden truth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_status: Option<TraceStatus>,
    /// Carrier responses to the trace (internal events), oldest first.
    pub carrier_responses: Vec<CarrierEvent>,
    /// Human tasks it created.
    pub tasks: Vec<HumanTaskSummary>,
    /// The refund, if the customer accepted one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refund_id: Option<RefundId>,
    /// The replacement order, if the customer accepted one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<OrderRef>,
    /// Admin's note when opened manually.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `POST /api/admin/orders/{number}/investigations`: open an investigation without a ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenInvestigationRequest {
    /// Client-generated key; the investigation ID derives from it.
    pub idempotency_key: String,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Validate for OpenInvestigationRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        check_client_key(&mut errors, "idempotency_key", &self.idempotency_key);
        if let Some(note) = &self.note {
            check_text(&mut errors, "note", note, 0, MAX_NOTE_CHARS);
        }
        ApiError::check(errors)
    }
}

/// Response of starting a workflow from the admin UI. The workflow runs asynchronously; its
/// D1 record appears within moments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvestigationStarted {
    /// The investigation (`/admin/investigations/<id>`).
    pub investigation_id: InvestigationId,
}

// ---------------------------------------------------------------------------------------------
// Refunds
// ---------------------------------------------------------------------------------------------

/// `GET /api/admin/refunds` query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundListQuery {
    /// Status filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RefundStatus>,
    /// Order filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<OrderNumber>,
    /// Page cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A refund in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundSummary {
    /// Refund ID.
    pub id: RefundId,
    /// State.
    pub status: RefundStatus,
    /// The order.
    pub order: OrderRef,
    /// The customer.
    pub customer: CustomerRef,
    /// Amount.
    pub amount: Money,
    /// Reason.
    pub reason: RefundReason,
    /// Free-text reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_text: Option<String>,
    /// Pre-approved because a person made the offer (or initiated it).
    pub pre_approved: bool,
    /// Created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Completed.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub completed_at: Option<OffsetDateTime>,
}

/// `GET /api/admin/refunds/{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefundDetail {
    /// Header.
    pub refund: RefundSummary,
    /// Evidence gathered when it was requested.
    pub evidence: Vec<Evidence>,
    /// The charge being reversed (the duplicate, for double charges).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge: Option<PaymentEntry>,
    /// The refund ledger entry, once executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refund_payment: Option<PaymentEntry>,
    /// The credit note, for invoiced orders (never for a duplicate charge, see
    /// [`RefundReason::credits_invoice`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_note: Option<InvoiceRef>,
    /// The ticket that asked for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_id: Option<TicketId>,
    /// The investigation that offered it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub investigation_id: Option<InvestigationId>,
    /// The approval task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_task_id: Option<HumanTaskId>,
    /// Who approved or rejected it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<UserId>,
    /// When.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub decided_at: Option<OffsetDateTime>,
    /// Why it was rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
}

/// `POST /api/admin/orders/{number}/refunds`: an admin initiates a refund. A person is
/// initiating it, so it is pre-approved (support.md §4); `RefundWorkflow` still executes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitiateRefundRequest {
    /// Client-generated key; the refund ID derives from it.
    pub idempotency_key: String,
    /// Amount; absent refunds everything left of the charge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<Money>,
    /// Reason.
    pub reason: RefundReason,
    /// Free-text reason (required for `other`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_text: Option<String>,
    /// The charge to reverse; absent picks, for a duplicate charge, the latest extra charge,
    /// otherwise the oldest captured charge with something left (`plan_refund`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_payment_id: Option<PaymentId>,
}

impl Validate for InitiateRefundRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        check_client_key(&mut errors, "idempotency_key", &self.idempotency_key);
        if self.amount.is_some_and(|a| a.cents() <= 0) {
            errors.push(FieldError::new("amount", "Must be positive"));
        }
        match (&self.reason_text, self.reason) {
            (Some(text), _) => check_text(&mut errors, "reason_text", text, 0, MAX_NOTE_CHARS),
            (None, RefundReason::Other) => errors.push(FieldError::new("reason_text", "Required")),
            (None, _) => {}
        }
        ApiError::check(errors)
    }
}

/// Response of `POST /api/admin/orders/{number}/refunds`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundStarted {
    /// The refund (`/admin/refunds/<id>`).
    pub refund_id: RefundId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refund_request_validation() {
        let mut req = InitiateRefundRequest {
            idempotency_key: "abcdefgh".into(),
            amount: None,
            reason: RefundReason::Other,
            reason_text: None,
            charge_payment_id: None,
        };
        assert_eq!(req.validate().unwrap_err().fields[0].field, "reason_text");
        req.reason = RefundReason::Goodwill;
        req.amount = Some(Money::from_cents(-1));
        assert_eq!(req.validate().unwrap_err().fields[0].field, "amount");
        req.amount = Some(Money::from_cents(500));
        assert!(req.validate().is_ok());
    }
}
