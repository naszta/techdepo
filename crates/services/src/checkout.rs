//! Checkout as a pure function: from a journaled snapshot of the cart, the stock and the org
//! settings to the order to place, or the `409` that stops it. `Customer/checkout` runs it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use techdepo_api_types::account::Cart;
use techdepo_api_types::ingress::CheckoutInput;
use techdepo_api_types::vocab::PaymentMethod;
use techdepo_api_types::{ApiError, ErrorCode, FieldError};
use techdepo_catalog::Catalog;
use techdepo_compat::Severity;
use techdepo_db::carts::{CartItemRow, CartRows};
use techdepo_db::orders::NewOrderItem;
use techdepo_db::views;
use techdepo_domain::{Money, OrgSettings, ProductId, VatBreakdown, VatRate, ships_to};
use time::OffsetDateTime;

/// What checkout reads from D1 in one step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckoutSnapshot {
    /// Cart lines in the order they were added.
    pub lines: Vec<(ProductId, u32)>,
    /// The org's stock for the cart's products.
    pub stock: Vec<(ProductId, u32)>,
    /// The org's settings (VAT rate, default faults).
    pub settings: OrgSettings,
}

/// The order checkout will place.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckoutPlan {
    /// Lines as sold.
    pub items: Vec<NewOrderItem>,
    /// Shipping fee.
    pub shipping: Money,
    /// What the customer pays (lines plus shipping).
    pub total: Money,
    /// The total's VAT breakdown at the org's rate.
    pub vat: VatBreakdown,
    /// Compatibility errors were overridden.
    pub compat_override: bool,
}

/// The priced cart of a snapshot (the same pricing as `GET /api/cart`).
pub fn price(snapshot: &CheckoutSnapshot, catalog: &Catalog) -> Cart {
    let rows = CartRows {
        items: snapshot
            .lines
            .iter()
            .map(|(product_id, quantity)| CartItemRow {
                product_id: product_id.clone(),
                quantity: *quantity,
                added_at: OffsetDateTime::UNIX_EPOCH,
                updated_at: OffsetDateTime::UNIX_EPOCH,
            })
            .collect(),
        updated_at: None,
    };
    let stock: HashMap<ProductId, u32> = snapshot.stock.iter().cloned().collect();
    views::cart(&rows, &stock, catalog)
}

/// Checks the snapshot against what the customer saw and agreed to, in this order: the input
/// is valid (`422`), the cart is unchanged (`cart_changed`), everything is in stock
/// (`out_of_stock`), and the parts are compatible unless overridden (`incompatible_build`).
pub fn plan(
    snapshot: &CheckoutSnapshot,
    input: &CheckoutInput,
    catalog: &Catalog,
) -> Result<CheckoutPlan, ApiError> {
    let mut fields = Vec::new();
    if !PaymentMethod::CUSTOMER.contains(&input.payment_method) {
        fields.push(FieldError::new(
            "payment_method",
            "Choose card or bank transfer",
        ));
    }
    if !ships_to(&input.shipping_address.country) {
        fields.push(FieldError::new(
            "shipping_address.country",
            "We ship to EU countries only",
        ));
    }
    ApiError::check(fields)?;

    let cart = price(snapshot, catalog);
    if cart.lines.is_empty() {
        return Err(ApiError::new(ErrorCode::CartChanged, "Your cart is empty"));
    }
    if cart.lines.len() != snapshot.lines.len() {
        return Err(ApiError::new(
            ErrorCode::CartChanged,
            "Some products in your cart are no longer sold; review your cart",
        ));
    }
    if cart.total != input.expected_total {
        return Err(ApiError::new(
            ErrorCode::CartChanged,
            format!("Your cart changed: the total is now {}", cart.total),
        ));
    }
    if let Some(line) = cart.lines.iter().find(|l| !l.in_stock()) {
        let message = match line.stock.stock {
            0 => format!("{} is out of stock", line.name),
            n => format!("Only {n} left of {}", line.name),
        };
        return Err(ApiError::new(ErrorCode::OutOfStock, message));
    }
    let compat_override = cart.needs_compat_override();
    if compat_override && !input.override_compat {
        let first = cart
            .report
            .as_ref()
            .and_then(|r| r.with_severity(Severity::Error).next())
            .map_or_else(
                || "The parts in your cart are not compatible".to_owned(),
                |i| i.message.clone(),
            );
        return Err(ApiError::new(ErrorCode::IncompatibleBuild, first));
    }

    let items = cart
        .lines
        .iter()
        .map(|l| NewOrderItem {
            product_id: l.product_id.clone(),
            sku: l.sku.clone(),
            name: l.name.clone(),
            category: l.category,
            unit_price: l.unit_price,
            quantity: l.quantity,
        })
        .collect();
    Ok(CheckoutPlan {
        items,
        shipping: cart.shipping,
        total: cart.total,
        vat: totals(cart.total, snapshot.settings.vat_rate()),
        compat_override,
    })
}

