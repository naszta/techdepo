# Catalog data

The product catalog is compiled into the binaries from this directory by
`crates/catalog` (its `build.rs` embeds every file with `include_str!`). There is
no database table for products. Only per-org stock lives in D1, seeded from
`default_stock`.

```
data/catalog/
  cpu.yaml  motherboard.yaml  memory.yaml  storage.yaml
  gpu.yaml  case.yaml  psu.yaml  cooler.yaml        # one list of products per category
  content/<product-id>.md                           # rich content, one file per product
```

- The file stem is the category, so a product never states its category.
- New files are picked up automatically: there is no list to update.
- The catalog hash (`CATALOG_VERSION`) covers every file, so clients reload
  after any change.
- Check your work with `cargo test -p techdepo-catalog` (runs `Catalog::validate`
  on the embedded data) or `cargo run -p xtask -- catalog validate`.

Parsing is strict. Unknown keys are errors, every key listed below is required
(there are no defaults; the one exception is a cooler's `ram_clearance_mm`),
and enum values must be spelled exactly as listed.

## Product fields (every category)

```yaml
- id: voltaris-v5-8600          # unique; lower-case words of [a-z0-9] joined by "-"
  sku: CPU-VOL-8600             # unique; upper-case words of [A-Z0-9] joined by "-"
  brand: Voltaris               # fictional brand
  name: Voltaris V5 8600        # unique; full display name, must start with the brand
  price_cents: 22990            # integer euro cents, VAT included (€229.90); 100..=1000000
  default_stock: 25             # units seeded into each new org; 0..=500
  tier: mid                     # budget | mid | high
  tags: [gaming, am5]           # lower-case slugs, no duplicates; may be []
  summary: One sentence …       # 20..=200 characters, shown in listings
  spec: { … }                   # category-specific, see below
```

- `id` is also the content file name: `content/voltaris-v5-8600.md`.
- Prices are never floats. `229.90` is a parse error, so write `22990`.
- Units are part of the key name: `_mm`, `_w` (watts), `_mhz`, `_mts`
  (MT/s), `_gb`, `_mbps` (MB/s).
- YAML flow style (`{ a: 1, b: 2 }`) and block style are interchangeable.

### Shared enum values

| Type | Values |
|---|---|
| socket | `am4`, `am5`, `lga1700`, `lga1851` |
| memory type | `ddr4`, `ddr5` |
| motherboard form factor | `e_atx`, `atx`, `micro_atx`, `mini_itx` |
| PSU form factor | `atx`, `sfx` |
| PCIe generation | integer `3`, `4` or `5` |

**CPU families.** Fictional codenames, each bound to one socket:

| Family | Socket | Memory |
|---|---|---|
| `granite` | am4 | DDR4 |
| `quartz` | am5 (1st gen) | DDR5 |
| `obsidian` | am5 (2nd gen; early AM5 boards need a BIOS update) | DDR5 |
| `falcon` | lga1700 (1st gen) | DDR4 or DDR5 |
| `harrier` | lga1700 (2nd gen; early boards need a BIOS update) | DDR4 or DDR5 |
| `osprey` | lga1851 | DDR5 |

**Chipsets per socket.**

| Socket | Chipsets |
|---|---|
| am4 | `a520`, `b550`, `x570` |
| am5 | `a620`, `b650`, `b650e`, `x670e`, `x870`, `x870e` |
| lga1700 | `h610`, `b760`, `z790` |
| lga1851 | `b860`, `z890` |

**Memory platforms.** am4 runs DDR4 only. am5 and lga1851 run DDR5 only.
lga1700 runs either, but a given board supports only one of them.

## `cpu.yaml`

| Key | Type | Meaning / validation |
|---|---|---|
| `socket` | socket | must equal the family's socket |
| `family` | CPU family | see the table above |
| `cores` | int | 2..=32 |
| `threads` | int | `cores`..=2×`cores` |
| `base_clock_mhz` | int | 1000..=6000 |
| `boost_clock_mhz` | int | `base_clock_mhz`..=6500 |
| `tdp_w` | int | rated TDP as marketed; 15..=300; shown to customers |
| `peak_power_w` | int | sustained package power; `tdp_w`..=400; used for the PSU load estimate and compared against cooler `tdp_rating_w` |
| `memory` | list | non-empty, one entry per supported type: `{ memory_type, max_speed_mts, max_speed_2dpc_mts }`; types must be valid for the socket; `max_speed_mts` (two modules) within DDR4 2133..=4800 / DDR5 4800..=8800; `max_speed_2dpc_mts` (four modules, two per channel) 2133..=`max_speed_mts` |
| `integrated_graphics` | bool | without it, a build needs a GPU |
| `bundled_cooler` | bool | stock cooler in the box |

```yaml
- id: voltaris-v5-8600
  sku: CPU-VOL-8600
  brand: Voltaris
  name: Voltaris V5 8600
  price_cents: 22990
  default_stock: 25
  tier: mid
  tags: [gaming, am5]
  summary: Six fast Obsidian cores on AM5 with integrated graphics.
  spec:
    socket: am5
    family: obsidian
    cores: 6
    threads: 12
    base_clock_mhz: 3800
    boost_clock_mhz: 5400
    tdp_w: 65
    peak_power_w: 88
    memory:
      - { memory_type: ddr5, max_speed_mts: 5600, max_speed_2dpc_mts: 3600 }
    integrated_graphics: true
    bundled_cooler: false
```

An LGA1700 CPU lists both types:
`memory: [{ memory_type: ddr5, max_speed_mts: 5600, max_speed_2dpc_mts: 3600 }, { memory_type: ddr4, max_speed_mts: 3200, max_speed_2dpc_mts: 2933 }]`.
Every CPU here is dual-channel, so more than two modules means two per channel.

## `motherboard.yaml`

| Key | Type | Meaning / validation |
|---|---|---|
| `socket` | socket | must equal the chipset's socket |
| `chipset` | chipset | see the table above |
| `form_factor` | motherboard form factor | |
| `memory_type` | memory type | must be valid for the socket |
| `memory_slots` | int | 2..=8; `mini_itx` boards have exactly 2 |
| `max_memory_gb` | int | 32..=512 |
| `max_memory_speed_mts` | int | DDR4 2133..=4800 / DDR5 4800..=8800 |
| `m2_slots` | list | 0..=6 entries of `{ key, max_length_mm, pcie_gen, sata }` |
| ↳ `key` | `m` \| `e` | `m` = storage; `e` = Wi-Fi module only (never counts for drives) |
| ↳ `max_length_mm` | int | longest module: one of `30, 42, 60, 80, 110` (2230 … 22110) |
| ↳ `pcie_gen` | 3 \| 4 \| 5 | |
| ↳ `sata` | bool | also accepts M.2 SATA drives |
| `sata_ports` | int | 0..=8 |
| `pcie_x16_gen` | 3 \| 4 \| 5 | primary graphics slot |
| `bios_version` | int | BIOS revision the board ships with; 1..=999 |
| `bios_flashback` | bool | can update the BIOS without a CPU |
| `cpu_support` | list | non-empty `{ family, min_bios }`; each family must match the socket; families not listed are **unsupported**; `min_bios > bios_version` means "BIOS update needed" |
| `eps_8pin` | int | 8-pin CPU power headers; 1..=2 |
| `eps_4pin` | int | extra 4-pin CPU power headers; 0..=1 |
| `wifi` | bool | onboard Wi-Fi |

```yaml
- id: kestrel-b650m-wing
  sku: MB-KES-B650M-WING
  brand: Kestrel
  name: Kestrel B650M Wing
  price_cents: 15990
  default_stock: 18
  tier: mid
  tags: [am5, ddr5, wifi, micro-atx]
  summary: A compact AM5 board with DDR5, two Gen 4 M.2 slots, Wi-Fi and BIOS flashback.
  spec:
    socket: am5
    chipset: b650
    form_factor: micro_atx
    memory_type: ddr5
    memory_slots: 4
    max_memory_gb: 192
    max_memory_speed_mts: 7200
    m2_slots:
      - { key: m, max_length_mm: 80, pcie_gen: 4, sata: false }
      - { key: m, max_length_mm: 80, pcie_gen: 4, sata: true }
      - { key: e, max_length_mm: 30, pcie_gen: 3, sata: false }
    sata_ports: 4
    pcie_x16_gen: 4
    bios_version: 10
    bios_flashback: true
    cpu_support:
      - { family: quartz, min_bios: 1 }
      - { family: obsidian, min_bios: 16 }
    eps_8pin: 1
    eps_4pin: 1
    wifi: true
```

## `memory.yaml`

One product is one **kit**.

| Key | Type | Meaning / validation |
|---|---|---|
| `memory_type` | memory type | |
| `modules` | int | modules in the kit; 1..=4 |
| `module_gb` | int | per module; one of `8, 16, 24, 32, 48, 64` |
| `speed_mts` | int | rated (XMP/EXPO) speed; DDR4 2133..=4800 / DDR5 4800..=8800 |
| `cas_latency` | int | 10..=50 |
| `height_mm` | int | module height incl. heat spreader; 30..=70 |

```yaml
- id: nimbus-ddr5-6000-32
  sku: RAM-NIM-D5-6000-2X16
  brand: Nimbus
  name: Nimbus Stratus DDR5-6000 32 GB (2x16 GB)
  price_cents: 10990
  default_stock: 35
  tier: mid
  tags: [ddr5, gaming]
  summary: A 32 GB dual-channel DDR5 kit at 6000 MT/s with a low heat spreader.
  spec:
    memory_type: ddr5
    modules: 2
    module_gb: 16
    speed_mts: 6000
    cas_latency: 30
    height_mm: 34
```

## `storage.yaml`

| Key | Type | Meaning / validation |
|---|---|---|
| `form_factor` | `m2_2280` \| `"2.5in"` \| `"3.5in"` | quote the inch values; M.2 uses an M.2 slot, 2.5"/3.5" use a SATA port and a bay |
| `interface` | tagged map | `{ type: nvme, pcie_gen: 3\|4\|5 }` or `{ type: sata }`; 2.5"/3.5" drives must be `sata`; an `m2_2280` drive may be either |
| `capacity_gb` | int | 120..=24000 |
| `read_mbps` | int | 80..=interface ceiling (SATA 600, Gen3 3600, Gen4 7500, Gen5 14900) |
| `write_mbps` | int | same range as reads |

```yaml
- id: arcwave-a4-1tb
  sku: SSD-ARC-A4-1TB
  brand: Arcwave
  name: Arcwave A4 1 TB
  price_cents: 7490
  default_stock: 45
  tier: mid
  tags: [nvme, boot-drive]
  summary: A fast 1 TB PCIe 4.0 NVMe SSD that makes an excellent boot and game drive.
  spec:
    form_factor: m2_2280
    interface: { type: nvme, pcie_gen: 4 }
    capacity_gb: 1000
    read_mbps: 7000
    write_mbps: 6000
```

## `gpu.yaml`

| Key | Type | Meaning / validation |
|---|---|---|
| `length_mm` | int | 150..=360; compared against case `max_gpu_length_mm` |
| `slot_width` | number | 1.0..=4.0 in steps of 0.5 (e.g. `2.0`, `2.5`) |
| `board_power_w` | int | 30..=600; used for the PSU load estimate |
| `power_connectors` | map | `{ pcie_8pin: 0..=4, twelve_v_2x6: 0..=1 }`; must be able to deliver `board_power_w` (75 W slot + 150 W per 8-pin + 600 W per 12V-2x6) |
| `pcie_gen` | 3 \| 4 \| 5 | |
| `vram_gb` | int | 4..=32 |
| `recommended_psu_w` | int | max(300, `board_power_w`)..=1200 |

```yaml
- id: halcyon-hx6060-dual
  sku: GPU-HAL-HX6060-D
  brand: Halcyon
  name: Halcyon HX 6060 Dual
  price_cents: 32990
  default_stock: 15
  tier: budget
  tags: [1080p, compact]
  summary: A compact two-fan card for high-refresh 1080p gaming on a single 8-pin connector.
  spec:
    length_mm: 242
    slot_width: 2.0
    board_power_w: 165
    power_connectors: { pcie_8pin: 1, twelve_v_2x6: 0 }
    pcie_gen: 4
    vram_gb: 8
    recommended_psu_w: 550
```

## `case.yaml`

| Key | Type | Meaning / validation |
|---|---|---|
| `size` | `full_tower` \| `mid_tower` \| `mini_tower` \| `small_form_factor` | |
| `motherboard_form_factors` | list of form factor | non-empty, no duplicates; list **every** size that fits |
| `max_gpu_length_mm` | int | 150..=450 |
| `max_gpu_slots` | int | thickest graphics card in expansion slots; 1..=10; compared against gpu `slot_width` |
| `max_cooler_height_mm` | int | air cooler clearance; 30..=190 |
| `psu_form_factors` | list of PSU form factor | non-empty, no duplicates; the form factors that fit as they are. An SFX unit in a case that lists only `atx` needs an SFX-to-ATX bracket (a warning); a case that ships with the bracket lists both |
| `max_psu_length_mm` | int | 100..=250 |
| `radiator_mounts` | list | `{ position, sizes_mm }`; each position at most once; `position` is `front` \| `top` \| `rear` \| `side` \| `bottom`; `sizes_mm` lists every radiator size that fits there, from `120, 140, 240, 280, 360, 420`; may be `[]` for cases without mounts |
| `bays_2_5` | int | dedicated 2.5" bays; 0..=10 |
| `bays_3_5` | int | 3.5" bays (each also takes a 2.5" drive); 0..=10 |
| `included_fans` | int | 0..=10 |

