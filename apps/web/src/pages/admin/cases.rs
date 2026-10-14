//! Investigations and refunds: records mirrored from their Restate workflows. Investigations
//! have their own list because an admin can open one from an order without a ticket.

use dioxus::prelude::*;
use techdepo_api_types::LostOutcome;
use techdepo_api_types::admin::parse_order_query;
use techdepo_api_types::cases::{InvestigationListQuery, RefundListQuery};
use techdepo_api_types::endpoints::{GetInvestigation, GetRefund, ListInvestigations, ListRefunds};
use techdepo_api_types::orders::PaymentEntry;
use techdepo_api_types::restate_ui::RestateUiLink;
use techdepo_api_types::vocab::{InvestigationStatus, RefundStatus};

use super::common::{
    BackLink, CustomerCell, EvidenceList, Fact, Facts, FilterBar, Muted, OrderLink, OrderRefView,
    SearchFilter, SelectFilter, TableWrap, TicketRefView, ToggleFilter, non_blank, options, parsed,
    use_paged,
};
use super::tasks::TaskRow;
use crate::api::use_api;
use crate::pages::account::order::cards::ShipmentCard;
use crate::pages::{PageHeader, Panel};
use crate::routes::Route;
use crate::ui::format::{date, datetime};
use crate::ui::{
    ActorTag, DemoFrame, DemoSection, EmptyState, ErrorAlert, IconName, InvestigationStatusBadge,
    MoneyText, Mono, PageSkeleton, PaymentBadge, RefundStatusBadge, RestateLink,
};

// ---------------------------------------------------------------------------------------------
// Investigations
// ---------------------------------------------------------------------------------------------

#[component]
pub fn AdminInvestigations() -> Element {
    let api = use_api();
    let open_only = use_signal(|| false);
    let status = use_signal(String::new);
    let investigations = use_paged(move |cursor| async move {
        let status = parsed::<InvestigationStatus>(&status());
        let query = InvestigationListQuery {
            status,
            open: (open_only() && status.is_none()).then_some(true),
            cursor,
            limit: Some(50),
        };
        api.query::<ListInvestigations>(&[], &query).await
    });

    rsx! {
        PageHeader { title: "Investigations", subtitle: "Carrier traces for parcels that did not arrive, from a ticket or opened by hand." }
        FilterBar {
            ToggleFilter { label: "Open only", value: open_only }
            SelectFilter { label: "Status", value: status, options: options(InvestigationStatus::ALL, InvestigationStatus::as_str, InvestigationStatus::label) }
        }
        {investigations.render(|items| rsx! {
            if items.is_empty() {
                EmptyState { message: "No investigations. Lose a parcel with the demo controls, then ask where it is.", icon: IconName::Search }
            } else {
                TableWrap {
                    table { class: "table table-sm",
                        thead {
                            tr {
                                th { "Investigation" }
                                th { "Order" }
                                th { "Ticket" }
                                th { "Opened by" }
                                th { "Carrier says" }
                                th { "Outcome" }
                                th { "Status" }
                            }
                        }
                        tbody {
                            for inv in items {
                                tr { key: "{inv.id}", class: "hover:bg-base-200",
                                    td {
                                        Link { to: Route::AdminInvestigation { id: inv.id.to_string() }, class: "link link-hover flex flex-col",
                                            Mono { "{inv.id}" }
                                            span { class: "text-xs text-base-content/60", "{date(inv.created_at)}" }
                                        }
                                    }
                                    td { OrderLink { number: inv.order.number } }
                                    td {
                                        match inv.ticket.clone() {
                                            Some(t) => rsx! { Link { to: Route::admin_ticket(&t.id), class: "link link-hover", Mono { "{t.id}" } } },
                                            None => rsx! { span { class: "text-base-content/40", "–" } },
                                        }
                                    }
                                    td { ActorTag { actor: inv.opened_by } }
                                    td { {inv.carrier_result.map(carrier_text).unwrap_or("–")} }
                                    td {
                                        if let Some(outcome) = inv.outcome { "{outcome.label()}" }
                                        else if let Some(decision) = inv.decision { span { class: "text-base-content/70", "{decision.label()}" } }
                                        else { span { class: "text-base-content/40", "–" } }
                                    }
                                    td { InvestigationStatusBadge { status: inv.status } }
                                }
                            }
                        }
                    }
                }
            }
        })}
    }
}

