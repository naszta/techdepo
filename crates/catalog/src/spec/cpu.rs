use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::platform::{CpuFamily, MemoryType, Socket};

/// Memory channels of every CPU in the catalog (all consumer platforms are dual-channel).
pub const MEMORY_CHANNELS: u32 = 2;

/// CPU specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CpuSpec {
    /// Socket; must match `family`'s socket.
    pub socket: Socket,
    /// Generation; motherboards list a minimum BIOS per family.
    pub family: CpuFamily,
    /// Physical cores.
    pub cores: u8,
    /// Hardware threads.
    pub threads: u8,
    /// Base clock in MHz.
    pub base_clock_mhz: u16,
    /// Maximum boost clock in MHz.
    pub boost_clock_mhz: u16,
    /// Rated TDP in watts, as marketed. Shown to customers; the rules use `peak_power_w`.
    pub tdp_w: u16,
    /// Sustained peak package power in watts: the heat a cooler must remove under all-core
    /// load and the CPU's share of the PSU load estimate.
    pub peak_power_w: u16,
    /// Supported memory generations with the officially supported speed for each.
    pub memory: Vec<MemorySupport>,
    /// Has integrated graphics (a build without a GPU still has video output).
    pub integrated_graphics: bool,
    /// Ships with a stock cooler in the box.
    pub bundled_cooler: bool,
}

/// One memory generation a CPU supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemorySupport {
    /// Memory generation.
    pub memory_type: MemoryType,
    /// Officially supported speed in MT/s with one module per channel (two modules);
    /// faster kits run at this speed.
    pub max_speed_mts: u32,
    /// Officially supported speed in MT/s with two modules per channel (four modules on these
    /// dual-channel platforms). Lower than `max_speed_mts`, sharply so for DDR5.
    pub max_speed_2dpc_mts: u32,
}

impl CpuSpec {
    /// Whether the memory controller supports `memory_type`.
    pub fn supports_memory(&self, memory_type: MemoryType) -> bool {
        self.memory.iter().any(|m| m.memory_type == memory_type)
    }

    /// Officially supported speed for `memory_type` with `modules` modules in total, if the
    /// type is supported at all. More than [`MEMORY_CHANNELS`] modules means two per channel.
    pub fn max_memory_speed_mts(&self, memory_type: MemoryType, modules: u32) -> Option<u32> {
        let support = self.memory.iter().find(|m| m.memory_type == memory_type)?;
        Some(if modules > MEMORY_CHANNELS {
            support.max_speed_2dpc_mts
        } else {
            support.max_speed_mts
        })
    }
}
