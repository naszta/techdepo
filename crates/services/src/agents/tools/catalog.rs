//! Catalog and compatibility tools: pure, over the compiled-in catalog (`Catalog::global()`) and
//! the compat engine, so they run without a journaled step.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use techdepo_agent::{Runtime, Tool, ToolError};
use techdepo_catalog::{
    Catalog, Category, FormFactor, MemoryType, Product, ProductSpec, Socket,
    spec::{CoolerKind, StorageInterface},
};
use techdepo_compat::{
    Build, Report, Severity, Slot, Verdict, alternatives, check_build, check_pair,
    price_band_around,
};
use techdepo_domain::{Money, ProductId};

use super::ToolCx;

/// Most rows a search returns.
const MAX_ROWS: usize = 10;
/// Rows a search returns by default.
const DEFAULT_ROWS: usize = 8;
/// Alternatives returned.
const MAX_ALTERNATIVES: usize = 3;
/// Characters of a product's rich content returned by `get_product`.
const MAX_CONTENT_CHARS: usize = 3_000;

/// A product as a compact row: what the model needs to talk about it and to call the other
/// tools with its `id`.
pub fn row(p: &Product) -> Value {
    json!({
        "id": p.id,
        "name": p.name,
        "sku": p.sku,
        "category": p.category,
        "price": p.price.to_string(),
        "specs": key_specs(p),
    })
}

/// The few specs that matter when choosing, in words.
pub fn key_specs(p: &Product) -> String {
    match &p.spec {
        ProductSpec::Cpu(s) => {
            let memory: Vec<&str> = s.memory.iter().map(|m| m.memory_type.label()).collect();
            format!(
                "{} socket, {} cores / {} threads, up to {} MHz, {} W TDP, memory: {}{}",
                s.socket.label(),
                s.cores,
                s.threads,
                s.boost_clock_mhz,
                s.tdp_w,
                memory.join(" or "),
                if s.integrated_graphics {
                    ", integrated graphics"
                } else {
                    ""
                }
            )
        }
        ProductSpec::Motherboard(s) => format!(
            "{} socket, {} chipset, {}, {} memory ({} slots, up to {} GB, up to {} MT/s), {} M.2 slots{}",
            s.socket.label(),
            s.chipset.label(),
            s.form_factor.label(),
            s.memory_type.label(),
            s.memory_slots,
            s.max_memory_gb,
            s.max_memory_speed_mts,
            s.m2_slots.len(),
            if s.wifi { ", Wi-Fi" } else { "" }
        ),
        ProductSpec::Memory(s) => format!(
            "{}-{}, {}x{} GB ({} GB), CL{}, {} mm tall",
            s.memory_type.label(),
            s.speed_mts,
            s.modules,
            s.module_gb,
            s.total_gb(),
            s.cas_latency,
            s.height_mm
        ),
        ProductSpec::Storage(s) => format!(
            "{} GB, {}, {}, {} MB/s read",
            s.capacity_gb,
            s.form_factor.label(),
            match s.interface {
                StorageInterface::Nvme { pcie_gen } => format!("NVMe PCIe {pcie_gen}"),
                StorageInterface::Sata => "SATA".to_owned(),
            },
            s.read_mbps
        ),
        ProductSpec::Gpu(s) => format!(
            "{} GB VRAM, {} mm long, {} slots, {} W board power, {} W PSU recommended",
            s.vram_gb, s.length_mm, s.slot_width, s.board_power_w, s.recommended_psu_w
        ),
        ProductSpec::Case(s) => format!(
            "{}, boards: {}, graphics cards up to {} mm, coolers up to {} mm",
            s.size.label(),
            s.motherboard_form_factors
                .iter()
                .map(|f| f.label())
                .collect::<Vec<_>>()
                .join(", "),
            s.max_gpu_length_mm,
            s.max_cooler_height_mm
        ),
        ProductSpec::Psu(s) => format!(
            "{} W, {}, {}, {}",
            s.wattage_w,
            s.efficiency.label(),
            s.modular.label(),
            s.form_factor.label()
        ),
        ProductSpec::Cooler(s) => format!(
            "{}, sockets: {}, rated for {} W",
            match s.kind {
                CoolerKind::Air { height_mm, .. } => format!("air cooler, {height_mm} mm tall"),
                CoolerKind::Aio { radiator_mm } => format!("{radiator_mm} mm liquid cooler"),
            },
            s.sockets
                .iter()
                .map(|s| s.label())
                .collect::<Vec<_>>()
                .join(", "),
            s.tdp_rating_w
        ),
    }
}