/// A VAT-inclusive total split at `rate`.
pub fn totals(total: Money, rate: VatRate) -> VatBreakdown {
    total.vat_breakdown(rate)
}

#[cfg(test)]
mod tests {
    use techdepo_api_types::Address;
    use techdepo_api_types::ingress::CustomerInput;
    use techdepo_catalog::Category;
    use techdepo_domain::{OrgId, UserId};

    use super::*;

    fn input(expected_total: Money) -> CheckoutInput {
        CheckoutInput {
            customer: CustomerInput {
                org_id: OrgId::new("org_1"),
                user_id: UserId::new("user_1"),
            },
            customer_email: "ada@example.com".into(),
            customer_name: None,
            shipping_address: Address {
                name: "Ada".into(),
                line1: "Fő utca 1".into(),
                postal_code: "1011".into(),
                city: "Budapest".into(),
                country: "HU".into(),
                ..Address::default()
            },
            billing_address: None,
            payment_method: PaymentMethod::Card,
            expected_total,
            override_compat: false,
            save_address: false,
        }
    }

    fn snapshot(lines: &[(&ProductId, u32)], stock: u32) -> CheckoutSnapshot {
        CheckoutSnapshot {
            lines: lines.iter().map(|(id, q)| ((*id).clone(), *q)).collect(),
            stock: lines.iter().map(|(id, _)| ((*id).clone(), stock)).collect(),
            settings: OrgSettings::default(),
        }
    }

    #[test]
    fn plans_an_order_with_vat() {
        let catalog = Catalog::global();
        let cpu = &catalog.by_category(Category::Cpu)[0];
        let snap = snapshot(&[(&cpu.id, 2)], 10);
        let plan = plan(&snap, &input(cpu.price * 2_u32), catalog).unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.total, cpu.price * 2_u32);
        assert_eq!(plan.vat.net + plan.vat.vat, plan.total);
        assert_eq!(plan.vat, plan.total.vat_breakdown(VatRate::DEFAULT));
        assert!(!plan.compat_override);
    }

    #[test]
    fn rejects_changed_carts_and_missing_stock() {
        let catalog = Catalog::global();
        let cpu = &catalog.by_category(Category::Cpu)[0];
        let snap = snapshot(&[(&cpu.id, 2)], 1);
        let err = plan(&snap, &input(cpu.price), catalog).unwrap_err();
        assert_eq!(err.code, ErrorCode::CartChanged);
        let err = plan(&snap, &input(cpu.price * 2_u32), catalog).unwrap_err();
        assert_eq!(err.code, ErrorCode::OutOfStock);
        assert_eq!(err.message, format!("Only 1 left of {}", cpu.name));
        let empty = snapshot(&[], 0);
        assert_eq!(
            plan(&empty, &input(Money::ZERO), catalog).unwrap_err().code,
            ErrorCode::CartChanged
        );
        let gone = ProductId::new("no-such-product");
        let err = plan(&snapshot(&[(&gone, 1)], 5), &input(Money::ZERO), catalog).unwrap_err();
        assert_eq!(err.code, ErrorCode::CartChanged);
    }

    #[test]
    fn incompatible_parts_need_an_override() {
        let catalog = Catalog::global();
        // The DDR5 board + DDR4 kit trap from the compat docs.
        let board = ProductId::new("kestrel-b650m-wing");
        let ram = ProductId::new("nimbus-ddr4-3200-16");
        let snap = snapshot(&[(&board, 1), (&ram, 1)], 5);
        let total = price(&snap, catalog).total;
        let err = plan(&snap, &input(total), catalog).unwrap_err();
        assert_eq!(err.code, ErrorCode::IncompatibleBuild);
        let mut overridden = input(total);
        overridden.override_compat = true;
        assert!(plan(&snap, &overridden, catalog).unwrap().compat_override);
    }

    #[test]
    fn validates_country_and_method() {
        let catalog = Catalog::global();
        let cpu = &catalog.by_category(Category::Cpu)[0];
        let snap = snapshot(&[(&cpu.id, 1)], 5);
        let mut bad = input(cpu.price);
        bad.shipping_address.country = "CH".into();
        bad.payment_method = PaymentMethod::Store;
        let err = plan(&snap, &bad, catalog).unwrap_err();
        assert_eq!(err.code, ErrorCode::Validation);
        assert_eq!(err.fields.len(), 2);
    }
}
