//! Drives on the motherboard: M.2 slot assignment, M.2 link speed, SATA ports.

use techdepo_catalog::PcieGen;
use techdepo_catalog::spec::{M2Slot, MotherboardSpec, StorageInterface, StorageSpec};

use super::{CHECKED, Checked, Ctx, count, names};
use crate::build::Part;
use crate::report::Issue;
use crate::rule::RuleId;

pub(super) fn m2_slots(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let board = cx.motherboard()?;
    let drives = m2_drives(cx);
    if drives.is_empty() {
        return None;
    }
    let assignment = assign_m2(&drives, board.spec);
    let unplaced: Vec<_> = drives
        .iter()
        .zip(&assignment)
        .filter_map(|(d, slot)| slot.is_none().then_some(*d))
        .collect();
    if unplaced.is_empty() {
        return CHECKED;
    }

    let slots: Vec<&M2Slot> = board.spec.storage_m2_slots().collect();
    let sata_drives = drives.iter().filter(|d| is_sata(d.spec)).count();
    let sata_slots = slots.iter().filter(|s| s.sata).count();
    let message = if drives.len() > slots.len() {
        format!(
            "The build has {}, but {board} has only {}, so {} {} no slot.",
            count(drives.len() as u64, "M.2 drive", "M.2 drives"),
            count(slots.len() as u64, "M.2 storage slot", "M.2 storage slots"),
            names(&unplaced),
            if unplaced.len() == 1 { "has" } else { "have" },
        )
    } else if sata_drives > sata_slots {
        format!(
            "{} {} M.2 SATA, but {} of {board}'s M.2 slots {} SATA drives.",
            names(drives.iter().filter(|d| is_sata(d.spec))),
            if sata_drives == 1 { "is" } else { "are" },
            if sata_slots == 0 {
                "none".to_owned()
            } else {
                format!("only {sata_slots}")
            },
            if sata_slots == 1 { "accepts" } else { "accept" },
        )
    } else {
        format!(
            "{board}'s M.2 slots can't hold every drive: {} has no free slot that takes its \
             length and interface.",
            names(&unplaced),
        )
    };
    out.push(Issue::new(
        RuleId::M2Slots,
        std::iter::once(board.id()).chain(unplaced.iter().map(|d| d.id())),
        message,
    ));
    CHECKED
}

pub(super) fn m2_speed(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let board = cx.motherboard()?;
    let drives = m2_drives(cx);
    if drives.is_empty() {
        return None;
    }
    let slots: Vec<&M2Slot> = board.spec.storage_m2_slots().collect();
    for (drive, slot) in drives.iter().zip(assign_m2(&drives, board.spec)) {
        let (Some(slot), StorageInterface::Nvme { pcie_gen }) = (slot, drive.spec.interface) else {
            continue;
        };
        let slot_gen = slots[slot].pcie_gen;
        if pcie_gen <= slot_gen {
            continue;
        }
        let cap = link_ceiling_mbps(slot_gen);
        let effect = if drive.spec.read_mbps > cap {
            format!(
                "reads top out around {cap} MB/s instead of {} MB/s",
                drive.spec.read_mbps
            )
        } else {
            "the drive's rated speeds still fit the slower link".to_owned()
        };
        out.push(Issue::new(
            RuleId::M2Speed,
            [drive.id(), board.id()],
            format!(
                "{drive} is a {pcie_gen} drive, but the fastest M.2 slot left for it on {board} \
                 is {slot_gen}, so it runs at {slot_gen} speed: {effect}.",
            ),
        ));
    }
    CHECKED
}

pub(super) fn sata_ports(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let board = cx.motherboard()?;
    let drives: Vec<_> = cx
        .storage()
        .iter()
        .filter(|d| d.spec.uses_sata_port())
        .collect();
    if drives.is_empty() {
        return None;
    }
    let ports = usize::from(board.spec.sata_ports);
    if drives.len() > ports {
        let has = if ports == 0 {
            "no SATA ports".to_owned()
        } else {
            format!("only {}", count(ports as u64, "SATA port", "SATA ports"))
        };
        out.push(Issue::new(
            RuleId::SataPorts,
            std::iter::once(board.id()).chain(drives.iter().map(|d| d.id())),
            format!(
                "The build has {} ({}), but {board} has {has}.",
                count(drives.len() as u64, "SATA drive", "SATA drives"),
                names(drives.iter().copied()),
            ),
        ));
    }
    CHECKED
}

