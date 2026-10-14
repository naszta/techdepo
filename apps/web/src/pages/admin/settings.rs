//! Org settings (design.md "Settings", commerce.md §7) as grouped cards: routing thresholds,
//! the AI reply policy, timings, the holiday list, VAT, the default demo faults, scenario
//! presets and, at the bottom, the demo reset danger zone.
//!
//! The form edits a local copy and saves it whole (`PUT /api/admin/settings` replaces the
//! settings, validated by `OrgSettings::validate`).

use dioxus::prelude::*;
use techdepo_api_types::admin::{
    DemoResetRequest, RESET_CONFIRMATION, SettingsResponse, UpdateSettingsRequest,
};
use techdepo_api_types::endpoints::{GetSettings, ResetDemo, UpdateSettings};
use techdepo_api_types::{FaultToggles, LostOutcome, Validate};
use techdepo_domain::{
    CarrierFault, EmailFault, InvoiceFault, MAX_HOLIDAYS, PaymentFault, ReplyPolicy, ShipmentFault,
};
use time::Date;
use time::macros::format_description;

use crate::api::use_api;
use crate::components::dialog::{Dialog, DialogDescription, DialogTitle};
use crate::components::toast::{ToastOptions, use_toast};
use crate::pages::account::demo::ScenarioPresets;
use crate::pages::{PageHeader, Panel};
use crate::ui::explain::{prob, reply_policy_label};
use crate::ui::format;
use crate::ui::{
    DEMO_BUTTON, DemoFrame, DemoSection, ErrorAlert, Icon, IconName, Mono, PageSkeleton,
};

#[component]
pub fn AdminSettings() -> Element {
    let api = use_api();
    let mut response = use_resource(move || async move { api.get::<GetSettings>(&[]).await });
    rsx! {
        PageHeader { title: "Settings", subtitle: "How this shop routes, replies and simulates. Changes apply to new messages and new orders." }
        match &*response.read() {
            None => rsx! { PageSkeleton { rows: 4 } },
            Some(Err(error)) => rsx! { ErrorAlert { error: error.clone(), on_retry: move |_| response.restart() } },
            Some(Ok(data)) => rsx! { SettingsForm { data: data.clone() } },
        }
    }
}

