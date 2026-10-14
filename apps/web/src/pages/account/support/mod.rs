//! Customer support (commerce.md §6, design.md "Support (customer)"): the caller's tickets,
//! each a conversation, and the "Send as email" form.
//!
//! This is the customer's window onto the support pipeline (mail-in, `SupportTicket`, the
//! router and its routes); the pipeline itself lives in `crates/services`. The pages only read
//! `GET /api/tickets*` and post `POST /api/support/messages`.

mod composer;
mod thread;

pub use thread::AccountTicket;

use dioxus::prelude::*;
use techdepo_api_types::Channel;
use techdepo_api_types::endpoints::ListTickets;
use techdepo_api_types::support::{TicketListQuery, TicketSummary};

use crate::api::use_api;
use crate::pages::admin::common::use_paged;
use crate::pages::{PageHeader, Panel};
use crate::routes::Route;
use crate::session::use_session;
use crate::ui::format::datetime;
use crate::ui::{EmptyState, Icon, IconName, Mono, TicketStatusBadge};
use composer::Composer;

#[component]
pub fn AccountSupport(subject: String) -> Element {
    let api = use_api();
    let session = use_session();
    let nav = navigator();
    let address = session.me().map(|me| me.org.support_address);
    // Paged like the order list: newest first, "Load more" for the rest.
    let tickets = use_paged(move |cursor| async move {
        let query = TicketListQuery {
            cursor,
            limit: Some(50),
            ..TicketListQuery::default()
        };
        api.query::<ListTickets>(&[], &query).await
    });

    rsx! {
        PageHeader { title: "Support", subtitle: "Your conversations with TechDepó. AI answers what it can; people handle the rest." }
        div { class: "grid gap-6 lg:grid-cols-[1fr_24rem]",
            div { class: "flex flex-col gap-3",
                {tickets.render(|items| if items.is_empty() {
                    rsx! {
                        EmptyState { message: "No conversations yet.", icon: IconName::LifeBuoy,
                            if let Some(address) = address.clone() {
                                a { class: "btn btn-primary btn-sm", href: "mailto:{address}", "Email support" }
                            }
                        }
                    }
                } else {
                    rsx! {
                        ul { class: "flex flex-col gap-2",
                            for ticket in items {
                                TicketRow { key: "{ticket.id}", ticket }
                            }
                        }
                    }
                })}
            }
            div { class: "flex flex-col gap-4 lg:sticky lg:top-20 lg:self-start",
                Panel { title: "Send as email", subtitle: "Opens a new conversation.",
                    Composer {
                        subject,
                        on_sent: move |sent: techdepo_api_types::support::SendSupportMessageResponse| {
                            nav.push(Route::AccountTicket { id: sent.ticket_id.to_string() });
                        },
                    }
                }
                if let Some(address) = address {
                    div { class: "flex items-start gap-2 rounded-box border border-base-300 p-4 text-sm",
                        Icon { name: IconName::Mail, class: "mt-0.5 size-4 text-base-content/60" }
                        span {
                            "Or email "
                            a { class: "link link-primary", href: "mailto:{address}", Mono { "{address}" } }
                            " from your account's address."
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TicketRow(ticket: TicketSummary) -> Element {
    let last = ticket.last_message_at.unwrap_or(ticket.updated_at);
    rsx! {
        li {
            Link {
                to: Route::AccountTicket { id: ticket.id.to_string() },
                class: "flex items-center gap-3 rounded-box border border-base-300 bg-base-100 p-4 hover:bg-base-200",
                Icon {
                    name: if ticket.channel == Channel::Email { IconName::Mail } else { IconName::MessageSquarePlus },
                    class: "size-4 text-base-content/60",
                    label: format!("Started by {}", ticket.channel.label().to_lowercase()),
                }
                div { class: "flex min-w-0 flex-1 flex-col gap-0.5",
                    span { class: "truncate font-medium", "{ticket.subject}" }
                    span { class: "text-xs text-base-content/60",
                        Mono { "{ticket.id}" }
                        if let Some(order) = ticket.order.clone() {
                            " · "
                            Mono { "{order.number}" }
                        }
                        " · {ticket.message_count} message{plural(ticket.message_count)} · {datetime(last)}"
                    }
                }
                TicketStatusBadge { status: ticket.status }
            }
        }
    }
}

const fn plural(n: u32) -> &'static str {
    if n == 1 { "" } else { "s" }
}