/// Finds a product by ID, or by its exact name (models sometimes pass the name).
pub(crate) fn lookup(id: &str) -> Option<&'static Product> {
    let catalog = Catalog::global();
    let id = id.trim();
    catalog.get(id).or_else(|| {
        catalog
            .all()
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(id) || p.sku.as_str().eq_ignore_ascii_case(id))
    })
}

fn lookup_all(ids: &[String]) -> Result<Vec<&'static Product>, ToolError> {
    let mut products = Vec::with_capacity(ids.len());
    let mut unknown = Vec::new();
    for id in ids {
        match lookup(id) {
            Some(p) if products.iter().any(|q: &&Product| q.id == p.id) => {}
            Some(p) => products.push(p),
            None => unknown.push(id.as_str()),
        }
    }
    if !unknown.is_empty() {
        return Err(ToolError::NotFound(format!(
            "no product with the ID {}; find IDs with search_products",
            unknown.join(", ")
        )));
    }
    Ok(products)
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Search words that say nothing about which product.
const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "for", "with", "my", "kit", "kits", "card", "product", "gb",
];

/// How well `p` matches the query words: (words matched, score). A word in the name scores 3
/// (whole word) or 2 (prefix of a name word), elsewhere (brand, tags, category, SKU) 1.
fn relevance(p: &Product, query: &[String]) -> (usize, u32) {
    let name = words(&p.name);
    let other = words(&format!(
        "{} {} {} {} {}",
        p.brand,
        p.tags.join(" "),
        p.category.as_str(),
        p.category.label(),
        p.sku
    ));
    let mut matched = 0;
    let mut score = 0;
    for q in query {
        let s = if name.iter().any(|w| w == q) {
            3
        } else if name.iter().any(|w| w.starts_with(q.as_str())) {
            2
        } else if other.iter().any(|w| w.starts_with(q.as_str())) {
            1
        } else {
            0
        };
        if s > 0 {
            matched += 1;
            score += s;
        }
    }
    (matched, score)
}

/// Result order of `search_products`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    /// Best match first.
    #[default]
    Relevance,
    /// Cheapest first.
    PriceAsc,
    /// Most expensive first.
    PriceDesc,
}

/// Arguments of `search_products`. Every filter is optional; combine them freely.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Words to match in product names, brands and tags, e.g. `Kestrel B650` or `quiet`.
    #[serde(default)]
    pub text: Option<String>,
    /// Only this category.
    #[serde(default)]
    pub category: Option<Category>,
    /// Only parts for this CPU socket (CPUs, motherboards, coolers).
    #[serde(default)]
    pub socket: Option<Socket>,
    /// Only parts of this memory generation (memory kits, motherboards, CPUs supporting it).
    #[serde(default)]
    pub memory_type: Option<MemoryType>,
    /// Only motherboards of this form factor, or cases that take it.
    #[serde(default)]
    pub form_factor: Option<FormFactor>,
    /// Lowest price in euros.
    #[serde(default)]
    pub price_min: Option<f64>,
    /// Highest price in euros.
    #[serde(default)]
    pub price_max: Option<f64>,
    /// Result order (default: best match first).
    #[serde(default)]
    pub sort: Option<Sort>,
    /// How many rows (default 8, at most 10).
    #[serde(default)]
    pub limit: Option<u32>,
}

fn euros(value: f64) -> Money {
    #[allow(clippy::cast_possible_truncation)] // prices are far below i64::MAX cents
    Money::from_cents((value * 100.0).round() as i64)
}

