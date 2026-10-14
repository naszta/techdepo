//! Exhaustive pairwise matrix over the embedded catalog.
//!
//! Every pair of products is checked with `check_pair`. Invariants are asserted for each pair,
//! errors are compared against an independent oracle written directly over the spec fields,
//! and the whole matrix is stored as an `insta` snapshot that a person reviews whenever the
//! catalog changes (`INSTA_UPDATE=always cargo test -p techdepo-compat`, or `cargo insta review`).

use std::collections::BTreeSet;
use std::fmt::Write as _;

use techdepo_catalog::spec::{CoolerKind, M2Key, StorageFormFactor, StorageInterface};
use techdepo_catalog::{Catalog, Product, ProductSpec as S, PsuFormFactor as Psu};
use techdepo_compat::power::with_margin;
use techdepo_compat::{Build, Report, RuleId, Slot, check_pair};

/// The errors a pair must produce, restated from the catalog README's rule table without
/// using the engine. Only rules that can fail with two parts appear here.
fn expected_errors(a: &Product, b: &Product) -> BTreeSet<RuleId> {
    use RuleId::*;
    let mut errors = BTreeSet::new();
    let mut expect = |cond: bool, rule| {
        if cond {
            errors.insert(rule);
        }
    };
    for (x, y) in [(a, b), (b, a)] {
        match (&x.spec, &y.spec) {
            (S::Cpu(cpu), S::Motherboard(mb)) => {
                expect(cpu.socket != mb.socket, CpuSocket);
                if cpu.socket == mb.socket {
                    let listed = mb.cpu_support.iter().any(|s| s.family == cpu.family);
                    expect(!listed, CpuSupport);
                    let ram = cpu.memory.iter().any(|m| m.memory_type == mb.memory_type);
                    expect(!ram, CpuMemoryType);
                }
            }
            (S::Cpu(cpu), S::Memory(kit)) => {
                let ram = cpu.memory.iter().any(|m| m.memory_type == kit.memory_type);
                expect(!ram, CpuMemoryType);
            }
            (S::Memory(kit), S::Motherboard(mb)) => {
                expect(kit.memory_type != mb.memory_type, MemoryType);
                expect(kit.modules > mb.memory_slots, MemorySlots);
                let total = u32::from(kit.modules) * u32::from(kit.module_gb);
                expect(total > u32::from(mb.max_memory_gb), MemoryCapacity);
            }
            (S::Memory(k1), S::Memory(k2)) => expect(k1.memory_type != k2.memory_type, MemoryType),
            (S::Motherboard(mb), S::Case(case)) => expect(
                !case.motherboard_form_factors.contains(&mb.form_factor),
                CaseFormFactor,
            ),
            (S::Gpu(gpu), S::Case(case)) => {
                expect(gpu.length_mm > case.max_gpu_length_mm, GpuLength);
                expect(gpu.slot_width > f32::from(case.max_gpu_slots), GpuThickness);
            }
            (S::Cooler(cooler), S::Case(case)) => match cooler.kind {
                CoolerKind::Air { height_mm, .. } => {
                    expect(height_mm > case.max_cooler_height_mm, CoolerHeight);
                }
                CoolerKind::Aio { radiator_mm } => {
                    let mounted = case
                        .radiator_mounts
                        .iter()
                        .any(|m| m.sizes_mm.contains(&radiator_mm));
                    expect(!mounted, RadiatorMount);
                }
            },
            (S::Cooler(cooler), S::Cpu(cpu)) => {
                expect(!cooler.sockets.contains(&cpu.socket), CoolerSocket);
            }
            (S::Cooler(cooler), S::Motherboard(mb)) => {
                expect(!cooler.sockets.contains(&mb.socket), CoolerSocket);
            }
            (S::Psu(psu), S::Case(case)) => {
                // SFX also goes into an ATX bay, with a bracket (a warning).
                let fits = case.psu_form_factors.contains(&psu.form_factor)
                    || psu.form_factor == Psu::Sfx && case.psu_form_factors.contains(&Psu::Atx);
                expect(!fits, PsuFormFactor);
                expect(fits && psu.length_mm > case.max_psu_length_mm, PsuLength);
            }
            (S::Psu(psu), S::Gpu(gpu)) => {
                let (need, have) = (gpu.power_connectors, psu.connectors);
                expect(
                    need.pcie_8pin > have.pcie_8pin || need.twelve_v_2x6 > have.twelve_v_2x6,
                    GpuPowerConnectors,
                );
                let load = with_margin(u32::from(gpu.board_power_w));
                expect(load > u32::from(psu.wattage_w), PsuWattage);
            }
            (S::Psu(psu), S::Cpu(cpu)) => {
                let load = with_margin(u32::from(cpu.peak_power_w));
                expect(load > u32::from(psu.wattage_w), PsuWattage);
            }
            (S::Storage(drive), S::Motherboard(mb)) => {
                if drive.form_factor == StorageFormFactor::M2_2280 {
                    let sata = drive.interface == StorageInterface::Sata;
                    let slot = mb
                        .m2_slots
                        .iter()
                        .any(|s| s.key == M2Key::M && s.max_length_mm >= 80 && (s.sata || !sata));
                    expect(!slot, M2Slots);
                } else {
                    expect(mb.sata_ports == 0, SataPorts);
                }
            }
            (S::Storage(drive), S::Case(case)) => {
                let bays = match drive.form_factor {
                    StorageFormFactor::M2_2280 => u8::MAX,
                    StorageFormFactor::Sata25 => case.bays_2_5 + case.bays_3_5,
                    StorageFormFactor::Sata35 => case.bays_3_5,
                };
                expect(bays == 0, DriveBays);
            }
            (S::Storage(drive), S::Psu(psu)) => expect(
                drive.form_factor != StorageFormFactor::M2_2280 && psu.connectors.sata == 0,
                SataPower,
            ),
            _ => {}
        }
    }
    errors
}

