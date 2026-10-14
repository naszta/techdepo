//! The right side of the inbox: one task's header, kind-specific context, evidence, and the
//! decision form (or the recorded decision).

use dioxus::prelude::*;
use techdepo_api_types::LostOutcome;
use techdepo_api_types::endpoints::GetTask;
use techdepo_api_types::restate_ui::RestateUiLink;
use techdepo_api_types::tasks::{HumanTaskDetail, RecordedDecision, TaskContext};
use techdepo_api_types::vocab::HumanTaskStatus;

use super::decision::{DecisionForm, decision_label};
use crate::api::use_api;
use crate::pages::Panel;
use crate::pages::admin::common::{
    CustomerCell, EvidenceList, Fact, Facts, Muted, OrderRefView, TicketRefView,
};
use crate::routes::Route;
use crate::session::use_session;
use crate::ui::explain::{ClassificationView, ConfidenceBreakdown};
use crate::ui::format::{ago, datetime, relative};
use crate::ui::{
    ActorTag, ErrorAlert, Icon, IconName, MoneyText, Mono, PageSkeleton, RestateLink, Tag,
    TaskKindBadge, TaskStatusBadge,
};

#[component]
pub fn TaskDetail(id: String, on_changed: EventHandler<()>) -> Element {
    let api = use_api();
    let org = use_session().me().map(|me| me.org.id);
    let mut task = use_resource(use_reactive!(|(id,)| async move {
        api.get::<GetTask>(&[id.as_str()]).await
    }));
    let detail = match &*task.read() {
        None => return rsx! { PageSkeleton { rows: 3 } },
        Some(Err(error)) => {
            return rsx! { ErrorAlert { error: error.clone(), on_retry: move |_| task.restart() } };
        }
        Some(Ok(detail)) => detail.clone(),
    };
    let summary = detail.task.clone();
    // The workflow or ticket waiting for the decision.
    let owner = org
        .as_ref()
        .and_then(|org| RestateUiLink::task_owner(org, &detail));
    let refresh = move |_| {
        task.restart();
        on_changed.call(());
    };

    rsx! {
        div { class: "flex flex-col gap-4",
            section { class: "rounded-box border border-base-300 bg-base-100 p-5",
                div { class: "flex flex-wrap items-center gap-2",
                    TaskKindBadge { kind: summary.kind }
                    TaskStatusBadge { status: summary.status, overdue: summary.overdue }
                    Mono { class: "text-xs text-base-content/50", "{summary.id}" }
                }
                h2 { class: "mt-2 text-lg font-semibold tracking-tight", "{summary.title}" }
                p { class: "mt-1 text-base-content/70", "{summary.summary}" }
                p { class: "mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-base-content/60",
                    span { title: "{datetime(summary.created_at)}", "Opened {ago(summary.created_at)}" }
                    if summary.status == HumanTaskStatus::Pending {
                        span {
                            class: "inline-flex items-center gap-1",
                            class: if summary.overdue { "font-medium text-warning" },
                            Icon { name: IconName::Hourglass, class: "size-3" }
                            if summary.overdue { "SLA passed {relative(summary.sla_due_at)}" } else { "SLA {relative(summary.sla_due_at)}" }
                        }
                    }
                    if let Some(link) = owner {
                        RestateLink { link }
                    }
                }
                if summary.order.is_some() || summary.ticket.is_some() || summary.customer.is_some() {
                    div { class: "mt-4 grid gap-3 border-t border-base-300 pt-4 sm:grid-cols-3",
                        if let Some(order) = summary.order.clone() {
                            div { class: "flex flex-col gap-1",
                                span { class: "text-xs text-base-content/60", "Order" }
                                OrderRefView { order }
                            }
                        }
                        if let Some(customer) = summary.customer.clone() {
                            div { class: "flex flex-col gap-1",
                                span { class: "text-xs text-base-content/60", "Customer" }
                                CustomerCell { customer }
                            }
                        }
                        if let Some(ticket) = summary.ticket.clone() {
                            div { class: "flex flex-col gap-1",
                                span { class: "text-xs text-base-content/60", "Ticket" }
                                TicketRefView { ticket }
                            }
                        }
                    }
                }
            }

            ContextPanel { detail: detail.clone() }

            Panel { title: "Evidence", subtitle: "Gathered by code, not by the AI.",
                EvidenceList { evidence: detail.evidence.clone() }
            }

            match (summary.status, detail.decision.clone()) {
                (HumanTaskStatus::Pending, _) => rsx! {
                    DecisionForm { detail: detail.clone(), on_decided: refresh }
                },
                (_, Some(decision)) => rsx! { DecisionRecord { decision } },
                _ => rsx! {
                    Panel { title: "Decision",
                        Muted { "Cancelled: someone acted on the ticket directly, which answered this task." }
                    }
                },
            }
        }
    }
}

