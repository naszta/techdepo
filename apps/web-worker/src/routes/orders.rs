//! The caller's orders, invoices and the demo controls (`Order` object, keyed by order ID; the
//! worker resolves `{number}` to the ID in D1 first).

use techdepo_api_types::demo::{CreateScenarioResponse, DemoPanel};
use techdepo_api_types::endpoints::{
    AdvanceDemoClock, CreateScenario, GetDemoPanel, GetOrder, GetOrderInvoice, ListOrders,
    RunDemoAction, SetDemoFaults,
};
use techdepo_api_types::ingress::{
    AdvanceClockInput, CreateScenarioInput, DemoActionInput, DemoCreateScenario, OrderAdvanceClock,
    OrderDemoAction, OrderSetFaults, SetFaultsInput, idempotency_key, keys,
};
use techdepo_api_types::orders::{InvoiceDocument, OrderDetail, OrderSummary};
use techdepo_api_types::{Endpoint, InvoiceNumber, OrderNumber, OrderRef, Page, Validate};
use techdepo_db::orders::{OrderFilter, OrderRow};
use techdepo_db::{PageRequest, views};

use crate::app::{App, now};
use crate::assemble;
use crate::auth::Caller;
use crate::endpoint::Input;
use crate::error::{ApiResult, DbResultExt, FoundExt, bad_request};
use crate::throttle;

/// Parses an order number path parameter (`TD-1042`).
pub fn order_number(param: &str) -> ApiResult<OrderNumber> {
    OrderNumber::parse(param).map_err(|e| bad_request(e.to_string()))
}

/// One of the caller's own orders (`404` for someone else's).
pub async fn own_order(app: &App, caller: &Caller, param: &str) -> ApiResult<OrderRow> {
    let number = order_number(param)?;
    app.db
        .orders(&caller.scope())
        .get_for_user(&caller.user_id, number)
        .await
        .found(format!("Order {number}"))
}

/// An order the caller may drive with the demo controls: their own, or any order of the org
/// for an `org:admin`.
async fn demo_order(app: &App, caller: &Caller, param: &str) -> ApiResult<OrderRow> {
    if caller.is_admin() {
        let number = order_number(param)?;
        app.db
            .orders(&caller.scope())
            .get_by_number(number)
            .await
            .found(format!("Order {number}"))
    } else {
        own_order(app, caller, param).await
    }
}

pub async fn list(
    app: App,
    caller: Caller,
    input: Input<ListOrders>,
) -> ApiResult<Page<OrderSummary>> {
    let q = input.query;
    let scope = caller.scope();
    let filter = OrderFilter {
        user_id: Some(caller.user_id.clone()),
        status: q.status,
        include_cancelled: q.include_cancelled.unwrap_or(false),
        ..OrderFilter::default()
    };
    let page = PageRequest::from_query(q.cursor.as_deref(), q.limit);
    let (rows, next_cursor) = app.db.orders(&scope).list(&filter, &page).await.db()?;
    Ok(Page {
        items: assemble::order_summaries(&app, &scope, &rows).await?,
        next_cursor,
    })
}

pub async fn detail(app: App, caller: Caller, input: Input<GetOrder>) -> ApiResult<OrderDetail> {
    let row = own_order(&app, &caller, input.param(0)).await?;
    assemble::order_detail(&app, &caller, &row, false, now()).await
}

pub async fn invoice(
    app: App,
    caller: Caller,
    input: Input<GetOrderInvoice>,
) -> ApiResult<InvoiceDocument> {
    let order = own_order(&app, &caller, input.param(0)).await?;
    let number = InvoiceNumber::parse(input.param(1)).map_err(|e| bad_request(e.to_string()))?;
    let scope = caller.scope();
    let invoices = app.db.invoices(&scope);
    let row = invoices
        .get_by_number(number)
        .await
        .db()?
        .filter(|i| i.order_id == order.id)
        .found(format!("Invoice {number}"))?;
    let lines = invoices.lines(&row.id).await.db()?;
    let html = invoices.html(&row.id).await.db()?.unwrap_or_default();
    Ok(views::invoice_document(&row, &lines, html))
}

// -- demo controls ------------------------------------------------------------------------------

async fn fresh_panel(app: &App, caller: &Caller, row: &OrderRow) -> ApiResult<DemoPanel> {
    let scope = caller.scope();
    let row = app
        .db
        .orders(&scope)
        .get(&row.id)
        .await
        .found(format!("Order {}", row.number))?;
    assemble::demo_panel(app, &scope, &row, now()).await
}

pub async fn demo_panel(
    app: App,
    caller: Caller,
    input: Input<GetDemoPanel>,
) -> ApiResult<DemoPanel> {
    let row = demo_order(&app, &caller, input.param(0)).await?;
    assemble::demo_panel(&app, &caller.scope(), &row, now()).await
}

pub async fn run_demo_action(
    app: App,
    caller: Caller,
    input: Input<RunDemoAction>,
) -> ApiResult<DemoPanel> {
    input.body.validate()?;
    let row = demo_order(&app, &caller, input.param(0)).await?;
    let action = DemoActionInput {
        org_id: caller.org_id().clone(),
        by: caller.user_id.clone(),
        action: input.body.action,
    };
    app.ingress
        .call::<OrderDemoAction>(Some(&keys::order(&row.id)), &action, None)
        .await?;
    fresh_panel(&app, &caller, &row).await
}

pub async fn set_demo_faults(
    app: App,
    caller: Caller,
    input: Input<SetDemoFaults>,
) -> ApiResult<DemoPanel> {
    input.body.validate()?;
    let row = demo_order(&app, &caller, input.param(0)).await?;
    let faults = SetFaultsInput {
        org_id: caller.org_id().clone(),
        by: caller.user_id.clone(),
        faults: input.body.faults,
    };
    app.ingress
        .call::<OrderSetFaults>(Some(&keys::order(&row.id)), &faults, None)
        .await?;
    fresh_panel(&app, &caller, &row).await
}

pub async fn advance_clock(
    app: App,
    caller: Caller,
    input: Input<AdvanceDemoClock>,
) -> ApiResult<DemoPanel> {
    let row = demo_order(&app, &caller, input.param(0)).await?;
    let step = AdvanceClockInput {
        org_id: caller.org_id().clone(),
        by: caller.user_id.clone(),
        step: input.body.step,
    };
    app.ingress
        .call::<OrderAdvanceClock>(Some(&keys::order(&row.id)), &step, None)
        .await?;
    fresh_panel(&app, &caller, &row).await
}

pub async fn create_scenario(
    app: App,
    caller: Caller,
    input: Input<CreateScenario>,
) -> ApiResult<CreateScenarioResponse> {
    let req = input.body;
    req.validate()?;
    throttle::order_quota(&app, &caller).await?;
    let key = idempotency_key(
        caller.org_id(),
        &caller.user_id,
        CreateScenario::ROUTE.name,
        &req.idempotency_key,
    );
    let scenario = CreateScenarioInput {
        customer: caller.customer(),
        customer_email: caller.email.clone(),
        customer_name: caller.name.clone(),
        scenario: req.scenario,
    };
    let placed = app
        .ingress
        .call::<DemoCreateScenario>(None, &scenario, Some(&key))
        .await?;
    Ok(CreateScenarioResponse {
        order: OrderRef {
            id: placed.order_id,
            number: placed.number,
            status: placed.status,
        },
    })
}