```yaml
- id: tessel-atlas-mid
  sku: CASE-TES-ATLAS
  brand: Tessel
  name: Tessel Atlas Mid Tower
  price_cents: 9990
  default_stock: 20
  tier: mid
  tags: [airflow, atx]
  summary: A high-airflow ATX mid tower with room for 360 mm radiators and long graphics cards.
  spec:
    size: mid_tower
    motherboard_form_factors: [atx, micro_atx, mini_itx]
    max_gpu_length_mm: 360
    max_gpu_slots: 7
    max_cooler_height_mm: 165
    psu_form_factors: [atx]
    max_psu_length_mm: 200
    radiator_mounts:
      - { position: front, sizes_mm: [240, 280, 360] }
      - { position: top, sizes_mm: [240, 280] }
      - { position: rear, sizes_mm: [120] }
    bays_2_5: 2
    bays_3_5: 2
    included_fans: 3
```

## `psu.yaml`

| Key | Type | Meaning / validation |
|---|---|---|
| `wattage_w` | int | 450..=1200 |
| `form_factor` | PSU form factor | |
| `length_mm` | int | ATX 120..=220, SFX 100..=130 |
| `efficiency` | `bronze` \| `gold` \| `platinum` \| `titanium` | 80 PLUS rating |
| `modular` | `non_modular` \| `semi_modular` \| `fully_modular` | |
| `connectors` | map | `{ eps_8pin: 1..=3, pcie_8pin: 0..=8, twelve_v_2x6: 0..=2, sata: 2..=16 }` |

