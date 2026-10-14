//! One ticket for a person (design.md "Ticket (admin)"): the thread including AI drafts, a side
//! panel with the classification probabilities, the route taken and the reply confidence,
//! **Take over** / **Hand back** / **Close**, a manual reply, and a "Trace" tab with every
//! agent step.
//!
//! The pipeline runs asynchronously, so while the ticket is being handled the page re-reads
//! it every few seconds (a bounded burst), and again after every action.

use dioxus::prelude::*;
use techdepo_api_types::admin::{AdminTicketDetail, TicketAction, TicketActionRequest};
use techdepo_api_types::endpoints::{ActOnTicket, GetAdminTicket};
use techdepo_api_types::restate_ui::RestateUiLink;
use techdepo_api_types::support::TicketMessage;
use techdepo_api_types::vocab::{Direction, MessageStatus};
use techdepo_api_types::{Assignee, FieldError, TicketStatus, Validate};

use super::trace::TicketTraceTab;
use crate::api::use_api;
use crate::browser;
use crate::components::chat::{
    Chat, ChatBubble, ChatBubbleColor, ChatFooter, ChatHeader, ChatPlacement,
};
use crate::components::toast::{ToastOptions, use_toast};
use crate::pages::Panel;
use crate::pages::admin::common::{
    BackLink, CustomerCell, Fact, Facts, HandBackIntent, Muted, OrderRefView, TextField, error_for,
    hand_back_choice,
};
use crate::pages::admin::tasks::TaskRow;
use crate::routes::Route;
use crate::session::use_session;
use crate::ui::explain::{ClassificationView, prob};
use crate::ui::format::{date, datetime};
use crate::ui::{
    ActorTag, AssigneeTag, ErrorAlert, Icon, IconName, InvestigationStatusBadge,
    MessageStatusBadge, MoneyText, Mono, PageSkeleton, RefundStatusBadge, RestateLink,
    TicketStatusBadge,
};

const POLL_MS: u32 = 3_000;
const POLL_BUDGET: u32 = 20;
const FOLLOW_UP_MS: [u32; 2] = [1_500, 4_000];

/// A route or workflow is working on it. `New` is not: a stored ticket is only `New` after a
/// hand-back with nothing left to answer, waiting for the customer's next message.
const fn in_progress(status: TicketStatus) -> bool {
    matches!(status, TicketStatus::Routing | TicketStatus::WithRoute)
}

