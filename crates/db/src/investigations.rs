//! Carrier investigations (`investigations`): the read side for the admin, and the statements
//! the `Investigation` workflow (the only writer) records its progress with
//! ([`Investigations::insert_statement`], [`Investigations::update_statement`]).

use serde::Deserialize;
use techdepo_api_types::vocab::{InvestigationDecision, InvestigationOutcome, InvestigationStatus};
use techdepo_domain::{
    Actor, HumanTaskId, InvestigationId, LostOutcome, OrderId, OrderNumber, OrderStatus, RefundId,
    ShipmentId, TicketId, UserId,
};
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutorExt, params};
use time::OffsetDateTime;

use crate::orders::number_int;
use crate::{Cursor, PageRequest, SqlError, Statement, repository, stmt};

/// An `investigations` row joined with its order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InvestigationRow {
    /// Investigation ID.
    pub id: InvestigationId,
    /// Order.
    pub order_id: OrderId,
    /// Order number.
    #[serde(with = "number_int")]
    pub order_number: OrderNumber,
    /// Order status.
    pub order_status: OrderStatus,
    /// Ticket (absent when an admin opened it).
    pub ticket_id: Option<TicketId>,
    /// Shipment.
    pub shipment_id: Option<ShipmentId>,
    /// State.
    pub status: InvestigationStatus,
    /// What the trace revealed.
    pub carrier_result: Option<LostOutcome>,
    /// The person's decision.
    pub decision: Option<InvestigationDecision>,
    /// How it ended.
    pub outcome: Option<InvestigationOutcome>,
    /// Conversation rounds.
    pub rounds: u32,
    /// Who opened it.
    pub opened_by: Actor,
    /// The admin who opened it.
    pub opened_by_user_id: Option<UserId>,
    /// Note.
    pub note: Option<String>,
    /// Decision task.
    pub human_task_id: Option<HumanTaskId>,
    /// Refund.
    pub refund_id: Option<RefundId>,
    /// Replacement order.
    pub replacement_order_id: Option<OrderId>,
    /// Opened.
    #[serde(with = "column::millis")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "column::millis")]
    pub updated_at: OffsetDateTime,
    /// Closed.
    #[serde(default, with = "column::millis::option")]
    pub closed_at: Option<OffsetDateTime>,
}

/// An investigation to open (status `open`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct NewInvestigation {
    /// Investigation ID.
    pub id: InvestigationId,
    /// Order.
    pub order_id: OrderId,
    /// Ticket (absent when an admin opened it).
    pub ticket_id: Option<TicketId>,
    /// The parcel.
    pub shipment_id: Option<ShipmentId>,
    /// Who opened it.
    pub opened_by: Actor,
    /// The admin who opened it.
    pub opened_by_user_id: Option<UserId>,
    /// Note.
    pub note: Option<String>,
    /// Opened.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// What changed in an investigation; absent fields keep their value. A closing status
/// (`resolved`, `escalated`) stamps `closed_at` once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InvestigationUpdate {
    /// State.
    pub status: Option<InvestigationStatus>,
    /// What the trace revealed.
    pub carrier_result: Option<LostOutcome>,
    /// The person's decision.
    pub decision: Option<InvestigationDecision>,
    /// How it ended.
    pub outcome: Option<InvestigationOutcome>,
    /// Conversation rounds so far.
    pub rounds: Option<u32>,
    /// Decision task.
    pub human_task_id: Option<HumanTaskId>,
    /// Refund.
    pub refund_id: Option<RefundId>,
    /// Replacement order.
    pub replacement_order_id: Option<OrderId>,
}

/// `investigations.carrier_result`.
const fn lost_str(outcome: LostOutcome) -> &'static str {
    match outcome {
        LostOutcome::Resurfaces => "resurfaces",
        LostOutcome::Gone => "gone",
    }
}

const SELECT: &str = "SELECT v.*, o.number AS order_number, o.status AS order_status \
     FROM investigations v JOIN orders o ON o.org_id = v.org_id AND o.id = v.order_id";

repository! {
    /// Investigations (read side).
    Investigations
}

impl<'a> Investigations<'a> {
    /// One investigation.
    pub async fn get(&self, id: &InvestigationId) -> Result<Option<InvestigationRow>, SqlError> {
        self.sql
            .query_first_as(
                &format!("{SELECT} WHERE v.org_id = ?1 AND v.id = ?2"),
                &params![self.org(), id],
            )
            .await
    }

