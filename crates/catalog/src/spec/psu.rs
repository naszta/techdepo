use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use techdepo_domain::labeled_enum;

use crate::platform::PsuFormFactor;

labeled_enum! {
    /// 80 PLUS efficiency rating.
    pub enum Efficiency {
        Bronze => ("bronze", "80 PLUS Bronze"),
        Gold => ("gold", "80 PLUS Gold"),
        Platinum => ("platinum", "80 PLUS Platinum"),
        Titanium => ("titanium", "80 PLUS Titanium"),
    }
}

labeled_enum! {
    /// Cable modularity.
    pub enum Modularity {
        NonModular => ("non_modular", "Non-modular"),
        SemiModular => ("semi_modular", "Semi-modular"),
        FullyModular => ("fully_modular", "Fully modular"),
    }
}

/// Connectors a PSU provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PsuConnectors {
    /// 8-pin (4+4) EPS CPU connectors.
    pub eps_8pin: u8,
    /// 8-pin (6+2) PCIe connectors.
    pub pcie_8pin: u8,
    /// 12V-2x6 (16-pin) connectors.
    pub twelve_v_2x6: u8,
    /// SATA power connectors.
    pub sata: u8,
}

/// Power supply specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PsuSpec {
    /// Rated continuous output in watts.
    pub wattage_w: u16,
    /// Form factor.
    pub form_factor: PsuFormFactor,
    /// Depth in mm (compared against the case's maximum).
    pub length_mm: u16,
    /// Efficiency rating.
    pub efficiency: Efficiency,
    /// Cable modularity.
    pub modular: Modularity,
    /// Connectors.
    pub connectors: PsuConnectors,
}
