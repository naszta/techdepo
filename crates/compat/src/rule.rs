//! Rule identifiers and severities.

use techdepo_catalog::Category;
use techdepo_domain::labeled_enum;

labeled_enum! {
    /// How bad an [`Issue`](crate::Issue) is. Ordered from least to most severe, so
    /// `issues.iter().map(|i| i.severity).max()` is the worst one.
    pub enum Severity {
        /// Informational, e.g. memory running below its rated speed.
        Note => ("note", "Note"),
        /// Works, with a caveat (BIOS update, little PSU headroom, …).
        Warning => ("warning", "Warning"),
        /// Physically or electrically won't work. Blocks checkout unless overridden.
        Error => ("error", "Incompatible"),
    }
}

labeled_enum! {
    /// A compatibility rule. The wire name is a stable id used by the UI, snapshots and the
    /// agent tool; never rename one. Variants are listed in evaluation order.
    pub enum RuleId {
        // Errors: physically or electrically won't work.
        CpuSocket => ("cpu_socket", "CPU socket"),
        CpuSupport => ("cpu_support", "CPU support"),
        MemoryType => ("memory_type", "Memory type"),
        CpuMemoryType => ("cpu_memory_type", "CPU memory support"),
        MemorySlots => ("memory_slots", "Memory slots"),
        MemoryCapacity => ("memory_capacity", "Memory capacity"),
        CaseFormFactor => ("case_form_factor", "Motherboard fit"),
        GpuLength => ("gpu_length", "Graphics card length"),
        GpuThickness => ("gpu_thickness", "Graphics card thickness"),
        CoolerHeight => ("cooler_height", "Cooler height"),
        CoolerSocket => ("cooler_socket", "Cooler socket"),
        RadiatorMount => ("radiator_mount", "Radiator mount"),
        PsuFormFactor => ("psu_form_factor", "PSU form factor"),
        PsuLength => ("psu_length", "PSU length"),
        PsuWattage => ("psu_wattage", "PSU wattage"),
        GpuPowerConnectors => ("gpu_power_connectors", "GPU power connectors"),
        M2Slots => ("m2_slots", "M.2 slots"),
        SataPorts => ("sata_ports", "SATA ports"),
        DriveBays => ("drive_bays", "Drive bays"),
        SataPower => ("sata_power", "SATA power"),
        // Warnings: works with a caveat.
        BiosUpdate => ("bios_update", "BIOS update"),
        PsuHeadroom => ("psu_headroom", "PSU headroom"),
        CoolerTdp => ("cooler_tdp", "Cooler capacity"),
        RamClearance => ("ram_clearance", "Memory clearance"),
        PsuBracket => ("psu_bracket", "SFX bracket"),
        CpuPowerConnectors => ("cpu_power_connectors", "CPU power connectors"),
        NoVideoOutput => ("no_video_output", "Video output"),
        NoCooler => ("no_cooler", "CPU cooler"),
        // Notes: informational.
        MemorySpeed => ("memory_speed", "Memory speed"),
        MemoryMixedKits => ("memory_mixed_kits", "Mixed memory kits"),
        M2Speed => ("m2_speed", "M.2 link speed"),
        GpuPcieGen => ("gpu_pcie_gen", "GPU link speed"),
        GpuRecommendedPsu => ("gpu_recommended_psu", "Recommended PSU"),
    }
}

impl RuleId {
    /// Severity of every issue this rule reports.
    pub const fn severity(self) -> Severity {
        match self {
            Self::CpuSocket
            | Self::CpuSupport
            | Self::MemoryType
            | Self::CpuMemoryType
            | Self::MemorySlots
            | Self::MemoryCapacity
            | Self::CaseFormFactor
            | Self::GpuLength
            | Self::GpuThickness
            | Self::CoolerHeight
            | Self::CoolerSocket
            | Self::RadiatorMount
            | Self::PsuFormFactor
            | Self::PsuLength
            | Self::PsuWattage
            | Self::GpuPowerConnectors
            | Self::M2Slots
            | Self::SataPorts
            | Self::DriveBays
            | Self::SataPower => Severity::Error,
            Self::BiosUpdate
            | Self::PsuHeadroom
            | Self::CoolerTdp
            | Self::RamClearance
            | Self::PsuBracket
            | Self::CpuPowerConnectors
            | Self::NoVideoOutput
            | Self::NoCooler => Severity::Warning,
            Self::MemorySpeed
            | Self::MemoryMixedKits
            | Self::M2Speed
            | Self::GpuPcieGen
            | Self::GpuRecommendedPsu => Severity::Note,
        }
    }

