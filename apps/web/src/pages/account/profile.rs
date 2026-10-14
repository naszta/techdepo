//! Profile: who you are (identity is managed by Clerk, or the dev login) and the address book.
//!
//! Addresses are created, edited and deleted through `Customer/save_address` and
//! `Customer/delete_address`; the form is a dioform model shaped like `SaveAddressRequest`, so
//! the request's own `validate` (the one the web worker runs too) drives the inline errors.

use dioform::prelude::*;
use dioxus::prelude::*;
use techdepo_api_types::account::{SaveAddressRequest, SavedAddress};
use techdepo_api_types::endpoints::{CreateAddress, DeleteAddress, ListAddresses, UpdateAddress};
use techdepo_api_types::session::AuthMode;
use techdepo_api_types::{Empty, Validate};

use crate::api::use_api;
use crate::browser;
use crate::components::badge::{Badge, BadgeAppearance, BadgeSize};
use crate::components::dialog::{Dialog, DialogDescription, DialogTitle};
use crate::components::input::InputField;
use crate::components::toast::{ToastOptions, use_toast};
use crate::forms::{AddressFields, AddressModel, FormErrors, submit_errors, validation_errors};
use crate::pages::{PageHeader, Panel};
use crate::session::use_session;
use crate::ui::{EmptyState, ErrorAlert, Icon, IconName, Mono, PageSkeleton};

/// What the address dialog is editing.
#[derive(Clone, PartialEq)]
enum Editing {
    New,
    Existing(SavedAddress),
}

#[component]
pub fn AccountProfile() -> Element {
    let api = use_api();
    let mut addresses = use_resource(move || async move { api.get::<ListAddresses>(&[]).await });
    let mut editing = use_signal(|| None::<Editing>);
    let mut deleting = use_signal(|| None::<SavedAddress>);

    rsx! {
        PageHeader { title: "Profile" }
        div { class: "flex flex-col gap-6",
            IdentityPanel {}
            Panel {
                title: "Addresses",
                subtitle: "Saved addresses prefill checkout.",
                actions: rsx! {
                    button { class: "btn btn-primary btn-sm", onclick: move |_| editing.set(Some(Editing::New)),
                        Icon { name: IconName::Plus }
                        "Add address"
                    }
                },
                match &*addresses.read() {
                    None => rsx! { div { class: "skeleton h-28 w-full" } },
                    Some(Err(error)) => rsx! { ErrorAlert { error: error.clone(), on_retry: move |_| addresses.restart() } },
                    Some(Ok(list)) if list.addresses.is_empty() => rsx! {
                        EmptyState { message: "No saved addresses yet.", icon: IconName::MapPin,
                            button { class: "btn btn-primary btn-sm", onclick: move |_| editing.set(Some(Editing::New)), "Add an address" }
                        }
                    },
                    Some(Ok(list)) => rsx! {
                        ul { class: "grid gap-4 sm:grid-cols-2",
                            for saved in list.addresses.clone() {
                                AddressCard {
                                    key: "{saved.id}",
                                    saved: saved.clone(),
                                    on_edit: {
                                        let saved = saved.clone();
                                        move |_| editing.set(Some(Editing::Existing(saved.clone())))
                                    },
                                    on_delete: move |_| deleting.set(Some(saved.clone())),
                                }
                            }
                        }
                    },
                }
            }
        }
        if let Some(what) = editing() {
            AddressDialog {
                editing: what,
                on_close: move |_| editing.set(None),
                on_saved: move |_| {
                    editing.set(None);
                    addresses.restart();
                },
            }
        }
        if let Some(saved) = deleting() {
            DeleteDialog {
                saved,
                on_close: move |_| deleting.set(None),
                on_deleted: move |_| {
                    deleting.set(None);
                    addresses.restart();
                },
            }
        }
    }
}

