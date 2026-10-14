//! The customer's order page (design.md "Order detail", commerce.md §6): a status stepper,
//! then cards for items, the card statement, invoices, the shipment and linked tickets, then
//! the event timeline. The demo controls drawer opens from a floating button.
//!
//! Demo actions go through Restate and some effects land asynchronously (the invoice and the
//! confirmation email follow a payment), so after every action the page re-reads the order a
//! few times instead of trusting one response.

pub(crate) mod cards;
pub(crate) mod invoice;
mod progress;
pub(crate) mod timeline;

use dioxus::prelude::*;
use techdepo_api_types::endpoints::{GetDemoPanel, GetOrder};

use crate::api::use_api;
use crate::browser;
use crate::pages::account::demo::OrderDemoControls;
use crate::routes::Route;
use crate::ui::format::date;
use crate::ui::{
    ErrorAlert, Icon, IconName, Mono, OrderFlagBadges, OrderStatusBadge, PageSkeleton,
};

use cards::{AddressesCard, InvoicesCard, ItemsCard, ShipmentCard, StatementCard, TicketsCard};
use progress::OrderProgress;
use timeline::OrderTimeline;

/// After a demo action: re-read now, then again while asynchronous effects land.
const FOLLOW_UP_MS: [u32; 2] = [1_500, 4_000];

#[component]
pub fn AccountOrder(number: String) -> Element {
    let api = use_api();
    let number_for_demo = number.clone();
    let mut order = use_resource(use_reactive!(|(number,)| async move {
        api.get::<GetOrder>(&[number.as_str()]).await
    }));
    let number = number_for_demo;
    let mut demo = use_resource(use_reactive!(|(number,)| async move {
        api.get::<GetDemoPanel>(&[number.as_str()]).await
    }));

    let refresh = move |_: ()| {
        order.restart();
        demo.restart();
        spawn(async move {
            for ms in FOLLOW_UP_MS {
                browser::sleep(ms).await;
                order.restart();
                demo.restart();
            }
        });
    };

    let detail = match &*order.read() {
        None => return rsx! { PageSkeleton { rows: 4 } },
        Some(Err(error)) => {
            return rsx! {
                BackToOrders {}
                ErrorAlert { error: error.clone(), on_retry: move |_| order.restart() }
            };
        }
        Some(Ok(detail)) => detail.clone(),
    };
    let summary = detail.order.clone();
    let panel = demo.read().clone();

    rsx! {
        BackToOrders {}
        div { class: "mb-6 flex flex-wrap items-start justify-between gap-4",
            div { class: "flex flex-col gap-1",
                div { class: "flex flex-wrap items-center gap-2",
                    h1 { class: "text-2xl font-semibold tracking-tight", Mono { "{summary.number}" } }
                    OrderStatusBadge { status: summary.status }
                    OrderFlagBadges { flags: summary.flags.clone() }
                }
                p { class: "text-sm text-base-content/60",
                    "Placed {date(summary.created_at)}"
                    if let Some(original) = summary.replacement_for_number {
                        " · replaces "
                        Link {
                            to: Route::AccountOrder { number: original.to_string() },
                            class: "link link-hover",
                            Mono { "{original}" }
                        }
                    }
                }
            }
            a { class: "btn btn-sm", href: "{detail.support_mailto}",
                Icon { name: IconName::Mail }
                "Email support about this order"
            }
        }

        OrderProgress { detail: detail.clone() }

        div { class: "mt-6 grid gap-6 *:min-w-0 @2xl:grid-cols-2",
            ItemsCard { detail: detail.clone() }
            div { class: "flex min-w-0 flex-col gap-6",
                ShipmentCard { shipment: detail.shipment.clone(), status: summary.status }
                AddressesCard { shipping: detail.shipping_address.clone(), billing: detail.billing_address.clone() }
            }
            StatementCard { detail: detail.clone() }
            InvoicesCard { number: summary.number.to_string(), invoices: detail.invoices.clone() }
        }
        // Bottom padding keeps the floating demo button off the last card.
        div { class: "mt-6 grid gap-6 pb-16 *:min-w-0 @2xl:grid-cols-[1fr_20rem]",
            OrderTimeline { events: detail.timeline.clone() }
            TicketsCard { number: summary.number, tickets: detail.tickets.clone(), mailto: detail.support_mailto.clone() }
        }

        OrderDemoControls { detail: detail.clone(), panel, on_changed: refresh }
    }
}

#[component]
fn BackToOrders() -> Element {
    rsx! {
        Link { to: Route::AccountOrders {}, class: "mb-4 inline-flex items-center gap-1 text-sm text-base-content/70 hover:text-base-content",
            Icon { name: IconName::ArrowLeft, class: "size-3.5" }
            "All orders"
        }
    }
}
