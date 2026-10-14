//! `xtask catalog validate`: everything the roadmap asks of the generated catalog, as one
//! report with a non-zero exit when anything fails.
//!
//! 1. `Catalog::validate`: schema, enums, unique IDs/SKUs/names, plausibility ranges,
//!    cross-field consistency, price tiers, content sections and unambiguous product names.
//! 2. Price tiers per category (the bands from `data/catalog/README.md`).
//! 3. Tier and platform coverage ([`crate::tiers`]).
//! 4. Rule coverage: a failing and a passing combination for every rule ([`crate::coverage`]).
//! 5. Content: every product has all required sections.
//! 6. Knowledge base: product names in `data/kb` resolve to one product each.

use techdepo_catalog::{CATALOG_VERSION, Catalog, Category, REQUIRED_SECTIONS, Tier};
use techdepo_compat::RuleId;
use techdepo_domain::Money;

use crate::coverage::{describe, rule_coverage};
use crate::table::Table;
use crate::tiers::{Platform, tier_coverage};

pub fn run() -> Result<(), String> {
    let catalog = Catalog::load_embedded().map_err(|e| format!("catalog does not parse: {e}"))?;
    let mut failures = Vec::new();

    println!(
        "Catalog {CATALOG_VERSION}: {} products, {} content files\n",
        catalog.len(),
        catalog.contents().count()
    );

    println!("Validation (schema, plausibility, consistency, tiers, content)");
    match catalog.validate() {
        Ok(()) => println!("  ok\n"),
        Err(errors) => {
            for error in &errors.0 {
                println!("  FAIL {error}");
            }
            println!();
            failures.push(format!("{} validation problem(s)", errors.0.len()));
        }
    }

    price_tiers(&catalog, &mut failures);
    platforms(&catalog, &mut failures);
    rules(&catalog, &mut failures);
    content(&catalog, &mut failures);
    knowledge_base(&catalog, &mut failures);

    if failures.is_empty() {
        println!("catalog validate: ok");
        Ok(())
    } else {
        Err(format!("catalog validate failed: {}", failures.join("; ")))
    }
}

fn price_tiers(catalog: &Catalog, failures: &mut Vec<String>) {
    println!("Price tiers");
    let mut table = Table::new(["category", "budget", "mid", "high", "status"]);
    let mut wrong = 0;
    for &category in Category::ALL {
        let products = catalog.by_category(category);
        let count = |tier| products.iter().filter(|p| p.tier == tier).count();
        let mismatched: Vec<&str> = products
            .iter()
            .filter(|p| p.tier != p.expected_tier())
            .map(|p| p.id.as_str())
            .collect();
        wrong += mismatched.len();
        let (budget, mid, high) = match category.price_bands() {
            Some(b) => (
                format!("< {}", b.mid_from),
                format!("{} – {}", b.mid_from, b.high_from - Money::from_cents(1)),
                format!(">= {}", b.high_from),
            ),
            None => (
                "SATA, PCIe 3.0".to_owned(),
                "PCIe 4.0".to_owned(),
                "PCIe 5.0".to_owned(),
            ),
        };
        table.row([
            category.as_str().to_owned(),
            format!("{budget} ({})", count(Tier::Budget)),
            format!("{mid} ({})", count(Tier::Mid)),
            format!("{high} ({})", count(Tier::High)),
            status(mismatched.is_empty(), || {
                format!("wrong tier: {}", mismatched.join(", "))
            }),
        ]);
    }
    println!("{table}");
    if wrong > 0 {
        failures.push(format!(
            "{wrong} product(s) outside their tier's price band"
        ));
    }
}

fn platforms(catalog: &Catalog, failures: &mut Vec<String>) {
    let coverage = tier_coverage(catalog);
    println!("Tier and platform coverage (budget / mid / high)");
    let mut table = Table::new(["category", "platform", "budget", "mid", "high", "total"]);
    for cell in &coverage.cells {
        table.row([
            cell.category.as_str().to_owned(),
            cell.platform.to_string(),
            cell.per_tier[0].to_string(),
            cell.per_tier[1].to_string(),
            cell.per_tier[2].to_string(),
            cell.total().to_string(),
        ]);
    }
    // Platform-bound categories also get a per-category total row.
    for (category, per_tier) in &coverage.per_category {
        let bound = coverage
            .cells
            .iter()
            .any(|c| c.category == *category && c.platform != Platform::Any);
        if bound {
            table.row([
                category.as_str().to_owned(),
                "(all)".to_owned(),
                per_tier[0].to_string(),
                per_tier[1].to_string(),
                per_tier[2].to_string(),
                per_tier.iter().sum::<usize>().to_string(),
            ]);
        }
    }
    print!("{table}");
    for problem in &coverage.problems {
        println!("  FAIL {problem}");
    }
    println!();
    if !coverage.problems.is_empty() {
        failures.push(format!(
            "{} tier/platform coverage problem(s)",
            coverage.problems.len()
        ));
    }
}

