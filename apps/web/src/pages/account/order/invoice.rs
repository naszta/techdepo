//! One invoice or credit note as issued: the rendered HTML document in a sandboxed frame.
//!
//! The customer reads it through their order (`GetOrderInvoice`); the admin pages pass `admin`
//! and read any invoice of the org by ID (`GetInvoice`).
//!
//! The frame runs no scripts (`sandbox` without `allow-scripts`); `allow-same-origin` only lets
//! the page call the frame's `print()`.

use dioxus::prelude::*;
use techdepo_api_types::InvoiceStatus;
use techdepo_api_types::endpoints::{GetInvoice, GetOrderInvoice};
use techdepo_api_types::orders::InvoiceSummary;

use crate::api::use_api;
use crate::components::dialog::{Dialog, DialogDescription, DialogTitle};
use crate::ui::format::date;
use crate::ui::{ErrorAlert, Icon, IconName, InvoiceBadge, Mono};

const FRAME_ID: &str = "invoice-document";

#[component]
pub fn InvoiceDialog(
    number: String,
    invoice: InvoiceSummary,
    on_close: EventHandler<()>,
    #[props(default)] admin: bool,
) -> Element {
    let api = use_api();
    let invoice_number = invoice.number.to_string();
    let invoice_id = invoice.id.to_string();
    let mut document = use_resource(move || {
        let (number, invoice_number, invoice_id) =
            (number.clone(), invoice_number.clone(), invoice_id.clone());
        async move {
            if admin {
                api.get::<GetInvoice>(&[invoice_id.as_str()]).await
            } else {
                api.get::<GetOrderInvoice>(&[number.as_str(), invoice_number.as_str()])
                    .await
            }
        }
    });

    // A constant script (no interpolated data besides our own element id): print the frame.
    let print = move |_| {
        document::eval(&format!(
            "document.getElementById('{FRAME_ID}')?.contentWindow?.print()"
        ));
    };

    rsx! {
        Dialog {
            open: Some(true),
            on_open_change: move |open: bool| if !open { on_close.call(()) },
            class: "max-w-4xl",
            div { class: "flex flex-wrap items-start justify-between gap-3",
                div { class: "flex flex-col gap-1",
                    DialogTitle {
                        span { class: "flex items-center gap-2",
                            "{invoice.kind.label()} "
                            Mono { "{invoice.number}" }
                            InvoiceBadge { invoice: invoice.clone() }
                        }
                    }
                    DialogDescription {
                        if invoice.status == InvoiceStatus::Voided {
                            "Voided"
                            if let Some(at) = invoice.voided_at { " on {date(at)}" }
                            if let Some(reason) = invoice.void_reason.clone() { ": {reason}" }
                            if let Some(by) = invoice.superseded_by.clone() { ". Replaced by {by.number}." }
                        } else {
                            "Issued {date(invoice.issued_at)} to {invoice.billing_name}."
                        }
                    }
                }
                div { class: "flex gap-2",
                    button { class: "btn btn-sm", onclick: print, disabled: !matches!(&*document.read(), Some(Ok(_))),
                        Icon { name: IconName::Printer }
                        "Print"
                    }
                    button { class: "btn btn-ghost btn-sm btn-square", aria_label: "Close", onclick: move |_| on_close.call(()),
                        Icon { name: IconName::X }
                    }
                }
            }
            div { class: "mt-4",
                match &*document.read() {
                    None => rsx! { div { class: "skeleton h-[60vh] w-full" } },
                    Some(Err(error)) => rsx! { ErrorAlert { error: error.clone(), on_retry: move |_| document.restart() } },
                    Some(Ok(doc)) => rsx! {
                        iframe {
                            id: FRAME_ID,
                            title: "Invoice {invoice.number}",
                            class: "h-[65vh] w-full rounded-box border border-base-300 bg-white",
                            "sandbox": "allow-same-origin allow-modals",
                            "srcdoc": "{doc.html}",
                        }
                    },
                }
            }
        }
    }
}
