use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use techdepo_domain::labeled_enum;

use crate::platform::PcieGen;

labeled_enum! {
    /// Physical drive format.
    #[allow(non_camel_case_types)]
    pub enum StorageFormFactor {
        /// M.2 2280 module (takes an M.2 slot).
        M2_2280 => ("m2_2280", "M.2 2280"),
        /// 2.5" drive (takes a SATA port and a 2.5" bay).
        Sata25 => ("2.5in", "2.5\""),
        /// 3.5" drive (takes a SATA port and a 3.5" bay).
        Sata35 => ("3.5in", "3.5\""),
    }
}

impl StorageFormFactor {
    /// Module length in mm for M.2 drives.
    pub const fn m2_length_mm(self) -> Option<u16> {
        match self {
            Self::M2_2280 => Some(80),
            Self::Sata25 | Self::Sata35 => None,
        }
    }
}

/// Drive interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StorageInterface {
    /// NVMe over PCIe.
    Nvme {
        /// PCIe generation the drive is built for.
        pcie_gen: PcieGen,
    },
    /// SATA (a SATA port for 2.5"/3.5" drives; an M.2 slot with SATA support for M.2 drives).
    Sata,
}

/// Storage drive specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StorageSpec {
    /// Physical format.
    pub form_factor: StorageFormFactor,
    /// Interface.
    pub interface: StorageInterface,
    /// Capacity in GB.
    pub capacity_gb: u32,
    /// Sequential read in MB/s.
    pub read_mbps: u32,
    /// Sequential write in MB/s.
    pub write_mbps: u32,
}

impl StorageSpec {
    /// Occupies an M.2 slot.
    pub fn uses_m2_slot(&self) -> bool {
        self.form_factor.m2_length_mm().is_some()
    }

    /// Occupies a motherboard SATA port.
    pub fn uses_sata_port(&self) -> bool {
        !self.uses_m2_slot() && self.interface == StorageInterface::Sata
    }
}
