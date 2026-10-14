//! The admin actions of the order page: cancel an unpaid order, start a refund, open a carrier
//! investigation. Only the ones that apply now are shown (`AdminOrderActions`); each opens a
//! dialog and goes through Restate, so the page shows a pending state until the API confirms.
//! Refunds and investigations are workflows started one-way: their records appear moments
//! later, which the page's follow-up reloads pick up.

use dioxus::prelude::*;
use techdepo_api_types::admin::{AdminCancelOrderRequest, AdminOrderDetail};
use techdepo_api_types::cases::{InitiateRefundRequest, OpenInvestigationRequest};
use techdepo_api_types::endpoints::{CancelOrder, InitiateRefund, OpenInvestigation};
use techdepo_api_types::orders::{InvoiceSummary, PaymentEntry};
use techdepo_api_types::vocab::{PaymentKind, PaymentStatus, RefundReason};
use techdepo_api_types::{FieldError, Money, PaymentId, Validate};

use crate::api::use_api;
use crate::browser;
use crate::components::dialog::{Dialog, DialogDescription, DialogTitle};
use crate::components::toast::{ToastOptions, use_toast};
use crate::pages::admin::common::{TextField, error_for, euros_input, non_blank, parse_euros};
use crate::ui::format::datetime;
use crate::ui::{Icon, IconName, Mono};

/// Which action dialog is open.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionDialog {
    /// Cancel an unpaid order.
    Cancel,
    /// Start a refund.
    Refund,
    /// Open a carrier investigation.
    Investigate,
    /// Void and reissue an invoice.
    VoidReissue(InvoiceSummary),
}

/// The header buttons for the actions that apply now.
#[component]
pub fn OrderActions(detail: AdminOrderDetail, on_open: EventHandler<ActionDialog>) -> Element {
    let actions = detail.actions;
    if !actions.can_cancel && !actions.can_refund && !actions.can_investigate {
        return rsx! {};
    }
    rsx! {
        div { class: "flex flex-wrap gap-2",
            if actions.can_refund {
                button { class: "btn btn-sm", onclick: move |_| on_open.call(ActionDialog::Refund),
                    Icon { name: IconName::Scale, class: "size-3.5" }
                    "Start refund"
                }
            }
            if actions.can_investigate {
                button { class: "btn btn-sm", onclick: move |_| on_open.call(ActionDialog::Investigate),
                    Icon { name: IconName::Search, class: "size-3.5" }
                    "Open investigation"
                }
            }
            if actions.can_cancel {
                button { class: "btn btn-sm btn-outline btn-error", onclick: move |_| on_open.call(ActionDialog::Cancel),
                    Icon { name: IconName::Ban, class: "size-3.5" }
                    "Cancel order"
                }
            }
        }
    }
}

#[component]
pub fn ActionDialogView(
    dialog: ActionDialog,
    detail: AdminOrderDetail,
    on_close: EventHandler<()>,
    on_done: EventHandler<()>,
) -> Element {
    match dialog {
        ActionDialog::Cancel => rsx! { CancelDialog { detail, on_close, on_done } },
        ActionDialog::Refund => rsx! { RefundDialog { detail, on_close, on_done } },
        ActionDialog::Investigate => rsx! { InvestigateDialog { detail, on_close, on_done } },
        ActionDialog::VoidReissue(_) => rsx! {},
    }
}

