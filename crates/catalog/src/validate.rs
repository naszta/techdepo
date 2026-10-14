//! Catalog consistency and plausibility checks (`Catalog::validate`, `xtask catalog validate`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use techdepo_domain::ProductId;

use crate::catalog::Catalog;
use crate::category::{Category, Tier};
use crate::content::{ProductContent, REQUIRED_SECTIONS};
use crate::mentions::MentionTarget;
use crate::platform::{FormFactor, PsuFormFactor};
use crate::product::Product;
use crate::spec::{
    CaseSpec, CoolerKind, CoolerSpec, CpuSpec, GpuSpec, M2_LENGTHS_MM, MemorySpec, MotherboardSpec,
    ProductSpec, PsuSpec, RADIATOR_SIZES_MM, StorageFormFactor, StorageInterface, StorageSpec,
};

/// One problem found by [`Catalog::validate`].
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    /// The product concerned, if the problem is about one product.
    pub product: Option<ProductId>,
    /// What is wrong.
    pub kind: ValidationErrorKind,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.product {
            Some(id) => write!(f, "{id}: {}", self.kind),
            None => self.kind.fmt(f),
        }
    }
}

impl std::error::Error for ValidationError {}

/// What a [`ValidationError`] is about.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ValidationErrorKind {
    /// Another product has the same ID.
    #[error("duplicate product ID")]
    DuplicateId,
    /// Another product has the same SKU.
    #[error("duplicate SKU `{0}`")]
    DuplicateSku(String),
    /// Another product has the same name.
    #[error("duplicate name `{0}`")]
    DuplicateName(String),
    /// A string field does not have the required shape.
    #[error("{field} `{value}` is invalid: {expected}")]
    InvalidFormat {
        /// Field name.
        field: &'static str,
        /// Rejected value.
        value: String,
        /// Description of the accepted format.
        expected: &'static str,
    },
    /// A number is outside its plausible range.
    #[error("{field} = {value} is outside {min}..={max}")]
    OutOfRange {
        /// Field name.
        field: &'static str,
        /// Rejected value.
        value: f64,
        /// Lowest plausible value.
        min: f64,
        /// Highest plausible value.
        max: f64,
    },
    /// Fields contradict each other or a standard.
    #[error("{0}")]
    Inconsistent(String),
    /// The tier does not match the category's tier rule (price band or storage interface).
    #[error("tier is `{}` but its {basis} puts it in `{}`", .actual.as_str(), .expected.as_str())]
    WrongTier {
        /// The tier written in the YAML.
        actual: Tier,
        /// The tier the rule assigns.
        expected: Tier,
        /// What decided it: `price` or `interface`.
        basis: &'static str,
    },
    /// The category has no products.
    #[error("category `{}` has no products", .0.as_str())]
    EmptyCategory(Category),
    /// No content file for the product.
    #[error("missing content file data/catalog/content/<id>.md")]
    MissingContent,
    /// Content sections are missing, extra, empty or out of order.
    #[error("content: {0}")]
    InvalidContent(String),
    /// A content file has no matching product.
    #[error("content file has no matching product")]
    OrphanContent,
}

/// Every problem found by [`Catalog::validate`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub struct ValidationErrors(pub Vec<ValidationError>);

impl fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} catalog problem(s):", self.0.len())?;
        for error in &self.0 {
            writeln!(f, "  - {error}")?;
        }
        Ok(())
    }
}

impl Catalog {
    /// Checks uniqueness, formats, plausibility ranges, cross-field consistency and content.
    pub fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = Vec::new();

        for category in Category::ALL {
            if self.by_category(*category).is_empty() {
                errors.push(ValidationError {
                    product: None,
                    kind: ValidationErrorKind::EmptyCategory(*category),
                });
            }
        }

