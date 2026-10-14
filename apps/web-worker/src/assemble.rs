//! DTOs that need more than one row: they read the related records through the repositories and
//! build the response with `techdepo_db::views`. Pure row → DTO conversions live in
//! `techdepo_db::views`; this module only gathers what they need.

use std::collections::HashMap;

use techdepo_api_types::admin::ShipmentTruth;
use techdepo_api_types::cases::{InvestigationSummary, RefundSummary};
use techdepo_api_types::demo::DemoPanel;
use techdepo_api_types::orders::{OrderDetail, OrderEvent, OrderSummary, support_mailto};
use techdepo_api_types::support::TicketSummary;
use techdepo_api_types::tasks::{HumanTaskDetail, HumanTaskSummary, RecordedDecision};
use techdepo_api_types::vocab::{HumanTaskStatus, RefKind};
use techdepo_api_types::{CustomerRef, OrderRef, TicketRef};
use techdepo_db::investigations::InvestigationRow;
use techdepo_db::orders::{OrderEventRow, OrderRow};
use techdepo_db::refunds::RefundRow;
use techdepo_db::tasks::TaskRow;
use techdepo_db::tickets::TicketRow;
use techdepo_db::{OrgScope, views};
use techdepo_domain::{OrderId, OrderNumber, TicketId};
use time::OffsetDateTime;

use crate::app::App;
use crate::auth::Caller;
use crate::error::{ApiResult, DbResultExt};

// -- orders -------------------------------------------------------------------------------------

/// List rows with headlines and the numbers of the orders they replace.
pub async fn order_summaries(
    app: &App,
    scope: &OrgScope,
    rows: &[OrderRow],
) -> ApiResult<Vec<OrderSummary>> {
    let orders = app.db.orders(scope);
    let ids: Vec<OrderId> = rows.iter().map(|r| r.id.clone()).collect();
    let headlines: HashMap<OrderId, (String, u32)> = orders
        .headlines(&ids)
        .await
        .db()?
        .into_iter()
        .map(|(id, headline, units)| (id, (headline, units)))
        .collect();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let (headline, units) = headlines.get(&row.id).cloned().unwrap_or_default();
        let replaces = replaced_number(app, scope, row).await?;
        out.push(views::order_summary(row, headline, units, replaces));
    }
    Ok(out)
}

async fn replaced_number(
    app: &App,
    scope: &OrgScope,
    row: &OrderRow,
) -> ApiResult<Option<OrderNumber>> {
    match &row.replacement_for {
        Some(id) => Ok(app.db.orders(scope).get(id).await.db()?.map(|o| o.number)),
        None => Ok(None),
    }
}

/// The order page. `internal`: the admin view (every timeline entry, internal carrier events).
pub async fn order_detail(
    app: &App,
    caller: &Caller,
    row: &OrderRow,
    internal: bool,
    now: OffsetDateTime,
) -> ApiResult<OrderDetail> {
    let scope = caller.scope();
    let db = &app.db;
    let orders = db.orders(&scope);
    let items = orders.items(&row.id).await.db()?;
    let events = orders.events(&row.id).await.db()?;
    let clock = orders.clock(&row.id).await.db()?;
    let payments = db.payments(&scope).for_order(&row.id).await.db()?;
    let invoices = db.invoices(&scope).for_order(&row.id).await.db()?;
    let shipment = match db.shipments(&scope).for_order(&row.id).await.db()? {
        Some(s) => {
            let events = db.shipments(&scope).events(&s.id, internal).await.db()?;
            Some(views::shipment_view(&s, &events))
        }
        None => None,
    };
    let tickets = db.tickets(&scope).for_order(&row.id).await.db()?;
    let (headline, units) = views::headline(&items);
    let replaces = replaced_number(app, &scope, row).await?;

    let labels: HashMap<String, String> = invoices
        .iter()
        .map(|i| (i.id.as_str().to_owned(), i.number.to_string()))
        .chain(
            shipment
                .iter()
                .map(|s| (s.id.as_str().to_owned(), s.tracking_number.to_string())),
        )
        .chain(
            payments
                .iter()
                .map(|p| (p.id.as_str().to_owned(), p.provider_ref.clone())),
        )
        .collect();
    let timeline = events
        .iter()
        .filter(|e| internal || e.kind.is_customer_visible())
        .map(|e| timeline_event(e, &labels))
        .collect();
    let address = app.support().address(&caller.org.public_id);
    Ok(OrderDetail {
        order: views::order_summary(row, headline, units, replaces),
        items: items.iter().map(views::order_item).collect(),
        totals: views::order_totals(row),
        shipping_address: row.shipping_address.clone(),
        billing_address: row.billing_address.clone(),
        payment_method: row.payment_method,
        compat_override: row.compat_override,
        milestones: views::order_milestones(row),
        timeline,
        payments: payments.iter().map(views::payment_entry).collect(),
        invoices: invoices.iter().map(views::invoice_summary).collect(),
        shipment,
        tickets: tickets.iter().map(views::ticket_ref).collect(),
        simulated_now: clock.now(now),
        support_mailto: support_mailto(&address, row.number),
    })
}

