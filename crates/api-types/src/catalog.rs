//! Catalog, product pages, per-org stock and the compatibility API.
//!
//! The catalog itself is compiled into the SPA too (`techdepo-catalog`), so `/api/catalog` is
//! mostly for cache checks and non-Rust clients. Stock is per org and always comes from D1.

use serde::{Deserialize, Serialize};
use techdepo_catalog::{Category, Product, ProductContent};
use techdepo_compat::{Fix, Report};
use techdepo_domain::{Money, ProductId, labeled_enum};

use crate::error::{ApiError, FieldError, Validate};

/// Stock at or below this is "low" (highlighted in admin, "only N left" in the shop).
pub const LOW_STOCK_THRESHOLD: u32 = 3;

/// Most products a build, compat check or cart may hold.
pub const MAX_BUILD_PARTS: usize = 16;

labeled_enum! {
    /// Coarse stock level for badges.
    pub enum StockLevel {
        /// More than [`LOW_STOCK_THRESHOLD`].
        InStock => ("in_stock", "In stock"),
        /// 1 to [`LOW_STOCK_THRESHOLD`].
        Low => ("low", "Low stock"),
        /// None left.
        OutOfStock => ("out_of_stock", "Out of stock"),
    }
}

impl StockLevel {
    /// The level for a stock count.
    pub const fn of(stock: u32) -> Self {
        match stock {
            0 => Self::OutOfStock,
            n if n <= LOW_STOCK_THRESHOLD => Self::Low,
            _ => Self::InStock,
        }
    }
}

/// `GET /api/catalog`: every product (without long-form content). Cacheable: the response
/// carries `ETag: "<version>"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogResponse {
    /// `techdepo_catalog::CATALOG_VERSION`.
    pub version: String,
    /// All products, grouped by category in catalog order.
    pub products: Vec<Product>,
}

/// One product's stock in the caller's org.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stock {
    /// Units available.
    pub stock: u32,
    /// Badge level.
    pub level: StockLevel,
}

impl Stock {
    /// Stock with its level.
    pub const fn new(stock: u32) -> Self {
        Self {
            stock,
            level: StockLevel::of(stock),
        }
    }
}

/// One row of `GET /api/stock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StockEntry {
    /// Product.
    pub product_id: ProductId,
    /// Its stock.
    #[serde(flatten)]
    pub stock: Stock,
}

/// `GET /api/stock`: stock of every product in the caller's org.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StockResponse {
    /// One entry per catalog product.
    pub items: Vec<StockEntry>,
}

/// A product's long-form content, split into the sections the product page shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductContentDto {
    /// Overview (Markdown).
    pub overview: String,
    /// "Who it's for" (Markdown).
    pub who_its_for: String,
    /// Pros, one per item.
    pub pros: Vec<String>,
    /// Cons, one per item.
    pub cons: Vec<String>,
    /// Frequently asked questions.
    pub faq: Vec<FaqItem>,
}

/// One FAQ entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaqItem {
    /// Question.
    pub question: String,
    /// Answer (Markdown).
    pub answer: String,
}

impl From<&ProductContent> for ProductContentDto {
    fn from(content: &ProductContent) -> Self {
        let owned = |items: Vec<&str>| items.into_iter().map(str::to_owned).collect();
        Self {
            overview: content.overview().unwrap_or_default().to_owned(),
            who_its_for: content.who_its_for().unwrap_or_default().to_owned(),
            pros: owned(content.pros()),
            cons: owned(content.cons()),
            faq: content
                .faq()
                .into_iter()
                .map(|e| FaqItem {
                    question: e.question.to_owned(),
                    answer: e.answer.to_owned(),
                })
                .collect(),
        }
    }
}

/// `GET /api/products/{id}`: the product page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductDetail {
    /// The product with its typed spec.
    pub product: Product,
    /// Rich content.
    pub content: ProductContentDto,
    /// Stock in the caller's org.
    pub stock: Stock,
    /// "Works well with": compatible products from other categories in the same tier, a few
    /// per category, cheapest first.
    pub works_well_with: Vec<ProductId>,
}

/// `POST /api/compat/check`: check a set of parts as one build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatCheckRequest {
    /// The parts, in any order. Repeats are allowed for memory kits and drives.
    pub product_ids: Vec<ProductId>,
    /// Also compute "fix it" suggestions.
    #[serde(default)]
    pub include_fixes: bool,
}

impl Validate for CompatCheckRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        check_part_list(&mut errors, "product_ids", &self.product_ids);
        ApiError::check(errors)
    }
}

/// Response of `POST /api/compat/check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatCheckResponse {
    /// The engine's report.
    pub report: Report,
    /// "Fix it" suggestions (empty unless requested).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<Fix>,
    /// Required categories with no part yet.
    pub missing: Vec<Category>,
    /// Sum of the parts' prices.
    pub total: Money,
}

/// `POST /api/compat/alternatives`: compatible replacements for one slot within a price band.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlternativesRequest {
    /// The current build.
    pub product_ids: Vec<ProductId>,
    /// The category to replace (or fill).
    pub category: Category,
    /// Lowest price, inclusive. Defaults to no lower bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_min: Option<Money>,
    /// Highest price, inclusive. Defaults to no upper bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_max: Option<Money>,
}

impl Validate for AlternativesRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        check_part_list(&mut errors, "product_ids", &self.product_ids);
        if let (Some(min), Some(max)) = (self.price_min, self.price_max)
            && min > max
        {
            errors.push(FieldError::new(
                "price_max",
                "Must be at least the minimum price",
            ));
        }
        ApiError::check(errors)
    }
}

/// Response of `POST /api/compat/alternatives`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlternativesResponse {
    /// Candidates that keep the build free of errors, best match first.
    pub candidates: Vec<ProductId>,
}

/// Pushes an error if `ids` is longer than [`MAX_BUILD_PARTS`].
pub(crate) fn check_part_list(errors: &mut Vec<FieldError>, field: &str, ids: &[ProductId]) {
    if ids.len() > MAX_BUILD_PARTS {
        errors.push(FieldError::new(
            field,
            format!("At most {MAX_BUILD_PARTS} parts"),
        ));
    }
}

#[cfg(test)]
mod tests {
    use techdepo_catalog::Catalog;

    use super::*;

    #[test]
    fn stock_levels() {
        assert_eq!(StockLevel::of(0), StockLevel::OutOfStock);
        assert_eq!(StockLevel::of(LOW_STOCK_THRESHOLD), StockLevel::Low);
        assert_eq!(StockLevel::of(LOW_STOCK_THRESHOLD + 1), StockLevel::InStock);
        let entry = StockEntry {
            product_id: ProductId::new("x"),
            stock: Stock::new(2),
        };
        assert_eq!(
            serde_json::to_value(&entry).unwrap(),
            serde_json::json!({ "product_id": "x", "stock": 2, "level": "low" })
        );
    }

    #[test]
    fn content_converts() {
        let catalog = Catalog::global();
        let product = &catalog.all()[0];
        let dto = ProductContentDto::from(catalog.content(&product.id).unwrap());
        assert!(!dto.overview.is_empty());
        assert!(!dto.faq.is_empty());
    }
}
