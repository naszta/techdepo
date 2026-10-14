//! The customer's own things: cart, saved builds, address book, and checkout.
//!
//! All of it is per user within the org and written by the `Customer` virtual object.

use serde::{Deserialize, Serialize};
use techdepo_catalog::Category;
use techdepo_compat::Report;
use techdepo_domain::{Money, ProductId, Sku, ships_to};
use time::OffsetDateTime;

use crate::catalog::{Stock, check_part_list};
use crate::common::{Address, OrderRef};
use crate::error::{ApiError, FieldError, Validate, check_client_key, check_text};
use crate::ids::{AddressId, BuildId};
use crate::vocab::PaymentMethod;

/// Largest quantity of one product in a cart.
pub const MAX_CART_QUANTITY: u32 = 99;

/// Shipping is free in the demo; kept as a field so totals stay honest if that changes.
pub const SHIPPING_FEE: Money = Money::ZERO;

// ---------------------------------------------------------------------------------------------
// Cart
// ---------------------------------------------------------------------------------------------

/// One cart line, priced from the current catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CartLine {
    /// Product.
    pub product_id: ProductId,
    /// SKU.
    pub sku: Sku,
    /// Product name.
    pub name: String,
    /// Category.
    pub category: Category,
    /// Price per unit (VAT-inclusive).
    pub unit_price: Money,
    /// Quantity.
    pub quantity: u32,
    /// `unit_price × quantity`.
    pub line_total: Money,
    /// Stock in the org.
    pub stock: Stock,
}

impl CartLine {
    /// Whether the org has enough stock for this line.
    pub const fn in_stock(&self) -> bool {
        self.stock.stock >= self.quantity
    }
}

/// The caller's cart (`GET /api/cart` and every cart mutation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cart {
    /// Lines, in the order they were added.
    pub lines: Vec<CartLine>,
    /// Sum of the lines.
    pub items_total: Money,
    /// Shipping fee.
    pub shipping: Money,
    /// What checkout will charge. Send it back as [`PlaceOrderRequest::expected_total`].
    pub total: Money,
    /// Compatibility report of the build-relevant parts, if the cart has at least two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<Report>,
    /// Lines that exceed the org's stock.
    pub out_of_stock: Vec<ProductId>,
    /// Last change, absent for a cart that was never written.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub updated_at: Option<OffsetDateTime>,
}

impl Cart {
    /// Number of units in the cart (for the header badge).
    pub fn item_count(&self) -> u32 {
        self.lines.iter().map(|l| l.quantity).sum()
    }

    /// Whether checkout would be blocked by compatibility errors without an override.
    pub fn needs_compat_override(&self) -> bool {
        self.report.as_ref().is_some_and(|r| !r.is_compatible())
    }
}

/// `POST /api/cart/items`: add units of a product (adds to an existing line).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddToCartRequest {
    /// Product.
    pub product_id: ProductId,
    /// Units to add, at least 1.
    #[serde(default = "one")]
    pub quantity: u32,
}

const fn one() -> u32 {
    1
}

impl Validate for AddToCartRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        check_quantity(&mut errors, self.quantity, 1);
        ApiError::check(errors)
    }
}

/// `PUT /api/cart/items/{product_id}`: set a line's quantity; `0` removes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetCartItemRequest {
    /// New quantity.
    pub quantity: u32,
}

impl Validate for SetCartItemRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        check_quantity(&mut errors, self.quantity, 0);
        ApiError::check(errors)
    }
}

fn check_quantity(errors: &mut Vec<FieldError>, quantity: u32, min: u32) {
    if !(min..=MAX_CART_QUANTITY).contains(&quantity) {
        errors.push(FieldError::new(
            "quantity",
            format!("Must be between {min} and {MAX_CART_QUANTITY}"),
        ));
    }
}

// ---------------------------------------------------------------------------------------------
// Builds
// ---------------------------------------------------------------------------------------------

/// A saved build in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildSummary {
    /// Build ID.
    pub id: BuildId,
    /// Name.
    pub name: String,
    /// Number of parts.
    pub part_count: u32,
    /// Sum of the parts' current prices.
    pub total: Money,
    /// Overall compatibility.
    pub verdict: techdepo_compat::Verdict,
    /// Created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last saved.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// `GET /api/builds`: the caller's saved builds, most recently saved first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildList {
    /// Builds.
    pub builds: Vec<BuildSummary>,
}

/// One saved build with its live report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildDetail {
    /// Build ID.
    pub id: BuildId,
    /// Name.
    pub name: String,
    /// Parts in slot order. IDs no longer in the catalog are dropped (and listed in `removed`).
    pub product_ids: Vec<ProductId>,
    /// Parts that were saved but no longer exist in the catalog.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<ProductId>,
    /// Current compatibility report.
    pub report: Report,
    /// Required categories with no part yet.
    pub missing: Vec<Category>,
    /// Sum of the parts' current prices.
    pub total: Money,
    /// Created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last saved.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// `POST /api/builds`: save the builder's current parts under a name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateBuildRequest {
    /// Client-generated key (e.g. a UUID), so a retried request creates one build.
    pub idempotency_key: String,
    /// Name, 1–80 characters.
    pub name: String,
    /// Parts in slot order.
    pub product_ids: Vec<ProductId>,
}

impl Validate for CreateBuildRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        check_client_key(&mut errors, "idempotency_key", &self.idempotency_key);
        check_text(&mut errors, "name", &self.name, 1, 80);
        check_part_list(&mut errors, "product_ids", &self.product_ids);
        ApiError::check(errors)
    }
}

/// `PUT /api/builds/{id}`: rename and/or replace the parts. Absent fields stay unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateBuildRequest {
    /// New name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// New parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_ids: Option<Vec<ProductId>>,
}

