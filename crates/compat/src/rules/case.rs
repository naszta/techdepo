//! What has to fit inside the case: board, graphics card, cooler, radiator, PSU, drives.

use techdepo_catalog::spec::{CoolerKind, PsuFit, StorageFormFactor};

use super::cooling::fan_raise;
use super::{CHECKED, Checked, Ctx, count, join_and, join_or, names};
use crate::report::Issue;
use crate::rule::RuleId;

pub(super) fn case_form_factor(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (case, board) = (cx.case()?, cx.motherboard()?);
    let form_factor = board.spec.form_factor;
    if !case.spec.motherboard_form_factors.contains(&form_factor) {
        out.push(Issue::new(
            RuleId::CaseFormFactor,
            [board.id(), case.id()],
            format!(
                "{board} is {form_factor}, but {case} only fits {} boards.",
                join_or(&case.spec.motherboard_form_factors),
            ),
        ));
    }
    CHECKED
}

pub(super) fn gpu_length(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (gpu, case) = (cx.gpu()?, cx.case()?);
    let (length, max) = (gpu.spec.length_mm, case.spec.max_gpu_length_mm);
    if length > max {
        out.push(Issue::new(
            RuleId::GpuLength,
            [gpu.id(), case.id()],
            format!(
                "{gpu} is {length} mm long, but {case} fits graphics cards up to {max} mm \
                 ({} mm too long).",
                length - max,
            ),
        ));
    }
    CHECKED
}

pub(super) fn gpu_thickness(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (gpu, case) = (cx.gpu()?, cx.case()?);
    let (width, max) = (gpu.spec.slot_width, case.spec.max_gpu_slots);
    if width > f32::from(max) {
        out.push(Issue::new(
            RuleId::GpuThickness,
            [gpu.id(), case.id()],
            format!(
                "{gpu} is {width}-slot thick, but {case} takes graphics cards up to {} thick.",
                count(max, "slot", "slots"),
            ),
        ));
    }
    CHECKED
}

/// Tall memory under the cooler's fan raises the fan, and with it the cooler (see
/// `cooling::ram_clearance`); the raised height is what has to fit.
pub(super) fn cooler_height(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (cooler, case) = (cx.cooler()?, cx.case()?);
    let max = case.spec.max_cooler_height_mm;
    let CoolerKind::Air { height_mm, .. } = cooler.spec.kind else {
        return CHECKED;
    };
    let raised = fan_raise(cx);
    let total = height_mm + raised.map_or(0, |(_, raise)| raise);
    if total > max {
        let (parts, tall) = match raised {
            Some((kit, _)) if height_mm <= max => (
                vec![cooler.id(), case.id(), kit.id()],
                format!("{total} mm tall with its fan raised for {kit}"),
            ),
            _ => (vec![cooler.id(), case.id()], format!("{height_mm} mm tall")),
        };
        out.push(Issue::new(
            RuleId::CoolerHeight,
            parts,
            format!(
                "{cooler} is {tall}, but {case} has room for air coolers up to {max} mm, so the \
                 side panel won't close.",
            ),
        ));
    }
    CHECKED
}

pub(super) fn radiator_mount(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (cooler, case) = (cx.cooler()?, cx.case()?);
    if let CoolerKind::Aio { radiator_mm } = cooler.spec.kind
        && !case.spec.fits_radiator(radiator_mm)
    {
        let mounts = &case.spec.radiator_mounts;
        let options = if mounts.iter().all(|m| m.sizes_mm.is_empty()) {
            "it has no radiator mounts".to_owned()
        } else {
            let positions = mounts.iter().filter(|m| !m.sizes_mm.is_empty()).map(|m| {
                let sizes: Vec<String> = m.sizes_mm.iter().map(u16::to_string).collect();
                format!("{} {} mm", m.position.as_str(), sizes.join("/"))
            });
            format!("it takes {}", join_and(positions))
        };
        out.push(Issue::new(
            RuleId::RadiatorMount,
            [cooler.id(), case.id()],
            format!("{cooler} has a {radiator_mm} mm radiator, but {case} has no mount for it: {options}."),
        ));
    }
    CHECKED
}

