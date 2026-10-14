//! Invoices and credit notes (`invoices`, `invoice_lines`, `invoice_sequences`).
//!
//! Documents are immutable. Numbers are allocated per org and year in the same batch as the
//! insert (`UPDATE … RETURNING` on `invoice_sequences`), and the rendered HTML carries the
//! number through [`NUMBER_PLACEHOLDER`], which the insert replaces in SQL, so the document as
//! issued shows the number it got.

use serde::{Deserialize, Serialize};
use techdepo_api_types::Address;
use techdepo_domain::{
    InvoiceId, InvoiceKind, InvoiceNumber, InvoiceStatus, Money, OrderId, OrderNumber, ProductId,
    Sku, VatBreakdown, VatRate,
};
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutorExt, params};
use time::{Date, OffsetDateTime};

use crate::orders::number_int;
use crate::{Cursor, PageRequest, Param, SqlError, Statement, json_param, repository, stmt};

/// Put this in the HTML where the invoice number goes; the insert replaces it.
pub const NUMBER_PLACEHOLDER: &str = "{{INVOICE_NUMBER}}";

/// An `invoices` row with its links resolved.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InvoiceRow {
    /// Invoice ID.
    pub id: InvoiceId,
    /// Order.
    pub order_id: OrderId,
    /// Order number.
    #[serde(with = "number_int")]
    pub order_number: OrderNumber,
    /// Number.
    pub number: InvoiceNumber,
    /// Invoice or credit note.
    pub kind: InvoiceKind,
    /// Issued or voided.
    pub status: InvoiceStatus,
    /// Reissued invoice: the voided one it replaces.
    pub supersedes_id: Option<InvoiceId>,
    /// Its number.
    pub supersedes_number: Option<InvoiceNumber>,
    /// Voided invoice: the one that replaced it.
    pub superseded_by_id: Option<InvoiceId>,
    /// Its number.
    pub superseded_by_number: Option<InvoiceNumber>,
    /// Credit note: the invoice it cancels.
    pub credit_for_id: Option<InvoiceId>,
    /// Its number.
    pub credit_for_number: Option<InvoiceNumber>,
    /// Billing name.
    pub billing_name: String,
    /// Billing address (`None`: the "missing address" fault).
    #[serde(
        rename = "billing_address_json",
        default,
        with = "column::json_text::option"
    )]
    pub billing_address: Option<Address>,
    /// Gross (negative on credit notes).
    #[serde(rename = "gross_cents")]
    pub gross: Money,
    /// Net.
    #[serde(rename = "net_cents")]
    pub net: Money,
    /// VAT.
    #[serde(rename = "vat_cents")]
    pub vat: Money,
    /// VAT rate.
    #[serde(rename = "vat_rate_bp")]
    pub vat_rate: VatRate,
    /// Issue date (simulated).
    #[serde(with = "column::date_text")]
    pub issue_date: Date,
    /// Real issue time.
    #[serde(with = "column::millis")]
    pub issued_at: OffsetDateTime,
    /// Voided.
    #[serde(default, with = "column::millis::option")]
    pub voided_at: Option<OffsetDateTime>,
    /// Why.
    pub void_reason: Option<String>,
}

/// An `invoice_lines` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceLineRow {
    /// Line number.
    pub line_no: u32,
    /// Product.
    pub product_id: Option<ProductId>,
    /// SKU.
    pub sku: Option<Sku>,
    /// Description.
    pub description: String,
    /// Quantity.
    pub quantity: u32,
    /// Gross unit price.
    #[serde(rename = "unit_price_cents")]
    pub unit_price: Money,
    /// Gross.
    #[serde(rename = "gross_cents")]
    pub gross: Money,
    /// Net.
    #[serde(rename = "net_cents")]
    pub net: Money,
    /// VAT.
    #[serde(rename = "vat_cents")]
    pub vat: Money,
}

/// A line of a new document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewInvoiceLine {
    /// Product.
    pub product_id: Option<ProductId>,
    /// SKU.
    pub sku: Option<Sku>,
    /// Description.
    pub description: String,
    /// Quantity.
    pub quantity: u32,
    /// Gross unit price (negative on credit notes).
    pub unit_price: Money,
}