fn timeline_event(row: &OrderEventRow, labels: &HashMap<String, String>) -> OrderEvent {
    let label = match (row.ref_kind, &row.ref_id) {
        (Some(RefKind::Order), _) => None,
        (_, Some(id)) => labels.get(id).cloned(),
        _ => None,
    };
    views::order_event(row, label)
}

/// The demo controls drawer of an order.
pub async fn demo_panel(
    app: &App,
    scope: &OrgScope,
    row: &OrderRow,
    now: OffsetDateTime,
) -> ApiResult<DemoPanel> {
    let orders = app.db.orders(scope);
    let faults = orders.faults(&row.id).await.db()?;
    let clock = orders.clock(&row.id).await.db()?;
    let shipment = app.db.shipments(scope).for_order(&row.id).await.db()?;
    let calendar = app.db.orgs(scope).settings().await.db()?.calendar();
    Ok(views::demo_panel(
        row,
        faults.as_ref(),
        clock,
        shipment.as_ref(),
        &calendar,
        now,
    ))
}

/// The shipment's hidden truth, once shipped.
pub async fn shipment_truth(
    app: &App,
    scope: &OrgScope,
    order: &OrderId,
) -> ApiResult<Option<ShipmentTruth>> {
    Ok(app
        .db
        .shipments(scope)
        .for_order(order)
        .await
        .db()?
        .map(|s| ShipmentTruth {
            lost_outcome: s.lost_outcome,
            trace_status: s.trace_status,
        }))
}

// -- tickets ------------------------------------------------------------------------------------

/// A ticket in a list, with its customer-visible message count.
pub async fn ticket_summary(
    app: &App,
    scope: &OrgScope,
    row: &TicketRow,
) -> ApiResult<TicketSummary> {
    let visible = app
        .db
        .tickets(scope)
        .visible_message_count(&row.id)
        .await
        .db()?;
    Ok(views::ticket_summary(row, visible))
}

/// The ticket's customer, with the name from the member cache when known.
pub async fn ticket_customer(
    app: &App,
    scope: &OrgScope,
    row: &TicketRow,
) -> ApiResult<CustomerRef> {
    let name = app
        .db
        .orgs(scope)
        .member(&row.user_id)
        .await
        .db()?
        .and_then(|m| m.name);
    Ok(CustomerRef {
        user_id: row.user_id.clone(),
        email: row.customer_email.clone(),
        name,
    })
}

async fn ticket_ref(app: &App, scope: &OrgScope, id: &TicketId) -> ApiResult<Option<TicketRef>> {
    Ok(app
        .db
        .tickets(scope)
        .get(id)
        .await
        .db()?
        .map(|t| views::ticket_ref(&t)))
}

// -- human tasks --------------------------------------------------------------------------------