pub(super) fn psu_form_factor(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (psu, case) = (cx.psu()?, cx.case()?);
    let form_factor = psu.spec.form_factor;
    if case.spec.psu_fit(form_factor) == PsuFit::No {
        out.push(Issue::new(
            RuleId::PsuFormFactor,
            [psu.id(), case.id()],
            format!(
                "{psu} is {form_factor}, but {case} only takes {} power supplies.",
                join_or(&case.spec.psu_form_factors),
            ),
        ));
    }
    CHECKED
}

/// An SFX unit goes into an ATX bay with an SFX-to-ATX bracket. Cases that include one list
/// both form factors, so here the bracket is an extra part to get.
pub(super) fn psu_bracket(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (psu, case) = (cx.psu()?, cx.case()?);
    if case.spec.psu_fit(psu.spec.form_factor) == PsuFit::Bracket {
        out.push(Issue::new(
            RuleId::PsuBracket,
            [psu.id(), case.id()],
            format!(
                "{psu} is {}, and {case} has a bay for {} power supplies. It fits with an \
                 SFX-to-ATX bracket, which comes with neither part.",
                psu.spec.form_factor,
                join_or(&case.spec.psu_form_factors),
            ),
        ));
    }
    CHECKED
}

/// Only meaningful once the form factor fits; otherwise `psu_form_factor` already failed.
pub(super) fn psu_length(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (psu, case) = (cx.psu()?, cx.case()?);
    let (length, max) = (psu.spec.length_mm, case.spec.max_psu_length_mm);
    if case.spec.psu_fit(psu.spec.form_factor) != PsuFit::No && length > max {
        out.push(Issue::new(
            RuleId::PsuLength,
            [psu.id(), case.id()],
            format!("{psu} is {length} mm long, but {case} fits power supplies up to {max} mm."),
        ));
    }
    CHECKED
}

/// 3.5" drives need 3.5" bays; 2.5" drives use 2.5" bays or whatever 3.5" bays are left.
pub(super) fn drive_bays(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let case = cx.case()?;
    if cx.storage().is_empty() {
        return None;
    }
    let of = |form_factor| {
        cx.storage()
            .iter()
            .filter(move |d| d.spec.form_factor == form_factor)
    };
    let (n35, n25) = (
        of(StorageFormFactor::Sata35).count(),
        of(StorageFormFactor::Sata25).count(),
    );
    let (b35, b25) = (
        usize::from(case.spec.bays_3_5),
        usize::from(case.spec.bays_2_5),
    );
    let fits = n35 <= b35 && n25 <= b25 + (b35 - n35);
    if !fits {
        let drives: Vec<_> = of(StorageFormFactor::Sata35)
            .chain(of(StorageFormFactor::Sata25))
            .collect();
        let has = |n: usize, one: &str, many: &str| match n {
            0 => None,
            n => Some(count(n as u64, one, many)),
        };
        let needed = [
            has(n35, "3.5\" drive", "3.5\" drives"),
            has(n25, "2.5\" drive", "2.5\" drives"),
        ];
        let bays = [
            has(b35, "3.5\" bay", "3.5\" bays"),
            has(b25, "2.5\" bay", "2.5\" bays"),
        ];
        let bays = match join_and(bays.into_iter().flatten()) {
            none if none.is_empty() => "no drive bays".to_owned(),
            some => format!("only {some}"),
        };
        let shared = if n25 > 0 && b35 > 0 {
            " (a 3.5\" bay also takes a 2.5\" drive)"
        } else {
            ""
        };
        out.push(Issue::new(
            RuleId::DriveBays,
            std::iter::once(case.id()).chain(drives.iter().map(|d| d.id())),
            format!(
                "The build has {} ({}), but {case} has {bays}{shared}.",
                join_and(needed.into_iter().flatten()),
                names(drives.iter().copied()),
            ),
        ));
    }
    CHECKED
}

#[cfg(test)]
mod tests {
    use techdepo_catalog::spec::{CoolerKind, StorageFormFactor, StorageInterface};
    use techdepo_catalog::{FormFactor, PsuFormFactor};

    use crate::fixtures::*;
    use crate::rule::RuleId;

    #[test]
    fn board_must_fit_case() {
        let board = board_product("b", |s| s.form_factor = FormFactor::EAtx);
        let case = case_product("c", |_| {});
        let issue = only_issue(&check(&[&board, &case]), RuleId::CaseFormFactor).clone();
        assert!(issue.message.contains("E-ATX"), "{}", issue.message);
        assert!(
            issue.message.contains("ATX, Micro-ATX or Mini-ITX"),
            "{}",
            issue.message
        );
        let itx = board_product("b", |s| s.form_factor = FormFactor::MiniItx);
        assert!(check(&[&itx, &case]).issues.is_empty());
    }

