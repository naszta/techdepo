//! Support tickets (commerce.md §7): the list with classification, route and assignee, and one
//! ticket's thread, "why" side panel and agent trace.
//!
//! Tickets come from the support pipeline (mail-in and the web form, `SupportTicket`); until
//! it runs, the list is simply empty and every AI field renders as "not classified yet".

mod detail;
mod trace;

use dioxus::prelude::*;
use techdepo_api_types::admin::{AdminTicketListQuery, AdminTicketSummary};
use techdepo_api_types::endpoints::ListAdminTickets;
use techdepo_api_types::{Assignee, Intent, TicketStatus};

pub use detail::AdminTicket;

use super::common::{
    CustomerCell, FilterBar, SelectFilter, TableWrap, ToggleFilter, options, parsed, use_paged,
};
use crate::api::use_api;
use crate::pages::PageHeader;
use crate::routes::Route;
use crate::ui::explain::prob;
use crate::ui::format::ago;
use crate::ui::{AssigneeTag, EmptyState, Icon, IconName, Mono, Tag, TicketStatusBadge};

#[component]
pub fn AdminTickets() -> Element {
    let api = use_api();
    let open_only = use_signal(|| true);
    let status = use_signal(String::new);
    let intent = use_signal(String::new);
    let assignee = use_signal(String::new);
    let tickets = use_paged(move |cursor| async move {
        let status = parsed::<TicketStatus>(&status());
        let query = AdminTicketListQuery {
            status,
            // A status filter wins over "open only" (filtering by resolved shows resolved).
            open: (open_only() && status.is_none()).then_some(true),
            intent: parsed::<Intent>(&intent()),
            assignee: parsed::<Assignee>(&assignee()),
            user_id: None,
            cursor,
            limit: Some(50),
        };
        api.query::<ListAdminTickets>(&[], &query).await
    });

    rsx! {
        PageHeader { title: "Tickets", subtitle: "Every conversation, how it was classified, and who handles it." }
        FilterBar {
            ToggleFilter { label: "Open only", value: open_only }
            SelectFilter { label: "Status", value: status, options: options(TicketStatus::ALL, TicketStatus::as_str, TicketStatus::label) }
            SelectFilter { label: "Intent", value: intent, options: options(Intent::ALL, Intent::as_str, Intent::label) }
            SelectFilter { label: "Assignee", value: assignee, options: vec![("ai".to_owned(), "AI".to_owned()), ("human".to_owned(), "A person".to_owned())] }
        }
        {tickets.render(|items| rsx! { TicketsTable { tickets: items } })}
    }
}

#[component]
fn TicketsTable(tickets: Vec<AdminTicketSummary>) -> Element {
    if tickets.is_empty() {
        return rsx! {
            EmptyState {
                message: "No tickets match. Email the support address or use the web form under My account → Support.",
                icon: IconName::Ticket,
            }
        };
    }
    rsx! {
        TableWrap {
            table { class: "table table-sm",
                thead {
                    tr {
                        th { "Ticket" }
                        th { "Customer" }
                        th { "Intent" }
                        th { "Route" }
                        th { "Handled by" }
                        th { "Status" }
                        th { "Updated" }
                    }
                }
                tbody {
                    for row in tickets {
                        tr { key: "{row.ticket.id}", class: "hover:bg-base-200",
                            td { class: "max-w-72",
                                Link { to: Route::admin_ticket(&row.ticket.id), class: "flex flex-col",
                                    span { class: "link-hover truncate font-medium", "{row.ticket.subject}" }
                                    span { class: "text-xs text-base-content/60",
                                        Mono { "{row.ticket.id}" }
                                        if let Some(order) = row.ticket.order.clone() { " · " Mono { "{order.number}" } }
                                        " · {row.ticket.channel.label()}"
                                    }
                                }
                            }
                            td { class: "max-w-48", CustomerCell { customer: row.customer.clone() } }
                            td { class: "whitespace-nowrap",
                                match row.intent {
                                    Some(intent) => rsx! {
                                        "{intent.label()}"
                                        if let Some(p) = row.intent_confidence {
                                            span { class: "ml-1 font-mono text-xs text-base-content/60", "{prob(p)}" }
                                        }
                                    },
                                    None => rsx! { span { class: "text-base-content/40", "–" } },
                                }
                            }
                            td { class: "whitespace-nowrap",
                                match row.route {
                                    Some(route) => rsx! { span { class: "inline-flex items-center gap-1",
                                        Icon { name: if route.is_agent() { IconName::Bot } else { IconName::Route }, class: "size-3.5 text-base-content/60" }
                                        "{route.label()}"
                                    } },
                                    None => rsx! { span { class: "text-base-content/40", "–" } },
                                }
                            }
                            td { AssigneeTag { assignee: row.assignee } }
                            td {
                                div { class: "flex flex-wrap items-center gap-1",
                                    TicketStatusBadge { status: row.ticket.status }
                                    if row.pending_tasks > 0 {
                                        Tag { icon: IconName::ListChecks,
                                            "{row.pending_tasks} task{crate::pages::admin::common::plural(row.pending_tasks)}"
                                        }
                                    }
                                }
                            }
                            td { class: "whitespace-nowrap text-base-content/70", "{ago(row.ticket.updated_at)}" }
                        }
                    }
                }
            }
        }
    }
}
