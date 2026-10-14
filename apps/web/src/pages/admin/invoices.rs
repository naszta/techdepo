//! Every invoice and credit note of the shop, filterable by state, kind and order, with the
//! document viewer and void & reissue (a credit note plus a corrected invoice; no money moves,
//! so no approval).

use dioxus::prelude::*;
use techdepo_api_types::admin::{InvoiceListQuery, VoidReissueRequest, parse_order_query};
use techdepo_api_types::endpoints::{ListInvoices, VoidReissueInvoice};
use techdepo_api_types::orders::InvoiceSummary;
use techdepo_api_types::{Address, FieldError, InvoiceKind, InvoiceStatus, Validate};

use super::common::{
    FilterBar, OrderLink, SearchFilter, SelectFilter, TableWrap, TextField, error_for, non_blank,
    options, parsed, use_paged,
};
use crate::api::use_api;
use crate::components::dialog::{Dialog, DialogDescription, DialogTitle};
use crate::components::toast::{ToastOptions, use_toast};
use crate::pages::PageHeader;
use crate::pages::account::order::cards::invoice_note;
use crate::pages::account::order::invoice::InvoiceDialog;
use crate::ui::format::day;
use crate::ui::{EmptyState, Icon, IconName, InvoiceBadge, MoneyText, Mono};

#[component]
pub fn AdminInvoices() -> Element {
    let api = use_api();
    let status = use_signal(String::new);
    let kind = use_signal(String::new);
    let order = use_signal(String::new);
    let mut viewing = use_signal(|| None::<InvoiceSummary>);
    let mut voiding = use_signal(|| None::<InvoiceSummary>);
    let mut invoices = use_paged(move |cursor| async move {
        let query = InvoiceListQuery {
            status: parsed::<InvoiceStatus>(&status()),
            kind: parsed::<InvoiceKind>(&kind()),
            order: non_blank(&order()).and_then(|q| parse_order_query(&q)),
            cursor,
            limit: Some(50),
        };
        api.query::<ListInvoices>(&[], &query).await
    });

    rsx! {
        PageHeader { title: "Invoices", subtitle: "Invoices and credit notes. A correction is a credit note plus a new invoice; nothing is edited." }
        FilterBar {
            SelectFilter { label: "State", value: status, options: options(InvoiceStatus::ALL, InvoiceStatus::as_str, InvoiceStatus::label) }
            SelectFilter { label: "Kind", value: kind, options: options(InvoiceKind::ALL, InvoiceKind::as_str, InvoiceKind::label) }
            SearchFilter { label: "Order", placeholder: "TD-1042", value: order }
        }
        {invoices.render(|items| rsx! {
            if items.is_empty() {
                EmptyState { message: "No invoices match.", icon: IconName::Receipt }
            } else {
                TableWrap {
                    table { class: "table table-sm",
                        thead {
                            tr {
                                th { "Number" }
                                th { "State" }
                                th { "Order" }
                                th { "Billed to" }
                                th { "Issued" }
                                th { class: "text-right", "Amount" }
                                th { "Notes" }
                                th {}
                            }
                        }
                        tbody {
                            for invoice in items {
                                InvoiceRow {
                                    key: "{invoice.id}",
                                    invoice: invoice.clone(),
                                    on_view: move |i| viewing.set(Some(i)),
                                    on_void: move |i| voiding.set(Some(i)),
                                }
                            }
                        }
                    }
                }
            }
        })}
        if let Some(invoice) = viewing() {
            InvoiceDialog { number: invoice.order_number.to_string(), invoice, admin: true, on_close: move |_| viewing.set(None) }
        }
        if let Some(invoice) = voiding() {
            VoidReissueDialog {
                invoice,
                on_close: move |_| voiding.set(None),
                on_done: move |_| {
                    voiding.set(None);
                    invoices.restart();
                },
            }
        }
    }
}

#[component]
fn InvoiceRow(
    invoice: InvoiceSummary,
    on_view: EventHandler<InvoiceSummary>,
    on_void: EventHandler<InvoiceSummary>,
) -> Element {
    let voidable = can_void(&invoice);
    rsx! {
        tr { class: "hover:bg-base-200",
            td {
                Mono {
                    class: if invoice.status == InvoiceStatus::Voided { "line-through text-base-content/60" } else { "" },
                    "{invoice.number}"
                }
            }
            td { InvoiceBadge { invoice: invoice.clone() } }
            td { OrderLink { number: invoice.order_number } }
            td {
                "{invoice.billing_name}"
                if invoice.kind == InvoiceKind::Invoice && invoice.missing_billing_address {
                    span { class: "ml-1 inline-flex items-center gap-1 text-xs text-warning",
                        Icon { name: IconName::TriangleAlert, class: "size-3" }
                        "no address"
                    }
                }
            }
            td { class: "whitespace-nowrap", "{day(invoice.issue_date)}" }
            td { class: "text-right", MoneyText { amount: invoice.amounts.gross } }
            td { class: "max-w-56 truncate text-xs text-base-content/60", title: "{invoice_note(&invoice)}", {relations(&invoice)} }
            td { class: "whitespace-nowrap text-right",
                button { class: "btn btn-ghost btn-xs", onclick: {
                    let invoice = invoice.clone();
                    move |_| on_view.call(invoice.clone())
                }, "View" }
                if voidable {
                    button { class: "btn btn-ghost btn-xs", onclick: {
                        let invoice = invoice.clone();
                        move |_| on_void.call(invoice.clone())
                    }, "Void & reissue" }
                }
            }
        }
    }
}

