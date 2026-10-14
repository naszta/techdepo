//! The decision form: the task's allowed decisions as radio cards, the text a decision sends
//! (a reason, a reply, an edited draft), a note for the record, and "Send decision".
//!
//! Tasks that belong to a ticket can also be answered by taking the ticket over: acting on the
//! ticket cancels its pending ticket tasks (api.md, `ActOnTicket`).

use dioxus::prelude::*;
use techdepo_api_types::admin::{TicketAction, TicketActionRequest};
use techdepo_api_types::endpoints::{ActOnTicket, DecideTask};
use techdepo_api_types::tasks::{DecideTaskRequest, HumanDecision, HumanTaskDetail, TaskContext};
use techdepo_api_types::vocab::DecisionKind;
use techdepo_api_types::{ApiError, ErrorCode, FieldError};

use crate::api::use_api;
use crate::components::toast::{ToastOptions, use_toast};
use crate::pages::Panel;
use crate::pages::admin::common::{HandBackIntent, hand_back_choice};
use crate::routes::Route;
use crate::ui::{Icon, IconName};

/// A decision's name as the button says it.
pub fn decision_label(kind: DecisionKind) -> &'static str {
    match kind {
        DecisionKind::Approve => "Approve refund",
        DecisionKind::Reject => "Reject refund",
        DecisionKind::Discard => "Discard draft and take over",
        other => other.label(),
    }
}

/// One line on what the decision does.
fn decision_help(kind: DecisionKind, context: &TaskContext) -> String {
    match (kind, context) {
        (DecisionKind::Approve, TaskContext::ApproveRefund { amount, .. }) => {
            format!("Refund {amount} now. RefundWorkflow executes it and emails the customer.")
        }
        (DecisionKind::Reject, _) => {
            "Keep the charge. Your reason goes into the email to the customer.".to_owned()
        }
        (DecisionKind::OfferRefund, TaskContext::InvestigationDecision { refundable, .. }) => {
            format!("Offer a refund of {refundable}.")
        }
        (DecisionKind::OfferRefund, _) => "Offer a refund.".to_owned(),
        (DecisionKind::OfferReship, _) => "Offer to ship a replacement order.".to_owned(),
        (DecisionKind::OfferBoth, _) => {
            "Let the customer choose a refund or a replacement.".to_owned()
        }
        (DecisionKind::CustomReply, _) => "Write your own reply instead of an offer.".to_owned(),
        (DecisionKind::Send, _) => "Send the AI draft as it is.".to_owned(),
        (DecisionKind::EditAndSend, _) => "Edit the draft, then send your version.".to_owned(),
        (DecisionKind::Discard, _) => "Drop the draft; the ticket comes to you.".to_owned(),
        (DecisionKind::Reply, _) => "Reply to the customer yourself.".to_owned(),
        (DecisionKind::HandBack, _) => {
            "The AI answers what is still unanswered, as what you pick below.".to_owned()
        }
        (DecisionKind::Close, _) => "Resolve the ticket without replying.".to_owned(),
        (DecisionKind::Approve, _) => "Approve.".to_owned(),
    }
}

/// The label of the text a decision sends, if it sends one.
const fn body_label(kind: DecisionKind) -> Option<&'static str> {
    match kind {
        DecisionKind::Reject => Some("Reason, for the customer"),
        DecisionKind::CustomReply | DecisionKind::Reply => Some("Your reply"),
        DecisionKind::EditAndSend => Some("Edited reply"),
        _ => None,
    }
}

fn build(kind: DecisionKind, body: String, close: bool, intent: &str) -> HumanDecision {
    match kind {
        DecisionKind::Approve => HumanDecision::Approve,
        DecisionKind::Reject => HumanDecision::Reject { reason: body },
        DecisionKind::OfferRefund => HumanDecision::OfferRefund,
        DecisionKind::OfferReship => HumanDecision::OfferReship,
        DecisionKind::OfferBoth => HumanDecision::OfferBoth,
        DecisionKind::CustomReply => HumanDecision::CustomReply { body },
        DecisionKind::Send => HumanDecision::Send,
        DecisionKind::EditAndSend => HumanDecision::EditAndSend { body },
        DecisionKind::Discard => HumanDecision::Discard,
        DecisionKind::Reply => HumanDecision::Reply { body, close },
        DecisionKind::HandBack => HumanDecision::HandBack {
            intent: hand_back_choice(intent),
        },
        DecisionKind::Close => HumanDecision::Close,
    }
}