/// The products matching `args`, best first.
pub fn search(args: &SearchArgs) -> Vec<&'static Product> {
    let query: Vec<String> = args
        .text
        .as_deref()
        .map(words)
        .unwrap_or_default()
        .into_iter()
        .filter(|w| !STOP_WORDS.contains(&w.as_str()))
        .collect();
    let min = args.price_min.map(euros);
    let max = args.price_max.map(euros);
    let mut hits: Vec<((usize, u32), usize, &Product)> = Catalog::global()
        .all()
        .iter()
        .enumerate()
        .filter(|(_, p)| args.category.is_none_or(|c| p.category == c))
        .filter(|(_, p)| min.is_none_or(|m| p.price >= m) && max.is_none_or(|m| p.price <= m))
        .filter(|(_, p)| args.socket.is_none_or(|s| fits_socket(p, s)))
        .filter(|(_, p)| args.memory_type.is_none_or(|m| fits_memory(p, m)))
        .filter(|(_, p)| args.form_factor.is_none_or(|f| fits_form_factor(p, f)))
        .map(|(i, p)| (relevance(p, &query), i, p))
        .collect();
    if !query.is_empty() {
        // Keep the products that match the most query words (at least one).
        let best = hits.iter().map(|(r, ..)| r.0).max().unwrap_or(0);
        hits.retain(|(r, ..)| r.0 > 0 && r.0 == best);
    }
    match args.sort.unwrap_or_default() {
        Sort::Relevance => hits.sort_by(|a, b| b.0.1.cmp(&a.0.1).then(a.1.cmp(&b.1))),
        Sort::PriceAsc => hits.sort_by(|a, b| a.2.price.cmp(&b.2.price).then(a.1.cmp(&b.1))),
        Sort::PriceDesc => hits.sort_by(|a, b| b.2.price.cmp(&a.2.price).then(a.1.cmp(&b.1))),
    }
    hits.into_iter().map(|(.., p)| p).collect()
}

fn fits_socket(p: &Product, socket: Socket) -> bool {
    match &p.spec {
        ProductSpec::Cpu(s) => s.socket == socket,
        ProductSpec::Motherboard(s) => s.socket == socket,
        ProductSpec::Cooler(s) => s.supports(socket),
        _ => false,
    }
}

fn fits_memory(p: &Product, memory: MemoryType) -> bool {
    match &p.spec {
        ProductSpec::Memory(s) => s.memory_type == memory,
        ProductSpec::Motherboard(s) => s.memory_type == memory,
        ProductSpec::Cpu(s) => s.supports_memory(memory),
        _ => false,
    }
}

fn fits_form_factor(p: &Product, form_factor: FormFactor) -> bool {
    match &p.spec {
        ProductSpec::Motherboard(s) => s.form_factor == form_factor,
        ProductSpec::Case(s) => s.motherboard_form_factors.contains(&form_factor),
        _ => false,
    }
}

/// `search_products`.
pub struct SearchProducts;

impl<R: Runtime> Tool<ToolCx<'_, R>> for SearchProducts {
    const NAME: &'static str = "search_products";
    const DESCRIPTION: &'static str = "Search the shop's catalog by name words and typed \
        filters. Returns compact rows (id, name, SKU, category, price, key specs). Use the `id` \
        with the other tools.";
    type Args = SearchArgs;

    async fn call(&self, _cx: &ToolCx<'_, R>, args: SearchArgs) -> Result<Value, ToolError> {
        let limit = args
            .limit
            .map_or(DEFAULT_ROWS, |l| (l as usize).clamp(1, MAX_ROWS));
        let hits = search(&args);
        let rows: Vec<Value> = hits.iter().take(limit).map(|p| row(p)).collect();
        Ok(json!({ "count": hits.len(), "products": rows }))
    }
}

/// Arguments of `get_product`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ProductArgs {
    /// The product's `id` from `search_products`.
    pub id: String,
}

/// `get_product`.
pub struct GetProduct;

