//! Checkout: one page in three sections (shipping address, billing address, payment method)
//! with the order summary on the side. The form is a dioform model shaped like
//! `PlaceOrderRequest`, so `PlaceOrderRequest::validate` (the same check the web worker runs)
//! drives the inline errors.
//!
//! "Place order" calls `POST /api/checkout` with the cart total the customer saw
//! (`expected_total`) and an idempotency key per cart seen, then opens the new order. When
//! checkout answers that the cart changed (or a part ran out, or the build turned incompatible),
//! the page shows the fresh cart and the next attempt gets a new key.

use dioform::prelude::*;
use dioxus::prelude::*;
use techdepo_api_types::account::{AddressList, Cart, PlaceOrderRequest, SavedAddress};
use techdepo_api_types::endpoints::{ListAddresses, PlaceOrder};
use techdepo_api_types::vocab::PaymentMethod;
use techdepo_api_types::{Address, ApiError, ErrorCode, Validate};

use crate::api::use_api;
use crate::auth::RequireAuth;
use crate::browser;
use crate::forms::{AddressFields, AddressModel, FormErrors, submit_errors, validation_errors};
use crate::pages::PageHeader;
use crate::routes::Route;
use crate::state::build::catalog;
use crate::state::use_cart;
use crate::ui::product::ProductTile;
use crate::ui::{CompatBadge, EmptyState, ErrorAlert, Icon, IconName, MoneyText, PageSkeleton};

#[component]
pub fn Checkout() -> Element {
    rsx! {
        RequireAuth { CheckoutLoader {} }
    }
}

/// Loads a fresh cart and the address book, then mounts the form with its initial values.
#[component]
fn CheckoutLoader() -> Element {
    let api = use_api();
    let cart = use_cart();
    let mut data = use_resource(move || async move {
        let cart = cart.refresh().await?;
        // The address book only prefills; checkout works without it.
        let addresses = api.get::<ListAddresses>(&[]).await.unwrap_or(AddressList {
            addresses: Vec::new(),
        });
        Ok::<_, ApiError>((cart, addresses.addresses))
    });

    match &*data.read() {
        None => rsx! { PageSkeleton {} },
        Some(Err(error)) => {
            rsx! { ErrorAlert { error: error.clone(), on_retry: move |_| data.restart() } }
        }
        Some(Ok((cart, _))) if cart.lines.is_empty() => rsx! {
            PageHeader { title: "Checkout" }
            EmptyState { message: "Your cart is empty.", icon: IconName::Cart,
                Link { to: Route::Home {}, class: "btn btn-primary btn-sm", "Browse parts" }
            }
        },
        Some(Ok((cart, addresses))) => rsx! {
            CheckoutForm { cart: cart.clone(), addresses: addresses.clone() }
        },
    }
}

/// Shaped like `PlaceOrderRequest`, so its field errors land on these fields.
#[derive(Clone, PartialEq, Form)]
struct CheckoutModel {
    shipping_address: AddressModel,
    same_as_shipping: bool,
    billing_address: AddressModel,
    payment_method: PaymentMethod,
    save_address: bool,
    override_compat: bool,
}

impl CheckoutModel {
    fn request(
        &self,
        idempotency_key: &str,
        expected_total: techdepo_api_types::Money,
    ) -> PlaceOrderRequest {
        PlaceOrderRequest {
            idempotency_key: idempotency_key.to_owned(),
            shipping_address: self.shipping_address.to_address(),
            billing_address: (!self.same_as_shipping).then(|| self.billing_address.to_address()),
            payment_method: self.payment_method,
            expected_total,
            override_compat: self.override_compat,
            save_address: self.save_address,
        }
    }
}

fn default_address(addresses: &[SavedAddress], billing: bool) -> Option<&SavedAddress> {
    addresses.iter().find(|a| {
        if billing {
            a.default_billing
        } else {
            a.default_shipping
        }
    })
}