    /// One sentence on what the rule checks, for the UI and the agent.
    pub const fn description(self) -> &'static str {
        match self {
            Self::CpuSocket => "The CPU and the motherboard use the same socket.",
            Self::CpuSupport => "The motherboard lists the CPU's family as supported.",
            Self::MemoryType => {
                "Memory kits match the motherboard's DDR generation (and each other when no board is chosen)."
            }
            Self::CpuMemoryType => "The CPU's memory controller supports the DDR generation used.",
            Self::MemorySlots => "All memory modules fit in the motherboard's DIMM slots.",
            Self::MemoryCapacity => "Total memory does not exceed the motherboard's maximum.",
            Self::CaseFormFactor => "The case takes the motherboard's form factor.",
            Self::GpuLength => "The graphics card is no longer than the case allows.",
            Self::GpuThickness => "The graphics card is no thicker than the case allows.",
            Self::CoolerHeight => {
                "An air cooler, with its fan raised over tall memory, is no taller than the \
                 case's cooler clearance."
            }
            Self::CoolerSocket => "The cooler's mounting kit supports the CPU socket.",
            Self::RadiatorMount => "The case has a mount for the AIO's radiator size.",
            Self::PsuFormFactor => {
                "The case takes the power supply's form factor (SFX also fits an ATX bay with a \
                 bracket)."
            }
            Self::PsuLength => "The power supply is no longer than the case allows.",
            Self::PsuWattage => {
                "The power supply covers the estimated load (components plus 10 %)."
            }
            Self::GpuPowerConnectors => {
                "The power supply has the PCIe 8-pin and 12V-2x6 connectors the graphics card needs."
            }
            Self::M2Slots => {
                "Every M.2 drive gets an M.2 storage slot that takes its length and interface."
            }
            Self::SataPorts => "The motherboard has a SATA port for every SATA drive.",
            Self::DriveBays => "The case has a bay for every 2.5\" and 3.5\" drive.",
            Self::SataPower => "The power supply has a SATA power connector for every SATA drive.",
            Self::BiosUpdate => {
                "The motherboard's shipped BIOS already supports the CPU, or can be updated."
            }
            Self::PsuHeadroom => "The power supply keeps at least 20 % headroom over the estimate.",
            Self::CoolerTdp => {
                "The cooler is rated for at least the CPU's sustained peak power, not just its TDP."
            }
            Self::RamClearance => "Memory fits under an air cooler's fan without raising it.",
            Self::PsuBracket => {
                "An SFX power supply in an ATX-only case needs an SFX-to-ATX bracket."
            }
            Self::CpuPowerConnectors => {
                "With a high-power CPU, the power supply has a CPU (EPS) cable for every 8-pin \
                 CPU power header."
            }
            Self::NoVideoOutput => {
                "A CPU without integrated graphics is paired with a graphics card."
            }
            Self::NoCooler => "A CPU without a bundled cooler is paired with a cooler.",
            Self::MemorySpeed => {
                "Memory faster than the CPU (with this many modules) or motherboard supports runs \
                 at the lower speed."
            }
            Self::MemoryMixedKits => "Several kits run at the slowest kit's settings.",
            Self::M2Speed => "An NVMe drive in a slower M.2 slot runs at the slot's speed.",
            Self::GpuPcieGen => {
                "A graphics card in a slower PCIe slot runs at the slot's link speed."
            }
            Self::GpuRecommendedPsu => {
                "The power supply is below the graphics card vendor's recommendation."
            }
        }
    }

    /// The categories whose parts this rule reads. A rule runs only when the parts it needs
    /// are in the build (except the "missing part" rules, see [`RuleId::is_absence`]).
    pub const fn categories(self) -> &'static [Category] {
        use Category::*;
        match self {
            Self::CpuSocket | Self::CpuSupport | Self::BiosUpdate => &[Cpu, Motherboard],
            Self::MemoryType | Self::MemorySlots | Self::MemoryCapacity => &[Memory, Motherboard],
            Self::CpuMemoryType | Self::MemorySpeed => &[Cpu, Motherboard, Memory],
            Self::MemoryMixedKits => &[Memory],
            Self::CaseFormFactor => &[Case, Motherboard],
            Self::GpuLength | Self::GpuThickness => &[Gpu, Case],
            Self::CoolerHeight => &[Cooler, Case, Memory],
            Self::RadiatorMount => &[Cooler, Case],
            Self::CoolerSocket => &[Cooler, Cpu, Motherboard],
            Self::PsuFormFactor | Self::PsuLength | Self::PsuBracket => &[Psu, Case],
            Self::PsuWattage | Self::PsuHeadroom => &[Psu, Cpu, Gpu],
            Self::GpuPowerConnectors | Self::GpuRecommendedPsu => &[Gpu, Psu],
            Self::M2Slots | Self::SataPorts | Self::M2Speed => &[Storage, Motherboard],
            Self::DriveBays => &[Storage, Case],
            Self::SataPower => &[Storage, Psu],
            Self::CoolerTdp => &[Cooler, Cpu],
            Self::RamClearance => &[Cooler, Memory],
            Self::CpuPowerConnectors => &[Cpu, Motherboard, Psu],
            Self::NoVideoOutput | Self::NoCooler => &[Cpu],
            Self::GpuPcieGen => &[Gpu, Motherboard],
        }
    }

    /// Fires because a part is *missing* (no GPU, no cooler) rather than because two parts
    /// clash. [`check_pair`](crate::check_pair) skips these: a pair is not a whole build.
    pub const fn is_absence(self) -> bool {
        matches!(self, Self::NoVideoOutput | Self::NoCooler)
    }

    /// The category to add to the build to resolve an absence rule.
    pub const fn fixed_by_adding(self) -> Option<Category> {
        match self {
            Self::NoVideoOutput => Some(Category::Gpu),
            Self::NoCooler => Some(Category::Cooler),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severities_are_grouped_in_declaration_order() {
        let severities: Vec<_> = RuleId::ALL.iter().map(|r| r.severity()).collect();
        let mut sorted = severities.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(severities, sorted, "errors, then warnings, then notes");
    }

    #[test]
    fn wire_names_round_trip() {
        for rule in RuleId::ALL {
            assert_eq!(rule.as_str().parse::<RuleId>().unwrap(), *rule);
            assert!(!rule.description().is_empty());
            assert!(!rule.categories().is_empty());
        }
        assert!(Severity::Error > Severity::Warning && Severity::Warning > Severity::Note);
    }
}
