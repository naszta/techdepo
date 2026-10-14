use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use techdepo_domain::labeled_enum;

use crate::platform::{FormFactor, PsuFormFactor};

/// Standard radiator sizes in mm.
pub const RADIATOR_SIZES_MM: [u16; 6] = [120, 140, 240, 280, 360, 420];

labeled_enum! {
    /// Case size class.
    pub enum CaseSize {
        FullTower => ("full_tower", "Full tower"),
        MidTower => ("mid_tower", "Mid tower"),
        MiniTower => ("mini_tower", "Mini tower"),
        SmallFormFactor => ("small_form_factor", "Small form factor"),
    }
}

labeled_enum! {
    /// Where a radiator can be mounted.
    pub enum RadiatorPosition {
        Front => ("front", "Front"),
        Top => ("top", "Top"),
        Rear => ("rear", "Rear"),
        Side => ("side", "Side"),
        Bottom => ("bottom", "Bottom"),
    }
}

/// Radiator sizes one position takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RadiatorMount {
    /// Mount position.
    pub position: RadiatorPosition,
    /// Radiator sizes in mm that fit here (from [`RADIATOR_SIZES_MM`]).
    pub sizes_mm: Vec<u16>,
}

/// Case specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaseSpec {
    /// Size class.
    pub size: CaseSize,
    /// Motherboard form factors that fit.
    pub motherboard_form_factors: Vec<FormFactor>,
    /// Longest graphics card in mm.
    pub max_gpu_length_mm: u16,
    /// Thickest graphics card, in expansion slots.
    pub max_gpu_slots: u8,
    /// Tallest air cooler in mm.
    pub max_cooler_height_mm: u16,
    /// PSU form factors that fit as they are. An ATX-only case takes an SFX unit with an
    /// SFX-to-ATX bracket; a case that ships with the bracket lists both.
    pub psu_form_factors: Vec<PsuFormFactor>,
    /// Longest PSU in mm.
    pub max_psu_length_mm: u16,
    /// Radiator mounts per position.
    pub radiator_mounts: Vec<RadiatorMount>,
    /// 2.5" drive bays (dedicated).
    pub bays_2_5: u8,
    /// 3.5" drive bays (each also takes a 2.5" drive).
    pub bays_3_5: u8,
    /// Fans included with the case.
    pub included_fans: u8,
}

/// How a power supply goes into a case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PsuFit {
    /// The case takes the form factor as it is.
    Native,
    /// An SFX unit in an ATX bay: needs an SFX-to-ATX bracket that is not included.
    Bracket,
    /// It does not go in at all.
    No,
}

impl CaseSpec {
    /// How a `form_factor` power supply goes into this case.
    pub fn psu_fit(&self, form_factor: PsuFormFactor) -> PsuFit {
        if self.psu_form_factors.contains(&form_factor) {
            PsuFit::Native
        } else if form_factor == PsuFormFactor::Sfx
            && self.psu_form_factors.contains(&PsuFormFactor::Atx)
        {
            PsuFit::Bracket
        } else {
            PsuFit::No
        }
    }

    /// Whether some position takes a radiator of `size_mm`.
    pub fn fits_radiator(&self, size_mm: u16) -> bool {
        self.radiator_mounts
            .iter()
            .any(|m| m.sizes_mm.contains(&size_mm))
    }
}
