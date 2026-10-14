//! The order page's cards: items, shipment, addresses, the card statement, invoices and linked
//! tickets.

use dioxus::prelude::*;
use techdepo_api_types::orders::{InvoiceSummary, OrderDetail, ShipmentView};
use techdepo_api_types::vocab::{PaymentKind, PaymentMethod, PaymentStatus};
use techdepo_api_types::{
    Address, InvoiceKind, InvoiceStatus, Money, OrderNumber, OrderStatus, TicketRef,
};

use super::invoice::InvoiceDialog;
use crate::pages::Panel;
use crate::routes::Route;
use crate::ui::format::{datetime, day};
use crate::ui::product::ProductTile;
use crate::ui::{
    Icon, IconName, InvoiceBadge, MoneyText, Mono, PaymentBadge, ShipmentStatusBadge,
    TicketStatusBadge,
};

#[component]
pub fn ItemsCard(detail: OrderDetail) -> Element {
    let totals = detail.totals;
    rsx! {
        Panel { title: "Items",
            ul { class: "flex flex-col divide-y divide-base-300",
                for item in detail.items.iter().cloned() {
                    li { key: "{item.line_no}", class: "flex items-center gap-3 py-2.5",
                        ProductTile { category: item.category, class: "size-10 shrink-0", icon_class: "size-4" }
                        div { class: "flex min-w-0 flex-1 flex-col",
                            Link { to: Route::Product { id: item.product_id.to_string() }, class: "link link-hover truncate font-medium",
                                "{item.name}"
                            }
                            Mono { class: "text-xs text-base-content/60", "{item.sku}" }
                        }
                        if item.quantity > 1 {
                            span { class: "text-sm tabular-nums text-base-content/60", "{item.quantity} × {item.unit_price}" }
                        }
                        MoneyText { amount: item.line_total, class: "font-medium" }
                    }
                }
            }
            dl { class: "flex flex-col gap-1 border-t border-base-300 pt-3 text-sm",
                div { class: "flex justify-between",
                    dt { "Items" }
                    dd { MoneyText { amount: totals.items_total } }
                }
                div { class: "flex justify-between",
                    dt { "Shipping" }
                    dd {
                        if totals.shipping == Money::ZERO { "Free" } else { MoneyText { amount: totals.shipping } }
                    }
                }
                div { class: "flex justify-between text-base font-semibold",
                    dt { "Total" }
                    dd { MoneyText { amount: totals.total.gross } }
                }
                div { class: "flex justify-between text-xs text-base-content/60",
                    dt { "incl. {totals.vat_rate} VAT" }
                    dd { MoneyText { amount: totals.total.vat } }
                }
            }
            if detail.compat_override {
                p { class: "flex items-center gap-1.5 text-xs text-base-content/70",
                    Icon { name: IconName::TriangleAlert, class: "size-3.5" }
                    "Ordered despite compatibility errors."
                }
            }
        }
    }
}

