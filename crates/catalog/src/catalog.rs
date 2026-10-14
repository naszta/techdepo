//! Loading the catalog from YAML + Markdown, and lookups.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::OnceLock;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use techdepo_domain::{Money, ProductId, Sku};

use crate::category::{Category, Tier};
use crate::content::ProductContent;
use crate::product::Product;
use crate::spec::ProductSpec;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}

pub use embedded::CATALOG_VERSION;

/// The catalog source could not be parsed.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// A YAML file is not named after a category.
    #[error(
        "`{0}.yaml` is not a category file (expected one of: cpu, motherboard, memory, storage, gpu, case, psu, cooler)"
    )]
    UnknownCategoryFile(String),
    /// Two YAML sources for the same category.
    #[error("category `{0}` is defined twice")]
    DuplicateCategory(Category),
    /// A YAML file does not match the schema.
    #[error("{file}: {source}")]
    Yaml {
        /// Source file name.
        file: String,
        /// Parser error, with line and column.
        #[source]
        source: serde_yaml::Error,
    },
}

/// One product as written in a category YAML file. The category comes from the file name.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry<S> {
    id: ProductId,
    sku: Sku,
    brand: String,
    name: String,
    price_cents: i64,
    default_stock: u32,
    tier: Tier,
    tags: Vec<String>,
    summary: String,
    spec: S,
}

/// The product catalog: every product, grouped by category in file order, plus rich content.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    products: Vec<Product>,
    ranges: [Range<usize>; Category::COUNT],
    index: BTreeMap<ProductId, usize>,
    content: BTreeMap<ProductId, ProductContent>,
}

impl Catalog {
    /// The catalog compiled into this binary, parsed once.
    ///
    /// # Panics
    ///
    /// If the embedded data does not parse. The crate's tests and `xtask catalog validate`
    /// guarantee it does.
    pub fn global() -> &'static Catalog {
        static CATALOG: OnceLock<Catalog> = OnceLock::new();
        CATALOG.get_or_init(|| {
            Self::load_embedded().unwrap_or_else(|e| panic!("embedded catalog is invalid: {e}"))
        })
    }

    /// Parses the catalog compiled into this binary.
    pub fn load_embedded() -> Result<Self, LoadError> {
        let mut files = Vec::with_capacity(embedded::CATEGORY_FILES.len());
        for (stem, yaml) in embedded::CATEGORY_FILES {
            let category = stem
                .parse::<Category>()
                .map_err(|_| LoadError::UnknownCategoryFile((*stem).to_owned()))?;
            files.push((category, *yaml));
        }
        Self::from_sources(files, embedded::CONTENT_FILES.iter().copied())
    }

    /// Parses a catalog from category YAML documents and `(product id, markdown)` pairs.
    ///
    /// Only parsing happens here; run [`Catalog::validate`] for the consistency checks.
    pub fn from_sources<'a>(
        categories: impl IntoIterator<Item = (Category, &'a str)>,
        content: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, LoadError> {
        let mut by_category: [Option<Vec<Product>>; Category::COUNT] = Default::default();
        for (category, yaml) in categories {
            let slot = &mut by_category[category.index()];
            if slot.is_some() {
                return Err(LoadError::DuplicateCategory(category));
            }
            *slot = Some(parse_category(category, yaml)?);
        }

        let mut catalog = Self::default();
        for category in Category::ALL {
            let start = catalog.products.len();
            catalog
                .products
                .extend(by_category[category.index()].take().unwrap_or_default());
            catalog.ranges[category.index()] = start..catalog.products.len();
        }
        for (i, product) in catalog.products.iter().enumerate() {
            catalog.index.entry(product.id.clone()).or_insert(i);
        }
        catalog.content = content
            .into_iter()
            .map(|(id, markdown)| (ProductId::new(id), ProductContent::new(markdown)))
            .collect();
        Ok(catalog)
    }

    /// Looks a product up by ID.
    pub fn get(&self, id: &(impl AsRef<str> + ?Sized)) -> Option<&Product> {
        self.index.get(id.as_ref()).map(|&i| &self.products[i])
    }

    /// Every product, grouped by category in [`Category::ALL`] order.
    pub fn all(&self) -> &[Product] {
        &self.products
    }

    /// The products of one category, in file order.
    pub fn by_category(&self, category: Category) -> &[Product] {
        &self.products[self.ranges[category.index()].clone()]
    }

    /// Rich content for a product.
    pub fn content(&self, id: &(impl AsRef<str> + ?Sized)) -> Option<&ProductContent> {
        self.content.get(id.as_ref())
    }

    /// Every content entry, including ones without a product (validation reports those).
    pub fn contents(&self) -> impl Iterator<Item = (&ProductId, &ProductContent)> {
        self.content.iter()
    }

    /// Number of products.
    pub fn len(&self) -> usize {
        self.products.len()
    }

    /// No products at all.
    pub fn is_empty(&self) -> bool {
        self.products.is_empty()
    }
}

