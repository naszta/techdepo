//! Members of the shop (the `org_members` cache of Clerk or dev logins) with their orders and
//! tickets.

use dioxus::prelude::*;
use techdepo_api_types::admin::CustomerListQuery;
use techdepo_api_types::endpoints::{GetCustomer, ListCustomers};
use techdepo_api_types::vocab::OrgRole;

use super::common::{
    BackLink, FilterBar, Muted, OrderLink, SearchFilter, TableWrap, non_blank, use_paged,
};
use crate::api::use_api;
use crate::pages::{PageHeader, Panel};
use crate::routes::Route;
use crate::ui::format::{ago, date};
use crate::ui::{
    EmptyState, ErrorAlert, IconName, MoneyText, Mono, OrderFlagBadges, OrderStatusBadge,
    PageSkeleton, TicketStatusBadge,
};

#[component]
pub fn AdminCustomers() -> Element {
    let api = use_api();
    let q = use_signal(String::new);
    let customers = use_paged(move |cursor| async move {
        let query = CustomerListQuery {
            q: non_blank(&q()),
            cursor,
            limit: Some(50),
        };
        api.query::<ListCustomers>(&[], &query).await
    });

    rsx! {
        PageHeader { title: "Customers", subtitle: "Everyone who has signed in to this shop." }
        FilterBar {
            SearchFilter { label: "Search", placeholder: "Name or email", value: q }
        }
        {customers.render(|items| rsx! {
            if items.is_empty() {
                EmptyState { message: "No customers match.", icon: IconName::Users }
            } else {
                TableWrap {
                    table { class: "table table-sm",
                        thead {
                            tr {
                                th { "Customer" }
                                th { "Role" }
                                th { class: "text-right", "Orders" }
                                th { class: "text-right", "Open tickets" }
                                th { class: "text-right", "Spent" }
                                th { "Last seen" }
                            }
                        }
                        tbody {
                            for c in items {
                                tr { key: "{c.customer.user_id}", class: "hover:bg-base-200",
                                    td {
                                        Link { to: Route::AdminCustomer { user_id: c.customer.user_id.to_string() }, class: "flex flex-col",
                                            span { class: "link-hover font-medium", {c.customer.name.clone().unwrap_or_else(|| c.customer.email.clone())} }
                                            span { class: "text-xs text-base-content/60", "{c.customer.email}" }
                                        }
                                    }
                                    td { RoleBadge { role: c.role } }
                                    td { class: "text-right tabular-nums", "{c.order_count}" }
                                    td { class: "text-right tabular-nums", "{c.open_tickets}" }
                                    td { class: "text-right", MoneyText { amount: c.total_spent } }
                                    td { class: "whitespace-nowrap text-base-content/70",
                                        {c.last_seen_at.map(ago).unwrap_or_else(|| "–".to_owned())}
                                    }
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
fn RoleBadge(role: OrgRole) -> Element {
    rsx! {
        span {
            class: if role.is_admin() { "badge badge-sm badge-soft badge-primary" } else { "badge badge-sm badge-ghost" },
            "{role.label()}"
        }
    }
}

#[component]
pub fn AdminCustomer(user_id: String) -> Element {
    let api = use_api();
    let mut detail = use_resource(use_reactive!(|(user_id,)| async move {
        api.get::<GetCustomer>(&[user_id.as_str()]).await
    }));
    let d = match &*detail.read() {
        None => return rsx! { PageSkeleton { rows: 3 } },
        Some(Err(error)) => {
            return rsx! {
                BackLink { to: Route::AdminCustomers {}, label: "All customers" }
                ErrorAlert { error: error.clone(), on_retry: move |_| detail.restart() }
            };
        }
        Some(Ok(d)) => d.clone(),
    };
    let c = d.customer.clone();
    let name = c
        .customer
        .name
        .clone()
        .unwrap_or_else(|| c.customer.email.clone());

    rsx! {
        BackLink { to: Route::AdminCustomers {}, label: "All customers" }
        div { class: "mb-5 flex flex-col gap-1",
            div { class: "flex flex-wrap items-center gap-2",
                h1 { class: "text-xl font-semibold tracking-tight", "{name}" }
                RoleBadge { role: c.role }
            }
            p { class: "text-base-content/60",
                "{c.customer.email} · "
                Mono { "{c.customer.user_id}" }
                if let Some(seen) = c.last_seen_at { " · last seen {ago(seen)}" }
            }
        }
        div { class: "stats mb-5 w-full border border-base-300 bg-base-100",
            div { class: "stat",
                div { class: "stat-title", "Orders" }
                div { class: "stat-value text-2xl font-semibold", "{c.order_count}" }
            }
            div { class: "stat",
                div { class: "stat-title", "Open tickets" }
                div { class: "stat-value text-2xl font-semibold", "{c.open_tickets}" }
            }
            div { class: "stat",
                div { class: "stat-title", "Spent (net)" }
                div { class: "stat-value text-2xl font-semibold", MoneyText { amount: c.total_spent } }
            }
        }
        div { class: "grid items-start gap-5 *:min-w-0 lg:grid-cols-2",
            Panel { title: "Orders",
                if d.orders.is_empty() { Muted { "No orders yet." } }
                ul { class: "flex flex-col divide-y divide-base-300",
                    for order in d.orders.iter().cloned() {
                        li { key: "{order.id}", class: "flex flex-wrap items-center gap-2 py-2",
                            OrderLink { number: order.number }
                            OrderStatusBadge { status: order.status }
                            OrderFlagBadges { flags: order.flags.clone() }
                            span { class: "min-w-0 flex-1 truncate text-base-content/70", "{order.headline}" }
                            span { class: "text-xs text-base-content/60", "{date(order.created_at)}" }
                            MoneyText { amount: order.total }
                        }
                    }
                }
            }
            Panel { title: "Tickets",
                if d.tickets.is_empty() { Muted { "No tickets yet." } }
                ul { class: "flex flex-col divide-y divide-base-300",
                    for ticket in d.tickets.iter().cloned() {
                        li { key: "{ticket.id}", class: "py-2",
                            Link { to: Route::admin_ticket(&ticket.id), class: "flex items-center gap-2",
                                div { class: "flex min-w-0 flex-1 flex-col",
                                    span { class: "link-hover truncate font-medium", "{ticket.subject}" }
                                    span { class: "text-xs text-base-content/60", Mono { "{ticket.id}" } " · {date(ticket.created_at)}" }
                                }
                                TicketStatusBadge { status: ticket.status }
                            }
                        }
                    }
                }
            }
        }
    }
}
