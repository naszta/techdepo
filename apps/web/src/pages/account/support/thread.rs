//! One ticket as a conversation: the customer on the right, TechDepó on the left with an
//! AI / Human / System marker, and the reply composer at the bottom.
//!
//! Support runs asynchronously (a web message is a one-way call into `SupportTicket`), so the
//! page polls while the ticket is being handled and right after the customer writes. A ticket
//! created a moment ago may not be readable yet; a `404` is retried briefly before it is shown.

use dioxus::prelude::*;
use techdepo_api_types::endpoints::GetTicket;
use techdepo_api_types::support::{TicketDetail, TicketMessage};
use techdepo_api_types::vocab::Direction;
use techdepo_api_types::{Channel, ErrorCode, TicketStatus};

use super::composer::Composer;
use crate::api::use_api;
use crate::browser;
use crate::components::chat::{
    Chat, ChatBubble, ChatBubbleColor, ChatFooter, ChatHeader, ChatPlacement,
};
use crate::pages::Panel;
use crate::routes::Route;
use crate::ui::format::{date, datetime};
use crate::ui::{ActorTag, ErrorAlert, Icon, IconName, Mono, PageSkeleton, TicketStatusBadge};

/// Poll interval while something is expected to change.
const POLL_MS: u32 = 3_000;
/// Polls per burst (about a minute), so an idle tab stops asking.
const POLL_BUDGET: u32 = 20;
/// `404` retries for a ticket that is still being created.
const NOT_FOUND_RETRIES: u32 = 8;

/// The pipeline is still working on the latest message.
/// A route or workflow is working on it (a stored `New` ticket waits for the customer).
const fn in_progress(status: TicketStatus) -> bool {
    matches!(status, TicketStatus::Routing | TicketStatus::WithRoute)
}

#[component]
pub fn AccountTicket(id: String) -> Element {
    let api = use_api();
    let mut not_found_retries = use_signal(|| 0u32);
    let mut ticket = use_resource(use_reactive!(|(id,)| async move {
        api.get::<GetTicket>(&[id.as_str()]).await
    }));
    // Messages the customer has sent that have not shown up yet (a count to wait for).
    let mut expect_messages = use_signal(|| None::<u32>);
    let mut budget = use_signal(|| POLL_BUDGET);

    // A ticket opened a moment ago may not be readable yet.
    use_effect(move || {
        if let Some(Err(error)) = &*ticket.read()
            && error.code == ErrorCode::NotFound
            && *not_found_retries.peek() < NOT_FOUND_RETRIES
        {
            spawn(async move {
                browser::sleep(1_000).await;
                not_found_retries += 1;
                ticket.restart();
            });
        }
    });

    use_future(move || async move {
        loop {
            browser::sleep(POLL_MS).await;
            let waiting = match &*ticket.peek() {
                Some(Ok(detail)) => {
                    let visible = detail.ticket.message_count;
                    let expected = *expect_messages.peek();
                    if expected.is_some_and(|n| visible >= n) {
                        expect_messages.set(None);
                    }
                    in_progress(detail.ticket.status) || expected.is_some_and(|n| visible < n)
                }
                _ => false,
            };
            if waiting && *budget.peek() > 0 {
                budget -= 1;
                ticket.restart();
            }
        }
    });

    let still_creating = matches!(&*ticket.read(), Some(Err(e)) if e.code == ErrorCode::NotFound)
        && not_found_retries() < NOT_FOUND_RETRIES;
    let detail = match &*ticket.read() {
        None => return rsx! { PageSkeleton { rows: 3 } },
        Some(Err(_)) if still_creating => {
            return rsx! {
                p { class: "mb-4 text-sm text-base-content/70", "Opening your conversation…" }
                PageSkeleton { rows: 3 }
            };
        }
        Some(Err(error)) => {
            return rsx! {
                BackToSupport {}
                ErrorAlert { error: error.clone(), on_retry: move |_| ticket.restart() }
            };
        }
        Some(Ok(detail)) => detail.clone(),
    };
    let summary = detail.ticket.clone();
    let working = in_progress(summary.status) || expect_messages().is_some();
    let reply_subject = if summary.subject.to_lowercase().starts_with("re:") {
        summary.subject.clone()
    } else {
        format!("Re: {}", summary.subject)
    };

    rsx! {
        BackToSupport {}
        div { class: "mb-6 flex flex-col gap-1",
            div { class: "flex flex-wrap items-center gap-2",
                h1 { class: "text-2xl font-semibold tracking-tight", "{summary.subject}" }
                TicketStatusBadge { status: summary.status }
            }
            p { class: "text-sm text-base-content/60",
                Mono { "{summary.id}" }
                " · opened {date(summary.created_at)}"
                if let Some(order) = summary.order.clone() {
                    " · about "
                    Link { to: Route::AccountOrder { number: order.number.to_string() }, class: "link link-hover",
                        Mono { "{order.number}" }
                    }
                }
            }
        }

        div { class: "grid gap-6 lg:grid-cols-[1fr_18rem]",
            div { class: "flex flex-col gap-6",
                Thread { detail: detail.clone(), working }
                Panel { title: "Reply",
                    Composer {
                        ticket: Some(summary.id.clone()),
                        subject: reply_subject.clone(),
                        on_sent: move |_| {
                            expect_messages.set(Some(summary.message_count + 1));
                            budget.set(POLL_BUDGET);
                            ticket.restart();
                        },
                    }
                }
            }
            aside { class: "flex flex-col gap-3 text-sm lg:sticky lg:top-20 lg:self-start",
                div { class: "rounded-box border border-base-300 p-4",
                    p { class: "mb-1 font-medium", "Reply by email" }
                    p { class: "text-base-content/70",
                        "Answers to "
                        a { class: "link link-primary break-all", href: "mailto:{detail.reply_address}?subject={uri_component(&reply_subject)}",
                            Mono { "{detail.reply_address}" }
                        }
                        " continue this conversation."
                    }
                }
                div { class: "rounded-box border border-base-300 p-4 text-base-content/70",
                    p { class: "mb-2 font-medium text-base-content", "Who answers" }
                    ul { class: "flex flex-col gap-2",
                        li { class: "flex items-center gap-2", ActorTag { actor: techdepo_api_types::Actor::Ai } "answers routine questions" }
                        li { class: "flex items-center gap-2", ActorTag { actor: techdepo_api_types::Actor::Human } "steps in when it matters" }
                        li { class: "flex items-center gap-2", ActorTag { actor: techdepo_api_types::Actor::System } "sends updates" }
                    }
                }
            }
        }
    }
}