impl NewInvoiceLine {
    /// `unit_price × quantity`.
    pub fn gross(&self) -> Money {
        self.unit_price * self.quantity
    }

    /// The line's gross split into net and VAT at `rate`.
    pub fn breakdown(&self, rate: VatRate) -> VatBreakdown {
        self.gross().vat_breakdown(rate)
    }
}

/// A document's totals: the sums of its lines' breakdowns, so the printed lines always add up
/// to the printed totals (splitting the total once would round differently, a cent off).
pub fn totals(lines: &[NewInvoiceLine], rate: VatRate) -> VatBreakdown {
    lines.iter().map(|l| l.breakdown(rate)).fold(
        VatBreakdown {
            gross: Money::ZERO,
            net: Money::ZERO,
            vat: Money::ZERO,
        },
        |sum, b| VatBreakdown {
            gross: sum.gross + b.gross,
            net: sum.net + b.net,
            vat: sum.vat + b.vat,
        },
    )
}

/// A document to issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewInvoice {
    /// Invoice ID.
    pub id: InvoiceId,
    /// Order.
    pub order_id: OrderId,
    /// Invoice or credit note.
    pub kind: InvoiceKind,
    /// Reissue: the invoice it replaces.
    pub supersedes_id: Option<InvoiceId>,
    /// Credit note: the invoice it cancels.
    pub credit_for_id: Option<InvoiceId>,
    /// Billing name.
    pub billing_name: String,
    /// Billing address (`None` for the fault).
    pub billing_address: Option<Address>,
    /// VAT rate.
    pub vat_rate: VatRate,
    /// Issue date (simulated): its year picks the sequence.
    pub issue_date: Date,
    /// Real issue time.
    #[serde(with = "time::serde::rfc3339")]
    pub issued_at: OffsetDateTime,
    /// Lines.
    pub lines: Vec<NewInvoiceLine>,
    /// Rendered HTML with [`NUMBER_PLACEHOLDER`] where the number goes.
    pub html: String,
}

impl NewInvoice {
    /// Sum of the lines.
    pub fn gross(&self) -> Money {
        self.lines.iter().map(NewInvoiceLine::gross).sum()
    }
}

/// Filters of the admin invoice list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InvoiceFilter {
    /// Issued or voided.
    pub status: Option<InvoiceStatus>,
    /// Invoice or credit note.
    pub kind: Option<InvoiceKind>,
    /// Order.
    pub order_number: Option<OrderNumber>,
}

const SELECT: &str = "SELECT i.*, o.number AS order_number, s.number AS supersedes_number, \
     b.id AS superseded_by_id, b.number AS superseded_by_number, c.number AS credit_for_number \
     FROM invoices i JOIN orders o ON o.org_id = i.org_id AND o.id = i.order_id \
     LEFT JOIN invoices s ON s.org_id = i.org_id AND s.id = i.supersedes_id \
     LEFT JOIN invoices b ON b.org_id = i.org_id AND b.supersedes_id = i.id \
     LEFT JOIN invoices c ON c.org_id = i.org_id AND c.id = i.credit_for_id";

repository! {
    /// Invoices and credit notes.
    Invoices
}

impl<'a> Invoices<'a> {
    /// One document by ID.
    pub async fn get(&self, id: &InvoiceId) -> Result<Option<InvoiceRow>, SqlError> {
        self.sql
            .query_first_as(
                &format!("{SELECT} WHERE i.org_id = ?1 AND i.id = ?2"),
                &params![self.org(), id],
            )
            .await
    }

    /// One document by number.
    pub async fn get_by_number(
        &self,
        number: InvoiceNumber,
    ) -> Result<Option<InvoiceRow>, SqlError> {
        self.sql
            .query_first_as(
                &format!("{SELECT} WHERE i.org_id = ?1 AND i.number = ?2"),
                &params![self.org(), number],
            )
            .await
    }

    /// The order's documents, oldest first.
    pub async fn for_order(&self, order: &OrderId) -> Result<Vec<InvoiceRow>, SqlError> {
        self.sql
            .query_as(
                &format!(
                    "{SELECT} WHERE i.org_id = ?1 AND i.order_id = ?2 ORDER BY i.issued_at, i.seq"
                ),
                &params![self.org(), order],
            )
            .await
    }