    #[test]
    fn gpu_length_boundary() {
        let case = case_product("c", |s| s.max_gpu_length_mm = 300);
        let exact = gpu_product("g", |s| s.length_mm = 300);
        assert!(check(&[&exact, &case]).issues.is_empty());
        let long = gpu_product("g", |s| s.length_mm = 336);
        let issue = only_issue(&check(&[&long, &case]), RuleId::GpuLength).clone();
        assert!(issue.message.contains("336 mm") && issue.message.contains("36 mm too long"));
    }

    #[test]
    fn gpu_thickness_against_case_slots() {
        let case = case_product("c", |s| s.max_gpu_slots = 2);
        let two = gpu_product("g", |s| s.slot_width = 2.0);
        assert!(check(&[&two, &case]).issues.is_empty());
        let thick = gpu_product("g", |s| s.slot_width = 2.5);
        let issue = only_issue(&check(&[&thick, &case]), RuleId::GpuThickness).clone();
        assert_eq!(issue.severity, crate::Severity::Error);
        assert!(
            issue
                .message
                .contains("g is 2.5-slot thick, but c takes graphics cards up to 2 slots thick."),
            "{}",
            issue.message
        );
    }

    #[test]
    fn raised_fan_counts_towards_cooler_height() {
        let case = case_product("c", |s| s.max_cooler_height_mm = 160);
        let cooler = cooler_product("k", |s| {
            s.kind = CoolerKind::Air {
                height_mm: 155,
                ram_clearance_mm: Some(42),
            }
        });
        let low = memory_product("low", |s| s.height_mm = 42);
        assert!(check(&[&cooler, &case, &low]).issues.is_empty());
        let tall = memory_product("tall", |s| s.height_mm = 52);
        let report = check(&[&cooler, &case, &tall]);
        assert_eq!(fired(&report), [RuleId::CoolerHeight, RuleId::RamClearance]);
        let issue = only_issue(&report, RuleId::CoolerHeight);
        assert_eq!(
            issue.parts,
            [cooler.id.clone(), case.id.clone(), tall.id.clone()]
        );
        assert!(
            issue
                .message
                .contains("k is 165 mm tall with its fan raised for tall, but c has room"),
            "{}",
            issue.message
        );
        // Too tall anyway: the memory is beside the point.
        let short_case = case_product("c", |s| s.max_cooler_height_mm = 150);
        let issue =
            only_issue(&check(&[&cooler, &short_case, &tall]), RuleId::CoolerHeight).clone();
        assert_eq!(issue.parts, [cooler.id.clone(), short_case.id.clone()]);
        assert!(
            issue.message.contains("k is 155 mm tall, but"),
            "{}",
            issue.message
        );
    }

    #[test]
    fn air_cooler_height() {
        let case = case_product("c", |s| s.max_cooler_height_mm = 70);
        let tall = cooler_product("k", |s| {
            s.kind = CoolerKind::Air {
                height_mm: 155,
                ram_clearance_mm: None,
            }
        });
        let issue = only_issue(&check(&[&tall, &case]), RuleId::CoolerHeight).clone();
        assert!(issue.message.contains("155 mm") && issue.message.contains("70 mm"));
        let low = cooler_product("k", |s| {
            s.kind = CoolerKind::Air {
                height_mm: 70,
                ram_clearance_mm: None,
            }
        });
        assert!(check(&[&low, &case]).issues.is_empty());
        // An AIO's height is irrelevant.
        let aio = cooler_product("k", |s| s.kind = CoolerKind::Aio { radiator_mm: 240 });
        assert!(check(&[&aio, &case]).issues.is_empty());
    }