fn uri_component(text: &str) -> String {
    String::from(js_sys::encode_uri_component(text))
}

#[component]
fn Thread(detail: TicketDetail, working: bool) -> Element {
    rsx! {
        div { class: "flex flex-col gap-1 rounded-box border border-base-300 bg-base-100 p-4",
            if detail.messages.is_empty() {
                p { class: "text-sm text-base-content/70", "No messages yet." }
            }
            for message in detail.messages.iter().cloned() {
                MessageBubble { key: "{message.seq}", message }
            }
            if working {
                Chat { placement: ChatPlacement::Start,
                    ChatHeader { class: "text-xs text-base-content/60", "TechDepó" }
                    ChatBubble { class: "flex items-center gap-2 text-sm text-base-content/70",
                        span { class: "loading loading-dots loading-sm" }
                        "Working on it…"
                    }
                }
            }
        }
    }
}

#[component]
fn MessageBubble(message: TicketMessage) -> Element {
    let customer = message.direction == Direction::Inbound;
    let at = message.sent_at.unwrap_or(message.created_at);
    let via = match message.channel {
        Channel::Email => "via email",
        Channel::Web => "via the web form",
    };
    rsx! {
        Chat { placement: if customer { ChatPlacement::End } else { ChatPlacement::Start },
            ChatHeader { class: "mb-1 flex items-center gap-2 text-xs text-base-content/60",
                if customer {
                    "You"
                } else {
                    "TechDepó"
                    ActorTag { actor: message.author }
                }
                time { "{datetime(at)}" }
            }
            ChatBubble {
                color: if customer { ChatBubbleColor::Primary } else { ChatBubbleColor::Default },
                class: "max-w-[85%] whitespace-pre-wrap break-words text-sm",
                "{message.body_text}"
            }
            ChatFooter { class: "mt-1 flex items-center gap-1 text-xs text-base-content/50",
                Icon { name: if message.channel == Channel::Email { IconName::Mail } else { IconName::Send }, class: "size-3" }
                "{via}"
            }
        }
    }
}

#[component]
fn BackToSupport() -> Element {
    rsx! {
        Link { to: Route::support(), class: "mb-4 inline-flex items-center gap-1 text-sm text-base-content/70 hover:text-base-content",
            Icon { name: IconName::ArrowLeft, class: "size-3.5" }
            "All conversations"
        }
    }
}
