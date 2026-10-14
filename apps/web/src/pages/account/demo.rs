//! The demo controls of the customer's order page and the scenario presets (commerce.md §5–6,
//! design.md "Demo controls style"). In order: the simulated clock, the valid next actions,
//! fault toggles for the next step, the hidden truth, a "try it" hint and scenario presets.
//!
//! Only what is possible now is shown (the API sends valid actions and clock steps only);
//! nothing is rendered disabled. Every lever goes through Restate, so a pressed control shows
//! a pending state until the API confirms, and the order page then re-reads the order.

use std::future::Future;

use dioxus::prelude::*;
use techdepo_api_types::demo::{
    AdvanceClockRequest, ClockStep, CreateScenarioRequest, DemoPanel, DemoScenario, HiddenTruth,
    RunDemoActionRequest, SetFaultsRequest,
};
use techdepo_api_types::endpoints::{
    AdvanceDemoClock, CreateScenario, RunDemoAction, SetDemoFaults,
};
use techdepo_api_types::orders::OrderDetail;
use techdepo_api_types::vocab::{PaymentKind, PaymentStatus};
use techdepo_api_types::{
    ApiError, FaultToggles, InvoiceKind, InvoiceStatus, LostOutcome, OrderStatus,
};
use techdepo_domain::{CarrierFault, EmailFault, InvoiceFault, PaymentFault, ShipmentFault};

use crate::api::use_api;
use crate::browser;
use crate::components::toast::{ToastOptions, Toasts, use_toast};
use crate::routes::Route;
use crate::session::use_session;
use crate::ui::format::{datetime, datetime_long, day, duration};
use crate::ui::{DEMO_BUTTON, DemoDrawer, DemoSection, Icon, IconName, Mono};

/// The control whose request is in flight (`None` when idle).
type Busy = Signal<Option<String>>;

/// Runs one demo mutation: pending state, a toast, then `on_changed` so the page re-reads.
async fn perform<T>(
    mut busy: Busy,
    toasts: Toasts,
    on_changed: EventHandler<()>,
    control: String,
    done: Option<String>,
    call: impl Future<Output = Result<T, ApiError>>,
) {
    busy.set(Some(control));
    match call.await {
        Ok(_) => {
            if let Some(done) = done {
                toasts.success(done, ToastOptions::new());
            }
            on_changed.call(());
        }
        Err(error) => toasts.error(error.message, ToastOptions::new()),
    }
    busy.set(None);
}

#[component]
pub fn OrderDemoControls(
    detail: OrderDetail,
    panel: Option<Result<DemoPanel, ApiError>>,
    on_changed: EventHandler<()>,
    /// On the admin order page: presets open the admin order page.
    #[props(default)]
    admin: bool,
) -> Element {
    let busy: Busy = use_signal(|| None);
    let number = detail.order.number.to_string();

    rsx! {
        DemoDrawer { subtitle: format!("Simulate what happens outside the shop · {number}"),
            match panel {
                None => rsx! {
                    for _ in 0..3 {
                        div { class: "h-20 w-full animate-pulse rounded-box bg-amber-100 dark:bg-amber-900/40" }
                    }
                },
                Some(Err(error)) => rsx! {
                    div { role: "alert", class: "rounded-box border border-amber-600/50 p-3 text-sm",
                        p { class: "font-medium", "Demo controls unavailable" }
                        p { "{error.message}" }
                    }
                },
                Some(Ok(panel)) => rsx! {
                    ClockSection { number: number.clone(), panel: panel.clone(), busy, on_changed }
                    ActionsSection { number: number.clone(), panel: panel.clone(), busy, on_changed }
                    FaultsSection { number: number.clone(), panel: panel.clone(), busy, on_changed }
                    HiddenTruthSection { truths: panel.hidden_truth.clone() }
                    TryIt { detail: detail.clone() }
                },
            }
            DemoSection { title: "Scenario presets", icon: IconName::Play,
                p { class: "text-xs text-amber-900/80 dark:text-amber-100/80",
                    "Start a new order already in an interesting state."
                }
                ScenarioPresets { admin }
            }
        }
    }
}

