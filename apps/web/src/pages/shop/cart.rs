//! The cart: lines with stock, the compatibility report of the build-relevant parts, totals.

use dioxus::prelude::*;
use techdepo_api_types::account::{Cart, CartLine, MAX_CART_QUANTITY};
use techdepo_api_types::{ApiError, ProductId};

use crate::auth::RequireAuth;
use crate::components::toast::{ToastOptions, use_toast};
use crate::pages::PageHeader;
use crate::routes::Route;
use crate::state::build::catalog;
use crate::state::use_cart;
use crate::ui::product::ProductTile;
use crate::ui::{
    CompatBadge, EmptyState, ErrorAlert, Icon, IconName, MoneyText, Mono, PageSkeleton,
    SeverityIcon, StockBadge,
};

#[component]
pub fn CartPage() -> Element {
    rsx! {
        RequireAuth { CartView {} }
    }
}

#[component]
fn CartView() -> Element {
    let cart = use_cart();
    let mut error = use_signal(|| None::<ApiError>);
    // Always show fresh stock and prices on this page.
    use_hook(move || {
        spawn(async move {
            if let Err(e) = cart.refresh().await {
                error.set(Some(e));
            }
        });
    });

    if let Some(e) = error() {
        return rsx! {
            ErrorAlert {
                error: e,
                on_retry: move |_| {
                    error.set(None);
                    spawn(async move {
                        if let Err(e) = cart.refresh().await {
                            error.set(Some(e));
                        }
                    });
                },
            }
        };
    }
    let Some(current) = cart.cart.read().clone() else {
        return rsx! { PageSkeleton {} };
    };
    if current.lines.is_empty() {
        return rsx! {
            PageHeader { title: "Cart" }
            EmptyState { message: "Your cart is empty.", icon: IconName::Cart,
                div { class: "flex gap-2",
                    Link { to: Route::Builder {}, class: "btn btn-primary btn-sm", "Build a PC" }
                    Link { to: Route::Home {}, class: "btn btn-sm", "Browse parts" }
                }
            }
        };
    }

    rsx! {
        PageHeader { title: "Cart", subtitle: "{current.item_count()} items" }
        div { class: "grid gap-6 lg:grid-cols-[1fr_22rem]",
            div { class: "flex flex-col gap-4",
                ul { class: "flex flex-col divide-y divide-base-300 rounded-box border border-base-300",
                    for line in current.lines.clone() {
                        CartLineRow { key: "{line.product_id}", line }
                    }
                }
                CartCompat { cart: current.clone() }
            }
            CartSummary { cart: current }
        }
    }
}

#[component]
fn CartLineRow(line: CartLine) -> Element {
    let cart = use_cart();
    let toasts = use_toast();
    let mut busy = use_signal(|| false);
    let id = line.product_id.clone();

    let set = move |id: ProductId, quantity: u32| {
        spawn(async move {
            busy.set(true);
            if let Err(e) = cart.set_quantity(id, quantity).await {
                toasts.error(e.message, ToastOptions::new());
            }
            busy.set(false);
        });
    };
    let q = line.quantity;
    let (id_less, id_more, id_remove) = (id.clone(), id.clone(), id.clone());

    rsx! {
        li { class: "flex flex-col gap-3 p-4 sm:flex-row sm:items-center",
            div { class: "flex flex-1 items-center gap-3 min-w-0",
                ProductTile { category: line.category, class: "size-14 shrink-0", icon_class: "size-6" }
                div { class: "flex min-w-0 flex-col gap-0.5",
                    Link { to: Route::Product { id: id.to_string() }, class: "font-medium hover:text-primary truncate", "{line.name}" }
                    Mono { class: "text-xs text-base-content/60", "{line.sku}" }
                    div { class: "flex flex-wrap items-center gap-2",
                        StockBadge { stock: line.stock }
                        if !line.in_stock() && line.stock.stock > 0 {
                            span { class: "text-xs text-error", "Only {line.stock.stock} available" }
                        }
                    }
                }
            }
            div { class: "flex items-center justify-between gap-4 sm:justify-end",
                div { class: "join",
                    button {
                        class: "btn btn-sm join-item btn-square",
                        aria_label: "One less",
                        disabled: busy() || q <= 1,
                        onclick: move |_| set(id_less.clone(), q - 1),
                        Icon { name: IconName::Minus }
                    }
                    span { class: "join-item grid w-10 place-items-center border-y border-base-300 text-sm tabular-nums", "{q}" }
                    button {
                        class: "btn btn-sm join-item btn-square",
                        aria_label: "One more",
                        disabled: busy() || q >= MAX_CART_QUANTITY,
                        onclick: move |_| set(id_more.clone(), q + 1),
                        Icon { name: IconName::Plus }
                    }
                }
                // Fixed width so the steppers line up across rows whatever the price.
                div { class: "flex min-w-24 flex-col items-end whitespace-nowrap",
                    MoneyText { amount: line.line_total, class: "font-semibold" }
                    if q > 1 {
                        span { class: "text-xs text-base-content/60",
                            MoneyText { amount: line.unit_price }
                            " each"
                        }
                    }
                }
                button {
                    class: "btn btn-ghost btn-sm btn-square",
                    aria_label: "Remove {line.name}",
                    disabled: busy(),
                    onclick: move |_| {
                        let id = id_remove.clone();
                        spawn(async move {
                            busy.set(true);
                            if let Err(e) = cart.remove(id).await {
                                toasts.error(e.message, ToastOptions::new());
                            }
                            busy.set(false);
                        });
                    },
                    Icon { name: IconName::Trash }
                }
            }
        }
    }
}