    #[test]
    fn radiator_needs_a_mount() {
        let case = case_product("c", |_| {});
        let big = cooler_product("k", |s| s.kind = CoolerKind::Aio { radiator_mm: 420 });
        let issue = only_issue(&check(&[&big, &case]), RuleId::RadiatorMount).clone();
        assert!(
            issue
                .message
                .contains("front 240/280/360 mm and top 240 mm"),
            "{}",
            issue.message
        );
        let bare = case_product("c", |s| s.radiator_mounts.clear());
        let small = cooler_product("k", |s| s.kind = CoolerKind::Aio { radiator_mm: 120 });
        let issue = only_issue(&check(&[&small, &bare]), RuleId::RadiatorMount).clone();
        assert!(issue.message.contains("no radiator mounts"));
        let ok = cooler_product("k", |s| s.kind = CoolerKind::Aio { radiator_mm: 360 });
        assert!(check(&[&ok, &case]).issues.is_empty());
    }

    #[test]
    fn psu_form_factor_then_length() {
        let case = case_product("c", |_| {});
        let sfx_case = case_product("c", |s| s.psu_form_factors = vec![PsuFormFactor::Sfx]);
        let atx = psu_product("p", |_| {});
        let report = check(&[&atx, &sfx_case]);
        assert_eq!(fired(&report), [RuleId::PsuFormFactor]);
        assert!(
            report.issues[0]
                .message
                .contains("p is ATX, but c only takes SFX"),
            "{}",
            report.issues[0].message
        );
        // Too long, but it doesn't go in at all: one error is enough.
        let long_atx = psu_product("p", |s| s.length_mm = 250);
        assert_eq!(
            fired(&check(&[&long_atx, &sfx_case])),
            [RuleId::PsuFormFactor]
        );
        let long = psu_product("p", |s| s.length_mm = 210);
        assert_eq!(fired(&check(&[&long, &case])), [RuleId::PsuLength]);
        let exact = psu_product("p", |s| s.length_mm = 200);
        assert!(check(&[&exact, &case]).issues.is_empty());
    }

    #[test]
    fn sfx_in_an_atx_bay_needs_a_bracket() {
        let atx_case = case_product("c", |_| {});
        let sfx = psu_product("p", |s| {
            s.form_factor = PsuFormFactor::Sfx;
            s.length_mm = 100;
        });
        let report = check(&[&sfx, &atx_case]);
        let issue = only_issue(&report, RuleId::PsuBracket);
        assert_eq!(issue.severity, crate::Severity::Warning);
        assert!(
            issue.message.contains("SFX-to-ATX bracket"),
            "{}",
            issue.message
        );
        assert_eq!(report.verdict, crate::Verdict::Warnings);
        // A case that ships with the bracket lists both form factors.
        let both = case_product("c", |s| {
            s.psu_form_factors = vec![PsuFormFactor::Atx, PsuFormFactor::Sfx]
        });
        assert!(check(&[&sfx, &both]).issues.is_empty());
    }

    #[test]
    fn drive_bays_count_shared_3_5_bays() {
        let hdd = storage_product("hdd", |s| {
            s.form_factor = StorageFormFactor::Sata35;
            s.interface = StorageInterface::Sata;
        });
        let ssd = storage_product("ssd", |s| {
            s.form_factor = StorageFormFactor::Sata25;
            s.interface = StorageInterface::Sata;
        });
        let case = case_product("c", |s| {
            s.bays_3_5 = 1;
            s.bays_2_5 = 1;
        });
        // One 3.5" + one 2.5": fits.
        assert!(check(&[&case, &hdd, &ssd]).issues.is_empty());
        // Two 2.5": one in the 2.5" bay, one in the free 3.5" bay.
        assert!(check(&[&case, &ssd, &ssd]).issues.is_empty());
        // Two 3.5": only one 3.5" bay.
        let issue = only_issue(&check(&[&case, &hdd, &hdd]), RuleId::DriveBays).clone();
        assert!(
            issue.message.contains(
                "has 2 3.5\" drives (2× hdd), but c has only 1 3.5\" bay and 1 2.5\" bay."
            ),
            "{}",
            issue.message
        );
        // 3.5" + two 2.5": the 3.5" bay is taken.
        assert_eq!(
            fired(&check(&[&case, &hdd, &ssd, &ssd])),
            [RuleId::DriveBays]
        );
        let bare = case_product("c", |s| {
            s.bays_3_5 = 0;
            s.bays_2_5 = 0;
        });
        let issue = only_issue(&check(&[&bare, &ssd]), RuleId::DriveBays).clone();
        assert!(
            issue.message.ends_with("has no drive bays."),
            "{}",
            issue.message
        );
    }
}
