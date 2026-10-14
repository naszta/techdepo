use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use techdepo_domain::{Money, ProductId, Sku};

use crate::category::{Category, Tier};
use crate::spec::ProductSpec;

/// A catalog product. Global and read-only; only stock is per org.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Product {
    /// Stable lower-case slug, e.g. `voltaris-v5-8600`.
    pub id: ProductId,
    /// Stock keeping unit, e.g. `CPU-VOL-8600`.
    pub sku: Sku,
    /// Fictional brand, e.g. `Voltaris`.
    pub brand: String,
    /// Full display name including the brand, e.g. `Voltaris V5 8600`.
    pub name: String,
    /// Category; always equal to `spec.category()`.
    pub category: Category,
    /// VAT-inclusive price.
    pub price: Money,
    /// Stock seeded into each new org.
    pub default_stock: u32,
    /// Price tier within the category.
    pub tier: Tier,
    /// Lower-case search/facet tags such as `gaming` or `quiet`.
    pub tags: Vec<String>,
    /// One-sentence pitch for listings.
    pub summary: String,
    /// Typed specification.
    pub spec: ProductSpec,
}
