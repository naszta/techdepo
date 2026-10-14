//! CPU cooling: mounting socket, capacity, memory under the fan, and a build without any cooler.

use techdepo_catalog::spec::{CoolerKind, MemorySpec};

use super::{CHECKED, Checked, Ctx, distinct, join_and, names};
use crate::build::Part;
use crate::report::Issue;
use crate::rule::RuleId;

/// The mounting kit attaches to the board's socket; without a board, the CPU's socket decides.
pub(super) fn cooler_socket(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let cooler = cx.cooler()?;
    let (socket, target, name) = match (cx.motherboard(), cx.cpu()) {
        (Some(board), _) => (board.spec.socket, board.id(), board.name()),
        (None, Some(cpu)) => (cpu.spec.socket, cpu.id(), cpu.name()),
        (None, None) => return None,
    };
    if !cooler.spec.supports(socket) {
        out.push(Issue::new(
            RuleId::CoolerSocket,
            [cooler.id(), target],
            format!(
                "{cooler}'s mounting kit fits {}, but {name} uses {socket}.",
                join_and(&cooler.spec.sockets),
            ),
        ));
    }
    CHECKED
}

/// The rating is compared with the CPU's sustained peak power, not its TDP: some CPUs sold as
/// 125 W parts draw twice that under all-core load. Quiet when the cooler cannot mount on the
/// CPU at all: `cooler_socket` already says so.
pub(super) fn cooler_tdp(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (cooler, cpu) = (cx.cooler()?, cx.cpu()?);
    let (rating, peak, tdp) = (
        cooler.spec.tdp_rating_w,
        cpu.spec.peak_power_w,
        cpu.spec.tdp_w,
    );
    if rating < peak && cooler.spec.supports(cpu.spec.socket) {
        let tdp_note = if tdp < peak {
            format!(" (its TDP is {tdp} W)")
        } else {
            String::new()
        };
        out.push(Issue::new(
            RuleId::CoolerTdp,
            [cooler.id(), cpu.id()],
            format!(
                "{cooler} is rated for {rating} W, but {cpu} draws up to {peak} W under sustained \
                 load{tdp_note}. It will work, but the CPU will run hot and throttle under long \
                 heavy loads.",
            ),
        ));
    }
    CHECKED
}

/// The tallest memory kit that does not fit under the air cooler's fan, and how far the fan has
/// to be raised for it. `None` when everything fits (or there is nothing to fit).
pub(super) fn fan_raise<'a>(cx: &Ctx<'_, 'a>) -> Option<(Part<'a, MemorySpec>, u16)> {
    let cooler = cx.cooler()?;
    let CoolerKind::Air {
        ram_clearance_mm: Some(clearance),
        ..
    } = cooler.spec.kind
    else {
        return None;
    };
    // The first of the tallest kits, so the result does not depend on duplicates.
    let tallest = cx.memory().iter().rev().max_by_key(|k| k.spec.height_mm)?;
    let raise = tallest.spec.height_mm.saturating_sub(clearance);
    (raise > 0).then_some((*tallest, raise))
}

/// Memory taller than the cooler's clearance means raising the fan, which makes the cooler
/// taller; `cooler_height` checks the raised height against the case.
pub(super) fn ram_clearance(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let cooler = cx.cooler()?;
    if cx.memory().is_empty() {
        return None;
    }
    let CoolerKind::Air {
        height_mm,
        ram_clearance_mm: Some(clearance),
    } = cooler.spec.kind
    else {
        return CHECKED;
    };
    let Some((tallest, raise)) = fan_raise(cx) else {
        return CHECKED;
    };
    let tall: Vec<_> = distinct(cx.memory())
        .into_iter()
        .filter(|group| group[0].spec.height_mm > clearance)
        .collect();
    let kits = names(tall.iter().flatten().copied());
    let verb = match tall.as_slice() {
        [group] if group.len() == 1 => "is",
        [_] => "are",
        _ => "are up to",
    };
    out.push(Issue::new(
        RuleId::RamClearance,
        std::iter::once(cooler.id()).chain(tall.iter().map(|g| g[0].id())),
        format!(
            "{kits} {verb} {} mm tall, but {cooler} leaves {clearance} mm for memory under \
             its fan. The fan has to sit {raise} mm higher, which makes the cooler {} mm tall; \
             memory up to {clearance} mm fits without that.",
            tallest.spec.height_mm,
            height_mm + raise,
        ),
    ));
    CHECKED
}