#[component]
pub fn ShipmentCard(shipment: Option<ShipmentView>, status: OrderStatus) -> Element {
    let Some(shipment) = shipment else {
        let note = match status {
            OrderStatus::AwaitingPayment => "Ships after payment.",
            OrderStatus::Paid | OrderStatus::Processing => {
                "Being prepared. Tracking appears once it ships."
            }
            _ => "This order was not shipped.",
        };
        return rsx! {
            Panel { title: "Shipment",
                p { class: "flex items-center gap-2 text-sm text-base-content/70",
                    Icon { name: IconName::Truck, class: "size-4" }
                    "{note}"
                }
            }
        };
    };
    let last = shipment.events.last().cloned();
    rsx! {
        Panel {
            title: "Shipment",
            actions: rsx! { ShipmentStatusBadge { status: shipment.status } },
            dl { class: "grid grid-cols-[7rem_1fr] gap-x-3 gap-y-1.5 text-sm",
                dt { class: "text-base-content/60", "Carrier" }
                dd { "{shipment.carrier}" }
                dt { class: "text-base-content/60", "Tracking" }
                dd { Mono { "{shipment.tracking_number}" } }
                dt { class: "text-base-content/60", "Shipped" }
                dd { "{day(shipment.shipped_on)}" }
                if let Some(delivered) = shipment.delivered_on {
                    dt { class: "text-base-content/60", "Delivered" }
                    dd { "{day(delivered)}" }
                } else {
                    dt { class: "text-base-content/60", "Expected" }
                    dd { "by {day(shipment.eta)}" }
                }
                if let Some(event) = last.clone() {
                    dt { class: "text-base-content/60", "Last scan" }
                    dd {
                        "{datetime(event.sim_at)}"
                        if let Some(location) = event.location.clone() { " · {location}" }
                        span { class: "block text-base-content/70", "{event.description}" }
                    }
                }
            }
            if shipment.events.len() > 1 {
                details { class: "text-sm",
                    summary { class: "cursor-pointer text-base-content/70 hover:text-base-content", "All {shipment.events.len()} scans" }
                    ol { class: "mt-2 flex flex-col gap-1.5 border-l border-base-300 pl-3",
                        for event in shipment.events.iter().rev().cloned() {
                            li { key: "{event.seq}", class: "flex flex-col",
                                span { "{event.description}" }
                                span { class: "text-xs text-base-content/60",
                                    "{datetime(event.sim_at)}"
                                    if let Some(location) = event.location.clone() { " · {location}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn AddressesCard(shipping: Address, billing: Option<Address>) -> Element {
    rsx! {
        Panel { title: "Addresses",
            div { class: "grid gap-4 text-sm @md:grid-cols-2",
                div { class: "flex flex-col",
                    span { class: "mb-1 text-xs font-medium uppercase tracking-wider text-base-content/50", "Shipping" }
                    for (i , line) in shipping.lines().into_iter().enumerate() {
                        span { key: "{i}", "{line}" }
                    }
                }
                div { class: "flex flex-col",
                    span { class: "mb-1 text-xs font-medium uppercase tracking-wider text-base-content/50", "Billing" }
                    match billing {
                        Some(billing) => rsx! {
                            for (i , line) in billing.lines().into_iter().enumerate() {
                                span { key: "{i}", "{line}" }
                            }
                        },
                        None => rsx! { span { class: "text-base-content/70", "Same as shipping" } },
                    }
                }
            }
        }
    }
}

/// The "card statement": every charge and refund as the customer's bank shows it. This is how
/// a customer notices a double charge.
#[component]
pub fn StatementCard(detail: OrderDetail) -> Element {
    let (title, subtitle) = match detail.payment_method {
        PaymentMethod::Card => ("Card statement", "Visa ···· 4242 (simulated)"),
        PaymentMethod::BankTransfer => ("Account statement", "Bank transfer (simulated)"),
        PaymentMethod::Store => ("Statement", "Paid by the store"),
    };
    let payments = detail.payments.clone();
    let net_charged = -payments
        .iter()
        .map(|p| p.signed_amount())
        .fold(Money::ZERO, |a, b| a + b);
    let captured = payments
        .iter()
        .filter(|p| p.kind == PaymentKind::Charge && p.status == PaymentStatus::Succeeded)
        .count();
    let total = detail.totals.total.gross;

    rsx! {
        Panel { title, subtitle: subtitle.to_owned(),
            if payments.is_empty() {
                p { class: "text-sm text-base-content/70",
                    if detail.order.status == OrderStatus::AwaitingPayment {
                        "Nothing charged yet. Payment is pending."
                    } else {
                        "No payments on this order."
                    }
                }
            } else {
                ul { class: "flex flex-col divide-y divide-base-300",
                    for payment in payments.iter().cloned() {
                        li { key: "{payment.id}", class: "flex items-center gap-3 py-2.5",
                            div { class: "flex min-w-0 flex-1 flex-col",
                                Mono { class: "whitespace-nowrap text-xs @md:text-sm", "{payment.statement_descriptor}" }
                                span { class: "text-xs text-base-content/60",
                                    "{datetime(payment.sim_at)}"
                                    if let Some(reason) = payment.failure_reason.clone() { " · {reason}" }
                                }
                            }
                            PaymentBadge { kind: payment.kind, status: payment.status }
                            span {
                                class: "min-w-24 whitespace-nowrap text-right font-medium tabular-nums",
                                class: if payment.status == PaymentStatus::Failed { "text-base-content/50 line-through" },
                                if payment.kind == PaymentKind::Refund { "+{payment.amount}" } else { "−{payment.amount}" }
                            }
                        }
                    }
                }
                div { class: "flex justify-between border-t border-base-300 pt-3 text-sm",
                    span { "Net charged" }
                    MoneyText { amount: net_charged, class: "font-semibold" }
                }
                if captured > 1 && net_charged > total {
                    p { class: "flex items-start gap-1.5 text-xs text-base-content/70",
                        Icon { name: IconName::Info, class: "mt-0.5 size-3.5" }
                        "More was charged than the order total ({total}). Email support and we'll sort it out."
                    }
                }
            }
        }
    }
}

#[component]
pub fn InvoicesCard(number: String, invoices: Vec<InvoiceSummary>) -> Element {
    let mut viewing = use_signal(|| None::<InvoiceSummary>);
    rsx! {
        Panel { title: "Invoices",
            if invoices.is_empty() {
                p { class: "text-sm text-base-content/70", "The invoice is issued once the order is paid." }
            } else {
                ul { class: "flex flex-col divide-y divide-base-300",
                    for invoice in invoices.iter().cloned() {
                        li { key: "{invoice.id}", class: "flex items-center gap-3 py-2.5",
                            Icon { name: IconName::FileText, class: "size-4 text-base-content/60" }
                            div { class: "flex min-w-0 flex-1 flex-col",
                                div { class: "flex flex-wrap items-center gap-2",
                                    Mono {
                                        class: if invoice.status == InvoiceStatus::Voided { "line-through text-base-content/60" } else { "" },
                                        "{invoice.number}"
                                    }
                                    InvoiceBadge { invoice: invoice.clone() }
                                }
                                span { class: "text-xs text-base-content/60", {invoice_note(&invoice)} }
                            }
                            MoneyText { amount: invoice.amounts.gross, class: "text-sm" }
                            button {
                                class: "btn btn-ghost btn-sm",
                                onclick: {
                                    let invoice = invoice.clone();
                                    move |_| viewing.set(Some(invoice.clone()))
                                },
                                "View"
                            }
                        }
                    }
                }
            }
        }
        if let Some(invoice) = viewing() {
            InvoiceDialog { number, invoice, on_close: move |_| viewing.set(None) }
        }
    }
}

/// `Invoice · issued Mon 12 Oct · replaces TD-2026-000041 · no billing address`.
pub(crate) fn invoice_note(invoice: &InvoiceSummary) -> String {
    let mut parts = vec![format!(
        "{} · issued {}",
        invoice.kind.label(),
        day(invoice.issue_date)
    )];
    if let Some(credit_for) = &invoice.credit_for {
        parts.push(format!("cancels {}", credit_for.number));
    }
    if let Some(supersedes) = &invoice.supersedes {
        parts.push(format!("replaces {}", supersedes.number));
    }
    if let Some(by) = &invoice.superseded_by {
        parts.push(format!("replaced by {}", by.number));
    }
    if invoice.kind == InvoiceKind::Invoice && invoice.missing_billing_address {
        parts.push("no billing address".to_owned());
    }
    parts.join(" · ")
}

#[component]
pub fn TicketsCard(number: OrderNumber, tickets: Vec<TicketRef>, mailto: String) -> Element {
    rsx! {
        Panel { title: "Support", class: "self-start",
            if tickets.is_empty() {
                p { class: "text-sm text-base-content/70", "No conversations about this order yet." }
            } else {
                ul { class: "flex flex-col gap-2",
                    for ticket in tickets {
                        li { key: "{ticket.id}",
                            Link {
                                to: Route::AccountTicket { id: ticket.id.to_string() },
                                class: "flex items-center gap-2 rounded-field border border-base-300 p-2.5 hover:bg-base-200",
                                div { class: "flex min-w-0 flex-1 flex-col",
                                    span { class: "truncate text-sm font-medium", "{ticket.subject}" }
                                    Mono { class: "text-xs text-base-content/60", "{ticket.id}" }
                                }
                                TicketStatusBadge { status: ticket.status }
                            }
                        }
                    }
                }
            }
            div { class: "flex flex-col gap-2",
                a { class: "btn btn-sm", href: "{mailto}",
                    Icon { name: IconName::Mail }
                    "Email support"
                }
                Link {
                    to: Route::AccountSupport { subject: format!("Order {number}") },
                    class: "btn btn-ghost btn-sm",
                    Icon { name: IconName::Send }
                    "Write from the web form"
                }
            }
        }
    }
}
