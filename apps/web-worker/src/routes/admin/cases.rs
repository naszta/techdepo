//! Investigations and refunds (records mirrored from their Restate workflows).

use techdepo_api_types::cases::{
    InvestigationDetail, InvestigationSummary, RefundDetail, RefundSummary,
};
use techdepo_api_types::endpoints::{GetInvestigation, GetRefund, ListInvestigations, ListRefunds};
use techdepo_api_types::orders::InvoiceRef;
use techdepo_api_types::{InvestigationId, Page, RefundId};
use techdepo_db::{PageRequest, views};

use crate::app::{App, now};
use crate::assemble;
use crate::auth::Caller;
use crate::endpoint::Input;
use crate::error::{ApiResult, DbResultExt, FoundExt};

pub async fn list_investigations(
    app: App,
    caller: Caller,
    input: Input<ListInvestigations>,
) -> ApiResult<Page<InvestigationSummary>> {
    let q = input.query;
    let scope = caller.scope();
    let page = PageRequest::from_query(q.cursor.as_deref(), q.limit);
    let (rows, next_cursor) = app
        .db
        .investigations(&scope)
        .list(q.status, q.open, &page)
        .await
        .db()?;
    Ok(Page {
        items: assemble::investigation_summaries(&app, &scope, &rows).await?,
        next_cursor,
    })
}

pub async fn investigation(
    app: App,
    caller: Caller,
    input: Input<GetInvestigation>,
) -> ApiResult<InvestigationDetail> {
    let scope = caller.scope();
    let db = &app.db;
    let id = InvestigationId::new(input.param(0));
    let row = db
        .investigations(&scope)
        .get(&id)
        .await
        .found(format!("Investigation {id}"))?;
    let shipment = db.shipments(&scope).for_order(&row.order_id).await.db()?;
    let (shipment_view, trace_status, carrier_responses) = match &shipment {
        Some(s) => {
            let events = db.shipments(&scope).events(&s.id, true).await.db()?;
            let internal = events
                .iter()
                .filter(|e| e.internal)
                .map(views::carrier_event)
                .collect();
            (
                Some(views::shipment_view(s, &events)),
                s.trace_status,
                internal,
            )
        }
        None => (None, None, Vec::new()),
    };
    let tasks: Vec<_> = db
        .tasks(&scope)
        .for_order(&row.order_id)
        .await
        .db()?
        .into_iter()
        .filter(|t| t.investigation_id.as_ref() == Some(&row.id))
        .collect();
    let replacement = match &row.replacement_order_id {
        Some(id) => db
            .orders(&scope)
            .get(id)
            .await
            .db()?
            .map(|o| views::order_ref(&o)),
        None => None,
    };
    Ok(InvestigationDetail {
        investigation: assemble::investigation_summary(&app, &scope, &row).await?,
        shipment: shipment_view,
        trace_status,
        carrier_responses,
        tasks: assemble::task_summaries(&app, &scope, &tasks, now()).await?,
        refund_id: row.refund_id.clone(),
        replacement,
        note: row.note.clone(),
    })
}

pub async fn list_refunds(
    app: App,
    caller: Caller,
    input: Input<ListRefunds>,
) -> ApiResult<Page<RefundSummary>> {
    let q = input.query;
    let page = PageRequest::from_query(q.cursor.as_deref(), q.limit);
    let (rows, next_cursor) = app
        .db
        .refunds(&caller.scope())
        .list(q.status, q.order, &page)
        .await
        .db()?;
    Ok(Page {
        items: rows.iter().map(assemble::refund_summary).collect(),
        next_cursor,
    })
}

pub async fn refund(app: App, caller: Caller, input: Input<GetRefund>) -> ApiResult<RefundDetail> {
    let scope = caller.scope();
    let db = &app.db;
    let id = RefundId::new(input.param(0));
    let row = db
        .refunds(&scope)
        .get(&id)
        .await
        .found(format!("Refund {id}"))?;
    let payments = db.payments(&scope);
    let charge = match &row.charge_payment_id {
        Some(p) => payments
            .get(p)
            .await
            .db()?
            .map(|p| views::payment_entry(&p)),
        None => None,
    };
    let refund_payment = match &row.refund_payment_id {
        Some(p) => payments
            .get(p)
            .await
            .db()?
            .map(|p| views::payment_entry(&p)),
        None => None,
    };
    let credit_note = match &row.credit_note_id {
        Some(i) => db.invoices(&scope).get(i).await.db()?.map(|i| InvoiceRef {
            id: i.id,
            number: i.number,
        }),
        None => None,
    };
    Ok(RefundDetail {
        refund: assemble::refund_summary(&row),
        evidence: row.evidence.clone(),
        charge,
        refund_payment,
        credit_note,
        ticket_id: row.ticket_id.clone(),
        investigation_id: row.investigation_id.clone(),
        human_task_id: row.human_task_id.clone(),
        decided_by: row.decided_by.clone(),
        decided_at: row.decided_at,
        rejection_reason: row.rejection_reason.clone(),
    })
}
