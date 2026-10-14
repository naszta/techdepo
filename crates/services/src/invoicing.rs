//! `Invoicing` (service): issues invoices after payment, voids and reissues them, and writes
//! credit notes for refunds of invoiced orders.
//!
//! Documents are immutable; a fix is a credit note that cancels the original plus a new
//! invoice that supersedes it. Numbers come from the per-org, per-year sequence, allocated in
//! the same batch as the insert (`techdepo_db::invoices`). The HTML is rendered with askama
//! and stored as issued.
//!
//! `issue` is called by the `Order` VO (which records the timeline event itself). The other
//! handlers are not called by the order, so they record their events with one-way sends to
//! `Order/record_event`; never call them from inside the `Order` VO. `void_and_reissue` also
//! emails the corrected invoice to the customer (`Notifications/order`).

use std::sync::Arc;

use askama::Template;
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::admin::InvoiceReissued;
use techdepo_api_types::ingress::VoidReissueInput;
use techdepo_api_types::orders::InvoiceRef;
use techdepo_api_types::vocab::{OrderEventKind, RefKind};
use techdepo_api_types::{Address, ErrorCode};
use techdepo_db::invoices::{
    InvoiceLineRow, InvoiceRow, NUMBER_PLACEHOLDER, NewInvoice, NewInvoiceLine, issued_number,
    totals,
};
use techdepo_db::orders::OrderItemRow;
use techdepo_db::{OrgScope, SqlError, views};
use techdepo_domain::{
    Actor, InvoiceId, InvoiceKind, InvoiceNumber, Money, OrderId, OrderNumber, OrgId, RefundId,
    UserId, VatRate,
};
use techdepo_ports::ConstraintKind;
use time::{Date, OffsetDateTime};

use crate::Deps;
use crate::errors::{bug, internal, not_found, not_now, reject, sql};
use crate::notifications::{NotificationsClient, OrderNotice, OrderNoticeInput};
use crate::order::{OrderClient, RecordEventInput};
use crate::util::{self, mint};

/// `Invoicing/issue` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueInvoiceInput {
    /// Org.
    pub org_id: OrgId,
    /// Order.
    pub order_id: OrderId,
    /// The "missing billing address" fault.
    pub missing_billing_address: bool,
    /// Issue date (the order's simulated today).
    #[serde(with = "techdepo_api_types::serde_fmt::date")]
    pub issue_date: Date,
}

/// `Invoicing/issue` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuedInvoice {
    /// The invoice.
    pub invoice: InvoiceRef,
    /// Issued without the billing address.
    pub missing_billing_address: bool,
    /// Its gross amount.
    pub gross: Money,
}

/// `Invoicing/credit_note` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditNoteInput {
    /// Org.
    pub org_id: OrgId,
    /// Order.
    pub order_id: OrderId,
    /// Amount refunded (positive).
    pub amount: Money,
    /// Printed on the credit note.
    pub reason: String,
    /// The refund it documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refund_id: Option<RefundId>,
}

// -- documents ----------------------------------------------------------------------------------

/// The invoice lines for an order: its items, plus a shipping line when the order paid for
/// shipping, so the invoice's gross is the order total.
pub fn invoice_lines(items: &[OrderItemRow], shipping: Money) -> Vec<NewInvoiceLine> {
    let mut lines: Vec<NewInvoiceLine> = items
        .iter()
        .map(|i| NewInvoiceLine {
            product_id: Some(i.product_id.clone()),
            sku: Some(i.sku.clone()),
            description: i.name.clone(),
            quantity: i.quantity,
            unit_price: i.unit_price,
        })
        .collect();
    if shipping.is_positive() {
        lines.push(NewInvoiceLine {
            product_id: None,
            sku: None,
            description: "Shipping".to_owned(),
            quantity: 1,
            unit_price: shipping,
        });
    }
    lines
}

/// Credit note lines cancelling `lines` in full.
pub fn reversed_lines(lines: &[InvoiceLineRow]) -> Vec<NewInvoiceLine> {
    lines
        .iter()
        .map(|l| NewInvoiceLine {
            product_id: l.product_id.clone(),
            sku: l.sku.clone(),
            description: l.description.clone(),
            quantity: l.quantity,
            unit_price: -l.unit_price,
        })
        .collect()
}

