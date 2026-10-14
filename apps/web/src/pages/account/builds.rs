//! Saved builds: open one in the builder, add it to the cart, or delete it.

use dioxus::prelude::*;
use techdepo_api_types::account::BuildSummary;
use techdepo_api_types::endpoints::{DeleteBuild, GetBuild, ListBuilds};
use techdepo_api_types::{BuildId, Empty};

use crate::api::use_api;
use crate::components::toast::{ToastOptions, use_toast};
use crate::pages::PageHeader;
use crate::routes::Route;
use crate::session::use_session;
use crate::state::build::owner;
use crate::state::{use_build, use_cart};
use crate::ui::format::date;
use crate::ui::{CompatBadge, EmptyState, ErrorAlert, Icon, IconName, MoneyText, PageSkeleton};

#[component]
pub fn AccountBuilds() -> Element {
    let api = use_api();
    let mut builds = use_resource(move || async move { api.get::<ListBuilds>(&[]).await });

    rsx! {
        PageHeader {
            title: "Saved builds",
            actions: rsx! {
                Link { to: Route::Builder {}, class: "btn btn-primary btn-sm", Icon { name: IconName::Wrench } "Open the builder" }
            },
        }
        match &*builds.read() {
            None => rsx! { PageSkeleton {} },
            Some(Err(error)) => rsx! { ErrorAlert { error: error.clone(), on_retry: move |_| builds.restart() } },
            Some(Ok(list)) if list.builds.is_empty() => rsx! {
                EmptyState { message: "No saved builds yet.", icon: IconName::Wrench,
                    Link { to: Route::Builder {}, class: "btn btn-primary btn-sm", "Build a PC" }
                }
            },
            Some(Ok(list)) => rsx! {
                div { class: "grid gap-4 md:grid-cols-2",
                    for build in list.builds.clone() {
                        BuildCard { key: "{build.id}", summary: build, on_deleted: move |_| builds.restart() }
                    }
                }
            },
        }
    }
}

#[component]
fn BuildCard(summary: BuildSummary, on_deleted: EventHandler<()>) -> Element {
    let api = use_api();
    let store = use_build();
    let session = use_session();
    let cart = use_cart();
    let toasts = use_toast();
    let nav = navigator();
    let mut busy = use_signal(|| false);
    // Deleting takes a second click ("Confirm"), so a stray click loses nothing.
    let mut confirming = use_signal(|| false);
    let build = summary;
    let id = build.id.clone();

    let open = {
        let id = id.clone();
        move |_| {
            let id = id.clone();
            spawn(async move {
                busy.set(true);
                match api.get::<GetBuild>(&[id.as_str()]).await {
                    Ok(detail) => {
                        store.open(&detail, session.me().map(|me| owner(&me)));
                        nav.push(Route::Builder {});
                    }
                    Err(e) => toasts.error(e.message, ToastOptions::new()),
                }
                busy.set(false);
            });
        }
    };
    let add_to_cart = {
        let id = id.clone();
        move |_| {
            let id: BuildId = id.clone();
            spawn(async move {
                busy.set(true);
                match cart.add_build(id).await {
                    Ok(_) => toasts.success("Build added to your cart".into(), ToastOptions::new()),
                    Err(e) => toasts.error(e.message, ToastOptions::new()),
                }
                busy.set(false);
            });
        }
    };
    let delete = move |_| {
        if !confirming() {
            confirming.set(true);
            return;
        }
        let id = id.clone();
        spawn(async move {
            busy.set(true);
            match api.send::<DeleteBuild>(&[id.as_str()], &Empty {}).await {
                Ok(_) => {
                    toasts.info("Build deleted".into(), ToastOptions::new());
                    // The builder's draft may point at it: keep the parts, forget the build.
                    store.forget_deleted(&id);
                    on_deleted.call(());
                }
                Err(e) => toasts.error(e.message, ToastOptions::new()),
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "card card-border border-base-300 bg-base-100",
            div { class: "card-body gap-3",
                div { class: "flex items-start justify-between gap-2",
                    div {
                        h2 { class: "card-title text-base", "{build.name}" }
                        p { class: "text-sm text-base-content/60",
                            "{build.part_count} parts · saved {date(build.updated_at)}"
                        }
                    }
                    CompatBadge { verdict: build.verdict, small: true }
                }
                MoneyText { amount: build.total, class: "text-lg font-semibold" }
                div { class: "card-actions justify-end",
                    if confirming() {
                        button { class: "btn btn-ghost btn-sm", onclick: move |_| confirming.set(false), "Keep" }
                    }
                    button {
                        class: if confirming() { "btn btn-error btn-sm" } else { "btn btn-ghost btn-sm" },
                        disabled: busy(),
                        onclick: delete,
                        Icon { name: IconName::Trash }
                        if confirming() { "Confirm delete" } else { "Delete" }
                    }
                    button { class: "btn btn-sm", disabled: busy(), onclick: add_to_cart,
                        Icon { name: IconName::Cart }
                        "Add to cart"
                    }
                    button { class: "btn btn-primary btn-sm", disabled: busy(), onclick: open, "Open" }
                }
            }
        }
    }
}
