//! Admin overview (design.md "Admin overview"): a stat row, the oldest pending human tasks
//! prominently below it, orders by status and the recent activity feed.

use dioxus::prelude::*;
use techdepo_api_types::OrderStatus;
use techdepo_api_types::admin::AdminOverview as Overview;
use techdepo_api_types::endpoints::GetAdminOverview;

use super::common::EntityLink;
use super::tasks::TaskRow;
use crate::api::use_api;
use crate::pages::{PageHeader, Panel};
use crate::routes::Route;
use crate::ui::format::ago;
use crate::ui::{ActorTag, ErrorAlert, Icon, IconName, OrderStatusBadge, PageSkeleton};

#[component]
pub fn AdminOverview() -> Element {
    let api = use_api();
    let mut overview = use_resource(move || async move { api.get::<GetAdminOverview>(&[]).await });

    rsx! {
        PageHeader {
            title: "Overview",
            actions: rsx! {
                button { class: "btn btn-ghost btn-sm", onclick: move |_| overview.restart(),
                    Icon { name: IconName::RefreshCw, class: "size-3.5" }
                    "Refresh"
                }
            },
        }
        match &*overview.read() {
            None => rsx! { PageSkeleton { rows: 3 } },
            Some(Err(error)) => rsx! { ErrorAlert { error: error.clone(), on_retry: move |_| overview.restart() } },
            Some(Ok(data)) => rsx! { OverviewBody { data: data.clone() } },
        }
    }
}

#[component]
fn OverviewBody(data: Overview) -> Element {
    let total_orders: u32 = data.orders_by_status.iter().map(|s| s.count).sum();
    let count = |status: OrderStatus| {
        data.orders_by_status
            .iter()
            .find(|s| s.status == status)
            .map_or(0, |s| s.count)
    };
    let to_ship = count(OrderStatus::Paid) + count(OrderStatus::Processing);
    let in_transit = count(OrderStatus::Shipped);
    let max = data
        .orders_by_status
        .iter()
        .map(|s| s.count)
        .max()
        .unwrap_or(0)
        .max(1);

    rsx! {
        div { class: "stats stats-vertical w-full border border-base-300 bg-base-100 sm:stats-horizontal",
            Stat {
                title: "Orders",
                value: "{total_orders}",
                desc: format!("{to_ship} to ship · {in_transit} in transit"),
                to: Route::AdminOrders {},
                icon: IconName::Package,
            }
            Stat {
                title: "Open tickets",
                value: "{data.open_tickets}",
                desc: format!("{} open investigation{}", data.open_investigations, if data.open_investigations == 1 { "" } else { "s" }),
                to: Route::AdminTickets {},
                icon: IconName::Ticket,
            }
            Stat {
                title: "Pending tasks",
                value: "{data.pending_tasks}",
                desc: if data.overdue_tasks > 0 { format!("{} past SLA", data.overdue_tasks) } else { "All within SLA".to_owned() },
                to: Route::AdminTasks {},
                icon: IconName::ListChecks,
                warn: data.overdue_tasks > 0,
            }
            Stat {
                title: "Revenue (simulated)",
                value: "{data.revenue}",
                desc: format!("{} refunded · {} awaiting approval", data.refunded, data.refunds_pending),
                to: Route::AdminPayments {},
                icon: IconName::CreditCard,
            }
        }

        div { class: "mt-6 grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_20rem]",
            Panel {
                title: "Human tasks",
                subtitle: "Oldest first. Workflows are waiting on these.",
                actions: rsx! {
                    Link { to: Route::AdminTasks {}, class: "btn btn-sm",
                        "Open inbox"
                        Icon { name: IconName::ArrowRight, class: "size-3.5" }
                    }
                },
                if data.tasks.is_empty() {
                    div { class: "flex items-center gap-2 rounded-field border border-dashed border-base-300 p-4 text-base-content/70",
                        Icon { name: IconName::CircleCheck, class: "size-4 text-success" }
                        "Nothing waiting for a decision."
                    }
                } else {
                    ul { class: "flex flex-col gap-2",
                        for task in data.tasks.iter().cloned() {
                            li { key: "{task.id}", TaskRow { task } }
                        }
                    }
                }
            }
            div { class: "flex flex-col gap-6",
                Panel { title: "Orders by status",
                    ul { class: "flex flex-col gap-1.5",
                        for row in data.orders_by_status.iter().copied() {
                            li { key: "{row.status.as_str()}", class: "grid grid-cols-[8.5rem_1fr_2rem] items-center gap-2",
                                span { OrderStatusBadge { status: row.status } }
                                progress {
                                    class: "progress progress-primary h-1.5 w-full",
                                    value: "{row.count}",
                                    max: "{max}",
                                    aria_label: "{row.status.label()}",
                                }
                                span { class: "text-right tabular-nums", "{row.count}" }
                            }
                        }
                    }
                }
                Panel { title: "Recent activity",
                    if data.activity.is_empty() {
                        p { class: "text-base-content/60", "Nothing has happened yet. Place an order or try a scenario preset." }
                    }
                    ul { class: "flex flex-col divide-y divide-base-300",
                        for (i , item) in data.activity.iter().cloned().enumerate() {
                            li { key: "{i}", class: "flex flex-col gap-1 py-2",
                                span { "{item.summary}" }
                                span { class: "flex flex-wrap items-center gap-2 text-xs text-base-content/60",
                                    ActorTag { actor: item.actor }
                                    EntityLink { entity: item.entity.clone() }
                                    span { "{ago(item.at)}" }
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
fn Stat(
    title: &'static str,
    value: String,
    desc: String,
    to: Route,
    icon: IconName,
    #[props(default)] warn: bool,
) -> Element {
    rsx! {
        Link { to, class: "stat hover:bg-base-200",
            div { class: "stat-figure text-base-content/40", Icon { name: icon, class: "size-6" } }
            div { class: "stat-title", "{title}" }
            div { class: "stat-value text-2xl font-semibold tabular-nums", "{value}" }
            div {
                class: if warn { "stat-desc flex items-center gap-1 font-medium text-warning" } else { "stat-desc" },
                if warn { Icon { name: IconName::Hourglass, class: "size-3" } }
                "{desc}"
            }
        }
    }
}