#[component]
fn CheckoutForm(cart: Cart, addresses: Vec<SavedAddress>) -> Element {
    let api = use_api();
    let store = use_cart();
    let nav = navigator();
    // The cart the customer is looking at (and pays `expected_total` for). Replaced by the
    // fresh one when checkout says the cart changed, so the summary shows what changed.
    let mut current = use_signal(|| cart.clone());
    // One key per cart the customer saw: a double click or a retry places one order. A new
    // cart gets a new key, or Restate would answer the retry with the stored rejection.
    let mut idempotency_key = use_signal(browser::uuid);
    let cart = current();
    let needs_override = cart.needs_compat_override();

    let form = use_hook(|| {
        let shipping = default_address(&addresses, false)
            .map(|a| AddressModel::from_address(&a.address))
            .unwrap_or_else(|| AddressModel {
                country: "HU".to_owned(),
                ..AddressModel::default()
            });
        let billing =
            default_address(&addresses, true).map(|a| AddressModel::from_address(&a.address));
        let same = billing.as_ref().is_none_or(|b| *b == shipping);
        FormConfig::new(CheckoutModel {
            billing_address: billing.unwrap_or_else(|| shipping.clone()),
            shipping_address: shipping,
            same_as_shipping: same,
            payment_method: PaymentMethod::Card,
            save_address: addresses.is_empty(),
            override_compat: false,
        })
        .form_validator("place_order")
        .check(move |ctx| {
            let model = ctx.form();
            let (key, needs_override, expected_total) = {
                let cart = current.peek();
                (
                    idempotency_key.peek().clone(),
                    cart.needs_compat_override(),
                    cart.total,
                )
            };
            let mut errors = match model.request(&key, expected_total).validate() {
                Ok(()) => Vec::new(),
                Err(error) => validation_errors(&error),
            };
            if needs_override && !model.override_compat {
                errors.push(FormValidationError::field(
                    CheckoutModel::fields().override_compat(),
                    "Confirm that you want these parts even though they don't work together"
                        .to_owned(),
                ));
            }
            errors
        })
    });
    let form = use_form_config(form);

    let fields = CheckoutModel::fields();
    let same_as_shipping = form.checkbox(fields.same_as_shipping());
    let save_address = form.checkbox(fields.save_address());
    let override_compat = form.checkbox(fields.override_compat());
    let payment = form.field_value(fields.payment_method());
    let override_error: Option<String> = form
        .visible_field_validation_errors(fields.override_compat())
        .first()
        .map(|e| e.error().clone());
    let form_errors: Vec<String> = form
        .visible_form_validation_errors()
        .iter()
        .map(|e| e.error().clone())
        .collect();
    let submitting = form.is_submitting();

    let submit_form = form.clone();
    let onsubmit = move |e: FormEvent| {
        e.prevent_default();
        let key = idempotency_key.peek().clone();
        let expected_total = current.peek().total;
        submit_form.submit_async_managed(move |snapshot| async move {
            let request = snapshot.value().request(&key, expected_total);
            match api.send::<PlaceOrder>(&[], &request).await {
                Ok(placed) => {
                    let _ = store.refresh().await;
                    nav.push(Route::AccountOrder {
                        number: placed.order.number.to_string(),
                    });
                    SubmitErrors::none()
                }
                Err(error) => {
                    if matches!(
                        error.code,
                        ErrorCode::CartChanged
                            | ErrorCode::OutOfStock
                            | ErrorCode::IncompatibleBuild
                    ) {
                        // Show what changed; the customer reviews it and tries again, as a
                        // new attempt (new key, the new total).
                        if let Ok(fresh) = store.refresh().await {
                            current.set(fresh);
                        }
                        idempotency_key.set(browser::uuid());
                    }
                    submit_errors(&error)
                }
            }
        });
    };

    let fill_shipping = {
        let form = form.clone();
        move |address: Address| {
            let f = CheckoutModel::fields();
            form.set_field(f.shipping_address(), AddressModel::from_address(&address));
        }
    };

    rsx! {
        PageHeader { title: "Checkout" }
        form { class: "grid gap-8 lg:grid-cols-[1fr_22rem]", novalidate: true, onsubmit,
            div { class: "flex flex-col gap-8",
                FormErrors { errors: form_errors }

                Section { number: 1, title: "Shipping address",
                    hint: "Where we deliver the parcel. We ship to EU countries, free of charge.",
                    if !addresses.is_empty() {
                        div { class: "flex flex-wrap gap-2",
                            for saved in addresses.iter().cloned() {
                                button {
                                    r#type: "button",
                                    class: "btn btn-sm btn-outline",
                                    onclick: {
                                        let fill = fill_shipping.clone();
                                        move |_| fill(saved.address.clone())
                                    },
                                    "Use “{saved.label}”"
                                }
                            }
                        }
                    }
                    AddressFields { form: form.clone(), prefix: fields.shipping_address() }
                    label { class: "label text-sm text-base-content",
                        input {
                            r#type: "checkbox",
                            class: "checkbox checkbox-sm",
                            checked: save_address.checked(),
                            onchange: move |e| save_address.on_change(e.checked()),
                        }
                        "Save this address to my account"
                    }
                }

                Section { number: 2, title: "Billing address",
                    hint: "The name and address on your invoice.",
                    label { class: "label text-sm text-base-content",
                        input {
                            r#type: "checkbox",
                            class: "checkbox checkbox-sm",
                            checked: same_as_shipping.checked(),
                            onchange: move |e| same_as_shipping.on_change(e.checked()),
                        }
                        "Same as the shipping address"
                    }
                    if !same_as_shipping.checked() {
                        AddressFields { form: form.clone(), prefix: fields.billing_address() }
                    }
                }

                Section { number: 3, title: "Payment method",
                    hint: "Payments are simulated: nothing is charged. After ordering, use the demo controls on the order page to pay, decline or double-charge.",
                    div { role: "radiogroup", aria_label: "Payment method", class: "grid gap-3 sm:grid-cols-2",
                        for method in PaymentMethod::CUSTOMER.iter().copied() {
                            label {
                                class: "flex cursor-pointer items-center gap-3 rounded-box border p-4",
                                class: if payment == method { "border-primary bg-primary/5" } else { "border-base-300" },
                                input {
                                    r#type: "radio",
                                    name: "payment_method",
                                    class: "radio radio-primary radio-sm",
                                    checked: payment == method,
                                    onchange: {
                                        let form = form.clone();
                                        move |_| form.set_user_field(CheckoutModel::fields().payment_method(), method)
                                    },
                                }
                                Icon { name: if method == PaymentMethod::Card { IconName::CreditCard } else { IconName::Receipt }, class: "size-5" }
                                span { class: "font-medium", "{method.label()}" }
                            }
                        }
                    }
                }
            }

            // Order summary.
            div { class: "lg:sticky lg:top-24 lg:self-start",
                div { class: "card card-border border-base-300 bg-base-100",
                    div { class: "card-body gap-3",
                        h2 { class: "card-title text-base", "Order summary" }
                        ul { class: "flex flex-col gap-2",
                            for line in cart.lines.iter() {
                                li { key: "{line.product_id}", class: "flex items-center gap-2 text-sm",
                                    ProductTile { category: line.category, class: "size-9 shrink-0", icon_class: "size-4" }
                                    span { class: "flex-1 truncate", "{line.name}" }
                                    if line.quantity > 1 {
                                        span { class: "text-base-content/60 tabular-nums", "×{line.quantity}" }
                                    }
                                    MoneyText { amount: line.line_total }
                                }
                            }
                        }
                        div { class: "flex justify-between border-t border-base-300 pt-3 text-sm",
                            span { "Shipping cost" }
                            span { "Free" }
                        }
                        div { class: "flex items-baseline justify-between",
                            span { "Total" }
                            MoneyText { amount: cart.total, class: "text-2xl font-semibold" }
                        }
                        span { class: "text-xs text-base-content/60", "Prices include VAT." }

                        if let Some(report) = cart.report.clone() {
                            if needs_override {
                                div { class: "flex flex-col gap-2 rounded-box border border-error/40 p-3",
                                    div { class: "flex items-center justify-between",
                                        span { class: "text-sm font-medium", "Compatibility" }
                                        CompatBadge { verdict: report.verdict, small: true }
                                    }
                                    ul { class: "flex flex-col gap-1 text-xs text-base-content/80",
                                        for issue in report.issues.iter().filter(|i| i.severity == techdepo_compat::Severity::Error) {
                                            li { "{issue.message}" }
                                        }
                                    }
                                    label { class: "label items-start whitespace-normal text-sm text-base-content",
                                        input {
                                            r#type: "checkbox",
                                            class: "checkbox checkbox-sm checkbox-error mt-0.5",
                                            checked: override_compat.checked(),
                                            onchange: move |e| override_compat.on_change(e.checked()),
                                        }
                                        "Order anyway: I know these parts don't work together"
                                    }
                                    if let Some(message) = override_error {
                                        p { class: "text-xs text-error", "{message}" }
                                    }
                                }
                            }
                        }

                        button { r#type: "submit", class: "btn btn-primary", disabled: submitting || cart.lines.is_empty() || !cart.out_of_stock.is_empty(),
                            if submitting {
                                span { class: "loading loading-spinner loading-sm" }
                            }
                            "Place order"
                        }
                        if !cart.out_of_stock.is_empty() {
                            p { class: "text-xs text-error",
                                "Some items are out of stock: "
                                "{cart.out_of_stock.iter().filter_map(|id| catalog().get(id)).map(|p| p.name.as_str()).collect::<Vec<_>>().join(\", \")}"
                            }
                        }
                        Link { to: Route::Cart {}, class: "link link-hover text-center text-sm", "Back to the cart" }
                    }
                }
            }
        }
    }
}

/// A numbered checkout step: its title says exactly what it asks for, the hint what it is used
/// for.
#[component]
fn Section(number: u8, title: &'static str, hint: &'static str, children: Element) -> Element {
    rsx! {
        section { class: "flex flex-col gap-4",
            div { class: "flex flex-col gap-1",
                h2 { class: "flex items-center gap-3 text-lg font-semibold",
                    span { class: "grid size-7 place-items-center rounded-full bg-primary text-sm text-primary-content", "{number}" }
                    "{title}"
                }
                p { class: "ml-10 text-sm text-base-content/70", "{hint}" }
            }
            {children}
        }
    }
}
