//! Rule coverage: every compatibility rule must have at least one failing ("trap") and one
//! passing combination in the catalog.
//!
//! The search checks small builds over every non-empty subset of the categories a rule reads
//! ([`RuleId::categories`]): one product per single slot, and for memory and storage every
//! multiset of up to a few kits or drives plus "n of the same" up to [`MAX_REPEAT`], which is
//! what "more modules than slots" or "more drives than ports" needs. Subsets matter because
//! some rules only fire while a part is missing (a DDR5 kit with a DDR4-only CPU and no board
//! yet). Smaller builds come first, so the examples are minimal.
//!
//! A build *fails* a rule when the rule reports an issue; the example shown prefers a build where
//! no other rule reports an error. A build *passes* a rule when it has every category the rule
//! reads, the rule stays silent and the build has no errors at all, so the example is a working
//! alternative rather than a different trap.

use std::collections::{BTreeMap, BTreeSet};

use techdepo_catalog::{Catalog, Category, Product};
use techdepo_compat::{Build, Report, RuleId, Severity, check_build};

/// Distinct kits or drives combined freely in one build.
const MAX_MIXED: [(Category, usize); 2] = [(Category::Memory, 2), (Category::Storage, 3)];
/// Copies of one kit or drive in one build.
const MAX_REPEAT: usize = 8;

/// What the search found for one rule.
#[derive(Debug, Clone)]
pub struct RuleCoverage<'a> {
    /// The rule.
    pub rule: RuleId,
    /// Builds searched for this rule.
    pub builds: usize,
    /// Builds the rule reported an issue for.
    pub failing: usize,
    /// Error-free builds with all of the rule's categories where it stayed silent.
    pub passing: usize,
    /// The smallest failing build, preferring one without other errors.
    pub failing_example: Option<Vec<&'a str>>,
    /// The smallest passing build.
    pub passing_example: Option<Vec<&'a str>>,
    /// Whether `failing_example` has errors from other rules too.
    failing_example_clean: bool,
}

impl<'a> RuleCoverage<'a> {
    fn new(rule: RuleId) -> Self {
        Self {
            rule,
            builds: 0,
            failing: 0,
            passing: 0,
            failing_example: None,
            passing_example: None,
            failing_example_clean: false,
        }
    }

    /// Both a failing and a passing combination exist.
    pub fn is_covered(&self) -> bool {
        self.failing > 0 && self.passing > 0
    }

    fn record(&mut self, complete: bool, parts: &[&'a Product], report: &Report) {
        self.builds += 1;
        let rule = self.rule;
        if report.issues.iter().any(|i| i.rule == rule) {
            self.failing += 1;
            let clean = !report
                .issues
                .iter()
                .any(|i| i.rule != rule && i.severity == Severity::Error);
            if self.failing_example.is_none() || clean && !self.failing_example_clean {
                self.failing_example = Some(ids(parts));
                self.failing_example_clean = clean;
            }
        } else if complete && report.worst() != Some(Severity::Error) {
            self.passing += 1;
            self.passing_example.get_or_insert_with(|| ids(parts));
        }
    }
}

/// Searches `catalog` for a failing and a passing combination of every rule.
pub fn rule_coverage(catalog: &Catalog) -> Vec<RuleCoverage<'_>> {
    // Every category subset any rule needs, each enumerated once, smallest first.
    let mut subsets: BTreeSet<(usize, Vec<Category>)> = BTreeSet::new();
    for &rule in RuleId::ALL {
        let categories = rule.categories();
        for mask in 1..1u32 << categories.len() {
            let mut subset: Vec<Category> = categories
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, c)| *c)
                .collect();
            // Multi-part categories first, so builds grow in size as the search proceeds.
            subset.sort_by_key(|c| (!is_multi(*c), c.index()));
            subsets.insert((subset.len(), subset));
        }
    }

    let mut coverage: BTreeMap<RuleId, RuleCoverage<'_>> = RuleId::ALL
        .iter()
        .map(|&rule| (rule, RuleCoverage::new(rule)))
        .collect();
    for (_, subset) in subsets {
        // (rule, whether the subset has all of its categories)
        let rules: Vec<(RuleId, bool)> = RuleId::ALL
            .iter()
            .filter(|r| subset.iter().all(|c| r.categories().contains(c)))
            .map(|&r| (r, subset.len() == r.categories().len()))
            .collect();
        let options: Vec<Vec<Vec<&Product>>> = subset
            .iter()
            .map(|&category| category_options(catalog, category))
            .collect();
        for_each_combination(&options, &mut |parts| {
            let Ok(build) = Build::from_products(parts.iter().copied()) else {
                return;
            };
            let report = check_build(&build);
            for &(rule, complete) in &rules {
                let entry = coverage.get_mut(&rule).expect("every rule has an entry");
                entry.record(complete, parts, &report);
            }
        });
    }
    coverage.into_values().collect()
}

