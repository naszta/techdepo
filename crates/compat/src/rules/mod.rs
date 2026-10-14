//! The v1 rules, one small function each.
//!
//! A rule reads the parts it needs from the build with `?`: if one is missing the rule returns
//! `None` (not applicable, so not listed in `Report::checked_rules`). Otherwise it pushes zero
//! or more issues and returns [`CHECKED`].

mod case;
mod cooling;
mod graphics;
mod memory;
mod platform;
mod power;
mod storage;

use std::ops::Deref;

use crate::build::{Build, Part};
use crate::power::PowerEstimate;
use crate::report::Issue;
use crate::rule::RuleId;

/// `Some(())` when the rule was evaluated, `None` when the build lacks the parts it needs.
pub(crate) type Checked = Option<()>;

/// The rule was evaluated.
pub(crate) const CHECKED: Checked = Some(());

/// What a rule sees: the build plus values computed once for all rules.
pub(crate) struct Ctx<'b, 'a> {
    build: &'b Build<'a>,
    /// Power estimate of the whole build.
    pub power: PowerEstimate,
}

impl<'b, 'a> Ctx<'b, 'a> {
    pub fn new(build: &'b Build<'a>) -> Self {
        Self {
            build,
            power: PowerEstimate::of(build),
        }
    }

    /// CPU and board are both present and share a socket. Rules about how a CPU and a board
    /// work together are moot (and stay quiet) when the socket rule already failed.
    pub fn same_platform(&self) -> bool {
        match (self.cpu(), self.motherboard()) {
            (Some(cpu), Some(board)) => cpu.spec.socket == board.spec.socket,
            _ => false,
        }
    }
}

impl<'a> Deref for Ctx<'_, 'a> {
    type Target = Build<'a>;

    fn deref(&self) -> &Self::Target {
        self.build
    }
}

type RuleFn = fn(&Ctx<'_, '_>, &mut Vec<Issue>) -> Checked;

/// The function implementing `rule`. The match is exhaustive, so every rule has one.
fn rule_fn(rule: RuleId) -> RuleFn {
    match rule {
        RuleId::CpuSocket => platform::cpu_socket,
        RuleId::CpuSupport => platform::cpu_support,
        RuleId::BiosUpdate => platform::bios_update,
        RuleId::MemoryType => memory::memory_type,
        RuleId::CpuMemoryType => memory::cpu_memory_type,
        RuleId::MemorySlots => memory::memory_slots,
        RuleId::MemoryCapacity => memory::memory_capacity,
        RuleId::MemorySpeed => memory::memory_speed,
        RuleId::MemoryMixedKits => memory::memory_mixed_kits,
        RuleId::CaseFormFactor => case::case_form_factor,
        RuleId::GpuLength => case::gpu_length,
        RuleId::GpuThickness => case::gpu_thickness,
        RuleId::CoolerHeight => case::cooler_height,
        RuleId::RadiatorMount => case::radiator_mount,
        RuleId::PsuFormFactor => case::psu_form_factor,
        RuleId::PsuLength => case::psu_length,
        RuleId::PsuBracket => case::psu_bracket,
        RuleId::DriveBays => case::drive_bays,
        RuleId::CoolerSocket => cooling::cooler_socket,
        RuleId::CoolerTdp => cooling::cooler_tdp,
        RuleId::RamClearance => cooling::ram_clearance,
        RuleId::NoCooler => cooling::no_cooler,
        RuleId::PsuWattage => power::psu_wattage,
        RuleId::PsuHeadroom => power::psu_headroom,
        RuleId::GpuPowerConnectors => power::gpu_power_connectors,
        RuleId::CpuPowerConnectors => power::cpu_power_connectors,
        RuleId::SataPower => power::sata_power,
        RuleId::GpuRecommendedPsu => power::gpu_recommended_psu,
        RuleId::M2Slots => storage::m2_slots,
        RuleId::M2Speed => storage::m2_speed,
        RuleId::SataPorts => storage::sata_ports,
        RuleId::NoVideoOutput => graphics::no_video_output,
        RuleId::GpuPcieGen => graphics::gpu_pcie_gen,
    }
}

/// Runs `rule` against `cx`, appending its issues; `false` if it was not applicable.
pub(crate) fn run(rule: RuleId, cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> bool {
    let before = out.len();
    let checked = rule_fn(rule)(cx, out).is_some();
    debug_assert!(
        checked || out.len() == before,
        "{rule:?} reported issues but claimed not to apply"
    );
    debug_assert!(out[before..].iter().all(|i| i.rule == rule));
    checked
}

/// "a", "a and b", "a, b and c".
pub(crate) fn join_and<T: std::fmt::Display>(items: impl IntoIterator<Item = T>) -> String {
    let items: Vec<String> = items.into_iter().map(|i| i.to_string()).collect();
    match items.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// Part names joined with "and"; repeated parts are counted: "2× Nimbus DDR5 and Arcwave A4".
pub(crate) fn names<'a, S: 'a>(parts: impl IntoIterator<Item = &'a Part<'a, S>>) -> String {
    let mut counted: Vec<(&str, usize)> = Vec::new();
    for part in parts {
        match counted.iter_mut().find(|(name, _)| *name == part.name()) {
            Some((_, n)) => *n += 1,
            None => counted.push((part.name(), 1)),
        }
    }
    join_and(counted.into_iter().map(|(name, n)| {
        if n == 1 {
            name.to_owned()
        } else {
            format!("{n}× {name}")
        }
    }))
}

/// Parts grouped by product, in first-seen order: two copies of a kit are one group of two.
pub(crate) fn distinct<'p, 'a, S>(parts: &'p [Part<'a, S>]) -> Vec<Vec<&'p Part<'a, S>>> {
    let mut groups: Vec<Vec<&Part<'a, S>>> = Vec::new();
    for part in parts {
        match groups.iter_mut().find(|g| g[0].id() == part.id()) {
            Some(group) => group.push(part),
            None => groups.push(vec![part]),
        }
    }
    groups
}

/// "a", "a or b", "a, b or c".
pub(crate) fn join_or<T: std::fmt::Display>(items: impl IntoIterator<Item = T>) -> String {
    let items: Vec<String> = items.into_iter().map(|i| i.to_string()).collect();
    match items.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} or {last}", init.join(", ")),
    }
}

/// `1 slot`, `2 slots`.
pub(crate) fn count(n: impl Into<u64>, singular: &str, plural: &str) -> String {
    let n = n.into();
    format!("{n} {}", if n == 1 { singular } else { plural })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_lists() {
        assert_eq!(join_and(["a"]), "a");
        assert_eq!(join_and(["a", "b"]), "a and b");
        assert_eq!(join_or(["a", "b", "c"]), "a, b or c");
        assert_eq!(count(1u8, "slot", "slots"), "1 slot");
        assert_eq!(count(0u8, "slot", "slots"), "0 slots");
        let (a, b) = (
            crate::fixtures::memory_product("a", |_| {}),
            crate::fixtures::memory_product("b", |_| {}),
        );
        let build = Build::from_products([&a, &b, &a]).unwrap();
        assert_eq!(names(build.memory()), "2× a and b");
    }
}
