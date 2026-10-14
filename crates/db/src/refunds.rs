//! Refund records (`refunds`), mirrored from `RefundWorkflow` (support pipeline). Only that
//! workflow moves money; this module gives it the writes and the admin the reads, plus the pure
//! [`plan_refund`] both run: the admin route rejects a refund the workflow would reject, before
//! starting it.

use serde::{Deserialize, Serialize};
use techdepo_api_types::vocab::{PaymentKind, PaymentStatus, RefundReason, RefundStatus};
use techdepo_api_types::{ApiError, ErrorCode, Evidence, PaymentId};
use techdepo_domain::{
    HumanTaskId, InvestigationId, InvoiceId, Money, OrderId, OrderNumber, RefundId, TicketId,
    UserId,
};
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutorExt, params};
use time::OffsetDateTime;

use crate::orders::number_int;
use crate::payments::PaymentRow;
use crate::{Cursor, PageRequest, SqlError, Statement, json_param, repository, stmt};

/// Which charge a refund reverses and how much.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundPlan {
    /// The charge.
    pub charge_payment_id: PaymentId,
    /// Amount.
    pub amount: Money,
    /// Refunds everything left of the order's payment (the order becomes `refunded`).
    pub full: bool,
}

/// Plans a refund from the order's ledger and its other refunds. `requested` absent refunds
/// everything left of the chosen charge.
///
/// What is left of a charge is its amount minus its succeeded refunds *and* the refunds still
/// open against it (`requested` or `approved`, not yet in the ledger), so a second refund cannot
/// be planned on money a pending one already claims. `others` must not contain the refund being
/// planned. By default the oldest captured charge with something left is refunded (the first
/// one, until it is refunded in full); for a duplicate charge, the latest extra charge.
pub fn plan_refund(
    payments: &[PaymentRow],
    others: &[RefundRow],
    reason: RefundReason,
    charge: Option<&PaymentId>,
    requested: Option<Money>,
) -> Result<RefundPlan, ApiError> {
    let mut charges: Vec<&PaymentRow> = payments
        .iter()
        .filter(|p| p.kind == PaymentKind::Charge && p.status == PaymentStatus::Succeeded)
        .collect();
    charges.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    let refunded = |c: &PaymentRow| -> Money {
        payments
            .iter()
            .filter(|p| p.refund_of.as_ref() == Some(&c.id) && p.status == PaymentStatus::Succeeded)
            .map(|p| p.amount)
            .sum()
    };
    let pending = |c: &PaymentRow| -> Money {
        others
            .iter()
            .filter(|r| {
                matches!(r.status, RefundStatus::Requested | RefundStatus::Approved)
                    && r.charge_payment_id.as_ref() == Some(&c.id)
                    && !payments.iter().any(|p| p.refund_id.as_ref() == Some(&r.id))
            })
            .map(|r| r.amount)
            .sum()
    };
    let left = |c: &PaymentRow| c.amount - refunded(c) - pending(c);
    let primary = charges.first().map(|c| c.id.clone());
    let chosen = match (charge, reason) {
        (Some(id), _) => charges
            .iter()
            .find(|c| &c.id == id)
            .copied()
            .ok_or_else(|| {
                ApiError::invalid_field(
                    "charge_payment_id",
                    "No captured charge with this ID on the order",
                )
            })?,
        (None, RefundReason::DuplicateCharge) => charges
            .iter()
            .skip(1)
            .rev()
            .find(|c| left(c).is_positive())
            .copied()
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidTransition,
                    "The order has no duplicate charge to refund",
                )
            })?,
        // The oldest charge with something left (the original, until it is refunded in full).
        (None, _) => charges
            .iter()
            .find(|c| left(c).is_positive())
            .or(charges.first())
            .copied()
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidTransition,
                    "The order has no captured payment to refund",
                )
            })?,
    };
    let remaining = left(chosen);
    if !remaining.is_positive() {
        return Err(ApiError::new(
            ErrorCode::InvalidTransition,
            if pending(chosen).is_positive() {
                "A refund of this payment is already in progress"
            } else {
                "This payment was already refunded in full"
            },
        ));
    }
    let amount = requested.unwrap_or(remaining);
    if !amount.is_positive() {
        return Err(ApiError::invalid_field("amount", "Must be positive"));
    }
    if amount > remaining {
        return Err(ApiError::invalid_field(
            "amount",
            format!("At most {remaining} is left to refund"),
        ));
    }
    Ok(RefundPlan {
        charge_payment_id: chosen.id.clone(),
        amount,
        full: reason.credits_invoice()
            && primary.as_ref() == Some(&chosen.id)
            && amount == chosen.amount - refunded(chosen),
    })
}

