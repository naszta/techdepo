//! `Notifications` (service, ingress-private): every email to a customer or an admin.
//!
//! Callers say *what happened*; this service loads the facts from D1, picks the template
//! ([`templates`]), renders it in the shared layout ([`content`]), sets the addresses and
//! threading headers ([`addressing`]) and hands the result to `Email/send`, which logs it,
//! applies the demo's provider outage and retries until it is delivered.
//!
//! | Handler | Caller | What |
//! |---|---|---|
//! | `order` | `Order` (one-way), `RefundWorkflow`, `Invoicing` | the order lifecycle emails; an `email_sent` entry goes on the order's timeline |
//! | `ticket` | `SupportTicket` (support pipeline) | a support template or free-text reply, threaded into the ticket |
//! | `task_nudge` | `HumanTasks` (delayed) | the SLA nudge to the org's admins |
//!
//! Agents never call this service: they return an outcome and `SupportTicket` decides whether
//! and how to send (support.md §6). Callers inside the `Order` object must call `order`
//! one-way, because it records the timeline entry through `Order/record_event`.

pub mod addressing;
pub mod content;
pub mod templates;

use std::sync::Arc;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::admin::InvoiceReissued;
use techdepo_api_types::vocab::{OrderEventKind, PaymentMethod, RefKind};
use techdepo_db::orders::{OrderItemRow, OrderRow};
use techdepo_db::{Db, OrgScope};
use techdepo_domain::{
    Actor, HumanTaskId, InvoiceNumber, Money, OrderId, OrgId, PublicOrgId, RefundId, UserId,
};
use techdepo_ports::{Attachment, Header, Mailbox, OutboundEmail};

use crate::email::{EmailClient, SendEmailInput, SentEmail};
use crate::errors::{bug, not_found, sql};
use crate::order::{OrderClient, RecordEventInput};
use crate::{Config, Deps};
use content::{EmailContent, Frame, ItemLine, Rendered};
use templates::{
    InvoiceFacts, Links, OrderFacts, OrderTemplate, ShipmentFacts, SupportTemplate, TicketFacts,
};

/// What happened to an order (`Notifications/order`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OrderNotice {
    /// Paid: confirmation with the invoice summary and the invoice attached.
    Confirmation,
    /// A payment was captured.
    PaymentReceived {
        /// Amount.
        amount: Money,
    },
    /// Handed to the carrier.
    Shipped,
    /// Delivered.
    Delivered,
    /// Cancelled before payment.
    Cancelled {
        /// Why.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// A refund awaits approval.
    RefundStarted {
        /// Refund.
        refund_id: RefundId,
        /// Amount.
        amount: Money,
    },
    /// A refund completed.
    RefundDone {
        /// Refund.
        refund_id: RefundId,
        /// Amount.
        amount: Money,
        /// The whole order was refunded.
        full: bool,
        /// Credit note, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credit_note: Option<InvoiceNumber>,
    },
    /// A refund was rejected.
    RefundRejected {
        /// Refund.
        refund_id: RefundId,
        /// Amount.
        amount: Money,
        /// Why.
        reason: String,
    },
    /// The invoice was voided and reissued.
    InvoiceReissued {
        /// The documents.
        documents: InvoiceReissued,
        /// Why.
        reason: String,
    },
}

/// `Notifications/order` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderNoticeInput {
    /// Org.
    pub org_id: OrgId,
    /// Order.
    pub order_id: OrderId,
    /// What happened.
    pub notice: OrderNotice,
}

/// `Notifications/ticket` input: one support email in a ticket's thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketNoticeInput {
    /// Org.
    pub org_id: OrgId,
    /// The ticket.
    pub ticket: TicketFacts,
    /// The customer.
    pub to: Mailbox,
    /// The customer's member ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<UserId>,
    /// The message's number in the ticket (its `Message-ID` is `<ticket>.<n>@<domain>`).
    pub seq: u32,
    /// The customer message this answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    /// The thread's message IDs, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<String>,
    /// Related order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_id: Option<OrderId>,
    /// What to send.
    pub template: SupportTemplate,
}

/// `Notifications/ticket` output: what was sent, for the ticket's message record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketEmailSent {
    /// `Message-ID`.
    pub message_id: String,
    /// The log entry and provider ID.
    pub sent: SentEmail,
    /// Subject, text and HTML as sent.
    pub rendered: Rendered,
}

/// `Notifications/task_nudge` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskNudgeInput {
    /// Org.
    pub org_id: OrgId,
    /// The overdue task.
    pub task_id: HumanTaskId,
}

