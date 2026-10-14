//! Glue between dioform and the API's validation errors.
//!
//! Request types implement `Validate` with field paths like `shipping_address.city`. Form models
//! mirror the request's shape (nested structs joined with `FieldPath::join`), so those paths are
//! exactly dioform's field identities: the same messages land under the same fields whether the
//! SPA's own check or the server's `422` produced them.

use dioform::prelude::*;
use dioform::{FieldIdentity, FormValidationError, SubmitError, SubmitErrors};
use dioxus::prelude::*;
use techdepo_api_types::{Address, ApiError};

use crate::components::input::InputField;

/// Field errors of `error` as form-validator errors (form-level when no field is named).
pub fn validation_errors(error: &ApiError) -> Vec<FormValidationError<String>> {
    if error.fields.is_empty() {
        return vec![FormValidationError::form(error.message.clone())];
    }
    error
        .fields
        .iter()
        .map(|f| {
            FormValidationError::field_identity(
                FieldIdentity::new(f.field.as_str()),
                f.message.clone(),
            )
        })
        .collect()
}

/// An API error as submit errors: one per invalid field, or one for the whole form.
pub fn submit_errors<Model>(error: &ApiError) -> SubmitErrors<Model, String> {
    if error.fields.is_empty() {
        return SubmitErrors::from(SubmitError::form(error.message.clone()));
    }
    error
        .fields
        .iter()
        .map(|f| {
            SubmitError::field_identity(FieldIdentity::new(f.field.as_str()), f.message.clone())
        })
        .collect()
}

/// Form-level errors (not tied to a field) as an alert.
#[component]
pub fn FormErrors(errors: Vec<String>) -> Element {
    if errors.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { role: "alert", class: "alert alert-error alert-soft",
            ul { class: "flex flex-col gap-1",
                for (i , error) in errors.into_iter().enumerate() {
                    li { key: "{i}", "{error}" }
                }
            }
        }
    }
}

/// An address as typed: optional parts are empty strings until submit.
#[derive(Clone, Default, PartialEq, Form)]
pub struct AddressModel {
    pub name: String,
    pub company: String,
    pub line1: String,
    pub line2: String,
    pub postal_code: String,
    pub city: String,
    pub country: String,
    pub phone: String,
    pub vat_id: String,
}

impl AddressModel {
    /// The form value of a saved address.
    pub fn from_address(a: &Address) -> Self {
        Self {
            name: a.name.clone(),
            company: a.company.clone().unwrap_or_default(),
            line1: a.line1.clone(),
            line2: a.line2.clone().unwrap_or_default(),
            postal_code: a.postal_code.clone(),
            city: a.city.clone(),
            country: a.country.clone(),
            phone: a.phone.clone().unwrap_or_default(),
            vat_id: a.vat_id.clone().unwrap_or_default(),
        }
    }

    /// The address as sent: trimmed, optional parts `None` when blank, country upper case.
    pub fn to_address(&self) -> Address {
        let optional = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_owned());
        Address {
            name: self.name.trim().to_owned(),
            company: optional(&self.company),
            line1: self.line1.trim().to_owned(),
            line2: optional(&self.line2),
            postal_code: self.postal_code.trim().to_owned(),
            city: self.city.trim().to_owned(),
            country: self.country.trim().to_uppercase(),
            phone: optional(&self.phone),
            vat_id: optional(&self.vat_id),
        }
    }
}

/// The fields of one address, bound under `prefix` (e.g. `shipping_address`, so the API's
/// `shipping_address.city` errors land on the right input).
#[component]
pub fn AddressFields<M: Clone + PartialEq + 'static>(
    form: FormHandle<M>,
    prefix: FieldPath<M, AddressModel>,
) -> Element {
    let a = AddressModel::fields();
    let path = |child: FieldPath<AddressModel, String>| form.field(prefix.clone().join(child));
    rsx! {
        // `items-start`: a field growing an error or a hint must not re-center its neighbour.
        div { class: "grid items-start gap-4 sm:grid-cols-2",
            InputField { context: path(a.name()), label: "Full name", autocomplete: "name", class: "w-full" }
            InputField { context: path(a.company()), label: "Company (optional)", autocomplete: "organization", class: "w-full" }
            div { class: "sm:col-span-2",
                InputField { context: path(a.line1()), label: "Street and number", autocomplete: "address-line1", class: "w-full" }
            }
            div { class: "sm:col-span-2",
                InputField { context: path(a.line2()), label: "Apartment, floor (optional)", autocomplete: "address-line2", class: "w-full" }
            }
            InputField { context: path(a.postal_code()), label: "Postal code", autocomplete: "postal-code", class: "w-full" }
            InputField { context: path(a.city()), label: "City", autocomplete: "address-level2", class: "w-full" }
            InputField {
                context: path(a.country()),
                label: "Country",
                description: "Two-letter code, e.g. HU, DE, AT. We ship within the EU.",
                autocomplete: "country",
                maxlength: "2",
                class: "w-full uppercase",
            }
            InputField { context: path(a.phone()), label: "Phone (optional)", r#type: "tel", autocomplete: "tel", class: "w-full" }
            InputField { context: path(a.vat_id()), label: "VAT number (optional)", class: "w-full" }
        }
    }
}
