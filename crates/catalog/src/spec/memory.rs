use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::platform::MemoryType;

/// Memory kit specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemorySpec {
    /// DRAM generation.
    pub memory_type: MemoryType,
    /// Number of modules in the kit.
    pub modules: u8,
    /// Capacity per module in GB.
    pub module_gb: u16,
    /// Rated speed in MT/s (XMP/EXPO profile).
    pub speed_mts: u32,
    /// CAS latency at the rated speed.
    pub cas_latency: u8,
    /// Module height in mm (heat spreader included).
    pub height_mm: u16,
}

impl MemorySpec {
    /// Total kit capacity in GB.
    pub fn total_gb(&self) -> u32 {
        u32::from(self.modules) * u32::from(self.module_gb)
    }
}
