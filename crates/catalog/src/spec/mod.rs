//! Typed, per-category specifications. Every value the compatibility rules read is an enum or a
//! number with a unit in its name, never a free string.

mod case;
mod cooler;
mod cpu;
mod gpu;
mod memory;
mod motherboard;
mod psu;
mod storage;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use case::{CaseSize, CaseSpec, PsuFit, RADIATOR_SIZES_MM, RadiatorMount, RadiatorPosition};
pub use cooler::{CoolerKind, CoolerSpec};
pub use cpu::{CpuSpec, MEMORY_CHANNELS, MemorySupport};
pub use gpu::{GpuPowerConnectors, GpuSpec};
pub use memory::MemorySpec;
pub use motherboard::{CpuSupport, M2_LENGTHS_MM, M2Key, M2Slot, MotherboardSpec};
pub use psu::{Efficiency, Modularity, PsuConnectors, PsuSpec};
pub use storage::{StorageFormFactor, StorageInterface, StorageSpec};

use crate::Category;

/// The specification of a product, one variant per category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProductSpec {
    Cpu(CpuSpec),
    Motherboard(MotherboardSpec),
    Memory(MemorySpec),
    Storage(StorageSpec),
    Gpu(GpuSpec),
    Case(CaseSpec),
    Psu(PsuSpec),
    Cooler(CoolerSpec),
}

macro_rules! accessors {
    ($($fn:ident => $variant:ident($ty:ty)),+ $(,)?) => {$(
        #[doc = concat!("The spec if this is a `", stringify!($variant), "`.")]
        pub fn $fn(&self) -> Option<&$ty> {
            match self {
                Self::$variant(spec) => Some(spec),
                _ => None,
            }
        }
    )+};
}

impl ProductSpec {
    /// The category this spec belongs to.
    pub const fn category(&self) -> Category {
        match self {
            Self::Cpu(_) => Category::Cpu,
            Self::Motherboard(_) => Category::Motherboard,
            Self::Memory(_) => Category::Memory,
            Self::Storage(_) => Category::Storage,
            Self::Gpu(_) => Category::Gpu,
            Self::Case(_) => Category::Case,
            Self::Psu(_) => Category::Psu,
            Self::Cooler(_) => Category::Cooler,
        }
    }

    accessors! {
        as_cpu => Cpu(CpuSpec),
        as_motherboard => Motherboard(MotherboardSpec),
        as_memory => Memory(MemorySpec),
        as_storage => Storage(StorageSpec),
        as_gpu => Gpu(GpuSpec),
        as_case => Case(CaseSpec),
        as_psu => Psu(PsuSpec),
        as_cooler => Cooler(CoolerSpec),
    }
}
