//! Muhaha (`OrgFeature::Muhaha` orgs only): the operator's page. The model of each AI task
//! (app-wide, `PUT /api/admin/muhaha/models`), and every shop on the deployment with its
//! reset, clear and reseed actions. Without the feature the sidebar hides it, the page is a
//! not-found, and so are its API routes (`Access::Operator`).
//!
//! Every destructive action asks first: resetting every shop and clearing one take the typed
//! `RESET` confirmation of the demo reset; reseeding one shop's stock takes a confirm dialog.

use dioxus::prelude::*;
use techdepo_api_types::admin::{DemoResetRequest, DemoResetResponse, RESET_CONFIRMATION};
use techdepo_api_types::endpoints::{
    ClearOrg, GetModels, ListMuhahaOrgs, ReseedOrg, ResetAllOrgs, UpdateModels,
};
use techdepo_api_types::muhaha::{ModelsResponse, MuhahaOrg, UpdateModelsRequest};
use techdepo_api_types::{
    ApiError, Empty, ModelChoice, ModelKind, ModelProvider, ModelSettings, ModelSlot, OrgFeature,
    Validate,
};

use super::common::TableWrap;
use crate::api::use_api;
use crate::components::dialog::{Dialog, DialogDescription, DialogTitle};
use crate::components::toast::{ToastOptions, use_toast};
use crate::pages::{PageHeader, Panel};
use crate::routes::Route;
use crate::session::use_session;
use crate::ui::format;
use crate::ui::{EmptyState, ErrorAlert, Icon, IconName, Mono, PageSkeleton, Tag};

#[component]
pub fn AdminMuhaha() -> Element {
    let session = use_session();
    if !session.has_feature(OrgFeature::Muhaha) {
        let path = Route::AdminMuhaha {}.to_string();
        return rsx! {
            EmptyState { message: "There is nothing at {path}.", icon: IconName::Search,
                Link { to: Route::AdminOverview {}, class: "btn btn-primary btn-sm", "Go to the admin overview" }
            }
        };
    }
    rsx! {
        PageHeader { title: "Muhaha", subtitle: "The operator's tools: the models behind every AI task, and every shop on this deployment." }
        div { class: "flex flex-col gap-5 pb-24",
            Models {}
            Shops {}
        }
    }
}

fn first_message(e: &ApiError) -> String {
    e.fields
        .first()
        .map_or_else(|| e.message.clone(), |f| f.message.clone())
}

// --- Models ---------------------------------------------------------------------------------

#[component]
fn Models() -> Element {
    let api = use_api();
    let mut response = use_resource(move || async move { api.get::<GetModels>(&[]).await });
    rsx! {
        match &*response.read() {
            None => rsx! { PageSkeleton { rows: 3 } },
            Some(Err(error)) => rsx! { ErrorAlert { error: error.clone(), on_retry: move |_| response.restart() } },
            Some(Ok(data)) => rsx! { ModelsForm { data: data.clone() } },
        }
    }
}

