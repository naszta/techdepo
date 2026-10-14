//! Price-tier and platform coverage: "suggest an alternative in the same price range" must
//! always have an answer.
//!
//! Every category needs at least [`MIN_PER_TIER`] products in each tier. Categories bound to
//! a platform (CPU and motherboard by socket, memory by DDR generation, coolers by every
//! socket they mount on) also need at least [`MIN_PER_PLATFORM`] products on every platform,
//! and every such product needs another product on its platform in the same or an adjacent
//! tier. Two per tier *per platform* would take ~24 CPUs and ~24 boards, well past the plan's
//! 10–12 products per category, so the platform check is per platform, not per cell.

use std::fmt;

use techdepo_catalog::{Catalog, Category, MemoryType, Product, ProductSpec, Socket, Tier};

/// Products each category needs in every tier.
pub const MIN_PER_TIER: usize = 2;
/// Products each platform-bound category needs on every platform.
pub const MIN_PER_PLATFORM: usize = 2;

/// What a product must be compatible with to replace another one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Platform {
    /// Not platform-bound (storage, GPUs, cases, PSUs).
    Any,
    /// A CPU socket.
    Socket(Socket),
    /// A DDR generation.
    Memory(MemoryType),
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Any => f.write_str("any"),
            Self::Socket(socket) => f.write_str(socket.as_str()),
            Self::Memory(memory) => f.write_str(memory.as_str()),
        }
    }
}

/// Every platform a category is split by (`[Any]` when it is not platform-bound).
pub fn category_platforms(category: Category) -> Vec<Platform> {
    match category {
        Category::Cpu | Category::Motherboard | Category::Cooler => {
            Socket::ALL.iter().copied().map(Platform::Socket).collect()
        }
        Category::Memory => MemoryType::ALL
            .iter()
            .copied()
            .map(Platform::Memory)
            .collect(),
        Category::Storage | Category::Gpu | Category::Case | Category::Psu => vec![Platform::Any],
    }
}

/// The platforms a product belongs to.
pub fn product_platforms(product: &Product) -> Vec<Platform> {
    match &product.spec {
        ProductSpec::Cpu(s) => vec![Platform::Socket(s.socket)],
        ProductSpec::Motherboard(s) => vec![Platform::Socket(s.socket)],
        ProductSpec::Memory(s) => vec![Platform::Memory(s.memory_type)],
        ProductSpec::Cooler(s) => s.sockets.iter().copied().map(Platform::Socket).collect(),
        _ => vec![Platform::Any],
    }
}

/// Product counts for one category on one platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The category.
    pub category: Category,
    /// The platform (`Any` for categories that are not platform-bound).
    pub platform: Platform,
    /// Products per tier, in [`Tier::ALL`] order.
    pub per_tier: [usize; 3],
}

impl Cell {
    /// Products on this platform.
    pub fn total(&self) -> usize {
        self.per_tier.iter().sum()
    }
}

/// The category × platform × tier matrix plus every coverage problem.
#[derive(Debug, Clone, Default)]
pub struct TierCoverage {
    /// One cell per category and platform, in category order.
    pub cells: Vec<Cell>,
    /// Products per category and tier.
    pub per_category: Vec<(Category, [usize; 3])>,
    /// Human-readable problems; empty when coverage holds.
    pub problems: Vec<String>,
}

fn tier_index(tier: Tier) -> usize {
    Tier::ALL.iter().position(|t| *t == tier).unwrap_or(0)
}

/// Computes tier and platform coverage.
pub fn tier_coverage(catalog: &Catalog) -> TierCoverage {
    let mut coverage = TierCoverage::default();
    for &category in Category::ALL {
        let products = catalog.by_category(category);

        let mut per_tier = [0; 3];
        for product in products {
            per_tier[tier_index(product.tier)] += 1;
        }
        for (tier, &count) in Tier::ALL.iter().zip(&per_tier) {
            if count < MIN_PER_TIER {
                coverage.problems.push(format!(
                    "{} has {count} {} product(s), needs at least {MIN_PER_TIER}",
                    category.as_str(),
                    tier.as_str()
                ));
            }
        }
        coverage.per_category.push((category, per_tier));

        let platforms = category_platforms(category);
        for &platform in &platforms {
            let on_platform: Vec<&Product> = products
                .iter()
                .filter(|p| product_platforms(p).contains(&platform))
                .collect();
            let mut per_tier = [0; 3];
            for product in &on_platform {
                per_tier[tier_index(product.tier)] += 1;
            }
            coverage.cells.push(Cell {
                category,
                platform,
                per_tier,
            });
            if platform == Platform::Any {
                continue;
            }
            if on_platform.len() < MIN_PER_PLATFORM {
                coverage.problems.push(format!(
                    "{} has {} product(s) on {platform}, needs at least {MIN_PER_PLATFORM}",
                    category.as_str(),
                    on_platform.len()
                ));
            }
            for product in &on_platform {
                let tier = tier_index(product.tier);
                let has_alternative = on_platform.iter().any(|other| {
                    other.id != product.id && tier_index(other.tier).abs_diff(tier) <= 1
                });
                if !has_alternative {
                    coverage.problems.push(format!(
                        "{} has no alternative on {platform} in the same or an adjacent tier",
                        product.id
                    ));
                }
            }
        }
    }
    coverage
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_has_tier_coverage() {
        let coverage = tier_coverage(Catalog::global());
        assert!(coverage.problems.is_empty(), "{:#?}", coverage.problems);
        let cpu_am5 = coverage
            .cells
            .iter()
            .find(|c| c.category == Category::Cpu && c.platform == Platform::Socket(Socket::Am5))
            .unwrap();
        assert!(cpu_am5.total() >= MIN_PER_PLATFORM);
    }
}
