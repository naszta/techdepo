//! How products look: the product tile (photo or category icon, design.md "Product imagery"), key specs
//! for listings, the full spec table, and the product card.

use dioxus::prelude::*;
use techdepo_api_types::{Category, Product};
use techdepo_catalog::spec::{CoolerKind, ProductSpec, StorageInterface};

use crate::routes::Route;
use crate::state::{use_build, use_stock};
use crate::ui::badges::{FitBadge, StockBadge};
use crate::ui::icons::{Icon, IconName};
use crate::ui::money::MoneyText;

/// The icon for a category.
pub fn category_icon(category: Category) -> IconName {
    match category {
        Category::Cpu => IconName::Cpu,
        Category::Motherboard => IconName::CircuitBoard,
        Category::Memory => IconName::MemoryStick,
        Category::Storage => IconName::HardDrive,
        Category::Gpu => IconName::Gpu,
        Category::Case => IconName::PcCase,
        Category::Psu => IconName::PlugZap,
        Category::Cooler => IconName::Fan,
    }
}

/// Plural, for headings and menus ("CPUs", "Graphics cards").
pub fn category_plural(category: Category) -> &'static str {
    match category {
        Category::Cpu => "CPUs",
        Category::Motherboard => "Motherboards",
        Category::Memory => "Memory",
        Category::Storage => "Storage",
        Category::Gpu => "Graphics cards",
        Category::Case => "Cases",
        Category::Psu => "Power supplies",
        Category::Cooler => "CPU coolers",
    }
}

/// The generated product photo (`public/products/<id>.webp`).
pub fn product_image(product: &Product) -> Option<String> {
    Some(format!("/products/{}.webp", product.id))
}

/// A category icon on a tinted tile (or the product photo when given), with the brand wordmark
/// when given.
#[component]
pub fn ProductTile(
    category: Category,
    #[props(default)] brand: Option<String>,
    #[props(default)] image: Option<String>,
    #[props(default = "aspect-[4/3]".to_string())] class: String,
    #[props(default = "size-10".to_string())] icon_class: String,
) -> Element {
    rsx! {
        div { class: "relative flex items-center justify-center overflow-hidden rounded-box bg-primary/8 text-primary {class}",
            if let Some(src) = image {
                img { src, alt: "", loading: "lazy", class: "size-full bg-white object-contain" }
            } else {
                Icon { name: category_icon(category), class: "{icon_class} stroke-[1.5]" }
            }
            if let Some(brand) = brand {
                span { class: "absolute bottom-2 left-3 text-[0.65rem] font-semibold uppercase tracking-[0.18em] text-primary/70",
                    "{brand}"
                }
            }
        }
    }
}

/// `1 TB`, `512 GB`, `1.5 TB`.
pub fn capacity(gb: u32) -> String {
    if gb >= 1000 {
        let tb = f64::from(gb) / 1000.0;
        if gb.is_multiple_of(1000) {
            format!("{tb:.0} TB")
        } else {
            format!("{tb:.1} TB")
        }
    } else {
        format!("{gb} GB")
    }
}

fn interface(interface: StorageInterface) -> String {
    match interface {
        StorageInterface::Nvme { pcie_gen } => format!("NVMe {pcie_gen}"),
        StorageInterface::Sata => "SATA".to_owned(),
    }
}