        let mut ids = BTreeSet::new();
        let mut skus = BTreeSet::new();
        let mut names = BTreeSet::new();
        for product in self.all() {
            let mut check = Checker {
                product,
                errors: &mut errors,
            };
            if !ids.insert(product.id.as_str()) {
                check.push(ValidationErrorKind::DuplicateId);
            }
            if !skus.insert(product.sku.as_str()) {
                check.push(ValidationErrorKind::DuplicateSku(product.sku.to_string()));
            }
            if !names.insert(product.name.to_lowercase()) {
                check.push(ValidationErrorKind::DuplicateName(product.name.clone()));
            }
            check.product_fields();
            check.spec();
            match self.content(&product.id) {
                Some(content) => {
                    check.content(content);
                    for problem in self.ambiguous_mentions(content.markdown()) {
                        check.push(ValidationErrorKind::InvalidContent(problem));
                    }
                }
                None => check.push(ValidationErrorKind::MissingContent),
            }
        }

        let products: BTreeMap<&str, ()> = self.all().iter().map(|p| (p.id.as_str(), ())).collect();
        for (id, _) in self.contents() {
            if !products.contains_key(id.as_str()) {
                errors.push(ValidationError {
                    product: Some(id.clone()),
                    kind: ValidationErrorKind::OrphanContent,
                });
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationErrors(errors))
        }
    }
}

impl Catalog {
    /// One message per product mention in `text` that matches several products ("Halcyon HX
    /// 7070" for the Twin and the SFF). Agents quote content and knowledge-base text verbatim,
    /// and their grounding check accepts only names that resolve to one product.
    pub fn ambiguous_mentions(&self, text: &str) -> Vec<String> {
        self.product_mentions(text)
            .into_iter()
            .filter_map(|m| match m.target {
                MentionTarget::Ambiguous(products) => {
                    let names: Vec<&str> = products.iter().map(|p| p.name.as_str()).collect();
                    Some(format!(
                        "`{}` could be any of: {}; use the full product name",
                        m.text,
                        names.join(", ")
                    ))
                }
                MentionTarget::Product(_) => None,
            })
            .collect()
    }
}

struct Checker<'a> {
    product: &'a Product,
    errors: &'a mut Vec<ValidationError>,
}