    /// The order's current (issued, not voided) invoice.
    pub async fn current_for_order(&self, order: &OrderId) -> Result<Option<InvoiceRow>, SqlError> {
        self.sql
            .query_first_as(
                &format!(
                    "{SELECT} WHERE i.org_id = ?1 AND i.order_id = ?2 AND i.kind = 'invoice' AND i.status = 'issued' \
                     ORDER BY i.issued_at DESC, i.seq DESC LIMIT 1"
                ),
                &params![self.org(), order],
            )
            .await
    }

    /// A document's lines.
    pub async fn lines(&self, id: &InvoiceId) -> Result<Vec<InvoiceLineRow>, SqlError> {
        self.sql
            .query_as(
                "SELECT line_no, product_id, sku, description, quantity, unit_price_cents, gross_cents, net_cents, vat_cents \
                 FROM invoice_lines WHERE org_id = ?1 AND invoice_id = ?2 ORDER BY line_no",
                &params![self.org(), id],
            )
            .await
    }

    /// The document's HTML as issued.
    pub async fn html(&self, id: &InvoiceId) -> Result<Option<String>, SqlError> {
        self.sql
            .query_scalar(
                "SELECT html FROM invoices WHERE org_id = ?1 AND id = ?2",
                &params![self.org(), id],
            )
            .await
    }

    /// Documents matching `filter`, newest first.
    pub async fn list(
        &self,
        filter: &InvoiceFilter,
        page: &PageRequest,
    ) -> Result<(Vec<InvoiceRow>, Option<String>), SqlError> {
        let (key, id) = page.desc_bounds();
        let rows: Vec<InvoiceRow> = self
            .sql
            .query_as(
                &format!(
                    "{SELECT} WHERE i.org_id = ?1 AND (?2 IS NULL OR i.status = ?2) AND (?3 IS NULL OR i.kind = ?3) \
                     AND (?4 IS NULL OR o.number = ?4) AND (i.issued_at < ?5 OR (i.issued_at = ?5 AND i.id < ?6)) \
                     ORDER BY i.issued_at DESC, i.id DESC LIMIT ?7"
                ),
                &params![
                    self.org(),
                    filter.status,
                    filter.kind,
                    filter.order_number,
                    key,
                    id,
                    page.fetch()
                ],
            )
            .await?;
        Ok(page.finish(rows, |r| {
            Cursor::new(column::millis::to_millis(r.issued_at), r.id.as_str())
        }))
    }

    /// Issues a document: allocates the next number of its year and inserts it with its lines.
    /// Amounts are computed from the lines ([`totals`]). Not idempotent on its own: check [`Invoices::get`]
    /// first, inside the same `ctx.run`.
    pub fn issue_statements(&self, invoice: &NewInvoice) -> Result<Vec<Statement>, SqlError> {
        let org = self.org();
        let year = i64::from(invoice.issue_date.year());
        let breakdown = totals(&invoice.lines, invoice.vat_rate);
        let mut out = vec![
            stmt(
                "INSERT INTO invoice_sequences (org_id, year, last_seq) VALUES (?1, ?2, 1) \
                 ON CONFLICT (org_id, year) DO UPDATE SET last_seq = last_seq + 1 RETURNING last_seq",
                params![org, year],
            ),
            stmt(
                INSERT,
                vec![
                    org.into(),
                    (&invoice.id).into(),
                    (&invoice.order_id).into(),
                    year.into(),
                    invoice.kind.into(),
                    invoice.supersedes_id.as_ref().into(),
                    invoice.credit_for_id.as_ref().into(),
                    invoice.billing_name.as_str().into(),
                    invoice
                        .billing_address
                        .as_ref()
                        .map(json_param)
                        .transpose()?
                        .unwrap_or(Param::Null),
                    breakdown.gross.into(),
                    breakdown.net.into(),
                    breakdown.vat.into(),
                    invoice.vat_rate.into(),
                    invoice.issue_date.into(),
                    invoice.issued_at.into(),
                    invoice.html.as_str().into(),
                    NUMBER_PLACEHOLDER.into(),
                ],
            ),
        ];
        for (i, line) in invoice.lines.iter().enumerate() {
            let b = line.breakdown(invoice.vat_rate);
            out.push(stmt(
                "INSERT INTO invoice_lines (org_id, invoice_id, line_no, product_id, sku, description, quantity, \
                 unit_price_cents, gross_cents, net_cents, vat_cents) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    org,
                    &invoice.id,
                    u32::try_from(i + 1).unwrap_or(u32::MAX),
                    line.product_id.as_ref(),
                    line.sku.as_ref(),
                    line.description.as_str(),
                    line.quantity,
                    line.unit_price,
                    b.gross,
                    b.net,
                    b.vat
                ],
            ));
        }
        Ok(out)
    }

    /// Voids an issued invoice (no-op for anything else). Batch it with the credit note and
    /// the replacement (`supersedes_id`): a voided invoice has at most one replacement (unique
    /// index `invoices_one_replacement`), so a concurrent second void-and-reissue of the same
    /// invoice fails the whole batch with a unique-constraint error instead of issuing twice.
    pub fn void_statement(&self, id: &InvoiceId, reason: &str, at: OffsetDateTime) -> Statement {
        stmt(
            "UPDATE invoices SET status = 'voided', voided_at = ?3, void_reason = ?4 \
             WHERE org_id = ?1 AND id = ?2 AND kind = 'invoice' AND status = 'issued'",
            params![self.org(), id, at, reason],
        )
    }
}

