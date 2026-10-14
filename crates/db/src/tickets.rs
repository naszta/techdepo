//! Support tickets (`tickets`, `ticket_messages`): the read model of the `SupportTicket` VO.
//! The web API reads it; the VO is its only writer ([`Tickets::save_statement`] mirrors the
//! object's state, messages are appended with the `seq` the object assigns).

use serde::Deserialize;
use techdepo_api_types::support::Classification;
use techdepo_api_types::vocab::{Awaiting, Direction, MessageStatus, TicketRoute};
use techdepo_domain::{
    Actor, Assignee, Channel, Intent, OrderId, OrderNumber, OrderStatus, TicketId, TicketStatus,
    UserId,
};
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutorExt, params};
use time::OffsetDateTime;

use crate::orders::number_int;
use crate::{Cursor, PageRequest, SqlError, Statement, json_param, repository, stmt};

/// A `tickets` row, with the linked order's number and status.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TicketRow {
    /// Ticket ID.
    pub id: TicketId,
    /// Customer.
    pub user_id: UserId,
    /// Customer email.
    pub customer_email: String,
    /// Subject of the first message.
    pub subject: String,
    /// Channel of the first message.
    pub channel: Channel,
    /// Status.
    pub status: TicketStatus,
    /// AI or a person.
    pub assignee: Assignee,
    /// The order it is about.
    pub order_id: Option<OrderId>,
    /// The linked order's number.
    #[serde(default, with = "number_int::option")]
    pub linked_order_number: Option<OrderNumber>,
    /// The linked order's status.
    pub linked_order_status: Option<OrderStatus>,
    /// Classified intent.
    pub intent: Option<Intent>,
    /// Its probability.
    pub intent_confidence: Option<f64>,
    /// Full classification.
    #[serde(
        rename = "classification_json",
        default,
        with = "column::json_text::option"
    )]
    pub classification: Option<Classification>,
    /// Route.
    pub route: Option<TicketRoute>,
    /// What it waits for.
    pub awaiting: Option<Awaiting>,
    /// Why escalated.
    pub escalation_reason: Option<String>,
    /// Latest AI reply confidence.
    pub reply_confidence: Option<f64>,
    /// Messages (all of them; see [`Tickets::visible_message_count`]).
    pub message_count: u32,
    /// Latest message.
    #[serde(default, with = "column::millis::option")]
    pub last_message_at: Option<OffsetDateTime>,
    /// Opened.
    #[serde(with = "column::millis")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "column::millis")]
    pub updated_at: OffsetDateTime,
    /// Resolved.
    #[serde(default, with = "column::millis::option")]
    pub resolved_at: Option<OffsetDateTime>,
}

/// A `ticket_messages` row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TicketMessageRow {
    /// Position.
    pub seq: u32,
    /// Direction.
    pub direction: Direction,
    /// Author.
    pub author: Actor,
    /// The person, if any.
    pub author_user_id: Option<UserId>,
    /// Channel.
    pub channel: Channel,
    /// State.
    pub status: MessageStatus,
    /// Subject.
    pub subject: String,
    /// Text body.
    pub body_text: String,
    /// HTML body.
    pub body_html: Option<String>,
    /// `Message-ID`.
    pub message_id: Option<String>,
    /// Created.
    #[serde(with = "column::millis")]
    pub created_at: OffsetDateTime,
    /// Sent.
    #[serde(default, with = "column::millis::option")]
    pub sent_at: Option<OffsetDateTime>,
}

/// Filters of a ticket list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TicketFilter {
    /// Customer.
    pub user_id: Option<UserId>,
    /// Status.
    pub status: Option<TicketStatus>,
    /// Only open (`Some(true)`) or only resolved (`Some(false)`).
    pub open: Option<bool>,
    /// Intent.
    pub intent: Option<Intent>,
    /// Assignee.
    pub assignee: Option<Assignee>,
}