fn is_multi(category: Category) -> bool {
    MAX_MIXED.iter().any(|(c, _)| *c == category)
}

fn ids<'a>(parts: &[&'a Product]) -> Vec<&'a str> {
    parts.iter().map(|p| p.id.as_str()).collect()
}

/// The ways to fill one category in a search build, smallest first.
fn category_options(catalog: &Catalog, category: Category) -> Vec<Vec<&Product>> {
    let products: Vec<&Product> = catalog.by_category(category).iter().collect();
    let Some(&(_, max_mixed)) = MAX_MIXED.iter().find(|(c, _)| *c == category) else {
        return products.into_iter().map(|p| vec![p]).collect();
    };
    let mut options = Vec::new();
    for size in 1..=max_mixed {
        multisets(&products, size, 0, &mut Vec::new(), &mut options);
    }
    for count in max_mixed + 1..=MAX_REPEAT {
        options.extend(products.iter().map(|&p| vec![p; count]));
    }
    options
}

/// Every multiset of `size` items from `items[start..]`, in order.
fn multisets<'a>(
    items: &[&'a Product],
    size: usize,
    start: usize,
    current: &mut Vec<&'a Product>,
    out: &mut Vec<Vec<&'a Product>>,
) {
    if current.len() == size {
        out.push(current.clone());
        return;
    }
    for i in start..items.len() {
        current.push(items[i]);
        multisets(items, size, i, current, out);
        current.pop();
    }
}

/// Calls `f` with every combination of one option per category (first category outermost).
fn for_each_combination<'a>(options: &[Vec<Vec<&'a Product>>], f: &mut impl FnMut(&[&'a Product])) {
    fn go<'a>(
        options: &[Vec<Vec<&'a Product>>],
        parts: &mut Vec<&'a Product>,
        f: &mut impl FnMut(&[&'a Product]),
    ) {
        let Some((first, rest)) = options.split_first() else {
            f(parts);
            return;
        };
        for option in first {
            let len = parts.len();
            parts.extend_from_slice(option);
            go(rest, parts, f);
            parts.truncate(len);
        }
    }
    go(options, &mut Vec::new(), f);
}

/// `a + 3× b` style summary of a build.
pub fn describe(parts: &[&str]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let run = parts[i..].iter().take_while(|p| **p == parts[i]).count();
        out.push(if run > 1 {
            format!("{run}× {}", parts[i])
        } else {
            parts[i].to_owned()
        });
        i += run;
    }
    out.join(" + ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multisets_are_complete() {
        let catalog = Catalog::global();
        let three: Vec<&Product> = catalog.by_category(Category::Memory)[..3].iter().collect();
        let mut out = Vec::new();
        multisets(&three, 2, 0, &mut Vec::new(), &mut out);
        // C(3 + 2 - 1, 2)
        assert_eq!(out.len(), 6);
    }

    #[test]
    fn embedded_catalog_covers_every_rule() {
        let coverage = rule_coverage(Catalog::global());
        assert_eq!(coverage.len(), RuleId::ALL.len());
        let uncovered: Vec<_> = coverage.iter().filter(|c| !c.is_covered()).collect();
        assert!(uncovered.is_empty(), "{uncovered:#?}");
    }

    #[test]
    fn describe_collapses_repeats() {
        assert_eq!(describe(&["a", "b", "b", "b", "c"]), "a + 3× b + c");
    }
}