```yaml
- id: ferro-sf750
  sku: PSU-FER-SF750
  brand: Ferro
  name: Ferro SF750
  price_cents: 14990
  default_stock: 12
  tier: high
  tags: [sfx, gold]
  summary: A fully modular 750 W SFX unit with a native 12V-2x6 cable for small, powerful builds.
  spec:
    wattage_w: 750
    form_factor: sfx
    length_mm: 100
    efficiency: gold
    modular: fully_modular
    connectors: { eps_8pin: 2, pcie_8pin: 2, twelve_v_2x6: 1, sata: 4 }
```

## `cooler.yaml`

| Key | Type | Meaning / validation |
|---|---|---|
| `sockets` | list of socket | non-empty, no duplicates; sockets the mounting kit supports |
| `kind` | tagged map | `{ type: air, height_mm: 30..=190, ram_clearance_mm: 25..=70 }` or `{ type: aio, radiator_mm: 120\|140\|240\|280\|360\|420 }`; `ram_clearance_mm` is the tallest memory under the fan in its standard position, and the one optional key: leave it out when the cooler does not overhang the memory slots |
| `tdp_rating_w` | int | 65..=350; compared against CPU `peak_power_w` (not its TDP) |

```yaml
- id: breeze-glacier-240
  sku: COOL-BRZ-GLACIER240
  brand: Breeze
  name: Breeze Glacier 240
  price_cents: 9990
  default_stock: 15
  tier: mid
  tags: [aio, liquid]
  summary: A 240 mm all-in-one liquid cooler for hot CPUs and cases without tall-cooler clearance.
  spec:
    sockets: [am4, am5, lga1700, lga1851]
    kind: { type: aio, radiator_mm: 240 }
    tdp_rating_w: 250
```

