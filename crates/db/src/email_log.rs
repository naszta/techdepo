//! The email log (`email_log`): every inbound email (including rejected ones) and every
//! outbound copy. Written by mail-in and the `Email` service (support pipeline).
//!
//! Inbound mail rejected before its org was known has no `org_id`; it is visible in no org's
//! admin, so no scoped query returns it. [`insert_unscoped_rejection_statement`] writes it.

use serde::{Deserialize, Serialize};
use techdepo_api_types::EmailId;
use techdepo_api_types::vocab::{Direction, EmailStatus, RejectReason};
use techdepo_domain::{Channel, OrderId, OrderNumber, TicketId, UserId};
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutorExt, params};
use time::OffsetDateTime;

use crate::orders::number_int;
use crate::{
    Cursor, OrgScope, PageRequest, Param, SqlError, Statement, json_param, repository, stmt,
};

/// An `email_log` row, with the related order's number.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EmailRow {
    /// Log entry ID.
    pub id: EmailId,
    /// Direction.
    pub direction: Direction,
    /// Email or web form.
    pub channel: Channel,
    /// Outcome.
    pub status: EmailStatus,
    /// Why inbound mail was dropped.
    pub reject_reason: Option<RejectReason>,
    /// `Message-ID`.
    pub message_id: Option<String>,
    /// `In-Reply-To`.
    pub in_reply_to: Option<String>,
    /// `References`.
    #[serde(rename = "references_json", with = "column::json_text")]
    pub references: Vec<String>,
    /// From.
    pub from_address: String,
    /// From name.
    pub from_name: Option<String>,
    /// To.
    pub to_address: String,
    /// Reply-To.
    pub reply_to: Option<String>,
    /// Subject.
    pub subject: String,
    /// Text body.
    pub text_body: Option<String>,
    /// HTML body.
    pub html_body: Option<String>,
    /// Raw size.
    pub raw_size: Option<u32>,
    /// Authentication-Results summary.
    pub auth_results: Option<String>,
    /// Outbound template.
    pub template: Option<String>,
    /// Provider message ID.
    pub provider_id: Option<String>,
    /// Last error.
    pub error: Option<String>,
    /// Send attempts.
    pub attempts: u32,
    /// Sender / recipient member.
    pub user_id: Option<UserId>,
    /// Ticket.
    pub ticket_id: Option<TicketId>,
    /// Order.
    pub order_id: Option<OrderId>,
    /// Order number.
    #[serde(default, with = "number_int::option")]
    pub order_number: Option<OrderNumber>,
    /// Logged.
    #[serde(with = "column::millis")]
    pub created_at: OffsetDateTime,
    /// Sent.
    #[serde(default, with = "column::millis::option")]
    pub sent_at: Option<OffsetDateTime>,
}

/// An entry to log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NewEmail {
    /// Log entry ID.
    pub id: String,
    /// `inbound` or `outbound`.
    pub direction: Option<Direction>,
    /// Email or web form.
    pub channel: Option<Channel>,
    /// Outcome.
    pub status: Option<EmailStatus>,
    /// Rejection reason.
    pub reject_reason: Option<RejectReason>,
    /// `Message-ID`.
    pub message_id: Option<String>,
    /// `In-Reply-To`.
    pub in_reply_to: Option<String>,
    /// `References`.
    pub references: Vec<String>,
    /// From.
    pub from_address: String,
    /// From name.
    pub from_name: Option<String>,
    /// To.
    pub to_address: String,
    /// Reply-To.
    pub reply_to: Option<String>,
    /// Subject.
    pub subject: String,
    /// Text body.
    pub text_body: Option<String>,
    /// HTML body.
    pub html_body: Option<String>,
    /// Raw size.
    pub raw_size: Option<u32>,
    /// Authentication-Results.
    pub auth_results: Option<String>,
    /// Template.
    pub template: Option<String>,
    /// Member.
    pub user_id: Option<UserId>,
    /// Ticket.
    pub ticket_id: Option<TicketId>,
    /// Order.
    pub order_id: Option<OrderId>,
    /// Logged (epoch ms).
    pub created_at_ms: i64,
}

const SELECT: &str = "SELECT e.*, o.number AS order_number FROM email_log e \
     LEFT JOIN orders o ON o.org_id = e.org_id AND o.id = e.order_id";

repository! {
    /// The email log.
    EmailLog
}

impl<'a> EmailLog<'a> {
    /// One entry.
    pub async fn get(&self, id: &EmailId) -> Result<Option<EmailRow>, SqlError> {
        self.sql
            .query_first_as(
                &format!("{SELECT} WHERE e.org_id = ?1 AND e.id = ?2"),
                &params![self.org(), id.as_str()],
            )
            .await
    }

