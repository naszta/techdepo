//! The fake PSP ledger (`payments`): charges and refunds.

use serde::{Deserialize, Serialize};
use techdepo_api_types::PaymentId;
use techdepo_api_types::vocab::{PaymentKind, PaymentMethod, PaymentStatus};
use techdepo_domain::{Money, OrderId, OrderNumber, RefundId, UserId};
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutorExt, params};
use time::OffsetDateTime;

use crate::orders::number_int;
use crate::{Cursor, PageRequest, SqlError, Statement, repository, stmt};

/// A `payments` row, joined with its order's number and customer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaymentRow {
    /// Entry ID.
    pub id: PaymentId,
    /// Order.
    pub order_id: OrderId,
    /// Order number.
    #[serde(with = "number_int")]
    pub order_number: OrderNumber,
    /// The order's customer.
    pub user_id: UserId,
    /// The order's customer email.
    pub customer_email: String,
    /// The order's customer name.
    pub customer_name: Option<String>,
    /// Charge or refund.
    pub kind: PaymentKind,
    /// Outcome.
    pub status: PaymentStatus,
    /// Amount (positive).
    #[serde(rename = "amount_cents")]
    pub amount: Money,
    /// Method.
    pub method: PaymentMethod,
    /// Fake PSP reference.
    pub provider_ref: String,
    /// Refunds: the charge reversed.
    pub refund_of: Option<PaymentId>,
    /// Refunds: the refund workflow.
    pub refund_id: Option<RefundId>,
    /// Why it failed.
    pub failure_reason: Option<String>,
    /// Real time.
    #[serde(with = "column::millis")]
    pub created_at: OffsetDateTime,
    /// The order's simulated time.
    #[serde(with = "column::millis")]
    pub sim_at: OffsetDateTime,
}

/// A ledger entry to write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPayment {
    /// Entry ID.
    pub id: PaymentId,
    /// Order.
    pub order_id: OrderId,
    /// Charge or refund.
    pub kind: PaymentKind,
    /// Outcome.
    pub status: PaymentStatus,
    /// Amount (positive).
    pub amount: Money,
    /// Method (`card` or `bank_transfer`).
    pub method: PaymentMethod,
    /// Fake PSP reference.
    pub provider_ref: String,
    /// Refunds: the charge reversed.
    pub refund_of: Option<PaymentId>,
    /// Refunds: the refund workflow.
    pub refund_id: Option<RefundId>,
    /// Why it failed.
    pub failure_reason: Option<String>,
    /// Real time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// The order's simulated time.
    #[serde(with = "time::serde::rfc3339")]
    pub sim_at: OffsetDateTime,
}

/// Filters of the admin ledger.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PaymentFilter {
    /// Charge or refund.
    pub kind: Option<PaymentKind>,
    /// Outcome.
    pub status: Option<PaymentStatus>,
    /// Order.
    pub order_number: Option<OrderNumber>,
}

const SELECT: &str = "SELECT p.*, o.number AS order_number, o.user_id, o.customer_email, o.customer_name \
     FROM payments p JOIN orders o ON o.org_id = p.org_id AND o.id = p.order_id";

repository! {
    /// The payments ledger.
    Payments
}

impl<'a> Payments<'a> {
    /// Appends a ledger entry (idempotent on its ID).
    pub fn insert_statement(&self, p: &NewPayment) -> Statement {
        stmt(
            "INSERT INTO payments (org_id, id, order_id, kind, status, amount_cents, method, provider_ref, \
             refund_of, refund_id, failure_reason, created_at, sim_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) ON CONFLICT (org_id, id) DO NOTHING",
            params![
                self.org(),
                p.id.as_str(),
                &p.order_id,
                p.kind.as_str(),
                p.status.as_str(),
                p.amount,
                p.method.as_str(),
                p.provider_ref.as_str(),
                p.refund_of.as_ref().map(PaymentId::as_str),
                p.refund_id.as_ref(),
                p.failure_reason.clone(),
                p.created_at,
                p.sim_at
            ],
        )
    }