#[component]
fn IdentityPanel() -> Element {
    let session = use_session();
    let Some(me) = session.me() else {
        return rsx! { PageSkeleton { rows: 1 } };
    };
    let managed_by = match session.auth_mode() {
        Some(AuthMode::Clerk) => {
            "Name, email and password are managed in your Clerk account (the menu at the top right)."
        }
        _ => "Signed in with the local dev login; there is no password.",
    };
    rsx! {
        Panel { title: "Account",
            dl { class: "grid grid-cols-[7rem_1fr] gap-x-3 gap-y-1.5 text-sm",
                dt { class: "text-base-content/60", "Name" }
                dd { {me.user.name.clone().unwrap_or_else(|| "—".to_owned())} }
                dt { class: "text-base-content/60", "Email" }
                dd { class: "break-all", "{me.user.email}" }
                dt { class: "text-base-content/60", "Shop" }
                dd { "{me.org.name} ({me.role.label()})" }
                dt { class: "text-base-content/60", "Support" }
                dd { a { class: "link link-primary break-all", href: "mailto:{me.org.support_address}", Mono { "{me.org.support_address}" } } }
            }
            p { class: "text-xs text-base-content/60", "{managed_by}" }
        }
    }
}

#[component]
fn AddressCard(
    saved: SavedAddress,
    on_edit: EventHandler<()>,
    on_delete: EventHandler<()>,
) -> Element {
    rsx! {
        li { class: "flex flex-col gap-3 rounded-box border border-base-300 p-4",
            div { class: "flex flex-wrap items-center gap-2",
                span { class: "font-medium", "{saved.label}" }
                if saved.default_shipping {
                    Badge { size: BadgeSize::Sm, appearance: BadgeAppearance::Outline, "Default shipping" }
                }
                if saved.default_billing {
                    Badge { size: BadgeSize::Sm, appearance: BadgeAppearance::Outline, "Default billing" }
                }
            }
            address { class: "flex flex-col text-sm not-italic text-base-content/80",
                for (i , line) in saved.address.lines().into_iter().enumerate() {
                    span { key: "{i}", "{line}" }
                }
                if let Some(phone) = saved.address.phone.clone() {
                    span { class: "text-base-content/60", "{phone}" }
                }
                if let Some(vat) = saved.address.vat_id.clone() {
                    span { class: "text-base-content/60", "VAT {vat}" }
                }
            }
            div { class: "mt-auto flex justify-end gap-2",
                button { class: "btn btn-ghost btn-sm", onclick: move |_| on_delete.call(()),
                    Icon { name: IconName::Trash }
                    "Delete"
                }
                button { class: "btn btn-sm", onclick: move |_| on_edit.call(()),
                    Icon { name: IconName::Pencil }
                    "Edit"
                }
            }
        }
    }
}

/// Shaped like `SaveAddressRequest`, so its field errors (`label`, `address.city`, …) land on
/// these fields.
#[derive(Clone, PartialEq, Form)]
struct SavedAddressModel {
    label: String,
    address: AddressModel,
    default_shipping: bool,
    default_billing: bool,
}

impl SavedAddressModel {
    fn request(&self, idempotency_key: Option<String>) -> SaveAddressRequest {
        SaveAddressRequest {
            idempotency_key,
            label: self.label.trim().to_owned(),
            address: self.address.to_address(),
            default_shipping: self.default_shipping,
            default_billing: self.default_billing,
        }
    }
}