impl<R: Runtime> Tool<ToolCx<'_, R>> for GetProduct {
    const NAME: &'static str = "get_product";
    const DESCRIPTION: &'static str = "One product's full specification and its description \
        (overview, who it is for, pros and cons, FAQ).";
    type Args = ProductArgs;

    async fn call(&self, _cx: &ToolCx<'_, R>, args: ProductArgs) -> Result<Value, ToolError> {
        let p = lookup(&args.id).ok_or_else(|| {
            ToolError::NotFound(format!(
                "no product with the ID `{}`; find IDs with search_products",
                args.id
            ))
        })?;
        let mut out = row(p);
        out["summary"] = json!(p.summary);
        out["tags"] = json!(p.tags);
        out["tier"] = json!(p.tier.label());
        out["spec"] = serde_json::to_value(&p.spec).unwrap_or(Value::Null);
        if let Some(content) = Catalog::global().content(&p.id) {
            out["description"] = json!(crate::support::conversation::truncate(
                content.markdown(),
                MAX_CONTENT_CHARS
            ));
        }
        Ok(out)
    }
}

/// Arguments of `check_compatibility`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CheckArgs {
    /// The `id`s of the parts to check together (at least two; one per slot, except memory kits
    /// and drives).
    pub product_ids: Vec<String>,
}

/// The engine's report, as the model sees it.
fn report_json(products: &[&Product], report: &Report) -> Value {
    let name = |id: &ProductId| {
        Catalog::global()
            .get(id)
            .map_or_else(|| id.to_string(), |p| p.name.clone())
    };
    json!({
        "verdict": report.verdict,
        "summary": report.verdict.label(),
        "parts": products.iter().map(|p| json!({ "id": p.id, "name": p.name })).collect::<Vec<_>>(),
        "issues": report.issues.iter().map(|i| json!({
            "severity": i.severity,
            "rule": i.rule,
            "parts": i.parts.iter().map(name).collect::<Vec<_>>(),
            "message": i.message,
        })).collect::<Vec<_>>(),
        "estimated_watts": report.estimated_watts,
        "psu_watts": report.psu_watts,
        "rules_checked": report.checked_rules.len(),
    })
}

/// Checks `products` as one build without the "missing part" rules (no cooler, no PSU, …): a
/// question about some parts is not a complete build.
pub fn check(products: &[&'static Product]) -> Result<Report, ToolError> {
    if let [a, b] = products {
        return Ok(check_pair(a, b));
    }
    let build = Build::from_products(products.iter().copied())
        .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
    let mut report = check_build(&build);
    report.issues.retain(|i| !i.rule.is_absence());
    report.checked_rules.retain(|r| !r.is_absence());
    report.verdict = Verdict::from_worst(report.issues.iter().map(|i| i.severity).max());
    Ok(report)
}

/// `check_compatibility`.
pub struct CheckCompatibility;

impl<R: Runtime> Tool<ToolCx<'_, R>> for CheckCompatibility {
    const NAME: &'static str = "check_compatibility";
    const DESCRIPTION: &'static str = "Run the shop's compatibility engine on parts. You MUST \
        call this before saying anything about whether parts work together, and report its \
        verdict and issue messages faithfully. Verdicts: compatible, warnings (works, with a \
        caveat), incompatible.";
    type Args = CheckArgs;

    async fn call(&self, _cx: &ToolCx<'_, R>, args: CheckArgs) -> Result<Value, ToolError> {
        let products = lookup_all(&args.product_ids)?;
        if products.len() < 2 {
            return Err(ToolError::InvalidArguments(
                "pass at least two different product IDs".to_owned(),
            ));
        }
        let report = check(&products)?;
        Ok(report_json(&products, &report))
    }
}

/// How far from the replaced part's price alternatives may be.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PriceBand {
    #[default]
    Similar,
    Cheaper,
    Any,
}

/// Arguments of `find_alternatives`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AlternativesArgs {
    /// The `id`s of the build's parts, including the one to replace.
    pub parts: Vec<String>,
    /// The category of the part to replace (or to add, if the build has none).
    pub replace_category: Category,
    /// `similar` (default): within 25 % of the replaced part's price; `cheaper`: below it;
    /// `any`: any price.
    #[serde(default)]
    pub price_band: PriceBand,
}

/// `find_alternatives`.
pub struct FindAlternatives;