#[component]
fn CancelDialog(
    detail: AdminOrderDetail,
    on_close: EventHandler<()>,
    on_done: EventHandler<()>,
) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let reason = use_signal(String::new);
    let mut errors = use_signal(Vec::<FieldError>::new);
    let mut pending = use_signal(|| false);
    let number = detail.order.order.number.to_string();

    let submit = {
        let number = number.clone();
        move |_| {
            let request = AdminCancelOrderRequest {
                reason: non_blank(&reason()),
            };
            if let Err(error) = request.validate() {
                errors.set(error.fields);
                return;
            }
            let number = number.clone();
            spawn(async move {
                pending.set(true);
                match api.send::<CancelOrder>(&[number.as_str()], &request).await {
                    Ok(_) => {
                        toasts.success(format!("{number} cancelled"), ToastOptions::new());
                        on_done.call(());
                    }
                    Err(error) => {
                        errors.set(error.fields.clone());
                        toasts.error(error.message, ToastOptions::new());
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
            class: "max-w-lg",
            DialogTitle { "Cancel " Mono { "{number}" } "?" }
            DialogDescription { "The order is unpaid, so nothing is refunded. Reserved stock goes back on the shelf and the customer is emailed." }
            div { class: "mt-4",
                TextField { label: "Reason (optional, for the timeline)", value: reason, multiline: true, rows: 2, error: error_for(&errors(), "reason") }
            }
            div { class: "mt-6 flex justify-end gap-2",
                button { class: "btn btn-ghost", onclick: move |_| on_close.call(()), "Keep the order" }
                button { class: "btn btn-error", disabled: pending(), onclick: submit,
                    if pending() { span { class: "loading loading-spinner loading-xs" } }
                    "Cancel order"
                }
            }
        }
    }
}

#[component]
fn RefundDialog(
    detail: AdminOrderDetail,
    on_close: EventHandler<()>,
    on_done: EventHandler<()>,
) -> Element {
    let api = use_api();
    let toasts = use_toast();
    // One key per dialog: a double click starts one refund, a second dialog another.
    let key = use_hook(browser::uuid);
    // Captured charges with something left to reverse, oldest first. The dialog always names
    // the charge, so the amount is capped by what is left of that charge.
    let charges = refundable_charges(&detail);
    let initial = default_charge(&charges, RefundReason::Goodwill);
    let mut amount =
        use_signal(|| euros_input(initial.map_or(detail.actions.refundable, |c| c.left)));
    let mut reason = use_signal(|| RefundReason::Goodwill);
    let reason_text = use_signal(String::new);
    let mut charge = use_signal(|| initial.map(|c| c.entry.id.to_string()).unwrap_or_default());
    let refundable = charges
        .iter()
        .find(|c| c.entry.id.as_str() == charge())
        .map_or(detail.actions.refundable, |c| c.left);
    let mut errors = use_signal(Vec::<FieldError>::new);
    let mut pending = use_signal(|| false);
    let number = detail.order.order.number.to_string();

    let submit = {
        let (number, key) = (number.clone(), key.clone());
        move |_| {
            let Some(parsed) = parse_euros(&amount()) else {
                errors.set(vec![FieldError::new(
                    "amount",
                    "Enter an amount, e.g. 229.90",
                )]);
                return;
            };
            if parsed > refundable {
                errors.set(vec![FieldError::new(
                    "amount",
                    format!("At most {refundable} is left of this charge"),
                )]);
                return;
            }
            let request = InitiateRefundRequest {
                idempotency_key: key.clone(),
                amount: (parsed != refundable).then_some(parsed),
                reason: reason(),
                reason_text: non_blank(&reason_text()),
                charge_payment_id: non_blank(&charge()).map(PaymentId::new),
            };
            if let Err(error) = request.validate() {
                errors.set(error.fields);
                return;
            }
            errors.set(Vec::new());
            let number = number.clone();
            spawn(async move {
                pending.set(true);
                match api
                    .send::<InitiateRefund>(&[number.as_str()], &request)
                    .await
                {
                    Ok(started) => {
                        toasts.success(
                            format!("Refund {} started for {number}", started.refund_id),
                            ToastOptions::new(),
                        );
                        on_done.call(());
                    }
                    Err(error) => {
                        errors.set(error.fields.clone());
                        toasts.error(error.message, ToastOptions::new());
                    }
                }
                pending.set(false);
            });
        }
    };
    let e = errors();

    rsx! {
        Dialog {
            open: Some(true),
            on_open_change: move |open: bool| if !open { on_close.call(()) },
            class: "max-w-lg",
            DialogTitle { "Refund " Mono { "{number}" } }
            DialogDescription {
                "You are initiating it, so it is pre-approved. RefundWorkflow reverses the charge, issues a credit note where one is due and emails the customer."
            }
            div { class: "mt-4 flex flex-col gap-4",
                TextField { label: "Amount (€)", value: amount, error: error_for(&e, "amount"), hint: format!("Up to {refundable} is left of this charge.") }
                label { class: "flex flex-col gap-1",
                    span { class: "font-medium", "Reason" }
                    select {
                        class: "select w-full",
                        onchange: {
                            let charges = charges.clone();
                            move |ev: FormEvent| if let Ok(r) = ev.value().parse() {
                                reason.set(r);
                                // A double charge reverses the later charge, anything else the
                                // original: the same default RefundWorkflow would pick.
                                if let Some(c) = default_charge(&charges, r) {
                                    charge.set(c.entry.id.to_string());
                                    amount.set(euros_input(c.left));
                                }
                            }
                        },
                        for r in RefundReason::ALL.iter().copied() {
                            option { key: "{r.as_str()}", value: "{r.as_str()}", selected: reason() == r, "{r.label()}" }
                        }
                    }
                }
                TextField {
                    label: if reason() == RefundReason::Other { "Details" } else { "Details (optional)" },
                    value: reason_text,
                    multiline: true,
                    rows: 2,
                    error: error_for(&e, "reason_text"),
                }
                if charges.len() > 1 {
                    label { class: "flex flex-col gap-1",
                        span { class: "font-medium", "Charge to reverse" }
                        select {
                            class: "select w-full",
                            onchange: {
                                let charges = charges.clone();
                                move |ev: FormEvent| {
                                    if let Some(c) = charges.iter().find(|c| c.entry.id.as_str() == ev.value()) {
                                        charge.set(c.entry.id.to_string());
                                        amount.set(euros_input(c.left));
                                    }
                                }
                            },
                            for c in charges.iter().cloned() {
                                option { key: "{c.entry.id}", value: "{c.entry.id}", selected: charge() == c.entry.id.as_str(),
                                    "{c.entry.id} · {c.entry.amount} charged {datetime(c.entry.sim_at)} · {c.left} left"
                                }
                            }
                        }
                        span { class: "text-xs text-base-content/60", "For a double charge, reverse the later one." }
                    }
                }
                if let Some(message) = error_for(&e, "charge_payment_id") {
                    span { class: "text-xs text-error", "{message}" }
                }
            }
            div { class: "mt-6 flex justify-end gap-2",
                button { class: "btn btn-ghost", onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn btn-primary", disabled: pending(), onclick: submit,
                    if pending() { span { class: "loading loading-spinner loading-xs" } }
                    "Start refund"
                }
            }
        }
    }
}

/// A captured charge and what no refund has reversed of it yet.
#[derive(Debug, Clone, PartialEq)]
struct RefundableCharge {
    entry: PaymentEntry,
    /// The order's first charge (the one the invoice covers).
    original: bool,
    left: Money,
}

/// The order's captured charges with something left to refund, oldest first.
fn refundable_charges(detail: &AdminOrderDetail) -> Vec<RefundableCharge> {
    let payments = &detail.order.payments;
    let mut charges: Vec<&PaymentEntry> = payments
        .iter()
        .filter(|p| p.kind == PaymentKind::Charge && p.status == PaymentStatus::Succeeded)
        .collect();
    charges.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    charges
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let refunded: Money = payments
                .iter()
                .filter(|r| {
                    r.kind == PaymentKind::Refund
                        && r.status == PaymentStatus::Succeeded
                        && r.refund_of.as_ref() == Some(&c.id)
                })
                .map(|r| r.amount)
                .sum();
            RefundableCharge {
                entry: (*c).clone(),
                original: i == 0,
                left: c.amount - refunded,
            }
        })
        .filter(|c| c.left.is_positive())
        .collect()
}

/// The charge `plan_refund` would pick without one named: for a duplicate charge the latest
/// extra charge, otherwise the oldest charge with something left.
fn default_charge(charges: &[RefundableCharge], reason: RefundReason) -> Option<&RefundableCharge> {
    if reason == RefundReason::DuplicateCharge {
        charges.iter().rev().find(|c| !c.original)
    } else {
        charges.first()
    }
}

#[component]
fn InvestigateDialog(
    detail: AdminOrderDetail,
    on_close: EventHandler<()>,
    on_done: EventHandler<()>,
) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let key = use_hook(browser::uuid);
    let note = use_signal(String::new);
    let mut errors = use_signal(Vec::<FieldError>::new);
    let mut pending = use_signal(|| false);
    let number = detail.order.order.number.to_string();
    let tracking = detail
        .order
        .shipment
        .as_ref()
        .map(|s| s.tracking_number.to_string());

    let submit = {
        let (number, key) = (number.clone(), key.clone());
        move |_| {
            let request = OpenInvestigationRequest {
                idempotency_key: key.clone(),
                note: non_blank(&note()),
            };
            if let Err(error) = request.validate() {
                errors.set(error.fields);
                return;
            }
            let number = number.clone();
            spawn(async move {
                pending.set(true);
                match api
                    .send::<OpenInvestigation>(&[number.as_str()], &request)
                    .await
                {
                    Ok(started) => {
                        toasts.success(
                            format!("Investigation {} opened", started.investigation_id),
                            ToastOptions::new(),
                        );
                        on_done.call(());
                    }
                    Err(error) => {
                        errors.set(error.fields.clone());
                        toasts.error(error.message, ToastOptions::new());
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
            class: "max-w-lg",
            DialogTitle { "Open an investigation for " Mono { "{number}" } }
            DialogDescription {
                "Asks the carrier to trace "
                if let Some(t) = tracking { Mono { "{t}" } } else { "the parcel" }
                ". The Investigation workflow waits for the answer; if there is no information, it asks a person what to offer."
            }
            div { class: "mt-4",
                TextField { label: "Note (optional)", value: note, multiline: true, rows: 2, error: error_for(&errors(), "note") }
            }
            div { class: "mt-6 flex justify-end gap-2",
                button { class: "btn btn-ghost", onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn btn-primary", disabled: pending(), onclick: submit,
                    if pending() { span { class: "loading loading-spinner loading-xs" } }
                    "Open investigation"
                }
            }
        }
    }
}