## Which fields the compatibility rules read

When you design trap products, these are the fields that matter:

| Rule | Fields |
|---|---|
| CPU ↔ motherboard socket | cpu `socket`, motherboard `socket` |
| CPU supported / BIOS update needed | cpu `family`; motherboard `cpu_support`, `bios_version`, `bios_flashback` |
| Motherboard ↔ RAM type | motherboard `memory_type`, memory `memory_type` (and cpu `memory`) |
| RAM modules / capacity | memory `modules`, `module_gb`; motherboard `memory_slots`, `max_memory_gb` |
| RAM speed note | memory `speed_mts`, `modules`; motherboard `max_memory_speed_mts`; cpu `memory[].max_speed_mts`, `max_speed_2dpc_mts` (more than two modules in total) |
| Case ↔ motherboard | case `motherboard_form_factors`, motherboard `form_factor` |
| GPU length | gpu `length_mm`, case `max_gpu_length_mm` |
| GPU thickness | gpu `slot_width`, case `max_gpu_slots` |
| Air cooler height | cooler `kind.height_mm` (plus the fan raise for memory taller than `kind.ram_clearance_mm`), case `max_cooler_height_mm` |
| Memory clearance (warning) | cooler `kind.ram_clearance_mm`, memory `height_mm` |
| Cooler socket | cooler `sockets`, cpu `socket` |
| AIO radiator | cooler `kind.radiator_mm`, case `radiator_mounts` |
| PSU ↔ case (SFX in an ATX-only case: bracket warning) | psu `form_factor`, `length_mm`; case `psu_form_factors`, `max_psu_length_mm` |
| PSU wattage / headroom | cpu `peak_power_w`, gpu `board_power_w`, psu `wattage_w` |
| GPU power connectors | gpu `power_connectors`, psu `connectors` |
| M.2 / SATA count | storage `form_factor`, `interface`; motherboard `m2_slots`, `sata_ports` |
| Cooler capacity | cooler `tdp_rating_w`, cpu `peak_power_w` |
| CPU power connectors (warning) | motherboard `eps_8pin`, psu `connectors.eps_8pin`, cpu `peak_power_w` (above 200 W) |
| No video output | cpu `integrated_graphics`, presence of a GPU |
| No cooler | cpu `bundled_cooler`, presence of a cooler |