/// Credit note lines for a refund of `amount` against an invoice of `gross`: the invoice
/// reversed in full, or one line for a partial refund.
pub fn refund_lines(
    lines: &[InvoiceLineRow],
    gross: Money,
    amount: Money,
    reason: &str,
) -> Vec<NewInvoiceLine> {
    if amount >= gross {
        reversed_lines(lines)
    } else {
        vec![NewInvoiceLine {
            product_id: None,
            sku: None,
            description: format!("Partial refund: {reason}"),
            quantity: 1,
            unit_price: -amount,
        }]
    }
}

/// The re-invoiced lines (same as the voided invoice's).
pub fn copied_lines(lines: &[InvoiceLineRow]) -> Vec<NewInvoiceLine> {
    lines
        .iter()
        .map(|l| NewInvoiceLine {
            product_id: l.product_id.clone(),
            sku: l.sku.clone(),
            description: l.description.clone(),
            quantity: l.quantity,
            unit_price: l.unit_price,
        })
        .collect()
}

/// One table row of the HTML.
struct HtmlLine {
    description: String,
    quantity: u32,
    unit: String,
    net: String,
    vat: String,
    gross: String,
}

#[derive(Template)]
#[template(path = "invoice.html")]
struct InvoiceHtml {
    title: &'static str,
    number: &'static str,
    issue_date: String,
    order_number: String,
    billing_name: String,
    billing_lines: Vec<String>,
    missing_address: bool,
    note: Option<String>,
    lines: Vec<HtmlLine>,
    net: String,
    vat: String,
    gross: String,
    vat_rate: String,
}

/// What [`render`] prints besides the lines.
pub struct DocumentHeader<'a> {
    /// Invoice or credit note.
    pub kind: InvoiceKind,
    /// Issue date.
    pub issue_date: Date,
    /// Order number.
    pub order_number: OrderNumber,
    /// Billing name.
    pub billing_name: &'a str,
    /// Billing address (`None`: printed as missing).
    pub billing_address: Option<&'a Address>,
    /// "Cancels invoice …" / "Replaces invoice …".
    pub note: Option<String>,
    /// VAT rate.
    pub vat_rate: VatRate,
}

/// Renders a document. The number is [`NUMBER_PLACEHOLDER`]; the insert fills it in.
pub fn render(
    header: &DocumentHeader<'_>,
    lines: &[NewInvoiceLine],
) -> Result<String, askama::Error> {
    let rate = header.vat_rate;
    let rows = lines
        .iter()
        .map(|l| {
            let b = l.breakdown(rate);
            HtmlLine {
                description: l.description.clone(),
                quantity: l.quantity,
                unit: l.unit_price.to_string(),
                net: b.net.to_string(),
                vat: b.vat.to_string(),
                gross: b.gross.to_string(),
            }
        })
        .collect();
    // The stored totals are the same sums (`techdepo_db::invoices::issue_statements`).
    let b = totals(lines, rate);
    InvoiceHtml {
        title: match header.kind {
            InvoiceKind::Invoice => "Invoice",
            InvoiceKind::CreditNote => "Credit note",
        },
        number: NUMBER_PLACEHOLDER,
        issue_date: techdepo_ports::sql::column::date_text::format(header.issue_date),
        order_number: header.order_number.to_string(),
        billing_name: header.billing_name.to_owned(),
        billing_lines: header
            .billing_address
            .map(|a| a.lines().into_iter().skip(1).collect())
            .unwrap_or_default(),
        missing_address: header.billing_address.is_none(),
        note: header.note.clone(),
        lines: rows,
        net: b.net.to_string(),
        vat: b.vat.to_string(),
        gross: b.gross.to_string(),
        vat_rate: rate.to_string(),
    }
    .render()
}

