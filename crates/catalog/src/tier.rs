//! How a product's price tier is decided (`data/catalog/README.md`, "Tiers").
//!
//! Every category except storage uses fixed price bands, so the tier written in the YAML can be
//! checked. Storage tiers follow the interface class instead, because a large SATA drive can cost
//! more than a small fast one.

use techdepo_domain::Money;

use crate::category::{Category, Tier};
use crate::product::Product;
use crate::spec::{ProductSpec, StorageInterface};

/// The price bands of one category: budget below `mid_from`, high from `high_from`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceBands {
    /// Lowest mid-tier price.
    pub mid_from: Money,
    /// Lowest high-tier price.
    pub high_from: Money,
}

impl PriceBands {
    const fn euros(mid_from: i64, high_from: i64) -> Self {
        Self {
            mid_from: Money::from_euros(mid_from),
            high_from: Money::from_euros(high_from),
        }
    }

    /// The tier a price falls in.
    pub fn tier(&self, price: Money) -> Tier {
        if price >= self.high_from {
            Tier::High
        } else if price >= self.mid_from {
            Tier::Mid
        } else {
            Tier::Budget
        }
    }
}

impl Category {
    /// The price bands that decide the tier, or `None` for storage (tiered by interface).
    pub const fn price_bands(self) -> Option<PriceBands> {
        Some(match self {
            Self::Cpu => PriceBands::euros(200, 400),
            Self::Motherboard => PriceBands::euros(120, 250),
            Self::Memory => PriceBands::euros(90, 180),
            Self::Gpu => PriceBands::euros(400, 800),
            Self::Case => PriceBands::euros(80, 150),
            Self::Psu => PriceBands::euros(80, 140),
            Self::Cooler => PriceBands::euros(50, 120),
            Self::Storage => return None,
        })
    }
}

impl Product {
    /// The tier the catalog's tier rules assign to this product: its price band, or for storage
    /// its interface class (SATA and PCIe 3.0 budget, 4.0 mid, 5.0 high).
    pub fn expected_tier(&self) -> Tier {
        match (&self.spec, self.category.price_bands()) {
            (ProductSpec::Storage(spec), _) => match spec.interface {
                StorageInterface::Sata => Tier::Budget,
                StorageInterface::Nvme { pcie_gen } => match pcie_gen.number() {
                    0..=3 => Tier::Budget,
                    4 => Tier::Mid,
                    _ => Tier::High,
                },
            },
            (_, Some(bands)) => bands.tier(self.price),
            // Only storage has no bands, and its spec is handled above.
            (_, None) => self.tier,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_edges() {
        let bands = Category::Cpu.price_bands().unwrap();
        assert_eq!(bands.tier(Money::from_cents(19_999)), Tier::Budget);
        assert_eq!(bands.tier(Money::from_euros(200)), Tier::Mid);
        assert_eq!(bands.tier(Money::from_cents(39_999)), Tier::Mid);
        assert_eq!(bands.tier(Money::from_euros(400)), Tier::High);
        assert!(Category::Storage.price_bands().is_none());
    }

    #[test]
    fn storage_tier_follows_interface() {
        let catalog = crate::Catalog::global();
        let tier = |id: &str| catalog.get(id).unwrap().expected_tier();
        assert_eq!(tier("lodestar-vault-4tb"), Tier::Budget);
        assert_eq!(tier("arcwave-a4-1tb"), Tier::Mid);
        assert_eq!(tier("arcwave-a5-2tb"), Tier::High);
    }
}