## Content files: `content/<product-id>.md`

Every product needs one, and every file needs a product. A content file has
exactly these five `##` sections, in this order, and nothing else at the top
level. Do not add a `#` title, because the page shows the product name.

```markdown
## Overview

Two or three paragraphs: what it is, the specs that matter and why, and any
caveat (BIOS update, clearance, connector).

## Who it's for

One short paragraph.

## Pros

- One bullet per line, starting with "- "
- At least one bullet

## Cons

- At least one bullet

## FAQ

### A question a customer would ask?

The answer, in one short paragraph.

### Another question?

At least one `###` question, each with a non-empty answer.
```

- The heading text must match exactly, including the ASCII apostrophe in
  `Who it's for`.
- Pros and cons are read as single-line `- ` bullets.
- FAQ entries are `### Question` headings, each followed by its answer.
- Keep claims consistent with the YAML specs, because agents quote this text
  to customers. Mention compatibility caveats explicitly. For example, the
  DDR4 kit's FAQ says it does not fit DDR5 boards.

## Tiers

`tier` follows fixed price bands per category, so a band check can be
automated. Storage is the exception: its tier follows the interface class,
because a large SATA drive can cost more than a small fast one.

| Category | budget | mid | high |
|---|---|---|---|
| cpu | < €200 | €200–€399.99 | ≥ €400 |
| motherboard | < €120 | €120–€249.99 | ≥ €250 |
| memory | < €90 | €90–€179.99 | ≥ €180 |
| gpu | < €400 | €400–€799.99 | ≥ €800 |
| case | < €80 | €80–€149.99 | ≥ €150 |
| psu | < €80 | €80–€139.99 | ≥ €140 |
| cooler | < €50 | €50–€119.99 | ≥ €120 |
| storage | SATA, HDD, PCIe 3.0 NVMe | PCIe 4.0 NVMe | PCIe 5.0 NVMe |

