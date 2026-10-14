//! The root component: providers, config bootstrap and the router.

use dioxus::prelude::*;
use techdepo_api_types::session::AuthMode;

use crate::components::toast::ToastProvider;
use crate::routes::Route;
use crate::session::{AuthState, ClerkBridge, Session};
use crate::state::{BuildStore, CartStore, StockStore};

#[component]
pub fn App() -> Element {
    let session = use_context_provider(Session::new);
    let cart = use_context_provider(|| CartStore::new(session));
    let stock = use_context_provider(|| StockStore::new(session));
    use_context_provider(BuildStore::load);

    use_hook(|| {
        spawn(session.bootstrap());
    });

    // Server state follows the session: load on sign-in, forget on sign-out.
    let signed_in_as = use_memo(move || match &*session.auth.read() {
        AuthState::SignedIn(me) => Some((me.user.id.clone(), me.org.id.clone())),
        _ => None,
    });
    use_effect(move || {
        if signed_in_as().is_some() {
            spawn(async move {
                let _ = cart.refresh().await;
            });
            spawn(async move {
                let _ = stock.refresh().await;
            });
        } else {
            cart.clear_local();
            stock.clear_local();
        }
    });

    let config = session.config.read().clone();
    rsx! {
        ToastProvider {
            match config {
                None => rsx! { Booting {} },
                Some(config) if config.auth_mode == AuthMode::Clerk => {
                    let key = config.clerk_publishable_key.clone().unwrap_or_default();
                    rsx! {
                        dioxus_clerk::prelude::ClerkProvider {
                            publishable_key: key,
                            sign_in_url: "/sign-in",
                            after_sign_out_url: "/",
                            ClerkBridge {}
                            Router::<Route> {}
                        }
                    }
                }
                Some(_) => rsx! { Router::<Route> {} },
            }
        }
    }
}

/// Shown for the moment `/api/config` takes.
#[component]
fn Booting() -> Element {
    rsx! {
        div { class: "flex min-h-screen flex-col",
            div { class: "h-7 bg-base-200" }
            div { class: "border-b border-base-300 h-16" }
            div { class: "mx-auto w-full max-w-[1200px] px-6 py-8 flex flex-col gap-4",
                div { class: "skeleton h-10 w-1/2" }
                div { class: "skeleton h-40 w-full" }
            }
        }
    }
}
