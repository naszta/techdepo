//! The PC builder: a slot table (category → part → price → status, Choose/Swap) and a sticky
//! summary with the verdict, the issues and their fixes, the wattage bar, the total, Save and
//! Add build to cart. On mobile the summary follows the table and a bottom bar keeps the verdict
//! and total in view.
//!
//! Everything compatibility-related runs in the browser (`techdepo_compat`); only saving and
//! the cart go through the API.

use dioxus::prelude::*;
use techdepo_api_types::account::{CreateBuildRequest, UpdateBuildRequest};
use techdepo_api_types::endpoints::{CreateBuild, UpdateBuild};
use techdepo_api_types::{ApiError, Category, ErrorCode, Product, ProductId, Validate};
use techdepo_compat::{
    Fix, REQUIRED_CATEGORIES, Report, Severity, Slot, check_build, suggest_fixes,
};

use crate::api::Api;
use crate::components::dialog::{Dialog, DialogDescription, DialogTitle};
use crate::components::toast::{ToastOptions, use_toast};
use crate::pages::PageHeader;
use crate::routes::Route;
use crate::session::{Session, use_session};
use crate::state::build::{BuildStore, BuildView, build_of, catalog, owner};
use crate::state::{use_build, use_cart, use_stock};
use crate::ui::product::{category_icon, key_specs};
use crate::ui::{CompatBadge, Icon, IconName, MoneyText, SeverityIcon, StockBadge};

#[component]
pub fn Builder() -> Element {
    let build = use_build();
    let session = use_session();
    let cart = use_cart();
    let toasts = use_toast();
    let nav = navigator();
    let view = build.view;
    let mut picking = use_signal(|| None::<Slot>);
    let mut saving = use_signal(|| false);
    let mut busy = use_signal(|| false);

    let fixes = use_memo(move || {
        let view = view.read();
        let parts: Vec<ProductId> = view.slots.iter().map(|(_, p)| p.id.clone()).collect();
        suggest_fixes(catalog(), &build_of(&parts), &view.report)
    });

    let draft = build.draft.read().clone();
    let saved = build.saved_for(session.me().map(|me| owner(&me)).as_deref());
    let title = saved
        .as_ref()
        .map_or_else(|| "Build a PC".to_owned(), |s| s.name.clone());
    let subtitle = match (&saved, draft.dirty) {
        (Some(_), true) => "Unsaved changes",
        (Some(_), false) => "Saved",
        (None, _) => "Pick parts slot by slot. Every change is checked instantly.",
    };

    let add_build_to_cart = move |_| {
        if !session.is_signed_in() {
            nav.push(Route::sign_in_to("/builder"));
            return;
        }
        spawn(async move {
            busy.set(true);
            let result = async {
                let id = save_draft(session, build, None).await?;
                match cart.add_build(id).await {
                    // The saved build is gone (deleted in another tab): save the draft anew.
                    Err(e) if e.code == ErrorCode::NotFound => {
                        build.forget_saved();
                        let id = save_draft(session, build, None).await?;
                        cart.add_build(id).await
                    }
                    other => other,
                }
            }
            .await;
            busy.set(false);
            match result {
                Ok(_) => {
                    toasts.success("Your build is in the cart".into(), ToastOptions::new());
                    nav.push(Route::Cart {});
                }
                Err(e) => toasts.error(e.message, ToastOptions::new()),
            }
        });
    };

    rsx! {
        PageHeader {
            title,
            subtitle: subtitle.to_string(),
            actions: rsx! {
                if !view.read().is_empty() {
                    button { class: "btn btn-ghost btn-sm", onclick: move |_| build.clear(), "New build" }
                }
                button {
                    class: "btn btn-sm",
                    disabled: view.read().is_empty(),
                    onclick: move |_| {
                        if session.is_signed_in() {
                            saving.set(true);
                        } else {
                            nav.push(Route::sign_in_to("/builder"));
                        }
                    },
                    Icon { name: IconName::Save }
                    "Save build"
                }
            },
        }

        div { class: "grid gap-6 pb-24 lg:grid-cols-[1fr_22rem] lg:pb-0",
            SlotTable { view: view(), on_choose: move |slot| picking.set(Some(slot)) }
            div { id: "build-summary", class: "lg:sticky lg:top-24 lg:self-start",
                Summary {
                    view: view(),
                    fixes: fixes(),
                    busy: busy(),
                    on_add_to_cart: add_build_to_cart,
                }
            }
        }

        // Mobile: the verdict and total stay in view.
        if !view.read().is_empty() {
            div { class: "fixed inset-x-0 bottom-0 z-20 border-t border-base-300 bg-base-100 px-4 py-3 lg:hidden",
                div { class: "flex items-center justify-between gap-3",
                    CompatBadge { verdict: view.read().report.verdict, small: true }
                    MoneyText { amount: view.read().total, class: "font-semibold" }
                    a { href: "#build-summary", class: "btn btn-sm btn-primary", "Summary" }
                }
            }
        }

        if let Some(slot) = picking() {
            PartPicker { slot, on_close: move |_| picking.set(None) }
        }
        if saving() {
            SaveDialog { on_close: move |_| saving.set(false) }
        }
    }
}

