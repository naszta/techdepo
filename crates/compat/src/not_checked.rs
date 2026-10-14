//! What the engine deliberately does not check, so the UI and the agent can say so.

use serde::Serialize;

/// A known compatibility concern outside the engine's closed world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NotChecked {
    /// Stable snake_case id.
    pub id: &'static str,
    /// Short title.
    pub title: &'static str,
    /// Why it is not checked and what the customer should look at instead.
    pub detail: &'static str,
}

/// Everything the rules do not cover. Shown under the builder's verdict ("What we don't check")
/// and available to the agent, so neither ever implies more certainty than the data supports.
pub const NOT_CHECKED: &[NotChecked] = &[
    NotChecked {
        id: "gpu_slot_layout",
        title: "Slots covered by the graphics card",
        detail: "A thick graphics card can cover the motherboard's lower PCIe or M.2 slots; only \
                 its thickness against the case is checked.",
    },
    NotChecked {
        id: "raised_fan_clearance",
        title: "Raised cooler fans",
        detail: "When tall memory raises an air cooler's fan, the cooler is assumed to get taller \
                 by exactly the overhang; fan clips and side-panel fans are not modelled.",
    },
    NotChecked {
        id: "radiator_gpu_clearance",
        title: "Radiator thickness",
        detail: "A front radiator takes space from the graphics card's length clearance; only \
                 the card's length against the case maximum is checked.",
    },
    NotChecked {
        id: "stock_cooler",
        title: "Bundled coolers",
        detail: "A CPU's stock cooler is assumed to fit the case and handle the CPU's TDP.",
    },
    NotChecked {
        id: "board_video_outputs",
        title: "Motherboard video outputs",
        detail: "Every board is assumed to have a display output for the CPU's integrated \
                 graphics.",
    },
    NotChecked {
        id: "lane_sharing",
        title: "M.2 and SATA lane sharing",
        detail: "On some boards an occupied M.2 slot disables SATA ports or slows another \
                 slot; ports and slots are counted as independent.",
    },
    NotChecked {
        id: "memory_validation",
        title: "Memory overclocking stability",
        detail: "EXPO/XMP speeds are not guaranteed, least of all with four modules; the memory \
                 speed note uses the officially supported speed, which is lower with two modules \
                 per channel.",
    },
    NotChecked {
        id: "power_adapters",
        title: "Power adapters and cable lengths",
        detail: "Adapter cables (such as 2x 8-pin to 12V-2x6) are not counted, and cable \
                 lengths are assumed to reach.",
    },
    NotChecked {
        id: "transient_power",
        title: "Power spikes and thermals",
        detail: "The wattage estimate is component peak draw plus 10 %, not a measurement; \
                 short GPU spikes and case airflow are not modelled.",
    },
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn ids_are_unique_snake_case() {
        let ids: BTreeSet<_> = NOT_CHECKED.iter().map(|n| n.id).collect();
        assert_eq!(ids.len(), NOT_CHECKED.len());
        for id in ids {
            assert!(
                id.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{id}"
            );
        }
    }
}