fn m2_drives<'a>(cx: &Ctx<'_, 'a>) -> Vec<Part<'a, StorageSpec>> {
    cx.storage()
        .iter()
        .copied()
        .filter(|d| d.spec.uses_m2_slot())
        .collect()
}

fn is_sata(drive: &StorageSpec) -> bool {
    drive.interface == StorageInterface::Sata
}

fn fits(drive: &StorageSpec, slot: &M2Slot) -> bool {
    let length_ok = drive
        .form_factor
        .m2_length_mm()
        .is_some_and(|len| len <= slot.max_length_mm);
    length_ok && (!is_sata(drive) || slot.sata)
}

/// Sequential throughput ceiling of a PCIe x4 link, in MB/s.
fn link_ceiling_mbps(generation: PcieGen) -> u32 {
    match generation.number() {
        3 => 3600,
        4 => 7500,
        _ => 14900,
    }
}

/// Assigns M.2 drives to the board's M-key slots (indices into `storage_m2_slots()`), placing
/// as many as possible (bipartite matching with augmenting paths; the sizes are tiny).
///
/// Drives are tried most-constrained first (SATA, then fastest NVMe) and slots fastest first,
/// so the result also tends to give fast drives fast slots, which `m2_speed` reports on.
fn assign_m2(drives: &[Part<'_, StorageSpec>], board: &MotherboardSpec) -> Vec<Option<usize>> {
    let slots: Vec<&M2Slot> = board.storage_m2_slots().collect();
    let mut slot_order: Vec<usize> = (0..slots.len()).collect();
    slot_order.sort_by_key(|&s| std::cmp::Reverse(slots[s].pcie_gen));
    let mut drive_order: Vec<usize> = (0..drives.len()).collect();
    drive_order.sort_by_key(|&d| match drives[d].spec.interface {
        StorageInterface::Sata => (0, 0),
        StorageInterface::Nvme { pcie_gen } => (1, u8::MAX - pcie_gen.number()),
    });

    let mut slot_owner: Vec<Option<usize>> = vec![None; slots.len()];
    for &drive in &drive_order {
        let mut visited = vec![false; slots.len()];
        augment(
            drive,
            drives,
            &slots,
            &slot_order,
            &mut slot_owner,
            &mut visited,
        );
    }

    let mut assignment = vec![None; drives.len()];
    for (slot, owner) in slot_owner.iter().enumerate() {
        if let Some(drive) = owner {
            assignment[*drive] = Some(slot);
        }
    }
    assignment
}

/// Places `drive` in a free fitting slot, or else moves an owner along an augmenting path.
fn augment(
    drive: usize,
    drives: &[Part<'_, StorageSpec>],
    slots: &[&M2Slot],
    slot_order: &[usize],
    slot_owner: &mut [Option<usize>],
    visited: &mut [bool],
) -> bool {
    let fitting = || {
        slot_order
            .iter()
            .copied()
            .filter(|&s| fits(drives[drive].spec, slots[s]))
    };
    if let Some(free) = fitting().find(|&s| slot_owner[s].is_none()) {
        slot_owner[free] = Some(drive);
        return true;
    }
    for slot in fitting() {
        if std::mem::replace(&mut visited[slot], true) {
            continue;
        }
        let Some(owner) = slot_owner[slot] else {
            continue;
        };
        if augment(owner, drives, slots, slot_order, slot_owner, visited) {
            slot_owner[slot] = Some(drive);
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use techdepo_catalog::PcieGen;
    use techdepo_catalog::spec::{M2Key, M2Slot, StorageFormFactor, StorageInterface};

    use crate::fixtures::*;
    use crate::rule::{RuleId, Severity};

    fn slot(sata: bool, generation: PcieGen) -> M2Slot {
        M2Slot {
            key: M2Key::M,
            max_length_mm: 80,
            pcie_gen: generation,
            sata,
        }
    }

    fn m2_sata(id: &str) -> techdepo_catalog::Product {
        storage_product(id, |s| {
            s.interface = StorageInterface::Sata;
            s.read_mbps = 550;
            s.write_mbps = 500;
        })
    }

    #[test]
    fn too_many_m2_drives() {
        let board = board_product("b", |_| {});
        let nvme = storage_product("n", |_| {});
        assert!(check(&[&board, &nvme, &nvme]).issues.is_empty());
        let issue = only_issue(&check(&[&board, &nvme, &nvme, &nvme]), RuleId::M2Slots).clone();
        assert!(issue.message.contains("3 M.2 drives") && issue.message.contains("only 2 M.2"));
        assert_eq!(issue.parts.len(), 2, "the board plus the drive left over");
    }

    #[test]
    fn e_key_slots_do_not_count() {
        let board = board_product("b", |s| {
            s.m2_slots = vec![M2Slot {
                key: M2Key::E,
                max_length_mm: 30,
                pcie_gen: PcieGen::GEN3,
                sata: false,
            }]
        });
        let issue = only_issue(
            &check(&[&board, &storage_product("n", |_| {})]),
            RuleId::M2Slots,
        )
        .clone();
        assert!(
            issue.message.contains("only 0 M.2 storage slots"),
            "{}",
            issue.message
        );
    }

    #[test]
    fn m2_sata_drive_needs_a_sata_slot() {
        // Only slot 0 takes SATA: the SATA drive gets it even though the NVMe drive is listed first.
        let board = board_product("b", |s| {
            s.m2_slots = vec![slot(true, PcieGen::GEN4), slot(false, PcieGen::GEN4)]
        });
        let (nvme, sata) = (storage_product("n", |_| {}), m2_sata("s"));
        assert!(check(&[&board, &nvme, &sata]).issues.is_empty());

        let no_sata = board_product("b", |s| s.m2_slots[1].sata = false);
        let issue = only_issue(&check(&[&no_sata, &sata]), RuleId::M2Slots).clone();
        assert!(
            issue.message.contains("s is M.2 SATA, but none of"),
            "{}",
            issue.message
        );
    }

    #[test]
    fn short_slots_reject_2280_drives() {
        let board = board_product("b", |s| {
            s.m2_slots = vec![M2Slot {
                max_length_mm: 42,
                ..slot(false, PcieGen::GEN4)
            }]
        });
        let issue = only_issue(
            &check(&[&board, &storage_product("n", |_| {})]),
            RuleId::M2Slots,
        )
        .clone();
        assert!(
            issue.message.contains("length and interface"),
            "{}",
            issue.message
        );
    }

    #[test]
    fn fast_drive_gets_the_fast_slot() {
        let board = board_product("b", |s| {
            s.m2_slots = vec![slot(false, PcieGen::GEN3), slot(false, PcieGen::GEN5)]
        });
        let gen5 = storage_product("g5", |s| {
            s.interface = StorageInterface::Nvme {
                pcie_gen: PcieGen::GEN5,
            };
            s.read_mbps = 12000;
        });
        let gen4 = storage_product("g4", |_| {});
        // The Gen 5 drive takes the Gen 5 slot; the Gen 4 drive lands in Gen 3.
        let report = check(&[&board, &gen4, &gen5]);
        let issue = only_issue(&report, RuleId::M2Speed);
        assert_eq!(issue.severity, Severity::Note);
        assert_eq!(issue.parts[0], gen4.id);
        assert!(
            issue.message.contains("around 3600 MB/s instead of 7000"),
            "{}",
            issue.message
        );
    }

    #[test]
    fn sata_ports_count_bay_drives_only() {
        let board = board_product("b", |s| s.sata_ports = 1);
        let ssd = storage_product("s", |s| {
            s.form_factor = StorageFormFactor::Sata25;
            s.interface = StorageInterface::Sata;
        });
        // An M.2 SATA drive uses an M.2 slot, not a port.
        assert!(check(&[&board, &ssd, &m2_sata("m")]).issues.is_empty());
        let issue = only_issue(&check(&[&board, &ssd, &ssd]), RuleId::SataPorts).clone();
        assert!(
            issue.message.contains("2 SATA drives") && issue.message.contains("only 1 SATA port")
        );
    }
}