/// Saves the draft (create, or update when it belongs to the customer's saved build and
/// changed) and returns the saved build's ID. `name` names a new build. A saved build that is
/// gone (deleted elsewhere) is forgotten and the draft created anew.
async fn save_draft(
    session: Session,
    build: BuildStore,
    name: Option<String>,
) -> Result<techdepo_api_types::BuildId, ApiError> {
    let api = Api::new(session);
    let owner = session.me().map(|me| owner(&me));
    let draft = build.draft.peek().clone();
    let saved = build.saved_for(owner.as_deref());
    if let Some(saved) = &saved {
        if !draft.dirty && name.as_ref().is_none_or(|n| *n == saved.name) {
            return Ok(saved.id.clone());
        }
        let body = UpdateBuildRequest {
            name: name.clone(),
            product_ids: Some(draft.parts.clone()),
        };
        body.validate()?;
        match api.send::<UpdateBuild>(&[saved.id.as_str()], &body).await {
            Ok(detail) => {
                build.mark_saved(&detail, owner);
                return Ok(detail.id);
            }
            Err(e) if e.code == ErrorCode::NotFound => build.forget_saved(),
            Err(e) => return Err(e),
        }
    }
    let body = CreateBuildRequest {
        // One key per draft version (api.md: "a UUID per form load"), kept across attempts.
        idempotency_key: build.create_key(),
        name: name
            .or_else(|| saved.map(|s| s.name))
            .unwrap_or_else(|| "My build".to_owned()),
        product_ids: draft.parts.clone(),
    };
    body.validate()?;
    let detail = api.send::<CreateBuild>(&[], &body).await?;
    build.mark_saved(&detail, owner);
    Ok(detail.id)
}

