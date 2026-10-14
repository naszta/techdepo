//! The "Send as email" form: a message that enters the same support pipeline as a real email
//! (`SupportTicket/inbound` with `channel = web`).
//!
//! Each form load has its own idempotency key (it also becomes the message's `Message-ID`), so
//! a double click or a retry sends one message. After a send the form is remounted with a fresh
//! key for the next one.

use dioform::prelude::*;
use dioxus::prelude::*;
use techdepo_api_types::endpoints::SendSupportMessage;
use techdepo_api_types::support::{SendSupportMessageRequest, SendSupportMessageResponse};
use techdepo_api_types::{TicketId, Validate};

use crate::api::use_api;
use crate::browser;
use crate::components::input::InputField;
use crate::components::textarea::TextareaField;
use crate::components::toast::{ToastOptions, use_toast};
use crate::forms::{FormErrors, submit_errors, validation_errors};
use crate::ui::{Icon, IconName};

/// A new ticket (`ticket = None`, with a subject field) or a reply on `ticket`.
#[component]
pub fn Composer(
    #[props(default)] ticket: Option<TicketId>,
    #[props(default)] subject: String,
    on_sent: EventHandler<SendSupportMessageResponse>,
) -> Element {
    let mut generation = use_signal(|| 0u32);
    rsx! {
        ComposerForm {
            key: "{generation}",
            ticket,
            subject,
            on_sent: move |sent| {
                generation += 1;
                on_sent.call(sent);
            },
        }
    }
}

#[derive(Clone, PartialEq, Form)]
struct MessageModel {
    subject: String,
    body: String,
}

impl MessageModel {
    fn request(&self, key: &str, ticket: Option<TicketId>) -> SendSupportMessageRequest {
        SendSupportMessageRequest {
            idempotency_key: key.to_owned(),
            ticket_id: ticket,
            subject: self.subject.trim().to_owned(),
            body: self.body.trim().to_owned(),
        }
    }
}

#[component]
fn ComposerForm(
    ticket: Option<TicketId>,
    subject: String,
    on_sent: EventHandler<SendSupportMessageResponse>,
) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let key = use_hook(browser::uuid);
    let is_reply = ticket.is_some();

    let form = use_hook(|| {
        let (key, ticket) = (key.clone(), ticket.clone());
        FormConfig::new(MessageModel {
            subject: subject.clone(),
            body: String::new(),
        })
        .form_validator("send_message")
        .check(
            move |ctx| match ctx.form().request(&key, ticket.clone()).validate() {
                Ok(()) => Vec::new(),
                Err(error) => validation_errors(&error),
            },
        )
    });
    let form = use_form_config(form);
    let fields = MessageModel::fields();
    let submitting = form.is_submitting();
    let form_errors: Vec<String> = form
        .visible_form_validation_errors()
        .iter()
        .map(|e| e.error().clone())
        .collect();

    let submit_form = form.clone();
    let onsubmit = move |e: FormEvent| {
        e.prevent_default();
        let (key, ticket) = (key.clone(), ticket.clone());
        submit_form.submit_async_managed(move |snapshot| async move {
            let request = snapshot.value().request(&key, ticket);
            match api.send::<SendSupportMessage>(&[], &request).await {
                Ok(sent) => {
                    toasts.success(
                        "Sent. Replies appear here and in your inbox.".into(),
                        ToastOptions::new(),
                    );
                    on_sent.call(sent);
                    SubmitErrors::none()
                }
                Err(error) => submit_errors(&error),
            }
        });
    };

    rsx! {
        form { class: "flex flex-col gap-3", novalidate: true, onsubmit,
            FormErrors { errors: form_errors }
            if !is_reply {
                InputField { context: form.field(fields.subject()), label: "Subject", maxlength: "200", class: "w-full" }
            }
            TextareaField {
                context: form.field(fields.body()),
                label: if is_reply { "Reply" } else { "Message" },
                description: if is_reply { None } else { Some("Mention your order number (e.g. TD-1042) if it is about an order.".to_owned()) },
                rows: if is_reply { "3" } else { "6" },
                class: "w-full",
            }
            div { class: "flex items-center justify-between gap-3",
                span { class: "text-xs text-base-content/60",
                    "Goes through the same pipeline as an email to support."
                }
                button { r#type: "submit", class: "btn btn-primary btn-sm", disabled: submitting,
                    if submitting {
                        span { class: "loading loading-spinner loading-xs" }
                    } else {
                        Icon { name: IconName::Send }
                    }
                    "Send as email"
                }
            }
        }
    }
}