/// What this kind of task is about.
#[component]
fn ContextPanel(detail: HumanTaskDetail) -> Element {
    let ticket_id = detail
        .ticket_id
        .clone()
        .or(detail.task.ticket.clone().map(|t| t.id));
    match detail.context {
        TaskContext::ApproveRefund {
            refund_id,
            amount,
            reason,
            reason_text,
        } => rsx! {
            Panel { title: "Refund request",
                Facts {
                    Fact { label: "Amount", MoneyText { amount, class: "text-base font-semibold" } }
                    Fact { label: "Reason", "{reason.label()}" }
                    if let Some(text) = reason_text {
                        Fact { label: "Details", "{text}" }
                    }
                    Fact { label: "Refund",
                        Link { to: Route::AdminRefund { id: refund_id.to_string() }, class: "link link-hover", Mono { "{refund_id}" } }
                    }
                }
                p { class: "text-xs text-base-content/60",
                    "Approval is always a person's call, whatever the AI's confidence. RefundWorkflow waits for this decision."
                }
            }
        },
        TaskContext::InvestigationDecision {
            investigation_id,
            tracking_number,
            carrier_result,
            refundable,
        } => rsx! {
            Panel { title: "Lost parcel: what do we offer?",
                Facts {
                    Fact { label: "Investigation",
                        Link { to: Route::AdminInvestigation { id: investigation_id.to_string() }, class: "link link-hover", Mono { "{investigation_id}" } }
                    }
                    Fact { label: "Tracking", Mono { "{tracking_number}" } }
                    Fact { label: "Carrier trace",
                        match carrier_result {
                            LostOutcome::Gone => "No information: the parcel is gone",
                            LostOutcome::Resurfaces => "Found",
                        }
                    }
                    Fact { label: "Refundable", MoneyText { amount: refundable } }
                }
                p { class: "text-xs text-base-content/60", "The Investigation workflow sends your offer and waits for the customer's answer." }
            }
        },
        TaskContext::ReviewReply {
            subject,
            draft,
            confidence,
            ..
        } => rsx! {
            div { class: "grid gap-4 xl:grid-cols-[minmax(0,1fr)_20rem]",
                Panel {
                    title: "AI draft",
                    subtitle: "Not sent. The reply policy held it for review.",
                    actions: rsx! {
                        if let Some(id) = ticket_id.clone() {
                            Link { to: Route::admin_ticket_trace(&id), class: "btn btn-ghost btn-xs",
                                Icon { name: IconName::Activity, class: "size-3.5" }
                                "Trace"
                            }
                        }
                    },
                    div { class: "flex flex-col gap-2 rounded-field border border-dashed border-base-300 bg-base-200/50 p-3",
                        div { class: "flex items-center gap-2 text-xs text-base-content/60",
                            ActorTag { actor: techdepo_api_types::Actor::Ai }
                            span { class: "truncate", "Subject: {subject}" }
                        }
                        p { class: "whitespace-pre-wrap break-words", "{draft}" }
                    }
                }
                Panel { title: "Confidence breakdown",
                    ConfidenceBreakdown { confidence }
                }
            }
        },
        TaskContext::Escalation {
            reason,
            causes,
            classification,
            new_customer_message,
            ..
        } => rsx! {
            Panel {
                title: "Escalated to a person",
                actions: rsx! {
                    if let Some(id) = ticket_id.clone() {
                        Link { to: Route::admin_ticket(&id), class: "btn btn-ghost btn-xs",
                            Icon { name: IconName::Ticket, class: "size-3.5" }
                            "Open ticket"
                        }
                    }
                },
                p { "{reason}" }
                if !causes.is_empty() {
                    div { class: "flex flex-wrap gap-1.5",
                        for cause in causes {
                            Tag { key: "{cause.as_str()}", "{cause.label()}" }
                        }
                    }
                }
                if new_customer_message {
                    div { role: "status", class: "alert alert-info alert-soft py-2",
                        Icon { name: IconName::Mail, class: "size-4" }
                        "The customer wrote again while the ticket was with a person."
                    }
                }
                if let Some(classification) = classification {
                    div { class: "border-t border-base-300 pt-3",
                        p { class: "mb-2 text-xs font-medium uppercase tracking-wider text-base-content/50", "Classification" }
                        ClassificationView { classification }
                    }
                }
            }
        },
        TaskContext::OffersDeclined {
            investigation_id,
            last_reply,
        } => rsx! {
            Panel { title: "The customer declined every offer",
                Facts {
                    Fact { label: "Investigation",
                        Link { to: Route::AdminInvestigation { id: investigation_id.to_string() }, class: "link link-hover", Mono { "{investigation_id}" } }
                    }
                }
                blockquote { class: "whitespace-pre-wrap border-l-2 border-base-300 pl-3 italic text-base-content/80", "{last_reply}" }
            }
        },
    }
}

#[component]
fn DecisionRecord(decision: RecordedDecision) -> Element {
    let body = decision.decision.body().map(ToOwned::to_owned);
    rsx! {
        Panel { title: "Decision",
            div { class: "flex items-center gap-2",
                Icon { name: IconName::CircleCheck, class: "size-4 text-success" }
                span { class: "font-medium", "{decision_label(decision.decision.kind())}" }
            }
            p { class: "text-xs text-base-content/60",
                "By "
                Mono { "{decision.decided_by}" }
                " · {datetime(decision.decided_at)}"
            }
            if let Some(body) = body {
                p { class: "whitespace-pre-wrap rounded-field border border-base-300 bg-base-200/50 p-3", "{body}" }
            }
            if let Some(note) = decision.note {
                p { class: "text-base-content/70", span { class: "font-medium", "Note: " } "{note}" }
            }
        }
    }
}