#[component]
fn SettingsForm(data: SettingsResponse) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let mut saved = use_signal(|| data.settings.clone());
    let mut draft = use_signal(|| data.settings.clone());
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);
    let defaults = data.defaults.clone();
    let dirty = draft() != saved();

    let save = move |_| {
        let request = UpdateSettingsRequest { settings: draft() };
        if let Err(e) = request.validate() {
            error.set(Some(
                e.fields
                    .first()
                    .map_or(e.message.clone(), |f| f.message.clone()),
            ));
            return;
        }
        error.set(None);
        spawn(async move {
            saving.set(true);
            match api.send::<UpdateSettings>(&[], &request).await {
                Ok(response) => {
                    saved.set(response.settings.clone());
                    draft.set(response.settings);
                    toasts.success("Settings saved".to_owned(), ToastOptions::new());
                }
                Err(e) => {
                    error.set(Some(
                        e.fields
                            .first()
                            .map_or(e.message.clone(), |f| f.message.clone()),
                    ));
                    toasts.error(e.message, ToastOptions::new());
                }
            }
            saving.set(false);
        });
    };

    let s = draft();
    rsx! {
        div { class: "flex flex-col gap-5 pb-24",
            div { class: "flex flex-wrap items-center justify-between gap-2",
                p { class: "text-base-content/60",
                    "Shop: {data.org.name} · support address "
                    Mono { "{data.org.support_address}" }
                    if let Some(at) = data.org.expires_at {
                        span { title: "Demo shops expire: the shop and all its data are deleted, and the next sign-in starts a fresh one.",
                            " · deleted on {format::date(at)}"
                        }
                    }
                }
                button {
                    class: "btn btn-ghost btn-sm",
                    disabled: s == defaults,
                    onclick: {
                        let defaults = defaults.clone();
                        move |_| draft.set(defaults.clone())
                    },
                    Icon { name: IconName::Undo, class: "size-3.5" }
                    "Restore defaults"
                }
            }

            Panel { title: "Routing", subtitle: "When the router hands a ticket to a person instead of a workflow or agent.",
                Slider {
                    label: "Escalation threshold",
                    value: s.escalation_threshold,
                    default: defaults.escalation_threshold,
                    help: "Escalate when the most likely intent is less probable than this.",
                    on_change: move |v| draft.write().escalation_threshold = v,
                }
                Slider {
                    label: "Escalation margin",
                    value: s.escalation_margin,
                    default: defaults.escalation_margin,
                    max: 0.5,
                    help: "Escalate when the most likely intent leads the runner-up by less than this.",
                    on_change: move |v| draft.write().escalation_margin = v,
                }
            }

            Panel { title: "AI replies", subtitle: "Applies to text the AI writes; workflow emails come from templates and are always sent.",
                div { class: "flex flex-col gap-2",
                    span { class: "font-medium", "Reply policy" }
                    div { class: "join w-full sm:w-auto", role: "radiogroup", aria_label: "Reply policy",
                        for policy in [ReplyPolicy::AlwaysSend, ReplyPolicy::SendAboveConfidence, ReplyPolicy::AlwaysReview] {
                            button {
                                key: "{reply_policy_label(policy)}",
                                r#type: "button",
                                role: "radio",
                                aria_checked: if s.reply_policy == policy { "true" } else { "false" },
                                class: "btn btn-sm join-item flex-1 sm:flex-none",
                                class: if s.reply_policy == policy { "btn-primary" },
                                onclick: move |_| draft.write().reply_policy = policy,
                                "{reply_policy_label(policy)}"
                            }
                        }
                    }
                    p { class: "text-xs text-base-content/60", {policy_help(s.reply_policy)} }
                }
                if s.reply_policy == ReplyPolicy::SendAboveConfidence {
                    Slider {
                        label: "Reply confidence threshold",
                        value: s.reply_confidence_threshold,
                        default: defaults.reply_confidence_threshold,
                        help: "Send automatically at or above this confidence (the lower of p(intent) and the judge score); below it, a person reviews the draft.",
                        on_change: move |v| draft.write().reply_confidence_threshold = v,
                    }
                }
            }

            Panel { title: "Timing", subtitle: "Real-time durations, short so a demo can show them.",
                div { class: "grid gap-4 sm:grid-cols-3",
                    Minutes {
                        label: "Remind the customer after",
                        secs: s.reminder_after_secs,
                        on_change: move |v| draft.write().reminder_after_secs = v,
                    }
                    Minutes {
                        label: "Then close the ticket after",
                        secs: s.auto_close_after_secs,
                        on_change: move |v| draft.write().auto_close_after_secs = v,
                    }
                    Minutes {
                        label: "Human task SLA",
                        secs: s.human_sla_secs,
                        on_change: move |v| draft.write().human_sla_secs = v,
                    }
                }
            }

            Panel { title: "Workdays", subtitle: "Monday to Friday, except these holidays. They push carrier ETAs and \"+1 workday\", and the shipping policy only counts workdays since shipment. ETAs already given stay.",
                Holidays { holidays: s.holidays.clone(), on_change: move |h| draft.write().holidays = h }
            }

            Panel { title: "Tax",
                VatField { bp: s.vat_rate_bp, on_change: move |bp| draft.write().vat_rate_bp = bp }
            }

            DemoCard { title: "Default faults", icon: IconName::Zap,
                p { class: "text-xs text-amber-900/80 dark:text-amber-100/80",
                    "Armed on every new order. The demo controls on an order change them for that order only."
                }
                FaultDefaults { faults: s.faults, on_change: move |f| draft.write().faults = f }
            }

            DemoCard { title: "Scenario presets", icon: IconName::Play,
                p { class: "text-xs text-amber-900/80 dark:text-amber-100/80",
                    "One click creates an order for you that is already in an interesting state."
                }
                ScenarioPresets { admin: true }
            }

            DangerZone {}
        }

        // The save bar: only when something changed.
        if dirty || error().is_some() {
            div { class: "fixed inset-x-0 bottom-0 z-20 border-t border-base-300 bg-base-100/95 backdrop-blur lg:left-64",
                div { class: "mx-auto flex max-w-7xl flex-wrap items-center gap-3 px-4 py-3 sm:px-6",
                    if let Some(message) = error() {
                        span { class: "flex items-center gap-1.5 text-error",
                            Icon { name: IconName::CircleX, class: "size-4" }
                            "{message}"
                        }
                    } else {
                        span { class: "text-base-content/70", "Unsaved changes" }
                    }
                    span { class: "flex-1" }
                    button { class: "btn btn-ghost btn-sm", onclick: move |_| { draft.set(saved()); error.set(None); }, "Discard" }
                    button { class: "btn btn-primary btn-sm", disabled: saving() || !dirty, onclick: save,
                        if saving() { span { class: "loading loading-spinner loading-xs" } } else { Icon { name: IconName::Save, class: "size-3.5" } }
                        "Save settings"
                    }
                }
            }
        }
    }
}

