//! Human tasks (`human_tasks`): one queue for every decision a person has to make. Written by
//! the `HumanTasks` service (support pipeline); read by the admin inbox.
//!
//! `awakeable_id` never leaves the server: [`TaskRow`] does not carry it, only
//! [`Tasks::awakeable_id`] reads it.

use serde::{Deserialize, Serialize};
use techdepo_api_types::Evidence;
use techdepo_api_types::tasks::{HumanDecision, TaskContext};
use techdepo_api_types::vocab::{DecisionKind, HumanTaskKind, HumanTaskStatus};
use techdepo_domain::{HumanTaskId, InvestigationId, OrderId, RefundId, TicketId, UserId};
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutorExt, params};
use time::OffsetDateTime;

use crate::{Cursor, PageRequest, SqlError, Statement, json_param, repository, stmt};

/// A `human_tasks` row (without the awakeable).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TaskRow {
    /// Task ID.
    pub id: HumanTaskId,
    /// Kind.
    pub kind: HumanTaskKind,
    /// State.
    pub status: HumanTaskStatus,
    /// Title.
    pub title: String,
    /// Summary.
    pub summary: String,
    /// Context per kind.
    #[serde(rename = "context_json", with = "column::json_text")]
    pub context: TaskContext,
    /// Evidence.
    #[serde(rename = "evidence_json", with = "column::json_text")]
    pub evidence: Vec<Evidence>,
    /// Allowed decisions.
    #[serde(rename = "allowed_decisions_json", with = "column::json_text")]
    pub allowed_decisions: Vec<DecisionKind>,
    /// Decision kind.
    pub decision: Option<DecisionKind>,
    /// Decision details.
    #[serde(rename = "decision_json", default, with = "column::json_text::option")]
    pub decision_detail: Option<HumanDecision>,
    /// Note.
    pub note: Option<String>,
    /// Decided by.
    pub decided_by: Option<UserId>,
    /// Decided at.
    #[serde(default, with = "column::millis::option")]
    pub decided_at: Option<OffsetDateTime>,
    /// SLA.
    #[serde(with = "column::millis")]
    pub sla_due_at: OffsetDateTime,
    /// Last nudge.
    #[serde(default, with = "column::millis::option")]
    pub nudged_at: Option<OffsetDateTime>,
    /// Ticket.
    pub ticket_id: Option<TicketId>,
    /// Order.
    pub order_id: Option<OrderId>,
    /// Refund.
    pub refund_id: Option<RefundId>,
    /// Investigation.
    pub investigation_id: Option<InvestigationId>,
    /// Created.
    #[serde(with = "column::millis")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "column::millis")]
    pub updated_at: OffsetDateTime,
}

/// A task to create.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewTask {
    /// Task ID.
    pub id: HumanTaskId,
    /// Kind (its allowed decisions come from [`HumanTaskKind::allowed_decisions`]).
    pub kind: HumanTaskKind,
    /// Title.
    pub title: String,
    /// Summary.
    pub summary: String,
    /// Context.
    pub context: TaskContext,
    /// Evidence.
    pub evidence: Vec<Evidence>,
    /// Awakeable to resolve (exactly for [`HumanTaskKind::has_waiter`] kinds).
    pub awakeable_id: Option<String>,
    /// SLA.
    #[serde(with = "time::serde::rfc3339")]
    pub sla_due_at: OffsetDateTime,
    /// Ticket.
    pub ticket_id: Option<TicketId>,
    /// Order.
    pub order_id: Option<OrderId>,
    /// Refund.
    pub refund_id: Option<RefundId>,
    /// Investigation.
    pub investigation_id: Option<InvestigationId>,
    /// Created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

const COLUMNS: &str = "id, kind, status, title, summary, context_json, evidence_json, allowed_decisions_json, \
     decision, decision_json, note, decided_by, decided_at, sla_due_at, nudged_at, ticket_id, order_id, refund_id, \
     investigation_id, created_at, updated_at";

repository! {
    /// Human tasks.
    Tasks
}

impl<'a> Tasks<'a> {
    /// One task.
    pub async fn get(&self, id: &HumanTaskId) -> Result<Option<TaskRow>, SqlError> {
        self.sql
            .query_first_as(
                &format!("SELECT {COLUMNS} FROM human_tasks WHERE org_id = ?1 AND id = ?2"),
                &params![self.org(), id],
            )
            .await
    }

    /// The awakeable a decision resolves (server side only).
    pub async fn awakeable_id(&self, id: &HumanTaskId) -> Result<Option<String>, SqlError> {
        self.sql
            .query_scalar(
                "SELECT awakeable_id FROM human_tasks WHERE org_id = ?1 AND id = ?2",
                &params![self.org(), id],
            )
            .await
            .map(Option::flatten)
    }

    /// Tasks by status and kind. Pending tasks are listed oldest first, the rest newest first.
    pub async fn list(
        &self,
        status: HumanTaskStatus,
        kind: Option<HumanTaskKind>,
        page: &PageRequest,
    ) -> Result<(Vec<TaskRow>, Option<String>), SqlError> {
        let rows: Vec<TaskRow> = if status == HumanTaskStatus::Pending {
            let (key, id) = page.asc_bounds();
            self.sql
                .query_as(
                    &format!(
                        "SELECT {COLUMNS} FROM human_tasks WHERE org_id = ?1 AND status = ?2 AND (?3 IS NULL OR kind = ?3) \
                         AND (created_at > ?4 OR (created_at = ?4 AND id > ?5)) ORDER BY created_at, id LIMIT ?6"
                    ),
                    &params![self.org(), status.as_str(), kind.map(HumanTaskKind::as_str), key, id, page.fetch()],
                )
                .await?
        } else {
            let (key, id) = page.desc_bounds();
            self.sql
                .query_as(
                    &format!(
                        "SELECT {COLUMNS} FROM human_tasks WHERE org_id = ?1 AND status = ?2 AND (?3 IS NULL OR kind = ?3) \
                         AND (created_at < ?4 OR (created_at = ?4 AND id < ?5)) ORDER BY created_at DESC, id DESC LIMIT ?6"
                    ),
                    &params![self.org(), status.as_str(), kind.map(HumanTaskKind::as_str), key, id, page.fetch()],
                )
                .await?
        };
        Ok(page.finish(rows, |r| {
            Cursor::new(column::millis::to_millis(r.created_at), r.id.as_str())
        }))
    }