`Catalog::validate` checks every tier against this table (`Product::expected_tier`).
`xtask catalog validate` also checks that "an alternative in the same price range" always
exists:

- every category has at least two products in each tier;
- every platform has at least two products in each platform-bound category (CPUs, boards and
  coolers by socket, memory by DDR generation);
- every platform-bound product has another product on its platform in the same or an adjacent
  tier.

Two products per tier on *every* platform would need about 24 CPUs and 24 boards, so it is not
required.

Conventions the content relies on:

- ATX cases list only `atx` PSUs: none of them ships with an SFX bracket,
  so an SFX unit in them is a warning (get a bracket), not an error.
  The Lattice Pocket ITX includes one and lists both.
- GPUs ship without power adapters, so a 12V-2x6 card needs a PSU with a
  native 12V-2x6 cable.
- "Runs at X MT/s by default" means the lowest of the kit speed, the board
  maximum and the CPU's official speed, which is `max_speed_2dpc_mts` with
  more than two modules in the build. A faster profile is an overclock.
- Name products in full, or by a prefix only one product has ("Lumen Apex").
  "Halcyon HX 7070" (Twin or SFF?) or "Nimbus Stratus" fails validation:
  agents quote content, and their grounding check needs one product per name.
  Brand-level phrases ("Nimbus DDR5 kits", "Helion CPUs") are fine.

## Deliberate traps

Every compatibility rule has at least one failing combination in the data.
Agents, golden builds and the content files refer to these. `cargo run -p xtask -- catalog
validate` proves it: it searches small builds for a failing and a passing combination of every
rule and prints both.