fn document(
    id: InvoiceId,
    order_id: &OrderId,
    header: &DocumentHeader<'_>,
    lines: Vec<NewInvoiceLine>,
    links: (Option<InvoiceId>, Option<InvoiceId>),
    issued_at: OffsetDateTime,
) -> Result<NewInvoice, HandlerError> {
    let html = bug(render(header, &lines))?;
    Ok(NewInvoice {
        id,
        order_id: order_id.clone(),
        kind: header.kind,
        supersedes_id: links.0,
        credit_for_id: links.1,
        billing_name: header.billing_name.to_owned(),
        billing_address: header.billing_address.cloned(),
        vat_rate: header.vat_rate,
        issue_date: header.issue_date,
        issued_at,
        lines,
        html,
    })
}

fn invoice_ref(row: &InvoiceRow) -> InvoiceRef {
    views::invoice_ref(row)
}

// -- the service --------------------------------------------------------------------------------

/// The `Invoicing` service.
pub struct Invoicing {
    deps: Arc<Deps>,
}

impl Invoicing {
    /// The service over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }

    /// Puts an invoice event on the order's timeline (one-way: the order may be busy).
    fn record(ctx: &Context<'_>, order_id: &OrderId, mut event: RecordEventInput) {
        event.invocation_id = Some(ctx.invocation_id().to_owned());
        let _ = ctx
            .object_client::<OrderClient>(order_id.as_str())
            .record_event(Json(event))
            .send();
    }
}

/// A timeline entry about an invoice.
fn invoice_event(
    org_id: &OrgId,
    kind: OrderEventKind,
    summary: String,
    invoice: &InvoiceRef,
    actor: Actor,
    by: Option<&UserId>,
) -> RecordEventInput {
    RecordEventInput {
        org_id: org_id.clone(),
        kind,
        actor,
        actor_user_id: by.cloned(),
        summary,
        reference: Some((RefKind::Invoice, invoice.id.to_string())),
        data: serde_json::json!({ "number": invoice.number }),
        refund_pending: None,
        investigation_open: None,
        invocation_id: None,
    }
}