/// The whole `tickets` row as the `SupportTicket` VO holds it (upserted on every change).
#[derive(Debug, Clone, PartialEq)]
pub struct TicketRecord {
    /// Ticket ID.
    pub id: TicketId,
    /// Customer.
    pub user_id: UserId,
    /// Customer email.
    pub customer_email: String,
    /// Subject of the first message.
    pub subject: String,
    /// Channel of the first message.
    pub channel: Channel,
    /// Status.
    pub status: TicketStatus,
    /// AI or a person.
    pub assignee: Assignee,
    /// The order it is about (must exist in the org).
    pub order_id: Option<OrderId>,
    /// The order number as extracted, even if unknown.
    pub order_number: Option<OrderNumber>,
    /// Classified intent.
    pub intent: Option<Intent>,
    /// Its probability.
    pub intent_confidence: Option<f64>,
    /// Full classification.
    pub classification: Option<Classification>,
    /// Route.
    pub route: Option<TicketRoute>,
    /// What it waits for (exactly when `status` is `awaiting_customer`).
    pub awaiting: Option<Awaiting>,
    /// Why escalated.
    pub escalation_reason: Option<String>,
    /// Latest AI reply confidence.
    pub reply_confidence: Option<f64>,
    /// Messages, drafts included.
    pub message_count: u32,
    /// Latest message.
    pub last_message_at: Option<OffsetDateTime>,
    /// Opened.
    pub created_at: OffsetDateTime,
    /// Last change.
    pub updated_at: OffsetDateTime,
    /// Resolved.
    pub resolved_at: Option<OffsetDateTime>,
}

/// A message to append to a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTicketMessage {
    /// Position (assigned by the VO).
    pub seq: u32,
    /// Direction.
    pub direction: Direction,
    /// Author (`customer` exactly for inbound).
    pub author: Actor,
    /// The person, if any.
    pub author_user_id: Option<UserId>,
    /// Channel.
    pub channel: Channel,
    /// `received` (inbound), `draft` or `sent`.
    pub status: MessageStatus,
    /// Subject.
    pub subject: String,
    /// Text body.
    pub body_text: String,
    /// HTML body.
    pub body_html: Option<String>,
    /// `Message-ID` without angle brackets.
    pub message_id: Option<String>,
    /// The email log entry.
    pub email_log_id: Option<String>,
    /// The agent trace that wrote it.
    pub agent_run_id: Option<String>,
    /// Created.
    pub created_at: OffsetDateTime,
    /// Sent.
    pub sent_at: Option<OffsetDateTime>,
}

const SELECT: &str = "SELECT t.*, o.number AS linked_order_number, o.status AS linked_order_status \
     FROM tickets t LEFT JOIN orders o ON o.org_id = t.org_id AND o.id = t.order_id";

repository! {
    /// Tickets (read side).
    Tickets
}

impl<'a> Tickets<'a> {
    /// One ticket.
    pub async fn get(&self, id: &TicketId) -> Result<Option<TicketRow>, SqlError> {
        self.sql
            .query_first_as(
                &format!("{SELECT} WHERE t.org_id = ?1 AND t.id = ?2"),
                &params![self.org(), id],
            )
            .await
    }

    /// One of the customer's tickets (`None` if someone else's).
    pub async fn get_for_user(
        &self,
        user: &UserId,
        id: &TicketId,
    ) -> Result<Option<TicketRow>, SqlError> {
        self.sql
            .query_first_as(
                &format!("{SELECT} WHERE t.org_id = ?1 AND t.id = ?2 AND t.user_id = ?3"),
                &params![self.org(), id, user],
            )
            .await
    }

