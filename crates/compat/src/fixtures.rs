//! Handcrafted products for unit tests. Every default part is compatible with every other
//! default part, so a test changes exactly the field its rule reads.

use techdepo_catalog::spec::{
    CaseSize, CaseSpec, CoolerKind, CoolerSpec, CpuSpec, CpuSupport, Efficiency,
    GpuPowerConnectors, GpuSpec, M2Key, M2Slot, MemorySpec, MemorySupport, Modularity,
    MotherboardSpec, PsuConnectors, PsuSpec, RadiatorMount, RadiatorPosition, StorageFormFactor,
    StorageInterface, StorageSpec,
};
use techdepo_catalog::{
    Chipset, CpuFamily, FormFactor, MemoryType, PcieGen, Product, ProductSpec, PsuFormFactor,
    Socket, Tier,
};
use techdepo_domain::{Money, ProductId, Sku};

use crate::{Build, Report, RuleId, check_build};

/// A €100 product named after its id.
pub fn product(id: &str, spec: ProductSpec) -> Product {
    Product {
        id: ProductId::new(id),
        sku: Sku::new(id.to_uppercase()),
        brand: "Test".to_owned(),
        name: id.to_owned(),
        category: spec.category(),
        price: Money::from_euros(100),
        default_stock: 10,
        tier: Tier::Mid,
        tags: Vec::new(),
        summary: String::new(),
        spec,
    }
}

/// `product` with another price.
pub fn priced(mut product: Product, euros: i64) -> Product {
    product.price = Money::from_euros(euros);
    product
}

pub fn cpu_spec() -> CpuSpec {
    CpuSpec {
        socket: Socket::Am5,
        family: CpuFamily::Obsidian,
        cores: 6,
        threads: 12,
        base_clock_mhz: 3800,
        boost_clock_mhz: 5400,
        tdp_w: 65,
        peak_power_w: 88,
        memory: vec![MemorySupport {
            memory_type: MemoryType::Ddr5,
            max_speed_mts: 5600,
            max_speed_2dpc_mts: 3600,
        }],
        integrated_graphics: true,
        // Boxed cooler, so a CPU alone raises no "missing cooler" warning.
        bundled_cooler: true,
    }
}

pub fn board_spec() -> MotherboardSpec {
    let m2 = |sata| M2Slot {
        key: M2Key::M,
        max_length_mm: 80,
        pcie_gen: PcieGen::GEN4,
        sata,
    };
    MotherboardSpec {
        socket: Socket::Am5,
        chipset: Chipset::B650,
        form_factor: FormFactor::Atx,
        memory_type: MemoryType::Ddr5,
        memory_slots: 4,
        max_memory_gb: 128,
        max_memory_speed_mts: 6400,
        m2_slots: vec![m2(false), m2(true)],
        sata_ports: 4,
        pcie_x16_gen: PcieGen::GEN4,
        bios_version: 20,
        bios_flashback: true,
        cpu_support: vec![
            CpuSupport {
                family: CpuFamily::Quartz,
                min_bios: 1,
            },
            CpuSupport {
                family: CpuFamily::Obsidian,
                min_bios: 1,
            },
        ],
        eps_8pin: 1,
        eps_4pin: 0,
        wifi: false,
    }
}

pub fn memory_spec() -> MemorySpec {
    MemorySpec {
        memory_type: MemoryType::Ddr5,
        modules: 2,
        module_gb: 16,
        speed_mts: 5600,
        cas_latency: 36,
        height_mm: 35,
    }
}

pub fn storage_spec() -> StorageSpec {
    StorageSpec {
        form_factor: StorageFormFactor::M2_2280,
        interface: StorageInterface::Nvme {
            pcie_gen: PcieGen::GEN4,
        },
        capacity_gb: 1000,
        read_mbps: 7000,
        write_mbps: 6000,
    }
}

pub fn gpu_spec() -> GpuSpec {
    GpuSpec {
        length_mm: 250,
        slot_width: 2.0,
        board_power_w: 200,
        power_connectors: GpuPowerConnectors {
            pcie_8pin: 1,
            twelve_v_2x6: 0,
        },
        pcie_gen: PcieGen::GEN4,
        vram_gb: 8,
        recommended_psu_w: 550,
    }
}

