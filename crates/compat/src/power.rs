//! Power draw estimate: the sum of component draws plus a 10 % margin.
//!
//! The CPU's sustained peak and the GPU's board power come from the catalog; everything else
//! gets a fixed allowance. The estimate is a planning figure for PSU sizing, not a measurement.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use techdepo_catalog::Category;
use techdepo_catalog::spec::{CoolerKind, StorageFormFactor, StorageInterface};
use techdepo_domain::ProductId;

use crate::build::Build;

/// Motherboard, chipset and onboard devices.
pub const MOTHERBOARD_W: u32 = 50;
/// Per memory module (a 2-module kit draws twice this).
pub const MEMORY_MODULE_W: u32 = 5;
/// NVMe SSD under load.
pub const NVME_W: u32 = 8;
/// SATA SSD (M.2 or 2.5").
pub const SATA_SSD_W: u32 = 5;
/// 3.5" hard drive.
pub const HDD_W: u32 = 10;
/// Per fan included with the case.
pub const CASE_FAN_W: u32 = 3;
/// Air cooler fans.
pub const AIR_COOLER_W: u32 = 5;
/// AIO pump plus radiator fans.
pub const AIO_COOLER_W: u32 = 12;
/// Safety margin on top of the component sum, in percent.
pub const MARGIN_PCT: u32 = 10;
/// Below this much spare PSU capacity (percent of the PSU's rating) the build gets a warning.
pub const HEADROOM_WARNING_PCT: i32 = 20;
/// A CPU drawing more than this (sustained peak) wants every 8-pin CPU power header connected.
pub const HIGH_POWER_CPU_W: u32 = 200;

/// One part's contribution to the estimate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PowerDraw {
    /// The part drawing the power.
    pub part: ProductId,
    /// Its category.
    pub category: Category,
    /// Estimated draw in watts.
    pub watts: u32,
}

/// The build's estimated power draw.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PowerEstimate {
    /// Per-part draws, in slot order (the PSU itself is not listed).
    pub draws: Vec<PowerDraw>,
    /// Sum of `draws`.
    pub components_w: u32,
    /// `components_w` plus [`MARGIN_PCT`], rounded up.
    pub total_w: u32,
}

impl PowerEstimate {
    /// Estimates the draw of every part in `build`.
    pub fn of(build: &Build<'_>) -> Self {
        let draws: Vec<PowerDraw> = build
            .parts()
            .into_iter()
            .filter_map(|(_, product)| {
                let watts = part_draw_w(build, &product.id, product.category)?;
                Some(PowerDraw {
                    part: product.id.clone(),
                    category: product.category,
                    watts,
                })
            })
            .collect();
        let components_w = draws.iter().map(|d| d.watts).sum();
        Self {
            draws,
            components_w,
            total_w: with_margin(components_w),
        }
    }

    /// Spare capacity of a `psu_w` PSU over this estimate, in percent of `psu_w` (negative
    /// when overloaded).
    pub fn headroom_pct(&self, psu_w: u32) -> i32 {
        if psu_w == 0 {
            return i32::MIN;
        }
        let spare = i64::from(psu_w) - i64::from(self.total_w);
        // Floor, so 19.9 % reads as 19 % and still warns.
        (spare * 100).div_euclid(i64::from(psu_w)) as i32
    }
}

/// `watts` plus [`MARGIN_PCT`], rounded up.
pub const fn with_margin(watts: u32) -> u32 {
    (watts * (100 + MARGIN_PCT)).div_ceil(100)
}

fn part_draw_w(build: &Build<'_>, id: &ProductId, category: Category) -> Option<u32> {
    let watts = match category {
        Category::Cpu => u32::from(build.cpu()?.spec.peak_power_w),
        Category::Motherboard => MOTHERBOARD_W,
        Category::Gpu => u32::from(build.gpu()?.spec.board_power_w),
        Category::Memory => {
            let kit = build.memory().iter().find(|k| k.id() == id)?;
            u32::from(kit.spec.modules) * MEMORY_MODULE_W
        }
        Category::Storage => {
            let drive = build.storage().iter().find(|d| d.id() == id)?;
            match (drive.spec.form_factor, drive.spec.interface) {
                (StorageFormFactor::Sata35, _) => HDD_W,
                (_, StorageInterface::Nvme { .. }) => NVME_W,
                (_, StorageInterface::Sata) => SATA_SSD_W,
            }
        }
        Category::Case => u32::from(build.case()?.spec.included_fans) * CASE_FAN_W,
        Category::Cooler => match build.cooler()?.spec.kind {
            CoolerKind::Air { .. } => AIR_COOLER_W,
            CoolerKind::Aio { .. } => AIO_COOLER_W,
        },
        Category::Psu => return None,
    };
    Some(watts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::*;

    #[test]
    fn margin_rounds_up() {
        assert_eq!(with_margin(0), 0);
        assert_eq!(with_margin(100), 110);
        assert_eq!(with_margin(101), 112);
    }

    #[test]
    fn sums_every_part_but_the_psu() {
        let (cpu, board, kit, nvme, hdd, gpu, case, psu, cooler) = (
            cpu_product("cpu", |s| s.peak_power_w = 100),
            board_product("mb", |_| {}),
            memory_product("kit", |s| s.modules = 2),
            storage_product("nvme", |_| {}),
            storage_product("hdd", |s| {
                s.form_factor = StorageFormFactor::Sata35;
                s.interface = StorageInterface::Sata;
            }),
            gpu_product("gpu", |s| s.board_power_w = 200),
            case_product("case", |s| s.included_fans = 2),
            psu_product("psu", |_| {}),
            cooler_product("cooler", |s| s.kind = CoolerKind::Aio { radiator_mm: 240 }),
        );
        let build = Build::from_products([
            &cpu, &board, &kit, &kit, &nvme, &hdd, &gpu, &case, &psu, &cooler,
        ])
        .unwrap();
        let estimate = PowerEstimate::of(&build);
        let expected = 100
            + MOTHERBOARD_W
            + 2 * 2 * MEMORY_MODULE_W
            + NVME_W
            + HDD_W
            + 200
            + 2 * CASE_FAN_W
            + AIO_COOLER_W;
        assert_eq!(estimate.components_w, expected);
        assert_eq!(estimate.total_w, with_margin(expected));
        assert_eq!(estimate.draws.len(), 9);
    }

    #[test]
    fn headroom_is_floored_share_of_psu() {
        let estimate = PowerEstimate {
            draws: vec![],
            components_w: 0,
            total_w: 401,
        };
        assert_eq!(estimate.headroom_pct(500), 19);
        assert_eq!(estimate.headroom_pct(400), -1);
        assert_eq!(estimate.headroom_pct(1000), 59);
    }
}
