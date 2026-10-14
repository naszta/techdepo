//! Invoices, void + reissue, and the payments ledger with the duplicate-charge detector.

use std::collections::{HashMap, HashSet};

use techdepo_api_types::admin::{
    InvoiceReissued, LedgerEntry, PaymentsLedger, find_duplicate_charges,
};
use techdepo_api_types::endpoints::{GetInvoice, GetPayments, ListInvoices, VoidReissueInvoice};
use techdepo_api_types::ingress::{InvoicingVoidAndReissue, VoidReissueInput};
use techdepo_api_types::orders::{InvoiceDocument, InvoiceSummary, PaymentEntry};
use techdepo_api_types::vocab::{PaymentKind, PaymentStatus};
use techdepo_api_types::{Actor, CustomerRef, InvoiceId, Page, PaymentId, Validate};
use techdepo_db::invoices::{InvoiceFilter, InvoiceRow};
use techdepo_db::payments::{PaymentFilter, PaymentRow};
use techdepo_db::{PageRequest, views};

use crate::app::App;
use crate::auth::Caller;
use crate::endpoint::Input;
use crate::error::{ApiResult, DbResultExt, FoundExt, invalid_transition};

async fn invoice_row(app: &App, caller: &Caller, id: &str) -> ApiResult<InvoiceRow> {
    app.db
        .invoices(&caller.scope())
        .get(&InvoiceId::new(id))
        .await
        .found(format!("Invoice {id}"))
}

pub async fn list_invoices(
    app: App,
    caller: Caller,
    input: Input<ListInvoices>,
) -> ApiResult<Page<InvoiceSummary>> {
    let q = input.query;
    let filter = InvoiceFilter {
        status: q.status,
        kind: q.kind,
        order_number: q.order,
    };
    let page = PageRequest::from_query(q.cursor.as_deref(), q.limit);
    let (rows, next_cursor) = app
        .db
        .invoices(&caller.scope())
        .list(&filter, &page)
        .await
        .db()?;
    Ok(Page {
        items: rows.iter().map(views::invoice_summary).collect(),
        next_cursor,
    })
}

pub async fn invoice(
    app: App,
    caller: Caller,
    input: Input<GetInvoice>,
) -> ApiResult<InvoiceDocument> {
    let row = invoice_row(&app, &caller, input.param(0)).await?;
    let scope = caller.scope();
    let invoices = app.db.invoices(&scope);
    let lines = invoices.lines(&row.id).await.db()?;
    let html = invoices.html(&row.id).await.db()?.unwrap_or_default();
    Ok(views::invoice_document(&row, &lines, html))
}

/// `Invoicing/void_and_reissue`: a credit note plus a corrected invoice. No money moves, so no
/// approval.
pub async fn void_reissue(
    app: App,
    caller: Caller,
    input: Input<VoidReissueInvoice>,
) -> ApiResult<InvoiceReissued> {
    input.body.validate()?;
    let row = invoice_row(&app, &caller, input.param(0)).await?;
    let req = input.body;
    if !views::can_void(&row) {
        return Err(invalid_transition(format!(
            "{} cannot be voided: only issued invoices can",
            row.number
        )));
    }
    let void = VoidReissueInput {
        org_id: caller.org_id().clone(),
        invoice_id: row.id,
        reason: req.reason,
        billing_address: req.billing_address,
        actor: Actor::Human,
        by: Some(caller.user_id.clone()),
    };
    app.ingress
        .call::<InvoicingVoidAndReissue>(None, &void, None)
        .await
}

fn customer(row: &PaymentRow) -> CustomerRef {
    CustomerRef {
        user_id: row.user_id.clone(),
        email: row.customer_email.clone(),
        name: row.customer_name.clone(),
    }
}

/// The ledger page plus every suspected duplicate of the org (deterministic detector).
pub async fn payments(
    app: App,
    caller: Caller,
    input: Input<GetPayments>,
) -> ApiResult<PaymentsLedger> {
    let q = input.query;
    let scope = caller.scope();
    let payments = app.db.payments(&scope);
    let all: Vec<PaymentEntry> = payments
        .all()
        .await
        .db()?
        .iter()
        .map(views::payment_entry)
        .collect();
    let mut duplicates = find_duplicate_charges(&all);
    duplicates.reverse();
    let duplicate_of: HashMap<&PaymentId, &PaymentId> = duplicates
        .iter()
        .map(|d| (&d.duplicate, &d.first))
        .collect();
    let suspicious: HashSet<&PaymentId> = duplicates
        .iter()
        .flat_map(|d| [&d.first, &d.duplicate])
        .collect();
    let refunded: HashSet<&PaymentId> = all
        .iter()
        .filter(|p| p.kind == PaymentKind::Refund && p.status == PaymentStatus::Succeeded)
        .filter_map(|p| p.refund_of.as_ref())
        .collect();

    let filter = PaymentFilter {
        kind: q.kind,
        status: q.status,
        order_number: q.order,
    };
    let page = PageRequest::from_query(q.cursor.as_deref(), q.limit);
    let (rows, next_cursor) = payments.list(&filter, &page).await.db()?;
    let items = rows
        .iter()
        .filter(|r| !q.suspicious.unwrap_or(false) || suspicious.contains(&r.id))
        .map(|r| LedgerEntry {
            payment: views::payment_entry(r),
            customer: customer(r),
            duplicate_of: duplicate_of.get(&r.id).map(|p| (*p).clone()),
            refunded: refunded.contains(&r.id),
        })
        .collect();
    Ok(PaymentsLedger {
        entries: Page { items, next_cursor },
        duplicates,
    })
}