    /// Tickets, most recently updated first.
    pub async fn list(
        &self,
        filter: &TicketFilter,
        page: &PageRequest,
    ) -> Result<(Vec<TicketRow>, Option<String>), SqlError> {
        let (key, id) = page.desc_bounds();
        let rows: Vec<TicketRow> = self
            .sql
            .query_as(
                &format!(
                    "{SELECT} WHERE t.org_id = ?1 AND (?2 IS NULL OR t.user_id = ?2) AND (?3 IS NULL OR t.status = ?3) \
                     AND (?4 IS NULL OR (?4 = 1) = (t.status <> 'resolved')) AND (?5 IS NULL OR t.intent = ?5) \
                     AND (?6 IS NULL OR t.assignee = ?6) AND (t.updated_at < ?7 OR (t.updated_at = ?7 AND t.id < ?8)) \
                     ORDER BY t.updated_at DESC, t.id DESC LIMIT ?9"
                ),
                &params![
                    self.org(),
                    filter.user_id.as_ref(),
                    filter.status,
                    filter.open,
                    filter.intent,
                    filter.assignee,
                    key,
                    id,
                    page.fetch()
                ],
            )
            .await?;
        Ok(page.finish(rows, |r| {
            Cursor::new(column::millis::to_millis(r.updated_at), r.id.to_string())
        }))
    }

    /// Tickets about an order, newest first.
    pub async fn for_order(&self, order: &OrderId) -> Result<Vec<TicketRow>, SqlError> {
        self.sql
            .query_as(
                &format!(
                    "{SELECT} WHERE t.org_id = ?1 AND t.order_id = ?2 ORDER BY t.updated_at DESC"
                ),
                &params![self.org(), order],
            )
            .await
    }

    /// The conversation, oldest first. `customer_view` drops drafts and discarded drafts.
    pub async fn messages(
        &self,
        id: &TicketId,
        customer_view: bool,
    ) -> Result<Vec<TicketMessageRow>, SqlError> {
        self.sql
            .query_as(
                "SELECT seq, direction, author, author_user_id, channel, status, subject, body_text, body_html, \
                 message_id, created_at, sent_at FROM ticket_messages \
                 WHERE org_id = ?1 AND ticket_id = ?2 AND (?3 = 0 OR status IN ('received', 'sent')) ORDER BY seq",
                &params![self.org(), id, customer_view],
            )
            .await
    }

    /// Messages the customer can see.
    pub async fn visible_message_count(&self, id: &TicketId) -> Result<u32, SqlError> {
        Ok(self
            .sql
            .query_scalar(
                "SELECT count(*) AS n FROM ticket_messages WHERE org_id = ?1 AND ticket_id = ?2 AND status IN ('received', 'sent')",
                &params![self.org(), id],
            )
            .await?
            .unwrap_or(0))
    }

    /// Unresolved tickets in the org (`None`) or of one customer, or about one order.
    pub async fn open_count(
        &self,
        user: Option<&UserId>,
        order: Option<&OrderId>,
    ) -> Result<u32, SqlError> {
        Ok(self
            .sql
            .query_scalar(
                "SELECT count(*) AS n FROM tickets WHERE org_id = ?1 AND status <> 'resolved' \
                 AND (?2 IS NULL OR user_id = ?2) AND (?3 IS NULL OR order_id = ?3)",
                &params![self.org(), user, order],
            )
            .await?
            .unwrap_or(0))
    }

    /// The ticket a message ID belongs to (`ticket_messages.message_id`): mail-in's threading
    /// fallback. `ids` are tried in order; at most the last 20 are looked at.
    pub async fn find_by_message_ids(&self, ids: &[String]) -> Result<Option<TicketId>, SqlError> {
        #[derive(Deserialize)]
        struct Hit {
            ticket_id: TicketId,
        }
        for id in ids.iter().rev().take(20) {
            let hit: Option<Hit> = self
                .sql
                .query_first_as(
                    "SELECT ticket_id FROM ticket_messages WHERE org_id = ?1 AND message_id = ?2 LIMIT 1",
                    &params![self.org(), id.as_str()],
                )
                .await?;
            if let Some(hit) = hit {
                return Ok(Some(hit.ticket_id));
            }
        }
        Ok(None)
    }

    // -- writes (the `SupportTicket` VO) -----------------------------------------------------

