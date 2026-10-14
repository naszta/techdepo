//! Mode-agnostic auth guards and the header's user control.
//!
//! These read [`AuthState`] only, so they work the same with Clerk and with the dev login.
//! Clerk's own components are only rendered when `auth_mode` is `clerk` (they need
//! `ClerkProvider` above them).
//!
//! Switching shops needs nothing here: `ClerkBridge` follows Clerk's active org, so a switch
//! re-runs `/api/me` and the cart and stock reload for the new org.

use dioxus::prelude::*;
use techdepo_api_types::ErrorCode;
use techdepo_api_types::session::AuthMode;

use crate::routes::Route;
use crate::session::{AuthState, use_session};
use crate::ui::icons::{Icon, IconName};
use crate::ui::{ErrorAlert, PageSkeleton};

/// Renders its children only when signed in.
#[component]
pub fn SignedIn(children: Element) -> Element {
    let session = use_session();
    if session.is_signed_in() {
        rsx! { {children} }
    } else {
        rsx! {}
    }
}

/// Renders its children only when signed out (not while loading).
#[component]
pub fn SignedOut(children: Element) -> Element {
    let session = use_session();
    if matches!(*session.auth.read(), AuthState::SignedOut) {
        rsx! { {children} }
    } else {
        rsx! {}
    }
}

/// The current path, for "sign in and come back".
pub fn current_path() -> String {
    use_route::<Route>().to_string()
}

/// Guards a page: a skeleton while loading, a sign-in prompt when signed out, and (with
/// `admin`) a "not allowed" note for members who are not `org:admin`.
#[component]
pub fn RequireAuth(#[props(default)] admin: bool, children: Element) -> Element {
    let session = use_session();
    let here = current_path();
    let auth = session.auth.read().clone();
    match auth {
        AuthState::Loading => rsx! { PageSkeleton {} },
        AuthState::SignedOut => rsx! {
            div { class: "mx-auto max-w-md py-16",
                div { class: "card card-border border-base-300 bg-base-100",
                    div { class: "card-body items-center text-center gap-4",
                        div { class: "rounded-full bg-primary/10 p-3 text-primary",
                            Icon { name: IconName::User, class: "size-6" }
                        }
                        h2 { class: "card-title", "Sign in to continue" }
                        p { class: "text-base-content/70",
                            "Your cart, builds and orders belong to your account."
                        }
                        Link { class: "btn btn-primary", to: Route::sign_in_to(here), "Sign in" }
                    }
                }
            }
        },
        AuthState::Failed(error)
            if error.code == ErrorCode::Forbidden
                && session.auth_mode() == Some(AuthMode::Clerk) =>
        {
            rsx! { ChooseOrganization {} }
        }
        AuthState::Failed(error) => rsx! {
            ErrorAlert { error, on_retry: move |_| { spawn(session.refresh_me()); } }
        },
        AuthState::SignedIn(me) if admin && !me.is_admin() => rsx! {
            div { role: "alert", class: "alert alert-warning alert-soft",
                Icon { name: IconName::ShieldCheck, class: "size-5" }
                span { "Only shop admins (org:admin) can open the admin area." }
            }
        },
        AuthState::SignedIn(_) => rsx! { {children} },
    }
}

/// Clerk session without an active organization: every user works inside an org.
#[component]
fn ChooseOrganization() -> Element {
    rsx! {
        div { class: "mx-auto flex max-w-lg flex-col items-center gap-4 py-12 text-center",
            h2 { class: "text-xl font-semibold", "Choose your shop" }
            p { class: "text-base-content/70",
                "Every TechDepó account works inside a shop (a Clerk organization). Create one, or pick one you belong to."
            }
            dioxus_clerk::prelude::OrganizationList { options: shops_only() }
        }
    }
}

/// Clerk options for the org widgets: no personal account, since a session without an
/// active org is refused (`403`) by the API.
fn shops_only() -> serde_json::Value {
    serde_json::json!({ "hidePersonal": true })
}

/// The user control at the end of the header: Clerk's `OrganizationSwitcher` (switch or
/// create a shop) and `UserButton`, or the dev menu.
#[component]
pub fn UserControl() -> Element {
    let session = use_session();
    let here = current_path();
    let auth = session.auth.read().clone();
    match (session.auth_mode(), auth) {
        (_, AuthState::Loading) => rsx! { div { class: "skeleton size-9 rounded-full" } },
        (_, AuthState::SignedOut) => rsx! {
            Link { class: "btn btn-sm btn-primary", to: Route::sign_in_to(here), "Sign in" }
        },
        (Some(AuthMode::Clerk), _) => rsx! {
            div { class: "flex min-w-0 items-center gap-2",
                dioxus_clerk::prelude::OrganizationSwitcher {
                    class: "flex min-w-0 items-center",
                    options: shops_only(),
                    fallback: rsx! { div { class: "skeleton h-8 w-32" } },
                }
                dioxus_clerk::prelude::UserButton {}
            }
        },
        (_, AuthState::SignedIn(me)) => {
            let name = me
                .user
                .name
                .clone()
                .unwrap_or_else(|| me.user.email.clone());
            let initial = name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            rsx! {
                div { class: "dropdown dropdown-end",
                    div {
                        tabindex: "0",
                        role: "button",
                        class: "btn btn-ghost btn-circle",
                        aria_label: "Account menu for {name}",
                        div { class: "avatar avatar-placeholder",
                            div { class: "bg-neutral text-neutral-content w-9 rounded-full",
                                span { class: "text-sm", "{initial}" }
                            }
                        }
                    }
                    ul {
                        tabindex: "0",
                        class: "dropdown-content menu z-30 mt-2 w-64 rounded-box border border-base-300 bg-base-100 p-2",
                        li { class: "menu-title",
                            span { class: "truncate", "{name}" }
                            span { class: "truncate font-normal text-base-content/60", "{me.org.name} · {me.role.label()}" }
                        }
                        li { Link { to: Route::AccountHome {}, Icon { name: IconName::User }, "Account" } }
                        li { Link { to: Route::AccountOrders {}, Icon { name: IconName::Package }, "Orders" } }
                        if me.is_admin() {
                            li { Link { to: Route::AdminOverview {}, Icon { name: IconName::Dashboard }, "Admin" } }
                        }
                        li {
                            button { onclick: move |_| session.sign_out(),
                                Icon { name: IconName::LogOut }
                                "Sign out"
                            }
                        }
                    }
                }
            }
        }
        (_, AuthState::Failed(_)) => rsx! {
            button { class: "btn btn-sm btn-ghost", onclick: move |_| session.sign_out(), "Sign out" }
        },
    }
}
