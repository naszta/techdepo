//! Power supply: wattage, headroom, and the cables it provides.

use techdepo_catalog::spec::StorageFormFactor;
use techdepo_domain::ProductId;

use super::{CHECKED, Checked, Ctx, count, join_and, names};
use crate::power::{HEADROOM_WARNING_PCT, HIGH_POWER_CPU_W, MARGIN_PCT};
use crate::report::Issue;
use crate::rule::RuleId;

/// The PSU plus the big consumers: the parts a wattage issue is "about". Other parts also
/// count towards the estimate, but swapping a drive never fixes a wattage problem.
fn wattage_parts<'a>(cx: &Ctx<'_, 'a>) -> Option<Vec<&'a ProductId>> {
    let psu = cx.psu()?;
    let mut parts = vec![psu.id()];
    parts.extend(cx.cpu().map(|c| c.id()));
    parts.extend(cx.gpu().map(|g| g.id()));
    // Nothing that draws power: nothing to compare against.
    (cx.power.components_w > 0).then_some(parts)
}

/// "Voltaris V5 8600 88 W, Halcyon HX 7080 Triple 320 W, other parts 96 W".
fn breakdown(cx: &Ctx<'_, '_>) -> String {
    let mut items = Vec::new();
    let mut rest = cx.power.components_w;
    if let Some(cpu) = cx.cpu() {
        items.push(format!("{cpu} {} W", cpu.spec.peak_power_w));
        rest -= u32::from(cpu.spec.peak_power_w);
    }
    if let Some(gpu) = cx.gpu() {
        items.push(format!("{gpu} {} W", gpu.spec.board_power_w));
        rest -= u32::from(gpu.spec.board_power_w);
    }
    if rest > 0 {
        items.push(format!("other parts {rest} W"));
    }
    format!("{}, plus {MARGIN_PCT} % margin", join_and(items))
}

/// Smallest multiple of 50 W that leaves the recommended headroom over `need`.
fn comfortable_w(need: u32) -> u32 {
    (need * 100)
        .div_ceil(100 - HEADROOM_WARNING_PCT as u32)
        .div_ceil(50)
        * 50
}

/// The estimate includes the safety margin, so the parts alone may still be within the rating:
/// the message says the estimate is over, not that the PSU will fail.
pub(super) fn psu_wattage(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let parts = wattage_parts(cx)?;
    let psu = cx.psu()?;
    let (need, have) = (cx.power.total_w, u32::from(psu.spec.wattage_w));
    if need > have {
        out.push(Issue::new(
            RuleId::PsuWattage,
            parts,
            format!(
                "The build's estimated load is {need} W ({}), above {psu}'s {have} W rating, so \
                 the power supply has no safety margin left for this build. Choose {} W or more, \
                 which also leaves the recommended {HEADROOM_WARNING_PCT} % headroom.",
                breakdown(cx),
                comfortable_w(need),
            ),
        ));
    }
    CHECKED
}

pub(super) fn psu_headroom(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let parts = wattage_parts(cx)?;
    let psu = cx.psu()?;
    let (need, have) = (cx.power.total_w, u32::from(psu.spec.wattage_w));
    let headroom = cx.power.headroom_pct(have);
    if (0..HEADROOM_WARNING_PCT).contains(&headroom) {
        let comfortable = comfortable_w(need);
        out.push(Issue::new(
            RuleId::PsuHeadroom,
            parts,
            format!(
                "{psu} is rated for {have} W and the build needs an estimated {need} W ({}), \
                 leaving {headroom} % headroom. Below {HEADROOM_WARNING_PCT} % the PSU runs near its \
                 limit (louder, hotter, little reserve for load spikes); {comfortable} W or more \
                 would be comfortable.",
                breakdown(cx),
            ),
        ));
    }
    CHECKED
}

/// Adapters (two 8-pin to one 12V-2x6) are deliberately not counted.
pub(super) fn gpu_power_connectors(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (gpu, psu) = (cx.gpu()?, cx.psu()?);
    let (need, have) = (gpu.spec.power_connectors, psu.spec.connectors);
    let shortfalls: Vec<(u8, u8, &str, &str)> = [
        (
            need.twelve_v_2x6,
            have.twelve_v_2x6,
            "12V-2x6 connector",
            "12V-2x6 connectors",
        ),
        (
            need.pcie_8pin,
            have.pcie_8pin,
            "PCIe 8-pin connector",
            "PCIe 8-pin connectors",
        ),
    ]
    .into_iter()
    .filter(|(n, h, ..)| n > h)
    .collect();
    if !shortfalls.is_empty() {
        let clauses: Vec<String> = shortfalls
            .iter()
            .map(|&(n, h, one, many)| {
                let needs = if n == 1 {
                    format!("a {one}")
                } else {
                    format!("{n} {many}")
                };
                let has = if h == 0 {
                    "none".to_owned()
                } else {
                    format!("only {h}")
                };
                format!("{needs}, but {psu} has {has}")
            })
            .collect();
        out.push(Issue::new(
            RuleId::GpuPowerConnectors,
            [gpu.id(), psu.id()],
            format!(
                "{gpu} needs {}. Adapters are not counted: a native cable is the safe way to \
                 feed a graphics card.",
                clauses.join("; it also needs "),
            ),
        ));
    }
    CHECKED
}