#[component]
pub fn AdminTicket(id: String, tab: String) -> Element {
    let api = use_api();
    let org = use_session().me().map(|me| me.org.id);
    let id_for_fetch = id.clone();
    let mut ticket = use_resource(use_reactive!(|(id_for_fetch,)| async move {
        api.get::<GetAdminTicket>(&[id_for_fetch.as_str()]).await
    }));
    let mut budget = use_signal(|| POLL_BUDGET);
    use_future(move || async move {
        loop {
            browser::sleep(POLL_MS).await;
            let busy =
                matches!(&*ticket.peek(), Some(Ok(t)) if in_progress(t.ticket.ticket.status));
            if busy && *budget.peek() > 0 {
                budget -= 1;
                ticket.restart();
            }
        }
    });
    let refresh = use_callback(move |_: ()| {
        budget.set(POLL_BUDGET);
        ticket.restart();
        spawn(async move {
            for ms in FOLLOW_UP_MS {
                browser::sleep(ms).await;
                ticket.restart();
            }
        });
    });

    let detail = match &*ticket.read() {
        None => return rsx! { PageSkeleton { rows: 3 } },
        Some(Err(error)) => {
            return rsx! {
                BackLink { to: Route::AdminTickets {}, label: "All tickets" }
                ErrorAlert { error: error.clone(), on_retry: move |_| ticket.restart() }
            };
        }
        Some(Ok(detail)) => detail.clone(),
    };
    let header = detail.ticket.clone();
    let summary = header.ticket.clone();
    let on_trace = tab == "trace";

    rsx! {
        BackLink { to: Route::AdminTickets {}, label: "All tickets" }
        div { class: "mb-4 flex flex-col gap-1",
            div { class: "flex flex-wrap items-center gap-2",
                h1 { class: "text-xl font-semibold tracking-tight", "{summary.subject}" }
                TicketStatusBadge { status: summary.status }
                AssigneeTag { assignee: header.assignee }
                if in_progress(summary.status) {
                    span { class: "inline-flex items-center gap-1 text-xs text-base-content/60",
                        span { class: "loading loading-dots loading-xs" }
                        "Being handled"
                    }
                }
            }
            p { class: "flex flex-wrap items-center gap-x-2 text-base-content/60",
                Mono { "{summary.id}" }
                " · {summary.channel.label()} · opened {date(summary.created_at)}"
                if let Some(org) = org.as_ref() {
                    RestateLink { link: RestateUiLink::ticket(org, &summary.id), class: "text-sm" }
                }
            }
        }

        div { role: "tablist", class: "tabs tabs-border mb-4",
            Link {
                to: Route::admin_ticket(&summary.id),
                role: "tab",
                class: if on_trace { "tab" } else { "tab tab-active" },
                aria_selected: if on_trace { "false" } else { "true" },
                "Conversation"
            }
            Link {
                to: Route::admin_ticket_trace(&summary.id),
                role: "tab",
                class: if on_trace { "tab tab-active" } else { "tab" },
                aria_selected: if on_trace { "true" } else { "false" },
                "Trace"
                if !detail.runs.is_empty() {
                    span { class: "badge badge-ghost badge-sm ml-1.5", "{detail.runs.len()}" }
                }
            }
        }

        if on_trace {
            TicketTraceTab { id: id.clone(), runs: detail.runs.len() }
        } else {
            div { class: "grid items-start gap-5 *:min-w-0 xl:grid-cols-[minmax(0,1fr)_22rem]",
                div { class: "flex flex-col gap-5",
                    AdminThread {
                        messages: detail.messages.clone(),
                        working: in_progress(summary.status),
                        parked: summary.status == TicketStatus::New && header.assignee == Assignee::Ai,
                    }
                    ReplyComposer { id: id.clone(), assignee: header.assignee, on_done: move |_| refresh.call(()) }
                }
                SidePanel { detail: detail.clone(), on_changed: move |_| refresh.call(()) }
            }
        }
    }
}

#[component]
fn AdminThread(messages: Vec<TicketMessage>, working: bool, parked: bool) -> Element {
    rsx! {
        div { class: "flex flex-col gap-1 rounded-box border border-base-300 bg-base-100 p-4",
            if messages.is_empty() {
                Muted { "No messages yet." }
            }
            for message in messages {
                AdminMessage { key: "{message.seq}", message }
            }
            if working {
                Chat { placement: ChatPlacement::End,
                    ChatBubble { class: "flex items-center gap-2 text-base-content/70",
                        span { class: "loading loading-dots loading-sm" }
                        "The pipeline is working on it…"
                    }
                }
            }
            if parked {
                Muted { "Handed back to the AI: it picks up the customer's next message." }
            }
        }
    }
}

/// The customer on the left, TechDepó on the right (this is the shop's side of the thread).
/// Drafts that never reached the customer are dashed and labelled.
#[component]
fn AdminMessage(message: TicketMessage) -> Element {
    let customer = message.direction == Direction::Inbound;
    let unsent = !message.status.is_customer_visible();
    let at = message.sent_at.unwrap_or(message.created_at);
    rsx! {
        Chat { placement: if customer { ChatPlacement::Start } else { ChatPlacement::End },
            ChatHeader { class: "mb-1 flex flex-wrap items-center gap-2 text-xs text-base-content/60",
                ActorTag { actor: message.author }
                if unsent { MessageStatusBadge { status: message.status } }
                time { "{datetime(at)}" }
                span { "#{message.seq}" }
            }
            ChatBubble {
                color: if customer || unsent { ChatBubbleColor::Default } else { ChatBubbleColor::Primary },
                class: match (unsent, message.status == MessageStatus::Discarded) {
                    (_, true) => "max-w-[85%] whitespace-pre-wrap break-words border border-dashed border-base-content/30 bg-base-100 text-base-content line-through opacity-60",
                    (true, false) => "max-w-[85%] whitespace-pre-wrap break-words border border-dashed border-base-content/30 bg-base-100 text-base-content",
                    (false, false) => "max-w-[85%] whitespace-pre-wrap break-words",
                },
                "{message.body_text}"
            }
            ChatFooter { class: "mt-1 flex items-center gap-1 text-xs text-base-content/50",
                Icon { name: if message.channel == techdepo_api_types::Channel::Email { IconName::Mail } else { IconName::Send }, class: "size-3" }
                "{message.channel.label()} · {message.subject}"
            }
        }
    }
}