#[restate_sdk::service(name = "Invoicing")]
impl Invoicing {
    /// Issues the order's invoice (once: an order with an issued invoice gets it back).
    #[handler(ingress_private = true)]
    async fn issue(
        &self,
        mut ctx: Context<'_>,
        input: Json<IssueInvoiceInput>,
    ) -> Result<Json<IssuedInvoice>, HandlerError> {
        let input = input.into_inner();
        let at = util::now(&ctx).await?;
        let id = InvoiceId::new(mint("inv", ctx.rand_uuid()));
        let db = self.deps.db.clone();
        let scope = OrgScope::verified(input.org_id.clone());
        let issued: Json<IssuedInvoice> = ctx
            .run(|| async move {
                let invoices = db.invoices(&scope);
                if let Some(existing) = invoices
                    .current_for_order(&input.order_id)
                    .await
                    .map_err(sql)?
                {
                    return Ok(Json(IssuedInvoice {
                        invoice: invoice_ref(&existing),
                        missing_billing_address: existing.billing_address.is_none(),
                        gross: existing.gross,
                    }));
                }
                let orders = db.orders(&scope);
                let order = orders
                    .get(&input.order_id)
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| not_found("order"))?;
                let items = orders.items(&order.id).await.map_err(sql)?;
                let billing = order.effective_billing_address().clone();
                let header = DocumentHeader {
                    kind: InvoiceKind::Invoice,
                    issue_date: input.issue_date,
                    order_number: order.number,
                    billing_name: &billing.name,
                    billing_address: (!input.missing_billing_address).then_some(&billing),
                    note: None,
                    vat_rate: order.vat_rate,
                };
                let new = document(
                    id,
                    &order.id,
                    &header,
                    invoice_lines(&items, order.shipping),
                    (None, None),
                    at,
                )?;
                let results = db
                    .batch(&invoices.issue_statements(&new).map_err(sql)?)
                    .await
                    .map_err(sql)?;
                let number = issued_number(&results, 0, new.issue_date.year())
                    .ok_or_else(|| internal("invoice number not returned"))?;
                Ok(Json(IssuedInvoice {
                    invoice: InvoiceRef {
                        id: new.id.clone(),
                        number,
                    },
                    missing_billing_address: input.missing_billing_address,
                    gross: new.gross(),
                }))
            })
            .name("issue-invoice")
            .await?;
        Ok(issued)
    }

    /// Voids an issued invoice with a credit note and issues a corrected one (billing address
    /// from the input, else the order's). No money moves.
    #[handler]
    async fn void_and_reissue(
        &self,
        mut ctx: Context<'_>,
        input: Json<VoidReissueInput>,
    ) -> Result<Json<InvoiceReissued>, HandlerError> {
        let input = input.into_inner();
        let at = util::now(&ctx).await?;
        let credit_id = InvoiceId::new(mint("inv", ctx.rand_uuid()));
        let reissue_id = InvoiceId::new(mint("inv", ctx.rand_uuid()));
        let db = self.deps.db.clone();
        let scope = OrgScope::verified(input.org_id.clone());
        let (invoice_id, reason, address) = (
            input.invoice_id.clone(),
            input.reason.clone(),
            input.billing_address.clone(),
        );
        let done: Json<(OrderId, InvoiceReissued)> = ctx
            .run(|| async move {
                let invoices = db.invoices(&scope);
                let original = invoices
                    .get(&invoice_id)
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| not_found("invoice"))?;
                // A retry after the batch committed finds the documents in place.
                if let (Some(credit), Some(reissued)) = (
                    invoices.get(&credit_id).await.map_err(sql)?,
                    invoices.get(&reissue_id).await.map_err(sql)?,
                ) {
                    return Ok(Json((
                        original.order_id.clone(),
                        InvoiceReissued {
                            voided: invoice_ref(&original),
                            credit_note: invoice_ref(&credit),
                            reissued: invoice_ref(&reissued),
                        },
                    )));
                }
                if !views::can_void(&original) {
                    return Err(not_now(match original.kind {
                        InvoiceKind::CreditNote => "A credit note cannot be voided",
                        InvoiceKind::Invoice => "This invoice is already voided",
                    }));
                }
                let orders = db.orders(&scope);
                let order = orders
                    .get(&original.order_id)
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| not_found("order"))?;
                let lines = invoices.lines(&original.id).await.map_err(sql)?;
                let today = orders.clock(&order.id).await.map_err(sql)?.now(at).date();
                let billing = address.unwrap_or_else(|| order.effective_billing_address().clone());

                let credit_header = DocumentHeader {
                    kind: InvoiceKind::CreditNote,
                    issue_date: today,
                    order_number: order.number,
                    billing_name: &original.billing_name,
                    billing_address: original.billing_address.as_ref(),
                    note: Some(format!(
                        "Cancels invoice {}. Reason: {reason}",
                        original.number
                    )),
                    vat_rate: original.vat_rate,
                };
                let credit = document(
                    credit_id,
                    &order.id,
                    &credit_header,
                    reversed_lines(&lines),
                    (None, Some(original.id.clone())),
                    at,
                )?;
                let reissue_header = DocumentHeader {
                    kind: InvoiceKind::Invoice,
                    issue_date: today,
                    order_number: order.number,
                    billing_name: &billing.name,
                    billing_address: Some(&billing),
                    note: Some(format!("Replaces invoice {}", original.number)),
                    vat_rate: original.vat_rate,
                };
                let reissued = document(
                    reissue_id,
                    &order.id,
                    &reissue_header,
                    copied_lines(&lines),
                    (Some(original.id.clone()), None),
                    at,
                )?;

                let mut statements = vec![invoices.void_statement(&original.id, &reason, at)];
                let credit_at = statements.len();
                statements.extend(invoices.issue_statements(&credit).map_err(sql)?);
                let reissue_at = statements.len();
                statements.extend(invoices.issue_statements(&reissued).map_err(sql)?);
                // A concurrent void-and-reissue of the same invoice committed first: its
                // replacement holds the one slot (`invoices_one_replacement`), and this batch,
                // credit note included, rolled back.
                let results = db.batch(&statements).await.map_err(|e| match e {
                    SqlError::Constraint {
                        kind: ConstraintKind::Unique,
                        ..
                    } => not_now("This invoice is already voided"),
                    other => sql(other),
                })?;
                let number = |offset| {
                    issued_number(&results, offset, today.year())
                        .ok_or_else(|| internal("invoice number not returned"))
                };
                Ok(Json((
                    order.id.clone(),
                    InvoiceReissued {
                        voided: invoice_ref(&original),
                        credit_note: InvoiceRef {
                            id: credit.id.clone(),
                            number: number(credit_at)?,
                        },
                        reissued: InvoiceRef {
                            id: reissued.id.clone(),
                            number: number(reissue_at)?,
                        },
                    },
                )))
            })
            .name("void-and-reissue")
            .await?;
        let (order_id, reissued) = done.into_inner();
        let by = input.by.as_ref();
        let org = &input.org_id;
        for event in [
            invoice_event(
                org,
                OrderEventKind::InvoiceVoided,
                format!(
                    "Invoice {} voided: {}",
                    reissued.voided.number, input.reason
                ),
                &reissued.voided,
                input.actor,
                by,
            ),
            invoice_event(
                org,
                OrderEventKind::CreditNoteIssued,
                format!(
                    "Credit note {} issued for invoice {}",
                    reissued.credit_note.number, reissued.voided.number
                ),
                &reissued.credit_note,
                input.actor,
                by,
            ),
            invoice_event(
                org,
                OrderEventKind::InvoiceIssued,
                format!("Corrected invoice {} issued", reissued.reissued.number),
                &reissued.reissued,
                input.actor,
                by,
            ),
        ] {
            Self::record(&ctx, &order_id, event);
        }
        let _ = ctx
            .service_client::<NotificationsClient>()
            .order(Json(OrderNoticeInput {
                org_id: input.org_id.clone(),
                order_id,
                notice: OrderNotice::InvoiceReissued {
                    documents: reissued.clone(),
                    reason: input.reason.clone(),
                },
            }))
            .send();
        Ok(Json(reissued))
    }

    /// A credit note for a refund of an invoiced order (`None` when the order has no issued
    /// invoice). Not for duplicate charges, which were never invoiced
    /// (`RefundReason::credits_invoice`).
    #[handler(ingress_private = true)]
    async fn credit_note(
        &self,
        mut ctx: Context<'_>,
        input: Json<CreditNoteInput>,
    ) -> Result<Json<Option<InvoiceRef>>, HandlerError> {
        let input = input.into_inner();
        if !input.amount.is_positive() {
            return Err(reject(techdepo_api_types::ApiError::new(
                ErrorCode::Validation,
                "Amount must be positive",
            )));
        }
        let at = util::now(&ctx).await?;
        let id = InvoiceId::new(mint("inv", ctx.rand_uuid()));
        let db = self.deps.db.clone();
        let scope = OrgScope::verified(input.org_id.clone());
        let (order_id, amount, reason) =
            (input.order_id.clone(), input.amount, input.reason.clone());
        let credit: Json<Option<(InvoiceRef, InvoiceNumber)>> = ctx
            .run(|| async move {
                let invoices = db.invoices(&scope);
                if let Some(existing) = invoices.get(&id).await.map_err(sql)? {
                    let credited = existing
                        .credit_for_number
                        .ok_or_else(|| internal("credit note without invoice"))?;
                    return Ok(Json(Some((invoice_ref(&existing), credited))));
                }
                let Some(invoice) = invoices.current_for_order(&order_id).await.map_err(sql)?
                else {
                    return Ok(Json(None));
                };
                let orders = db.orders(&scope);
                let order = orders
                    .get(&order_id)
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| not_found("order"))?;
                let lines = invoices.lines(&invoice.id).await.map_err(sql)?;
                let today = orders.clock(&order.id).await.map_err(sql)?.now(at).date();
                let header = DocumentHeader {
                    kind: InvoiceKind::CreditNote,
                    issue_date: today,
                    order_number: order.number,
                    billing_name: &invoice.billing_name,
                    billing_address: invoice.billing_address.as_ref(),
                    note: Some(format!(
                        "Refund against invoice {}. Reason: {reason}",
                        invoice.number
                    )),
                    vat_rate: invoice.vat_rate,
                };
                let lines = refund_lines(&lines, invoice.gross, amount, &reason);
                let new = document(
                    id,
                    &order.id,
                    &header,
                    lines,
                    (None, Some(invoice.id.clone())),
                    at,
                )?;
                let results = db
                    .batch(&invoices.issue_statements(&new).map_err(sql)?)
                    .await
                    .map_err(sql)?;
                let number = issued_number(&results, 0, today.year())
                    .ok_or_else(|| internal("invoice number not returned"))?;
                Ok(Json(Some((
                    InvoiceRef {
                        id: new.id.clone(),
                        number,
                    },
                    invoice.number,
                ))))
            })
            .name("credit-note")
            .await?;
        let Some((credit, credited)) = credit.into_inner() else {
            return Ok(Json(None));
        };
        let event = invoice_event(
            &input.org_id,
            OrderEventKind::CreditNoteIssued,
            format!(
                "Credit note {} issued for invoice {credited} ({})",
                credit.number, input.amount
            ),
            &credit,
            Actor::System,
            None,
        );
        Self::record(&ctx, &input.order_id, event);
        Ok(Json(Some(credit)))
    }
}