#[component]
fn ModelsForm(data: ModelsResponse) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let mut current = use_signal(|| data.clone());
    let mut draft = use_signal(|| data.settings.clone());
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);
    let state = current();
    let dirty = draft() != state.settings;
    let available: Vec<ModelProvider> = state
        .providers
        .iter()
        .filter(|p| p.available)
        .map(|p| p.provider)
        .collect();

    let mut save = move |settings: Option<ModelSettings>| {
        let request = UpdateModelsRequest { settings };
        if let Err(e) = request.validate() {
            error.set(Some(first_message(&e)));
            return;
        }
        error.set(None);
        spawn(async move {
            saving.set(true);
            match api.send::<UpdateModels>(&[], &request).await {
                Ok(saved) => {
                    draft.set(saved.settings.clone());
                    current.set(saved);
                    toasts.success(
                        "Models saved: new work uses them".to_owned(),
                        ToastOptions::new(),
                    );
                }
                Err(e) => {
                    error.set(Some(first_message(&e)));
                    toasts.error(e.message, ToastOptions::new());
                }
            }
            saving.set(false);
        });
    };

    let status = match (state.stored, state.updated_at) {
        (true, Some(at)) => format!("Saved {}", format::ago(at)),
        (true, None) => "Saved".to_owned(),
        (false, _) => "The deployment's defaults".to_owned(),
    };
    let s = draft();
    rsx! {
        Panel {
            title: "Models",
            subtitle: "Which model answers each AI task, for every shop. Saved settings apply to work that starts afterwards: a running workflow keeps the models it started with.",
            actions: rsx! { Tag { icon: IconName::Bot, "{status}" } },
            div { class: "flex flex-wrap items-center gap-2 text-xs",
                span { class: "text-base-content/60", "Providers:" }
                for p in state.providers.iter().copied() {
                    span {
                        key: "{p.provider.as_str()}",
                        class: if p.available { "badge badge-sm badge-success badge-soft gap-1" } else { "badge badge-sm badge-ghost gap-1 opacity-70" },
                        title: if p.available { "Available" } else { p.provider.secrets_hint().map_or("Not available".to_owned(), |s| format!("Set {s} on the services worker")) },
                        Icon { name: if p.available { IconName::Check } else { IconName::X }, class: "size-3" }
                        "{p.provider.label()}"
                    }
                }
            }
            div { class: "flex flex-col divide-y divide-base-300",
                for slot in ModelSlot::ALL.iter().copied() {
                    SlotRow {
                        key: "{slot.as_str()}",
                        slot,
                        choice: s.get(slot).cloned(),
                        default: state.defaults.get(slot).cloned(),
                        available: available.clone(),
                        on_change: move |choice: Option<ModelChoice>| draft.write().set(slot, choice),
                    }
                }
            }
            if let Some(message) = error() {
                div { role: "alert", class: "alert alert-error alert-soft text-sm", "{message}" }
            }
            div { class: "flex flex-wrap items-center justify-end gap-2",
                button {
                    class: "btn btn-ghost btn-sm",
                    disabled: s == state.defaults,
                    title: "Put the defaults in the form; nothing changes until you save.",
                    onclick: {
                        let defaults = state.defaults.clone();
                        move |_| draft.set(defaults.clone())
                    },
                    Icon { name: IconName::Undo, class: "size-3.5" }
                    "Restore defaults"
                }
                if state.stored {
                    button {
                        class: "btn btn-ghost btn-sm",
                        disabled: saving(),
                        title: "Forget the saved settings: the deployment's defaults apply, and follow its configuration from now on.",
                        onclick: move |_| save(None),
                        "Use the deployment's defaults"
                    }
                }
                button {
                    class: "btn btn-primary btn-sm",
                    disabled: !dirty || saving(),
                    onclick: move |_| save(Some(draft())),
                    if saving() { span { class: "loading loading-spinner loading-xs" } }
                    Icon { name: IconName::Save, class: "size-3.5" }
                    "Save models"
                }
            }
        }
    }
}

