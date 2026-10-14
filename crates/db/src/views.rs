//! Rows → `techdepo_api_types` DTOs. Pure functions, shared by the web worker (responses) and
//! the services (checkout prices the cart with [`cart`], so the customer and the order agree on
//! the total).

use std::collections::HashMap;

use techdepo_api_types::account::{
    BuildDetail, BuildSummary, Cart, CartLine, SHIPPING_FEE, SavedAddress,
};
use techdepo_api_types::catalog::{Stock, StockEntry};
use techdepo_api_types::demo::{ClockStep, ClockView, DemoActionView, DemoPanel, HiddenTruth};
use techdepo_api_types::orders::{
    CarrierEvent, InvoiceDocument, InvoiceLine, InvoiceRef, InvoiceSummary, OrderEvent, OrderItem,
    OrderMilestones, OrderSummary, OrderTotals, PaymentEntry, ShipmentView,
};
use techdepo_api_types::session::{OrgInfo, SupportMailbox};
use techdepo_api_types::support::{TicketMessage, TicketSummary};
use techdepo_api_types::vocab::ShipmentStatus;
use techdepo_api_types::{CustomerRef, EntityRef, OrderRef, TicketRef};
use techdepo_catalog::Catalog;
use techdepo_compat::{Build, Report, Slot, check_build};
use techdepo_domain::{
    CarrierFault, FaultToggles, InvoiceKind, InvoiceStatus, LostOutcome, Money, OrderAction,
    OrderFlags, OrderStatus, PaymentFault, ProductId, ShipmentFault, VirtualClock, WorkCalendar,
    demo_actions,
};
use time::OffsetDateTime;

use crate::addresses::AddressRow;
use crate::builds::BuildRow;
use crate::carts::CartRows;
use crate::invoices::{InvoiceLineRow, InvoiceRow};
use crate::orders::{DemoFaultsRow, OrderEventRow, OrderItemRow, OrderRow};
use crate::orgs::OrgRow;
use crate::payments::PaymentRow;
use crate::shipments::{CarrierEventRow, ShipmentRow};
use crate::tickets::{TicketMessageRow, TicketRow};

// -- org ----------------------------------------------------------------------------------------

/// The org as `/api/me` shows it.
pub fn org_info(org: &OrgRow, support: &SupportMailbox) -> OrgInfo {
    OrgInfo {
        id: org.id.clone(),
        public_id: org.public_id.clone(),
        name: org.name.clone(),
        support_address: support.address(&org.public_id),
        created_at: org.created_at,
        expires_at: org.expires_at,
        features: org.features.clone(),
    }
}

/// A stock entry.
pub fn stock_entry(product_id: ProductId, stock: u32) -> StockEntry {
    StockEntry {
        product_id,
        stock: Stock::new(stock),
    }
}

// -- cart ---------------------------------------------------------------------------------------

/// Prices a cart from the catalog. Lines whose product left the catalog are dropped. The
/// compatibility report covers the build-relevant parts when there are at least two of them
/// and they form one build (a cart with two CPUs is not one build: no report).
pub fn cart(rows: &CartRows, stock: &HashMap<ProductId, u32>, catalog: &Catalog) -> Cart {
    let mut lines = Vec::with_capacity(rows.items.len());
    for item in &rows.items {
        let Some(product) = catalog.get(&item.product_id) else {
            continue;
        };
        let available = stock.get(&item.product_id).copied().unwrap_or(0);
        lines.push(CartLine {
            product_id: product.id.clone(),
            sku: product.sku.clone(),
            name: product.name.clone(),
            category: product.category,
            unit_price: product.price,
            quantity: item.quantity,
            line_total: product.price * item.quantity,
            stock: Stock::new(available),
        });
    }
    let items_total: Money = lines.iter().map(|l| l.line_total).sum();
    let shipping = if lines.is_empty() {
        Money::ZERO
    } else {
        SHIPPING_FEE
    };
    let out_of_stock = lines
        .iter()
        .filter(|l| !l.in_stock())
        .map(|l| l.product_id.clone())
        .collect();
    let report = cart_report(&lines, catalog);
    Cart {
        lines,
        items_total,
        shipping,
        total: items_total + shipping,
        report,
        out_of_stock,
        updated_at: rows.updated_at,
    }
}