    /// The order's investigations, newest first.
    pub async fn for_order(&self, order: &OrderId) -> Result<Vec<InvestigationRow>, SqlError> {
        self.sql
            .query_as(
                &format!(
                    "{SELECT} WHERE v.org_id = ?1 AND v.order_id = ?2 ORDER BY v.created_at DESC"
                ),
                &params![self.org(), order],
            )
            .await
    }

    /// The ticket's investigations, newest first.
    pub async fn for_ticket(&self, ticket: &TicketId) -> Result<Vec<InvestigationRow>, SqlError> {
        self.sql
            .query_as(
                &format!(
                    "{SELECT} WHERE v.org_id = ?1 AND v.ticket_id = ?2 ORDER BY v.created_at DESC"
                ),
                &params![self.org(), ticket],
            )
            .await
    }

    /// Investigations, newest first. `open`: only running (`Some(true)`) or closed ones.
    pub async fn list(
        &self,
        status: Option<InvestigationStatus>,
        open: Option<bool>,
        page: &PageRequest,
    ) -> Result<(Vec<InvestigationRow>, Option<String>), SqlError> {
        let (key, id) = page.desc_bounds();
        let rows: Vec<InvestigationRow> = self
            .sql
            .query_as(
                &format!(
                    "{SELECT} WHERE v.org_id = ?1 AND (?2 IS NULL OR v.status = ?2) \
                     AND (?3 IS NULL OR (?3 = 1) = (v.closed_at IS NULL)) \
                     AND (v.created_at < ?4 OR (v.created_at = ?4 AND v.id < ?5)) \
                     ORDER BY v.created_at DESC, v.id DESC LIMIT ?6"
                ),
                &params![
                    self.org(),
                    status.map(InvestigationStatus::as_str),
                    open,
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

    /// Opens an investigation (idempotent: a retry finds it in place).
    pub fn insert_statement(&self, v: &NewInvestigation) -> Statement {
        stmt(
            "INSERT INTO investigations (org_id, id, order_id, ticket_id, shipment_id, status, rounds, opened_by, \
             opened_by_user_id, note, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'open', 0, ?6, ?7, ?8, ?9, ?9) ON CONFLICT (org_id, id) DO NOTHING",
            params![
                self.org(),
                &v.id,
                &v.order_id,
                v.ticket_id.as_ref(),
                v.shipment_id.as_ref(),
                v.opened_by.as_str(),
                v.opened_by_user_id.as_ref(),
                v.note.clone(),
                v.created_at
            ],
        )
    }

    /// Records progress (see [`InvestigationUpdate`]).
    pub fn update_statement(
        &self,
        id: &InvestigationId,
        u: &InvestigationUpdate,
        at: OffsetDateTime,
    ) -> Statement {
        stmt(
            "UPDATE investigations SET status = coalesce(?3, status), \
             carrier_result = coalesce(?4, carrier_result), decision = coalesce(?5, decision), \
             outcome = coalesce(?6, outcome), rounds = coalesce(?7, rounds), \
             human_task_id = coalesce(?8, human_task_id), refund_id = coalesce(?9, refund_id), \
             replacement_order_id = coalesce(?10, replacement_order_id), updated_at = ?11, \
             closed_at = CASE WHEN coalesce(?3, status) IN ('resolved', 'escalated') \
                         THEN coalesce(closed_at, ?11) ELSE NULL END \
             WHERE org_id = ?1 AND id = ?2",
            params![
                self.org(),
                id,
                u.status.map(InvestigationStatus::as_str),
                u.carrier_result.map(lost_str),
                u.decision.map(InvestigationDecision::as_str),
                u.outcome.map(InvestigationOutcome::as_str),
                u.rounds,
                u.human_task_id.as_ref(),
                u.refund_id.as_ref(),
                u.replacement_order_id.as_ref(),
                at
            ],
        )
    }

    /// Running investigations (admin overview).
    pub async fn open_count(&self) -> Result<u32, SqlError> {
        Ok(self
            .sql
            .query_scalar(
                "SELECT count(*) AS n FROM investigations WHERE org_id = ?1 AND closed_at IS NULL",
                &params![self.org()],
            )
            .await?
            .unwrap_or(0))
    }
}