/// Rows in slot order: every category once, memory and storage once per part plus an "add
/// another" row.
#[derive(Clone, PartialEq)]
enum Row {
    Part(Slot, &'static Product),
    Empty(Slot),
    AddAnother(Slot),
}

fn rows(view: &BuildView) -> Vec<Row> {
    let mut rows = Vec::new();
    for &category in Category::ALL {
        let parts: Vec<_> = view
            .slots
            .iter()
            .filter(|(slot, _)| slot.category == category)
            .collect();
        if parts.is_empty() {
            rows.push(Row::Empty(Slot::new(category, 0)));
            continue;
        }
        rows.extend(parts.iter().map(|(slot, p)| Row::Part(*slot, p)));
        if Slot::is_multi(category) {
            rows.push(Row::AddAnother(Slot::new(category, parts.len())));
        }
    }
    rows
}

#[component]
fn SlotTable(view: BuildView, on_choose: EventHandler<Slot>) -> Element {
    let build = use_build();
    let report = view.report.clone();
    rsx! {
        div { class: "self-start overflow-x-auto rounded-box border border-base-300",
            // Tighter cells on phones, where five columns of padding would squeeze the part names.
            table { class: "table max-sm:table-sm",
                thead {
                    tr {
                        th { span { class: "sr-only sm:not-sr-only", "Part" } }
                        th { "Choice" }
                        th { class: "hidden text-right sm:table-cell", "Price" }
                        th { class: "w-10", span { class: "sr-only", "Status" } }
                        th { class: "w-px" }
                    }
                }
                tbody {
                    for row in rows(&view) {
                        match row {
                            Row::Part(slot, product) => rsx! {
                                tr { key: "{slot}",
                                    td { SlotLabel { category: slot.category } }
                                    td {
                                        Link { to: Route::Product { id: product.id.to_string() }, class: "font-medium hover:text-primary",
                                            "{product.name}"
                                        }
                                        div { class: "text-xs text-base-content/60",
                                            "{key_specs(product).join(\" · \")}"
                                        }
                                        MoneyText { amount: product.price, class: "text-sm sm:hidden" }
                                    }
                                    td { class: "hidden text-right sm:table-cell", MoneyText { amount: product.price } }
                                    td { PartStatus { report: report.clone(), id: product.id.clone() } }
                                    td {
                                        div { class: "flex justify-end gap-1",
                                            button { class: "btn btn-ghost btn-xs", onclick: move |_| on_choose.call(slot), "Swap" }
                                            button {
                                                class: "btn btn-ghost btn-xs btn-square",
                                                aria_label: "Remove {product.name}",
                                                onclick: move |_| build.remove(slot),
                                                Icon { name: IconName::X }
                                            }
                                        }
                                    }
                                }
                            },
                            Row::Empty(slot) => rsx! {
                                tr { key: "{slot}",
                                    td { SlotLabel { category: slot.category } }
                                    td { class: "text-base-content/50",
                                        if REQUIRED_CATEGORIES.contains(&slot.category) { "Required" } else { "Optional" }
                                    }
                                    td { class: "hidden sm:table-cell" }
                                    td {}
                                    td {
                                        div { class: "flex justify-end",
                                            button { class: "btn btn-primary btn-xs", onclick: move |_| on_choose.call(slot), "Choose" }
                                        }
                                    }
                                }
                            },
                            Row::AddAnother(slot) => rsx! {
                                tr { key: "{slot}-add",
                                    td {}
                                    td { colspan: "4",
                                        button { class: "btn btn-ghost btn-xs", onclick: move |_| on_choose.call(slot),
                                            Icon { name: IconName::Plus }
                                            if slot.category == Category::Memory { "Add another memory kit" } else { "Add another drive" }
                                        }
                                    }
                                }
                            },
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SlotLabel(category: Category) -> Element {
    rsx! {
        div { class: "flex items-center gap-2 whitespace-nowrap",
            span { class: "grid size-8 place-items-center rounded-lg bg-primary/8 text-primary",
                Icon { name: category_icon(category), class: "size-4" }
            }
            span { class: "sr-only font-medium sm:not-sr-only", "{category.label()}" }
        }
    }
}

#[component]
fn PartStatus(report: Report, id: ProductId) -> Element {
    match report.worst_for(&id) {
        Some(severity) => rsx! { SeverityIcon { severity } },
        None => {
            rsx! { Icon { name: IconName::Check, class: "size-4 text-success", label: "Compatible".to_string() } }
        }
    }
}

#[component]
fn Summary(
    view: BuildView,
    fixes: Vec<Fix>,
    busy: bool,
    on_add_to_cart: EventHandler<()>,
) -> Element {
    let build = use_build();
    let report = view.report.clone();
    let blocking = report
        .issues
        .iter()
        .filter(|i| i.severity >= Severity::Warning)
        .count();
    let notes: Vec<_> = report.with_severity(Severity::Note).cloned().collect();

    rsx! {
        div { class: "card card-border border-base-300 bg-base-100",
            div { class: "card-body gap-4",
                div { class: "flex items-center justify-between",
                    h2 { class: "card-title text-base", "Summary" }
                    if !view.is_empty() {
                        CompatBadge { verdict: report.verdict }
                    }
                }
                if view.is_empty() {
                    p { class: "text-sm text-base-content/70",
                        "Start with a CPU or a case. We'll check every part you add against the rest."
                    }
                }
                if !view.is_empty() && !view.missing.is_empty() {
                    div { class: "text-sm",
                        span { class: "text-base-content/60", "Still needed: " }
                        "{view.missing.iter().map(|c| c.label()).collect::<Vec<_>>().join(\", \")}"
                    }
                }

                if blocking > 0 {
                    div { class: "flex flex-col gap-2",
                        for issue in report.issues.iter().filter(|i| i.severity >= Severity::Warning).cloned() {
                            details { class: "rounded-box border border-base-300 p-3 text-sm",
                                open: issue.severity == Severity::Error,
                                summary { class: "flex cursor-pointer list-none gap-2",
                                    SeverityIcon { severity: issue.severity, class: "size-4 mt-0.5 shrink-0" }
                                    span { "{issue.message}" }
                                }
                                for fix in fixes.iter().filter(|f| f.rule == issue.rule).cloned() {
                                    FixSuggestion { fix }
                                }
                            }
                        }
                    }
                }
                if !notes.is_empty() {
                    details { class: "text-sm",
                        summary { class: "cursor-pointer text-base-content/70", "{notes.len()} notes" }
                        ul { class: "mt-2 flex flex-col gap-2",
                            for note in notes {
                                li { class: "flex gap-2",
                                    SeverityIcon { severity: note.severity, class: "size-4 mt-0.5" }
                                    span { "{note.message}" }
                                }
                            }
                        }
                    }
                }

                if !view.is_empty() {
                    WattageBar { report: report.clone() }
                }

                div { class: "flex items-baseline justify-between border-t border-base-300 pt-3",
                    span { class: "text-sm text-base-content/70", "Total" }
                    MoneyText { amount: view.total, class: "text-2xl font-semibold" }
                }
                button {
                    class: "btn btn-primary",
                    disabled: view.is_empty() || busy,
                    onclick: move |_| on_add_to_cart.call(()),
                    if busy {
                        span { class: "loading loading-spinner loading-sm" }
                    } else {
                        Icon { name: IconName::Cart }
                    }
                    "Add build to cart"
                }
                if !view.is_empty() && !report.is_compatible() {
                    p { class: "text-xs text-base-content/60",
                        "You can still order an incompatible build; checkout asks you to confirm."
                    }
                }
                if !view.is_empty() {
                    button { class: "btn btn-ghost btn-xs self-center", onclick: move |_| build.clear(), "Clear build" }
                }
            }
        }
    }
}

/// "Swap X for: A, B, C" (or "Add: …") buttons that apply a fix.
#[component]
fn FixSuggestion(fix: Fix) -> Element {
    let build = use_build();
    let replaces = fix.replaces.as_ref().and_then(|id| catalog().get(id));
    let lead = match replaces {
        Some(p) => format!("Swap {} for", p.name),
        None => format!("Add a {}", category_noun(fix.slot.category)),
    };
    rsx! {
        div { class: "mt-3 flex flex-col gap-1.5",
            span { class: "text-xs font-medium uppercase tracking-wider text-base-content/60", "Fix · {lead}" }
            div { class: "flex flex-col gap-1.5",
                for candidate in fix.candidates.iter().filter_map(|id| catalog().get(id)).take(3) {
                    button {
                        key: "{candidate.id}",
                        class: "btn btn-sm btn-outline h-auto min-h-8 justify-between gap-2 py-1 text-left font-normal whitespace-normal",
                        onclick: {
                            let slot = fix.slot;
                            move |_| build.set(slot, candidate)
                        },
                        "{candidate.name}"
                        MoneyText { amount: candidate.price, class: "text-base-content/60" }
                    }
                }
            }
        }
    }
}

/// Estimated draw against the PSU rating.
#[component]
fn WattageBar(report: Report) -> Element {
    let estimate = report.estimated_watts;
    let (max, color, text) = match (report.psu_watts, report.headroom_pct) {
        (Some(psu), Some(headroom)) => {
            let color = if headroom < 0 {
                "progress-error"
            } else if headroom < techdepo_compat::power::HEADROOM_WARNING_PCT {
                "progress-warning"
            } else {
                "progress-primary"
            };
            (
                psu,
                color,
                format!("≈ {estimate} W of {psu} W · {headroom}% headroom"),
            )
        }
        _ => (
            estimate.max(1) * 3 / 2,
            "progress-primary",
            format!("≈ {estimate} W estimated · no power supply yet"),
        ),
    };
    rsx! {
        div { class: "flex flex-col gap-1.5",
            div { class: "flex items-center gap-1.5 text-sm",
                Icon { name: IconName::Zap, class: "size-4 text-base-content/60" }
                span { "Power" }
            }
            progress {
                class: "progress {color} w-full",
                value: "{estimate.min(max)}",
                max: "{max}",
                aria_label: "Estimated power draw",
            }
            span { class: "text-xs text-base-content/60 tabular-nums", "{text}" }
        }
    }
}

/// The candidate list for one slot, each checked as if placed into it.
#[component]
fn PartPicker(slot: Slot, on_close: EventHandler<()>) -> Element {
    let build = use_build();
    let stock = use_stock();
    let mut only_compatible = use_signal(|| false);
    let parts = build.parts();
    let current = build_of(&parts).get(slot).map(|p| p.id.clone());

    let mut candidates: Vec<(&'static Product, Option<Severity>, Option<String>)> = catalog()
        .by_category(slot.category)
        .iter()
        .map(|candidate| {
            let mut trial = build_of(&parts);
            let _ = trial.set(slot, candidate);
            let report = check_build(&trial);
            let issue = report
                .issues
                .iter()
                .filter(|i| i.involves(&candidate.id) && !i.rule.is_absence())
                .max_by_key(|i| i.severity);
            (
                candidate,
                issue.map(|i| i.severity),
                issue.map(|i| i.message.clone()),
            )
        })
        .collect();
    candidates.sort_by_key(|(p, worst, _)| (*worst == Some(Severity::Error), p.price));
    let has_errors = candidates
        .iter()
        .any(|(_, w, _)| *w == Some(Severity::Error));

    rsx! {
        Dialog {
            open: Some(true),
            on_open_change: move |open: bool| if !open { on_close.call(()) },
            class: "max-w-2xl",
            DialogTitle { "Choose a {category_noun(slot.category)}" }
            DialogDescription { "Each part is checked against the rest of your build." }
            if has_errors {
                label { class: "label mt-2 text-sm",
                    input {
                        r#type: "checkbox",
                        class: "toggle toggle-sm toggle-primary",
                        checked: only_compatible(),
                        onchange: move |e| only_compatible.set(e.checked()),
                    }
                    "Only compatible parts"
                }
            }
            ul { class: "mt-3 flex max-h-[60vh] flex-col gap-2 overflow-y-auto",
                for (candidate , worst , reason) in candidates.into_iter().filter(|(_, w, _)| !only_compatible() || *w != Some(Severity::Error)) {
                    li { key: "{candidate.id}",
                        class: "flex items-start gap-3 rounded-box border border-base-300 p-3",
                        class: if current.as_ref() == Some(&candidate.id) { "border-primary" },
                        div { class: "mt-0.5",
                            match worst {
                                Some(severity) => rsx! { SeverityIcon { severity } },
                                None => rsx! { Icon { name: IconName::Check, class: "size-4 text-success", label: "Compatible".to_string() } },
                            }
                        }
                        div { class: "flex min-w-0 flex-1 flex-col gap-0.5",
                            span { class: "font-medium", "{candidate.name}" }
                            span { class: "text-xs text-base-content/60", "{key_specs(candidate).join(\" · \")}" }
                            if let Some(reason) = reason {
                                span { class: "text-xs text-base-content/80", "{reason}" }
                            }
                            if let Some(stock) = stock.get(&candidate.id) {
                                div { class: "mt-1", StockBadge { stock } }
                            }
                        }
                        div { class: "flex flex-col items-end gap-2",
                            MoneyText { amount: candidate.price, class: "font-semibold" }
                            button {
                                class: "btn btn-sm",
                                class: if worst != Some(Severity::Error) { "btn-primary" },
                                onclick: move |_| {
                                    build.set(slot, candidate);
                                    on_close.call(());
                                },
                                if current.as_ref() == Some(&candidate.id) { "Keep" } else { "Choose" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Names the draft and saves it.
#[component]
fn SaveDialog(on_close: EventHandler<()>) -> Element {
    let session = use_session();
    let build = use_build();
    let toasts = use_toast();
    let initial = build
        .draft
        .peek()
        .saved
        .as_ref()
        .map_or_else(|| "My build".to_owned(), |s| s.name.clone());
    let mut name = use_signal(|| initial);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);

    let save = move |e: FormEvent| {
        e.prevent_default();
        spawn(async move {
            busy.set(true);
            match save_draft(session, build, Some(name())).await {
                Ok(_) => {
                    toasts.success("Build saved".into(), ToastOptions::new());
                    on_close.call(());
                }
                Err(e) => error.set(Some(
                    e.fields
                        .first()
                        .map_or_else(|| e.message.clone(), |f| f.message.clone()),
                )),
            }
            busy.set(false);
        });
    };

    rsx! {
        Dialog {
            open: Some(true),
            on_open_change: move |open: bool| if !open { on_close.call(()) },
            DialogTitle { "Save build" }
            DialogDescription { "Saved builds are in your account, ready to reopen or add to the cart." }
            form {
                class: "mt-4 flex flex-col gap-4",
                onsubmit: save,
                fieldset { class: "fieldset",
                    label { class: "fieldset-legend", r#for: "build-name", "Name" }
                    input {
                        id: "build-name",
                        class: "input w-full",
                        class: if error().is_some() { "input-error" },
                        maxlength: "80",
                        value: "{name}",
                        oninput: move |e| {
                            name.set(e.value());
                            error.set(None);
                        },
                    }
                    if let Some(message) = error() {
                        p { class: "label text-error", "{message}" }
                    }
                }
                div { class: "flex justify-end gap-2",
                    button { r#type: "button", class: "btn btn-ghost", onclick: move |_| on_close.call(()), "Cancel" }
                    button { r#type: "submit", class: "btn btn-primary", disabled: busy(),
                        if busy() { span { class: "loading loading-spinner loading-sm" } }
                        "Save"
                    }
                }
            }
        }
    }
}

/// A category as a noun inside a sentence: "graphics card", but "CPU cooler", not "cpu cooler".
fn category_noun(category: Category) -> String {
    category.label().to_lowercase().replace("cpu", "CPU")
}