#[cfg(test)]
mod tests {
    use techdepo_api_types::Category;
    use techdepo_domain::{ProductId, Sku};
    use time::macros::date;

    use super::*;

    fn items() -> Vec<OrderItemRow> {
        vec![OrderItemRow {
            line_no: 1,
            product_id: ProductId::new("voltaris-v5-8600"),
            sku: Sku::new("CPU-VOL-8600"),
            name: "Voltaris V5 8600 <fast>".into(),
            category: Category::Cpu,
            unit_price: Money::from_euros(127),
            quantity: 2,
            line_total: Money::from_euros(254),
        }]
    }

    fn header(address: Option<&Address>) -> DocumentHeader<'_> {
        DocumentHeader {
            kind: InvoiceKind::Invoice,
            issue_date: date!(2026 - 10 - 16),
            order_number: OrderNumber::new(1042),
            billing_name: "Ada",
            billing_address: address,
            note: None,
            vat_rate: VatRate::DEFAULT,
        }
    }

    #[test]
    fn renders_with_placeholder_and_escaping() {
        let address = Address {
            name: "Ada".into(),
            line1: "Fő utca 1".into(),
            postal_code: "1011".into(),
            city: "Budapest".into(),
            country: "HU".into(),
            ..Address::default()
        };
        let html = render(
            &header(Some(&address)),
            &invoice_lines(&items(), Money::ZERO),
        )
        .unwrap();
        assert!(html.contains(NUMBER_PLACEHOLDER));
        assert!(html.contains("1011 Budapest"));
        assert!(html.contains("&#60;fast&#62;"), "escaped");
        assert!(html.contains("€200.00"), "net of €254 at 27%");
        assert!(html.contains("€54.00"));
        let missing = render(&header(None), &invoice_lines(&items(), Money::ZERO)).unwrap();
        assert!(missing.contains("Billing address missing"));
    }

    #[test]
    fn shipping_is_invoiced() {
        let lines = invoice_lines(&items(), Money::from_euros(5));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].description, "Shipping");
        let gross: Money = lines.iter().map(NewInvoiceLine::gross).sum();
        assert_eq!(gross, Money::from_euros(259));
        assert_eq!(invoice_lines(&items(), Money::ZERO).len(), 1);
    }

    #[test]
    fn credit_lines() {
        let lines = vec![InvoiceLineRow {
            line_no: 1,
            product_id: None,
            sku: None,
            description: "Thing".into(),
            quantity: 2,
            unit_price: Money::from_euros(127),
            gross: Money::from_euros(254),
            net: Money::from_euros(200),
            vat: Money::from_euros(54),
        }];
        let full = refund_lines(
            &lines,
            Money::from_euros(254),
            Money::from_euros(254),
            "lost",
        );
        assert_eq!(full[0].gross(), Money::from_euros(-254));
        let partial = refund_lines(
            &lines,
            Money::from_euros(254),
            Money::from_euros(10),
            "goodwill",
        );
        assert_eq!(partial.len(), 1);
        assert_eq!(partial[0].gross(), Money::from_euros(-10));
        assert_eq!(copied_lines(&lines)[0].gross(), Money::from_euros(254));
    }
}