/// The board's primary 8-pin is mandatory and every PSU has at least one EPS cable; the extra
/// 8-pin headers only matter for CPUs drawing more than [`HIGH_POWER_CPU_W`]. The optional 4-pin
/// header is not checked.
pub(super) fn cpu_power_connectors(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (cpu, board, psu) = (cx.cpu()?, cx.motherboard()?, cx.psu()?);
    let (headers, cables) = (board.spec.eps_8pin, psu.spec.connectors.eps_8pin);
    let peak = cpu.spec.peak_power_w;
    if cables < headers && u32::from(peak) > HIGH_POWER_CPU_W && cx.same_platform() {
        out.push(Issue::new(
            RuleId::CpuPowerConnectors,
            [board.id(), psu.id(), cpu.id()],
            format!(
                "{board} has {}, but {psu} has {}. The board runs with one connected, but {cpu} \
                 draws up to {peak} W and may be held back under sustained heavy load without \
                 the second.",
                count(headers, "8-pin CPU power header", "8-pin CPU power headers"),
                count(cables, "CPU (EPS) cable", "CPU (EPS) cables"),
            ),
        ));
    }
    CHECKED
}

pub(super) fn sata_power(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let psu = cx.psu()?;
    if cx.storage().is_empty() {
        return None;
    }
    let drives: Vec<_> = cx
        .storage()
        .iter()
        .filter(|d| d.spec.form_factor != StorageFormFactor::M2_2280)
        .collect();
    let have = usize::from(psu.spec.connectors.sata);
    if drives.len() > have {
        out.push(Issue::new(
            RuleId::SataPower,
            std::iter::once(psu.id()).chain(drives.iter().map(|d| d.id())),
            format!(
                "The build has {} that need SATA power ({}), but {psu} has only {}.",
                count(drives.len() as u64, "drive", "drives"),
                names(drives.iter().copied()),
                count(have as u64, "SATA power connector", "SATA power connectors"),
            ),
        ));
    }
    CHECKED
}

pub(super) fn gpu_recommended_psu(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (gpu, psu) = (cx.gpu()?, cx.psu()?);
    let (recommended, have) = (gpu.spec.recommended_psu_w, psu.spec.wattage_w);
    // When the estimate itself does not fit, `psu_wattage` says so with better numbers.
    if have < recommended && cx.power.total_w <= u32::from(have) {
        out.push(Issue::new(
            RuleId::GpuRecommendedPsu,
            [gpu.id(), psu.id()],
            format!(
                "{gpu}'s maker recommends at least {recommended} W and {psu} has {have} W. \
                 Our estimate for this build ({} W) fits, so this is only a heads-up: the \
                 recommendation assumes a power-hungry CPU.",
                cx.power.total_w,
            ),
        ));
    }
    CHECKED
}

#[cfg(test)]
mod tests {
    use techdepo_catalog::spec::{GpuPowerConnectors, StorageFormFactor, StorageInterface};

    use crate::fixtures::*;
    use crate::power::with_margin;
    use crate::rule::{RuleId, Severity};

    #[test]
    fn wattage_below_estimate_is_an_error() {
        let cpu = cpu_product("c", |s| s.peak_power_w = 250);
        let gpu = gpu_product("g", |s| {
            s.board_power_w = 450;
            s.power_connectors = GpuPowerConnectors {
                pcie_8pin: 0,
                twelve_v_2x6: 1,
            };
            s.recommended_psu_w = 1000;
        });
        let psu = psu_product("p", |s| s.wattage_w = 750);
        let report = check(&[&cpu, &gpu, &psu]);
        assert_eq!(report.estimated_watts, with_margin(700));
        assert_eq!(report.psu_watts, Some(750));
        let issue = only_issue(&report, RuleId::PsuWattage);
        assert_eq!(
            issue.parts,
            [psu.id.clone(), cpu.id.clone(), gpu.id.clone()]
        );
        assert!(issue.message.contains("770 W") && issue.message.contains("750 W"));
        assert!(
            issue
                .message
                .contains("c 250 W and g 450 W, plus 10 % margin"),
            "{}",
            issue.message
        );
        // No headroom warning or recommendation note on top of the error.
        assert_eq!(fired(&report), [RuleId::PsuWattage]);
    }