impl Checker<'_> {
    fn push(&mut self, kind: ValidationErrorKind) {
        self.errors.push(ValidationError {
            product: Some(self.product.id.clone()),
            kind,
        });
    }

    fn range(
        &mut self,
        field: &'static str,
        value: impl Into<f64>,
        min: impl Into<f64>,
        max: impl Into<f64>,
    ) {
        let (value, min, max) = (value.into(), min.into(), max.into());
        if !(min..=max).contains(&value) {
            self.push(ValidationErrorKind::OutOfRange {
                field,
                value,
                min,
                max,
            });
        }
    }

    fn ensure(&mut self, ok: bool, message: impl FnOnce() -> String) {
        if !ok {
            self.push(ValidationErrorKind::Inconsistent(message()));
        }
    }

    fn format(&mut self, field: &'static str, value: &str, ok: bool, expected: &'static str) {
        if !ok {
            self.push(ValidationErrorKind::InvalidFormat {
                field,
                value: value.to_owned(),
                expected,
            });
        }
    }

    fn unique<T: Ord + fmt::Debug>(
        &mut self,
        field: &'static str,
        values: impl IntoIterator<Item = T>,
    ) {
        let mut seen = BTreeSet::new();
        for value in values {
            if let Some(dup) = seen.replace(value) {
                self.push(ValidationErrorKind::Inconsistent(format!(
                    "{field} lists {dup:?} twice"
                )));
            }
        }
    }

    fn product_fields(&mut self) {
        let p = self.product;
        self.format(
            "id",
            p.id.as_str(),
            is_slug(p.id.as_str(), false),
            "lower-case words of [a-z0-9] joined by `-`",
        );
        self.format(
            "sku",
            p.sku.as_str(),
            is_slug(p.sku.as_str(), true),
            "upper-case words of [A-Z0-9] joined by `-`",
        );
        self.format("brand", &p.brand, !p.brand.trim().is_empty(), "non-empty");
        self.format(
            "name",
            &p.name,
            p.name.starts_with(&p.brand),
            "the full name, starting with the brand",
        );
        self.format(
            "summary",
            &p.summary,
            (20..=200).contains(&p.summary.chars().count()),
            "20 to 200 characters",
        );
        self.range("price_cents", p.price.cents() as f64, 100, 1_000_000);
        self.range("default_stock", p.default_stock, 0, 500);
        for tag in &p.tags {
            self.format(
                "tag",
                tag,
                is_slug(tag, false),
                "lower-case words of [a-z0-9] joined by `-`",
            );
        }
        self.unique("tags", p.tags.iter());
        let expected = p.expected_tier();
        if p.tier != expected {
            self.push(ValidationErrorKind::WrongTier {
                actual: p.tier,
                expected,
                basis: if p.category.price_bands().is_some() {
                    "price"
                } else {
                    "interface"
                },
            });
        }
        self.ensure(p.category == p.spec.category(), || {
            format!(
                "category {} does not match its {} spec",
                p.category.as_str(),
                p.spec.category().as_str()
            )
        });
    }

    fn spec(&mut self) {
        match &self.product.spec {
            ProductSpec::Cpu(s) => self.cpu(s),
            ProductSpec::Motherboard(s) => self.motherboard(s),
            ProductSpec::Memory(s) => self.memory(s),
            ProductSpec::Storage(s) => self.storage(s),
            ProductSpec::Gpu(s) => self.gpu(s),
            ProductSpec::Case(s) => self.case(s),
            ProductSpec::Psu(s) => self.psu(s),
            ProductSpec::Cooler(s) => self.cooler(s),
        }
    }

    fn cpu(&mut self, s: &CpuSpec) {
        self.ensure(s.family.socket() == s.socket, || {
            format!(
                "family {} belongs to {}, not {}",
                s.family.as_str(),
                s.family.socket(),
                s.socket
            )
        });
        self.range("cores", s.cores, 2, 32);
        self.range("threads", s.threads, s.cores, u16::from(s.cores) * 2);
        self.range("base_clock_mhz", s.base_clock_mhz, 1000, 6000);
        self.range("boost_clock_mhz", s.boost_clock_mhz, s.base_clock_mhz, 6500);
        self.range("tdp_w", s.tdp_w, 15, 300);
        self.range("peak_power_w", s.peak_power_w, s.tdp_w, 400);
        self.ensure(!s.memory.is_empty(), || {
            "memory lists no memory type".into()
        });
        self.unique("memory", s.memory.iter().map(|m| m.memory_type));
        for m in &s.memory {
            self.ensure(s.socket.memory_types().contains(&m.memory_type), || {
                format!("{} platforms do not run {}", s.socket, m.memory_type)
            });
            let (min, max) = m.memory_type.speed_range_mts();
            self.range("memory.max_speed_mts", m.max_speed_mts, min, max);
            // JEDEC's slowest DDR4 grade up to the one-module-per-channel speed.
            self.range(
                "memory.max_speed_2dpc_mts",
                m.max_speed_2dpc_mts,
                2133,
                m.max_speed_mts,
            );
        }
    }

    fn motherboard(&mut self, s: &MotherboardSpec) {
        self.ensure(s.chipset.socket() == s.socket, || {
            format!(
                "chipset {} is for {}, not {}",
                s.chipset,
                s.chipset.socket(),
                s.socket
            )
        });
        self.ensure(s.socket.memory_types().contains(&s.memory_type), || {
            format!("{} platforms do not run {}", s.socket, s.memory_type)
        });
        let slots = if s.form_factor == FormFactor::MiniItx {
            (2, 2)
        } else {
            (2, 8)
        };
        self.range("memory_slots", s.memory_slots, slots.0, slots.1);
        self.range("max_memory_gb", s.max_memory_gb, 32, 512);
        let (min, max) = s.memory_type.speed_range_mts();
        self.range("max_memory_speed_mts", s.max_memory_speed_mts, min, max);
        self.range("m2_slots (count)", s.m2_slots.len() as f64, 0, 6);
        for slot in &s.m2_slots {
            self.ensure(M2_LENGTHS_MM.contains(&slot.max_length_mm), || {
                format!(
                    "M.2 max_length_mm {} is not one of {M2_LENGTHS_MM:?}",
                    slot.max_length_mm
                )
            });
        }
        self.range("sata_ports", s.sata_ports, 0, 8);
        self.range("bios_version", s.bios_version, 1, 999);
        self.ensure(!s.cpu_support.is_empty(), || {
            "cpu_support lists no CPU family".into()
        });
        self.unique("cpu_support", s.cpu_support.iter().map(|c| c.family));
        for support in &s.cpu_support {
            self.ensure(support.family.socket() == s.socket, || {
                format!(
                    "cpu_support lists {} which is not an {} family",
                    support.family.as_str(),
                    s.socket
                )
            });
            self.range("cpu_support.min_bios", support.min_bios, 1, 999);
        }
        self.range("eps_8pin", s.eps_8pin, 1, 2);
        self.range("eps_4pin", s.eps_4pin, 0, 1);
    }

    fn memory(&mut self, s: &MemorySpec) {
        self.range("modules", s.modules, 1, 4);
        self.ensure([8, 16, 24, 32, 48, 64].contains(&s.module_gb), || {
            format!("module_gb {} is not a standard module size", s.module_gb)
        });
        let (min, max) = s.memory_type.speed_range_mts();
        self.range("speed_mts", s.speed_mts, min, max);
        self.range("cas_latency", s.cas_latency, 10, 50);
        self.range("height_mm", s.height_mm, 30, 70);
    }

    fn storage(&mut self, s: &StorageSpec) {
        self.ensure(
            s.form_factor == StorageFormFactor::M2_2280 || s.interface == StorageInterface::Sata,
            || format!("{} drives are SATA, not NVMe", s.form_factor),
        );
        // Interface ceilings: SATA 6 Gb/s, then PCIe x4 per generation.
        let max_read = match s.interface {
            StorageInterface::Sata => 600,
            StorageInterface::Nvme { pcie_gen } => match pcie_gen.number() {
                3 => 3_600,
                4 => 7_500,
                _ => 14_900,
            },
        };
        self.range("capacity_gb", s.capacity_gb, 120, 24_000);
        self.range("read_mbps", s.read_mbps, 80, max_read);
        self.range("write_mbps", s.write_mbps, 80, max_read);
    }

    fn gpu(&mut self, s: &GpuSpec) {
        self.range("length_mm", s.length_mm, 150, 360);
        self.range("slot_width", s.slot_width, 1.0, 4.0);
        self.ensure((s.slot_width * 2.0).fract() == 0.0, || {
            format!("slot_width {} is not a multiple of 0.5", s.slot_width)
        });
        self.range("board_power_w", s.board_power_w, 30, 600);
        let c = s.power_connectors;
        let supplied = 75 + 150 * u32::from(c.pcie_8pin) + 600 * u32::from(c.twelve_v_2x6);
        self.ensure(supplied >= u32::from(s.board_power_w), || {
            format!(
                "power_connectors deliver at most {supplied} W for a {} W card",
                s.board_power_w
            )
        });
        self.range("power_connectors.pcie_8pin", c.pcie_8pin, 0, 4);
        self.range("power_connectors.twelve_v_2x6", c.twelve_v_2x6, 0, 1);
        self.range("vram_gb", s.vram_gb, 4, 32);
        self.range(
            "recommended_psu_w",
            s.recommended_psu_w,
            s.board_power_w.max(300),
            1200,
        );
    }

    fn case(&mut self, s: &CaseSpec) {
        self.ensure(!s.motherboard_form_factors.is_empty(), || {
            "motherboard_form_factors is empty".into()
        });
        self.unique(
            "motherboard_form_factors",
            s.motherboard_form_factors.iter(),
        );
        self.range("max_gpu_length_mm", s.max_gpu_length_mm, 150, 450);
        self.range("max_gpu_slots", s.max_gpu_slots, 1, 10);
        self.range("max_cooler_height_mm", s.max_cooler_height_mm, 30, 190);
        self.ensure(!s.psu_form_factors.is_empty(), || {
            "psu_form_factors is empty".into()
        });
        self.unique("psu_form_factors", s.psu_form_factors.iter());
        self.range("max_psu_length_mm", s.max_psu_length_mm, 100, 250);
        self.unique(
            "radiator_mounts",
            s.radiator_mounts.iter().map(|m| m.position),
        );
        for mount in &s.radiator_mounts {
            self.ensure(!mount.sizes_mm.is_empty(), || {
                format!("radiator mount {} lists no sizes", mount.position)
            });
            for size in &mount.sizes_mm {
                self.ensure(RADIATOR_SIZES_MM.contains(size), || {
                    format!("radiator size {size} is not one of {RADIATOR_SIZES_MM:?}")
                });
            }
        }
        self.range("bays_2_5", s.bays_2_5, 0, 10);
        self.range("bays_3_5", s.bays_3_5, 0, 10);
        self.range("included_fans", s.included_fans, 0, 10);
    }

    fn psu(&mut self, s: &PsuSpec) {
        self.range("wattage_w", s.wattage_w, 450, 1200);
        let (min, max) = match s.form_factor {
            PsuFormFactor::Atx => (120, 220),
            PsuFormFactor::Sfx => (100, 130),
        };
        self.range("length_mm", s.length_mm, min, max);
        let c = s.connectors;
        self.range("connectors.eps_8pin", c.eps_8pin, 1, 3);
        self.range("connectors.pcie_8pin", c.pcie_8pin, 0, 8);
        self.range("connectors.twelve_v_2x6", c.twelve_v_2x6, 0, 2);
        self.range("connectors.sata", c.sata, 2, 16);
    }

    fn cooler(&mut self, s: &CoolerSpec) {
        self.ensure(!s.sockets.is_empty(), || "sockets is empty".into());
        self.unique("sockets", s.sockets.iter());
        self.range("tdp_rating_w", s.tdp_rating_w, 65, 350);
        match s.kind {
            CoolerKind::Air {
                height_mm,
                ram_clearance_mm,
            } => {
                self.range("kind.height_mm", height_mm, 30, 190);
                if let Some(clearance) = ram_clearance_mm {
                    self.range("kind.ram_clearance_mm", clearance, 25, 70);
                }
            }
            CoolerKind::Aio { radiator_mm } => self
                .ensure(RADIATOR_SIZES_MM.contains(&radiator_mm), || {
                    format!("radiator_mm {radiator_mm} is not one of {RADIATOR_SIZES_MM:?}")
                }),
        }
    }

    fn content(&mut self, content: &ProductContent) {
        if !content.preamble().is_empty() {
            self.push(ValidationErrorKind::InvalidContent(
                "text before the first `##` section (do not add a `#` title)".into(),
            ));
        }
        let sections = content.sections();
        let headings: Vec<&str> = sections.iter().map(|s| s.heading).collect();
        if headings != REQUIRED_SECTIONS {
            self.push(ValidationErrorKind::InvalidContent(format!(
                "sections are {headings:?}, expected exactly {REQUIRED_SECTIONS:?}"
            )));
        }
        for section in &sections {
            if section.body.is_empty() {
                self.push(ValidationErrorKind::InvalidContent(format!(
                    "section `{}` is empty",
                    section.heading
                )));
            }
        }
        for (name, items) in [("Pros", content.pros()), ("Cons", content.cons())] {
            if items.is_empty() {
                self.push(ValidationErrorKind::InvalidContent(format!(
                    "`{name}` has no `- ` bullet items"
                )));
            }
        }
        let faq = content.faq();
        if faq.is_empty() {
            self.push(ValidationErrorKind::InvalidContent(
                "`FAQ` has no `### Question` entries".into(),
            ));
        }
        for entry in faq.iter().filter(|e| e.answer.is_empty()) {
            self.push(ValidationErrorKind::InvalidContent(format!(
                "FAQ `{}` has no answer",
                entry.question
            )));
        }
    }
}