/// A task in the inbox, with its order, ticket and customer.
pub async fn task_summary(
    app: &App,
    scope: &OrgScope,
    row: &TaskRow,
    now: OffsetDateTime,
) -> ApiResult<HumanTaskSummary> {
    let order = match &row.order_id {
        Some(id) => app.db.orders(scope).get(id).await.db()?,
        None => None,
    };
    let ticket = match &row.ticket_id {
        Some(id) => app.db.tickets(scope).get(id).await.db()?,
        None => None,
    };
    let customer = order.as_ref().map(views::order_customer).or_else(|| {
        ticket.as_ref().map(|t| CustomerRef {
            user_id: t.user_id.clone(),
            email: t.customer_email.clone(),
            name: None,
        })
    });
    Ok(HumanTaskSummary {
        id: row.id.clone(),
        kind: row.kind,
        status: row.status,
        title: row.title.clone(),
        summary: row.summary.clone(),
        order: order.as_ref().map(views::order_ref),
        ticket: ticket.as_ref().map(views::ticket_ref),
        customer,
        sla_due_at: row.sla_due_at,
        overdue: row.status == HumanTaskStatus::Pending && row.sla_due_at < now,
        created_at: row.created_at,
    })
}

/// Many task summaries.
pub async fn task_summaries(
    app: &App,
    scope: &OrgScope,
    rows: &[TaskRow],
    now: OffsetDateTime,
) -> ApiResult<Vec<HumanTaskSummary>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(task_summary(app, scope, row, now).await?);
    }
    Ok(out)
}

/// A task with its context, evidence, allowed decisions and the recorded decision.
pub async fn task_detail(
    app: &App,
    scope: &OrgScope,
    row: &TaskRow,
    now: OffsetDateTime,
) -> ApiResult<HumanTaskDetail> {
    let decision = match (&row.decision_detail, &row.decided_by, row.decided_at) {
        (Some(decision), Some(by), Some(at)) => Some(RecordedDecision {
            decision: decision.clone(),
            note: row.note.clone(),
            decided_by: by.clone(),
            decided_at: at,
        }),
        _ => None,
    };
    Ok(HumanTaskDetail {
        task: task_summary(app, scope, row, now).await?,
        context: row.context.clone(),
        evidence: row.evidence.clone(),
        allowed_decisions: row.allowed_decisions.clone(),
        ticket_id: row.ticket_id.clone(),
        decision,
    })
}

// -- cases --------------------------------------------------------------------------------------

/// A refund in a list.
pub fn refund_summary(row: &RefundRow) -> RefundSummary {
    RefundSummary {
        id: row.id.clone(),
        status: row.status,
        order: OrderRef {
            id: row.order_id.clone(),
            number: row.order_number,
            status: row.order_status,
        },
        customer: CustomerRef {
            user_id: row.user_id.clone(),
            email: row.customer_email.clone(),
            name: row.customer_name.clone(),
        },
        amount: row.amount,
        reason: row.reason,
        reason_text: row.reason_text.clone(),
        pre_approved: row.pre_approved,
        created_at: row.created_at,
        completed_at: row.completed_at,
    }
}

/// An investigation in a list, with its ticket.
pub async fn investigation_summary(
    app: &App,
    scope: &OrgScope,
    row: &InvestigationRow,
) -> ApiResult<InvestigationSummary> {
    let ticket = match &row.ticket_id {
        Some(id) => ticket_ref(app, scope, id).await?,
        None => None,
    };
    Ok(InvestigationSummary {
        id: row.id.clone(),
        status: row.status,
        order: OrderRef {
            id: row.order_id.clone(),
            number: row.order_number,
            status: row.order_status,
        },
        ticket,
        carrier_result: row.carrier_result,
        decision: row.decision,
        outcome: row.outcome,
        rounds: row.rounds,
        opened_by: row.opened_by,
        created_at: row.created_at,
        updated_at: row.updated_at,
        closed_at: row.closed_at,
    })
}

/// Many investigation summaries.
pub async fn investigation_summaries(
    app: &App,
    scope: &OrgScope,
    rows: &[InvestigationRow],
) -> ApiResult<Vec<InvestigationSummary>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(investigation_summary(app, scope, row).await?);
    }
    Ok(out)
}