/// Only an issued invoice (not a credit note) can be voided and reissued.
pub fn can_void(invoice: &InvoiceSummary) -> bool {
    invoice.kind == InvoiceKind::Invoice && invoice.status == InvoiceStatus::Issued
}

fn relations(invoice: &InvoiceSummary) -> String {
    let mut parts = Vec::new();
    if let Some(credit_for) = &invoice.credit_for {
        parts.push(format!("cancels {}", credit_for.number));
    }
    if let Some(supersedes) = &invoice.supersedes {
        parts.push(format!("replaces {}", supersedes.number));
    }
    if let Some(by) = &invoice.superseded_by {
        parts.push(format!("replaced by {}", by.number));
    }
    if let Some(reason) = &invoice.void_reason {
        parts.push(format!("voided: {reason}"));
    }
    parts.join(" · ")
}

/// Void an invoice with a credit note and issue a corrected one, optionally with a corrected
/// billing address (the "invoice without address" fix).
#[component]
pub fn VoidReissueDialog(
    invoice: InvoiceSummary,
    on_close: EventHandler<()>,
    on_done: EventHandler<()>,
) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let default_reason = if invoice.missing_billing_address {
        "Billing address missing"
    } else {
        "Correction"
    };
    let reason = use_signal(|| default_reason.to_owned());
    let mut with_address = use_signal(|| invoice.missing_billing_address);
    let name = use_signal(|| invoice.billing_name.clone());
    let company = use_signal(String::new);
    let line1 = use_signal(String::new);
    let postal_code = use_signal(String::new);
    let city = use_signal(String::new);
    let country = use_signal(|| "HU".to_owned());
    let vat_id = use_signal(String::new);
    let mut errors = use_signal(Vec::<FieldError>::new);
    let mut pending = use_signal(|| false);
    let id = invoice.id.to_string();

    let submit = move |_| {
        let request = VoidReissueRequest {
            reason: reason().trim().to_owned(),
            billing_address: with_address().then(|| Address {
                name: name().trim().to_owned(),
                company: non_blank(&company()),
                line1: line1().trim().to_owned(),
                line2: None,
                postal_code: postal_code().trim().to_owned(),
                city: city().trim().to_owned(),
                country: country().trim().to_uppercase(),
                phone: None,
                vat_id: non_blank(&vat_id()),
            }),
        };
        if let Err(error) = request.validate() {
            errors.set(error.fields);
            return;
        }
        let id = id.clone();
        spawn(async move {
            pending.set(true);
            match api
                .send::<VoidReissueInvoice>(&[id.as_str()], &request)
                .await
            {
                Ok(done) => {
                    toasts.success(
                        format!(
                            "{} voided; credit note {} and invoice {} issued",
                            done.voided.number, done.credit_note.number, done.reissued.number
                        ),
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
    };
    let e = errors();

    rsx! {
        Dialog {
            open: Some(true),
            on_open_change: move |open: bool| if !open { on_close.call(()) },
            class: "max-w-xl",
            DialogTitle { "Void and reissue " Mono { "{invoice.number}" } }
            DialogDescription {
                "Issues a credit note that cancels this invoice and a corrected invoice for the same amount. No money moves."
            }
            div { class: "mt-4 flex flex-col gap-4",
                TextField { label: "Reason (printed on the credit note)", value: reason, error: error_for(&e, "reason") }
                label { class: "flex cursor-pointer items-center gap-2",
                    input { r#type: "checkbox", class: "checkbox checkbox-sm", checked: with_address(), onchange: move |ev| with_address.set(ev.checked()) }
                    "Correct the billing address"
                }
                if with_address() {
                    div { class: "grid gap-3 sm:grid-cols-2",
                        TextField { label: "Name", value: name, error: error_for(&e, "billing_address.name") }
                        TextField { label: "Company (optional)", value: company, error: error_for(&e, "billing_address.company") }
                        TextField { label: "Street and number", value: line1, error: error_for(&e, "billing_address.line1"), class: "w-full sm:col-span-2" }
                        TextField { label: "Postal code", value: postal_code, error: error_for(&e, "billing_address.postal_code") }
                        TextField { label: "City", value: city, error: error_for(&e, "billing_address.city") }
                        TextField { label: "Country", value: country, error: error_for(&e, "billing_address.country"), hint: "Two-letter code, e.g. HU" }
                        TextField { label: "VAT number (optional)", value: vat_id, error: error_for(&e, "billing_address.vat_id") }
                    }
                } else {
                    p { class: "text-xs text-base-content/60", "The corrected invoice uses the order's billing (or shipping) address." }
                }
            }
            div { class: "mt-6 flex justify-end gap-2",
                button { class: "btn btn-ghost", onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn btn-primary", disabled: pending(), onclick: submit,
                    if pending() { span { class: "loading loading-spinner loading-xs" } }
                    "Void and reissue"
                }
            }
        }
    }
}