/// The compatibility report of a cart's lines (see [`cart`]).
pub fn cart_report(lines: &[CartLine], catalog: &Catalog) -> Option<Report> {
    let mut ids = Vec::new();
    for line in lines {
        let copies = if Slot::is_multi(line.category) {
            line.quantity
        } else {
            1
        };
        for _ in 0..copies {
            ids.push(line.product_id.as_str());
        }
    }
    if ids.len() < 2 {
        return None;
    }
    Build::from_ids(catalog, ids).ok().map(|b| check_build(&b))
}

// -- builds and addresses -----------------------------------------------------------------------

/// A saved build in the list.
pub fn build_summary(row: &BuildRow, catalog: &Catalog) -> BuildSummary {
    let detail = build_detail(row, catalog);
    BuildSummary {
        id: row.id.clone(),
        name: row.name.clone(),
        part_count: u32::try_from(detail.product_ids.len()).unwrap_or(u32::MAX),
        total: detail.total,
        verdict: detail.report.verdict,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// A saved build with its live report. Parts no longer in the catalog are listed in
/// `removed`; parts that do not fit one build (a second CPU) are reported as removed too.
pub fn build_detail(row: &BuildRow, catalog: &Catalog) -> BuildDetail {
    let mut build = Build::new();
    let mut kept = Vec::new();
    let mut removed = Vec::new();
    for id in &row.product_ids {
        match catalog.get(id) {
            Some(p) if Slot::is_multi(p.category) || build.get(p.category.into()).is_none() => {
                build.add(p);
                kept.push(id.clone());
            }
            _ => removed.push(id.clone()),
        }
    }
    BuildDetail {
        id: row.id.clone(),
        name: row.name.clone(),
        product_ids: kept,
        removed,
        report: check_build(&build),
        missing: build.missing_required(),
        total: build.total_price(),
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// An address-book entry.
pub fn saved_address(row: &AddressRow) -> SavedAddress {
    SavedAddress {
        id: row.id.clone(),
        label: row.label.clone(),
        address: row.address.clone(),
        default_shipping: row.is_default_shipping,
        default_billing: row.is_default_billing,
    }
}

// -- orders -------------------------------------------------------------------------------------

/// A link to an order.
pub fn order_ref(row: &OrderRow) -> OrderRef {
    OrderRef {
        id: row.id.clone(),
        number: row.number,
        status: row.status,
    }
}

/// The overlay flags.
pub fn order_flags(row: &OrderRow) -> OrderFlags {
    OrderFlags {
        refund_pending: row.refund_pending,
        investigation_open: row.investigation_open,
        replacement_for: row.replacement_for.clone(),
    }
}

/// The order's customer.
pub fn order_customer(row: &OrderRow) -> CustomerRef {
    CustomerRef {
        user_id: row.user_id.clone(),
        email: row.customer_email.clone(),
        name: row.customer_name.clone(),
    }
}

/// A list row. `headline` and `item_count` come from [`crate::orders::Orders::headlines`];
/// `replacement_for_number` is the number of `row.replacement_for`.
pub fn order_summary(
    row: &OrderRow,
    headline: String,
    item_count: u32,
    replacement_for_number: Option<techdepo_domain::OrderNumber>,
) -> OrderSummary {
    OrderSummary {
        id: row.id.clone(),
        number: row.number,
        status: row.status,
        flags: order_flags(row),
        replacement_for_number,
        item_count,
        headline,
        total: row.total,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// The headline and unit count of an order from its lines.
pub fn headline(items: &[OrderItemRow]) -> (String, u32) {
    let units = items.iter().map(|i| i.quantity).sum();
    let headline = match items {
        [] => String::new(),
        [one] => one.name.clone(),
        [first, rest @ ..] => format!("{} and {} more", first.name, rest.len()),
    };
    (headline, units)
}

/// A line.
pub fn order_item(row: &OrderItemRow) -> OrderItem {
    OrderItem {
        line_no: row.line_no,
        product_id: row.product_id.clone(),
        sku: row.sku.clone(),
        name: row.name.clone(),
        category: row.category,
        unit_price: row.unit_price,
        quantity: row.quantity,
        line_total: row.line_total,
    }
}

/// Totals with the VAT breakdown.
pub fn order_totals(row: &OrderRow) -> OrderTotals {
    OrderTotals {
        items_total: row.items_total,
        shipping: row.shipping,
        total: row.total.vat_breakdown(row.vat_rate),
        vat_rate: row.vat_rate,
    }
}

/// Milestone instants.
pub fn order_milestones(row: &OrderRow) -> OrderMilestones {
    OrderMilestones {
        paid_at: row.paid_at,
        shipped_at: row.shipped_at,
        delivered_at: row.delivered_at,
        cancelled_at: row.cancelled_at,
        refunded_at: row.refunded_at,
        returned_at: row.returned_at,
    }
}

/// A timeline entry. `label` is the display text of the referenced record, when known.
pub fn order_event(row: &OrderEventRow, label: Option<String>) -> OrderEvent {
    let entity = match (row.ref_kind, &row.ref_id) {
        (Some(kind), Some(id)) => Some(EntityRef {
            kind,
            id: id.clone(),
            label: label.unwrap_or_else(|| id.clone()),
        }),
        _ => None,
    };
    OrderEvent {
        seq: row.seq,
        kind: row.kind,
        actor: row.actor,
        actor_user_id: row.actor_user_id.clone(),
        status_after: row.status_after,
        summary: row.summary.clone(),
        entity,
        data: row.data.clone(),
        invocation_id: row.invocation_id.clone(),
        at: row.at,
        sim_at: row.sim_at,
    }
}

/// The customer's timeline: only customer-visible events.
pub fn customer_timeline(rows: &[OrderEventRow]) -> Vec<OrderEvent> {
    rows.iter()
        .filter(|e| e.kind.is_customer_visible())
        .map(|e| order_event(e, None))
        .collect()
}

/// What a card statement shows for an order.
pub fn statement_descriptor(number: techdepo_domain::OrderNumber) -> String {
    format!("TECHDEPO*{number}")
}

/// A ledger entry.
pub fn payment_entry(row: &PaymentRow) -> PaymentEntry {
    PaymentEntry {
        id: row.id.clone(),
        order_id: row.order_id.clone(),
        order_number: row.order_number,
        kind: row.kind,
        status: row.status,
        amount: row.amount,
        method: row.method,
        provider_ref: row.provider_ref.clone(),
        statement_descriptor: statement_descriptor(row.order_number),
        refund_of: row.refund_of.clone(),
        failure_reason: row.failure_reason.clone(),
        created_at: row.created_at,
        sim_at: row.sim_at,
    }
}

/// An invoice in a list.
pub fn invoice_summary(row: &InvoiceRow) -> InvoiceSummary {
    let link = |id: &Option<techdepo_domain::InvoiceId>,
                number: &Option<techdepo_domain::InvoiceNumber>| {
        match (id, number) {
            (Some(id), Some(number)) => Some(InvoiceRef {
                id: id.clone(),
                number: *number,
            }),
            _ => None,
        }
    };
    InvoiceSummary {
        id: row.id.clone(),
        number: row.number,
        kind: row.kind,
        status: row.status,
        order_id: row.order_id.clone(),
        order_number: row.order_number,
        amounts: techdepo_domain::VatBreakdown {
            gross: row.gross,
            net: row.net,
            vat: row.vat,
        },
        vat_rate: row.vat_rate,
        billing_name: row.billing_name.clone(),
        missing_billing_address: row.kind == InvoiceKind::Invoice && row.billing_address.is_none(),
        issue_date: row.issue_date,
        issued_at: row.issued_at,
        voided_at: row.voided_at,
        void_reason: row.void_reason.clone(),
        supersedes: link(&row.supersedes_id, &row.supersedes_number),
        superseded_by: link(&row.superseded_by_id, &row.superseded_by_number),
        credit_for: link(&row.credit_for_id, &row.credit_for_number),
    }
}

/// A link to an invoice.
pub fn invoice_ref(row: &InvoiceRow) -> InvoiceRef {
    InvoiceRef {
        id: row.id.clone(),
        number: row.number,
    }
}

/// An invoice line.
pub fn invoice_line(row: &InvoiceLineRow) -> InvoiceLine {
    InvoiceLine {
        line_no: row.line_no,
        product_id: row.product_id.clone(),
        sku: row.sku.clone(),
        description: row.description.clone(),
        quantity: row.quantity,
        unit_price: row.unit_price,
        amounts: techdepo_domain::VatBreakdown {
            gross: row.gross,
            net: row.net,
            vat: row.vat,
        },
    }
}

/// The document as issued.
pub fn invoice_document(
    row: &InvoiceRow,
    lines: &[InvoiceLineRow],
    html: String,
) -> InvoiceDocument {
    InvoiceDocument {
        invoice: invoice_summary(row),
        billing_address: row.billing_address.clone(),
        lines: lines.iter().map(invoice_line).collect(),
        html,
    }
}

/// Whether an invoice can be voided and reissued.
pub fn can_void(row: &InvoiceRow) -> bool {
    row.kind == InvoiceKind::Invoice && row.status == InvoiceStatus::Issued
}

/// A carrier event.
pub fn carrier_event(row: &CarrierEventRow) -> CarrierEvent {
    CarrierEvent {
        seq: row.seq,
        kind: row.kind,
        internal: row.internal,
        description: row.description.clone(),
        location: row.location.clone(),
        at: row.at,
        sim_at: row.sim_at,
    }
}

/// Tracking. Pass only customer-visible events for the customer; the status is whatever the
/// carrier shows (a lost parcel stays `in_transit`).
pub fn shipment_view(row: &ShipmentRow, events: &[CarrierEventRow]) -> ShipmentView {
    ShipmentView {
        id: row.id.clone(),
        carrier: row.carrier.clone(),
        tracking_number: row.tracking_number.clone(),
        status: row.status,
        shipped_on: row.shipped_on,
        eta: row.eta,
        delivered_on: row.delivered_on,
        events: events.iter().map(carrier_event).collect(),
    }
}

// -- demo controls ------------------------------------------------------------------------------

/// The fault that fires if `action` runs now, as a label.
pub fn armed_fault(action: OrderAction, faults: &FaultToggles) -> Option<String> {
    match action {
        OrderAction::Pay => match faults.payment {
            PaymentFault::None => None,
            PaymentFault::ChargeTwice => Some("Charge twice".to_owned()),
            PaymentFault::Declined => Some("Payment declined".to_owned()),
        },
        OrderAction::Ship => {
            let lost = match faults.shipment {
                ShipmentFault::None => None,
                ShipmentFault::Lost { outcome } => {
                    Some(format!("Lost in transit ({})", lost_label(outcome)))
                }
            };
            let down =
                (faults.carrier == CarrierFault::ApiDown).then(|| "Carrier API down".to_owned());
            match (lost, down) {
                (Some(a), Some(b)) => Some(format!("{a}, {b}")),
                (a, b) => a.or(b),
            }
        }
        _ => None,
    }
}

fn lost_label(outcome: LostOutcome) -> &'static str {
    match outcome {
        LostOutcome::Resurfaces => "resurfaces",
        LostOutcome::Gone => "gone",
    }
}

/// The demo controls drawer: valid next actions only, armed faults, the simulated clock (with
/// workdays counted on the org's `calendar`) and the hidden truth.
pub fn demo_panel(
    order: &OrderRow,
    faults: Option<&DemoFaultsRow>,
    clock: VirtualClock,
    shipment: Option<&ShipmentRow>,
    calendar: &WorkCalendar,
    real_now: OffsetDateTime,
) -> DemoPanel {
    let toggles = faults.map(|f| f.faults).unwrap_or_default();
    let simulated_now = clock.now(real_now);
    let actions = demo_actions(order.status)
        .into_iter()
        .map(|action| DemoActionView {
            action,
            label: action.label().to_owned(),
            armed_fault: armed_fault(action, &toggles),
        })
        .collect();
    let mut hidden_truth = Vec::new();
    if let Some(s) = shipment {
        if let Some(outcome) = s.lost_outcome
            && s.status == ShipmentStatus::InTransit
        {
            hidden_truth.push(HiddenTruth::ParcelLost { outcome });
        }
        if let Some(status) = s.trace_status {
            hidden_truth.push(HiddenTruth::Trace { status });
        }
    }
    if let Some(until) = faults.and_then(|f| f.email_outage_until)
        && until > real_now
    {
        hidden_truth.push(HiddenTruth::EmailOutage { until });
    }
    if toggles.carrier == CarrierFault::ApiDown {
        hidden_truth.push(HiddenTruth::CarrierApiDown);
    }
    let clock_steps = if order.status.is_terminal() {
        Vec::new()
    } else {
        ClockStep::available(order.status)
    };
    DemoPanel {
        order: order_ref(order),
        actions,
        faults: toggles,
        clock: ClockView {
            offset_secs: clock.offset_secs(),
            real_now,
            simulated_now,
            shipped_on: shipment.map(|s| s.shipped_on),
            workdays_since_ship: shipment
                .map(|s| calendar.workdays_between(s.shipped_on, simulated_now.date())),
            eta: shipment.map(|s| s.eta),
        },
        clock_steps,
        hidden_truth,
    }
}

/// Whether the order status is one where the demo panel offers anything.
pub fn has_demo_controls(status: OrderStatus) -> bool {
    !status.is_terminal()
}

// -- tickets ------------------------------------------------------------------------------------

/// A link to a ticket.
pub fn ticket_ref(row: &TicketRow) -> TicketRef {
    TicketRef {
        id: row.id.clone(),
        subject: row.subject.clone(),
        status: row.status,
    }
}

/// A ticket in a list. `visible_messages` is the customer-visible message count.
pub fn ticket_summary(row: &TicketRow, visible_messages: u32) -> TicketSummary {
    let order = match (
        &row.order_id,
        row.linked_order_number,
        row.linked_order_status,
    ) {
        (Some(id), Some(number), Some(status)) => Some(OrderRef {
            id: id.clone(),
            number,
            status,
        }),
        _ => None,
    };
    TicketSummary {
        id: row.id.clone(),
        subject: row.subject.clone(),
        status: row.status,
        channel: row.channel,
        order,
        message_count: visible_messages,
        last_message_at: row.last_message_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// A message.
pub fn ticket_message(row: &TicketMessageRow) -> TicketMessage {
    TicketMessage {
        seq: row.seq,
        direction: row.direction,
        author: row.author,
        channel: row.channel,
        status: row.status,
        subject: row.subject.clone(),
        body_text: row.body_text.clone(),
        body_html: row.body_html.clone(),
        created_at: row.created_at,
        sent_at: row.sent_at,
    }
}

#[cfg(test)]
mod tests {
    use techdepo_api_types::Address;
    use techdepo_api_types::vocab::PaymentMethod;
    use techdepo_domain::{OrderId, OrderNumber, UserId, VatRate};
    use time::macros::datetime;

    use super::*;
    use crate::carts::CartItemRow;

    fn item(id: &str, quantity: u32) -> CartItemRow {
        CartItemRow {
            product_id: ProductId::new(id),
            quantity,
            added_at: datetime!(2026-10-13 12:00 UTC),
            updated_at: datetime!(2026-10-13 12:00 UTC),
        }
    }

    #[test]
    fn cart_prices_and_flags_stock() {
        let catalog = Catalog::global();
        let cpu = &catalog.by_category(techdepo_catalog::Category::Cpu)[0];
        let ram = &catalog.by_category(techdepo_catalog::Category::Memory)[0];
        let rows = CartRows {
            items: vec![
                item(cpu.id.as_str(), 1),
                item(ram.id.as_str(), 2),
                item("gone-product", 1),
            ],
            updated_at: None,
        };
        let stock = HashMap::from([(cpu.id.clone(), 5), (ram.id.clone(), 1)]);
        let cart = cart(&rows, &stock, catalog);
        assert_eq!(cart.lines.len(), 2, "unknown products are dropped");
        assert_eq!(cart.items_total, cpu.price + ram.price * 2_u32);
        assert_eq!(cart.total, cart.items_total + SHIPPING_FEE);
        assert_eq!(cart.out_of_stock, vec![ram.id.clone()]);
        assert!(cart.report.is_some());
    }

    #[test]
    fn two_cpus_are_not_one_build() {
        let catalog = Catalog::global();
        let cpus = catalog.by_category(techdepo_catalog::Category::Cpu);
        let rows = CartRows {
            items: vec![item(cpus[0].id.as_str(), 1), item(cpus[1].id.as_str(), 1)],
            updated_at: None,
        };
        assert!(cart(&rows, &HashMap::new(), catalog).report.is_none());
    }

    fn order(status: OrderStatus) -> OrderRow {
        let t = datetime!(2026-10-16 14:00 UTC);
        OrderRow {
            id: OrderId::new("ord_1"),
            number: OrderNumber::new(1042),
            user_id: UserId::new("user_1"),
            customer_email: "a@b.c".into(),
            customer_name: None,
            status,
            refund_pending: false,
            investigation_open: false,
            replacement_for: None,
            items_total: Money::from_euros(100),
            shipping: Money::ZERO,
            total: Money::from_euros(100),
            vat_rate: VatRate::DEFAULT,
            payment_method: PaymentMethod::Card,
            shipping_address: Address::default(),
            billing_address: None,
            compat_override: false,
            paid_at: None,
            shipped_at: None,
            delivered_at: None,
            cancelled_at: None,
            refunded_at: None,
            returned_at: None,
            created_at: t,
            updated_at: t,
        }
    }

    #[test]
    fn demo_panel_shows_valid_actions_and_armed_faults() {
        let now = datetime!(2026-10-16 14:00 UTC);
        let faults = DemoFaultsRow {
            faults: FaultToggles {
                payment: PaymentFault::ChargeTwice,
                ..FaultToggles::default()
            },
            email_outage_until: None,
            updated_at: now,
        };
        let panel = demo_panel(
            &order(OrderStatus::AwaitingPayment),
            Some(&faults),
            VirtualClock::default(),
            None,
            &WorkCalendar::new(),
            now,
        );
        let actions: Vec<_> = panel.actions.iter().map(|a| a.action).collect();
        assert_eq!(actions, vec![OrderAction::Pay, OrderAction::Cancel]);
        assert_eq!(
            panel.actions[0].armed_fault.as_deref(),
            Some("Charge twice")
        );
        assert_eq!(panel.clock.simulated_now, now);
        assert!(panel.hidden_truth.is_empty());
        assert!(!panel.clock_steps.contains(&ClockStep::ShipLastFriday));

        let shipped = demo_panel(
            &order(OrderStatus::Shipped),
            None,
            VirtualClock::default(),
            None,
            &WorkCalendar::new(),
            now,
        );
        assert!(shipped.actions.is_empty());
        assert!(shipped.clock_steps.contains(&ClockStep::ShipLastFriday));
        assert!(
            demo_panel(
                &order(OrderStatus::Cancelled),
                None,
                VirtualClock::default(),
                None,
                &WorkCalendar::new(),
                now
            )
            .clock_steps
            .is_empty()
        );
    }
}