/// One task's model: provider picker, model ID (suggestions per provider, any ID accepted).
#[component]
fn SlotRow(
    slot: ModelSlot,
    choice: Option<ModelChoice>,
    default: Option<ModelChoice>,
    available: Vec<ModelProvider>,
    on_change: EventHandler<Option<ModelChoice>>,
) -> Element {
    let kind = slot.kind();
    let optional = slot == ModelSlot::ClassifierFallback;
    let list_id = format!("models-{}", slot.as_str());
    let provider_value = choice
        .as_ref()
        .map_or("off", |c| c.provider.as_str())
        .to_owned();
    let suggestions: &[&str] = choice
        .as_ref()
        .map_or(&[], |c| c.provider.suggested_models(kind));
    let unavailable = choice
        .as_ref()
        .is_some_and(|c| !available.contains(&c.provider));
    let mode = match (&choice, kind) {
        (Some(c), ModelKind::Classifier) if c.provider.is_decision_model(&c.model) => {
            Some("Decision model: calibrated probabilities")
        }
        (Some(c), ModelKind::Classifier) if c.provider != ModelProvider::Mock => {
            Some("JSON mode: the model reports its own confidence")
        }
        _ => None,
    };
    let default_text = match &default {
        Some(d) => format!("Default: {}", d.address()),
        None => "Default: off".to_owned(),
    };
    let current_choice = choice.clone();
    rsx! {
        div { class: "grid gap-2 py-3 md:grid-cols-[minmax(0,16rem)_minmax(0,1fr)] md:gap-4",
            div { class: "flex flex-col gap-0.5",
                span { class: "flex items-center gap-2 text-sm font-medium",
                    "{slot.label()}"
                    Tag { "{kind.label()}" }
                }
                span { class: "text-xs text-base-content/60", "{slot.description()}" }
            }
            div { class: "flex flex-col gap-1",
                div { class: "flex flex-col gap-2 sm:flex-row",
                    select {
                        class: "select select-sm sm:w-56",
                        class: if unavailable { "select-error" },
                        aria_label: "{slot.label()} provider",
                        onchange: move |e: FormEvent| {
                            let value = e.value();
                            if value == "off" {
                                on_change.call(None);
                                return;
                            }
                            if let Ok(provider) = value.parse::<ModelProvider>() {
                                let model = provider
                                    .suggested_models(kind)
                                    .first()
                                    .copied()
                                    .unwrap_or_default();
                                on_change.call(Some(ModelChoice::new(provider, model)));
                            }
                        },
                        if optional {
                            option { value: "off", selected: provider_value == "off", "Off" }
                        }
                        for provider in ModelProvider::ALL.iter().copied() {
                            option {
                                key: "{provider.as_str()}",
                                value: "{provider.as_str()}",
                                selected: provider_value == provider.as_str(),
                                disabled: !available.contains(&provider),
                                if available.contains(&provider) {
                                    "{provider.label()}"
                                } else {
                                    "{provider.label()} (not configured)"
                                }
                            }
                        }
                    }
                    if let Some(c) = current_choice.clone() {
                        input {
                            class: "input input-sm grow font-mono",
                            aria_label: "{slot.label()} model",
                            list: "{list_id}",
                            disabled: c.provider == ModelProvider::Mock,
                            autocomplete: "off",
                            spellcheck: false,
                            value: "{c.model}",
                            oninput: move |e: FormEvent| {
                                on_change.call(Some(ModelChoice::new(c.provider, e.value().trim())));
                            },
                        }
                        datalist { id: "{list_id}",
                            for m in suggestions.iter() {
                                option { key: "{m}", value: "{m}" }
                            }
                        }
                    }
                }
                div { class: "flex flex-wrap gap-x-3 text-xs text-base-content/60",
                    if unavailable {
                        span { class: "text-error", "This provider is not configured in this deployment: calls fail and escalate." }
                    }
                    if let Some(mode) = mode {
                        span { "{mode}" }
                    }
                    if choice != default {
                        span { "{default_text}" }
                    }
                }
            }
        }
    }
}

// --- Shops ----------------------------------------------------------------------------------

/// A destructive action waiting for its confirmation.
#[derive(Debug, Clone, PartialEq)]
enum Danger {
    /// Reset every shop.
    ResetAll,
    /// Clear one shop's data.
    Clear(MuhahaOrg),
    /// Reseed one shop's stock.
    Reseed(MuhahaOrg),
}

