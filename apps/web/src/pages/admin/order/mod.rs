//! One order with everything internal (commerce.md §7): items, the payments ledger with the
//! duplicate detector, invoices with void & reissue, shipment with the carrier's internal events
//! and the hidden truth, tickets, investigations, refunds, human tasks and the full timeline.
//!
//! The admin actions that apply now (`AdminOrderActions`) sit in the header; the demo controls
//! drawer drives the order like on the customer's page (admins may drive any order).

mod actions;

use dioxus::prelude::*;
use techdepo_api_types::admin::{AdminOrderDetail, DuplicateCharge, find_duplicate_charges};
use techdepo_api_types::endpoints::GetAdminOrder;
use techdepo_api_types::orders::{InvoiceSummary, PaymentEntry};
use techdepo_api_types::restate_ui::RestateUiLink;
use techdepo_api_types::vocab::{PaymentKind, PaymentStatus};
use techdepo_api_types::{LostOutcome, Money};
use techdepo_domain::{CarrierFault, EmailFault, InvoiceFault, PaymentFault, ShipmentFault};

use super::common::{BackLink, CustomerCell, Muted, OrderLink, TicketRefView, apart};
use super::invoices::{VoidReissueDialog, can_void};
use super::tasks::TaskRow;
use crate::api::use_api;
use crate::browser;
use crate::pages::Panel;
use crate::pages::account::demo::OrderDemoControls;
use crate::pages::account::order::cards::{AddressesCard, ItemsCard, ShipmentCard};
use crate::pages::account::order::invoice::InvoiceDialog;
use crate::pages::account::order::timeline::OrderTimeline;
use crate::routes::Route;
use crate::ui::format::{date, datetime, datetime_long};
use crate::ui::{
    DemoFrame, DemoSection, ErrorAlert, Icon, IconName, InvestigationStatusBadge, InvoiceBadge,
    MoneyText, Mono, OrderFlagBadges, OrderStatusBadge, PageSkeleton, PaymentBadge,
    RefundStatusBadge, RestateLink, ShortId,
};

use actions::{ActionDialog, OrderActions};

/// After a mutation: re-read now, then again while asynchronous effects land (workflows start
/// one-way and write their records moments later).
const FOLLOW_UP_MS: [u32; 2] = [1_500, 4_000];