#[component]
fn ReplyComposer(id: String, assignee: Assignee, on_done: EventHandler<()>) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let mut body = use_signal(String::new);
    let mut close = use_signal(|| false);
    let mut errors = use_signal(Vec::<FieldError>::new);
    let mut pending = use_signal(|| false);

    let send = move |_| {
        let request = TicketActionRequest {
            action: TicketAction::Reply {
                body: body().trim().to_owned(),
                close: close(),
            },
        };
        if let Err(error) = request.validate() {
            errors.set(error.fields);
            return;
        }
        errors.set(Vec::new());
        let id = id.clone();
        spawn(async move {
            pending.set(true);
            match api.send::<ActOnTicket>(&[id.as_str()], &request).await {
                Ok(_) => {
                    toasts.success("Reply sent".to_owned(), ToastOptions::new());
                    body.set(String::new());
                    on_done.call(());
                }
                Err(error) => {
                    errors.set(error.fields.clone());
                    toasts.error(error.message, ToastOptions::new());
                }
            }
            pending.set(false);
        });
    };

    rsx! {
        Panel {
            title: "Reply",
            subtitle: if assignee == Assignee::Ai { "Replying takes the ticket over from the AI." } else { "Sent by email from the support address." },
            TextField { label: "Message", value: body, multiline: true, rows: 5, error: error_for(&errors(), "action.body") }
            div { class: "flex flex-wrap items-center justify-between gap-2",
                label { class: "flex cursor-pointer items-center gap-2",
                    input { r#type: "checkbox", class: "checkbox checkbox-sm", checked: close(), onchange: move |e| close.set(e.checked()) }
                    "Resolve after sending"
                }
                button { class: "btn btn-primary btn-sm", disabled: pending() || body().trim().is_empty(), onclick: send,
                    if pending() { span { class: "loading loading-spinner loading-xs" } } else { Icon { name: IconName::Send, class: "size-3.5" } }
                    "Send reply"
                }
            }
        }
    }
}

#[component]
fn SidePanel(detail: AdminTicketDetail, on_changed: EventHandler<()>) -> Element {
    let header = detail.ticket.clone();
    rsx! {
        div { class: "flex flex-col gap-5 xl:sticky xl:top-20",
            ActionsCard { id: header.ticket.id.to_string(), assignee: header.assignee, status: header.ticket.status, on_changed }
            Panel { title: "About",
                Facts {
                    Fact { label: "Customer", CustomerCell { customer: header.customer.clone() } }
                    Fact { label: "Order",
                        match header.ticket.order.clone() {
                            Some(order) => rsx! { OrderRefView { order } },
                            None => rsx! { span { class: "text-base-content/60", "Not known yet" } },
                        }
                    }
                    Fact { label: "Route",
                        match header.route {
                            Some(route) => rsx! { span { class: "inline-flex items-center gap-1",
                                Icon { name: if route.is_agent() { IconName::Bot } else { IconName::Route }, class: "size-3.5 text-base-content/60" }
                                "{route.label()}"
                            } },
                            None => rsx! { span { class: "text-base-content/60", "Not routed yet" } },
                        }
                    }
                    if let Some(c) = detail.reply_confidence {
                        Fact { label: "Reply confidence", span { class: "font-mono", "{prob(c)}" } }
                    }
                    if let Some(awaiting) = detail.awaiting {
                        Fact { label: "Waiting for", "{awaiting.label()}" }
                    }
                    if let Some(reason) = detail.escalation_reason.clone() {
                        Fact { label: "Escalated", "{reason}" }
                    }
                    Fact { label: "Reply address", Mono { class: "break-all text-xs", "{detail.reply_address}" } }
                }
            }
            Panel { title: "Classification", subtitle: "The router's probabilities for the latest message.",
                match detail.classification.clone() {
                    Some(classification) => rsx! { ClassificationView { classification } },
                    None => rsx! { Muted { "Not classified yet." } },
                }
            }
            if !detail.tasks.is_empty() {
                Panel { title: "Human tasks",
                    for task in detail.tasks.iter().cloned() {
                        TaskRow { key: "{task.id}", task }
                    }
                }
            }
            if !detail.investigations.is_empty() || !detail.refunds.is_empty() {
                Panel { title: "Cases",
                    for inv in detail.investigations.iter().cloned() {
                        Link {
                            key: "{inv.id}",
                            to: Route::AdminInvestigation { id: inv.id.to_string() },
                            class: "flex items-center justify-between gap-2 rounded-field border border-base-300 p-2 hover:bg-base-200",
                            span { "Investigation " Mono { "{inv.id}" } }
                            InvestigationStatusBadge { status: inv.status }
                        }
                    }
                    for refund in detail.refunds.iter().cloned() {
                        Link {
                            key: "{refund.id}",
                            to: Route::AdminRefund { id: refund.id.to_string() },
                            class: "flex items-center justify-between gap-2 rounded-field border border-base-300 p-2 hover:bg-base-200",
                            span { "Refund " MoneyText { amount: refund.amount } }
                            RefundStatusBadge { status: refund.status }
                        }
                    }
                }
            }
        }
    }
}