#[component]
fn ClockSection(
    number: String,
    panel: DemoPanel,
    busy: Busy,
    on_changed: EventHandler<()>,
) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let clock = panel.clock;
    rsx! {
        DemoSection { title: "Simulated clock", icon: IconName::Clock,
            div { class: "rounded-box border border-amber-600/40 bg-white/60 p-3 dark:bg-amber-950/40",
                p { class: "text-lg font-semibold tabular-nums", "{datetime_long(clock.simulated_now)}" }
                p { class: "text-xs text-amber-900/80 dark:text-amber-100/80",
                    if clock.offset_secs > 0 {
                        "{duration(clock.offset_secs)} ahead of real time"
                    } else {
                        "Same as real time"
                    }
                }
                if let Some(shipped) = clock.shipped_on {
                    p { class: "mt-1 text-xs text-amber-900/80 dark:text-amber-100/80",
                        "Shipped {day(shipped)}"
                        if let Some(n) = clock.workdays_since_ship {
                            " · {n} workday{plural(n)} since shipment"
                        }
                        if let Some(eta) = clock.eta { " · ETA {day(eta)}" }
                    }
                }
            }
            div { class: "flex flex-wrap gap-2",
                for step in panel.clock_steps.iter().copied() {
                    button {
                        key: "{step.as_str()}",
                        r#type: "button",
                        class: DEMO_BUTTON,
                        disabled: busy().is_some(),
                        onclick: {
                            let number = number.clone();
                            move |_| {
                                let number = number.clone();
                                spawn(async move {
                                    perform(busy, toasts, on_changed, step.as_str().to_owned(), None, async move {
                                        api.send::<AdvanceDemoClock>(&[number.as_str()], &AdvanceClockRequest { step }).await
                                    })
                                    .await;
                                });
                            }
                        },
                        if busy().as_deref() == Some(step.as_str()) {
                            span { class: "loading loading-spinner loading-xs" }
                        } else if step == ClockStep::ShipLastFriday {
                            Icon { name: IconName::CalendarClock, class: "size-3.5" }
                        }
                        "{step.label()}"
                    }
                }
            }
        }
    }
}

#[component]
fn ActionsSection(
    number: String,
    panel: DemoPanel,
    busy: Busy,
    on_changed: EventHandler<()>,
) -> Element {
    let api = use_api();
    let toasts = use_toast();
    rsx! {
        DemoSection { title: "Next actions", icon: IconName::Play,
            if panel.actions.is_empty() {
                p { class: "text-sm text-amber-900/80 dark:text-amber-100/80", "No further steps for this order." }
            }
            for view in panel.actions.iter().cloned() {
                div { key: "{view.action.as_str()}", class: "flex flex-col gap-1",
                    button {
                        r#type: "button",
                        class: "{DEMO_BUTTON} justify-start",
                        disabled: busy().is_some(),
                        onclick: {
                            let number = number.clone();
                            let label = view.label.clone();
                            move |_| {
                                let (number, label) = (number.clone(), label.clone());
                                let action = view.action;
                                spawn(async move {
                                    perform(busy, toasts, on_changed, action.as_str().to_owned(), Some(format!("Done: {label}")), async move {
                                        api.send::<RunDemoAction>(&[number.as_str()], &RunDemoActionRequest { action }).await
                                    })
                                    .await;
                                });
                            }
                        },
                        if busy().as_deref() == Some(view.action.as_str()) {
                            span { class: "loading loading-spinner loading-xs" }
                        } else {
                            Icon { name: IconName::ArrowRight, class: "size-3.5" }
                        }
                        "{view.label}"
                    }
                    if let Some(fault) = view.armed_fault.clone() {
                        span { class: "flex items-center gap-1 pl-1 text-xs font-medium text-amber-800 dark:text-amber-300",
                            Icon { name: IconName::Zap, class: "size-3" }
                            "Armed fault: {fault}"
                        }
                    }
                }
            }
        }
    }
}

/// Email outage lengths offered, in minutes (the API allows 1–60).
const OUTAGE_MINUTES: [u32; 5] = [1, 2, 5, 10, 30];