    /// Creates or overwrites the ticket row with the object's state.
    pub fn save_statement(&self, t: &TicketRecord) -> Result<Statement, SqlError> {
        Ok(stmt(
            "INSERT INTO tickets (org_id, id, user_id, customer_email, subject, channel, status, assignee, order_id, \
             order_number, intent, intent_confidence, classification_json, route, awaiting, escalation_reason, \
             reply_confidence, message_count, last_message_at, created_at, updated_at, resolved_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22) \
             ON CONFLICT (org_id, id) DO UPDATE SET status = excluded.status, assignee = excluded.assignee, \
             order_id = excluded.order_id, order_number = excluded.order_number, intent = excluded.intent, \
             intent_confidence = excluded.intent_confidence, classification_json = excluded.classification_json, \
             route = excluded.route, awaiting = excluded.awaiting, escalation_reason = excluded.escalation_reason, \
             reply_confidence = excluded.reply_confidence, message_count = excluded.message_count, \
             last_message_at = excluded.last_message_at, updated_at = excluded.updated_at, \
             resolved_at = excluded.resolved_at",
            vec![
                self.org().into(),
                (&t.id).into(),
                (&t.user_id).into(),
                t.customer_email.to_lowercase().into(),
                t.subject.as_str().into(),
                t.channel.into(),
                t.status.into(),
                t.assignee.into(),
                t.order_id.as_ref().into(),
                t.order_number.into(),
                t.intent.into(),
                t.intent_confidence.map(|p| p.clamp(0.0, 1.0)).into(),
                match &t.classification {
                    Some(c) => json_param(c)?,
                    None => crate::Param::Null,
                },
                t.route.map(TicketRoute::as_str).into(),
                t.awaiting.map(Awaiting::as_str).into(),
                t.escalation_reason.clone().into(),
                t.reply_confidence.map(|p| p.clamp(0.0, 1.0)).into(),
                t.message_count.into(),
                t.last_message_at.into(),
                t.created_at.into(),
                t.updated_at.into(),
                t.resolved_at.into(),
            ],
        ))
    }

    /// Appends a message (idempotent on its `seq`; a `Message-ID` already stored is skipped).
    pub fn insert_message_statement(&self, ticket: &TicketId, m: &NewTicketMessage) -> Statement {
        stmt(
            "INSERT INTO ticket_messages (org_id, ticket_id, seq, direction, author, author_user_id, channel, status, \
             subject, body_text, body_html, message_id, email_log_id, agent_run_id, created_at, sent_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16) ON CONFLICT DO NOTHING",
            params![
                self.org(),
                ticket,
                m.seq,
                m.direction.as_str(),
                m.author,
                m.author_user_id.as_ref(),
                m.channel,
                m.status.as_str(),
                m.subject.as_str(),
                m.body_text.as_str(),
                m.body_html.clone(),
                m.message_id.clone(),
                m.email_log_id.clone(),
                m.agent_run_id.clone(),
                m.created_at,
                m.sent_at
            ],
        )
    }

    /// Settles a draft: `sent` (with the text as sent, possibly edited, and its `Message-ID`) or
    /// `discarded`. Only drafts change.
    #[allow(clippy::too_many_arguments)]
    pub fn settle_draft_statement(
        &self,
        ticket: &TicketId,
        seq: u32,
        status: MessageStatus,
        author: Actor,
        author_user_id: Option<&UserId>,
        body: Option<(&str, Option<&str>)>,
        message_id: Option<&str>,
        sent_at: Option<OffsetDateTime>,
    ) -> Statement {
        stmt(
            "UPDATE ticket_messages SET status = ?4, author = ?5, author_user_id = coalesce(?6, author_user_id), \
             body_text = coalesce(?7, body_text), body_html = coalesce(?8, body_html), \
             message_id = coalesce(?9, message_id), sent_at = ?10 \
             WHERE org_id = ?1 AND ticket_id = ?2 AND seq = ?3 AND status = 'draft'",
            params![
                self.org(),
                ticket,
                seq,
                status.as_str(),
                author,
                author_user_id,
                body.map(|(text, _)| text),
                body.and_then(|(_, html)| html),
                message_id,
                sent_at
            ],
        )
    }
}