#[component]
pub fn AdminOrder(number: String) -> Element {
    let api = use_api();
    let mut order = use_resource(use_reactive!(|(number,)| async move {
        api.get::<GetAdminOrder>(&[number.as_str()]).await
    }));
    let mut dialog = use_signal(|| None::<ActionDialog>);

    let refresh = use_callback(move |_: ()| {
        order.restart();
        spawn(async move {
            for ms in FOLLOW_UP_MS {
                browser::sleep(ms).await;
                order.restart();
            }
        });
    });

    let detail = match &*order.read() {
        None => return rsx! { PageSkeleton { rows: 4 } },
        Some(Err(error)) => {
            return rsx! {
                BackLink { to: Route::AdminOrders {}, label: "All orders" }
                ErrorAlert { error: error.clone(), on_retry: move |_| order.restart() }
            };
        }
        Some(Ok(detail)) => detail.clone(),
    };
    let summary = detail.order.order.clone();

    rsx! {
        BackLink { to: Route::AdminOrders {}, label: "All orders" }
        div { class: "mb-5 flex flex-wrap items-start justify-between gap-4",
            div { class: "flex flex-col gap-1",
                div { class: "flex flex-wrap items-center gap-2",
                    h1 { class: "text-2xl font-semibold tracking-tight", Mono { "{summary.number}" } }
                    OrderStatusBadge { status: summary.status }
                    OrderFlagBadges { flags: summary.flags.clone() }
                    RestateLink { link: RestateUiLink::order(&summary.id), class: "text-sm" }
                }
                p { class: "flex flex-wrap items-center gap-x-2 text-base-content/60",
                    "Placed {date(summary.created_at)} · simulated now {datetime_long(detail.order.simulated_now)}"
                    if let Some(original) = summary.replacement_for_number {
                        " · replaces "
                        OrderLink { number: original }
                    }
                }
            }
            OrderActions { detail: detail.clone(), on_open: move |d| dialog.set(Some(d)) }
        }

        div { class: "grid items-start gap-5 *:min-w-0 @4xl:grid-cols-[minmax(0,1fr)_22rem]",
            div { class: "flex flex-col gap-5",
                ItemsCard { detail: detail.order.clone() }
                LedgerCard { payments: detail.order.payments.clone(), total: detail.order.totals.total.gross, refundable: detail.actions.refundable }
                InvoicesCard {
                    invoices: detail.order.invoices.clone(),
                    order_number: summary.number.to_string(),
                    on_void: move |invoice| dialog.set(Some(ActionDialog::VoidReissue(invoice))),
                }
                OrderTimeline { events: detail.order.timeline.clone(), admin: true }
            }
            div { class: "flex flex-col gap-5",
                Panel { title: "Customer",
                    CustomerCell { customer: detail.customer.clone() }
                }
                ShipmentCard { shipment: detail.order.shipment.clone(), status: summary.status }
                TruthCard { detail: detail.clone() }
                AddressesCard { shipping: detail.order.shipping_address.clone(), billing: detail.order.billing_address.clone() }
                CasesCard { detail: detail.clone() }
            }
        }
        div { class: "h-16" }

        OrderDemoControls {
            detail: detail.order.clone(),
            panel: Some(Ok(detail.demo.clone())),
            on_changed: refresh,
            admin: true,
        }

        match dialog() {
            Some(ActionDialog::VoidReissue(invoice)) => rsx! {
                VoidReissueDialog {
                    invoice,
                    on_close: move |_| dialog.set(None),
                    on_done: move |_| {
                        dialog.set(None);
                        refresh.call(());
                    },
                }
            },
            Some(other) => rsx! {
                actions::ActionDialogView {
                    dialog: other,
                    detail: detail.clone(),
                    on_close: move |_| dialog.set(None),
                    on_done: move |_| {
                        dialog.set(None);
                        refresh.call(());
                    },
                }
            },
            None => rsx! {},
        }
    }
}