fn rules(catalog: &Catalog, failures: &mut Vec<String>) {
    let coverage = rule_coverage(catalog);
    println!("Rule coverage (small builds over the categories each rule reads)");
    let mut table = Table::new([
        "rule", "severity", "builds", "failing", "passing", "status", "trap", "passes",
    ]);
    let mut uncovered = Vec::new();
    for c in &coverage {
        let covered = c.is_covered();
        if !covered {
            uncovered.push(c.rule.as_str());
        }
        table.row([
            c.rule.as_str().to_owned(),
            c.rule.severity().as_str().to_owned(),
            c.builds.to_string(),
            c.failing.to_string(),
            c.passing.to_string(),
            status(covered, || {
                match (c.failing, c.passing) {
                    (0, 0) => "no trap, no pass",
                    (0, _) => "no trap",
                    _ => "no pass",
                }
                .to_owned()
            }),
            c.failing_example
                .as_deref()
                .map(describe)
                .unwrap_or_default(),
            c.passing_example
                .as_deref()
                .map(describe)
                .unwrap_or_default(),
        ]);
    }
    println!("{table}");
    debug_assert_eq!(coverage.len(), RuleId::ALL.len());
    if !uncovered.is_empty() {
        failures.push(format!("rules without coverage: {}", uncovered.join(", ")));
    }
}

fn content(catalog: &Catalog, failures: &mut Vec<String>) {
    println!("Content ({})", REQUIRED_SECTIONS.join(", "));
    let mut incomplete = Vec::new();
    let (mut pros, mut cons, mut faq, mut words) = (0, 0, 0, 0);
    for product in catalog.all() {
        let Some(content) = catalog.content(&product.id) else {
            incomplete.push(format!("{}: no content file", product.id));
            continue;
        };
        let headings: Vec<&str> = content.sections().iter().map(|s| s.heading).collect();
        let complete = headings == REQUIRED_SECTIONS
            && content.sections().iter().all(|s| !s.body.is_empty())
            && !content.pros().is_empty()
            && !content.cons().is_empty()
            && !content.faq().is_empty();
        if !complete {
            incomplete.push(format!("{}: sections {headings:?}", product.id));
        }
        pros += content.pros().len();
        cons += content.cons().len();
        faq += content.faq().len();
        words += content.markdown().split_whitespace().count();
    }
    let complete = catalog.len() - incomplete.len();
    let n = catalog.len().max(1);
    println!(
        "  {complete}/{} products complete; per product on average {} words, {:.1} pros, {:.1} cons, {:.1} FAQ entries",
        catalog.len(),
        words / n,
        pros as f64 / n as f64,
        cons as f64 / n as f64,
        faq as f64 / n as f64,
    );
    for problem in &incomplete {
        println!("  FAIL {problem}");
    }
    println!();
    if !incomplete.is_empty() {
        failures.push(format!(
            "{} product(s) with incomplete content",
            incomplete.len()
        ));
    }
}

/// Knowledge-base articles (`data/kb`) name products the way content files must: by a name
/// that resolves to exactly one product. Agents read both through tools and quote them.
fn knowledge_base(catalog: &Catalog, failures: &mut Vec<String>) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/kb");
    println!("Knowledge base product names ({})", dir.display());
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) => {
            println!("  FAIL cannot read: {e}\n");
            failures.push(format!("knowledge base not readable: {e}"));
            return;
        }
    };
    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    paths.sort();
    let mut problems = Vec::new();
    for path in &paths {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        match std::fs::read_to_string(path) {
            Ok(text) => problems.extend(
                catalog
                    .ambiguous_mentions(&text)
                    .into_iter()
                    .map(|p| format!("{name}: {p}")),
            ),
            Err(e) => problems.push(format!("{name}: {e}")),
        }
    }
    println!("  {} articles checked", paths.len());
    for problem in &problems {
        println!("  FAIL {problem}");
    }
    println!();
    if !problems.is_empty() {
        failures.push(format!(
            "{} ambiguous product name(s) in the knowledge base",
            problems.len()
        ));
    }
}

fn status(ok: bool, problem: impl FnOnce() -> String) -> String {
    if ok {
        "ok".to_owned()
    } else {
        format!("FAIL {}", problem())
    }
}
