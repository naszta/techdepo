//! CPU ↔ motherboard: socket, family support, BIOS version.

use super::{CHECKED, Checked, Ctx, join_and, join_or};
use crate::report::Issue;
use crate::rule::RuleId;

pub(super) fn cpu_socket(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (cpu, board) = (cx.cpu()?, cx.motherboard()?);
    if cpu.spec.socket != board.spec.socket {
        out.push(Issue::new(
            RuleId::CpuSocket,
            [cpu.id(), board.id()],
            format!(
                "{cpu} uses the {} socket, but {board} has an {} socket. The CPU physically \
                 won't fit the board.",
                cpu.spec.socket, board.spec.socket,
            ),
        ));
    }
    CHECKED
}

pub(super) fn cpu_support(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (cpu, board) = (cx.cpu()?, cx.motherboard()?);
    if cx.same_platform() && board.spec.min_bios_for(cpu.spec.family).is_none() {
        let supported = board.spec.cpu_support.iter().map(|s| s.family);
        out.push(Issue::new(
            RuleId::CpuSupport,
            [cpu.id(), board.id()],
            format!(
                "{cpu} is from the {} family. {board} has the right socket but doesn't support \
                 {} CPUs with any BIOS version; it only supports {} CPUs.",
                cpu.spec.family,
                cpu.spec.family,
                join_and(supported),
            ),
        ));
    }
    CHECKED
}

pub(super) fn bios_update(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (cpu, board) = (cx.cpu()?, cx.motherboard()?);
    let Some(min_bios) = board.spec.min_bios_for(cpu.spec.family) else {
        return CHECKED;
    };
    if !cx.same_platform() || min_bios <= board.spec.bios_version {
        return CHECKED;
    }
    let shipped = board.spec.bios_version;
    let how = if board.spec.bios_flashback {
        "The board has BIOS Flashback, so you can update it from a USB stick before installing \
         the CPU."
            .to_owned()
    } else {
        let bootable: Vec<_> = board
            .spec
            .cpu_support
            .iter()
            .filter(|s| s.min_bios <= shipped)
            .map(|s| s.family)
            .collect();
        let with = if bootable.is_empty() {
            String::new()
        } else {
            format!(" ({} family)", join_or(bootable))
        };
        format!(
            "The board has no BIOS Flashback, so it won't boot with this CPU until the BIOS is \
             updated using a CPU it already supports{with}."
        )
    };
    out.push(Issue::new(
        RuleId::BiosUpdate,
        [board.id(), cpu.id()],
        format!(
            "{board} ships with BIOS {shipped}, but {} CPUs like {cpu} need BIOS {min_bios} or \
             newer. {how}",
            cpu.spec.family,
        ),
    ));
    CHECKED
}

#[cfg(test)]
mod tests {
    use techdepo_catalog::spec::CpuSupport;
    use techdepo_catalog::{Chipset, CpuFamily, MemoryType, Socket};

    use crate::fixtures::*;
    use crate::rule::RuleId;

    #[test]
    fn socket_mismatch_is_an_error_and_silences_follow_ups() {
        let cpu = cpu_product("am4-cpu", |s| {
            s.socket = Socket::Am4;
            s.family = CpuFamily::Granite;
            s.memory[0].memory_type = MemoryType::Ddr4;
            s.memory[0].max_speed_mts = 3200;
        });
        let board = board_product("am5-board", |_| {});
        let report = check(&[&cpu, &board]);
        let issue = only_issue(&report, RuleId::CpuSocket);
        assert_eq!(issue.parts, [cpu.id.clone(), board.id.clone()]);
        assert!(issue.message.contains("AM4") && issue.message.contains("AM5"));
        // Family support, BIOS and memory-controller rules don't pile on.
        assert_eq!(fired(&report), [RuleId::CpuSocket]);
    }

    #[test]
    fn matching_socket_passes() {
        let report = check(&[&cpu_product("c", |_| {}), &board_product("b", |_| {})]);
        assert!(report.issues.is_empty(), "{:#?}", report.issues);
        assert!(report.checked(RuleId::CpuSocket));
    }

    #[test]
    fn unlisted_family_is_unsupported() {
        let cpu = cpu_product("c", |_| {});
        let board = board_product("b", |s| {
            s.chipset = Chipset::A620;
            s.cpu_support = vec![CpuSupport {
                family: CpuFamily::Quartz,
                min_bios: 1,
            }];
        });
        let report = check(&[&cpu, &board]);
        let issue = only_issue(&report, RuleId::CpuSupport);
        assert!(issue.message.contains("Obsidian") && issue.message.contains("Quartz"));
        assert_eq!(fired(&report), [RuleId::CpuSupport]);
    }

    #[test]
    fn bios_update_with_flashback() {
        let cpu = cpu_product("c", |_| {});
        let board = board_product("b", |s| {
            s.bios_version = 10;
            s.cpu_support[1].min_bios = 16;
        });
        let report = check(&[&cpu, &board]);
        let issue = only_issue(&report, RuleId::BiosUpdate);
        assert!(issue.message.contains("BIOS 10") && issue.message.contains("BIOS 16"));
        assert!(issue.message.contains("Flashback, so you can update"));
        assert_eq!(report.verdict, crate::Verdict::Warnings);
    }

    #[test]
    fn bios_update_without_flashback_names_a_bootable_family() {
        let cpu = cpu_product("c", |_| {});
        let board = board_product("b", |s| {
            s.bios_version = 10;
            s.bios_flashback = false;
            s.cpu_support[1].min_bios = 16;
        });
        let issue = only_issue(&check(&[&cpu, &board]), RuleId::BiosUpdate).clone();
        assert!(
            issue.message.contains("no BIOS Flashback"),
            "{}",
            issue.message
        );
        assert!(
            issue.message.contains("(Quartz family)"),
            "{}",
            issue.message
        );
    }

    #[test]
    fn bios_at_exactly_the_minimum_is_fine() {
        let cpu = cpu_product("c", |_| {});
        let board = board_product("b", |s| {
            s.bios_version = 16;
            s.cpu_support[1].min_bios = 16;
        });
        assert!(check(&[&cpu, &board]).issues.is_empty());
    }
}
