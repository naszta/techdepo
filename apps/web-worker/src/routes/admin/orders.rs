//! Admin order list and page, cancel, refunds and investigations.

use techdepo_api_types::admin::{
    AdminOrderActions, AdminOrderDetail, AdminOrderSummary, OrderUpdated, parse_order_query,
};
use techdepo_api_types::cases::{InvestigationStarted, RefundStarted};
use techdepo_api_types::endpoints::{
    CancelOrder, GetAdminOrder, InitiateRefund, ListAdminOrders, OpenInvestigation,
};
use techdepo_api_types::ingress::{
    CancelOrderInput, InvestigationRun, OrderCancel, RefundWorkflowRun, StartInvestigationInput,
    StartRefundInput, derive_id, idempotency_key, keys,
};
use techdepo_api_types::{
    Actor, Endpoint, InvestigationId, Money, OrderStatus, Page, RefundId, Validate,
};
use techdepo_db::orders::{OrderFilter, OrderRow};
use techdepo_db::refunds::plan_refund;
use techdepo_db::{PageRequest, views};

use crate::app::{App, now};
use crate::assemble;
use crate::auth::Caller;
use crate::endpoint::Input;
use crate::error::{ApiResult, DbResultExt, FoundExt, invalid_transition};
use crate::routes::orders::order_number;

/// Any order of the org by number.
pub async fn org_order(app: &App, caller: &Caller, param: &str) -> ApiResult<OrderRow> {
    let number = order_number(param)?;
    app.db
        .orders(&caller.scope())
        .get_by_number(number)
        .await
        .found(format!("Order {number}"))
}

pub async fn list(
    app: App,
    caller: Caller,
    input: Input<ListAdminOrders>,
) -> ApiResult<Page<AdminOrderSummary>> {
    let q = input.query;
    let scope = caller.scope();
    let query = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let number = query.and_then(parse_order_query);
    let filter = OrderFilter {
        user_id: q.user_id,
        status: q.status,
        include_cancelled: q.include_cancelled.unwrap_or(false),
        flagged: q.flagged.unwrap_or(false),
        number,
        email_like: query.filter(|_| number.is_none()).map(str::to_owned),
    };
    let page = PageRequest::from_query(q.cursor.as_deref(), q.limit);
    let (rows, next_cursor) = app.db.orders(&scope).list(&filter, &page).await.db()?;
    let summaries = assemble::order_summaries(&app, &scope, &rows).await?;
    let mut items = Vec::with_capacity(rows.len());
    for (row, order) in rows.iter().zip(summaries) {
        items.push(AdminOrderSummary {
            order,
            customer: views::order_customer(row),
            open_tickets: app
                .db
                .tickets(&scope)
                .open_count(None, Some(&row.id))
                .await
                .db()?,
        });
    }
    Ok(Page { items, next_cursor })
}

pub async fn detail(
    app: App,
    caller: Caller,
    input: Input<GetAdminOrder>,
) -> ApiResult<AdminOrderDetail> {
    let row = org_order(&app, &caller, input.param(0)).await?;
    let scope = caller.scope();
    let db = &app.db;
    let now = now();
    let order = assemble::order_detail(&app, &caller, &row, true, now).await?;
    let faults = db.orders(&scope).faults(&row.id).await.db()?;
    let investigations = db.investigations(&scope).for_order(&row.id).await.db()?;
    let refunds = db.refunds(&scope).for_order(&row.id).await.db()?;
    let tasks = db.tasks(&scope).for_order(&row.id).await.db()?;
    let refundable = db.payments(&scope).net_captured(&row.id).await.db()?;
    Ok(AdminOrderDetail {
        customer: views::order_customer(&row),
        shipment_truth: assemble::shipment_truth(&app, &scope, &row.id).await?,
        faults: faults.map(|f| f.faults).unwrap_or_default(),
        investigations: assemble::investigation_summaries(&app, &scope, &investigations).await?,
        refunds: refunds.iter().map(assemble::refund_summary).collect(),
        tasks: assemble::task_summaries(&app, &scope, &tasks, now).await?,
        demo: assemble::demo_panel(&app, &scope, &row, now).await?,
        actions: actions(&row, refundable),
        order,
    })
}

/// The admin actions that apply to an order now.
pub fn actions(row: &OrderRow, refundable: Money) -> AdminOrderActions {
    let paid = row.paid_at.is_some()
        && !matches!(row.status, OrderStatus::Cancelled | OrderStatus::Refunded);
    AdminOrderActions {
        can_cancel: row.status == OrderStatus::AwaitingPayment,
        can_refund: paid && refundable.cents() > 0 && !row.refund_pending,
        can_investigate: row.status == OrderStatus::Shipped && !row.investigation_open,
        refundable,
    }
}