/// A `refunds` row joined with its order.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RefundRow {
    /// Refund ID.
    pub id: RefundId,
    /// Order.
    pub order_id: OrderId,
    /// Order number.
    #[serde(with = "number_int")]
    pub order_number: OrderNumber,
    /// Order status.
    pub order_status: techdepo_domain::OrderStatus,
    /// Customer.
    pub user_id: UserId,
    /// Customer email.
    pub customer_email: String,
    /// Customer name.
    pub customer_name: Option<String>,
    /// Ticket that asked.
    pub ticket_id: Option<TicketId>,
    /// Investigation that offered it.
    pub investigation_id: Option<InvestigationId>,
    /// Amount.
    #[serde(rename = "amount_cents")]
    pub amount: Money,
    /// Reason.
    pub reason: RefundReason,
    /// Free-text reason.
    pub reason_text: Option<String>,
    /// State.
    pub status: RefundStatus,
    /// Pre-approved.
    #[serde(with = "column::bool_int")]
    pub pre_approved: bool,
    /// The charge to reverse.
    pub charge_payment_id: Option<PaymentId>,
    /// The refund ledger entry.
    pub refund_payment_id: Option<PaymentId>,
    /// The credit note.
    pub credit_note_id: Option<InvoiceId>,
    /// The approval task.
    pub human_task_id: Option<HumanTaskId>,
    /// Approver or rejecter.
    pub decided_by: Option<UserId>,
    /// When decided.
    #[serde(default, with = "column::millis::option")]
    pub decided_at: Option<OffsetDateTime>,
    /// Why rejected.
    pub rejection_reason: Option<String>,
    /// Evidence.
    #[serde(rename = "evidence_json", with = "column::json_text")]
    pub evidence: Vec<Evidence>,
    /// Created.
    #[serde(with = "column::millis")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "column::millis")]
    pub updated_at: OffsetDateTime,
    /// Completed.
    #[serde(default, with = "column::millis::option")]
    pub completed_at: Option<OffsetDateTime>,
}

/// A refund record to create (status `requested`, or `approved` when pre-approved).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewRefund {
    /// Refund ID.
    pub id: RefundId,
    /// Order.
    pub order_id: OrderId,
    /// Ticket.
    pub ticket_id: Option<TicketId>,
    /// Investigation.
    pub investigation_id: Option<InvestigationId>,
    /// Amount.
    pub amount: Money,
    /// Reason.
    pub reason: RefundReason,
    /// Free text.
    pub reason_text: Option<String>,
    /// Pre-approved.
    pub pre_approved: bool,
    /// The charge to reverse.
    pub charge_payment_id: Option<PaymentId>,
    /// Evidence.
    pub evidence: Vec<Evidence>,
    /// Created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

const SELECT: &str = "SELECT r.*, o.number AS order_number, o.status AS order_status, o.user_id, \
     o.customer_email, o.customer_name FROM refunds r JOIN orders o ON o.org_id = r.org_id AND o.id = r.order_id";

repository! {
    /// Refunds.
    Refunds
}

impl<'a> Refunds<'a> {
    /// One refund.
    pub async fn get(&self, id: &RefundId) -> Result<Option<RefundRow>, SqlError> {
        self.sql
            .query_first_as(
                &format!("{SELECT} WHERE r.org_id = ?1 AND r.id = ?2"),
                &params![self.org(), id],
            )
            .await
    }

    /// The order's refunds, oldest first.
    pub async fn for_order(&self, order: &OrderId) -> Result<Vec<RefundRow>, SqlError> {
        self.sql
            .query_as(
                &format!(
                    "{SELECT} WHERE r.org_id = ?1 AND r.order_id = ?2 ORDER BY r.created_at, r.id"
                ),
                &params![self.org(), order],
            )
            .await
    }

    /// The ticket's refunds, oldest first.
    pub async fn for_ticket(&self, ticket: &TicketId) -> Result<Vec<RefundRow>, SqlError> {
        self.sql
            .query_as(
                &format!(
                    "{SELECT} WHERE r.org_id = ?1 AND r.ticket_id = ?2 ORDER BY r.created_at, r.id"
                ),
                &params![self.org(), ticket],
            )
            .await
    }