/// Words of `[a-z0-9]` (or `[A-Z0-9]`) joined by single dashes.
fn is_slug(s: &str, upper: bool) -> bool {
    !s.is_empty()
        && s.split('-').all(|word| {
            !word.is_empty()
                && word.bytes().all(|b| {
                    b.is_ascii_digit()
                        || if upper {
                            b.is_ascii_uppercase()
                        } else {
                            b.is_ascii_lowercase()
                        }
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAD_COOLER: &str = r#"
- id: Bad_Cooler
  sku: COOL-BAD
  brand: Breeze
  name: Breeze Bad
  price_cents: 0
  default_stock: 5
  tier: budget
  tags: [quiet, quiet]
  summary: A cooler that is far too tall for anything.
  spec:
    sockets: [am5, am5]
    kind: { type: air, height_mm: 250 }
    tdp_rating_w: 180
"#;

    #[test]
    fn reports_problems() {
        let catalog = Catalog::from_sources(
            [(Category::Cooler, BAD_COOLER)],
            [("ghost", "## Overview\nBoo\n")],
        )
        .unwrap();
        let errors = catalog.validate().unwrap_err().0;
        let has =
            |pred: &dyn Fn(&ValidationErrorKind) -> bool| errors.iter().any(|e| pred(&e.kind));

        assert!(has(&|k| matches!(
            k,
            ValidationErrorKind::EmptyCategory(Category::Cpu)
        )));
        assert!(has(&|k| matches!(
            k,
            ValidationErrorKind::InvalidFormat { field: "id", .. }
        )));
        assert!(has(&|k| matches!(
            k,
            ValidationErrorKind::OutOfRange {
                field: "price_cents",
                ..
            }
        )));
        assert!(has(&|k| matches!(
            k,
            ValidationErrorKind::OutOfRange {
                field: "kind.height_mm",
                ..
            }
        )));
        assert!(has(
            &|k| matches!(k, ValidationErrorKind::Inconsistent(m) if m.contains("sockets"))
        ));
        assert!(has(
            &|k| matches!(k, ValidationErrorKind::Inconsistent(m) if m.contains("tags"))
        ));
        assert!(has(&|k| *k == ValidationErrorKind::MissingContent));
        assert!(
            errors
                .iter()
                .any(|e| e.kind == ValidationErrorKind::OrphanContent
                    && e.product.as_ref().is_some_and(|p| p.as_str() == "ghost"))
        );
        let report = ValidationErrors(errors).to_string();
        assert!(
            report.contains("Bad_Cooler: kind.height_mm = 250 is outside 30..=190"),
            "{report}"
        );
    }

    #[test]
    fn tier_must_match_the_price_band() {
        let yaml = BAD_COOLER
            .replace("price_cents: 0", "price_cents: 12990")
            .replace("Bad_Cooler", "bad-cooler");
        let catalog = Catalog::from_sources([(Category::Cooler, yaml.as_str())], []).unwrap();
        let errors = catalog.validate().unwrap_err().0;
        let tier = errors
            .iter()
            .find(|e| matches!(e.kind, ValidationErrorKind::WrongTier { .. }))
            .expect("tier error");
        assert_eq!(
            tier.to_string(),
            "bad-cooler: tier is `budget` but its price puts it in `high`"
        );
    }

    #[test]
    fn slugs() {
        assert!(is_slug("voltaris-v5-8600", false));
        assert!(!is_slug("Voltaris", false));
        assert!(!is_slug("a--b", false));
        assert!(!is_slug("-a", false));
        assert!(is_slug("CPU-VOL-8600", true));
        assert!(!is_slug("cpu-vol", true));
    }
}
