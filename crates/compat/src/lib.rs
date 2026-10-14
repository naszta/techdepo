//! TechDepó's compatibility engine: pure rules over typed catalog specs, no I/O.
//!
//! "Perfect compatibility" means a **closed world**: the catalog only models attributes the
//! rules check, every catalog pair is tested (see `tests/pairwise.rs`), and what is not
//! checked is listed in [`NOT_CHECKED`]. The same code runs in the SPA builder, in the API
//! worker at checkout, and behind the agent's compatibility tool.
//!
//! ```
//! use techdepo_catalog::Catalog;
//! use techdepo_compat::{Build, RuleId, Verdict, check_build};
//!
//! let catalog = Catalog::global();
//! let build = Build::from_ids(catalog, ["kestrel-b650m-wing", "nimbus-ddr4-3200-16"]).unwrap();
//! let report = check_build(&build);
//! assert_eq!(report.verdict, Verdict::Incompatible);
//! assert_eq!(report.issues[0].rule, RuleId::MemoryType);
//! ```

mod build;
mod not_checked;
pub mod power;
mod report;
mod rule;
mod rules;
mod suggest;

#[cfg(test)]
mod fixtures;

pub use build::{Build, BuildError, Part, REQUIRED_CATEGORIES, Slot};
pub use not_checked::{NOT_CHECKED, NotChecked};
pub use power::{PowerDraw, PowerEstimate};
pub use report::{Issue, Report, Verdict};
pub use rule::{RuleId, Severity};
pub use suggest::{Fix, MAX_FIX_CANDIDATES, alternatives, price_band_around, suggest_fixes};

use techdepo_catalog::{Catalog, Product};
use techdepo_domain::ProductId;

use crate::rules::Ctx;

/// Runs every rule against `build`.
pub fn check_build(build: &Build<'_>) -> Report {
    evaluate(build, |_| true)
}

/// Checks a build of just `a` and `b`, skipping the "missing part" rules
/// ([`RuleId::is_absence`]): "does this CPU work with this board?" should not warn that there
/// is no cooler.
///
/// Two products for the same single slot (two CPUs) never share a build; the report is then
/// empty with no checked rules. Two memory kits or two drives are checked together. The result
/// does not depend on the argument order.
pub fn check_pair(a: &Product, b: &Product) -> Report {
    // Memory kits and drives keep insertion order, which shows in messages and part lists.
    let pair = if b.id < a.id { [b, a] } else { [a, b] };
    let build = Build::from_products(pair).unwrap_or_default();
    evaluate(&build, |rule| !rule.is_absence())
}

/// Looks `ids` up in `catalog` and checks them as one build.
pub fn check_products(catalog: &Catalog, ids: &[ProductId]) -> Result<Report, BuildError> {
    Build::from_ids(catalog, ids).map(|build| check_build(&build))
}

fn evaluate(build: &Build<'_>, include: impl Fn(RuleId) -> bool) -> Report {
    let cx = Ctx::new(build);
    let mut issues = Vec::new();
    let mut checked_rules = Vec::new();
    for &rule in RuleId::ALL.iter().filter(|&&r| include(r)) {
        if rules::run(rule, &cx, &mut issues) {
            checked_rules.push(rule);
        }
    }
    // Stable: within a severity, issues stay in rule order.
    issues.sort_by_key(|i| std::cmp::Reverse(i.severity));

    let psu_watts = build.psu().map(|psu| u32::from(psu.spec.wattage_w));
    let power = cx.power;
    Report {
        verdict: Verdict::from_worst(issues.iter().map(|i| i.severity).max()),
        issues,
        estimated_watts: power.total_w,
        psu_watts,
        headroom_pct: psu_watts.map(|w| power.headroom_pct(w)),
        power_draws: power.draws,
        checked_rules,
    }
}

#[cfg(test)]
mod tests {
    use techdepo_catalog::Category;

    use super::*;
    use crate::fixtures::*;

    #[test]
    fn default_fixtures_are_fully_compatible() {
        let parts = default_parts();
        let build = Build::from_products(&parts).unwrap();
        let report = check_build(&build);
        assert!(report.issues.is_empty(), "{:#?}", report.issues);
        assert_eq!(report.verdict, Verdict::Compatible);
        assert!(build.missing_required().is_empty());
        // Every rule had its parts, except the ones needing several kits or a SATA drive.
        let unchecked: Vec<_> = RuleId::ALL
            .iter()
            .filter(|r| !report.checked(**r))
            .collect();
        assert_eq!(unchecked, [&RuleId::SataPorts, &RuleId::MemoryMixedKits]);
    }

    #[test]
    fn empty_build_is_trivially_compatible() {
        let report = check_build(&Build::new());
        assert_eq!(report.verdict, Verdict::Compatible);
        assert!(report.checked_rules.is_empty());
        assert_eq!(report.estimated_watts, 0);
        assert_eq!(report.psu_watts, None);
    }

    #[test]
    fn issues_are_sorted_by_severity() {
        let cpu = cpu_product("cpu", |s| s.peak_power_w = 250);
        let kit = memory_product("kit", |s| s.speed_mts = 7200);
        let gpu = gpu_product("gpu", |s| s.length_mm = 400);
        let case = case_product("case", |_| {});
        let cooler = cooler_product("cooler", |_| {});
        let report = check(&[&cpu, &kit, &gpu, &case, &cooler]);
        let severities: Vec<_> = report.issues.iter().map(|i| i.severity).collect();
        assert_eq!(
            severities,
            [Severity::Error, Severity::Warning, Severity::Note]
        );
        assert_eq!(report.verdict, Verdict::Incompatible);
        assert_eq!(report.worst_for(&gpu.id), Some(Severity::Error));
        assert_eq!(report.worst_for(&cooler.id), Some(Severity::Warning));
    }

    #[test]
    fn pair_check_skips_absence_rules_and_is_symmetric() {
        let cpu = cpu_product("cpu", |s| s.integrated_graphics = false);
        let board = board_product("board", |s| {
            s.bios_version = 1;
            s.cpu_support[1].min_bios = 5;
        });
        let ab = check_pair(&cpu, &board);
        let ba = check_pair(&board, &cpu);
        assert_eq!(ab, ba);
        assert_eq!(fired(&ab), [RuleId::BiosUpdate]);
        assert!(!ab.checked(RuleId::NoCooler));
    }

    #[test]
    fn pair_of_kits_is_order_independent() {
        let fast = memory_product("fast", |s| s.speed_mts = 6000);
        let slow = memory_product("slow", |s| s.speed_mts = 5200);
        let (ab, ba) = (check_pair(&fast, &slow), check_pair(&slow, &fast));
        assert_eq!(ab, ba);
        assert!(
            ab.issues[0].message.starts_with("fast and slow"),
            "{:?}",
            ab.issues
        );
    }

    #[test]
    fn same_single_slot_pair_is_empty() {
        let (a, b) = (cpu_product("a", |_| {}), cpu_product("b", |_| {}));
        let report = check_pair(&a, &b);
        assert!(report.issues.is_empty() && report.checked_rules.is_empty());
    }

    #[test]
    fn report_serializes() {
        let parts = default_parts();
        let report = check_build(&Build::from_products(&parts).unwrap());
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["verdict"], "compatible");
        assert_eq!(json["checked_rules"][0], "cpu_socket");
        let back: Report = serde_json::from_value(json).unwrap();
        assert_eq!(back, report);
        let slot = serde_json::to_value(Slot::new(Category::Memory, 1)).unwrap();
        assert_eq!(
            slot,
            serde_json::json!({ "category": "memory", "index": 1 })
        );
    }
}