fn parse_category(category: Category, yaml: &str) -> Result<Vec<Product>, LoadError> {
    match category {
        Category::Cpu => parse_entries(category, yaml, ProductSpec::Cpu),
        Category::Motherboard => parse_entries(category, yaml, ProductSpec::Motherboard),
        Category::Memory => parse_entries(category, yaml, ProductSpec::Memory),
        Category::Storage => parse_entries(category, yaml, ProductSpec::Storage),
        Category::Gpu => parse_entries(category, yaml, ProductSpec::Gpu),
        Category::Case => parse_entries(category, yaml, ProductSpec::Case),
        Category::Psu => parse_entries(category, yaml, ProductSpec::Psu),
        Category::Cooler => parse_entries(category, yaml, ProductSpec::Cooler),
    }
}

fn parse_entries<S: DeserializeOwned>(
    category: Category,
    yaml: &str,
    wrap: fn(S) -> ProductSpec,
) -> Result<Vec<Product>, LoadError> {
    let entries: Vec<Entry<S>> = serde_yaml::from_str(yaml).map_err(|source| LoadError::Yaml {
        file: format!("{}.yaml", category.as_str()),
        source,
    })?;
    Ok(entries
        .into_iter()
        .map(|e| Product {
            id: e.id,
            sku: e.sku,
            brand: e.brand,
            name: e.name,
            category,
            price: Money::from_cents(e.price_cents),
            default_stock: e.default_stock,
            tier: e.tier,
            tags: e.tags,
            summary: e.summary,
            spec: wrap(e.spec),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Socket;
    use crate::spec::{CoolerKind, StorageInterface};

    const COOLER: &str = r#"
- id: test-cooler
  sku: COOL-TEST
  brand: Test
  name: Test Cooler
  price_cents: 3990
  default_stock: 5
  tier: budget
  tags: [quiet]
  summary: A cooler.
  spec:
    sockets: [am5, lga1851]
    kind: { type: air, height_mm: 150 }
    tdp_rating_w: 180
"#;

    #[test]
    fn parses_a_category_file() {
        let catalog = Catalog::from_sources([(Category::Cooler, COOLER)], []).unwrap();
        let p = catalog.get("test-cooler").unwrap();
        assert_eq!(p.category, Category::Cooler);
        assert_eq!(p.price, Money::from_cents(3990));
        let spec = p.spec.as_cooler().unwrap();
        assert_eq!(
            spec.kind,
            CoolerKind::Air {
                height_mm: 150,
                ram_clearance_mm: None
            }
        );
        assert!(spec.supports(Socket::Am5));
        assert_eq!(catalog.by_category(Category::Cooler).len(), 1);
        assert!(catalog.by_category(Category::Cpu).is_empty());
        assert!(catalog.get(&ProductId::new("test-cooler")).is_some());
    }

    #[test]
    fn rejects_unknown_fields() {
        let yaml = COOLER.replace("tdp_rating_w: 180", "tdp_rating_w: 180\n    colour: red");
        let err = Catalog::from_sources([(Category::Cooler, yaml.as_str())], []).unwrap_err();
        assert!(err.to_string().contains("colour"), "{err}");

        let yaml = COOLER.replace("height_mm: 150", "height_mm: 150, rgb: true");
        let err = Catalog::from_sources([(Category::Cooler, yaml.as_str())], []).unwrap_err();
        assert!(err.to_string().contains("rgb"), "{err}");

        let yaml = COOLER.replace("type: air", "type: hybrid");
        assert!(Catalog::from_sources([(Category::Cooler, yaml.as_str())], []).is_err());
    }

    #[test]
    fn storage_interface_is_tagged() {
        let nvme: StorageInterface = serde_yaml::from_str("{ type: nvme, pcie_gen: 4 }").unwrap();
        assert!(matches!(nvme, StorageInterface::Nvme { .. }));
        let sata: StorageInterface = serde_yaml::from_str("{ type: sata }").unwrap();
        assert_eq!(sata, StorageInterface::Sata);
        // serde ignores extra keys on unit variants of internally tagged enums, so
        // `{ type: sata, pcie_gen: 3 }` is accepted; struct variants do reject unknown keys.
        assert!(
            serde_yaml::from_str::<StorageInterface>("{ type: nvme, pcie_gen: 4, x: 1 }").is_err()
        );
    }

    #[test]
    fn duplicate_category_is_rejected() {
        let err =
            Catalog::from_sources([(Category::Cooler, COOLER), (Category::Cooler, COOLER)], [])
                .unwrap_err();
        assert!(matches!(
            err,
            LoadError::DuplicateCategory(Category::Cooler)
        ));
    }
}