/// The order's payments ledger, with suspected duplicates marked (the same deterministic
/// detector as the payments page).
#[component]
fn LedgerCard(payments: Vec<PaymentEntry>, total: Money, refundable: Money) -> Element {
    let duplicates = find_duplicate_charges(&payments);
    let net: Money = -payments
        .iter()
        .map(PaymentEntry::signed_amount)
        .fold(Money::ZERO, |a, b| a + b);
    rsx! {
        Panel {
            title: "Payments",
            subtitle: format!("Net charged {net} of {total} · {refundable} refundable"),
            if payments.is_empty() {
                Muted { "No payments yet." }
            } else {
                div { class: "-mx-2 overflow-x-auto",
                    table { class: "table table-sm",
                        thead {
                            tr {
                                th { "Payment" }
                                th { "When (simulated)" }
                                th { "State" }
                                th { class: "text-right", "Amount" }
                            }
                        }
                        tbody {
                            for payment in payments.iter().cloned() {
                                LedgerRow {
                                    key: "{payment.id}",
                                    duplicate: duplicates.iter().find(|d| d.duplicate == payment.id).cloned(),
                                    payment,
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
fn LedgerRow(payment: PaymentEntry, duplicate: Option<DuplicateCharge>) -> Element {
    let suspicious = duplicate.as_ref().is_some_and(|d| !d.resolved);
    rsx! {
        tr { class: if suspicious { "bg-warning/10" },
            td {
                div { class: "flex flex-col",
                    ShortId { id: payment.id.to_string() }
                    span { class: "text-xs text-base-content/60",
                        "{payment.kind.label()} · "
                        ShortId { id: payment.provider_ref.to_string() }
                        if let Some(of) = payment.refund_of.clone() { " · reverses " ShortId { id: of.to_string() } }
                        if let Some(reason) = payment.failure_reason.clone() { " · {reason}" }
                    }
                    if let Some(d) = duplicate {
                        span { class: "mt-0.5 flex flex-wrap items-center gap-x-1 text-xs font-medium text-warning",
                            Icon { name: IconName::TriangleAlert, class: "size-3" }
                            span { class: "whitespace-nowrap", "Duplicate of " ShortId { id: d.first.to_string() } }
                            span { class: "whitespace-nowrap",
                                "({apart(d.seconds_apart)} apart)"
                                if d.resolved { " · refunded" }
                            }
                        }
                    }
                }
            }
            td { class: "whitespace-nowrap", "{datetime(payment.sim_at)}" }
            td { PaymentBadge { kind: payment.kind, status: payment.status } }
            td {
                class: "whitespace-nowrap text-right font-medium tabular-nums",
                class: if payment.status == PaymentStatus::Failed { "text-base-content/50 line-through" },
                if payment.kind == PaymentKind::Refund { "−{payment.amount}" } else { "+{payment.amount}" }
            }
        }
    }
}

#[component]
fn InvoicesCard(
    invoices: Vec<InvoiceSummary>,
    order_number: String,
    on_void: EventHandler<InvoiceSummary>,
) -> Element {
    let mut viewing = use_signal(|| None::<InvoiceSummary>);
    rsx! {
        Panel { title: "Invoices",
            if invoices.is_empty() {
                Muted { "Issued after payment." }
            }
            ul { class: "flex flex-col divide-y divide-base-300",
                for invoice in invoices.iter().cloned() {
                    li { key: "{invoice.id}", class: "flex flex-wrap items-center gap-2 py-2",
                        Mono { class: "font-medium", "{invoice.number}" }
                        InvoiceBadge { invoice: invoice.clone() }
                        if invoice.missing_billing_address && invoice.kind == techdepo_api_types::InvoiceKind::Invoice {
                            span { class: "inline-flex items-center gap-1 text-xs text-warning",
                                Icon { name: IconName::TriangleAlert, class: "size-3" }
                                "No billing address"
                            }
                        }
                        span { class: "flex-1" }
                        MoneyText { amount: invoice.amounts.gross }
                        button { class: "btn btn-ghost btn-xs", onclick: {
                            let invoice = invoice.clone();
                            move |_| viewing.set(Some(invoice.clone()))
                        }, "View" }
                        if can_void(&invoice) {
                            button { class: "btn btn-ghost btn-xs", onclick: {
                                let invoice = invoice.clone();
                                move |_| on_void.call(invoice.clone())
                            }, "Void & reissue" }
                        }
                    }
                }
            }
        }
        if let Some(invoice) = viewing() {
            InvoiceDialog { number: order_number.clone(), invoice, admin: true, on_close: move |_| viewing.set(None) }
        }
    }
}

/// The hidden truth and the armed faults: what the customer cannot see. Demo-styled, because
/// it describes the simulation, not the shop.
#[component]
fn TruthCard(detail: AdminOrderDetail) -> Element {
    let faults = detail.faults;
    let mut armed = Vec::new();
    match faults.payment {
        PaymentFault::ChargeTwice => armed.push("Payment: charge twice".to_owned()),
        PaymentFault::Declined => armed.push("Payment: decline".to_owned()),
        PaymentFault::None => {}
    }
    if faults.invoice == InvoiceFault::MissingBillingAddress {
        armed.push("Invoice: no billing address".to_owned());
    }
    if let ShipmentFault::Lost { outcome } = faults.shipment {
        armed.push(format!("Shipment: lost ({})", outcome_word(outcome)));
    }
    if let EmailFault::Outage { minutes } = faults.email {
        armed.push(format!("Email: {minutes} min outage"));
    }
    if faults.carrier == CarrierFault::ApiDown {
        armed.push("Carrier API down".to_owned());
    }
    let truth = detail.shipment_truth;

    rsx! {
        DemoFrame {
            DemoSection { title: "Hidden truth", icon: IconName::EyeOff,
                match truth {
                    Some(t) if t.lost_outcome.is_some() || t.trace_status.is_some() => rsx! {
                        ul { class: "flex flex-col gap-1",
                            if let Some(outcome) = t.lost_outcome {
                                li { "The parcel is lost (outcome: {outcome_word(outcome)})." }
                            }
                            if let Some(status) = t.trace_status {
                                li { "Carrier trace: {status.label().to_lowercase()}." }
                            }
                        }
                    },
                    _ => rsx! { p { "Nothing hidden about the shipment." } },
                }
            }
            DemoSection { title: "Armed faults", icon: IconName::Zap,
                if armed.is_empty() {
                    p { "None." }
                }
                ul { class: "flex flex-col gap-1",
                    for (i , fault) in armed.into_iter().enumerate() {
                        li { key: "{i}", class: "flex items-center gap-1.5",
                            Icon { name: IconName::Zap, class: "size-3" }
                            "{fault}"
                        }
                    }
                }
            }
        }
    }
}

const fn outcome_word(outcome: LostOutcome) -> &'static str {
    match outcome {
        LostOutcome::Resurfaces => "resurfaces",
        LostOutcome::Gone => "gone",
    }
}

/// Tickets, investigations, refunds and human tasks about the order.
#[component]
fn CasesCard(detail: AdminOrderDetail) -> Element {
    rsx! {
        Panel { title: "Support and cases",
            div { class: "flex flex-col gap-4",
                div { class: "flex flex-col gap-2",
                    h3 { class: "text-xs font-medium uppercase tracking-wider text-base-content/50", "Tickets" }
                    if detail.order.tickets.is_empty() {
                        Muted { "None." }
                    }
                    for ticket in detail.order.tickets.iter().cloned() {
                        TicketRefView { key: "{ticket.id}", ticket }
                    }
                }
                div { class: "flex flex-col gap-2",
                    h3 { class: "text-xs font-medium uppercase tracking-wider text-base-content/50", "Investigations" }
                    if detail.investigations.is_empty() {
                        Muted { "None." }
                    }
                    for inv in detail.investigations.iter().cloned() {
                        Link {
                            key: "{inv.id}",
                            to: Route::AdminInvestigation { id: inv.id.to_string() },
                            class: "flex items-center justify-between gap-2 rounded-field border border-base-300 p-2 hover:bg-base-200",
                            span { class: "flex flex-col",
                                Mono { "{inv.id}" }
                                span { class: "text-xs text-base-content/60", "Opened {date(inv.created_at)} by {inv.opened_by.label().to_lowercase()}" }
                            }
                            InvestigationStatusBadge { status: inv.status }
                        }
                    }
                }
                div { class: "flex flex-col gap-2",
                    h3 { class: "text-xs font-medium uppercase tracking-wider text-base-content/50", "Refunds" }
                    if detail.refunds.is_empty() {
                        Muted { "None." }
                    }
                    for refund in detail.refunds.iter().cloned() {
                        Link {
                            key: "{refund.id}",
                            to: Route::AdminRefund { id: refund.id.to_string() },
                            class: "flex items-center justify-between gap-2 rounded-field border border-base-300 p-2 hover:bg-base-200",
                            span { class: "flex flex-col",
                                MoneyText { amount: refund.amount, class: "font-medium" }
                                span { class: "text-xs text-base-content/60", "{refund.reason.label()}" }
                            }
                            RefundStatusBadge { status: refund.status }
                        }
                    }
                }
                div { class: "flex flex-col gap-2",
                    h3 { class: "text-xs font-medium uppercase tracking-wider text-base-content/50", "Human tasks" }
                    if detail.tasks.is_empty() {
                        Muted { "None." }
                    }
                    for task in detail.tasks.iter().cloned() {
                        TaskRow { key: "{task.id}", task }
                    }
                }
            }
        }
    }
}
