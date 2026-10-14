//! Alternatives for a slot and fix suggestions for a report.

use std::collections::BTreeSet;
use std::ops::Range;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use techdepo_catalog::{Catalog, Product};
use techdepo_domain::{Money, ProductId};

use crate::build::{Build, Slot};
use crate::check_build;
use crate::report::Report;
use crate::rule::{RuleId, Severity};

/// At most this many candidates per [`Fix`].
pub const MAX_FIX_CANDIDATES: usize = 5;

/// A way to resolve one rule's issue: put one of `candidates` into `slot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Fix {
    /// The rule whose issue this resolves.
    pub rule: RuleId,
    /// The slot to change.
    pub slot: Slot,
    /// The part to replace; `None` means add a part to an empty slot.
    pub replaces: Option<ProductId>,
    /// Replacements that resolve the issue without new errors or (if any can) new warnings,
    /// best first.
    pub candidates: Vec<ProductId>,
}

/// `price` ± `pct` percent, both ends included.
pub fn price_band_around(price: Money, pct: u32) -> Range<Money> {
    let cents = price.cents();
    let delta = cents * i64::from(pct) / 100;
    Money::from_cents((cents - delta).max(0))..Money::from_cents(cents + delta + 1)
}

/// Any price at all.
fn any_price() -> Range<Money> {
    Money::ZERO..Money::from_cents(i64::MAX)
}

/// Catalog products that could go into `slot` instead of its current part (or into it, when
/// empty), best first.
///
/// A candidate qualifies when its price is in `price_band` and, placed into the build, no
/// error involves it: it is compatible with the rest of the build. Candidates that bring no
/// warnings rank before ones that do; then the closest price to the replaced part wins (for
/// an empty slot: the cheapest).
pub fn alternatives<'a>(
    catalog: &'a Catalog,
    build: &Build<'a>,
    slot: Slot,
    price_band: Range<Money>,
) -> Vec<ProductId> {
    ranked(catalog.by_category(slot.category), build, slot, &price_band)
        .into_iter()
        .map(|c| c.product.id.clone())
        .collect()
}

/// For every error and warning in `report` (which must be `check_build(build)`), the slots
/// whose replacement (or, for a missing part, addition) resolves it, with candidates.
///
/// A candidate resolves an issue when the new build has fewer issues of that rule and no
/// error involving the candidate. Every candidate is re-checked against the whole build: one
/// that brings a new error anywhere is never suggested, and one that brings a new warning (a
/// 650 W PSU for a wattage error whose message asks for 850 W, which then warns about
/// headroom) only for an error, and only when no candidate resolves it cleanly. A warning is
/// never "fixed" by trading it for another. There is no price band: candidates are ranked like
/// [`alternatives`] (warning-free first, then closest to the replaced part's price, or
/// cheapest when adding) and capped at [`MAX_FIX_CANDIDATES`]. Slots without any candidate
/// are left out.
pub fn suggest_fixes<'a>(catalog: &'a Catalog, build: &Build<'a>, report: &Report) -> Vec<Fix> {
    let mut fixes = Vec::new();
    let mut seen = BTreeSet::new();
    for issue in report
        .issues
        .iter()
        .filter(|i| i.severity >= Severity::Warning)
    {
        let added = issue.rule.fixed_by_adding().map(Slot::from);
        let replaced = issue.parts.iter().filter_map(|id| build.slot_of(id));
        for slot in added.into_iter().chain(replaced) {
            if !seen.insert((issue.rule, slot)) {
                continue;
            }
            let products = catalog.by_category(slot.category);
            if let Some(fix) = fix_for(products, build, report, issue.rule, slot) {
                fixes.push(fix);
            }
        }
    }
    fixes
}

