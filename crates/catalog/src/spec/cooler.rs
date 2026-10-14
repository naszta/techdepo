use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::platform::Socket;

/// Air tower or all-in-one liquid cooler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CoolerKind {
    /// Air cooler; its height must fit the case.
    Air {
        /// Height above the motherboard in mm.
        height_mm: u16,
        /// Tallest memory module that fits under the cooler with its fan in the standard
        /// position, in mm. Absent when the cooler does not overhang the memory slots.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ram_clearance_mm: Option<u16>,
    },
    /// All-in-one liquid cooler; its radiator needs a matching case mount.
    Aio {
        /// Radiator size in mm (120, 240, 280, 360, …).
        radiator_mm: u16,
    },
}

/// CPU cooler specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CoolerSpec {
    /// Sockets the included mounting kit supports.
    pub sockets: Vec<Socket>,
    /// Air or AIO, with the dimension that matters for case fit.
    pub kind: CoolerKind,
    /// Heat the cooler is rated to dissipate, in watts (compared against the CPU's sustained
    /// peak power).
    pub tdp_rating_w: u16,
}

impl CoolerSpec {
    /// Whether the mounting kit fits `socket`.
    pub fn supports(&self, socket: Socket) -> bool {
        self.sockets.contains(&socket)
    }
}
