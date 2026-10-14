//! Inventory, customers, the email log, settings and the demo reset.

use std::collections::HashMap;

use techdepo_api_types::admin::{
    CustomerDetail, CustomerSummary, DemoResetResponse, EmailDetail, EmailSummary, InventoryItem,
    InventoryList, SettingsResponse,
};
use techdepo_api_types::catalog::{LOW_STOCK_THRESHOLD, Stock};
use techdepo_api_types::endpoints::{
    GetCustomer, GetEmail, GetInventory, GetSettings, ListCustomers, ListEmails, ResetDemo,
    SetStock, UpdateSettings,
};
use techdepo_api_types::ingress::{
    OrgResetDemo, OrgSetStock, OrgUpdateSettings, ResetDemoInput, SetStockInput,
    UpdateSettingsInput, keys,
};
use techdepo_api_types::{
    CustomerRef, EmailId, OrgSettings, Page, Product, ProductId, UserId, Validate,
};
use techdepo_db::email_log::EmailRow;
use techdepo_db::inventory::StockRow;
use techdepo_db::orders::OrderFilter;
use techdepo_db::orgs::MemberRow;
use techdepo_db::tickets::TicketFilter;
use techdepo_db::{OrgScope, PageRequest, views};

use crate::app::App;
use crate::assemble;
use crate::auth::Caller;
use crate::endpoint::Input;
use crate::error::{ApiResult, DbResultExt, FoundExt};

// -- inventory ----------------------------------------------------------------------------------

fn inventory_item(
    product: &Product,
    row: Option<&StockRow>,
    reserved: u32,
    fallback_updated: time::OffsetDateTime,
) -> InventoryItem {
    InventoryItem {
        product_id: product.id.clone(),
        sku: product.sku.clone(),
        name: product.name.clone(),
        category: product.category,
        tier: product.tier,
        price: product.price,
        stock: Stock::new(row.map_or(0, |r| r.stock)),
        default_stock: product.default_stock,
        reserved,
        updated_at: row.map_or(fallback_updated, |r| r.updated_at),
    }
}

async fn reserved(app: &App, scope: &OrgScope) -> ApiResult<HashMap<ProductId, u32>> {
    Ok(app
        .db
        .orders(scope)
        .reserved_units()
        .await
        .db()?
        .into_iter()
        .collect())
}

pub async fn inventory(
    app: App,
    caller: Caller,
    input: Input<GetInventory>,
) -> ApiResult<InventoryList> {
    let q = input.query;
    let scope = caller.scope();
    let rows: HashMap<ProductId, StockRow> = app
        .db
        .inventory(&scope)
        .list()
        .await
        .db()?
        .into_iter()
        .map(|r| (r.product_id.clone(), r))
        .collect();
    let reserved = reserved(&app, &scope).await?;
    let items = app
        .catalog()
        .all()
        .iter()
        .filter(|p| q.category.is_none_or(|c| c == p.category))
        .map(|p| {
            inventory_item(
                p,
                rows.get(&p.id),
                reserved.get(&p.id).copied().unwrap_or(0),
                caller.org.created_at,
            )
        })
        .filter(|i| !q.low_only.unwrap_or(false) || i.stock.stock <= LOW_STOCK_THRESHOLD)
        .collect();
    Ok(InventoryList { items })
}

pub async fn set_stock(
    app: App,
    caller: Caller,
    input: Input<SetStock>,
) -> ApiResult<InventoryItem> {
    input.body.validate()?;
    let product_id = ProductId::new(input.param(0));
    let product = app
        .catalog()
        .get(&product_id)
        .found(format!("Product {product_id}"))?;
    let set = SetStockInput {
        by: caller.user_id.clone(),
        product_id: product_id.clone(),
        stock: input.body.stock,
    };
    app.ingress
        .call::<OrgSetStock>(Some(&keys::org(caller.org_id())), &set, None)
        .await?;
    let scope = caller.scope();
    let row = app.db.inventory(&scope).get(&product_id).await.db()?;
    let reserved = reserved(&app, &scope).await?;
    Ok(inventory_item(
        product,
        row.as_ref(),
        reserved.get(&product_id).copied().unwrap_or(0),
        caller.org.created_at,
    ))
}

// -- customers ----------------------------------------------------------------------------------

async fn customer_summary(
    app: &App,
    scope: &OrgScope,
    m: &MemberRow,
) -> ApiResult<CustomerSummary> {
    let (order_count, total_spent) = app.db.orders(scope).customer_stats(&m.user_id).await.db()?;
    Ok(CustomerSummary {
        customer: CustomerRef {
            user_id: m.user_id.clone(),
            email: m.email.clone(),
            name: m.name.clone(),
        },
        role: m.role,
        order_count,
        open_tickets: app
            .db
            .tickets(scope)
            .open_count(Some(&m.user_id), None)
            .await
            .db()?,
        total_spent,
        last_seen_at: Some(m.last_seen_at),
    })
}