fn fix_for<'a>(
    products: &'a [Product],
    build: &Build<'a>,
    report: &Report,
    rule: RuleId,
    slot: Slot,
) -> Option<Fix> {
    let replaces = build.get(slot);
    let mut clean = Vec::new();
    let mut caveat = Vec::new();
    for c in ranked(products, build, slot, &any_price()) {
        if count(&c.report, rule) >= count(report, rule) {
            continue;
        }
        match regression(report, &c.report, rule) {
            None => clean.push(c.product.id.clone()),
            Some(Severity::Warning) if rule.severity() == Severity::Error => {
                caveat.push(c.product.id.clone());
            }
            Some(_) => {}
        }
    }
    let mut candidates = if clean.is_empty() { caveat } else { clean };
    candidates.truncate(MAX_FIX_CANDIDATES);
    (!candidates.is_empty()).then(|| Fix {
        rule,
        slot,
        replaces: replaces.map(|p| p.id.clone()),
        candidates,
    })
}

fn count(report: &Report, rule: RuleId) -> usize {
    report.issues.iter().filter(|i| i.rule == rule).count()
}

/// The worst severity (warning or error) among the rules other than `fixed` that report more
/// issues `after` than `before`: what a candidate breaks while fixing `fixed`.
fn regression(before: &Report, after: &Report, fixed: RuleId) -> Option<Severity> {
    after
        .issues
        .iter()
        .filter(|i| i.severity >= Severity::Warning && i.rule != fixed)
        .filter(|i| count(after, i.rule) > count(before, i.rule))
        .map(|i| i.severity)
        .max()
}

/// A qualifying replacement with the report of the build it produces.
pub(crate) struct Candidate<'a> {
    pub product: &'a Product,
    pub report: Report,
}