/// An email ready for `Email/send`, plus what the timeline needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Prepared {
    email: OutboundEmail,
    template: String,
    user_id: Option<UserId>,
}

// -- facts ----------------------------------------------------------------------------------------

fn order_facts(order: &OrderRow, items: &[OrderItemRow]) -> OrderFacts {
    OrderFacts {
        number: order.number,
        customer_name: order.customer_name.clone(),
        items: items
            .iter()
            .map(|i| ItemLine {
                name: i.name.clone(),
                sku: i.sku.to_string(),
                quantity: i.quantity,
                amount: i.line_total.to_string(),
            })
            .collect(),
        items_total: order.items_total,
        shipping: order.shipping,
        total: order.total,
        vat_rate: order.vat_rate,
        ship_to: order.shipping_address.lines(),
        store_paid: order.payment_method == PaymentMethod::Store,
    }
}

fn frame_for_order(config: &Config, public: &PublicOrgId, order: &OrderRow) -> Frame {
    Frame {
        support_address: addressing::support_address(config, public),
        footer: format!(
            "You get this email because you placed order {} in the TechDepó demo shop.",
            order.number
        ),
    }
}

/// Renders `content` into an [`OutboundEmail`].
#[allow(clippy::too_many_arguments)]
fn outbound(
    content: &EmailContent,
    frame: &Frame,
    message_id: String,
    from: Mailbox,
    to: Mailbox,
    reply_to: Option<Mailbox>,
    headers: Vec<Header>,
    attachments: Vec<Attachment>,
) -> Result<(OutboundEmail, Rendered), HandlerError> {
    let rendered = bug(content::render(content, frame))?;
    Ok((
        OutboundEmail {
            message_id,
            from,
            to,
            reply_to,
            subject: rendered.subject.clone(),
            text: rendered.text.clone(),
            html: Some(rendered.html.clone()),
            headers,
            attachments,
        },
        rendered,
    ))
}

fn html_attachment(number: &InvoiceNumber, html: String) -> Attachment {
    Attachment {
        filename: format!("{number}.html"),
        content_type: "text/html; charset=utf-8".to_owned(),
        content: html,
    }
}

async fn public_id(db: &Db, scope: &OrgScope) -> Result<PublicOrgId, HandlerError> {
    db.orgs(scope)
        .get()
        .await
        .map_err(sql)?
        .map(|o| o.public_id)
        .ok_or_else(|| not_found("org"))
}

/// Loads the facts of an order notice and renders it.
async fn prepare_order(
    db: &Db,
    config: &Config,
    input: &OrderNoticeInput,
) -> Result<Prepared, HandlerError> {
    let scope = OrgScope::verified(input.org_id.clone());
    let public = public_id(db, &scope).await?;
    let orders = db.orders(&scope);
    let order = orders
        .get(&input.order_id)
        .await
        .map_err(sql)?
        .ok_or_else(|| not_found("order"))?;
    let items = orders.items(&order.id).await.map_err(sql)?;
    let mut attachments = Vec::new();
    let (template, reference) = match &input.notice {
        OrderNotice::Confirmation => {
            let invoices = db.invoices(&scope);
            let invoice = invoices.current_for_order(&order.id).await.map_err(sql)?;
            if let Some(inv) = &invoice
                && let Some(html) = invoices.html(&inv.id).await.map_err(sql)?
            {
                attachments.push(html_attachment(&inv.number, html));
            }
            (
                OrderTemplate::OrderConfirmation {
                    invoice: invoice.map(|i| InvoiceFacts {
                        number: i.number,
                        issue_date: i.issue_date,
                        gross: i.gross,
                        vat_rate: i.vat_rate,
                    }),
                },
                None,
            )
        }
        OrderNotice::PaymentReceived { amount } => {
            (OrderTemplate::PaymentReceived { amount: *amount }, None)
        }
        OrderNotice::Shipped | OrderNotice::Delivered => {
            let shipment = db
                .shipments(&scope)
                .for_order(&order.id)
                .await
                .map_err(sql)?
                .ok_or_else(|| not_found("shipment"))?;
            if input.notice == OrderNotice::Shipped {
                (
                    OrderTemplate::Shipped {
                        shipment: ShipmentFacts {
                            carrier: shipment.carrier,
                            tracking_number: shipment.tracking_number,
                            shipped_on: shipment.shipped_on,
                            eta: shipment.eta,
                        },
                    },
                    None,
                )
            } else {
                (
                    OrderTemplate::Delivered {
                        on: shipment.delivered_on.unwrap_or(shipment.eta),
                    },
                    None,
                )
            }
        }
        OrderNotice::Cancelled { reason } => (
            OrderTemplate::Cancelled {
                reason: reason.clone(),
            },
            None,
        ),
        OrderNotice::RefundStarted { refund_id, amount } => (
            OrderTemplate::RefundStarted { amount: *amount },
            Some(refund_id.to_string()),
        ),
        OrderNotice::RefundDone {
            refund_id,
            amount,
            full,
            credit_note,
        } => (
            OrderTemplate::RefundDone {
                amount: *amount,
                full: *full,
                credit_note: *credit_note,
            },
            Some(refund_id.to_string()),
        ),
        OrderNotice::RefundRejected {
            refund_id,
            amount,
            reason,
        } => (
            OrderTemplate::RefundRejected {
                amount: *amount,
                reason: reason.clone(),
            },
            Some(refund_id.to_string()),
        ),
        OrderNotice::InvoiceReissued { documents, reason } => {
            if let Some(html) = db
                .invoices(&scope)
                .html(&documents.reissued.id)
                .await
                .map_err(sql)?
            {
                attachments.push(html_attachment(&documents.reissued.number, html));
            }
            (
                OrderTemplate::InvoiceReissued {
                    voided: documents.voided.number,
                    credit_note: documents.credit_note.number,
                    reissued: documents.reissued.number,
                    reason: reason.clone(),
                },
                Some(documents.reissued.id.to_string()),
            )
        }
    };
    let content = templates::order_email(
        &order_facts(&order, &items),
        &template,
        &Links::new(&config.app_base_url),
    );
    let message_id =
        addressing::order_message_id(config, &order.id, template.name(), reference.as_deref());
    let (email, _) = outbound(
        &content,
        &frame_for_order(config, &public, &order),
        message_id,
        addressing::from(config, &public),
        Mailbox {
            email: order.customer_email.clone(),
            name: order.customer_name.clone(),
        },
        Some(addressing::from(config, &public)),
        vec![addressing::auto_generated()],
        attachments,
    )?;
    Ok(Prepared {
        email,
        template: template.name().to_owned(),
        user_id: Some(order.user_id),
    })
}