fn policy_help(policy: ReplyPolicy) -> &'static str {
    match policy {
        ReplyPolicy::AlwaysSend => "Send AI replies as soon as the guardrails pass.",
        ReplyPolicy::SendAboveConfidence => {
            "Send confident replies automatically; hold the rest for a person to review."
        }
        ReplyPolicy::AlwaysReview => "Every AI reply becomes a review task for a person.",
    }
}

/// A `range` from 0 to `max` with its value, the default, and a one-line explanation.
#[component]
fn Slider(
    label: &'static str,
    value: f64,
    default: f64,
    help: &'static str,
    on_change: EventHandler<f64>,
    #[props(default = 1.0)] max: f64,
) -> Element {
    rsx! {
        label { class: "flex flex-col gap-1.5",
            span { class: "flex items-baseline justify-between gap-2",
                span { class: "font-medium", "{label}" }
                span { class: "flex items-baseline gap-2",
                    if (value - default).abs() > f64::EPSILON {
                        span { class: "text-xs text-base-content/50", "default {prob(default)}" }
                    }
                    span { class: "font-mono text-base font-semibold tabular-nums", "{prob(value)}" }
                }
            }
            input {
                r#type: "range",
                class: "range range-primary range-sm w-full",
                min: "0",
                max: "{max}",
                step: "0.01",
                value: "{value}",
                aria_valuetext: "{prob(value)}",
                oninput: move |e| if let Ok(v) = e.value().parse::<f64>() { on_change.call((v * 100.0).round() / 100.0) },
            }
            span { class: "text-xs text-base-content/60", "{help}" }
        }
    }
}

/// A duration in whole minutes (stored in seconds; at least one minute).
#[component]
fn Minutes(label: &'static str, secs: u64, on_change: EventHandler<u64>) -> Element {
    // One `label` per input (a label inside a label is invalid HTML and doubles the name): the
    // field's box is the label, the title above is plain text and the name says the unit.
    rsx! {
        div { class: "flex flex-col gap-1",
            span { class: "font-medium", aria_hidden: "true", "{label}" }
            label { class: "input input-sm w-full",
                input {
                    r#type: "number",
                    aria_label: "{label}, in minutes",
                    min: "1",
                    max: "1440",
                    class: "grow tabular-nums",
                    value: "{secs / 60}",
                    oninput: move |e| if let Ok(m) = e.value().parse::<u64>() { on_change.call(m.clamp(1, 1440) * 60) },
                }
                span { class: "text-base-content/60", "min" }
            }
        }
    }
}

#[component]
fn VatField(bp: u32, on_change: EventHandler<u32>) -> Element {
    rsx! {
        div { class: "flex max-w-xs flex-col gap-1",
            span { class: "font-medium", aria_hidden: "true", "VAT rate" }
            label { class: "input input-sm w-full",
                input {
                    r#type: "number",
                    aria_label: "VAT rate, in percent",
                    min: "0",
                    max: "100",
                    step: "0.01",
                    class: "grow tabular-nums",
                    value: "{f64::from(bp) / 100.0}",
                    oninput: move |e| if let Ok(p) = e.value().parse::<f64>() {
                        on_change.call((p.clamp(0.0, 100.0) * 100.0).round() as u32)
                    },
                }
                span { class: "text-base-content/60", "%" }
            }
            span { class: "text-xs text-base-content/60", "Prices are VAT-inclusive; this sets how invoices split them." }
        }
    }
}