const fn carrier_text(outcome: LostOutcome) -> &'static str {
    match outcome {
        LostOutcome::Resurfaces => "Found",
        LostOutcome::Gone => "No information",
    }
}

#[component]
pub fn AdminInvestigation(id: String) -> Element {
    let api = use_api();
    let mut detail = use_resource(use_reactive!(|(id,)| async move {
        api.get::<GetInvestigation>(&[id.as_str()]).await
    }));
    let d = match &*detail.read() {
        None => return rsx! { PageSkeleton { rows: 3 } },
        Some(Err(error)) => {
            return rsx! {
                BackLink { to: Route::AdminInvestigations {}, label: "All investigations" }
                ErrorAlert { error: error.clone(), on_retry: move |_| detail.restart() }
            };
        }
        Some(Ok(d)) => d.clone(),
    };
    let inv = d.investigation.clone();

    rsx! {
        BackLink { to: Route::AdminInvestigations {}, label: "All investigations" }
        div { class: "mb-5 flex flex-wrap items-center gap-2",
            h1 { class: "text-xl font-semibold tracking-tight", "Investigation " Mono { "{inv.id}" } }
            InvestigationStatusBadge { status: inv.status }
            RestateLink { link: RestateUiLink::investigation(&inv.id), class: "text-sm" }
        }
        div { class: "grid items-start gap-5 *:min-w-0 xl:grid-cols-[minmax(0,1fr)_22rem]",
            div { class: "flex flex-col gap-5",
                Panel { title: "Summary",
                    Facts {
                        Fact { label: "Order", OrderRefView { order: inv.order.clone() } }
                        Fact { label: "Opened by", span { class: "inline-flex items-center gap-2", ActorTag { actor: inv.opened_by } "{datetime(inv.created_at)}" } }
                        Fact { label: "Carrier result", {inv.carrier_result.map(carrier_text).unwrap_or("Waiting for the trace")} }
                        Fact { label: "Decision", {inv.decision.map(|x| x.label()).unwrap_or("–")} }
                        Fact { label: "Outcome", {inv.outcome.map(|x| x.label()).unwrap_or("–")} }
                        Fact { label: "Rounds with customer", "{inv.rounds} of 2" }
                        if let Some(closed) = inv.closed_at {
                            Fact { label: "Closed", "{datetime(closed)}" }
                        }
                        if let Some(note) = d.note.clone() {
                            Fact { label: "Note", "{note}" }
                        }
                        if let Some(refund) = d.refund_id.clone() {
                            Fact { label: "Refund", Link { to: Route::AdminRefund { id: refund.to_string() }, class: "link link-hover", Mono { "{refund}" } } }
                        }
                        if let Some(replacement) = d.replacement.clone() {
                            Fact { label: "Replacement", OrderRefView { order: replacement } }
                        }
                    }
                }
                // Simulation truth, styled as a demo control (design.md, principle 5).
                if let Some(status) = d.trace_status {
                    DemoFrame {
                        DemoSection { title: "Hidden truth", icon: IconName::EyeOff,
                            p { "Carrier trace: {status.label().to_lowercase()}." }
                        }
                    }
                }
                Panel { title: "Carrier responses", subtitle: "Internal trace events, oldest first.",
                    if d.carrier_responses.is_empty() {
                        Muted { "No response from the carrier yet." }
                    }
                    ol { class: "flex flex-col gap-2 border-l border-base-300 pl-3",
                        for event in d.carrier_responses.iter().cloned() {
                            li { key: "{event.seq}", class: "flex flex-col",
                                span { class: "font-medium", "{event.kind.label()}" }
                                span { "{event.description}" }
                                span { class: "text-xs text-base-content/60",
                                    "{datetime(event.sim_at)}"
                                    if let Some(location) = event.location.clone() { " · {location}" }
                                }
                            }
                        }
                    }
                }
                ShipmentCard { shipment: d.shipment.clone(), status: inv.order.status }
            }
            div { class: "flex flex-col gap-5",
                if let Some(ticket) = inv.ticket.clone() {
                    Panel { title: "Ticket", TicketRefView { ticket } }
                }
                Panel { title: "Human tasks",
                    if d.tasks.is_empty() { Muted { "None." } }
                    for task in d.tasks.iter().cloned() {
                        TaskRow { key: "{task.id}", task }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Refunds
// ---------------------------------------------------------------------------------------------

#[component]
pub fn AdminRefunds() -> Element {
    let api = use_api();
    let status = use_signal(String::new);
    let order = use_signal(String::new);
    let refunds = use_paged(move |cursor| async move {
        let query = RefundListQuery {
            status: parsed::<RefundStatus>(&status()),
            order: non_blank(&order()).and_then(|q| parse_order_query(&q)),
            cursor,
            limit: Some(50),
        };
        api.query::<ListRefunds>(&[], &query).await
    });

    rsx! {
        PageHeader { title: "Refunds", subtitle: "Every refund: requested, approved or rejected by a person, and completed." }
        FilterBar {
            SelectFilter { label: "Status", value: status, options: options(RefundStatus::ALL, RefundStatus::as_str, RefundStatus::label) }
            SearchFilter { label: "Order", placeholder: "TD-1042", value: order }
        }
        {refunds.render(|items| rsx! {
            if items.is_empty() {
                EmptyState { message: "No refunds yet.", icon: IconName::Scale }
            } else {
                TableWrap {
                    table { class: "table table-sm",
                        thead {
                            tr {
                                th { "Refund" }
                                th { "Order" }
                                th { "Customer" }
                                th { "Reason" }
                                th { "Approval" }
                                th { "Status" }
                                th { class: "text-right", "Amount" }
                            }
                        }
                        tbody {
                            for refund in items {
                                tr { key: "{refund.id}", class: "hover:bg-base-200",
                                    td {
                                        Link { to: Route::AdminRefund { id: refund.id.to_string() }, class: "link link-hover flex flex-col",
                                            Mono { "{refund.id}" }
                                            span { class: "text-xs text-base-content/60", "{date(refund.created_at)}" }
                                        }
                                    }
                                    td { OrderLink { number: refund.order.number } }
                                    td { class: "max-w-48", CustomerCell { customer: refund.customer.clone() } }
                                    td { class: "max-w-56 truncate", title: refund.reason_text.clone().unwrap_or_default(), "{refund.reason.label()}" }
                                    td { class: "text-base-content/70", {approval_label(refund.pre_approved, refund.status)} }
                                    td { RefundStatusBadge { status: refund.status } }
                                    td { class: "text-right", MoneyText { amount: refund.amount } }
                                }
                            }
                        }
                    }
                }
            }
        })}
    }
}

#[component]
pub fn AdminRefund(id: String) -> Element {
    let api = use_api();
    let mut detail = use_resource(use_reactive!(|(id,)| async move {
        api.get::<GetRefund>(&[id.as_str()]).await
    }));
    let d = match &*detail.read() {
        None => return rsx! { PageSkeleton { rows: 3 } },
        Some(Err(error)) => {
            return rsx! {
                BackLink { to: Route::AdminRefunds {}, label: "All refunds" }
                ErrorAlert { error: error.clone(), on_retry: move |_| detail.restart() }
            };
        }
        Some(Ok(d)) => d.clone(),
    };
    let r = d.refund.clone();

    rsx! {
        BackLink { to: Route::AdminRefunds {}, label: "All refunds" }
        div { class: "mb-5 flex flex-wrap items-center gap-2",
            h1 { class: "text-xl font-semibold tracking-tight", "Refund of " MoneyText { amount: r.amount } }
            RefundStatusBadge { status: r.status }
            Mono { class: "text-base-content/60", "{r.id}" }
            RestateLink { link: RestateUiLink::refund(&r.id), class: "text-sm" }
        }
        div { class: "grid items-start gap-5 *:min-w-0 xl:grid-cols-[minmax(0,1fr)_22rem]",
            div { class: "flex flex-col gap-5",
                Panel { title: "Summary",
                    Facts {
                        Fact { label: "Order", OrderRefView { order: r.order.clone() } }
                        Fact { label: "Customer", CustomerCell { customer: r.customer.clone() } }
                        Fact { label: "Reason", "{r.reason.label()}" }
                        if let Some(text) = r.reason_text.clone() {
                            Fact { label: "Details", "{text}" }
                        }
                        Fact { label: "Approval",
                            if r.pre_approved { "Pre-approved (a person initiated or offered it)" } else { "Needed a person's approval" }
                        }
                        if let Some(by) = d.decided_by.clone() {
                            Fact { label: "Decided by",
                                Mono { "{by}" }
                                if let Some(at) = d.decided_at { " · {datetime(at)}" }
                            }
                        }
                        if let Some(reason) = d.rejection_reason.clone() {
                            Fact { label: "Rejected because", "{reason}" }
                        }
                        Fact { label: "Requested", "{datetime(r.created_at)}" }
                        if let Some(at) = r.completed_at {
                            Fact { label: "Completed", "{datetime(at)}" }
                        }
                    }
                }
                Panel { title: "Money",
                    div { class: "flex flex-col gap-2",
                        PaymentLine { label: "Charge reversed", payment: d.charge.clone() }
                        PaymentLine { label: "Refund entry", payment: d.refund_payment.clone() }
                        div { class: "flex items-center justify-between gap-2",
                            span { class: "text-base-content/60", "Credit note" }
                            match d.credit_note.clone() {
                                Some(note) => rsx! { Link { to: Route::AdminInvoices {}, class: "link link-hover", Mono { "{note.number}" } } },
                                None => rsx! { span { class: "text-base-content/60",
                                    if r.reason.credits_invoice() { "Not issued yet" } else { "None (a duplicate charge was never invoiced)" }
                                } },
                            }
                        }
                    }
                }
                Panel { title: "Evidence",
                    EvidenceList { evidence: d.evidence.clone() }
                }
            }
            div { class: "flex flex-col gap-5",
                Panel { title: "Links",
                    Facts {
                        Fact { label: "Ticket",
                            match d.ticket_id.clone() {
                                Some(t) => rsx! { Link { to: Route::admin_ticket(&t), class: "link link-hover", Mono { "{t}" } } },
                                None => rsx! { "–" },
                            }
                        }
                        Fact { label: "Investigation",
                            match d.investigation_id.clone() {
                                Some(i) => rsx! { Link { to: Route::AdminInvestigation { id: i.to_string() }, class: "link link-hover", Mono { "{i}" } } },
                                None => rsx! { "–" },
                            }
                        }
                        Fact { label: "Approval task",
                            match d.human_task_id.clone() {
                                Some(t) => rsx! { Link { to: Route::AdminTask { id: t.to_string() }, class: "link link-hover", Mono { "{t}" } } },
                                None => rsx! { "–" },
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PaymentLine(label: &'static str, payment: Option<PaymentEntry>) -> Element {
    rsx! {
        div { class: "flex flex-wrap items-center justify-between gap-2",
            span { class: "text-base-content/60", "{label}" }
            match payment {
                Some(p) => rsx! {
                    span { class: "inline-flex flex-wrap items-center gap-2",
                        Mono { "{p.id}" }
                        PaymentBadge { kind: p.kind, status: p.status }
                        MoneyText { amount: p.amount }
                        span { class: "text-xs text-base-content/60", "{datetime(p.sim_at)}" }
                    }
                },
                None => rsx! { span { class: "text-base-content/60", "Not yet" } },
            }
        }
    }
}

/// Who approved a refund, as far as its state tells.
const fn approval_label(pre_approved: bool, status: RefundStatus) -> &'static str {
    match (pre_approved, status) {
        (true, _) => "Pre-approved",
        (false, RefundStatus::Requested) => "Awaiting a person",
        (false, RefundStatus::Rejected) => "Rejected by a person",
        (false, _) => "Approved by a person",
    }
}
