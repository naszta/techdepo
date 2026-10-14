//! A PC build: at most one part per single slot, any number of memory kits and drives.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use techdepo_catalog::spec::{
    CaseSpec, CoolerSpec, CpuSpec, GpuSpec, MemorySpec, MotherboardSpec, PsuSpec, StorageSpec,
};
use techdepo_catalog::{Catalog, Category, Product, ProductSpec};
use techdepo_domain::{Money, ProductId};

/// A build could not be assembled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    /// The ID is not in the catalog.
    #[error("unknown product `{0}`")]
    UnknownProduct(ProductId),
    /// Two products for a slot that takes one (two CPUs, two cases, …).
    #[error("the {} slot already holds `{existing}`, cannot add `{added}`", .category.as_str())]
    SlotTaken {
        /// The single slot.
        category: Category,
        /// The part already in it.
        existing: ProductId,
        /// The part that did not fit.
        added: ProductId,
    },
    /// The product's category does not match the slot.
    #[error("`{product}` is {}, it does not go in the {slot} slot", .found.as_str())]
    WrongCategory {
        /// Target slot.
        slot: Slot,
        /// The product.
        product: ProductId,
        /// The product's category.
        found: Category,
    },
    /// The slot index is past the end (or non-zero for a single slot).
    #[error("no slot {slot}")]
    NoSuchSlot {
        /// The rejected slot.
        slot: Slot,
    },
}

/// A position in a build: a category plus an index for the multi-part categories (memory kits
/// and drives). Single-part categories always use index 0.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct Slot {
    /// The category the slot holds.
    pub category: Category,
    /// Position among the category's parts; always 0 for single slots.
    pub index: usize,
}

impl Slot {
    /// Slot `index` of `category`.
    pub const fn new(category: Category, index: usize) -> Self {
        Self { category, index }
    }

    /// Whether the category takes several parts (memory kits, drives).
    pub const fn is_multi(category: Category) -> bool {
        matches!(category, Category::Memory | Category::Storage)
    }
}

impl From<Category> for Slot {
    /// The first (for single categories: the only) slot of `category`.
    fn from(category: Category) -> Self {
        Self::new(category, 0)
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if Self::is_multi(self.category) {
            write!(f, "{} #{}", self.category.as_str(), self.index + 1)
        } else {
            f.write_str(self.category.as_str())
        }
    }
}

/// A product in a build together with its typed spec.
#[derive(Debug)]
pub struct Part<'a, S> {
    /// The catalog product.
    pub product: &'a Product,
    /// Its spec, already narrowed to the category.
    pub spec: &'a S,
}

impl<S> Clone for Part<'_, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for Part<'_, S> {}

impl<S> fmt::Display for Part<'_, S> {
    /// The product name.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.product.name)
    }
}

impl<'a, S> Part<'a, S> {
    /// Product ID.
    pub fn id(&self) -> &'a ProductId {
        &self.product.id
    }

    /// Display name, e.g. `Voltaris V5 8600`.
    pub fn name(&self) -> &'a str {
        &self.product.name
    }
}

/// A (possibly incomplete) PC build over catalog products.
///
/// Built from product references, so it is cheap to clone; [`check_build`](crate::check_build)
/// evaluates it. Parts are kept in typed form, so a CPU can only ever sit in the CPU slot.
#[derive(Debug, Clone, Default)]
pub struct Build<'a> {
    cpu: Option<Part<'a, CpuSpec>>,
    motherboard: Option<Part<'a, MotherboardSpec>>,
    memory: Vec<Part<'a, MemorySpec>>,
    storage: Vec<Part<'a, StorageSpec>>,
    gpu: Option<Part<'a, GpuSpec>>,
    case: Option<Part<'a, CaseSpec>>,
    psu: Option<Part<'a, PsuSpec>>,
    cooler: Option<Part<'a, CoolerSpec>>,
}

