//! Platform standards shared by several categories: sockets, memory, form factors, PCIe.
//!
//! Brands are fictional, standards are real. CPU families are fictional codenames, each bound
//! to one socket.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use techdepo_domain::labeled_enum;

labeled_enum! {
    /// CPU socket.
    pub enum Socket {
        Am4 => ("am4", "AM4"),
        Am5 => ("am5", "AM5"),
        Lga1700 => ("lga1700", "LGA1700"),
        Lga1851 => ("lga1851", "LGA1851"),
    }
}

impl Socket {
    /// Memory generations the socket's platform can run.
    pub const fn memory_types(self) -> &'static [MemoryType] {
        match self {
            Self::Am4 => &[MemoryType::Ddr4],
            Self::Am5 | Self::Lga1851 => &[MemoryType::Ddr5],
            Self::Lga1700 => &[MemoryType::Ddr4, MemoryType::Ddr5],
        }
    }
}

labeled_enum! {
    /// DRAM generation.
    pub enum MemoryType {
        Ddr4 => ("ddr4", "DDR4"),
        Ddr5 => ("ddr5", "DDR5"),
    }
}

impl MemoryType {
    /// Plausible kit speeds in MT/s (JEDEC base to the fastest retail kits).
    pub const fn speed_range_mts(self) -> (u32, u32) {
        match self {
            Self::Ddr4 => (2133, 4800),
            Self::Ddr5 => (4800, 8800),
        }
    }
}

labeled_enum! {
    /// Motherboard form factor.
    pub enum FormFactor {
        EAtx => ("e_atx", "E-ATX"),
        Atx => ("atx", "ATX"),
        MicroAtx => ("micro_atx", "Micro-ATX"),
        MiniItx => ("mini_itx", "Mini-ITX"),
    }
}

labeled_enum! {
    /// Power supply form factor.
    pub enum PsuFormFactor {
        Atx => ("atx", "ATX"),
        Sfx => ("sfx", "SFX"),
    }
}

labeled_enum! {
    /// Fictional CPU generation codenames. Each family belongs to exactly one socket; boards
    /// list a minimum BIOS version per family.
    pub enum CpuFamily {
        /// Voltaris, AM4, DDR4.
        Granite => ("granite", "Granite"),
        /// Voltaris, AM5 first generation.
        Quartz => ("quartz", "Quartz"),
        /// Voltaris, AM5 second generation (needs a newer BIOS on early AM5 boards).
        Obsidian => ("obsidian", "Obsidian"),
        /// LGA1700 first generation.
        Falcon => ("falcon", "Falcon"),
        /// LGA1700 second generation (needs a newer BIOS on early LGA1700 boards).
        Harrier => ("harrier", "Harrier"),
        /// LGA1851.
        Osprey => ("osprey", "Osprey"),
    }
}

impl CpuFamily {
    /// The socket this family uses.
    pub const fn socket(self) -> Socket {
        match self {
            Self::Granite => Socket::Am4,
            Self::Quartz | Self::Obsidian => Socket::Am5,
            Self::Falcon | Self::Harrier => Socket::Lga1700,
            Self::Osprey => Socket::Lga1851,
        }
    }
}

labeled_enum! {
    /// Motherboard chipset.
    pub enum Chipset {
        A520 => ("a520", "A520"),
        B550 => ("b550", "B550"),
        X570 => ("x570", "X570"),
        A620 => ("a620", "A620"),
        B650 => ("b650", "B650"),
        B650E => ("b650e", "B650E"),
        X670E => ("x670e", "X670E"),
        X870 => ("x870", "X870"),
        X870E => ("x870e", "X870E"),
        H610 => ("h610", "H610"),
        B760 => ("b760", "B760"),
        Z790 => ("z790", "Z790"),
        B860 => ("b860", "B860"),
        Z890 => ("z890", "Z890"),
    }
}

impl Chipset {
    /// The socket boards with this chipset use.
    pub const fn socket(self) -> Socket {
        match self {
            Self::A520 | Self::B550 | Self::X570 => Socket::Am4,
            Self::A620 | Self::B650 | Self::B650E | Self::X670E | Self::X870 | Self::X870E => {
                Socket::Am5
            }
            Self::H610 | Self::B760 | Self::Z790 => Socket::Lga1700,
            Self::B860 | Self::Z890 => Socket::Lga1851,
        }
    }
}

/// PCI Express generation (3, 4 or 5). Written as a plain integer in YAML.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "u8", into = "u8")]
pub struct PcieGen(u8);

impl PcieGen {
    /// PCIe 3.0.
    pub const GEN3: Self = Self(3);
    /// PCIe 4.0.
    pub const GEN4: Self = Self(4);
    /// PCIe 5.0.
    pub const GEN5: Self = Self(5);

    /// The generation number.
    pub const fn number(self) -> u8 {
        self.0
    }
}

/// A PCIe generation outside 3–5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unsupported PCIe generation {0} (expected 3, 4 or 5)")]
pub struct InvalidPcieGen(pub u8);

impl TryFrom<u8> for PcieGen {
    type Error = InvalidPcieGen;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if (3..=5).contains(&value) {
            Ok(Self(value))
        } else {
            Err(InvalidPcieGen(value))
        }
    }
}

impl From<PcieGen> for u8 {
    fn from(value: PcieGen) -> Self {
        value.0
    }
}

impl fmt::Display for PcieGen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PCIe {}.0", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_and_chipsets_have_sockets() {
        assert_eq!(CpuFamily::Obsidian.socket(), Socket::Am5);
        assert_eq!(Chipset::B650.socket(), Socket::Am5);
        assert_eq!(Socket::Lga1700.memory_types().len(), 2);
    }

    #[test]
    fn pcie_gen_is_validated() {
        assert_eq!(serde_yaml::from_str::<PcieGen>("4").unwrap(), PcieGen::GEN4);
        assert!(serde_yaml::from_str::<PcieGen>("6").is_err());
        assert_eq!(PcieGen::GEN5.to_string(), "PCIe 5.0");
    }
}