/// Renders a support email (also for `SupportTicket`, which stores the body it sends).
pub fn render_ticket_email(
    config: &Config,
    public: &PublicOrgId,
    input: &TicketNoticeInput,
) -> Result<(OutboundEmail, Rendered), HandlerError> {
    let content = templates::support_email(
        &input.ticket,
        &input.template,
        &Links::new(&config.app_base_url),
    );
    let frame = Frame {
        support_address: addressing::support_address(config, public),
        footer: format!(
            "You get this email because you contacted TechDepó support (ticket {}).",
            input.ticket.ticket_id
        ),
    };
    let mut headers =
        addressing::threading_headers(input.in_reply_to.as_deref(), &input.references);
    headers.push(addressing::auto_replied());
    outbound(
        &content,
        &frame,
        addressing::ticket_message_id(config, &input.ticket.ticket_id, input.seq),
        addressing::from(config, public),
        input.to.clone(),
        Some(addressing::ticket_reply_to(
            config,
            public,
            &input.ticket.ticket_id,
        )),
        headers,
        Vec::new(),
    )
}

// -- the service ----------------------------------------------------------------------------------

/// The `Notifications` service.
pub struct Notifications {
    deps: Arc<Deps>,
}

impl Notifications {
    /// The service over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }
}

#[restate_sdk::service(name = "Notifications", ingress_private = true)]
impl Notifications {
    /// Sends an order lifecycle email and puts it on the order's timeline.
    #[handler]
    async fn order(
        &self,
        ctx: Context<'_>,
        input: Json<OrderNoticeInput>,
    ) -> Result<Json<SentEmail>, HandlerError> {
        let input = input.into_inner();
        let (db, deps) = (self.deps.db.clone(), self.deps.clone());
        let step = format!(
            "prepare-{}",
            serde_json::to_value(&input.notice)
                .ok()
                .and_then(|v| v["kind"].as_str().map(str::to_owned))
                .unwrap_or_else(|| "email".to_owned())
        );
        let load = input.clone();
        let prepared: Json<Prepared> = ctx
            .run(|| async move { prepare_order(&db, &deps.config, &load).await.map(Json) })
            .name(step)
            .await?;
        let prepared = prepared.into_inner();
        let subject = prepared.email.subject.clone();
        let message_id = prepared.email.message_id.clone();
        let sent: Json<SentEmail> = ctx
            .service_client::<EmailClient>()
            .send(Json(SendEmailInput {
                org_id: input.org_id.clone(),
                email: prepared.email,
                template: Some(prepared.template.clone()),
                user_id: prepared.user_id,
                ticket_id: None,
                order_id: Some(input.order_id.clone()),
            }))
            .call()
            .await?;
        let sent = sent.into_inner();
        let _ = ctx
            .object_client::<OrderClient>(input.order_id.as_str())
            .record_event(Json(RecordEventInput {
                org_id: input.org_id.clone(),
                kind: OrderEventKind::EmailSent,
                actor: Actor::System,
                actor_user_id: None,
                summary: format!("Email sent: {subject}"),
                reference: Some((RefKind::Email, sent.email_id.to_string())),
                data: serde_json::json!({ "template": prepared.template, "message_id": message_id }),
                refund_pending: None,
                investigation_open: None,
                invocation_id: Some(ctx.invocation_id().to_owned()),
            }))
            .send();
        Ok(Json(sent))
    }