    /// Entries, newest first.
    pub async fn list(
        &self,
        direction: Option<Direction>,
        status: Option<EmailStatus>,
        ticket: Option<&TicketId>,
        page: &PageRequest,
    ) -> Result<(Vec<EmailRow>, Option<String>), SqlError> {
        let (key, id) = page.desc_bounds();
        let rows: Vec<EmailRow> = self
            .sql
            .query_as(
                &format!(
                    "{SELECT} WHERE e.org_id = ?1 AND (?2 IS NULL OR e.direction = ?2) AND (?3 IS NULL OR e.status = ?3) \
                     AND (?4 IS NULL OR e.ticket_id = ?4) AND (e.created_at < ?5 OR (e.created_at = ?5 AND e.id < ?6)) \
                     ORDER BY e.created_at DESC, e.id DESC LIMIT ?7"
                ),
                &params![
                    self.org(),
                    direction.map(Direction::as_str),
                    status.map(EmailStatus::as_str),
                    ticket,
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

    /// The kept (not rejected) inbound entry with this `Message-ID`: mail-in's redelivery check.
    pub async fn inbound_by_message_id(
        &self,
        message_id: &str,
    ) -> Result<Option<EmailRow>, SqlError> {
        self.sql
            .query_first_as(
                &format!(
                    "{SELECT} WHERE e.org_id = ?1 AND e.direction = 'inbound' AND e.message_id = ?2 \
                     AND e.status <> 'rejected' LIMIT 1"
                ),
                &params![self.org(), message_id],
            )
            .await
    }

    /// Inbound emails accepted since `since`: mail-in's per-org rate limit.
    pub async fn inbound_accepted_since(&self, since: OffsetDateTime) -> Result<u32, SqlError> {
        Ok(self
            .sql
            .query_scalar(
                "SELECT count(*) AS n FROM email_log WHERE org_id = ?1 AND direction = 'inbound' \
                 AND status = 'accepted' AND created_at >= ?2",
                &params![self.org(), since],
            )
            .await?
            .unwrap_or(0))
    }

    /// The ticket of the most recent of `message_ids` we logged (sent or received): mail-in's
    /// `In-Reply-To` / `References` fallback. At most the last 20 IDs are looked at.
    pub async fn ticket_for_message_ids(
        &self,
        message_ids: &[String],
    ) -> Result<Option<TicketId>, SqlError> {
        #[derive(Deserialize)]
        struct Hit {
            ticket_id: TicketId,
        }
        let start = message_ids.len().saturating_sub(20);
        let ids = &message_ids[start..];
        if ids.is_empty() {
            return Ok(None);
        }
        let hit: Option<Hit> = self
            .sql
            .query_first_as(
                "SELECT ticket_id FROM email_log WHERE org_id = ?1 AND ticket_id IS NOT NULL \
                 AND message_id IN (SELECT value FROM json_each(?2)) ORDER BY created_at DESC LIMIT 1",
                &[self.org().into(), json_param(ids)?],
            )
            .await?;
        Ok(hit.map(|h| h.ticket_id))
    }

    /// Logs an entry in this org (idempotent on its ID).
    pub fn insert_statement(&self, e: &NewEmail) -> Result<Statement, SqlError> {
        insert(Some(self.scope), e)
    }

    /// Counts a failed send attempt of a queued outbound email and keeps its error, so the
    /// admin email log shows a provider outage while Restate retries.
    pub fn attempt_failed_statement(&self, id: &EmailId, error: &str) -> Statement {
        stmt(
            "UPDATE email_log SET attempts = attempts + 1, error = ?3 \
             WHERE org_id = ?1 AND id = ?2 AND status = 'queued'",
            params![self.org(), id.as_str(), error],
        )
    }

    /// Marks a queued outbound email as accepted by the provider.
    pub fn sent_statement(&self, id: &EmailId, provider_id: &str, at: OffsetDateTime) -> Statement {
        stmt(
            "UPDATE email_log SET status = 'sent', provider_id = ?3, sent_at = ?4, attempts = attempts + 1 \
             WHERE org_id = ?1 AND id = ?2 AND status = 'queued'",
            params![self.org(), id.as_str(), provider_id, at],
        )
    }

    /// Marks a queued outbound email as permanently refused.
    pub fn failed_statement(&self, id: &EmailId, error: &str) -> Statement {
        stmt(
            "UPDATE email_log SET status = 'failed', error = ?3, attempts = attempts + 1 \
             WHERE org_id = ?1 AND id = ?2 AND status = 'queued'",
            params![self.org(), id.as_str(), error],
        )
    }
}

/// Logs inbound mail rejected before its org was known (`org_id` NULL, status `rejected`).
pub fn insert_unscoped_rejection_statement(e: &NewEmail) -> Result<Statement, SqlError> {
    insert(None, e)
}

fn insert(scope: Option<&OrgScope>, e: &NewEmail) -> Result<Statement, SqlError> {
    Ok(stmt(
        "INSERT INTO email_log (org_id, id, direction, channel, status, reject_reason, message_id, in_reply_to, \
         references_json, from_address, from_name, to_address, reply_to, subject, text_body, html_body, raw_size, \
         auth_results, template, user_id, ticket_id, order_id, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23) \
         ON CONFLICT (id) DO NOTHING",
        vec![
            scope.map(OrgScope::org_id).into(),
            e.id.as_str().into(),
            e.direction.map(Direction::as_str).into(),
            e.channel.unwrap_or(Channel::Email).into(),
            e.status.map(EmailStatus::as_str).into(),
            e.reject_reason.map(RejectReason::as_str).into(),
            e.message_id.clone().into(),
            e.in_reply_to.clone().into(),
            json_param(&e.references)?,
            e.from_address.as_str().into(),
            e.from_name.clone().into(),
            e.to_address.as_str().into(),
            e.reply_to.clone().into(),
            e.subject.as_str().into(),
            e.text_body.clone().into(),
            e.html_body.clone().into(),
            e.raw_size.into(),
            e.auth_results.clone().into(),
            e.template.clone().into(),
            e.user_id.as_ref().into(),
            e.ticket_id.as_ref().into(),
            e.order_id.as_ref().into(),
            Param::Int(e.created_at_ms),
        ],
    ))
}