    /// Records a refund of `p.refund_of` only if that charge (succeeded) still has at least
    /// `p.amount` left after its succeeded refunds; check [`crate::rows_affected`] (0: refused,
    /// nothing written). The check and the insert are one statement, so two refunds racing
    /// for the same charge cannot both pass (D1 runs writes one at a time).
    pub fn refund_insert_statement(&self, p: &NewPayment) -> Statement {
        stmt(
            "INSERT INTO payments (org_id, id, order_id, kind, status, amount_cents, method, provider_ref, \
             refund_of, refund_id, failure_reason, created_at, sim_at) \
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13 \
             WHERE ?6 <= (SELECT c.amount_cents FROM payments c WHERE c.org_id = ?1 AND c.id = ?9 \
                          AND c.kind = 'charge' AND c.status = 'succeeded') \
                  - (SELECT coalesce(sum(r.amount_cents), 0) FROM payments r WHERE r.org_id = ?1 \
                     AND r.refund_of = ?9 AND r.kind = 'refund' AND r.status = 'succeeded') \
             ON CONFLICT (org_id, id) DO NOTHING",
            params![
                self.org(),
                p.id.as_str(),
                &p.order_id,
                p.kind.as_str(),
                p.status.as_str(),
                p.amount,
                p.method.as_str(),
                p.provider_ref.as_str(),
                p.refund_of.as_ref().map(PaymentId::as_str),
                p.refund_id.as_ref(),
                p.failure_reason.clone(),
                p.created_at,
                p.sim_at
            ],
        )
    }

    /// One entry.
    pub async fn get(&self, id: &PaymentId) -> Result<Option<PaymentRow>, SqlError> {
        self.sql
            .query_first_as(
                &format!("{SELECT} WHERE p.org_id = ?1 AND p.id = ?2"),
                &params![self.org(), id.as_str()],
            )
            .await
    }

    /// The order's entries, oldest first (the card statement).
    pub async fn for_order(&self, order: &OrderId) -> Result<Vec<PaymentRow>, SqlError> {
        self.sql
            .query_as(
                &format!(
                    "{SELECT} WHERE p.org_id = ?1 AND p.order_id = ?2 ORDER BY p.created_at, p.id"
                ),
                &params![self.org(), order],
            )
            .await
    }

    /// Every entry of the org, newest first (input of the duplicate detector).
    pub async fn all(&self) -> Result<Vec<PaymentRow>, SqlError> {
        self.sql
            .query_as(
                &format!("{SELECT} WHERE p.org_id = ?1 ORDER BY p.created_at DESC, p.id DESC"),
                &params![self.org()],
            )
            .await
    }

    /// Entries matching `filter`, newest first.
    pub async fn list(
        &self,
        filter: &PaymentFilter,
        page: &PageRequest,
    ) -> Result<(Vec<PaymentRow>, Option<String>), SqlError> {
        let (key, id) = page.desc_bounds();
        let rows: Vec<PaymentRow> = self
            .sql
            .query_as(
                &format!(
                    "{SELECT} WHERE p.org_id = ?1 AND (?2 IS NULL OR p.kind = ?2) AND (?3 IS NULL OR p.status = ?3) \
                     AND (?4 IS NULL OR o.number = ?4) AND (p.created_at < ?5 OR (p.created_at = ?5 AND p.id < ?6)) \
                     ORDER BY p.created_at DESC, p.id DESC LIMIT ?7"
                ),
                &params![
                    self.org(),
                    filter.kind.map(PaymentKind::as_str),
                    filter.status.map(PaymentStatus::as_str),
                    filter.order_number,
                    key,
                    id,
                    page.fetch()
                ],
            )
            .await?;
        Ok(page.finish(rows, |r| {
            Cursor::new(column::millis::to_millis(r.created_at), r.id.as_str())
        }))
    }

    /// Succeeded charges minus succeeded refunds of an order (what is left to refund).
    pub async fn net_captured(&self, order: &OrderId) -> Result<Money, SqlError> {
        let cents: Option<i64> = self
            .sql
            .query_scalar(
                "SELECT coalesce(sum(CASE kind WHEN 'charge' THEN amount_cents ELSE -amount_cents END), 0) AS net \
                 FROM payments WHERE org_id = ?1 AND order_id = ?2 AND status = 'succeeded'",
                &params![self.org(), order],
            )
            .await?;
        Ok(Money::from_cents(cents.unwrap_or(0)))
    }

    /// Org-wide simulated revenue: succeeded charges minus succeeded refunds.
    pub async fn revenue(&self) -> Result<Money, SqlError> {
        let cents: Option<i64> = self
            .sql
            .query_scalar(
                "SELECT coalesce(sum(CASE kind WHEN 'charge' THEN amount_cents ELSE -amount_cents END), 0) AS net \
                 FROM payments WHERE org_id = ?1 AND status = 'succeeded'",
                &params![self.org()],
            )
            .await?;
        Ok(Money::from_cents(cents.unwrap_or(0)))
    }
}
