//! The simulated payments ledger with the duplicate-charge detector (deterministic, no AI):
//! two captured charges of the same amount on one order within 15 minutes. Suspected pairs are
//! listed on top and highlighted in the ledger.

use dioxus::prelude::*;
use techdepo_api_types::admin::{LedgerEntry, PaymentListQuery, PaymentsLedger, parse_order_query};
use techdepo_api_types::endpoints::GetPayments;
use techdepo_api_types::vocab::{PaymentKind, PaymentStatus};
use techdepo_api_types::{ApiError, PaymentId};

use super::common::{
    CustomerCell, FilterBar, OrderLink, SearchFilter, SelectFilter, TableWrap, ToggleFilter, apart,
    non_blank, options, parsed,
};
use crate::api::use_api;
use crate::pages::{PageHeader, Panel};
use crate::ui::format::datetime;
use crate::ui::{
    EmptyState, ErrorAlert, Icon, IconName, MoneyText, PageSkeleton, PaymentBadge, ShortId,
};

#[component]
pub fn AdminPayments() -> Element {
    let api = use_api();
    let kind = use_signal(String::new);
    let status = use_signal(String::new);
    let order = use_signal(String::new);
    let suspicious = use_signal(|| false);
    // Ledger pages after the first; the duplicate list comes whole with the first page.
    let mut more = use_signal(Vec::<LedgerEntry>::new);
    let mut cursor = use_signal(|| None::<String>);
    let mut loading = use_signal(|| false);
    let mut more_error = use_signal(|| None::<ApiError>);

    let query = move |cursor: Option<String>| PaymentListQuery {
        kind: parsed::<PaymentKind>(&kind()),
        status: parsed::<PaymentStatus>(&status()),
        order: non_blank(&order()).and_then(|q| parse_order_query(&q)),
        suspicious: suspicious().then_some(true),
        cursor,
        limit: Some(50),
    };
    let mut ledger = use_resource(move || async move {
        let result = api.query::<GetPayments>(&[], &query(None)).await;
        more.set(Vec::new());
        more_error.set(None);
        cursor.set(
            result
                .as_ref()
                .ok()
                .and_then(|l: &PaymentsLedger| l.entries.next_cursor.clone()),
        );
        result
    });
    let load_more = move |_| {
        spawn(async move {
            loading.set(true);
            match api.query::<GetPayments>(&[], &query(cursor())).await {
                Ok(page) => {
                    more.write().extend(page.entries.items);
                    cursor.set(page.entries.next_cursor);
                }
                Err(e) => more_error.set(Some(e)),
            }
            loading.set(false);
        });
    };

    rsx! {
        PageHeader { title: "Payments", subtitle: "The simulated ledger. Duplicate charges are found by a fixed rule, not by AI." }
        FilterBar {
            SelectFilter { label: "Kind", value: kind, options: options(PaymentKind::ALL, PaymentKind::as_str, PaymentKind::label) }
            SelectFilter { label: "Status", value: status, options: options(PaymentStatus::ALL, PaymentStatus::as_str, PaymentStatus::label) }
            SearchFilter { label: "Order", placeholder: "TD-1042", value: order }
            ToggleFilter { label: "Suspected duplicates only", value: suspicious }
        }
        match &*ledger.read() {
            None => rsx! { PageSkeleton { rows: 2 } },
            Some(Err(error)) => rsx! { ErrorAlert { error: error.clone(), on_retry: move |_| ledger.restart() } },
            Some(Ok(data)) => {
                let mut entries = data.entries.items.clone();
                entries.extend(more());
                rsx! {
                    Duplicates { ledger: data.clone() }
                    if entries.is_empty() {
                        EmptyState { message: "No payments match.", icon: IconName::CreditCard }
                    } else {
                        TableWrap {
                            table { class: "table table-sm",
                                thead {
                                    tr {
                                        th { "Payment" }
                                        th { "Order" }
                                        th { "Customer" }
                                        th { "When (simulated)" }
                                        th { "State" }
                                        th { "Notes" }
                                        th { class: "text-right", "Amount" }
                                    }
                                }
                                tbody {
                                    for entry in entries {
                                        LedgerRow { key: "{entry.payment.id}", entry }
                                    }
                                }
                            }
                        }
                    }
                    if let Some(error) = more_error() {
                        div { class: "mt-3", ErrorAlert { error } }
                    }
                    if cursor().is_some() {
                        div { class: "mt-3 flex justify-center",
                            button { class: "btn btn-sm", disabled: loading(), onclick: load_more,
                                if loading() { span { class: "loading loading-spinner loading-xs" } }
                                "Load more"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn Duplicates(ledger: PaymentsLedger) -> Element {
    if ledger.duplicates.is_empty() {
        return rsx! {
            div { class: "mb-4 flex items-center gap-2 rounded-box border border-base-300 bg-base-100 p-3 text-base-content/70",
                Icon { name: IconName::ShieldCheck, class: "size-4 text-success" }
                "No suspected duplicate charges."
            }
        };
    }
    let open = ledger.duplicates.iter().filter(|d| !d.resolved).count();
    rsx! {
        Panel {
            class: "mb-4",
            title: "Suspected duplicate charges",
            subtitle: format!("{open} unresolved · same order, same amount, within 15 minutes"),
            ul { class: "flex flex-col gap-2",
                for d in ledger.duplicates.iter().cloned() {
                    li {
                        key: "{d.duplicate}",
                        class: if d.resolved { "flex flex-wrap items-center gap-x-3 gap-y-1 rounded-field border border-base-300 p-2.5" } else { "flex flex-wrap items-center gap-x-3 gap-y-1 rounded-field border border-warning/60 bg-warning/10 p-2.5" },
                        if d.resolved {
                            Icon { name: IconName::CircleCheck, class: "size-4 text-success", label: "Resolved".to_string() }
                        } else {
                            Icon { name: IconName::TriangleAlert, class: "size-4 text-warning", label: "Unresolved".to_string() }
                        }
                        OrderLink { number: d.order_number }
                        span { "Two charges of " MoneyText { amount: d.amount } }
                        span { class: "text-base-content/70",
                            ShortId { id: d.first.to_string() } " then " ShortId { id: d.duplicate.to_string() } ", {apart(d.seconds_apart)} apart"
                        }
                        span { class: "flex-1" }
                        span { class: "text-xs font-medium",
                            if d.resolved { "Refunded" } else { "Not refunded yet" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn LedgerRow(entry: LedgerEntry) -> Element {
    let p = entry.payment.clone();
    let flagged = entry.duplicate_of.is_some() && !entry.refunded;
    rsx! {
        tr { class: if flagged { "bg-warning/10" } else { "hover:bg-base-200" },
            td {
                div { class: "flex flex-col",
                    ShortId { id: p.id.to_string() }
                    span { class: "text-xs text-base-content/60 whitespace-nowrap", "{p.kind.label()} · {p.method.label()} · " ShortId { id: p.provider_ref.to_string() } }
                }
            }
            td { OrderLink { number: p.order_number } }
            td { class: "max-w-48", CustomerCell { customer: entry.customer.clone() } }
            td { class: "whitespace-nowrap", "{datetime(p.sim_at)}" }
            td { PaymentBadge { kind: p.kind, status: p.status } }
            td { class: "text-xs",
                if let Some(first) = entry.duplicate_of.clone() {
                    DuplicateNote { first, refunded: entry.refunded }
                } else if entry.refunded {
                    span { class: "text-base-content/70", "Refunded" }
                }
                if let Some(of) = p.refund_of.clone() {
                    span { class: "whitespace-nowrap text-base-content/70", "Reverses " ShortId { id: of.to_string() } }
                }
                if let Some(reason) = p.failure_reason.clone() {
                    span { class: "text-base-content/70", "{reason}" }
                }
            }
            td {
                class: "whitespace-nowrap text-right font-medium tabular-nums",
                class: if p.status == PaymentStatus::Failed { "text-base-content/50 line-through" },
                if p.kind == PaymentKind::Refund { "−{p.amount}" } else { "+{p.amount}" }
            }
        }
    }
}

#[component]
fn DuplicateNote(first: PaymentId, refunded: bool) -> Element {
    rsx! {
        span {
            class: if refunded { "inline-flex flex-wrap items-center gap-x-1 text-base-content/70" } else { "inline-flex flex-wrap items-center gap-x-1 font-medium text-warning" },
            Icon { name: if refunded { IconName::CircleCheck } else { IconName::TriangleAlert }, class: "size-3" }
            span { class: "whitespace-nowrap", "Duplicate of" }
            ShortId { id: first.to_string() }
            if refunded { span { class: "whitespace-nowrap", "· refunded" } }
        }
    }
}