    /// Tasks about an order, newest first.
    pub async fn for_order(&self, order: &OrderId) -> Result<Vec<TaskRow>, SqlError> {
        self.sql
            .query_as(
                &format!("SELECT {COLUMNS} FROM human_tasks WHERE org_id = ?1 AND order_id = ?2 ORDER BY created_at DESC"),
                &params![self.org(), order],
            )
            .await
    }

    /// Tasks about a ticket, newest first.
    pub async fn for_ticket(&self, ticket: &TicketId) -> Result<Vec<TaskRow>, SqlError> {
        self.sql
            .query_as(
                &format!("SELECT {COLUMNS} FROM human_tasks WHERE org_id = ?1 AND ticket_id = ?2 ORDER BY created_at DESC"),
                &params![self.org(), ticket],
            )
            .await
    }

    /// Pending and overdue counts at `now` (admin overview).
    pub async fn pending_counts(&self, now: OffsetDateTime) -> Result<(u32, u32), SqlError> {
        #[derive(Deserialize)]
        struct C {
            pending: u32,
            overdue: u32,
        }
        let c: Option<C> = self
            .sql
            .query_first_as(
                "SELECT count(*) AS pending, coalesce(sum(CASE WHEN sla_due_at < ?2 THEN 1 ELSE 0 END), 0) AS overdue \
                 FROM human_tasks WHERE org_id = ?1 AND status = 'pending'",
                &params![self.org(), now],
            )
            .await?;
        Ok(c.map_or((0, 0), |c| (c.pending, c.overdue)))
    }

    /// Creates a task (idempotent on its ID).
    pub fn insert_statement(&self, t: &NewTask) -> Result<Statement, SqlError> {
        Ok(stmt(
            "INSERT INTO human_tasks (org_id, id, kind, status, title, summary, context_json, evidence_json, \
             allowed_decisions_json, awakeable_id, sla_due_at, ticket_id, order_id, refund_id, investigation_id, \
             created_at, updated_at) VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15) \
             ON CONFLICT (org_id, id) DO NOTHING",
            vec![
                self.org().into(),
                (&t.id).into(),
                t.kind.as_str().into(),
                t.title.as_str().into(),
                t.summary.as_str().into(),
                json_param(&t.context)?,
                json_param(&t.evidence)?,
                json_param(t.kind.allowed_decisions())?,
                t.awakeable_id.clone().into(),
                t.sla_due_at.into(),
                t.ticket_id.as_ref().into(),
                t.order_id.as_ref().into(),
                t.refund_id.as_ref().into(),
                t.investigation_id.as_ref().into(),
                t.created_at.into(),
            ],
        ))
    }

    /// Records a decision on a pending task; affects 0 rows if it was already decided.
    pub fn decide_statement(
        &self,
        id: &HumanTaskId,
        decision: &HumanDecision,
        kind: DecisionKind,
        note: Option<&str>,
        by: &UserId,
        at: OffsetDateTime,
    ) -> Result<Statement, SqlError> {
        Ok(stmt(
            "UPDATE human_tasks SET status = 'decided', decision = ?3, decision_json = ?4, note = ?5, decided_by = ?6, \
             decided_at = ?7, updated_at = ?7 WHERE org_id = ?1 AND id = ?2 AND status = 'pending'",
            vec![
                self.org().into(),
                id.into(),
                kind.as_str().into(),
                json_param(decision)?,
                note.into(),
                by.into(),
                at.into(),
            ],
        ))
    }

    /// Records that the admins were nudged about a pending task (its SLA passed); affects 0 rows
    /// if the task is no longer pending or was already nudged.
    pub fn nudge_statement(&self, id: &HumanTaskId, at: OffsetDateTime) -> Statement {
        stmt(
            "UPDATE human_tasks SET nudged_at = ?3, updated_at = ?3 \
             WHERE org_id = ?1 AND id = ?2 AND status = 'pending' AND nudged_at IS NULL",
            params![self.org(), id, at],
        )
    }

    /// Refreshes a pending task about a ticket the customer wrote to again: a new summary, and
    /// an escalation's context says so (`new_customer_message`); affects 0 rows if the task is
    /// no longer pending.
    pub fn refresh_statement(
        &self,
        id: &HumanTaskId,
        summary: &str,
        at: OffsetDateTime,
    ) -> Statement {
        stmt(
            "UPDATE human_tasks SET summary = ?3, context_json = CASE WHEN kind = 'escalation' \
             THEN json_set(context_json, '$.new_customer_message', json('true')) ELSE context_json END, \
             updated_at = ?4 WHERE org_id = ?1 AND id = ?2 AND status = 'pending'",
            params![self.org(), id, summary, at],
        )
    }

    /// Cancels a pending task.
    pub fn cancel_statement(&self, id: &HumanTaskId, at: OffsetDateTime) -> Statement {
        stmt(
            "UPDATE human_tasks SET status = 'cancelled', updated_at = ?3 WHERE org_id = ?1 AND id = ?2 AND status = 'pending'",
            params![self.org(), id, at],
        )
    }
}