#[component]
fn FaultsSection(
    number: String,
    panel: DemoPanel,
    busy: Busy,
    on_changed: EventHandler<()>,
) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let faults = panel.faults;
    let status = panel.order.status;

    let set = move |control: &'static str, faults: FaultToggles| {
        let number = number.clone();
        spawn(async move {
            perform(
                busy,
                toasts,
                on_changed,
                control.to_owned(),
                None,
                async move {
                    api.send::<SetDemoFaults>(&[number.as_str()], &SetFaultsRequest { faults })
                        .await
                },
            )
            .await;
        });
    };

    // Only faults for steps still ahead: a fault for a step that already ran can no longer fire.
    // The email outage applies to every email the order still sends.
    let before_payment = status == OrderStatus::AwaitingPayment;
    let before_shipping =
        before_payment || matches!(status, OrderStatus::Paid | OrderStatus::Processing);
    let carrier_matters = before_shipping || status == OrderStatus::Shipped;
    let pending = busy().is_some();
    let lost_outcome = match faults.shipment {
        ShipmentFault::Lost { outcome } => Some(outcome),
        ShipmentFault::None => None,
    };
    let outage = match faults.email {
        EmailFault::Outage { minutes } => Some(minutes),
        EmailFault::None => None,
    };

    rsx! {
        DemoSection { title: "Faults for the next step", icon: IconName::Zap,
            div { class: "flex flex-col gap-3 rounded-box border border-amber-600/40 bg-white/60 p-3 dark:bg-amber-950/40",
                if before_payment {
                    FaultToggle {
                        label: "Charge twice",
                        description: "Payment captures two charges for the same order.",
                        checked: faults.payment == PaymentFault::ChargeTwice,
                        disabled: pending,
                        on_toggle: {
                            let set = set.clone();
                            move |on: bool| set("payment", FaultToggles {
                                payment: if on { PaymentFault::ChargeTwice } else { PaymentFault::None },
                                ..faults
                            })
                        },
                    }
                }
                if before_payment {
                    FaultToggle {
                        label: "Decline the payment",
                        description: "The card is declined; the order stays unpaid and can be retried.",
                        checked: faults.payment == PaymentFault::Declined,
                        disabled: pending,
                        on_toggle: {
                            let set = set.clone();
                            move |on: bool| set("payment", FaultToggles {
                                payment: if on { PaymentFault::Declined } else { PaymentFault::None },
                                ..faults
                            })
                        },
                    }
                }
                if before_payment {
                    FaultToggle {
                        label: "Invoice without billing address",
                        description: "The invoice issued after payment leaves out the billing address.",
                        checked: faults.invoice != InvoiceFault::None,
                        disabled: pending,
                        on_toggle: {
                            let set = set.clone();
                            move |on: bool| set("invoice", FaultToggles {
                                invoice: if on { InvoiceFault::MissingBillingAddress } else { InvoiceFault::None },
                                ..faults
                            })
                        },
                    }
                }
                if before_shipping {
                    FaultToggle {
                        label: "Lose the parcel",
                        description: "Lost in transit. The customer only ever sees \"in transit\".",
                        checked: lost_outcome.is_some(),
                        disabled: pending,
                        on_toggle: {
                            let set = set.clone();
                            move |on: bool| set("shipment", FaultToggles {
                                shipment: if on { ShipmentFault::Lost { outcome: LostOutcome::Gone } } else { ShipmentFault::None },
                                ..faults
                            })
                        },
                    }
                    if let Some(current) = lost_outcome {
                        div { class: "ml-11 flex flex-col gap-1",
                            div { class: "join", role: "group", aria_label: "What a trace reveals",
                                for (outcome , label) in [(LostOutcome::Resurfaces, "Resurfaces"), (LostOutcome::Gone, "Gone")] {
                                    button {
                                        key: "{label}",
                                        r#type: "button",
                                        class: "{DEMO_BUTTON} join-item btn-xs",
                                        class: if current == outcome { "!bg-amber-600 !text-white" },
                                        aria_pressed: if current == outcome { "true" } else { "false" },
                                        disabled: pending,
                                        onclick: {
                                            let set = set.clone();
                                            move |_| set("shipment", FaultToggles { shipment: ShipmentFault::Lost { outcome }, ..faults })
                                        },
                                        "{label}"
                                    }
                                }
                            }
                            span { class: "text-xs text-amber-900/80 dark:text-amber-100/80",
                                {outcome_text(current)}
                            }
                        }
                    }
                }
                if carrier_matters {
                    FaultToggle {
                        label: "Carrier API unavailable",
                        description: "Tracking and trace requests fail, and Restate keeps retrying.",
                        checked: faults.carrier != CarrierFault::None,
                        disabled: pending,
                        on_toggle: {
                            let set = set.clone();
                            move |on: bool| set("carrier", FaultToggles {
                                carrier: if on { CarrierFault::ApiDown } else { CarrierFault::None },
                                ..faults
                            })
                        },
                    }
                }
                FaultToggle {
                    label: "Email provider outage",
                    description: "Outgoing emails fail for a while; Restate retries until the provider recovers.",
                    checked: outage.is_some(),
                    disabled: pending,
                    on_toggle: {
                        let set = set.clone();
                        move |on: bool| set("email", FaultToggles {
                            email: if on { EmailFault::Outage { minutes: 2 } } else { EmailFault::None },
                            ..faults
                        })
                    },
                }
                if let Some(minutes) = outage {
                    label { class: "ml-11 flex items-center gap-2 text-xs",
                        "Outage length"
                        select {
                            class: "select select-xs w-auto border-amber-600/50 bg-white/70 dark:bg-amber-900/40",
                            disabled: pending,
                            onchange: {
                                let set = set.clone();
                                move |e: FormEvent| {
                                    if let Ok(minutes) = e.value().parse::<u32>() {
                                        set("email", FaultToggles { email: EmailFault::Outage { minutes }, ..faults });
                                    }
                                }
                            },
                            for m in OUTAGE_MINUTES.into_iter().chain((!OUTAGE_MINUTES.contains(&minutes)).then_some(minutes)) {
                                option { key: "{m}", value: "{m}", selected: m == minutes, "{m} min" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn FaultToggle(
    label: &'static str,
    description: &'static str,
    checked: bool,
    disabled: bool,
    on_toggle: EventHandler<bool>,
) -> Element {
    rsx! {
        label { class: "flex cursor-pointer items-start gap-3",
            input {
                r#type: "checkbox",
                role: "switch",
                class: "toggle toggle-sm toggle-warning mt-0.5",
                checked,
                disabled,
                onchange: move |e| on_toggle.call(e.checked()),
            }
            span { class: "flex flex-col",
                span { class: "text-sm font-medium", "{label}" }
                span { class: "text-xs text-amber-900/80 dark:text-amber-100/80", "{description}" }
            }
        }
    }
}

fn outcome_text(outcome: LostOutcome) -> &'static str {
    match outcome {
        LostOutcome::Resurfaces => "A carrier trace finds it; delivery the next day.",
        LostOutcome::Gone => "A carrier trace returns no information; a person decides.",
    }
}

#[component]
fn HiddenTruthSection(truths: Vec<HiddenTruth>) -> Element {
    rsx! {
        DemoSection { title: "Hidden truth", icon: IconName::EyeOff,
            div { class: "rounded-box border border-dashed border-amber-700/60 bg-amber-100/70 p-3 text-sm dark:bg-amber-900/30",
                if truths.is_empty() {
                    p { "Nothing hidden: what the customer sees is the whole truth." }
                }
                ul { class: "flex flex-col gap-1.5",
                    for (i , truth) in truths.into_iter().enumerate() {
                        li { key: "{i}", {truth_text(&truth)} }
                    }
                }
            }
        }
    }
}

fn truth_text(truth: &HiddenTruth) -> String {
    match truth {
        HiddenTruth::ParcelLost { outcome } => format!(
            "The parcel is lost (outcome: {}). {}",
            match outcome {
                LostOutcome::Resurfaces => "resurfaces",
                LostOutcome::Gone => "gone",
            },
            outcome_text(*outcome)
        ),
        HiddenTruth::Trace { status } => {
            format!("Carrier trace: {}.", status.label().to_lowercase())
        }
        HiddenTruth::EmailOutage { until } => format!(
            "Email sends fail until {}; Restate keeps retrying.",
            datetime(*until)
        ),
        HiddenTruth::CarrierApiDown => {
            "The carrier API is down for this order; its calls fail and are retried.".to_owned()
        }
    }
}

/// What to email support about, given the order's state: the demo's next move.
#[component]
fn TryIt(detail: OrderDetail) -> Element {
    let session = use_session();
    let Some(address) = session.me().map(|me| me.org.support_address) else {
        return rsx! {};
    };
    let number = detail.order.number;
    let captured = detail
        .payments
        .iter()
        .filter(|p| p.kind == PaymentKind::Charge && p.status == PaymentStatus::Succeeded)
        .count();
    let invoice_without_address = detail.invoices.iter().any(|i| {
        i.kind == InvoiceKind::Invoice
            && i.status == InvoiceStatus::Issued
            && i.missing_billing_address
    });
    let ask = if captured > 1 {
        format!("say you were charged twice for {number}")
    } else if invoice_without_address {
        format!("complain that the invoice for {number} has no billing address")
    } else {
        match detail.order.status {
            OrderStatus::Shipped => format!("ask where {number} is"),
            OrderStatus::AwaitingPayment | OrderStatus::Paid | OrderStatus::Processing => {
                format!("ask to cancel {number}")
            }
            OrderStatus::Delivered => format!("ask whether {number} can be returned"),
            _ => format!("ask about {number}"),
        }
    };

    rsx! {
        DemoSection { title: "Try it", icon: IconName::Mail,
            p { class: "text-sm",
                "Email "
                a { class: "underline", href: "{detail.support_mailto}", Mono { "{address}" } }
                " and {ask}. Or use the "
                Link { to: Route::AccountSupport { subject: format!("Order {number}") }, class: "underline", "web form" }
                "."
            }
        }
    }
}

/// One-click presets (M7): an order for the caller already in the chosen state. Opens it, on
/// the admin order page with `admin`.
#[component]
pub fn ScenarioPresets(#[props(default)] admin: bool) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let nav = navigator();
    let mut busy = use_signal(|| None::<DemoScenario>);

    rsx! {
        ul { class: "flex flex-col gap-1.5",
            for scenario in DemoScenario::ALL.iter().copied() {
                li { key: "{scenario.as_str()}",
                    button {
                        r#type: "button",
                        class: "{DEMO_BUTTON} h-auto w-full justify-start py-1.5 text-left",
                        disabled: busy().is_some(),
                        onclick: move |_| {
                            spawn(async move {
                                busy.set(Some(scenario));
                                // A new key per click: every click is meant to create an order.
                                let request = CreateScenarioRequest { idempotency_key: browser::uuid(), scenario };
                                match api.send::<CreateScenario>(&[], &request).await {
                                    Ok(created) => {
                                        toasts.success(format!("{} created: {}", scenario.label(), created.order.number), ToastOptions::new());
                                        let number = created.order.number.to_string();
                                        nav.push(if admin { Route::AdminOrder { number } } else { Route::AccountOrder { number } });
                                    }
                                    Err(error) => toasts.error(error.message, ToastOptions::new()),
                                }
                                busy.set(None);
                            });
                        },
                        if busy() == Some(scenario) {
                            span { class: "loading loading-spinner loading-xs" }
                        }
                        span { class: "flex flex-col",
                            span { class: "font-medium", "{scenario.label()}" }
                            span { class: "text-xs font-normal opacity-75", {scenario_text(scenario)} }
                        }
                    }
                }
            }
        }
    }
}

fn scenario_text(scenario: DemoScenario) -> &'static str {
    match scenario {
        DemoScenario::Unpaid => "Placed, waiting for payment.",
        DemoScenario::DoubleCharged => "Paid, with two card charges.",
        DemoScenario::InvoiceWithoutAddress => "Paid; the invoice has no billing address.",
        DemoScenario::ShippedLastFriday => "Shipped last Friday; arrives normally.",
        DemoScenario::LostResurfaces => "Shipped last Friday, lost; a trace finds it.",
        DemoScenario::LostGone => "Shipped last Friday, lost for good.",
        DemoScenario::Delivered => "Delivered.",
    }
}

const fn plural(n: u32) -> &'static str {
    if n == 1 { "" } else { "s" }
}