#[component]
fn Shops() -> Element {
    let api = use_api();
    let mut orgs = use_resource(move || async move { api.get::<ListMuhahaOrgs>(&[]).await });
    let mut danger = use_signal(|| None::<Danger>);
    rsx! {
        Panel {
            title: "Shops",
            subtitle: "Every org on this deployment. Resetting deletes orders, tickets, invoices, payments, tasks, traces and logged emails, and restores the default stock; settings, members, carts, builds and addresses stay.",
            actions: rsx! {
                button { class: "btn btn-error btn-outline btn-sm", onclick: move |_| danger.set(Some(Danger::ResetAll)),
                    Icon { name: IconName::Trash, class: "size-3.5" }
                    "Reset every shop"
                }
            },
            match &*orgs.read() {
                None => rsx! { PageSkeleton { rows: 3 } },
                Some(Err(error)) => rsx! { ErrorAlert { error: error.clone(), on_retry: move |_| orgs.restart() } },
                Some(Ok(list)) if list.orgs.is_empty() => rsx! {
                    EmptyState { message: "No shops yet.", icon: IconName::Store }
                },
                Some(Ok(list)) => rsx! {
                    p { class: "text-base-content/60", "{list.orgs.len()} shops" }
                    TableWrap {
                        table { class: "table table-sm",
                            thead {
                                tr {
                                    th { "Shop" }
                                    th { "Created" }
                                    th { class: "text-right", "Members" }
                                    th { class: "text-right", "Orders" }
                                    th { class: "text-right", "Tickets" }
                                    th { "Stock" }
                                    th { class: "text-right", "Actions" }
                                }
                            }
                            tbody {
                                for org in list.orgs.iter().cloned() {
                                    OrgRow { key: "{org.id}", org, danger }
                                }
                            }
                        }
                    }
                },
            }
        }
        if let Some(action) = danger() {
            DangerDialog {
                action,
                on_close: move |_| danger.set(None),
                on_done: move |_| {
                    danger.set(None);
                    orgs.restart();
                },
            }
        }
    }
}

#[component]
fn OrgRow(org: MuhahaOrg, danger: Signal<Option<Danger>>) -> Element {
    let created = format::date(org.created_at);
    let (clear, reseed) = (org.clone(), org.clone());
    rsx! {
        tr {
            td {
                div { class: "flex flex-col",
                    span { class: "flex items-center gap-2 font-medium",
                        "{org.name}"
                        if org.current { Tag { "you" } }
                    }
                    span { class: "text-xs text-base-content/60",
                        Mono { "{org.public_id}" }
                        " · "
                        Mono { "{org.id}" }
                    }
                }
            }
            td { class: "whitespace-nowrap",
                "{created}"
                if let Some(at) = org.expires_at {
                    span { class: "block text-xs text-base-content/60", "expires {format::relative(at)}" }
                }
            }
            td { class: "text-right tabular-nums", "{org.members}" }
            td { class: "text-right tabular-nums", "{org.orders}" }
            td { class: "text-right tabular-nums", "{org.tickets}" }
            td {
                if org.catalog_current {
                    span { class: "badge badge-sm badge-ghost", "Current" }
                } else {
                    span {
                        class: "badge badge-sm badge-warning badge-soft",
                        title: "Seeded from an older catalog: products added since have no stock until it is reseeded.",
                        "Outdated"
                    }
                }
            }
            td {
                div { class: "flex justify-end gap-1",
                    button {
                        class: "btn btn-ghost btn-xs",
                        onclick: move |_| danger.set(Some(Danger::Reseed(reseed.clone()))),
                        Icon { name: IconName::Boxes, class: "size-3.5" }
                        "Reseed stock"
                    }
                    button {
                        class: "btn btn-ghost btn-xs text-error",
                        onclick: move |_| danger.set(Some(Danger::Clear(clear.clone()))),
                        Icon { name: IconName::Trash, class: "size-3.5" }
                        "Clear data"
                    }
                }
            }
        }
    }
}

fn deleted(r: &DemoResetResponse) -> String {
    format!(
        "{} orders, {} tickets, {} tasks and {} emails deleted",
        r.orders, r.tickets, r.tasks, r.emails
    )
}

