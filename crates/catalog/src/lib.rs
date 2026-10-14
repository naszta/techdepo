//! The TechDepó product catalog: typed specs and the data compiled in from `data/catalog`.
//!
//! The catalog is global and read-only. Category YAML files and per-product Markdown content
//! are embedded at build time (see `build.rs` and `data/catalog/README.md`) and parsed once
//! on first use:
//!
//! ```
//! use techdepo_catalog::{Catalog, Category};
//!
//! let catalog = Catalog::global();
//! for cpu in catalog.by_category(Category::Cpu) {
//!     let spec = cpu.spec.as_cpu().unwrap();
//!     println!("{} on {}: {}", cpu.name, spec.socket, cpu.price);
//! }
//! ```

mod catalog;
mod category;
mod content;
mod mentions;
mod platform;
mod product;
pub mod spec;
mod tier;
mod validate;

pub use catalog::{CATALOG_VERSION, Catalog, LoadError};
pub use category::{Category, Tier};
pub use content::{FaqEntry, ProductContent, REQUIRED_SECTIONS, Section};
pub use mentions::{MentionTarget, ProductMention};
pub use platform::{
    Chipset, CpuFamily, FormFactor, InvalidPcieGen, MemoryType, PcieGen, PsuFormFactor, Socket,
};
pub use product::Product;
pub use spec::ProductSpec;
pub use tier::PriceBands;
pub use validate::{ValidationError, ValidationErrorKind, ValidationErrors};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_is_valid() {
        let catalog = Catalog::load_embedded().expect("embedded catalog parses");
        if let Err(errors) = catalog.validate() {
            panic!("{errors}");
        }
        for category in Category::ALL {
            assert!(!catalog.by_category(*category).is_empty(), "{category:?}");
        }
    }

    #[test]
    fn global_is_shared_and_versioned() {
        assert!(std::ptr::eq(Catalog::global(), Catalog::global()));
        assert_eq!(CATALOG_VERSION.len(), 16);
    }

    #[test]
    fn products_round_trip_through_json() {
        for product in Catalog::global().all() {
            let json = serde_json::to_string(product).unwrap();
            let back: Product = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, product);
        }
    }

    #[test]
    fn every_product_has_content() {
        let catalog = Catalog::global();
        for product in catalog.all() {
            let content = catalog.content(&product.id).unwrap();
            assert!(!content.faq().is_empty(), "{}", product.id);
        }
    }
}
