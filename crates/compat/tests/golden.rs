//! Golden full builds over the embedded catalog, including the trap builds used in demos.
//!
//! The rule sets are asserted directly; the full reports (messages, wattage, fixes) are also
//! snapshotted so wording changes get reviewed.

use std::fmt::Write as _;

use techdepo_catalog::{Catalog, Category, MemoryType, Product};
use techdepo_compat::{
    Build, Fix, Report, RuleId, Severity, Slot, Verdict, alternatives, check_build, check_products,
    price_band_around, suggest_fixes,
};
use techdepo_domain::ProductId;

fn build(ids: &[&str]) -> Build<'static> {
    Build::from_ids(Catalog::global(), ids).expect("golden build uses catalog ids")
}

fn product(id: &str) -> &'static Product {
    Catalog::global().get(id).expect("catalog id")
}

fn rules(report: &Report) -> Vec<RuleId> {
    report.issues.iter().map(|i| i.rule).collect()
}

/// A compact, reviewable rendering of a report and its fixes.
fn render(name: &str, build: &Build<'_>, report: &Report, fixes: &[Fix]) -> String {
    let mut out = String::new();
    writeln!(out, "## {name}").unwrap();
    writeln!(
        out,
        "parts: {}",
        build
            .ids()
            .iter()
            .map(ProductId::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    )
    .unwrap();
    let psu = match (report.psu_watts, report.headroom_pct) {
        (Some(watts), Some(headroom)) => format!("{watts} W PSU, {headroom} % headroom"),
        _ => "no PSU".to_owned(),
    };
    writeln!(
        out,
        "verdict: {} | estimate {} W, {psu}",
        report.verdict.as_str(),
        report.estimated_watts,
    )
    .unwrap();
    for issue in &report.issues {
        writeln!(
            out,
            "- {} {}: {}",
            issue.severity.as_str(),
            issue.rule.as_str(),
            issue.message
        )
        .unwrap();
    }
    for fix in fixes {
        let action = match &fix.replaces {
            Some(id) => format!("replace {id}"),
            None => format!("add {}", fix.slot),
        };
        let candidates: Vec<&str> = fix.candidates.iter().map(ProductId::as_str).collect();
        writeln!(
            out,
            "  fix {} -> {action} with [{}]",
            fix.rule.as_str(),
            candidates.join(", ")
        )
        .unwrap();
    }
    out
}

const AM5_GAMING: &[&str] = &[
    "voltaris-v5-8600",
    "kestrel-b650m-wing",
    "nimbus-ddr5-6000-32",
    "arcwave-a4-1tb",
    "halcyon-hx6060-dual",
    "tessel-atlas-mid",
    "ferro-core-650",
    "breeze-glacier-240",
];

const AM4_BUDGET: &[&str] = &[
    "voltaris-v5-5600",
    "kestrel-b550-glide",
    "nimbus-ddr4-3200-16",
    "arcwave-s2-2tb",
    "halcyon-hx6060-dual",
    "tessel-atlas-mid",
    "ferro-core-650",
    "breeze-frost-120",
];

/// Demo scenario 8: "Will the Kestrel B650 work with my Nimbus DDR4 kit?"
const DDR4_ON_B650: &[&str] = &["kestrel-b650m-wing", "nimbus-ddr4-3200-16"];

/// Everything wrong with a small case at once.
const SFF_TRAPS: &[&str] = &[
    "tessel-nook-itx",
    "halcyon-hx7080-triple",
    "breeze-frost-120",
    "ferro-core-650",
];

/// AM4 CPU without integrated graphics, and no graphics card.
const NO_VIDEO: &[&str] = &[
    "voltaris-v5-5600",
    "kestrel-b550-glide",
    "nimbus-ddr4-3200-16",
];

/// A "125 W" Intel CPU that draws 253 W, four DDR5 modules and a single-EPS power supply.
const HOT_INTEL: &[&str] = &[
    "helion-h7-1470",
    "meridian-z790-crest",
    "nimbus-ddr5-6000-32",
    "nimbus-ddr5-6000-32",
    "arcwave-a4-1tb",
    "halcyon-hx6060-dual",
    "lattice-flow-500",
    "nordwatt-prime-650",
    "breeze-frost-120",
];

/// An SFX power supply in an ATX tower, and tall memory under a tower cooler in a low case.
const SFX_IN_TOWER: &[&str] = &[
    "voltaris-v5-8600",
    "kestrel-b650-aero",
    "lumen-ddr5-7200-32",
    "arcwave-a4-1tb",
    "halcyon-hx6060-dual",
    "lattice-core-200",
    "ferro-sf750",
    "breeze-frost-120",
];

const GOLDEN: &[(&str, &[&str])] = &[
    ("am5_gaming", AM5_GAMING),
    ("am4_budget", AM4_BUDGET),
    ("ddr4_on_b650", DDR4_ON_B650),
    ("sff_traps", SFF_TRAPS),
    ("no_video", NO_VIDEO),
    ("hot_intel", HOT_INTEL),
    ("sfx_in_tower", SFX_IN_TOWER),
];

#[test]
fn am5_gaming_needs_a_bios_update() {
    let report = check_build(&build(AM5_GAMING));
    assert_eq!(report.verdict, Verdict::Warnings);
    assert_eq!(rules(&report), [RuleId::BiosUpdate, RuleId::MemorySpeed]);
    let bios = &report.issues[0];
    assert!(bios.message.contains("BIOS 10") && bios.message.contains("BIOS 16"));
    assert!(bios.message.contains("Flashback"));
    assert!(report.issues[1].message.contains("run at 5600 MT/s"));
    assert!(build(AM5_GAMING).missing_required().is_empty());
}

#[test]
fn am4_budget_is_clean() {
    let report = check_build(&build(AM4_BUDGET));
    assert_eq!(report.verdict, Verdict::Compatible, "{:#?}", report.issues);
    assert!(report.issues.is_empty());
    assert!(report.headroom_pct.unwrap() >= 20);
}

#[test]
fn ddr4_kit_on_b650_suggests_ddr5_kits_or_a_ddr4_board() {
    let build = build(DDR4_ON_B650);
    let report = check_build(&build);
    assert_eq!(report.verdict, Verdict::Incompatible);
    assert_eq!(rules(&report), [RuleId::MemoryType]);
    assert!(report.issues[0].message.contains("won't physically fit"));

    let fixes = suggest_fixes(Catalog::global(), &build, &report);
    let memory = fixes
        .iter()
        .find(|f| f.slot == Slot::new(Category::Memory, 0))
        .expect("a memory fix");
    assert_eq!(
        memory.replaces.as_ref().map(ProductId::as_str),
        Some("nimbus-ddr4-3200-16")
    );
    assert!(!memory.candidates.is_empty());
    for id in &memory.candidates {
        let kit = product(id.as_str()).spec.as_memory().unwrap();
        assert_eq!(kit.memory_type, MemoryType::Ddr5, "{id}");
    }
    let board = fixes
        .iter()
        .find(|f| f.slot == Category::Motherboard.into())
        .expect("a board fix");
    for id in &board.candidates {
        let mb = product(id.as_str()).spec.as_motherboard().unwrap();
        assert_eq!(mb.memory_type, MemoryType::Ddr4, "{id}");
    }

    // "A DDR5 kit in the same price range" (±20 %) exists for every DDR4 kit of the scenario,
    // with the same capacity, and the closest-priced one comes first.
    let slot = Slot::new(Category::Memory, 0);
    for (ddr4, expected) in [
        ("nimbus-ddr4-3200-16", "nimbus-ddr5-4800-16"),
        ("nimbus-ddr4-3200-32", "nimbus-ddr5-5200-32"),
    ] {
        let kit = product(ddr4);
        let build = self::build(&["kestrel-b650m-wing", ddr4]);
        let band = price_band_around(kit.price, 20);
        let found = alternatives(Catalog::global(), &build, slot, band.clone());
        assert_eq!(
            found.first().map(ProductId::as_str),
            Some(expected),
            "{ddr4}: {found:?}"
        );
        for id in &found {
            let p = product(id.as_str());
            assert!(band.contains(&p.price), "{id}");
            assert_eq!(p.spec.as_memory().unwrap().memory_type, MemoryType::Ddr5);
        }
        let same_capacity = |id: &str| product(id).spec.as_memory().unwrap().total_gb();
        assert_eq!(same_capacity(expected), same_capacity(ddr4));
    }
}

#[test]
fn sff_traps_all_fire_and_have_fixes() {
    let build = build(SFF_TRAPS);
    let report = check_build(&build);
    assert_eq!(report.verdict, Verdict::Incompatible);
    assert_eq!(
        rules(&report),
        [
            RuleId::GpuLength,
            RuleId::GpuThickness,
            RuleId::CoolerHeight,
            RuleId::PsuFormFactor,
            RuleId::GpuPowerConnectors,
            RuleId::GpuRecommendedPsu,
        ]
    );
    let fixes = suggest_fixes(Catalog::global(), &build, &report);
    let psu_fix = fixes
        .iter()
        .find(|f| f.rule == RuleId::PsuFormFactor && f.slot == Category::Psu.into())
        .expect("an SFX power supply");
    assert!(
        psu_fix
            .candidates
            .iter()
            .any(|id| id.as_str() == "ferro-sf750")
    );
    // Every candidate really resolves its rule.
    for fix in &fixes {
        for candidate in &fix.candidates {
            let mut trial = build.clone();
            trial.set(fix.slot, product(candidate.as_str())).unwrap();
            let after = check_build(&trial);
            let before = report.issues.iter().filter(|i| i.rule == fix.rule).count();
            let now = after.issues.iter().filter(|i| i.rule == fix.rule).count();
            assert!(now < before, "{candidate} does not fix {:?}", fix.rule);
            assert_ne!(
                after.worst_for(candidate),
                Some(techdepo_compat::Severity::Error)
            );
        }
    }
}

#[test]
fn hot_intel_warns_about_cooling_power_and_memory_speed() {
    let report = check_build(&build(HOT_INTEL));
    assert_eq!(report.verdict, Verdict::Warnings, "{:#?}", report.issues);
    assert_eq!(
        rules(&report),
        [
            RuleId::PsuHeadroom,
            RuleId::CoolerTdp,
            RuleId::CpuPowerConnectors,
            RuleId::MemorySpeed,
            RuleId::MemoryMixedKits,
        ]
    );
    let cooler = &report.issues[1];
    assert!(
        cooler.message.contains("draws up to 253 W"),
        "{}",
        cooler.message
    );
    let speed = &report.issues[3];
    assert!(
        speed
            .message
            .starts_with("2× Nimbus Stratus DDR5-6000 32 GB (2x16 GB) are rated"),
        "{}",
        speed.message
    );
    assert!(
        speed.message.contains("run at 3600 MT/s"),
        "{}",
        speed.message
    );
}

#[test]
fn sfx_in_tower_needs_a_bracket_and_a_lower_cooler() {
    let report = check_build(&build(SFX_IN_TOWER));
    assert_eq!(report.verdict, Verdict::Incompatible);
    assert_eq!(
        rules(&report),
        [
            RuleId::CoolerHeight,
            RuleId::RamClearance,
            RuleId::PsuBracket,
            RuleId::MemorySpeed,
        ]
    );
    assert!(
        report.issues[0]
            .message
            .contains("165 mm tall with its fan raised for Lumen Apex DDR5-7200 32 GB (2x16 GB)"),
        "{}",
        report.issues[0].message
    );
}

#[test]
fn missing_video_output_suggests_adding_a_gpu() {
    let build = build(NO_VIDEO);
    let report = check_build(&build);
    assert_eq!(rules(&report), [RuleId::NoVideoOutput]);
    assert_eq!(
        build.missing_required(),
        [Category::Storage, Category::Case, Category::Psu]
    );
    let fixes = suggest_fixes(Catalog::global(), &build, &report);
    let add = fixes
        .iter()
        .find(|f| f.replaces.is_none())
        .expect("add a GPU");
    assert_eq!(add.slot, Category::Gpu.into());
    // Cheapest first when adding.
    let prices: Vec<_> = add
        .candidates
        .iter()
        .map(|id| product(id.as_str()).price)
        .collect();
    assert!(prices.is_sorted(), "{prices:?}");
    let swap = fixes
        .iter()
        .find(|f| f.slot == Category::Cpu.into())
        .expect("swap the CPU");
    for id in &swap.candidates {
        assert!(
            product(id.as_str())
                .spec
                .as_cpu()
                .unwrap()
                .integrated_graphics
        );
    }
}

#[test]
fn check_products_reports_unknown_ids() {
    let ids = [
        ProductId::new("kestrel-b650m-wing"),
        ProductId::new("no-such-part"),
    ];
    let err = check_products(Catalog::global(), &ids).unwrap_err();
    assert!(err.to_string().contains("no-such-part"));
    let ok = check_products(Catalog::global(), &[ProductId::new("kestrel-b650m-wing")]).unwrap();
    assert!(ok.issues.is_empty());
}

#[test]
fn golden_reports() {
    let mut out = String::new();
    for (name, ids) in GOLDEN {
        let build = build(ids);
        let report = check_build(&build);
        let fixes = suggest_fixes(Catalog::global(), &build, &report);
        out.push_str(&render(name, &build, &report, &fixes));
        out.push('\n');
    }
    insta::assert_snapshot!("golden_reports", out);
}

/// Every suggested fix, over pseudo-random full builds from the catalog: the candidate removes
/// an issue of its rule, never brings a new error, and brings a new warning only as the
/// fallback fix of an error. (A warning that its own fix brings back, such as a PSU-headroom
/// warning after taking the suggested PSU for a wattage error, contradicts the builder.)
#[test]
fn fixes_resolve_their_issue_without_breaking_the_build() {
    let catalog = Catalog::global();
    let count =
        |report: &Report, rule: RuleId| report.issues.iter().filter(|i| i.rule == rule).count();
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut pick = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    for _ in 0..500 {
        let mut build = Build::new();
        for category in Category::ALL {
            let products = catalog.by_category(*category);
            build.add(&products[pick(products.len())]);
        }
        let report = check_build(&build);
        for fix in suggest_fixes(catalog, &build, &report) {
            for id in &fix.candidates {
                let mut trial = build.clone();
                trial.set(fix.slot, product(id.as_str())).unwrap();
                let after = check_build(&trial);
                let context = format!("{:?} in {}: {id}", fix.rule, fix.slot.category.label());
                assert!(
                    count(&after, fix.rule) < count(&report, fix.rule),
                    "{context}"
                );
                for issue in after.issues.iter().filter(|i| i.rule != fix.rule) {
                    if count(&after, issue.rule) > count(&report, issue.rule) {
                        assert!(
                            issue.severity == Severity::Note
                                || (issue.severity == Severity::Warning
                                    && fix.rule.severity() == Severity::Error),
                            "{context} brings {:?}: {}",
                            issue.rule,
                            issue.message
                        );
                    }
                }
            }
        }
    }
}