/// The holiday list: a date picker to add one, a removable chip per holiday (kept sorted).
#[component]
fn Holidays(holidays: Vec<Date>, on_change: EventHandler<Vec<Date>>) -> Element {
    let mut picked = use_signal(String::new);
    let parsed = Date::parse(&picked(), format_description!("[year]-[month]-[day]")).ok();
    let full = holidays.len() >= MAX_HOLIDAYS;
    let add = {
        let holidays = holidays.clone();
        move |_| {
            let Some(date) = parsed else { return };
            let mut next = holidays.clone();
            if !next.contains(&date) {
                next.push(date);
                next.sort_unstable();
                on_change.call(next);
            }
            picked.set(String::new());
        }
    };
    rsx! {
        div { class: "flex flex-col gap-3",
            div { class: "flex flex-wrap items-center gap-2",
                input {
                    r#type: "date",
                    aria_label: "Holiday to add",
                    class: "input input-sm w-auto",
                    value: "{picked}",
                    oninput: move |e| picked.set(e.value()),
                }
                button { r#type: "button", class: "btn btn-sm", disabled: parsed.is_none() || full, onclick: add,
                    Icon { name: IconName::Plus, class: "size-3.5" }
                    "Add holiday"
                }
            }
            if holidays.is_empty() {
                p { class: "text-xs text-base-content/60", "No holidays: every Monday to Friday is a workday." }
            } else {
                ul { class: "flex flex-wrap gap-2", aria_label: "Holidays",
                    for (i , date) in holidays.iter().copied().enumerate() {
                        li { key: "{date}", class: "badge badge-outline gap-1 py-3 pr-1",
                            "{format::day(date)} {date.year()}"
                            button {
                                r#type: "button",
                                class: "btn btn-ghost btn-xs btn-circle",
                                aria_label: "Remove {date}",
                                onclick: {
                                    let mut next = holidays.clone();
                                    next.remove(i);
                                    move |_| on_change.call(next.clone())
                                },
                                Icon { name: IconName::X, class: "size-3" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// An amber card: a simulation lever, not shop configuration (design.md, demo controls style).
#[component]
fn DemoCard(title: &'static str, icon: IconName, children: Element) -> Element {
    rsx! {
        DemoFrame { DemoSection { title, icon, {children} } }
    }
}

#[component]
fn FaultDefaults(faults: FaultToggles, on_change: EventHandler<FaultToggles>) -> Element {
    let lost = match faults.shipment {
        ShipmentFault::Lost { outcome } => Some(outcome),
        ShipmentFault::None => None,
    };
    let outage = match faults.email {
        EmailFault::Outage { minutes } => Some(minutes),
        EmailFault::None => None,
    };
    rsx! {
        div { class: "flex flex-col gap-3",
            div { class: "flex flex-col gap-1",
                span { class: "text-sm font-medium", "Payment" }
                div { class: "join", role: "radiogroup", aria_label: "Payment fault",
                    for (fault , label) in [(PaymentFault::None, "Normal"), (PaymentFault::ChargeTwice, "Charge twice"), (PaymentFault::Declined, "Decline")] {
                        button {
                            key: "{label}",
                            r#type: "button",
                            role: "radio",
                            aria_checked: if faults.payment == fault { "true" } else { "false" },
                            class: "{DEMO_BUTTON} join-item btn-xs",
                            class: if faults.payment == fault { "!bg-amber-600 !text-white" },
                            onclick: move |_| on_change.call(FaultToggles { payment: fault, ..faults }),
                            "{label}"
                        }
                    }
                }
            }
            Toggle {
                label: "Invoice without billing address",
                description: "The invoice issued after payment leaves out the billing address.",
                checked: faults.invoice != InvoiceFault::None,
                on_toggle: move |on: bool| on_change.call(FaultToggles {
                    invoice: if on { InvoiceFault::MissingBillingAddress } else { InvoiceFault::None },
                    ..faults
                }),
            }
            Toggle {
                label: "Lose the parcel",
                description: "Lost in transit; the customer only ever sees \"in transit\".",
                checked: lost.is_some(),
                on_toggle: move |on: bool| on_change.call(FaultToggles {
                    shipment: if on { ShipmentFault::Lost { outcome: LostOutcome::Gone } } else { ShipmentFault::None },
                    ..faults
                }),
            }
            if let Some(current) = lost {
                div { class: "join ml-11", role: "radiogroup", aria_label: "What a trace reveals",
                    for (outcome , label) in [(LostOutcome::Resurfaces, "Resurfaces"), (LostOutcome::Gone, "Gone")] {
                        button {
                            key: "{label}",
                            r#type: "button",
                            role: "radio",
                            aria_checked: if current == outcome { "true" } else { "false" },
                            class: "{DEMO_BUTTON} join-item btn-xs",
                            class: if current == outcome { "!bg-amber-600 !text-white" },
                            onclick: move |_| on_change.call(FaultToggles { shipment: ShipmentFault::Lost { outcome }, ..faults }),
                            "{label}"
                        }
                    }
                }
            }
            Toggle {
                label: "Carrier API unavailable",
                description: "Tracking and trace requests fail; Restate keeps retrying.",
                checked: faults.carrier != CarrierFault::None,
                on_toggle: move |on: bool| on_change.call(FaultToggles {
                    carrier: if on { CarrierFault::ApiDown } else { CarrierFault::None },
                    ..faults
                }),
            }
            Toggle {
                label: "Email provider outage",
                description: "Outgoing emails fail for a while; Restate retries until the provider recovers.",
                checked: outage.is_some(),
                on_toggle: move |on: bool| on_change.call(FaultToggles {
                    email: if on { EmailFault::Outage { minutes: 2 } } else { EmailFault::None },
                    ..faults
                }),
            }
            if let Some(minutes) = outage {
                label { class: "ml-11 flex items-center gap-2 text-xs",
                    "Outage length"
                    select {
                        class: "select select-xs w-auto border-amber-600/50 bg-white/70 dark:bg-amber-900/40",
                        onchange: move |e: FormEvent| if let Ok(minutes) = e.value().parse::<u32>() {
                            on_change.call(FaultToggles { email: EmailFault::Outage { minutes }, ..faults })
                        },
                        for m in [1u32, 2, 5, 10, 30].into_iter().chain((![1, 2, 5, 10, 30].contains(&minutes)).then_some(minutes)) {
                            option { key: "{m}", value: "{m}", selected: m == minutes, "{m} min" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn Toggle(
    label: &'static str,
    description: &'static str,
    checked: bool,
    on_toggle: EventHandler<bool>,
) -> Element {
    rsx! {
        label { class: "flex cursor-pointer items-start gap-3",
            input {
                r#type: "checkbox",
                role: "switch",
                class: "toggle toggle-sm toggle-warning mt-0.5",
                checked,
                onchange: move |e| on_toggle.call(e.checked()),
            }
            span { class: "flex flex-col",
                span { class: "text-sm font-medium", "{label}" }
                span { class: "text-xs text-amber-900/80 dark:text-amber-100/80", "{description}" }
            }
        }
    }
}

/// Demo reset: wipes orders, tickets, invoices, tasks, traces and the email log, restores
/// default stock. Asks the admin to type `RESET`.
#[component]
fn DangerZone() -> Element {
    let mut open = use_signal(|| false);
    rsx! {
        section { class: "flex flex-col gap-3 rounded-box border border-error/50 bg-base-100 p-5",
            div { class: "flex flex-wrap items-start justify-between gap-4",
                div { class: "flex flex-col gap-1",
                    h2 { class: "flex items-center gap-2 text-base font-semibold",
                        Icon { name: IconName::TriangleAlert, class: "size-4 text-error" }
                        "Danger zone"
                    }
                    p { class: "max-w-xl text-base-content/70",
                        "Reset the demo: deletes every order, ticket, invoice, payment, human task, trace and logged email of this shop and restores the default stock. Settings, members, carts, builds and addresses stay."
                    }
                }
                button { class: "btn btn-error btn-outline btn-sm", onclick: move |_| open.set(true),
                    Icon { name: IconName::Trash, class: "size-3.5" }
                    "Reset demo data"
                }
            }
        }
        if open() {
            ResetDialog { on_close: move |_| open.set(false) }
        }
    }
}

#[component]
fn ResetDialog(on_close: EventHandler<()>) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let mut typed = use_signal(String::new);
    let mut pending = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let ready = typed() == RESET_CONFIRMATION;

    let reset = move |_| {
        let request = DemoResetRequest { confirm: typed() };
        if let Err(e) = request.validate() {
            error.set(e.fields.first().map(|f| f.message.clone()));
            return;
        }
        spawn(async move {
            pending.set(true);
            match api.send::<ResetDemo>(&[], &request).await {
                Ok(done) => {
                    toasts.success(
                        format!(
                            "Demo reset: {} orders, {} tickets, {} tasks and {} emails deleted; {} products restocked",
                            done.orders, done.tickets, done.tasks, done.emails, done.products_restocked
                        ),
                        ToastOptions::new(),
                    );
                    on_close.call(());
                }
                Err(e) => {
                    error.set(Some(e.message.clone()));
                    toasts.error(e.message, ToastOptions::new());
                }
            }
            pending.set(false);
        });
    };

    rsx! {
        Dialog {
            open: Some(true),
            on_open_change: move |open: bool| if !open { on_close.call(()) },
            class: "max-w-md",
            DialogTitle { "Reset the demo?" }
            DialogDescription { "This cannot be undone. Running workflows for deleted orders find nothing left to act on." }
            label { class: "mt-4 flex flex-col gap-1",
                span { "Type " Mono { class: "font-semibold", "{RESET_CONFIRMATION}" } " to confirm" }
                input {
                    class: "input w-full font-mono",
                    class: if error().is_some() { "input-error" },
                    autocomplete: "off",
                    value: "{typed}",
                    oninput: move |e| typed.set(e.value()),
                }
                if let Some(message) = error() {
                    span { class: "text-xs text-error", "{message}" }
                }
            }
            div { class: "mt-6 flex justify-end gap-2",
                button { class: "btn btn-ghost", onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn btn-error", disabled: !ready || pending(), onclick: reset,
                    if pending() { span { class: "loading loading-spinner loading-xs" } }
                    "Reset demo data"
                }
            }
        }
    }
}