| Rule | Failing combination | Passing alternative |
|---|---|---|
| CPU ↔ motherboard socket | `voltaris-v5-8600` + `kestrel-b550-glide` | `kestrel-b650-aero` |
| BIOS update, with flashback | `voltaris-v5-8600` + `kestrel-b650m-wing` (BIOS 10, needs 16) | `kestrel-b650-aero` (BIOS 22) |
| BIOS update, no flashback | `voltaris-v7-8700` + `meridian-a620m-lite` (BIOS 8, needs 15) | `voltaris-v5-7600` (Quartz) |
| CPU family not supported | `helion-h5-1440f` or `helion-h7-1470` (Harrier) + `meridian-h610m-base` (Falcon only, with any BIOS) | `kestrel-b760-vector` |
| Motherboard ↔ RAM type | `nimbus-ddr4-3200-16` + `kestrel-b650m-wing` (demo: "Kestrel B650 with my Nimbus DDR4 kit") | same price range (±20 %): `nimbus-ddr5-4800-16` (€44.90 vs €39.90); for `nimbus-ddr4-3200-32`, `nimbus-ddr5-5200-32` (€69.90 vs €64.90) |
| Motherboard ↔ RAM type (LGA1700) | `nimbus-ddr5-6000-32` + `meridian-b760m-forge-d4` | `kestrel-b760-vector` |
| CPU ↔ RAM type (no board yet) | `nimbus-ddr5-5600-16` + `voltaris-v5-5600` (DDR4 only); `nimbus-ddr4-3200-16` + `voltaris-v5-7600` (DDR5 only) | a kit of the CPU's type |
| RAM modules above slots | `nimbus-ddr5-6000-64` (4×16) + `kestrel-b650e-i-perch`; `nimbus-ddr4-3200-64` + `meridian-a520m-core` | `lumen-ddr5-6400-64` (2×32) |
| RAM capacity above maximum | `nimbus-ddr5-6400-96` + `meridian-a620m-lite` (max 64 GB) | `lumen-ddr5-6400-64` |
| RAM speed note | `lumen-ddr5-7200-32` + `voltaris-v5-8600` (5600) or + `meridian-a620m-lite` (6000); `lumen-ddr4-3600-32` + `voltaris-v5-5600` (3200); four modules: `nimbus-ddr5-6000-64` or 2× `nimbus-ddr5-6000-32` + `voltaris-v7-8700` (3600) | `nimbus-ddr5-5600-32`; for 64 GB, `lumen-ddr5-6400-64` (2×32) |
| Case ↔ motherboard | `kestrel-b650m-wing` + `tessel-nook-itx`; `kestrel-b550-glide` + `lattice-m1-mini` | `kestrel-b650e-i-perch` |
| GPU length | `halcyon-hx7070-twin` (301) + `tessel-nook-itx` (300); `halcyon-hx7080-triple` (336) + `lattice-air-400` (330) | `halcyon-hx7070-sff` (248) |
| GPU thickness | `halcyon-hx7070-sff`, `halcyon-hx6070-dual` or `aurex-nova-60` (2.5 slots) + `tessel-sliver-itx` (2 slots) | `halcyon-hx6060-dual` (2 slots) |
| Air cooler height | `breeze-frost-120` (155) + `tessel-nook-itx` (70); `coldmark-slim-67` + `tessel-sliver-itx` (58); `coldmark-peak-duo` (168) + `tessel-atlas-mid` (165); raised fan: `breeze-frost-120` + `lumen-ddr5-7200-32` (52 mm, so 165) + `lattice-core-200` (158) | `breeze-whisper-lp` (47); `nimbus-ddr5-6000-32` (34 mm) |
| Memory clearance (warning) | `lumen-ddr5-7200-32` (52) or `lumen-ddr5-6000-32` (46) under `breeze-frost-120` (42); any Nimbus DDR4 kit (42) under `breeze-frost-140-duo` (40) | a Nimbus DDR5 kit (32–34 mm), or an AIO |
| Cooler socket | `coldmark-classic-92` (AM4/AM5) + any `helion-*` CPU | `breeze-frost-120` |
| AIO radiator | `breeze-glacier-360` + `tessel-nook-itx`; `coldmark-hydro-420` + `tessel-atlas-mid`; `breeze-glacier-240` + `tessel-sliver-itx` (no mounts) | `breeze-glacier-240` + `tessel-nook-itx` |
| PSU form factor | `ferro-core-650` (ATX) + `tessel-nook-itx` | `ferro-sf750` + `tessel-nook-itx` |
| SFX bracket (warning) | `ferro-sf750` (SFX) + `tessel-atlas-mid` (ATX only, no bracket) | `ferro-sf750` + `lattice-pocket-itx` (bracket included) |
| PSU length | `ferro-sf850l` (130) + `tessel-sliver-itx` (100); `nordwatt-titan-1200` (190) + `lattice-m1-mini` (160) | `ferro-sf750`; `nordwatt-prime-850` |
| PSU wattage (error) | `ferro-core-550` + `helion-h7-1470` (253 W) + `aurex-nova-70` (263 W): 516 W before the rest and the 10 % | `ferro-core-750` |
| PSU headroom < 20 % (warning) | `nordwatt-prime-650` + `voltaris-v9-8950` + `halcyon-hx7070-twin`: 480 W from CPU and GPU | `nordwatt-prime-850` |
| GPU power connectors | `halcyon-hx7080-triple` (12V-2x6) + `ferro-core-650`; `aurex-nova-80` (3×8-pin) + `ferro-core-650` (2); `halcyon-hx7070-sff` + `ferro-sf600` | `nordwatt-prime-850`; `ferro-core-750`; `ferro-sf750` |
| M.2 drives above slots | two NVMe drives + `meridian-a520m-core` or `meridian-a620m-lite` (one slot); `lodestar-glide-1tb` (M.2 SATA) + `meridian-a620m-lite` (NVMe-only slot) | `kestrel-b650m-wing` |
| SATA drives above ports | `arcwave-s2-1tb` + `arcwave-s2-2tb` + `lodestar-vault-4tb` + `kestrel-b650e-i-perch` (2 ports) | `kestrel-b650-aero` (4 ports) |
| Drive bays | `lodestar-vault-4tb` (3.5") + `tessel-sliver-itx` or `tessel-nook-itx` (no 3.5" bays) | `lattice-pocket-itx` |
| SATA power | five SATA drives + `ferro-sf600` (4 SATA cables) | `ferro-core-550` (5) |
| Cooler below the CPU's sustained power | `breeze-whisper-lp` (95 W) + `voltaris-v7-8700` (162 W); `breeze-frost-120` (180 W) + `helion-h7-1470` (125 W TDP, 253 W sustained) | `breeze-frost-120` for the V7 8700; `coldmark-peak-duo` (270 W) for the H7 1470 |
| CPU power connectors (warning) | `voltaris-v9-8950` or `helion-h7-1470` (above 200 W) + `meridian-x870e-crest` or `meridian-z790-crest` (two EPS 8-pin) + `ferro-core-650` (one) | `nordwatt-prime-750`; or a CPU under 200 W, such as `voltaris-v7-8700` |
| No iGPU and no GPU | `voltaris-v5-5600`, `voltaris-v7-5800`, `helion-h5-1440f` without a GPU | `voltaris-v5-5600g` |
| No cooler | `voltaris-v5-8600` (or any CPU without `bundled_cooler`) without a cooler | `voltaris-v5-7600` |
| Mixed memory kits (note) | any two kits, even two of the same | one kit, e.g. `lumen-ddr5-6400-64` (2×32) |
| GPU link speed (note) | a PCIe 5.0 card (`halcyon-hx7070-twin`) on a PCIe 4.0 board; any PCIe 4.0 card on `meridian-a520m-core` (PCIe 3.0) | `meridian-x870e-crest` |
| Below recommended PSU (note) | `halcyon-hx6070-dual` (650 W recommended) + `ferro-core-550` | `ferro-core-650` |
| PCIe downgrade (note) | `arcwave-a5-2tb` (Gen 5) + `kestrel-b650m-wing` (Gen 4 slots) | `meridian-x870e-crest` |

The PSU rows are designed with margin. The wattage error holds whatever the
rest-of-system allowance is. The headroom row stays a warning, not an error
or a pass, for any allowance between about 0 and 110 W.