    /// Refunds, newest first.
    pub async fn list(
        &self,
        status: Option<RefundStatus>,
        order_number: Option<OrderNumber>,
        page: &PageRequest,
    ) -> Result<(Vec<RefundRow>, Option<String>), SqlError> {
        let (key, id) = page.desc_bounds();
        let rows: Vec<RefundRow> = self
            .sql
            .query_as(
                &format!(
                    "{SELECT} WHERE r.org_id = ?1 AND (?2 IS NULL OR r.status = ?2) AND (?3 IS NULL OR o.number = ?3) \
                     AND (r.created_at < ?4 OR (r.created_at = ?4 AND r.id < ?5)) ORDER BY r.created_at DESC, r.id DESC LIMIT ?6"
                ),
                &params![self.org(), status.map(RefundStatus::as_str), order_number, key, id, page.fetch()],
            )
            .await?;
        Ok(page.finish(rows, |r| {
            Cursor::new(column::millis::to_millis(r.created_at), r.id.as_str())
        }))
    }

    /// Sum of completed refunds and the number awaiting approval (admin overview).
    pub async fn totals(&self) -> Result<(Money, u32), SqlError> {
        #[derive(Deserialize)]
        struct T {
            refunded: i64,
            pending: u32,
        }
        let t: Option<T> = self
            .sql
            .query_first_as(
                "SELECT coalesce(sum(CASE WHEN status = 'completed' THEN amount_cents ELSE 0 END), 0) AS refunded, \
                 coalesce(sum(CASE WHEN status = 'requested' THEN 1 ELSE 0 END), 0) AS pending FROM refunds WHERE org_id = ?1",
                &params![self.org()],
            )
            .await?;
        Ok(t.map_or((Money::ZERO, 0), |t| {
            (Money::from_cents(t.refunded), t.pending)
        }))
    }

    /// Creates the record (idempotent on its ID).
    pub fn insert_statement(&self, r: &NewRefund) -> Result<Statement, SqlError> {
        let status = if r.pre_approved {
            RefundStatus::Approved
        } else {
            RefundStatus::Requested
        };
        Ok(stmt(
            "INSERT INTO refunds (org_id, id, order_id, ticket_id, investigation_id, amount_cents, reason, reason_text, \
             status, pre_approved, charge_payment_id, evidence_json, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13) ON CONFLICT (org_id, id) DO NOTHING",
            vec![
                self.org().into(),
                (&r.id).into(),
                (&r.order_id).into(),
                r.ticket_id.as_ref().into(),
                r.investigation_id.as_ref().into(),
                r.amount.into(),
                r.reason.as_str().into(),
                r.reason_text.clone().into(),
                status.as_str().into(),
                r.pre_approved.into(),
                r.charge_payment_id.as_ref().map(PaymentId::as_str).into(),
                json_param(&r.evidence)?,
                r.created_at.into(),
            ],
        ))
    }

    /// Records the approval decision.
    pub fn decide_statement(
        &self,
        id: &RefundId,
        approved: bool,
        by: Option<&UserId>,
        rejection_reason: Option<&str>,
        at: OffsetDateTime,
    ) -> Statement {
        let status = if approved {
            RefundStatus::Approved
        } else {
            RefundStatus::Rejected
        };
        stmt(
            "UPDATE refunds SET status = ?3, decided_by = ?4, decided_at = ?5, rejection_reason = ?6, updated_at = ?5 \
             WHERE org_id = ?1 AND id = ?2 AND status = 'requested'",
            params![self.org(), id, status.as_str(), by, at, rejection_reason],
        )
    }

    /// Closes a refund that could not be executed (the payment provider refused it, e.g. the
    /// charge was refunded meanwhile) as `rejected`, with `reason` saying so.
    pub fn fail_statement(&self, id: &RefundId, reason: &str, at: OffsetDateTime) -> Statement {
        stmt(
            "UPDATE refunds SET status = 'rejected', rejection_reason = ?3, updated_at = ?4 \
             WHERE org_id = ?1 AND id = ?2 AND status IN ('requested', 'approved')",
            params![self.org(), id, reason, at],
        )
    }

    /// Links the approval task.
    pub fn set_task_statement(
        &self,
        id: &RefundId,
        task: &HumanTaskId,
        at: OffsetDateTime,
    ) -> Statement {
        stmt(
            "UPDATE refunds SET human_task_id = ?3, updated_at = ?4 WHERE org_id = ?1 AND id = ?2",
            params![self.org(), id, task, at],
        )
    }

    /// Marks it completed with its ledger entry and credit note.
    pub fn complete_statement(
        &self,
        id: &RefundId,
        refund_payment: &PaymentId,
        credit_note: Option<&InvoiceId>,
        at: OffsetDateTime,
    ) -> Statement {
        stmt(
            "UPDATE refunds SET status = 'completed', refund_payment_id = ?3, credit_note_id = ?4, completed_at = ?5, \
             updated_at = ?5 WHERE org_id = ?1 AND id = ?2 AND status = 'approved'",
            params![self.org(), id, refund_payment.as_str(), credit_note, at],
        )
    }
}
