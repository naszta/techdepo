//! The customer's orders, newest first, with "load more" paging. The demo scenario presets sit
//! in the demo drawer here too, so a presenter can start from an order in any state.

use dioxus::prelude::*;
use techdepo_api_types::endpoints::ListOrders;
use techdepo_api_types::orders::{OrderListQuery, OrderSummary};
use techdepo_api_types::{ApiError, Page};

use crate::api::use_api;
use crate::pages::PageHeader;
use crate::pages::account::demo::ScenarioPresets;
use crate::routes::Route;
use crate::ui::format::date;
use crate::ui::{
    DemoDrawer, DemoSection, EmptyState, ErrorAlert, IconName, MoneyText, Mono, OrderFlagBadges,
    OrderStatusBadge, PageSkeleton,
};

#[component]
pub fn AccountOrders() -> Element {
    let api = use_api();
    let mut include_cancelled = use_signal(|| false);
    // Pages after the first, appended by "Load more"; reset when the filter changes.
    let mut more = use_signal(Vec::<OrderSummary>::new);
    let mut cursor = use_signal(|| None::<String>);
    let mut loading_more = use_signal(|| false);
    let mut more_error = use_signal(|| None::<ApiError>);

    let query = move |cursor: Option<String>| OrderListQuery {
        include_cancelled: include_cancelled().then_some(true),
        cursor,
        ..OrderListQuery::default()
    };
    let mut orders = use_resource(move || async move {
        let page = api.query::<ListOrders>(&[], &query(None)).await;
        more.set(Vec::new());
        more_error.set(None);
        cursor.set(
            page.as_ref()
                .ok()
                .and_then(|p: &Page<OrderSummary>| p.next_cursor.clone()),
        );
        page
    });

    let load_more = move |_| {
        spawn(async move {
            loading_more.set(true);
            match api.query::<ListOrders>(&[], &query(cursor())).await {
                Ok(page) => {
                    more.write().extend(page.items);
                    cursor.set(page.next_cursor);
                }
                Err(error) => more_error.set(Some(error)),
            }
            loading_more.set(false);
        });
    };

    rsx! {
        PageHeader {
            title: "Orders",
            actions: rsx! {
                label { class: "label text-sm",
                    input {
                        r#type: "checkbox",
                        class: "toggle toggle-sm",
                        checked: include_cancelled(),
                        onchange: move |e| include_cancelled.set(e.checked()),
                    }
                    "Show cancelled"
                }
            },
        }
        match &*orders.read() {
            None => rsx! { PageSkeleton {} },
            Some(Err(error)) => rsx! { ErrorAlert { error: error.clone(), on_retry: move |_| orders.restart() } },
            Some(Ok(page)) => {
                let mut items = page.items.clone();
                items.extend(more());
                rsx! {
                    OrderList { orders: items }
                    if let Some(error) = more_error() {
                        div { class: "mt-4", ErrorAlert { error } }
                    }
                    if cursor().is_some() {
                        div { class: "mt-4 flex justify-center",
                            button { class: "btn btn-sm", disabled: loading_more(), onclick: load_more,
                                if loading_more() { span { class: "loading loading-spinner loading-xs" } }
                                "Load more"
                            }
                        }
                    }
                }
            }
        }
        DemoDrawer { subtitle: "Start from an order that is already in an interesting state",
            DemoSection { title: "Scenario presets", icon: IconName::Play,
                ScenarioPresets {}
            }
        }
    }
}

/// Orders as a compact table (an empty state when there are none).
#[component]
pub fn OrderList(orders: Vec<OrderSummary>) -> Element {
    if orders.is_empty() {
        return rsx! {
            EmptyState { message: "No orders yet.", icon: IconName::Package,
                Link { to: Route::Builder {}, class: "btn btn-primary btn-sm", "Build a PC" }
            }
        };
    }
    rsx! {
        div { class: "overflow-x-auto rounded-box border border-base-300",
            table { class: "table",
                thead {
                    tr {
                        th { "Order" }
                        th { "Placed" }
                        th { "Items" }
                        th { "Status" }
                        th { class: "text-right", "Total" }
                    }
                }
                tbody {
                    for order in orders {
                        tr { key: "{order.id}", class: "hover:bg-base-200",
                            td {
                                Link { to: Route::AccountOrder { number: order.number.to_string() }, class: "link link-hover whitespace-nowrap",
                                    Mono { "{order.number}" }
                                }
                            }
                            td { class: "whitespace-nowrap", "{date(order.created_at)}" }
                            td { class: "max-w-xs truncate", "{order.headline}" }
                            td {
                                div { class: "flex flex-wrap items-center gap-1",
                                    OrderStatusBadge { status: order.status }
                                    OrderFlagBadges { flags: order.flags.clone() }
                                }
                            }
                            td { class: "text-right", MoneyText { amount: order.total } }
                        }
                    }
                }
            }
        }
    }
}