fn assert_invariants(a: &Product, b: &Product, report: &Report) {
    let pair = format!("{} + {}", a.id, b.id);

    let reversed = check_pair(b, a);
    assert_eq!(report, &reversed, "{pair}: not symmetric");

    let shares_a_build =
        Slot::is_multi(a.category) && a.category == b.category || a.category != b.category;
    if !shares_a_build {
        assert!(
            report.issues.is_empty() && report.checked_rules.is_empty(),
            "{pair}"
        );
    }

    for issue in &report.issues {
        assert_eq!(issue.severity, issue.rule.severity(), "{pair}");
        assert!(
            !issue.rule.is_absence(),
            "{pair}: absence rule in a pair check"
        );
        assert!(
            report.checked(issue.rule),
            "{pair}: {:?} not marked checked",
            issue.rule
        );
        assert!(
            !issue.parts.is_empty() && !issue.message.is_empty(),
            "{pair}"
        );
        for part in &issue.parts {
            let product = match part {
                p if p == &a.id => a,
                p if p == &b.id => b,
                p => panic!("{pair}: stray part {p}"),
            };
            assert!(
                issue.rule.categories().contains(&product.category),
                "{pair}: {:?} does not read {:?}",
                issue.rule,
                product.category,
            );
            assert!(
                issue.message.contains(product.name.as_str()),
                "{pair}: {:?} message does not name {}: {}",
                issue.rule,
                product.name,
                issue.message,
            );
        }
    }

    let errors: BTreeSet<RuleId> = report
        .issues
        .iter()
        .filter(|i| i.severity == techdepo_compat::Severity::Error)
        .map(|i| i.rule)
        .collect();
    assert_eq!(
        errors,
        expected_errors(a, b),
        "{pair}: errors differ from the oracle"
    );
}

#[test]
fn pairwise_matrix() {
    let catalog = Catalog::global();
    let products = catalog.all();
    let mut matrix = String::new();
    let mut pairs = 0;
    for (i, a) in products.iter().enumerate() {
        for b in &products[i + 1..] {
            pairs += 1;
            let report = check_pair(a, b);
            assert_invariants(a, b, &report);
            if report.issues.is_empty() {
                continue;
            }
            let issues: Vec<String> = report
                .issues
                .iter()
                .map(|i| format!("{}:{}", i.severity.as_str(), i.rule.as_str()))
                .collect();
            writeln!(matrix, "{} + {}: {}", a.id, b.id, issues.join(" ")).unwrap();
        }
    }
    assert_eq!(pairs, products.len() * (products.len() - 1) / 2);
    insta::assert_snapshot!("pairwise_matrix", matrix);
}

/// Each product on its own, and the empty build, never produce errors.
#[test]
fn single_products_have_no_errors() {
    for product in Catalog::global().all() {
        let build = Build::from_products([product]).unwrap();
        let report = techdepo_compat::check_build(&build);
        assert!(
            report.is_compatible(),
            "{}: {:#?}",
            product.id,
            report.issues
        );
    }
}