pub async fn customers(
    app: App,
    caller: Caller,
    input: Input<ListCustomers>,
) -> ApiResult<Page<CustomerSummary>> {
    let q = input.query;
    let scope = caller.scope();
    let page = PageRequest::from_query(q.cursor.as_deref(), q.limit);
    let query = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let (rows, next_cursor) = app.db.orgs(&scope).members(query, &page).await.db()?;
    let mut items = Vec::with_capacity(rows.len());
    for m in &rows {
        items.push(customer_summary(&app, &scope, m).await?);
    }
    Ok(Page { items, next_cursor })
}

pub async fn customer(
    app: App,
    caller: Caller,
    input: Input<GetCustomer>,
) -> ApiResult<CustomerDetail> {
    let scope = caller.scope();
    let user = UserId::new(input.param(0));
    let member = app
        .db
        .orgs(&scope)
        .member(&user)
        .await
        .found(format!("Customer {user}"))?;
    let orders = OrderFilter {
        user_id: Some(user.clone()),
        include_cancelled: true,
        ..OrderFilter::default()
    };
    let (order_rows, _) = app
        .db
        .orders(&scope)
        .list(&orders, &PageRequest::first())
        .await
        .db()?;
    let tickets = TicketFilter {
        user_id: Some(user),
        ..TicketFilter::default()
    };
    let (ticket_rows, _) = app
        .db
        .tickets(&scope)
        .list(&tickets, &PageRequest::first())
        .await
        .db()?;
    let mut ticket_summaries = Vec::with_capacity(ticket_rows.len());
    for t in &ticket_rows {
        ticket_summaries.push(assemble::ticket_summary(&app, &scope, t).await?);
    }
    Ok(CustomerDetail {
        customer: customer_summary(&app, &scope, &member).await?,
        orders: assemble::order_summaries(&app, &scope, &order_rows).await?,
        tickets: ticket_summaries,
    })
}

// -- email log ----------------------------------------------------------------------------------

fn email_summary(row: &EmailRow) -> EmailSummary {
    EmailSummary {
        id: row.id.clone(),
        direction: row.direction,
        channel: row.channel,
        status: row.status,
        reject_reason: row.reject_reason,
        from: row.from_address.clone(),
        to: row.to_address.clone(),
        subject: row.subject.clone(),
        template: row.template.clone(),
        ticket_id: row.ticket_id.clone(),
        order_number: row.order_number,
        attempts: row.attempts,
        created_at: row.created_at,
        sent_at: row.sent_at,
    }
}

pub async fn emails(
    app: App,
    caller: Caller,
    input: Input<ListEmails>,
) -> ApiResult<Page<EmailSummary>> {
    let q = input.query;
    let page = PageRequest::from_query(q.cursor.as_deref(), q.limit);
    let (rows, next_cursor) = app
        .db
        .email_log(&caller.scope())
        .list(q.direction, q.status, q.ticket.as_ref(), &page)
        .await
        .db()?;
    Ok(Page {
        items: rows.iter().map(email_summary).collect(),
        next_cursor,
    })
}

pub async fn email(app: App, caller: Caller, input: Input<GetEmail>) -> ApiResult<EmailDetail> {
    let id = EmailId::new(input.param(0));
    let row = app
        .db
        .email_log(&caller.scope())
        .get(&id)
        .await
        .found(format!("Email {id}"))?;
    Ok(EmailDetail {
        email: email_summary(&row),
        message_id: row.message_id,
        in_reply_to: row.in_reply_to,
        references: row.references,
        reply_to: row.reply_to,
        from_name: row.from_name,
        text_body: row.text_body,
        html_body: row.html_body,
        auth_results: row.auth_results,
        raw_size: row.raw_size,
        provider_id: row.provider_id,
        error: row.error,
    })
}

// -- settings and reset -------------------------------------------------------------------------

fn settings_response(app: &App, caller: &Caller, settings: OrgSettings) -> SettingsResponse {
    SettingsResponse {
        org: views::org_info(&caller.org, app.support()),
        settings,
        defaults: OrgSettings::default(),
    }
}

pub async fn settings(
    app: App,
    caller: Caller,
    _: Input<GetSettings>,
) -> ApiResult<SettingsResponse> {
    let settings = caller.org.settings.clone();
    Ok(settings_response(&app, &caller, settings))
}

pub async fn update_settings(
    app: App,
    caller: Caller,
    input: Input<UpdateSettings>,
) -> ApiResult<SettingsResponse> {
    let req = input.body;
    req.validate()?;
    let update = UpdateSettingsInput {
        by: caller.user_id.clone(),
        settings: req.settings,
    };
    let settings = app
        .ingress
        .call::<OrgUpdateSettings>(Some(&keys::org(caller.org_id())), &update, None)
        .await?;
    Ok(settings_response(&app, &caller, settings))
}

pub async fn reset_demo(
    app: App,
    caller: Caller,
    input: Input<ResetDemo>,
) -> ApiResult<DemoResetResponse> {
    input.body.validate()?;
    let reset = ResetDemoInput {
        by: caller.user_id.clone(),
    };
    app.ingress
        .call::<OrgResetDemo>(Some(&keys::org(caller.org_id())), &reset, None)
        .await
}
