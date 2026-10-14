//! One module per page. `shop` is the storefront; `account` and `admin` render inside their
//! sidebar layouts (and behind `RequireAuth`).

pub mod account;
pub mod admin;
pub mod shop;

use dioxus::prelude::*;

/// A page heading with an optional subtitle and actions on the right.
#[component]
pub fn PageHeader(
    title: String,
    #[props(default)] subtitle: Option<String>,
    #[props(default)] actions: Option<Element>,
) -> Element {
    rsx! {
        div { class: "mb-6 flex flex-wrap items-end justify-between gap-4",
            div { class: "flex flex-col gap-1",
                h1 { class: "text-2xl font-semibold tracking-tight", "{title}" }
                if let Some(subtitle) = subtitle {
                    p { class: "text-base-content/70", "{subtitle}" }
                }
            }
            if let Some(actions) = actions {
                div { class: "flex flex-wrap gap-2", {actions} }
            }
        }
    }
}

/// A bordered card with a title row (account pages: order cards, profile sections).
#[component]
pub fn Panel(
    title: String,
    #[props(default)] subtitle: Option<String>,
    #[props(default)] actions: Option<Element>,
    #[props(default)] class: String,
    children: Element,
) -> Element {
    rsx! {
        section { class: "card card-border border-base-300 bg-base-100 {class}",
            div { class: "card-body gap-3 p-5",
                div { class: "flex flex-wrap items-start justify-between gap-2",
                    div { class: "flex flex-col",
                        h2 { class: "card-title text-base", "{title}" }
                        if let Some(subtitle) = subtitle {
                            span { class: "text-sm text-base-content/60", "{subtitle}" }
                        }
                    }
                    if let Some(actions) = actions {
                        div { class: "flex flex-wrap items-center gap-2", {actions} }
                    }
                }
                {children}
            }
        }
    }
}