/// The compatibility report of the build-relevant parts in the cart.
#[component]
fn CartCompat(cart: Cart) -> Element {
    let Some(report) = cart.report else {
        return rsx! {};
    };
    let name = |id: &ProductId| {
        catalog()
            .get(id)
            .map_or_else(|| id.to_string(), |p| p.name.clone())
    };
    rsx! {
        div { class: "rounded-box border border-base-300 p-4 flex flex-col gap-3",
            div { class: "flex items-center justify-between",
                h2 { class: "font-semibold", "Do these parts work together?" }
                CompatBadge { verdict: report.verdict }
            }
            if report.issues.is_empty() {
                p { class: "text-sm text-base-content/70", "Every pairing we check is fine." }
            }
            ul { class: "flex flex-col gap-2",
                for issue in report.issues.iter() {
                    li { class: "flex gap-2 text-sm",
                        SeverityIcon { severity: issue.severity, class: "size-4 mt-0.5" }
                        div { class: "flex flex-col",
                            span { "{issue.message}" }
                            span { class: "text-xs text-base-content/60",
                                "{issue.parts.iter().map(name).collect::<Vec<_>>().join(\" + \")}"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CartSummary(cart: Cart) -> Element {
    let store = use_cart();
    let toasts = use_toast();
    let blocked = !cart.out_of_stock.is_empty();
    rsx! {
        div { class: "lg:sticky lg:top-24 lg:self-start",
            div { class: "card card-border border-base-300 bg-base-100",
                div { class: "card-body gap-3",
                    h2 { class: "card-title text-base", "Order summary" }
                    div { class: "flex justify-between text-sm",
                        span { "Items" }
                        MoneyText { amount: cart.items_total }
                    }
                    div { class: "flex justify-between text-sm",
                        span { "Shipping" }
                        if cart.shipping.is_zero() { span { "Free" } } else { MoneyText { amount: cart.shipping } }
                    }
                    div { class: "flex items-baseline justify-between border-t border-base-300 pt-3",
                        span { "Total" }
                        MoneyText { amount: cart.total, class: "text-2xl font-semibold" }
                    }
                    span { class: "text-xs text-base-content/60", "Prices include VAT." }
                    if blocked {
                        div { role: "alert", class: "alert alert-error alert-soft text-sm",
                            "Some items are out of stock. Lower the quantity or remove them to check out."
                        }
                    } else if cart.needs_compat_override() {
                        div { role: "alert", class: "alert alert-warning alert-soft text-sm",
                            "These parts have compatibility errors. You can still order after confirming at checkout."
                        }
                    }
                    Link {
                        to: Route::Checkout {},
                        class: if blocked { "btn btn-primary btn-disabled" } else { "btn btn-primary" },
                        aria_disabled: "{blocked}",
                        "Checkout"
                    }
                    button {
                        class: "btn btn-ghost btn-sm",
                        onclick: move |_| {
                            spawn(async move {
                                if let Err(e) = store.clear().await {
                                    toasts.error(e.message, ToastOptions::new());
                                }
                            });
                        },
                        "Empty cart"
                    }
                }
            }
        }
    }
}
