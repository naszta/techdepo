//! The caller's support tickets and the "Send as email" form.
//!
//! Tickets are read from D1 (`tickets`, `ticket_messages`, written by the `SupportTicket` VO).
//! The form enters the same pipeline as mail-in: a one-way `SupportTicket/inbound` with
//! `channel = web` (the ticket logs it to `email_log`), under the same per-org hourly limit as
//! mail-in (`429 rate_limited`). If the services are not registered with Restate, ingress rejects
//! the call and the route answers `503 unavailable`.

use techdepo_api_types::endpoints::{GetTicket, ListTickets, SendSupportMessage};
use techdepo_api_types::ingress::{InboundMessage, SupportTicketInbound, idempotency_key, keys};
use techdepo_api_types::support::{SendSupportMessageResponse, TicketDetail, TicketSummary};
use techdepo_api_types::{ApiError, Channel, Endpoint, ErrorCode, Page, TicketId, Validate};
use techdepo_db::tickets::{TicketFilter, TicketRow};
use techdepo_db::{PageRequest, views};

use crate::app::{App, now};
use crate::assemble;
use crate::auth::Caller;
use crate::endpoint::Input;
use crate::error::{ApiResult, DbResultExt, FoundExt, bad_request};

/// Parses a ticket ID path parameter (`T-7F2M9Q`).
pub fn ticket_id(param: &str) -> ApiResult<TicketId> {
    TicketId::parse(param).map_err(|e| bad_request(e.to_string()))
}

async fn own_ticket(app: &App, caller: &Caller, id: &TicketId) -> ApiResult<TicketRow> {
    app.db
        .tickets(&caller.scope())
        .get_for_user(&caller.user_id, id)
        .await
        .found(format!("Ticket {id}"))
}

pub async fn list(
    app: App,
    caller: Caller,
    input: Input<ListTickets>,
) -> ApiResult<Page<TicketSummary>> {
    let q = input.query;
    let scope = caller.scope();
    let filter = TicketFilter {
        user_id: Some(caller.user_id.clone()),
        status: q.status,
        ..TicketFilter::default()
    };
    let page = PageRequest::from_query(q.cursor.as_deref(), q.limit);
    let (rows, next_cursor) = app.db.tickets(&scope).list(&filter, &page).await.db()?;
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(assemble::ticket_summary(&app, &scope, row).await?);
    }
    Ok(Page { items, next_cursor })
}

pub async fn detail(app: App, caller: Caller, input: Input<GetTicket>) -> ApiResult<TicketDetail> {
    let id = ticket_id(input.param(0))?;
    let row = own_ticket(&app, &caller, &id).await?;
    let scope = caller.scope();
    let messages = app.db.tickets(&scope).messages(&id, true).await.db()?;
    Ok(TicketDetail {
        ticket: assemble::ticket_summary(&app, &scope, &row).await?,
        messages: messages.iter().map(views::ticket_message).collect(),
        reply_address: app.support().ticket_reply(&caller.org.public_id, &id),
    })
}

/// The "Send as email" form: `Message-ID` `web.<client key>@<domain>`; a new ticket's ID is
/// `TicketId::derive(public org, Message-ID)`. One-way; the SPA polls the ticket.
pub async fn send_message(
    app: App,
    caller: Caller,
    input: Input<SendSupportMessage>,
) -> ApiResult<SendSupportMessageResponse> {
    let req = input.body;
    req.validate()?;
    let message_id = format!("web.{}@{}", req.idempotency_key, app.domain());
    let ticket_id = match &req.ticket_id {
        Some(id) => own_ticket(&app, &caller, id).await?.id,
        None => TicketId::derive(&caller.org.public_id, &message_id),
    };
    // The org's hourly inbound limit, shared with mail-in (the ticket logs a web message to
    // `email_log` like an accepted email): every message costs a classifier pass. A retry of a
    // message already accepted is not counted again.
    let scope = caller.scope();
    let email_log = app.db.email_log(&scope);
    if email_log
        .inbound_by_message_id(&message_id)
        .await
        .db()?
        .is_none()
        && email_log
            .inbound_accepted_since(now() - time::Duration::hours(1))
            .await
            .db()?
            >= app.settings.max_inbound_per_hour
    {
        return Err(ApiError::new(
            ErrorCode::RateLimited,
            "Your organization sent too many support messages in the last hour. Please try \
             again later.",
        ));
    }
    let key = idempotency_key(
        caller.org_id(),
        &caller.user_id,
        SendSupportMessage::ROUTE.name,
        &req.idempotency_key,
    );
    let message = InboundMessage {
        org_id: caller.org_id().clone(),
        public_org_id: caller.org.public_id.clone(),
        ticket_id: ticket_id.clone(),
        user_id: caller.user_id.clone(),
        customer_email: caller.email.clone(),
        customer_name: caller.name.clone(),
        channel: Channel::Web,
        message_id: message_id.clone(),
        in_reply_to: None,
        references: Vec::new(),
        subject: req.subject,
        body_text: req.body,
        received_at: now(),
        email_log_id: None,
    };
    app.ingress
        .send::<SupportTicketInbound>(
            Some(&keys::ticket(caller.org_id(), &ticket_id)),
            &message,
            Some(&key),
        )
        .await?;
    Ok(SendSupportMessageResponse {
        ticket_id,
        message_id,
    })
}