/// The confirmation of a [`Danger`], which then runs it. Reset and clear take the typed
/// confirmation; reseeding a confirm button.
#[component]
fn DangerDialog(action: Danger, on_close: EventHandler<()>, on_done: EventHandler<()>) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let mut typed = use_signal(String::new);
    let mut pending = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let needs_typing = !matches!(action, Danger::Reseed(_));
    let ready = !needs_typing || typed() == RESET_CONFIRMATION;

    let (title, description, label) = match &action {
        Danger::ResetAll => (
            "Reset every shop?".to_owned(),
            "Every shop on this deployment loses its orders, tickets, invoices, payments, tasks, traces and logged emails, and gets its default stock back, one shop after another. This cannot be undone. Running workflows for deleted orders find nothing left to act on.".to_owned(),
            "Reset every shop",
        ),
        Danger::Clear(org) => (
            format!("Clear {}?", org.name),
            format!(
                "Deletes the orders, tickets, invoices, payments, tasks, traces and logged emails of {} ({} orders, {} tickets). Its stock stays as it is. This cannot be undone.",
                org.name, org.orders, org.tickets
            ),
            "Clear data",
        ),
        Danger::Reseed(org) => (
            format!("Reseed the stock of {}?", org.name),
            format!(
                "Every catalog product of {} goes back to its default stock, including products added since it was seeded. Stock changed by hand or by orders is overwritten.",
                org.name
            ),
            "Reseed stock",
        ),
    };

    let run = {
        let action = action.clone();
        move |_| {
            let confirm = DemoResetRequest { confirm: typed() };
            if needs_typing && let Err(e) = confirm.validate() {
                error.set(Some(first_message(&e)));
                return;
            }
            let action = action.clone();
            spawn(async move {
                pending.set(true);
                let outcome = match &action {
                    Danger::ResetAll => api.send::<ResetAllOrgs>(&[], &confirm).await.map(|done| {
                        let total = done.total();
                        format!(
                            "{} shops reset: {}; {} products restocked",
                            done.orgs.len(),
                            deleted(&total),
                            total.products_restocked
                        )
                    }),
                    Danger::Clear(org) => api
                        .send::<ClearOrg>(&[org.id.as_str()], &confirm)
                        .await
                        .map(|done| format!("{} cleared: {}", org.name, deleted(&done))),
                    Danger::Reseed(org) => api
                        .send::<ReseedOrg>(&[org.id.as_str()], &Empty {})
                        .await
                        .map(|done| {
                            format!(
                                "{} reseeded: {} products restocked",
                                org.name, done.products_restocked
                            )
                        }),
                };
                match outcome {
                    Ok(message) => {
                        toasts.success(message, ToastOptions::new());
                        on_done.call(());
                    }
                    Err(e) => {
                        error.set(Some(first_message(&e)));
                        toasts.error(e.message, ToastOptions::new());
                    }
                }
                pending.set(false);
            });
        }
    };

    rsx! {
        Dialog {
            open: Some(true),
            on_open_change: move |open: bool| if !open { on_close.call(()) },
            class: "max-w-md",
            DialogTitle { "{title}" }
            DialogDescription { "{description}" }
            if needs_typing {
                label { class: "mt-4 flex flex-col gap-1",
                    span { "Type " Mono { class: "font-semibold", "{RESET_CONFIRMATION}" } " to confirm" }
                    input {
                        class: "input w-full font-mono",
                        class: if error().is_some() { "input-error" },
                        autocomplete: "off",
                        value: "{typed}",
                        oninput: move |e| typed.set(e.value()),
                    }
                }
            }
            if let Some(message) = error() {
                span { class: "mt-2 block text-xs text-error", "{message}" }
            }
            div { class: "mt-6 flex justify-end gap-2",
                button { class: "btn btn-ghost", onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn btn-error", disabled: !ready || pending(), onclick: run,
                    if pending() { span { class: "loading loading-spinner loading-xs" } }
                    "{label}"
                }
            }
        }
    }
}
