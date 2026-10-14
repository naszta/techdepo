use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::platform::PcieGen;

/// Graphics card specification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GpuSpec {
    /// Card length in mm (compared against the case's maximum).
    pub length_mm: u16,
    /// Thickness in expansion slots (2, 2.5, 3, …).
    pub slot_width: f32,
    /// Total board power in watts (used for the PSU load estimate).
    pub board_power_w: u16,
    /// Power connectors the card needs from the PSU.
    pub power_connectors: GpuPowerConnectors,
    /// PCIe generation of the card's interface.
    pub pcie_gen: PcieGen,
    /// Video memory in GB.
    pub vram_gb: u8,
    /// PSU wattage the vendor recommends for a typical system.
    pub recommended_psu_w: u16,
}

/// Power connectors a graphics card needs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GpuPowerConnectors {
    /// 8-pin (6+2) PCIe connectors.
    pub pcie_8pin: u8,
    /// 12V-2x6 (16-pin) connectors.
    pub twelve_v_2x6: u8,
}