fn message_for(errors: &[FieldError], fields: &[&str]) -> Option<String> {
    errors
        .iter()
        .find(|e| fields.contains(&e.field.as_str()))
        .map(|e| e.message.clone())
}

#[component]
pub fn DecisionForm(detail: HumanTaskDetail, on_decided: EventHandler<()>) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let nav = navigator();
    let mut choice = use_signal(|| None::<DecisionKind>);
    let mut body = use_signal(String::new);
    let mut close = use_signal(|| false);
    let hand_back_as = use_signal(String::new);
    let mut note = use_signal(String::new);
    let mut errors = use_signal(Vec::<FieldError>::new);
    let mut pending = use_signal(|| None::<&'static str>);

    let task_id = detail.task.id.to_string();
    let kind = detail.task.kind;
    let draft = match &detail.context {
        TaskContext::ReviewReply { draft, .. } => Some(draft.clone()),
        _ => None,
    };
    let ticket_id = detail
        .ticket_id
        .clone()
        .or(detail.task.ticket.clone().map(|t| t.id))
        .map(|id| id.to_string());
    let can_take_over = ticket_id.is_some() && !kind.has_waiter();
    let allowed = detail.allowed_decisions.clone();
    let context = detail.context.clone();

    let submit = {
        let task_id = task_id.clone();
        move |_| {
            let Some(selected) = choice() else { return };
            let request = DecideTaskRequest {
                decision: build(selected, body().trim().to_owned(), close(), &hand_back_as()),
                note: Some(note().trim().to_owned()).filter(|n| !n.is_empty()),
            };
            if let Err(error) = request.validate_for(kind) {
                errors.set(error.fields);
                return;
            }
            errors.set(Vec::new());
            let task_id = task_id.clone();
            spawn(async move {
                pending.set(Some("decide"));
                match api.send::<DecideTask>(&[task_id.as_str()], &request).await {
                    Ok(_) => {
                        toasts.success(
                            format!("Decided: {}", decision_label(selected)),
                            ToastOptions::new(),
                        );
                        on_decided.call(());
                    }
                    Err(error) => {
                        report(toasts, &error);
                        errors.set(error.fields.clone());
                        if error.code == ErrorCode::AlreadyDecided {
                            on_decided.call(());
                        }
                    }
                }
                pending.set(None);
            });
        }
    };

    let take_over = {
        let ticket_id = ticket_id.clone();
        move |_| {
            let Some(ticket_id) = ticket_id.clone() else {
                return;
            };
            spawn(async move {
                pending.set(Some("take_over"));
                let request = TicketActionRequest {
                    action: TicketAction::TakeOver,
                };
                match api
                    .send::<ActOnTicket>(&[ticket_id.as_str()], &request)
                    .await
                {
                    Ok(_) => {
                        toasts.success(
                            format!("You have ticket {ticket_id} now"),
                            ToastOptions::new(),
                        );
                        on_decided.call(());
                        nav.push(Route::admin_ticket(&ticket_id));
                    }
                    Err(error) => report(toasts, &error),
                }
                pending.set(None);
            });
        }
    };

    let body_error = message_for(&errors(), &["decision.body", "decision.reason"]);
    let note_error = message_for(&errors(), &["note"]);
    let other_errors: Vec<String> = errors()
        .iter()
        .filter(|e| !["decision.body", "decision.reason", "note"].contains(&e.field.as_str()))
        .map(|e| e.message.clone())
        .collect();

    rsx! {
        Panel { title: "Your decision",
            fieldset { class: "flex flex-col gap-2", disabled: pending().is_some(),
                legend { class: "sr-only", "Decision" }
                div { class: "grid gap-2 sm:grid-cols-2",
                    for decision in allowed {
                        label {
                            key: "{decision.as_str()}",
                            class: "flex cursor-pointer items-start gap-3 rounded-box border p-3 transition-colors motion-reduce:transition-none",
                            class: if choice() == Some(decision) { "border-primary bg-primary/5" } else { "border-base-300 hover:bg-base-200" },
                            input {
                                r#type: "radio",
                                name: "decision",
                                class: "radio radio-sm radio-primary mt-0.5",
                                checked: choice() == Some(decision),
                                onchange: {
                                    let draft = draft.clone();
                                    move |_| {
                                        choice.set(Some(decision));
                                        errors.set(Vec::new());
                                        if decision == DecisionKind::EditAndSend && body.peek().is_empty() {
                                            body.set(draft.clone().unwrap_or_default());
                                        }
                                    }
                                },
                            }
                            span { class: "flex flex-col gap-0.5",
                                span { class: "font-medium", "{decision_label(decision)}" }
                                span { class: "text-xs text-base-content/60", {decision_help(decision, &context)} }
                            }
                        }
                    }
                }

                if let Some(label) = choice().and_then(body_label) {
                    label { class: "mt-2 flex flex-col gap-1",
                        span { class: "font-medium", "{label}" }
                        textarea {
                            class: "textarea w-full",
                            class: if body_error.is_some() { "textarea-error" },
                            rows: if choice() == Some(DecisionKind::Reject) { "3" } else { "8" },
                            value: "{body}",
                            oninput: move |e| body.set(e.value()),
                        }
                        if let Some(message) = body_error.clone() {
                            span { class: "text-xs text-error", "{message}" }
                        }
                    }
                    if choice() == Some(DecisionKind::Reply) {
                        label { class: "flex cursor-pointer items-center gap-2",
                            input { r#type: "checkbox", class: "checkbox checkbox-sm", checked: close(), onchange: move |e| close.set(e.checked()) }
                            "Resolve the ticket after replying"
                        }
                    }
                }

                if choice() == Some(DecisionKind::HandBack) {
                    label { class: "mt-2 flex flex-col gap-1",
                        span { class: "font-medium", "What does the customer want?" }
                        HandBackIntent { value: hand_back_as, class: "w-full sm:w-72" }
                        span { class: "text-xs text-base-content/60",
                            "Picking it skips the classifier, so a low-confidence ticket does not come straight back."
                        }
                    }
                }

                label { class: "mt-2 flex flex-col gap-1",
                    span { class: "font-medium", "Note "
                        span { class: "font-normal text-base-content/60", "(optional, for the record)" }
                    }
                    textarea {
                        class: "textarea w-full",
                        class: if note_error.is_some() { "textarea-error" },
                        rows: "2",
                        value: "{note}",
                        oninput: move |e| note.set(e.value()),
                    }
                    if let Some(message) = note_error.clone() {
                        span { class: "text-xs text-error", "{message}" }
                    }
                }

                for (i , message) in other_errors.into_iter().enumerate() {
                    p { key: "{i}", class: "text-xs text-error", "{message}" }
                }

                div { class: "mt-2 flex flex-wrap items-center gap-2",
                    button {
                        r#type: "button",
                        class: "btn btn-primary btn-sm",
                        disabled: choice().is_none() || pending().is_some(),
                        onclick: submit,
                        if pending() == Some("decide") {
                            span { class: "loading loading-spinner loading-xs" }
                        } else {
                            Icon { name: IconName::Send, class: "size-3.5" }
                        }
                        "Send decision"
                    }
                    if can_take_over {
                        button {
                            r#type: "button",
                            class: "btn btn-outline btn-sm",
                            disabled: pending().is_some(),
                            onclick: take_over,
                            if pending() == Some("take_over") {
                                span { class: "loading loading-spinner loading-xs" }
                            } else {
                                Icon { name: IconName::Hand, class: "size-3.5" }
                            }
                            "Take over ticket"
                        }
                        span { class: "text-xs text-base-content/60", "Taking over answers this task; reply from the ticket." }
                    }
                }
            }
        }
    }
}

fn report(toasts: crate::components::toast::Toasts, error: &ApiError) {
    toasts.error(error.message.clone(), ToastOptions::new());
}