impl<R: Runtime> Tool<ToolCx<'_, R>> for FindAlternatives {
    const NAME: &'static str = "find_alternatives";
    const DESCRIPTION: &'static str = "Catalog parts that could replace the build's part of one \
        category and are compatible with the rest of the build (checked by the compatibility \
        engine), best first.";
    type Args = AlternativesArgs;

    async fn call(&self, _cx: &ToolCx<'_, R>, args: AlternativesArgs) -> Result<Value, ToolError> {
        let catalog = Catalog::global();
        let products = lookup_all(&args.parts)?;
        let build = Build::from_products(products.iter().copied())
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        let category = args.replace_category;
        let replaced = products.iter().find(|p| p.category == category).copied();
        let slot = replaced
            .and_then(|p| build.slot_of(&p.id))
            .unwrap_or_else(|| Slot::from(category));
        let band = match (args.price_band, replaced) {
            (PriceBand::Similar, Some(p)) => price_band_around(p.price, 25),
            (PriceBand::Cheaper, Some(p)) => Money::ZERO..p.price,
            _ => Money::ZERO..Money::from_cents(i64::MAX),
        };
        let candidates: Vec<Value> = alternatives(catalog, &build, slot, band)
            .iter()
            .filter_map(|id| catalog.get(id))
            .take(MAX_ALTERNATIVES)
            .map(|p| {
                let mut r = row(p);
                let mut trial = build.clone();
                let caveats: Vec<String> = match trial.set(slot, p) {
                    Ok(_) => check_build(&trial)
                        .issues_for(&p.id)
                        .filter(|i| i.severity == Severity::Warning)
                        .map(|i| i.message.clone())
                        .collect(),
                    Err(_) => Vec::new(),
                };
                if !caveats.is_empty() {
                    r["caveats"] = json!(caveats);
                }
                r
            })
            .collect();
        Ok(json!({
            "replaces": replaced.map(row),
            "kept": products
                .iter()
                .filter(|p| replaced.is_none_or(|r| r.id != p.id))
                .map(|p| json!({ "id": p.id, "name": p.name }))
                .collect::<Vec<_>>(),
            "price_band": args.price_band,
            "candidates": candidates,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(args: SearchArgs) -> Vec<String> {
        search(&args).iter().map(|p| p.name.clone()).collect()
    }

    #[test]
    fn search_ranks_whole_name_words_first() {
        let found = names(SearchArgs {
            text: Some("Kestrel B650".into()),
            ..SearchArgs::default()
        });
        assert_eq!(found[0], "Kestrel B650 Aero", "{found:?}");
        assert!(found.iter().all(|n| n.contains("B650")), "{found:?}");

        let ddr4 = names(SearchArgs {
            text: Some("Nimbus DDR4".into()),
            ..SearchArgs::default()
        });
        assert!(
            !ddr4.is_empty() && ddr4.iter().all(|n| n.contains("DDR4")),
            "{ddr4:?}"
        );

        let am5_boards = search(&SearchArgs {
            category: Some(Category::Motherboard),
            socket: Some(Socket::Am5),
            sort: Some(Sort::PriceAsc),
            ..SearchArgs::default()
        });
        assert!(am5_boards.windows(2).all(|w| w[0].price <= w[1].price));
        assert!(
            am5_boards
                .iter()
                .all(|p| p.spec.as_motherboard().unwrap().socket == Socket::Am5)
        );
        assert!(
            names(SearchArgs {
                text: Some("teleporter".into()),
                ..SearchArgs::default()
            })
            .is_empty()
        );
    }

    #[test]
    fn checks_skip_missing_part_rules() {
        let board = lookup("kestrel-b650-aero").unwrap();
        let ddr4 = lookup("nimbus-ddr4-3200-32").unwrap();
        let report = check(&[board, ddr4]).unwrap();
        assert_eq!(report.verdict, Verdict::Incompatible);
        let cpu = Catalog::global()
            .all()
            .iter()
            .find(|p| p.spec.as_cpu().is_some_and(|s| s.socket == Socket::Am5))
            .unwrap();
        let ddr5 = lookup("nimbus-ddr5-5200-32").unwrap();
        let three = check(&[board, ddr5, cpu]).unwrap();
        assert!(three.issues.iter().all(|i| !i.rule.is_absence()));
        assert!(lookup("Kestrel B650 Aero").is_some(), "by name too");
    }
}