    #[test]
    fn low_headroom_warns_and_suggests_a_size() {
        let gpu = gpu_product("g", |s| s.board_power_w = 450);
        let psu = psu_product("p", |s| s.wattage_w = 550);
        // 450 W + 10 % = 495 W of 550 W: 10 % headroom.
        let report = check(&[&gpu, &psu]);
        assert_eq!(report.headroom_pct, Some(10));
        let issue = only_issue(&report, RuleId::PsuHeadroom);
        assert_eq!(issue.severity, Severity::Warning);
        // 495 / 0.8 = 618.75, rounded up to 650.
        assert!(issue.message.contains("650 W or more"), "{}", issue.message);
        let roomy = psu_product("p", |s| s.wattage_w = 650);
        assert!(!fired(&check(&[&gpu, &roomy])).contains(&RuleId::PsuHeadroom));
    }

    #[test]
    fn psu_rules_need_a_consumer() {
        let report = check(&[&psu_product("p", |_| {})]);
        assert!(!report.checked(RuleId::PsuWattage));
        assert_eq!(report.headroom_pct, Some(100));
    }

    #[test]
    fn gpu_connectors_must_be_native() {
        let gpu = gpu_product("g", |s| {
            s.power_connectors = GpuPowerConnectors {
                pcie_8pin: 0,
                twelve_v_2x6: 1,
            }
        });
        let psu = psu_product("p", |s| s.connectors.twelve_v_2x6 = 0);
        let issue = only_issue(&check(&[&gpu, &psu]), RuleId::GpuPowerConnectors).clone();
        assert!(
            issue
                .message
                .contains("needs a 12V-2x6 connector, but p has none"),
            "{}",
            issue.message
        );

        let three = gpu_product("g", |s| s.power_connectors.pcie_8pin = 3);
        let two = psu_product("p", |s| s.connectors.pcie_8pin = 2);
        let issue = only_issue(&check(&[&three, &two]), RuleId::GpuPowerConnectors).clone();
        assert!(
            issue
                .message
                .contains("needs 3 PCIe 8-pin connectors, but p has only 2"),
            "{}",
            issue.message
        );
        assert!(check(&[&gpu, &psu_product("p", |_| {})]).issues.is_empty());
    }

    #[test]
    fn missing_eps_cable_warns_for_a_high_power_cpu() {
        let board = board_product("b", |s| s.eps_8pin = 2);
        let psu = psu_product("p", |s| s.connectors.eps_8pin = 1);
        let hot = cpu_product("c", |s| s.peak_power_w = 230);
        let issue = only_issue(&check(&[&hot, &board, &psu]), RuleId::CpuPowerConnectors).clone();
        assert_eq!(issue.severity, Severity::Warning);
        assert_eq!(issue.parts[2], hot.id);
        assert!(issue.message.contains("230 W"), "{}", issue.message);
        // An efficient CPU, or no CPU yet: the second header is irrelevant.
        let cool = cpu_product("c", |s| s.peak_power_w = 88);
        assert!(check(&[&cool, &board, &psu]).issues.is_empty());
        let report = check(&[&board, &psu]);
        assert!(report.issues.is_empty() && !report.checked(RuleId::CpuPowerConnectors));
        // A missing cable for the optional 4-pin header is fine.
        let four = board_product("b", |s| s.eps_4pin = 1);
        assert!(check(&[&hot, &four, &psu]).issues.is_empty());
    }

    #[test]
    fn sata_power_counts_bay_drives() {
        let ssd = storage_product("s", |s| {
            s.form_factor = StorageFormFactor::Sata25;
            s.interface = StorageInterface::Sata;
        });
        let psu = psu_product("p", |s| s.connectors.sata = 2);
        let nvme = storage_product("n", |_| {});
        assert!(check(&[&psu, &ssd, &ssd, &nvme]).issues.is_empty());
        assert_eq!(
            fired(&check(&[&psu, &ssd, &ssd, &ssd])),
            [RuleId::SataPower]
        );
    }

    #[test]
    fn below_vendor_recommendation_is_a_note() {
        let gpu = gpu_product("g", |s| s.recommended_psu_w = 850);
        let psu = psu_product("p", |_| {});
        let issue = only_issue(&check(&[&gpu, &psu]), RuleId::GpuRecommendedPsu).clone();
        assert_eq!(issue.severity, Severity::Note);
        assert!(issue.message.contains("850 W") && issue.message.contains("750 W"));
    }
}
