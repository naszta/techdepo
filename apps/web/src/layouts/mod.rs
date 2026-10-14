//! Page frames: the shop (top navbar) and the sidebar layouts of account and admin.

mod search;
mod shop;
mod sidebar;

pub use shop::ShopLayout;
pub use sidebar::{AccountLayout, AdminLayout};

use dioxus::prelude::*;

use crate::routes::Route;
use crate::session::use_session;

/// The browser tab title of the current route ([`Route::title`]), so tabs, history and screen
/// readers can tell pages apart.
#[component]
pub fn RouteTitle() -> Element {
    let route = use_route::<Route>();
    rsx! {
        document::Title { "{route.title()}" }
    }
}

/// The wordmark, linking home.
#[component]
pub fn Logo() -> Element {
    rsx! {
        Link { to: Route::Home {}, class: "flex items-center gap-2 text-lg font-semibold tracking-tight",
            span { class: "grid size-7 place-items-center rounded-lg bg-primary text-primary-content text-sm font-bold",
                "T"
            }
            span { "TechDepó" }
        }
    }
}

/// Notices that sit under the header: API unreachable, catalog version drift.
#[component]
pub fn SystemNotices() -> Element {
    let session = use_session();
    let stale = session
        .config
        .read()
        .as_ref()
        .is_some_and(|c| c.catalog_version != techdepo_catalog::CATALOG_VERSION);
    rsx! {
        if (session.offline)() {
            div { role: "status", class: "alert alert-warning alert-soft rounded-none border-x-0 justify-center text-sm",
                "Can't reach the TechDepó API. You can browse, but sign-in, stock and the cart need the web worker (port 8787)."
            }
        } else if stale {
            div { role: "status", class: "alert alert-info alert-soft rounded-none border-x-0 justify-center text-sm",
                "A newer version of the shop is available. "
                button { class: "link", onclick: move |_| {
                    if let Some(w) = web_sys::window() { let _ = w.location().reload(); }
                }, "Reload" }
            }
        }
    }
}