/// Cancels an unpaid order (`Order/cancel`); paid orders are refunded instead.
pub async fn cancel(
    app: App,
    caller: Caller,
    input: Input<CancelOrder>,
) -> ApiResult<OrderUpdated> {
    input.body.validate()?;
    let row = org_order(&app, &caller, input.param(0)).await?;
    let cancel = CancelOrderInput {
        org_id: caller.org_id().clone(),
        actor: Actor::Human,
        by: Some(caller.user_id.clone()),
        reason: input.body.reason,
    };
    let status = app
        .ingress
        .call::<OrderCancel>(Some(&keys::order(&row.id)), &cancel, None)
        .await?;
    Ok(OrderUpdated {
        order: techdepo_api_types::OrderRef {
            id: row.id,
            number: row.number,
            status,
        },
    })
}

/// Starts a pre-approved `RefundWorkflow` one-way (a person initiated it); the refund ID
/// derives from the client key.
///
/// The workflow is started one-way, so whatever it would reject is rejected here first, with the
/// same [`plan_refund`] its `open-refund` step runs: a refund that fails there would leave no
/// record behind. Not while another refund of the order is pending (as `actions` says).
pub async fn refund(
    app: App,
    caller: Caller,
    input: Input<InitiateRefund>,
) -> ApiResult<RefundStarted> {
    input.body.validate()?;
    let row = org_order(&app, &caller, input.param(0)).await?;
    let req = input.body;
    let scope = caller.scope();
    let refund_id = RefundId::new(derive_id(
        "ref",
        &[
            caller.org_id().as_str(),
            caller.user_id.as_str(),
            &req.idempotency_key,
        ],
    ));
    // A resubmitted form (same key) gets its refund back, whatever has happened since.
    let others = app.db.refunds(&scope).for_order(&row.id).await.db()?;
    if others.iter().any(|r| r.id == refund_id) {
        return Ok(RefundStarted { refund_id });
    }
    if row.refund_pending {
        return Err(invalid_transition(format!(
            "A refund of {} is already pending: decide that one first",
            row.number
        )));
    }
    let payments = app.db.payments(&scope).for_order(&row.id).await.db()?;
    plan_refund(
        &payments,
        &others,
        req.reason,
        req.charge_payment_id.as_ref(),
        req.amount,
    )?;
    let key = idempotency_key(
        caller.org_id(),
        &caller.user_id,
        InitiateRefund::ROUTE.name,
        &req.idempotency_key,
    );
    let start = StartRefundInput {
        org_id: caller.org_id().clone(),
        order_id: row.id,
        amount: req.amount,
        reason: req.reason,
        reason_text: req.reason_text,
        evidence: Vec::new(),
        pre_approved: true,
        requested_by: Actor::Human,
        requested_by_user: Some(caller.user_id.clone()),
        charge_payment_id: req.charge_payment_id,
        ticket_id: None,
        investigation_id: None,
    };
    app.ingress
        .send::<RefundWorkflowRun>(Some(&keys::refund(&refund_id)), &start, Some(&key))
        .await?;
    Ok(RefundStarted { refund_id })
}

/// Starts an `Investigation` one-way, without a ticket. Shipped, undelivered orders without an
/// open investigation only.
pub async fn investigate(
    app: App,
    caller: Caller,
    input: Input<OpenInvestigation>,
) -> ApiResult<InvestigationStarted> {
    input.body.validate()?;
    let row = org_order(&app, &caller, input.param(0)).await?;
    let req = input.body;
    if row.status != OrderStatus::Shipped || row.investigation_open {
        return Err(invalid_transition(format!(
            "{} can only be investigated while shipped and not already investigated",
            row.number
        )));
    }
    let investigation_id = InvestigationId::new(derive_id(
        "inq",
        &[
            caller.org_id().as_str(),
            caller.user_id.as_str(),
            &req.idempotency_key,
        ],
    ));
    let key = idempotency_key(
        caller.org_id(),
        &caller.user_id,
        OpenInvestigation::ROUTE.name,
        &req.idempotency_key,
    );
    let start = StartInvestigationInput {
        org_id: caller.org_id().clone(),
        order_id: row.id,
        ticket_id: None,
        opened_by: Actor::Human,
        opened_by_user: Some(caller.user_id.clone()),
        note: req.note,
    };
    app.ingress
        .send::<InvestigationRun>(
            Some(&keys::investigation(&investigation_id)),
            &start,
            Some(&key),
        )
        .await?;
    Ok(InvestigationStarted { investigation_id })
}
