//! Orders as the customer sees them: list, detail, timeline, payments ("card statement"),
//! invoices and shipment tracking. The admin views in [`crate::admin`] reuse these and add the
//! hidden truth.

use serde::{Deserialize, Serialize};
use techdepo_catalog::Category;
use techdepo_domain::{
    Actor, InvoiceId, InvoiceKind, InvoiceNumber, InvoiceStatus, Money, OrderFlags, OrderId,
    OrderNumber, OrderStatus, ProductId, ShipmentId, Sku, TrackingNumber, UserId, VatBreakdown,
    VatRate,
};
use time::{Date, OffsetDateTime};

use crate::common::{Address, EntityRef, TicketRef};
use crate::ids::PaymentId;
use crate::vocab::{
    CarrierEventKind, OrderEventKind, PaymentKind, PaymentMethod, PaymentStatus, ShipmentStatus,
};

/// `GET /api/orders` query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderListQuery {
    /// Only orders in this status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<OrderStatus>,
    /// Include cancelled orders (hidden by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_cancelled: Option<bool>,
    /// Page cursor from [`crate::Page::next_cursor`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size (default [`crate::DEFAULT_PAGE_SIZE`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// An order in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderSummary {
    /// Order ID.
    pub id: OrderId,
    /// Order number.
    pub number: OrderNumber,
    /// Status.
    pub status: OrderStatus,
    /// Overlay flags (`refund_pending`, `investigation_open`, `replacement_for`).
    pub flags: OrderFlags,
    /// Number of the order this one replaces, for linking (`flags.replacement_for` is the ID).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_for_number: Option<OrderNumber>,
    /// Units ordered.
    pub item_count: u32,
    /// Name of the first line, for compact lists ("Voltaris V5 8600 and 5 more").
    pub headline: String,
    /// Total charged (VAT-inclusive). `0` for replacement orders.
    pub total: Money,
    /// Placed.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// One order line, a snapshot of the product as sold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderItem {
    /// Line number, from 1.
    pub line_no: u32,
    /// Product.
    pub product_id: ProductId,
    /// SKU at the time of sale.
    pub sku: Sku,
    /// Name at the time of sale.
    pub name: String,
    /// Category.
    pub category: Category,
    /// Unit price at the time of sale.
    pub unit_price: Money,
    /// Quantity.
    pub quantity: u32,
    /// `unit_price × quantity`.
    pub line_total: Money,
}

/// Order totals with the VAT breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderTotals {
    /// Sum of the lines.
    pub items_total: Money,
    /// Shipping fee.
    pub shipping: Money,
    /// Gross total with its net and VAT parts.
    pub total: VatBreakdown,
    /// The VAT rate applied (basis points).
    pub vat_rate: VatRate,
}

/// Real milestone instants (simulated ones are on the timeline's `sim_at`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderMilestones {
    /// Paid.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub paid_at: Option<OffsetDateTime>,
    /// Shipped.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub shipped_at: Option<OffsetDateTime>,
    /// Delivered.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub delivered_at: Option<OffsetDateTime>,
    /// Cancelled.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub cancelled_at: Option<OffsetDateTime>,
    /// Refunded in full.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub refunded_at: Option<OffsetDateTime>,
    /// Returned.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub returned_at: Option<OffsetDateTime>,
}

/// One timeline entry (`order_events`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderEvent {
    /// Position in the order's timeline, from 1.
    pub seq: u32,
    /// What happened.
    pub kind: OrderEventKind,
    /// Who did it.
    pub actor: Actor,
    /// The person, when `actor` is `human` or `customer`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_user_id: Option<UserId>,
    /// Status after the event, for state changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_after: Option<OrderStatus>,
    /// One line for the timeline.
    pub summary: String,
    /// Related record (payment, invoice, ticket, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<EntityRef>,
    /// Extra structured details (event-specific).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub data: serde_json::Value,
    /// Restate invocation that wrote it, for deep links from admin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_id: Option<String>,
    /// Real time.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The order's simulated time.
    #[serde(with = "time::serde::rfc3339")]
    pub sim_at: OffsetDateTime,
}

/// A payment ledger entry (a line of the "card statement").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentEntry {
    /// Ledger entry ID.
    pub id: PaymentId,
    /// Order.
    pub order_id: OrderId,
    /// Order number.
    pub order_number: OrderNumber,
    /// Charge or refund.
    pub kind: PaymentKind,
    /// Outcome.
    pub status: PaymentStatus,
    /// Amount, always positive.
    pub amount: Money,
    /// Method.
    pub method: PaymentMethod,
    /// Fake PSP reference (`ch_…`, `re_…`).
    pub provider_ref: String,
    /// What a card statement shows, e.g. `TECHDEPO*TD-1042`.
    pub statement_descriptor: String,
    /// For refunds: the charge reversed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refund_of: Option<PaymentId>,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    /// Real time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// The order's simulated time.
    #[serde(with = "time::serde::rfc3339")]
    pub sim_at: OffsetDateTime,
}

impl PaymentEntry {
    /// Signed effect on the customer's balance: charges negative, refunds positive, failures 0.
    pub fn signed_amount(&self) -> Money {
        match (self.status, self.kind) {
            (PaymentStatus::Failed, _) => Money::ZERO,
            (_, PaymentKind::Charge) => -self.amount,
            (_, PaymentKind::Refund) => self.amount,
        }
    }
}

/// A link to an invoice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceRef {
    /// Invoice ID.
    pub id: InvoiceId,
    /// Invoice number.
    pub number: InvoiceNumber,
}