pub(super) fn no_cooler(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let cpu = cx.cpu()?;
    if cx.cooler().is_none() && !cpu.spec.bundled_cooler {
        out.push(Issue::new(
            RuleId::NoCooler,
            [cpu.id()],
            format!("{cpu} doesn't come with a cooler and the build has none. Add a CPU cooler."),
        ));
    }
    CHECKED
}

#[cfg(test)]
mod tests {
    use techdepo_catalog::Socket;
    use techdepo_catalog::spec::CoolerKind;

    use crate::fixtures::*;
    use crate::rule::{RuleId, Severity};

    #[test]
    fn cooler_socket_uses_board_then_cpu() {
        let cooler = cooler_product("k", |s| s.sockets = vec![Socket::Am4, Socket::Lga1700]);
        let cpu = cpu_product("c", |_| {});
        let board = board_product("b", |_| {});
        let issue = only_issue(&check(&[&cooler, &cpu]), RuleId::CoolerSocket).clone();
        assert_eq!(issue.parts[1], cpu.id);
        assert!(
            issue
                .message
                .contains("fits AM4 and LGA1700, but c uses AM5"),
            "{}",
            issue.message
        );
        let issue = only_issue(&check(&[&cooler, &cpu, &board]), RuleId::CoolerSocket).clone();
        assert_eq!(issue.parts[1], board.id);
        assert!(
            check(&[&cooler_product("k", |_| {}), &board])
                .issues
                .is_empty()
        );
    }

    #[test]
    fn cooler_rating_below_sustained_power_warns() {
        // A "125 W" CPU that draws 253 W: a cooler rated above the TDP is not enough.
        let cpu = cpu_product("c", |s| {
            s.tdp_w = 125;
            s.peak_power_w = 253;
        });
        let weak = cooler_product("k", |s| s.tdp_rating_w = 180);
        let issue = only_issue(&check(&[&weak, &cpu]), RuleId::CoolerTdp).clone();
        assert_eq!(issue.severity, Severity::Warning);
        assert!(
            issue.message.contains(
                "rated for 180 W, but c draws up to 253 W under sustained load (its TDP is 125 W)"
            ),
            "{}",
            issue.message
        );
        let exact = cooler_product("k", |s| s.tdp_rating_w = 253);
        assert!(check(&[&exact, &cpu]).issues.is_empty());
    }

    #[test]
    fn cooler_rating_is_quiet_when_it_does_not_mount() {
        let cpu = cpu_product("c", |s| s.peak_power_w = 170);
        let weak = cooler_product("k", |s| {
            s.tdp_rating_w = 150;
            s.sockets = vec![Socket::Lga1700];
        });
        let report = check(&[&weak, &cpu]);
        assert_eq!(fired(&report), [RuleId::CoolerSocket]);
        assert!(report.checked(RuleId::CoolerTdp));
    }

    fn tower(clearance: Option<u16>) -> techdepo_catalog::Product {
        cooler_product("k", |s| {
            s.kind = CoolerKind::Air {
                height_mm: 155,
                ram_clearance_mm: clearance,
            }
        })
    }

    #[test]
    fn tall_memory_under_the_fan_warns() {
        let cooler = tower(Some(42));
        let low = memory_product("low", |s| s.height_mm = 42);
        let tall = memory_product("tall", |s| s.height_mm = 52);
        assert!(check(&[&cooler, &low]).issues.is_empty());
        let report = check(&[&cooler, &low, &tall, &tall]);
        let issue = only_issue(&report, RuleId::RamClearance);
        assert_eq!(issue.severity, Severity::Warning);
        assert_eq!(issue.parts, [cooler.id.clone(), tall.id.clone()]);
        assert!(
            issue
                .message
                .starts_with("2× tall are 52 mm tall, but k leaves 42 mm"),
            "{}",
            issue.message
        );
        assert!(
            issue
                .message
                .contains("10 mm higher, which makes the cooler 165 mm tall")
        );
        // No overhang, or an AIO: nothing to check against.
        assert!(check(&[&tower(None), &tall]).issues.is_empty());
        let aio = cooler_product("a", |s| s.kind = CoolerKind::Aio { radiator_mm: 240 });
        assert!(check(&[&aio, &tall]).issues.is_empty());
    }

    #[test]
    fn missing_cooler_warns_unless_bundled() {
        let cpu = cpu_product("c", |s| s.bundled_cooler = false);
        assert_eq!(fired(&check(&[&cpu])), [RuleId::NoCooler]);
        let boxed = cpu_product("c", |_| {});
        assert!(check(&[&boxed]).issues.is_empty());
        assert!(
            check(&[&cpu, &cooler_product("k", |_| {})])
                .issues
                .is_empty()
        );
    }
}
