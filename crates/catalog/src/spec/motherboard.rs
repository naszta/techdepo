use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use techdepo_domain::labeled_enum;

use crate::platform::{Chipset, CpuFamily, FormFactor, MemoryType, PcieGen, Socket};

/// Valid M.2 module lengths in mm (2230, 2242, 2260, 2280, 22110).
pub const M2_LENGTHS_MM: [u16; 5] = [30, 42, 60, 80, 110];

/// Motherboard specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MotherboardSpec {
    /// CPU socket; must match the chipset.
    pub socket: Socket,
    /// Chipset.
    pub chipset: Chipset,
    /// Board form factor.
    pub form_factor: FormFactor,
    /// The one memory generation the slots take.
    pub memory_type: MemoryType,
    /// Number of DIMM slots.
    pub memory_slots: u8,
    /// Maximum total memory in GB.
    pub max_memory_gb: u16,
    /// Fastest memory speed the board supports, in MT/s.
    pub max_memory_speed_mts: u32,
    /// M.2 slots.
    pub m2_slots: Vec<M2Slot>,
    /// SATA ports.
    pub sata_ports: u8,
    /// PCIe generation of the primary x16 slot.
    pub pcie_x16_gen: PcieGen,
    /// BIOS version the board ships with (a plain revision number).
    pub bios_version: u16,
    /// Can flash a BIOS without a CPU installed (USB BIOS Flashback).
    pub bios_flashback: bool,
    /// CPU families the board supports, with the minimum BIOS for each. A family that is not
    /// listed is not supported.
    pub cpu_support: Vec<CpuSupport>,
    /// 8-pin EPS CPU power headers.
    pub eps_8pin: u8,
    /// Additional 4-pin EPS CPU power headers.
    pub eps_4pin: u8,
    /// Onboard Wi-Fi.
    pub wifi: bool,
}

/// Minimum BIOS version for a CPU family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CpuSupport {
    /// CPU family.
    pub family: CpuFamily,
    /// Minimum BIOS version; above the board's `bios_version` means an update is needed.
    pub min_bios: u16,
}

labeled_enum! {
    /// M.2 slot keying.
    pub enum M2Key {
        /// Storage (NVMe or SATA M.2 SSDs).
        M => ("m", "M key"),
        /// Wireless modules; never storage.
        E => ("e", "E key"),
    }
}

/// One M.2 slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct M2Slot {
    /// Keying.
    pub key: M2Key,
    /// Longest module the slot takes, in mm (80 for 2280, 110 for 22110).
    pub max_length_mm: u16,
    /// PCIe generation of the slot's lanes.
    pub pcie_gen: PcieGen,
    /// Also accepts M.2 SATA SSDs.
    pub sata: bool,
}

impl MotherboardSpec {
    /// Minimum BIOS version for `family`, or `None` if the board does not support it.
    pub fn min_bios_for(&self, family: CpuFamily) -> Option<u16> {
        self.cpu_support
            .iter()
            .find(|s| s.family == family)
            .map(|s| s.min_bios)
    }

    /// M.2 slots that take storage (M key).
    pub fn storage_m2_slots(&self) -> impl Iterator<Item = &M2Slot> {
        self.m2_slots.iter().filter(|s| s.key == M2Key::M)
    }
}