pub fn case_spec() -> CaseSpec {
    CaseSpec {
        size: CaseSize::MidTower,
        motherboard_form_factors: vec![FormFactor::Atx, FormFactor::MicroAtx, FormFactor::MiniItx],
        max_gpu_length_mm: 350,
        max_gpu_slots: 4,
        max_cooler_height_mm: 165,
        psu_form_factors: vec![PsuFormFactor::Atx],
        max_psu_length_mm: 200,
        radiator_mounts: vec![
            RadiatorMount {
                position: RadiatorPosition::Front,
                sizes_mm: vec![240, 280, 360],
            },
            RadiatorMount {
                position: RadiatorPosition::Top,
                sizes_mm: vec![240],
            },
        ],
        bays_2_5: 2,
        bays_3_5: 2,
        included_fans: 2,
    }
}

pub fn psu_spec() -> PsuSpec {
    PsuSpec {
        wattage_w: 750,
        form_factor: PsuFormFactor::Atx,
        length_mm: 150,
        efficiency: Efficiency::Gold,
        modular: Modularity::FullyModular,
        connectors: PsuConnectors {
            eps_8pin: 2,
            pcie_8pin: 3,
            twelve_v_2x6: 1,
            sata: 6,
        },
    }
}

pub fn cooler_spec() -> CoolerSpec {
    CoolerSpec {
        sockets: vec![Socket::Am4, Socket::Am5, Socket::Lga1700, Socket::Lga1851],
        kind: CoolerKind::Air {
            height_mm: 150,
            ram_clearance_mm: None,
        },
        tdp_rating_w: 200,
    }
}

macro_rules! fixture {
    ($($fn:ident: $spec:ident -> $variant:ident($ty:ty)),+ $(,)?) => {$(
        #[doc = concat!("A `", stringify!($variant), "` from the default spec, adjusted by `edit`.")]
        pub fn $fn(id: &str, edit: impl FnOnce(&mut $ty)) -> Product {
            let mut spec = $spec();
            edit(&mut spec);
            product(id, ProductSpec::$variant(spec))
        }
    )+};
}

fixture! {
    cpu_product: cpu_spec -> Cpu(CpuSpec),
    board_product: board_spec -> Motherboard(MotherboardSpec),
    memory_product: memory_spec -> Memory(MemorySpec),
    storage_product: storage_spec -> Storage(StorageSpec),
    gpu_product: gpu_spec -> Gpu(GpuSpec),
    case_product: case_spec -> Case(CaseSpec),
    psu_product: psu_spec -> Psu(PsuSpec),
    cooler_product: cooler_spec -> Cooler(CoolerSpec),
}

/// One default part per category (one memory kit and one drive).
pub fn default_parts() -> Vec<Product> {
    vec![
        cpu_product("cpu", |_| {}),
        board_product("board", |_| {}),
        memory_product("ram", |_| {}),
        storage_product("ssd", |_| {}),
        gpu_product("gpu", |_| {}),
        case_product("case", |_| {}),
        psu_product("psu", |_| {}),
        cooler_product("cooler", |_| {}),
    ]
}

/// Checks a build of `products`.
pub fn check(products: &[&Product]) -> Report {
    check_build(&Build::from_products(products.iter().copied()).expect("valid build"))
}

/// The distinct rules that reported issues.
pub fn fired(report: &Report) -> Vec<RuleId> {
    let mut rules: Vec<RuleId> = report.issues.iter().map(|i| i.rule).collect();
    rules.dedup();
    rules
}

/// The single issue `rule` reported.
#[track_caller]
pub fn only_issue(report: &Report, rule: RuleId) -> &crate::Issue {
    let issues: Vec<_> = report.issues.iter().filter(|i| i.rule == rule).collect();
    assert_eq!(issues.len(), 1, "{rule:?} issues: {:#?}", report.issues);
    issues[0]
}