    /// Sends one email in a ticket's thread (support pipeline).
    #[handler]
    async fn ticket(
        &self,
        ctx: Context<'_>,
        input: Json<TicketNoticeInput>,
    ) -> Result<Json<TicketEmailSent>, HandlerError> {
        let input = input.into_inner();
        let (db, deps) = (self.deps.db.clone(), self.deps.clone());
        let load = input.clone();
        let prepared: Json<(OutboundEmail, Rendered)> = ctx
            .run(|| async move {
                let scope = OrgScope::verified(load.org_id.clone());
                let public = public_id(&db, &scope).await?;
                render_ticket_email(&deps.config, &public, &load).map(Json)
            })
            .name(format!("prepare-{}", input.template.name()))
            .await?;
        let (email, rendered) = prepared.into_inner();
        let message_id = email.message_id.clone();
        let sent: Json<SentEmail> = ctx
            .service_client::<EmailClient>()
            .send(Json(SendEmailInput {
                org_id: input.org_id.clone(),
                email,
                template: Some(rendered.template.clone()),
                user_id: input.user_id.clone(),
                ticket_id: Some(input.ticket.ticket_id.clone()),
                order_id: input.order_id.clone(),
            }))
            .call()
            .await?;
        Ok(Json(TicketEmailSent {
            message_id,
            sent: sent.into_inner(),
            rendered,
        }))
    }

    /// Nudges the org's admins about a task that passed its SLA.
    #[handler]
    async fn task_nudge(
        &self,
        ctx: Context<'_>,
        input: Json<TaskNudgeInput>,
    ) -> Result<u32, HandlerError> {
        let input = input.into_inner();
        let (db, deps) = (self.deps.db.clone(), self.deps.clone());
        let load = input.clone();
        let emails: Json<Vec<(OutboundEmail, UserId)>> = ctx
            .run(|| async move {
                let scope = OrgScope::verified(load.org_id.clone());
                let org = db
                    .orgs(&scope)
                    .get()
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| not_found("org"))?;
                let task = db
                    .tasks(&scope)
                    .get(&load.task_id)
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| not_found("task"))?;
                let admins = db.orgs(&scope).admins().await.map_err(sql)?;
                let config = &deps.config;
                let content = templates::task_overdue(
                    &task.id,
                    task.kind.label(),
                    &task.title,
                    &task.summary,
                    &format!(
                        "{} UTC",
                        task.created_at
                            .format(time::macros::format_description!(
                                "[year]-[month]-[day] [hour]:[minute]"
                            ))
                            .unwrap_or_default()
                    ),
                    &Links::new(&config.app_base_url),
                );
                let frame = Frame {
                    support_address: addressing::support_address(config, &org.public_id),
                    footer: format!(
                        "You get this email because you are an admin of {} in the TechDepó demo.",
                        org.name
                    ),
                };
                let mut out = Vec::new();
                for admin in admins {
                    let (email, _) = outbound(
                        &content,
                        &frame,
                        addressing::nudge_message_id(config, &task.id, &admin.user_id),
                        addressing::from(config, &org.public_id),
                        Mailbox {
                            email: admin.email.clone(),
                            name: admin.name.clone(),
                        },
                        None,
                        vec![addressing::auto_generated()],
                        Vec::new(),
                    )?;
                    out.push((email, admin.user_id));
                }
                Ok(Json(out))
            })
            .name("prepare-task-nudge")
            .await?;
        let emails = emails.into_inner();
        let count = u32::try_from(emails.len()).unwrap_or(u32::MAX);
        for (email, user_id) in emails {
            let _ = ctx
                .service_client::<EmailClient>()
                .send(Json(SendEmailInput {
                    org_id: input.org_id.clone(),
                    email,
                    template: Some("task_overdue".to_owned()),
                    user_id: Some(user_id),
                    ticket_id: None,
                    order_id: None,
                }))
                .send();
        }
        Ok(count)
    }
}
