//! Every order of the shop: filter by status and flags, search by order number or email.

use dioxus::prelude::*;
use techdepo_api_types::OrderStatus;
use techdepo_api_types::admin::{AdminOrderListQuery, AdminOrderSummary};
use techdepo_api_types::endpoints::ListAdminOrders;

use super::common::{
    CustomerCell, FilterBar, OrderLink, SearchFilter, SelectFilter, TableWrap, ToggleFilter,
    non_blank, options, parsed, use_paged,
};
use crate::api::use_api;
use crate::pages::PageHeader;
use crate::pages::account::demo::ScenarioPresets;
use crate::ui::format::date;
use crate::ui::{
    DemoDrawer, DemoSection, EmptyState, IconName, MoneyText, OrderFlagBadges, OrderStatusBadge,
};

#[component]
pub fn AdminOrders() -> Element {
    let api = use_api();
    let status = use_signal(String::new);
    let flagged = use_signal(|| false);
    let include_cancelled = use_signal(|| false);
    let q = use_signal(String::new);
    let orders = use_paged(move |cursor| async move {
        let status = parsed::<OrderStatus>(&status());
        let query = AdminOrderListQuery {
            status,
            user_id: None,
            flagged: flagged().then_some(true),
            // Filtering by "cancelled" shows them whatever the toggle says.
            include_cancelled: (include_cancelled() || status == Some(OrderStatus::Cancelled))
                .then_some(true),
            q: non_blank(&q()),
            cursor,
            limit: Some(50),
        };
        api.query::<ListAdminOrders>(&[], &query).await
    });

    rsx! {
        PageHeader { title: "Orders", subtitle: "Every order in the shop, newest first." }
        FilterBar {
            SearchFilter { label: "Search", placeholder: "TD-1042 or email", value: q }
            SelectFilter { label: "Status", value: status, options: options(OrderStatus::ALL, OrderStatus::as_str, OrderStatus::label) }
            ToggleFilter { label: "Needs attention", value: flagged }
            ToggleFilter { label: "Show cancelled", value: include_cancelled }
        }
        {orders.render(|items| rsx! { OrdersTable { orders: items } })}
        DemoDrawer { subtitle: "Start from an order that is already in an interesting state",
            DemoSection { title: "Scenario presets", icon: IconName::Play,
                ScenarioPresets { admin: true }
            }
        }
    }
}

#[component]
pub fn OrdersTable(orders: Vec<AdminOrderSummary>) -> Element {
    if orders.is_empty() {
        return rsx! { EmptyState { message: "No orders match.", icon: IconName::Package } };
    }
    rsx! {
        TableWrap {
            table { class: "table table-sm",
                thead {
                    tr {
                        th { "Order" }
                        th { "Customer" }
                        th { "Placed" }
                        th { "Items" }
                        th { "Status" }
                        th { class: "text-right", "Open tickets" }
                        th { class: "text-right", "Total" }
                    }
                }
                tbody {
                    for row in orders {
                        tr { key: "{row.order.id}", class: "hover:bg-base-200",
                            td { OrderLink { number: row.order.number } }
                            td { class: "max-w-48", CustomerCell { customer: row.customer.clone() } }
                            td { class: "whitespace-nowrap", "{date(row.order.created_at)}" }
                            td { class: "max-w-64 truncate", title: "{row.order.headline}", "{row.order.headline}" }
                            td {
                                div { class: "flex flex-wrap items-center gap-1",
                                    OrderStatusBadge { status: row.order.status }
                                    OrderFlagBadges { flags: row.order.flags.clone() }
                                }
                            }
                            td { class: "text-right tabular-nums",
                                if row.open_tickets > 0 { "{row.open_tickets}" } else { span { class: "text-base-content/40", "–" } }
                            }
                            td { class: "text-right", MoneyText { amount: row.order.total } }
                        }
                    }
                }
            }
        }
    }
}