/// An invoice or credit note in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceSummary {
    /// Invoice ID.
    pub id: InvoiceId,
    /// Number, e.g. `TD-2026-000042`.
    pub number: InvoiceNumber,
    /// Invoice or credit note.
    pub kind: InvoiceKind,
    /// Issued or voided.
    pub status: InvoiceStatus,
    /// Order.
    pub order_id: OrderId,
    /// Order number.
    pub order_number: OrderNumber,
    /// Gross, net and VAT. Negative on credit notes.
    pub amounts: VatBreakdown,
    /// VAT rate.
    pub vat_rate: VatRate,
    /// Billing name.
    pub billing_name: String,
    /// Issued without a billing address (the demo fault).
    pub missing_billing_address: bool,
    /// Issue date on the order's simulated calendar.
    #[serde(with = "crate::serde_fmt::date")]
    pub issue_date: Date,
    /// Real issue time.
    #[serde(with = "time::serde::rfc3339")]
    pub issued_at: OffsetDateTime,
    /// When it was voided.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub voided_at: Option<OffsetDateTime>,
    /// Why it was voided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub void_reason: Option<String>,
    /// Reissued invoice: the voided one it replaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<InvoiceRef>,
    /// Voided invoice: the one that replaced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<InvoiceRef>,
    /// Credit note: the invoice it cancels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_for: Option<InvoiceRef>,
}

/// One invoice line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceLine {
    /// Line number, from 1.
    pub line_no: u32,
    /// Product, for product lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_id: Option<ProductId>,
    /// SKU, for product lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sku: Option<Sku>,
    /// Description.
    pub description: String,
    /// Quantity.
    pub quantity: u32,
    /// Gross unit price (negative on credit notes).
    pub unit_price: Money,
    /// Gross, net and VAT of the line.
    pub amounts: VatBreakdown,
}

/// `GET /api/orders/{number}/invoices/{invoice}` and `GET /api/admin/invoices/{id}`: the
/// document as issued.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceDocument {
    /// Header data.
    pub invoice: InvoiceSummary,
    /// Billing address as printed; absent for the "missing address" fault.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing_address: Option<Address>,
    /// Lines.
    pub lines: Vec<InvoiceLine>,
    /// The rendered HTML document (render in a sandboxed iframe).
    pub html: String,
}

/// A carrier scan event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarrierEvent {
    /// Position in the shipment's events, from 1.
    pub seq: u32,
    /// What happened.
    pub kind: CarrierEventKind,
    /// Hidden from the customer (trace events). Customer endpoints never return these.
    pub internal: bool,
    /// Text as the carrier reports it.
    pub description: String,
    /// Depot or city.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Real time.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The order's simulated time.
    #[serde(with = "time::serde::rfc3339")]
    pub sim_at: OffsetDateTime,
}

/// Tracking as the customer sees it: a lost parcel is just "in transit".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipmentView {
    /// Shipment ID.
    pub id: ShipmentId,
    /// Carrier name.
    pub carrier: String,
    /// Tracking number.
    pub tracking_number: TrackingNumber,
    /// Status.
    pub status: ShipmentStatus,
    /// Ship date (simulated calendar).
    #[serde(with = "crate::serde_fmt::date")]
    pub shipped_on: Date,
    /// Carrier's estimated delivery date.
    #[serde(with = "crate::serde_fmt::date")]
    pub eta: Date,
    /// Delivery date.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_fmt::date::option"
    )]
    pub delivered_on: Option<Date>,
    /// Scan events, oldest first (customer-visible ones only, unless admin).
    pub events: Vec<CarrierEvent>,
}

/// `GET /api/orders/{number}`: the customer's order page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderDetail {
    /// Header.
    pub order: OrderSummary,
    /// Lines.
    pub items: Vec<OrderItem>,
    /// Totals.
    pub totals: OrderTotals,
    /// Shipping address.
    pub shipping_address: Address,
    /// Billing address; absent means "same as shipping".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing_address: Option<Address>,
    /// Payment method.
    pub payment_method: PaymentMethod,
    /// Placed despite compatibility errors.
    pub compat_override: bool,
    /// Real milestone instants.
    pub milestones: OrderMilestones,
    /// Timeline, oldest first (customer-visible events only, unless admin).
    pub timeline: Vec<OrderEvent>,
    /// Card statement: charges and refunds, oldest first.
    pub payments: Vec<PaymentEntry>,
    /// Invoices and credit notes, oldest first.
    pub invoices: Vec<InvoiceSummary>,
    /// Tracking, once shipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shipment: Option<ShipmentView>,
    /// Linked support tickets.
    pub tickets: Vec<TicketRef>,
    /// The order's simulated now.
    #[serde(with = "time::serde::rfc3339")]
    pub simulated_now: OffsetDateTime,
    /// `mailto:` link to the org's support address, subject pre-filled with the order number.
    pub support_mailto: String,
}

/// `mailto:support+<org>@<domain>?subject=Order%20TD-1042`.
pub fn support_mailto(support_address: &str, number: OrderNumber) -> String {
    format!("mailto:{support_address}?subject=Order%20{number}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailto_and_signed_amounts() {
        assert_eq!(
            support_mailto("support+k3x9q2ab@techdepo.dev", OrderNumber::new(1042)),
            "mailto:support+k3x9q2ab@techdepo.dev?subject=Order%20TD-1042"
        );
    }

    #[test]
    fn list_query_round_trips_as_json() {
        let q = OrderListQuery {
            status: Some(OrderStatus::Shipped),
            ..Default::default()
        };
        let json = serde_json::to_value(&q).unwrap();
        assert_eq!(json, serde_json::json!({ "status": "shipped" }));
    }
}