pub(crate) fn ranked<'a>(
    products: &'a [Product],
    build: &Build<'a>,
    slot: Slot,
    band: &Range<Money>,
) -> Vec<Candidate<'a>> {
    let current = build.get(slot);
    let reference = current.map_or(band.start, |p| p.price);
    let mut scored = Vec::new();
    for product in products {
        if product.category != slot.category
            || !band.contains(&product.price)
            || current.is_some_and(|c| c.id == product.id)
        {
            continue;
        }
        let mut trial = build.clone();
        if trial.set(slot, product).is_err() {
            return Vec::new();
        }
        let report = check_build(&trial);
        let worst = report.worst_for(&product.id);
        if worst == Some(Severity::Error) {
            continue;
        }
        let caveat = worst == Some(Severity::Warning);
        let distance = (product.price - reference).abs();
        scored.push((
            (caveat, distance, product.price, &product.id),
            Candidate { product, report },
        ));
    }
    scored.sort_by(|a, b| a.0.cmp(&b.0));
    scored.into_iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use techdepo_catalog::{Category, MemoryType, PsuFormFactor};

    use super::*;
    use crate::fixtures::*;

    fn ids<'c>(candidates: &'c [Candidate<'_>]) -> Vec<&'c str> {
        candidates.iter().map(|c| c.product.id.as_str()).collect()
    }

    #[test]
    fn band_is_inclusive() {
        let band = price_band_around(Money::from_euros(100), 20);
        assert!(band.contains(&Money::from_euros(80)));
        assert!(band.contains(&Money::from_euros(120)));
        assert!(!band.contains(&Money::from_cents(12001)));
    }

    #[test]
    fn alternatives_are_compatible_in_band_and_closest_first() {
        let board = board_product("board", |_| {});
        let current = priced(
            memory_product("ddr4-now", |s| s.memory_type = MemoryType::Ddr4),
            100,
        );
        let pool = vec![
            priced(memory_product("ddr5-far", |_| {}), 140),
            priced(memory_product("ddr5-near", |_| {}), 95),
            priced(memory_product("ddr5-mid", |_| {}), 110),
            priced(
                memory_product("ddr4-other", |s| s.memory_type = MemoryType::Ddr4),
                100,
            ),
            priced(memory_product("ddr5-pricey", |_| {}), 400),
            board_product("not-memory", |_| {}),
        ];
        let build = Build::from_products([&board, &current]).unwrap();
        let slot = Slot::new(Category::Memory, 0);
        let band = Money::from_euros(50)..Money::from_euros(200);
        assert_eq!(
            ids(&ranked(&pool, &build, slot, &band)),
            ["ddr5-near", "ddr5-mid", "ddr5-far"]
        );
    }

    #[test]
    fn warning_free_candidates_rank_first() {
        let cpu = cpu_product("cpu", |s| s.peak_power_w = 120);
        let current = priced(cooler_product("now", |s| s.tdp_rating_w = 100), 50);
        let pool = vec![
            priced(cooler_product("weak-near", |s| s.tdp_rating_w = 110), 50),
            priced(cooler_product("strong-far", |s| s.tdp_rating_w = 200), 80),
        ];
        let build = Build::from_products([&cpu, &current]).unwrap();
        let ranked = ranked(&pool, &build, Category::Cooler.into(), &any_price());
        assert_eq!(ids(&ranked), ["strong-far", "weak-near"]);
    }

    fn fix_ids(fix: Option<Fix>) -> Vec<String> {
        fix.map(|f| f.candidates.iter().map(|id| id.to_string()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn a_fix_for_an_error_prefers_candidates_that_bring_no_warning() {
        let gpu = gpu_product("gpu", |s| s.board_power_w = 450);
        let psu = priced(psu_product("p450", |s| s.wattage_w = 450), 50);
        let build = Build::from_products([&gpu, &psu]).unwrap();
        let report = check_build(&build);
        assert!(report.issues.iter().any(|i| i.rule == RuleId::PsuWattage));
        // 495 W of 550 W fixes the error but warns about headroom; 650 W is comfortable.
        let tight = priced(psu_product("p550", |s| s.wattage_w = 550), 60);
        let roomy = priced(psu_product("p650", |s| s.wattage_w = 650), 90);
        let slot = Category::Psu.into();
        let both = vec![tight.clone(), roomy];
        let fix = fix_for(&both, &build, &report, RuleId::PsuWattage, slot);
        assert_eq!(fix_ids(fix), ["p650"]);
        // Without a clean candidate, fixing the error still beats keeping it.
        let only_tight = vec![tight];
        let fix = fix_for(&only_tight, &build, &report, RuleId::PsuWattage, slot);
        assert_eq!(fix_ids(fix), ["p550"]);
    }

    #[test]
    fn a_warning_is_not_traded_for_another() {
        let gpu = gpu_product("gpu", |s| s.board_power_w = 450);
        let psu = priced(psu_product("p550", |s| s.wattage_w = 550), 50);
        let case = case_product("case", |_| {});
        let build = Build::from_products([&gpu, &psu, &case]).unwrap();
        let report = check_build(&build);
        assert!(report.issues.iter().any(|i| i.rule == RuleId::PsuHeadroom));
        // Enough watts, but an SFX unit in an ATX bay needs a bracket (another warning).
        let sfx = priced(
            psu_product("sfx650", |s| {
                s.wattage_w = 650;
                s.form_factor = PsuFormFactor::Sfx;
            }),
            55,
        );
        let atx = priced(psu_product("atx650", |s| s.wattage_w = 650), 90);
        let slot = Category::Psu.into();
        let both = vec![sfx.clone(), atx];
        let fix = fix_for(&both, &build, &report, RuleId::PsuHeadroom, slot);
        assert_eq!(fix_ids(fix), ["atx650"]);
        let only_sfx = vec![sfx];
        assert_eq!(
            fix_for(&only_sfx, &build, &report, RuleId::PsuHeadroom, slot),
            None
        );
    }

    #[test]
    fn empty_slot_prefers_the_cheapest() {
        let cpu = cpu_product("cpu", |_| {});
        let pool = vec![
            priced(cooler_product("b", |_| {}), 90),
            priced(cooler_product("a", |_| {}), 30),
        ];
        let build = Build::from_products([&cpu]).unwrap();
        let ranked = ranked(&pool, &build, Category::Cooler.into(), &any_price());
        assert_eq!(ids(&ranked), ["a", "b"]);
        // Past the end of a multi slot: nothing.
        let far = Slot::new(Category::Memory, 3);
        assert!(
            super::ranked(&[memory_product("k", |_| {})], &build, far, &any_price()).is_empty()
        );
    }
}