/// A build has these categories before it can boot (GPU and cooler are optional: the CPU may
/// have integrated graphics and a stock cooler; absence rules cover that).
pub const REQUIRED_CATEGORIES: [Category; 6] = [
    Category::Cpu,
    Category::Motherboard,
    Category::Memory,
    Category::Storage,
    Category::Case,
    Category::Psu,
];

macro_rules! typed_accessors {
    ($($name:ident: $spec:ty),+ $(,)?) => {$(
        #[doc = concat!("The `", stringify!($name), "` part, if chosen.")]
        pub fn $name(&self) -> Option<Part<'a, $spec>> {
            self.$name
        }
    )+};
}

impl<'a> Build<'a> {
    /// An empty build.
    pub fn new() -> Self {
        Self::default()
    }

    /// A build from products, each placed by its category. Memory kits and drives accumulate;
    /// a second product for a single slot is an error.
    pub fn from_products(
        products: impl IntoIterator<Item = &'a Product>,
    ) -> Result<Self, BuildError> {
        let mut build = Self::new();
        for product in products {
            if !Slot::is_multi(product.category)
                && let Some(existing) = build.get(product.category.into())
            {
                return Err(BuildError::SlotTaken {
                    category: product.category,
                    existing: existing.id.clone(),
                    added: product.id.clone(),
                });
            }
            build.add(product);
        }
        Ok(build)
    }

    /// A build from product IDs looked up in `catalog` (see [`Build::from_products`]).
    pub fn from_ids<I>(catalog: &'a Catalog, ids: I) -> Result<Self, BuildError>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let products = ids
            .into_iter()
            .map(|id| {
                catalog
                    .get(id.as_ref())
                    .ok_or_else(|| BuildError::UnknownProduct(ProductId::new(id.as_ref())))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_products(products)
    }

    /// Puts `product` into its category's slot: replaces the part in a single slot (returning
    /// it) or appends another memory kit or drive.
    pub fn add(&mut self, product: &'a Product) -> Option<&'a Product> {
        let index = match product.category {
            Category::Memory => self.memory.len(),
            Category::Storage => self.storage.len(),
            _ => 0,
        };
        self.place(Slot::new(product.category, index), product)
    }

    /// Puts `product` into `slot`, returning the part it replaced. For memory and storage,
    /// `slot.index == len` appends.
    pub fn set(
        &mut self,
        slot: Slot,
        product: &'a Product,
    ) -> Result<Option<&'a Product>, BuildError> {
        if product.category != slot.category {
            return Err(BuildError::WrongCategory {
                slot,
                product: product.id.clone(),
                found: product.category,
            });
        }
        let max_index = match slot.category {
            Category::Memory => self.memory.len(),
            Category::Storage => self.storage.len(),
            _ => 0,
        };
        if slot.index > max_index {
            return Err(BuildError::NoSuchSlot { slot });
        }
        Ok(self.place(slot, product))
    }

    /// Category and index already validated.
    fn place(&mut self, slot: Slot, product: &'a Product) -> Option<&'a Product> {
        fn put<'a, S>(
            target: &mut Option<Part<'a, S>>,
            product: &'a Product,
            spec: &'a S,
        ) -> Option<&'a Product> {
            target.replace(Part { product, spec }).map(|p| p.product)
        }
        fn put_at<'a, S>(
            list: &mut Vec<Part<'a, S>>,
            index: usize,
            product: &'a Product,
            spec: &'a S,
        ) -> Option<&'a Product> {
            let part = Part { product, spec };
            match list.get_mut(index) {
                Some(slot) => Some(std::mem::replace(slot, part).product),
                None => {
                    list.push(part);
                    None
                }
            }
        }