/// Take over / hand back / close. Acting on the ticket also answers its pending
/// ticket-owned human tasks.
#[component]
fn ActionsCard(
    id: String,
    assignee: Assignee,
    status: TicketStatus,
    on_changed: EventHandler<()>,
) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let mut pending = use_signal(|| None::<&'static str>);
    let mut confirm_close = use_signal(|| false);
    let hand_back_as = use_signal(String::new);

    let act = move |name: &'static str, action: TicketAction, done: &'static str| {
        let id = id.clone();
        spawn(async move {
            pending.set(Some(name));
            let request = TicketActionRequest { action };
            match api.send::<ActOnTicket>(&[id.as_str()], &request).await {
                Ok(_) => {
                    toasts.success(done.to_owned(), ToastOptions::new());
                    on_changed.call(());
                }
                Err(error) => toasts.error(error.message, ToastOptions::new()),
            }
            pending.set(None);
            confirm_close.set(false);
        });
    };
    let busy = pending().is_some();
    let resolved = status == TicketStatus::Resolved;

    rsx! {
        Panel { title: "Who handles it",
            div { class: "flex items-center gap-2",
                AssigneeTag { assignee }
                span { class: "text-base-content/70",
                    match assignee {
                        Assignee::Ai => "The AI handles new messages automatically.",
                        Assignee::Human => "A person has it; the AI stays out. Handing it back answers what is still unanswered.",
                    }
                }
            }
            div { class: "flex flex-wrap gap-2",
                if assignee == Assignee::Ai {
                    button {
                        class: "btn btn-primary btn-sm",
                        disabled: busy,
                        onclick: {
                            let act = act.clone();
                            move |_| act("take_over", TicketAction::TakeOver, "You have the ticket now")
                        },
                        if pending() == Some("take_over") { span { class: "loading loading-spinner loading-xs" } } else { Icon { name: IconName::Hand, class: "size-3.5" } }
                        "Take over"
                    }
                } else {
                    div { class: "join",
                        HandBackIntent { value: hand_back_as, class: "join-item w-52" }
                        button {
                            class: "btn btn-sm join-item",
                            disabled: busy,
                            onclick: {
                                let act = act.clone();
                                move |_| {
                                    let intent = hand_back_choice(&hand_back_as());
                                    act("hand_back", TicketAction::HandBack { intent }, "Handed back to the AI")
                                }
                            },
                            if pending() == Some("hand_back") { span { class: "loading loading-spinner loading-xs" } } else { Icon { name: IconName::Undo, class: "size-3.5" } }
                            "Hand back to AI"
                        }
                    }
                }
                if !resolved {
                    if confirm_close() {
                        button {
                            class: "btn btn-sm btn-outline",
                            disabled: busy,
                            onclick: move |_| act("close", TicketAction::Close, "Ticket resolved"),
                            if pending() == Some("close") { span { class: "loading loading-spinner loading-xs" } }
                            "Resolve without replying?"
                        }
                        button { class: "btn btn-ghost btn-sm", onclick: move |_| confirm_close.set(false), "Keep open" }
                    } else {
                        button { class: "btn btn-ghost btn-sm", disabled: busy, onclick: move |_| confirm_close.set(true),
                            Icon { name: IconName::CircleCheck, class: "size-3.5" }
                            "Close"
                        }
                    }
                }
            }
        }
    }
}