fn join_labels<T: std::fmt::Display>(items: &[T]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn yes_no(value: bool) -> String {
    if value { "Yes" } else { "No" }.to_owned()
}

/// Two or three specs that matter most, for cards and lists.
pub fn key_specs(product: &Product) -> Vec<String> {
    match &product.spec {
        ProductSpec::Cpu(s) => vec![
            format!("{} cores / {} threads", s.cores, s.threads),
            s.socket.to_string(),
            format!("up to {:.1} GHz", f64::from(s.boost_clock_mhz) / 1000.0),
        ],
        ProductSpec::Motherboard(s) => vec![
            format!("{} {}", s.socket, s.chipset),
            s.form_factor.to_string(),
            s.memory_type.to_string(),
        ],
        ProductSpec::Memory(s) => vec![
            format!("{} GB ({}×{} GB)", s.total_gb(), s.modules, s.module_gb),
            format!("{}-{}", s.memory_type, s.speed_mts),
            format!("CL{}", s.cas_latency),
        ],
        ProductSpec::Storage(s) => vec![
            capacity(s.capacity_gb),
            interface(s.interface),
            s.form_factor.to_string(),
        ],
        ProductSpec::Gpu(s) => vec![
            format!("{} GB VRAM", s.vram_gb),
            format!("{} W", s.board_power_w),
            format!("{} mm", s.length_mm),
        ],
        ProductSpec::Case(s) => vec![
            s.size.to_string(),
            format!("GPUs up to {} mm", s.max_gpu_length_mm),
            join_labels(&s.motherboard_form_factors),
        ],
        ProductSpec::Psu(s) => vec![
            format!("{} W", s.wattage_w),
            s.efficiency.to_string(),
            s.modular.to_string(),
        ],
        ProductSpec::Cooler(s) => vec![
            match s.kind {
                CoolerKind::Air { height_mm, .. } => format!("Air, {height_mm} mm tall"),
                CoolerKind::Aio { radiator_mm } => format!("AIO, {radiator_mm} mm radiator"),
            },
            format!("Rated {} W", s.tdp_rating_w),
        ],
    }
}

/// Every spec as `(label, value)` rows for the Specs tab and compare table.
pub fn spec_rows(product: &Product) -> Vec<(&'static str, String)> {
    let mut rows = match &product.spec {
        ProductSpec::Cpu(s) => vec![
            ("Socket", s.socket.to_string()),
            ("Family", s.family.to_string()),
            ("Cores / threads", format!("{} / {}", s.cores, s.threads)),
            (
                "Base clock",
                format!("{:.1} GHz", f64::from(s.base_clock_mhz) / 1000.0),
            ),
            (
                "Boost clock",
                format!("{:.1} GHz", f64::from(s.boost_clock_mhz) / 1000.0),
            ),
            ("TDP", format!("{} W", s.tdp_w)),
            ("Sustained power", format!("{} W", s.peak_power_w)),
            (
                "Memory",
                s.memory
                    .iter()
                    .map(|m| format!("{} up to {} MT/s", m.memory_type, m.max_speed_mts))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            ("Integrated graphics", yes_no(s.integrated_graphics)),
            ("Cooler in the box", yes_no(s.bundled_cooler)),
        ],
        ProductSpec::Motherboard(s) => vec![
            ("Socket", s.socket.to_string()),
            ("Chipset", s.chipset.to_string()),
            ("Form factor", s.form_factor.to_string()),
            (
                "Memory",
                format!("{}, {} slots", s.memory_type, s.memory_slots),
            ),
            ("Max memory", format!("{} GB", s.max_memory_gb)),
            (
                "Max memory speed",
                format!("{} MT/s", s.max_memory_speed_mts),
            ),
            (
                "M.2 slots",
                s.m2_slots
                    .iter()
                    .map(|m| format!("{} {} ({} mm)", m.key, m.pcie_gen, m.max_length_mm))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            ("SATA ports", s.sata_ports.to_string()),
            ("Graphics slot", s.pcie_x16_gen.to_string()),
            ("BIOS version", s.bios_version.to_string()),
            ("BIOS flashback", yes_no(s.bios_flashback)),
            (
                "CPU power",
                format!("{}× 8-pin + {}× 4-pin", s.eps_8pin, s.eps_4pin),
            ),
            ("Wi-Fi", yes_no(s.wifi)),
        ],
        ProductSpec::Memory(s) => vec![
            ("Type", s.memory_type.to_string()),
            (
                "Capacity",
                format!("{} GB ({}×{} GB)", s.total_gb(), s.modules, s.module_gb),
            ),
            ("Speed", format!("{} MT/s", s.speed_mts)),
            ("CAS latency", format!("CL{}", s.cas_latency)),
            ("Module height", format!("{} mm", s.height_mm)),
        ],
        ProductSpec::Storage(s) => vec![
            ("Capacity", capacity(s.capacity_gb)),
            ("Form factor", s.form_factor.to_string()),
            ("Interface", interface(s.interface)),
            ("Sequential read", format!("{} MB/s", s.read_mbps)),
            ("Sequential write", format!("{} MB/s", s.write_mbps)),
        ],
        ProductSpec::Gpu(s) => vec![
            ("VRAM", format!("{} GB", s.vram_gb)),
            ("Length", format!("{} mm", s.length_mm)),
            ("Thickness", format!("{} slots", s.slot_width)),
            ("Board power", format!("{} W", s.board_power_w)),
            (
                "Power connectors",
                match (
                    s.power_connectors.pcie_8pin,
                    s.power_connectors.twelve_v_2x6,
                ) {
                    (0, 0) => "None (slot power)".to_owned(),
                    (n, 0) => format!("{n}× 8-pin PCIe"),
                    (0, m) => format!("{m}× 12V-2x6"),
                    (n, m) => format!("{n}× 8-pin PCIe + {m}× 12V-2x6"),
                },
            ),
            ("Interface", s.pcie_gen.to_string()),
            ("Recommended PSU", format!("{} W", s.recommended_psu_w)),
        ],
        ProductSpec::Case(s) => vec![
            ("Size", s.size.to_string()),
            ("Motherboards", join_labels(&s.motherboard_form_factors)),
            ("Max GPU length", format!("{} mm", s.max_gpu_length_mm)),
            ("Max GPU thickness", format!("{} slots", s.max_gpu_slots)),
            (
                "Max cooler height",
                format!("{} mm", s.max_cooler_height_mm),
            ),
            ("Power supplies", join_labels(&s.psu_form_factors)),
            ("Max PSU length", format!("{} mm", s.max_psu_length_mm)),
            (
                "Radiators",
                if s.radiator_mounts.is_empty() {
                    "None".to_owned()
                } else {
                    s.radiator_mounts
                        .iter()
                        .map(|m| {
                            let sizes = m
                                .sizes_mm
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join("/");
                            format!("{} {sizes} mm", m.position)
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            ),
            (
                "Drive bays",
                format!("{}× 2.5\", {}× 3.5\"", s.bays_2_5, s.bays_3_5),
            ),
            ("Fans included", s.included_fans.to_string()),
        ],
        ProductSpec::Psu(s) => vec![
            ("Wattage", format!("{} W", s.wattage_w)),
            ("Form factor", s.form_factor.to_string()),
            ("Length", format!("{} mm", s.length_mm)),
            ("Efficiency", s.efficiency.to_string()),
            ("Cables", s.modular.to_string()),
            (
                "Connectors",
                format!(
                    "{}× EPS 8-pin, {}× PCIe 8-pin, {}× 12V-2x6, {}× SATA",
                    s.connectors.eps_8pin,
                    s.connectors.pcie_8pin,
                    s.connectors.twelve_v_2x6,
                    s.connectors.sata
                ),
            ),
        ],
        ProductSpec::Cooler(s) => {
            let mut rows = vec![("Sockets", join_labels(&s.sockets))];
            match s.kind {
                CoolerKind::Air {
                    height_mm,
                    ram_clearance_mm,
                } => {
                    rows.push(("Type", "Air tower".to_owned()));
                    rows.push(("Height", format!("{height_mm} mm")));
                    if let Some(clearance) = ram_clearance_mm {
                        rows.push(("Memory clearance", format!("{clearance} mm")));
                    }
                }
                CoolerKind::Aio { radiator_mm } => {
                    rows.push(("Type", "All-in-one liquid".to_owned()));
                    rows.push(("Radiator", format!("{radiator_mm} mm")));
                }
            }
            rows.push(("Rated for", format!("{} W", s.tdp_rating_w)));
            rows
        }
    };
    rows.insert(0, ("Brand", product.brand.clone()));
    rows.push(("Tier", product.tier.label().to_owned()));
    rows
}

/// A product in a grid: tile, name, key specs, price, and fit/stock badges.
#[component]
pub fn ProductCard(
    product: &'static Product,
    #[props(default)] compare: Option<Element>,
) -> Element {
    let build = use_build();
    let stock = use_stock();
    let fit = build.fit(product);
    let stock = stock.get(&product.id);
    let specs = key_specs(product);
    let id = product.id.to_string();

    rsx! {
        article { class: "card card-border bg-base-100 border-base-300 transition-colors hover:border-primary/40",
            Link { to: Route::Product { id: id.clone() }, class: "block p-3 pb-0",
                // A one-column phone grid gets a flatter tile: the icon is decoration, not content.
                ProductTile { category: product.category, brand: Some(product.brand.clone()), image: product_image(product), class: "aspect-[2/1] sm:aspect-[4/3]" }
            }
            div { class: "card-body gap-2 p-4",
                Link { to: Route::Product { id }, class: "font-semibold leading-snug hover:text-primary",
                    "{product.name}"
                }
                ul { class: "text-sm text-base-content/70 flex flex-col gap-0.5",
                    for spec in specs {
                        li { "{spec}" }
                    }
                }
                div { class: "flex flex-wrap gap-1.5 min-h-6",
                    if let Some(fit) = fit {
                        FitBadge { fit, small: true }
                    }
                    if let Some(stock) = stock {
                        StockBadge { stock }
                    }
                }
                div { class: "mt-auto flex items-center justify-between pt-2",
                    MoneyText { amount: product.price, class: "text-lg font-semibold" }
                    {compare}
                }
            }
        }
    }
}