/// The insert: the number is `printf('TD-%04d-%06d', year, seq)` of the just-allocated sequence
/// value (the table's CHECK), and the HTML gets it in place of the placeholder (`?17`).
const INSERT: &str = "INSERT INTO invoices (org_id, id, order_id, number, year, seq, kind, status, supersedes_id, credit_for_id, \
     billing_name, billing_address_json, gross_cents, net_cents, vat_cents, vat_rate_bp, issue_date, issued_at, html) \
     VALUES (?1, ?2, ?3, \
     printf('TD-%04d-%06d', ?4, (SELECT last_seq FROM invoice_sequences WHERE org_id = ?1 AND year = ?4)), ?4, \
     (SELECT last_seq FROM invoice_sequences WHERE org_id = ?1 AND year = ?4), ?5, 'issued', ?6, ?7, ?8, ?9, \
     ?10, ?11, ?12, ?13, ?14, ?15, \
     replace(?16, ?17, printf('TD-%04d-%06d', ?4, (SELECT last_seq FROM invoice_sequences WHERE org_id = ?1 AND year = ?4))))";

/// The number an [`Invoices::issue_statements`] batch allocated, given the batch's results and
/// the offset of the issue statements within it.
pub fn issued_number(
    results: &[crate::BatchResult],
    offset: usize,
    year: i32,
) -> Option<InvoiceNumber> {
    let seq = results
        .get(offset)?
        .rows
        .first()?
        .get("last_seq")?
        .as_i64()?;
    Some(InvoiceNumber::new(
        u16::try_from(year).ok()?,
        u32::try_from(seq).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_format_matches_the_check_constraint() {
        // The CHECK is `number = printf('TD-%04d-%06d', year, seq)`; the domain type must agree.
        assert_eq!(InvoiceNumber::new(2026, 42).to_string(), "TD-2026-000042");
        assert_eq!(
            InvoiceNumber::new(2026, 1_234_567).to_string(),
            "TD-2026-1234567"
        );
        assert!(INSERT.contains("printf('TD-%04d-%06d'"));
    }

    #[test]
    fn totals_add_up_the_lines() {
        let line = NewInvoiceLine {
            product_id: None,
            sku: None,
            description: "Thing".into(),
            quantity: 1,
            unit_price: Money::from_euros(1),
        };
        // €1.00 at 27% is 0.79 + 0.21; splitting the €2.00 total instead gives 1.57 + 0.43.
        let t = totals(&[line.clone(), line], VatRate::DEFAULT);
        assert_eq!(t.gross, Money::from_euros(2));
        assert_eq!(t.net, Money::from_cents(158));
        assert_eq!(t.vat, Money::from_cents(42));
    }
}