#[component]
fn AddressDialog(
    editing: Editing,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let api = use_api();
    let toasts = use_toast();
    // A new address carries a client key (one per dialog), so a double submit saves it once.
    let key = use_hook(|| (editing == Editing::New).then(browser::uuid));
    let existing = match &editing {
        Editing::Existing(saved) => Some(saved.clone()),
        Editing::New => None,
    };

    let form = use_hook(|| {
        let initial = match &existing {
            Some(saved) => SavedAddressModel {
                label: saved.label.clone(),
                address: AddressModel::from_address(&saved.address),
                default_shipping: saved.default_shipping,
                default_billing: saved.default_billing,
            },
            None => SavedAddressModel {
                label: "Home".to_owned(),
                address: AddressModel {
                    country: "HU".to_owned(),
                    ..AddressModel::default()
                },
                default_shipping: false,
                default_billing: false,
            },
        };
        let key = key.clone();
        FormConfig::new(initial)
            .form_validator("save_address")
            .check(
                move |ctx| match ctx.form().request(key.clone()).validate() {
                    Ok(()) => Vec::new(),
                    Err(error) => validation_errors(&error),
                },
            )
    });
    let form = use_form_config(form);
    let fields = SavedAddressModel::fields();
    let default_shipping = form.checkbox(fields.default_shipping());
    let default_billing = form.checkbox(fields.default_billing());
    let submitting = form.is_submitting();
    let form_errors: Vec<String> = form
        .visible_form_validation_errors()
        .iter()
        .map(|e| e.error().clone())
        .collect();

    let submit_form = form.clone();
    let id = existing.as_ref().map(|saved| saved.id.clone());
    let onsubmit = move |e: FormEvent| {
        e.prevent_default();
        let (key, id) = (key.clone(), id.clone());
        submit_form.submit_async_managed(move |snapshot| async move {
            let request = snapshot.value().request(key);
            let result = match &id {
                Some(id) => api.send::<UpdateAddress>(&[id.as_str()], &request).await,
                None => api.send::<CreateAddress>(&[], &request).await,
            };
            match result {
                Ok(saved) => {
                    toasts.success(
                        format!("Address “{}” saved", saved.label),
                        ToastOptions::new(),
                    );
                    on_saved.call(());
                    SubmitErrors::none()
                }
                Err(error) => submit_errors(&error),
            }
        });
    };

    rsx! {
        Dialog {
            open: Some(true),
            on_open_change: move |open: bool| if !open { on_close.call(()) },
            class: "max-w-2xl",
            DialogTitle { if existing.is_some() { "Edit address" } else { "New address" } }
            DialogDescription { "Used to prefill checkout. Orders keep the address they were placed with." }
            form { class: "mt-4 flex flex-col gap-4", novalidate: true, onsubmit,
                FormErrors { errors: form_errors }
                InputField {
                    context: form.field(fields.label()),
                    label: "Label",
                    description: "A short name, e.g. Home or Office.",
                    maxlength: "40",
                    class: "w-full sm:w-1/2",
                }
                AddressFields { form: form.clone(), prefix: fields.address() }
                div { class: "flex flex-wrap gap-x-6 gap-y-2",
                    label { class: "label text-sm text-base-content",
                        input {
                            r#type: "checkbox",
                            class: "checkbox checkbox-sm",
                            checked: default_shipping.checked(),
                            onchange: move |e| default_shipping.on_change(e.checked()),
                        }
                        "Default shipping address"
                    }
                    label { class: "label text-sm text-base-content",
                        input {
                            r#type: "checkbox",
                            class: "checkbox checkbox-sm",
                            checked: default_billing.checked(),
                            onchange: move |e| default_billing.on_change(e.checked()),
                        }
                        "Default billing address"
                    }
                }
                div { class: "flex justify-end gap-2",
                    button { r#type: "button", class: "btn btn-ghost", onclick: move |_| on_close.call(()), "Cancel" }
                    button { r#type: "submit", class: "btn btn-primary", disabled: submitting,
                        if submitting { span { class: "loading loading-spinner loading-sm" } }
                        "Save address"
                    }
                }
            }
        }
    }
}

#[component]
fn DeleteDialog(
    saved: SavedAddress,
    on_close: EventHandler<()>,
    on_deleted: EventHandler<()>,
) -> Element {
    let api = use_api();
    let toasts = use_toast();
    let mut busy = use_signal(|| false);
    let id = saved.id.clone();
    let delete = move |_| {
        let id = id.clone();
        spawn(async move {
            busy.set(true);
            match api.send::<DeleteAddress>(&[id.as_str()], &Empty {}).await {
                Ok(_) => {
                    toasts.info("Address deleted".into(), ToastOptions::new());
                    on_deleted.call(());
                }
                Err(error) => toasts.error(error.message, ToastOptions::new()),
            }
            busy.set(false);
        });
    };
    rsx! {
        Dialog {
            open: Some(true),
            on_open_change: move |open: bool| if !open { on_close.call(()) },
            DialogTitle { "Delete “{saved.label}”?" }
            DialogDescription { "Past orders keep their addresses; only the saved entry goes." }
            div { class: "mt-4 flex justify-end gap-2",
                button { class: "btn btn-ghost", onclick: move |_| on_close.call(()), "Keep it" }
                button { class: "btn btn-error", disabled: busy(), onclick: delete,
                    if busy() { span { class: "loading loading-spinner loading-sm" } }
                    "Delete"
                }
            }
        }
    }
}
