//! Account overview: who you are, your shop, and the latest orders.

use dioxus::prelude::*;
use techdepo_api_types::endpoints::ListOrders;
use techdepo_api_types::orders::OrderListQuery;

use crate::api::use_api;
use crate::pages::PageHeader;
use crate::pages::account::orders::OrderList;
use crate::routes::Route;
use crate::session::use_session;
use crate::ui::{ErrorAlert, Mono};

#[component]
pub fn AccountHome() -> Element {
    let session = use_session();
    let api = use_api();
    let mut orders = use_resource(move || async move {
        let query = OrderListQuery {
            limit: Some(5),
            ..OrderListQuery::default()
        };
        api.query::<ListOrders>(&[], &query).await
    });
    let Some(me) = session.me() else {
        return rsx! {};
    };
    let name = me
        .user
        .name
        .clone()
        .unwrap_or_else(|| me.user.email.clone());

    rsx! {
        PageHeader { title: "Hello, {name}", subtitle: "Your orders, builds and support in {me.org.name}." }
        div { class: "grid gap-4 md:grid-cols-3 mb-8",
            div { class: "card card-border border-base-300 bg-base-100",
                div { class: "card-body gap-1",
                    span { class: "text-sm text-base-content/60", "Signed in as" }
                    span { class: "font-medium truncate", "{me.user.email}" }
                    span { class: "text-sm text-base-content/60", "{me.role.label()} of {me.org.name}" }
                }
            }
            div { class: "card card-border border-base-300 bg-base-100 md:col-span-2",
                div { class: "card-body gap-1",
                    span { class: "text-sm text-base-content/60", "Support address" }
                    a { class: "link link-primary truncate", href: "mailto:{me.org.support_address}",
                        Mono { "{me.org.support_address}" }
                    }
                    span { class: "text-sm text-base-content/60",
                        "Email this address about an order and the support pipeline answers."
                    }
                }
            }
        }
        div { class: "mb-3 flex items-center justify-between",
            h2 { class: "text-lg font-semibold", "Recent orders" }
            Link { to: Route::AccountOrders {}, class: "link link-hover text-sm", "All orders" }
        }
        match &*orders.read() {
            None => rsx! { div { class: "skeleton h-32 w-full" } },
            Some(Err(error)) => rsx! { ErrorAlert { error: error.clone(), on_retry: move |_| orders.restart() } },
            Some(Ok(page)) => rsx! { OrderList { orders: page.items.clone() } },
        }
    }
}