impl Validate for UpdateBuildRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        if let Some(name) = &self.name {
            check_text(&mut errors, "name", name, 1, 80);
        }
        if let Some(ids) = &self.product_ids {
            check_part_list(&mut errors, "product_ids", ids);
        }
        ApiError::check(errors)
    }
}

// ---------------------------------------------------------------------------------------------
// Addresses
// ---------------------------------------------------------------------------------------------

/// A saved address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedAddress {
    /// Address ID.
    pub id: AddressId,
    /// Label, e.g. `Home`.
    pub label: String,
    /// The address.
    pub address: Address,
    /// Preselected as the shipping address at checkout.
    pub default_shipping: bool,
    /// Preselected as the billing address at checkout.
    pub default_billing: bool,
}

/// `GET /api/addresses`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressList {
    /// Addresses, defaults first.
    pub addresses: Vec<SavedAddress>,
}

/// `POST /api/addresses` (create) and `PUT /api/addresses/{id}` (replace).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveAddressRequest {
    /// Client-generated key for creates; ignored on `PUT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// Label, 1–40 characters.
    pub label: String,
    /// The address.
    pub address: Address,
    /// Make it the default shipping address (clears the flag on the others).
    #[serde(default)]
    pub default_shipping: bool,
    /// Make it the default billing address (clears the flag on the others).
    #[serde(default)]
    pub default_billing: bool,
}

impl Validate for SaveAddressRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        if let Some(key) = &self.idempotency_key {
            check_client_key(&mut errors, "idempotency_key", key);
        }
        check_text(&mut errors, "label", &self.label, 1, 40);
        self.address.check("address", &mut errors);
        ApiError::check(errors)
    }
}

// ---------------------------------------------------------------------------------------------
// Checkout
// ---------------------------------------------------------------------------------------------

/// `POST /api/checkout`: place an order from the cart. The order is created in
/// `awaiting_payment`, the stock is reserved, and the cart is emptied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceOrderRequest {
    /// Client-generated key (one per checkout page load), so a double click places one order.
    pub idempotency_key: String,
    /// Where to ship.
    pub shipping_address: Address,
    /// Billing address; absent means "same as shipping".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing_address: Option<Address>,
    /// Simulated payment method.
    pub payment_method: PaymentMethod,
    /// [`Cart::total`] as shown to the customer. If the cart changed since, the request fails
    /// with `cart_changed` and nothing is ordered.
    pub expected_total: Money,
    /// Place the order even though the cart's compatibility report has errors.
    #[serde(default)]
    pub override_compat: bool,
    /// Also save the shipping address to the address book.
    #[serde(default)]
    pub save_address: bool,
}

impl Validate for PlaceOrderRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        check_client_key(&mut errors, "idempotency_key", &self.idempotency_key);
        self.shipping_address.check("shipping_address", &mut errors);
        let country_ok = !errors.iter().any(|e| e.field == "shipping_address.country");
        if country_ok && !ships_to(&self.shipping_address.country) {
            errors.push(FieldError::new(
                "shipping_address.country",
                "We ship to EU countries only",
            ));
        }
        if let Some(billing) = &self.billing_address {
            billing.check("billing_address", &mut errors);
        }
        if !PaymentMethod::CUSTOMER.contains(&self.payment_method) {
            errors.push(FieldError::new(
                "payment_method",
                "Choose card or bank transfer",
            ));
        }
        if self.expected_total.cents() <= 0 {
            errors.push(FieldError::new("expected_total", "The cart is empty"));
        }
        ApiError::check(errors)
    }
}

/// Response of `POST /api/checkout`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceOrderResponse {
    /// The new order (`awaiting_payment`). Navigate to `/account/orders/<number>`.
    pub order: OrderRef,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address() -> Address {
        Address {
            name: "Jane Doe".into(),
            line1: "Andrássy út 1".into(),
            postal_code: "1061".into(),
            city: "Budapest".into(),
            country: "HU".into(),
            ..Address::default()
        }
    }

    #[test]
    fn place_order_validation() {
        let mut req = PlaceOrderRequest {
            idempotency_key: "0b6f6c1e-3f5a-4c55-9c1c-0d2f8e4b2a10".into(),
            shipping_address: address(),
            billing_address: None,
            payment_method: PaymentMethod::Card,
            expected_total: Money::from_cents(143_590),
            override_compat: false,
            save_address: false,
        };
        assert!(req.validate().is_ok());
        req.shipping_address.country = "Hungary".into();
        req.billing_address = Some(Address {
            city: String::new(),
            ..address()
        });
        let err = req.validate().unwrap_err();
        let fields: Vec<_> = err.fields.iter().map(|f| f.field.as_str()).collect();
        assert_eq!(fields, ["shipping_address.country", "billing_address.city"]);

        // A valid address outside the EU, and a method the customer cannot choose.
        req.billing_address = None;
        req.shipping_address.country = "CH".into();
        req.payment_method = PaymentMethod::Store;
        let err = req.validate().unwrap_err();
        let fields: Vec<_> = err.fields.iter().map(|f| f.field.as_str()).collect();
        assert_eq!(fields, ["shipping_address.country", "payment_method"]);
    }

    #[test]
    fn cart_quantities() {
        assert!(SetCartItemRequest { quantity: 0 }.validate().is_ok());
        assert!(SetCartItemRequest { quantity: 100 }.validate().is_err());
        let add: AddToCartRequest = serde_json::from_str(r#"{"product_id":"x"}"#).unwrap();
        assert_eq!(add.quantity, 1);
        assert!(AddToCartRequest { quantity: 0, ..add }.validate().is_err());
    }
}