        match &product.spec {
            ProductSpec::Cpu(s) => put(&mut self.cpu, product, s),
            ProductSpec::Motherboard(s) => put(&mut self.motherboard, product, s),
            ProductSpec::Memory(s) => put_at(&mut self.memory, slot.index, product, s),
            ProductSpec::Storage(s) => put_at(&mut self.storage, slot.index, product, s),
            ProductSpec::Gpu(s) => put(&mut self.gpu, product, s),
            ProductSpec::Case(s) => put(&mut self.case, product, s),
            ProductSpec::Psu(s) => put(&mut self.psu, product, s),
            ProductSpec::Cooler(s) => put(&mut self.cooler, product, s),
        }
    }

    /// Removes and returns the part in `slot`. Later memory kits or drives move up one index.
    pub fn remove(&mut self, slot: Slot) -> Option<&'a Product> {
        fn take<'a, S>(part: &mut Option<Part<'a, S>>, index: usize) -> Option<&'a Product> {
            if index == 0 {
                part.take().map(|p| p.product)
            } else {
                None
            }
        }
        fn take_at<'a, S>(list: &mut Vec<Part<'a, S>>, index: usize) -> Option<&'a Product> {
            (index < list.len()).then(|| list.remove(index).product)
        }
        let i = slot.index;
        match slot.category {
            Category::Cpu => take(&mut self.cpu, i),
            Category::Motherboard => take(&mut self.motherboard, i),
            Category::Memory => take_at(&mut self.memory, i),
            Category::Storage => take_at(&mut self.storage, i),
            Category::Gpu => take(&mut self.gpu, i),
            Category::Case => take(&mut self.case, i),
            Category::Psu => take(&mut self.psu, i),
            Category::Cooler => take(&mut self.cooler, i),
        }
    }

    /// The part in `slot`.
    pub fn get(&self, slot: Slot) -> Option<&'a Product> {
        let single = |product: Option<&'a Product>| product.filter(|_| slot.index == 0);
        match slot.category {
            Category::Cpu => single(self.cpu.map(|p| p.product)),
            Category::Motherboard => single(self.motherboard.map(|p| p.product)),
            Category::Memory => self.memory.get(slot.index).map(|p| p.product),
            Category::Storage => self.storage.get(slot.index).map(|p| p.product),
            Category::Gpu => single(self.gpu.map(|p| p.product)),
            Category::Case => single(self.case.map(|p| p.product)),
            Category::Psu => single(self.psu.map(|p| p.product)),
            Category::Cooler => single(self.cooler.map(|p| p.product)),
        }
    }

    /// Every part with its slot, in [`Category::ALL`] order.
    pub fn parts(&self) -> Vec<(Slot, &'a Product)> {
        let mut parts = Vec::with_capacity(6 + self.memory.len() + self.storage.len());
        for &category in Category::ALL {
            let count = match category {
                Category::Memory => self.memory.len(),
                Category::Storage => self.storage.len(),
                _ => 1,
            };
            for index in 0..count {
                let slot = Slot::new(category, index);
                if let Some(product) = self.get(slot) {
                    parts.push((slot, product));
                }
            }
        }
        parts
    }

    /// Product IDs in slot order; [`Build::from_ids`] rebuilds the same build from them.
    pub fn ids(&self) -> Vec<ProductId> {
        self.parts()
            .into_iter()
            .map(|(_, p)| p.id.clone())
            .collect()
    }

    /// The first slot holding `id`.
    pub fn slot_of(&self, id: &ProductId) -> Option<Slot> {
        self.parts()
            .into_iter()
            .find_map(|(slot, p)| (&p.id == id).then_some(slot))
    }

    /// Number of parts.
    pub fn len(&self) -> usize {
        self.parts().len()
    }

    /// No parts at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Sum of part prices.
    pub fn total_price(&self) -> Money {
        self.parts().into_iter().map(|(_, p)| p.price).sum()
    }

    /// [`REQUIRED_CATEGORIES`] that have no part yet.
    pub fn missing_required(&self) -> Vec<Category> {
        REQUIRED_CATEGORIES
            .into_iter()
            .filter(|&c| self.get(c.into()).is_none())
            .collect()
    }

    typed_accessors! {
        cpu: CpuSpec,
        motherboard: MotherboardSpec,
        gpu: GpuSpec,
        case: CaseSpec,
        psu: PsuSpec,
        cooler: CoolerSpec,
    }

    /// Memory kits, in slot order.
    pub fn memory(&self) -> &[Part<'a, MemorySpec>] {
        &self.memory
    }

    /// Drives, in slot order.
    pub fn storage(&self) -> &[Part<'a, StorageSpec>] {
        &self.storage
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::*;

    #[test]
    fn places_parts_by_category() {
        let parts = default_parts();
        let kit2 = memory_product("ram2", |_| {});
        let mut build = Build::from_products(&parts).unwrap();
        assert_eq!(build.add(&kit2), None);
        assert_eq!(build.len(), 9);
        assert_eq!(build.memory().len(), 2);
        assert_eq!(
            build.get(Slot::new(Category::Memory, 1)).unwrap().id,
            kit2.id
        );
        assert_eq!(
            build.slot_of(&kit2.id),
            Some(Slot::new(Category::Memory, 1))
        );
        assert_eq!(build.total_price(), Money::from_euros(900));

        // Replacing a single slot returns the old part.
        let cpu2 = cpu_product("cpu2", |_| {});
        assert_eq!(build.add(&cpu2).unwrap().id.as_str(), "cpu");
        assert_eq!(build.cpu().unwrap().id(), &cpu2.id);
    }

    #[test]
    fn set_validates_category_and_index() {
        let kit = memory_product("ram", |_| {});
        let cpu = cpu_product("cpu", |_| {});
        let mut build = Build::new();
        assert!(matches!(
            build.set(Category::Cpu.into(), &kit),
            Err(BuildError::WrongCategory { .. })
        ));
        assert!(matches!(
            build.set(Slot::new(Category::Memory, 1), &kit),
            Err(BuildError::NoSuchSlot { .. })
        ));
        assert!(matches!(
            build.set(Slot::new(Category::Cpu, 1), &cpu),
            Err(BuildError::NoSuchSlot { .. })
        ));
        assert_eq!(build.set(Slot::new(Category::Memory, 0), &kit), Ok(None));
        assert_eq!(
            build
                .set(Slot::new(Category::Memory, 0), &kit)
                .unwrap()
                .unwrap()
                .id,
            kit.id
        );
    }

    #[test]
    fn remove_shifts_multi_slots() {
        let (a, b) = (storage_product("a", |_| {}), storage_product("b", |_| {}));
        let cpu = cpu_product("cpu", |_| {});
        let mut build = Build::from_products([&a, &b, &cpu]).unwrap();
        assert_eq!(build.remove(Slot::new(Category::Cpu, 1)), None);
        assert_eq!(build.remove(Category::Cpu.into()).unwrap().id, cpu.id);
        assert_eq!(
            build.remove(Slot::new(Category::Storage, 0)).unwrap().id,
            a.id
        );
        assert_eq!(build.get(Slot::new(Category::Storage, 0)).unwrap().id, b.id);
        assert_eq!(build.ids(), std::slice::from_ref(&b.id));
    }

    #[test]
    fn rejects_two_parts_for_a_single_slot() {
        let (a, b) = (gpu_product("a", |_| {}), gpu_product("b", |_| {}));
        let err = Build::from_products([&a, &b]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "the gpu slot already holds `a`, cannot add `b`"
        );
    }

    #[test]
    fn ids_round_trip_through_the_catalog() {
        let catalog = Catalog::global();
        let ids = [
            "kestrel-b650m-wing",
            "nimbus-ddr5-6000-32",
            "nimbus-ddr5-6000-32",
        ];
        let build = Build::from_ids(catalog, ids).unwrap();
        let again = Build::from_ids(catalog, build.ids()).unwrap();
        assert_eq!(again.ids(), build.ids());
        assert_eq!(
            build.missing_required(),
            [
                Category::Cpu,
                Category::Storage,
                Category::Case,
                Category::Psu
            ]
        );
        assert!(matches!(
            Build::from_ids(catalog, ["nope"]),
            Err(BuildError::UnknownProduct(id)) if id.as_str() == "nope"
        ));
    }
}
