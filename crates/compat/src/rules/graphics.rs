//! Video output and the graphics card's PCIe link.

use super::{CHECKED, Checked, Ctx};
use crate::report::Issue;
use crate::rule::RuleId;

pub(super) fn no_video_output(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let cpu = cx.cpu()?;
    if cx.gpu().is_none() && !cpu.spec.integrated_graphics {
        out.push(Issue::new(
            RuleId::NoVideoOutput,
            [cpu.id()],
            format!(
                "{cpu} has no integrated graphics and the build has no graphics card, so the \
                 monitor would stay dark. Add a graphics card."
            ),
        ));
    }
    CHECKED
}

pub(super) fn gpu_pcie_gen(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let (gpu, board) = (cx.gpu()?, cx.motherboard()?);
    let (card, slot) = (gpu.spec.pcie_gen, board.spec.pcie_x16_gen);
    if card > slot {
        out.push(Issue::new(
            RuleId::GpuPcieGen,
            [gpu.id(), board.id()],
            format!(
                "{gpu} is a {card} card and {board}'s graphics slot is {slot}, so the link runs \
                 at {slot}. It works fine; the difference is a few percent at most."
            ),
        ));
    }
    CHECKED
}

#[cfg(test)]
mod tests {
    use techdepo_catalog::PcieGen;

    use crate::fixtures::*;
    use crate::rule::{RuleId, Severity};

    #[test]
    fn cpu_without_igpu_needs_a_gpu() {
        let cpu = cpu_product("c", |s| s.integrated_graphics = false);
        let issue = only_issue(&check(&[&cpu]), RuleId::NoVideoOutput).clone();
        assert_eq!(issue.severity, Severity::Warning);
        assert!(check(&[&cpu, &gpu_product("g", |_| {})]).issues.is_empty());
    }

    #[test]
    fn faster_card_in_slower_slot_is_a_note() {
        let gpu = gpu_product("g", |s| s.pcie_gen = PcieGen::GEN5);
        let board = board_product("b", |_| {});
        let issue = only_issue(&check(&[&gpu, &board]), RuleId::GpuPcieGen).clone();
        assert_eq!(issue.severity, Severity::Note);
        assert!(issue.message.contains("PCIe 5.0 card") && issue.message.contains("PCIe 4.0"));
        let slow = gpu_product("g", |s| s.pcie_gen = PcieGen::GEN3);
        assert!(check(&[&slow, &board]).issues.is_empty());
    }
}
