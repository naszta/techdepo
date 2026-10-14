//! A category: typed facets and a price range on the left (collapsible on mobile), a
//! "compatible with my build" toggle, and the product grid with compare checkboxes.

use std::collections::{BTreeMap, BTreeSet};

use dioxus::prelude::*;
use techdepo_api_types::{Category, Money, Product, ProductId};
use techdepo_catalog::spec::{CoolerKind, StorageInterface};

use crate::pages::PageHeader;
use crate::routes::Route;
use crate::state::build::catalog;
use crate::state::use_build;
use crate::ui::product::{ProductCard, capacity, category_plural};
use crate::ui::{EmptyState, Icon, IconName};

/// Most products the compare page takes.
pub const MAX_COMPARE: usize = 3;

/// A filter dimension: a label and the values a product has (several for e.g. case form
/// factors).
struct Facet {
    key: &'static str,
    label: &'static str,
    values: fn(&Product) -> Vec<String>,
}

fn facet(key: &'static str, label: &'static str, values: fn(&Product) -> Vec<String>) -> Facet {
    Facet { key, label, values }
}

fn yes_no(value: bool) -> Vec<String> {
    vec![if value { "Yes" } else { "No" }.to_owned()]
}

fn labels<T: ToString>(items: &[T]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

/// The facets of a category, from its typed spec.
fn facets(category: Category) -> Vec<Facet> {
    let mut facets = match category {
        Category::Cpu => vec![
            facet("socket", "Socket", |p| {
                p.spec
                    .as_cpu()
                    .map(|s| vec![s.socket.to_string()])
                    .unwrap_or_default()
            }),
            facet("cores", "Cores", |p| {
                p.spec
                    .as_cpu()
                    .map(|s| vec![format!("{} cores", s.cores)])
                    .unwrap_or_default()
            }),
            facet("igpu", "Integrated graphics", |p| {
                p.spec
                    .as_cpu()
                    .map(|s| yes_no(s.integrated_graphics))
                    .unwrap_or_default()
            }),
            facet("cooler", "Cooler in the box", |p| {
                p.spec
                    .as_cpu()
                    .map(|s| yes_no(s.bundled_cooler))
                    .unwrap_or_default()
            }),
        ],
        Category::Motherboard => vec![
            facet("socket", "Socket", |p| {
                p.spec
                    .as_motherboard()
                    .map(|s| vec![s.socket.to_string()])
                    .unwrap_or_default()
            }),
            facet("chipset", "Chipset", |p| {
                p.spec
                    .as_motherboard()
                    .map(|s| vec![s.chipset.to_string()])
                    .unwrap_or_default()
            }),
            facet("form", "Form factor", |p| {
                p.spec
                    .as_motherboard()
                    .map(|s| vec![s.form_factor.to_string()])
                    .unwrap_or_default()
            }),
            facet("memory", "Memory", |p| {
                p.spec
                    .as_motherboard()
                    .map(|s| vec![s.memory_type.to_string()])
                    .unwrap_or_default()
            }),
            facet("wifi", "Wi-Fi", |p| {
                p.spec
                    .as_motherboard()
                    .map(|s| yes_no(s.wifi))
                    .unwrap_or_default()
            }),
        ],
        Category::Memory => vec![
            facet("type", "Type", |p| {
                p.spec
                    .as_memory()
                    .map(|s| vec![s.memory_type.to_string()])
                    .unwrap_or_default()
            }),
            facet("capacity", "Capacity", |p| {
                p.spec
                    .as_memory()
                    .map(|s| vec![format!("{} GB", s.total_gb())])
                    .unwrap_or_default()
            }),
            facet("speed", "Speed", |p| {
                p.spec
                    .as_memory()
                    .map(|s| vec![format!("{} MT/s", s.speed_mts)])
                    .unwrap_or_default()
            }),
            facet("modules", "Modules", |p| {
                p.spec
                    .as_memory()
                    .map(|s| vec![format!("{} × {} GB", s.modules, s.module_gb)])
                    .unwrap_or_default()
            }),
        ],
        Category::Storage => vec![
            facet("form", "Form factor", |p| {
                p.spec
                    .as_storage()
                    .map(|s| vec![s.form_factor.to_string()])
                    .unwrap_or_default()
            }),
            facet("interface", "Interface", |p| {
                p.spec
                    .as_storage()
                    .map(|s| match s.interface {
                        StorageInterface::Nvme { pcie_gen } => vec![format!("NVMe {pcie_gen}")],
                        StorageInterface::Sata => vec!["SATA".to_owned()],
                    })
                    .unwrap_or_default()
            }),
            facet("capacity", "Capacity", |p| {
                p.spec
                    .as_storage()
                    .map(|s| vec![capacity(s.capacity_gb)])
                    .unwrap_or_default()
            }),
        ],
        Category::Gpu => vec![
            facet("vram", "VRAM", |p| {
                p.spec
                    .as_gpu()
                    .map(|s| vec![format!("{} GB", s.vram_gb)])
                    .unwrap_or_default()
            }),
            facet("length", "Length", |p| {
                p.spec
                    .as_gpu()
                    .map(|s| {
                        vec![
                            match s.length_mm {
                                0..=250 => "250 mm or shorter",
                                251..=300 => "251–300 mm",
                                _ => "Over 300 mm",
                            }
                            .to_owned(),
                        ]
                    })
                    .unwrap_or_default()
            }),
            facet("power", "Power connector", |p| {
                p.spec
                    .as_gpu()
                    .map(|s| {
                        vec![match (
                            s.power_connectors.pcie_8pin,
                            s.power_connectors.twelve_v_2x6,
                        ) {
                            (_, n) if n > 0 => "12V-2x6".to_owned(),
                            (0, _) => "Slot power only".to_owned(),
                            (n, _) => format!("{n}× 8-pin"),
                        }]
                    })
                    .unwrap_or_default()
            }),
        ],
        Category::Case => vec![
            facet("size", "Size", |p| {
                p.spec
                    .as_case()
                    .map(|s| vec![s.size.to_string()])
                    .unwrap_or_default()
            }),
            facet("boards", "Fits motherboard", |p| {
                p.spec
                    .as_case()
                    .map(|s| labels(&s.motherboard_form_factors))
                    .unwrap_or_default()
            }),
            facet("psu", "Fits PSU", |p| {
                p.spec
                    .as_case()
                    .map(|s| labels(&s.psu_form_factors))
                    .unwrap_or_default()
            }),
        ],
        Category::Psu => vec![
            facet("wattage", "Wattage", |p| {
                p.spec
                    .as_psu()
                    .map(|s| vec![format!("{} W", s.wattage_w)])
                    .unwrap_or_default()
            }),
            facet("form", "Form factor", |p| {
                p.spec
                    .as_psu()
                    .map(|s| vec![s.form_factor.to_string()])
                    .unwrap_or_default()
            }),
            facet("efficiency", "Efficiency", |p| {
                p.spec
                    .as_psu()
                    .map(|s| vec![s.efficiency.to_string()])
                    .unwrap_or_default()
            }),
            facet("modular", "Cables", |p| {
                p.spec
                    .as_psu()
                    .map(|s| vec![s.modular.to_string()])
                    .unwrap_or_default()
            }),
        ],
        Category::Cooler => vec![
            facet("kind", "Type", |p| {
                p.spec
                    .as_cooler()
                    .map(|s| match s.kind {
                        CoolerKind::Air { .. } => vec!["Air".to_owned()],
                        CoolerKind::Aio { radiator_mm } => vec![format!("AIO {radiator_mm} mm")],
                    })
                    .unwrap_or_default()
            }),
            facet("socket", "Socket", |p| {
                p.spec
                    .as_cooler()
                    .map(|s| labels(&s.sockets))
                    .unwrap_or_default()
            }),
        ],
    };
    facets.push(facet("tier", "Price tier", |p| {
        vec![p.tier.label().to_owned()]
    }));
    facets.push(facet("brand", "Brand", |p| vec![p.brand.clone()]));
    facets
}

/// Numbers sort numerically ("8 GB" before "16 GB"), everything else alphabetically.
fn sort_key(label: &str) -> (u8, u64, String) {
    let digits: String = label.chars().take_while(char::is_ascii_digit).collect();
    match digits.parse::<u64>() {
        Ok(n) => (0, n, label.to_owned()),
        Err(_) => (1, 0, label.to_owned()),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SortBy {
    PriceAsc,
    PriceDesc,
    Name,
}

#[component]
pub fn CategoryPage(category: String) -> Element {
    match category.parse::<Category>() {
        Ok(category) => rsx! { CategoryView { key: "{category.as_str()}", category } },
        Err(_) => rsx! {
            EmptyState { message: "We don't sell “{category}”.", icon: IconName::Search,
                Link { to: Route::Home {}, class: "btn btn-primary btn-sm", "See all categories" }
            }
        },
    }
}

#[component]
fn CategoryView(category: Category) -> Element {
    let build = use_build();
    let products: &'static [Product] = catalog().by_category(category);
    let facet_defs = use_hook(|| std::rc::Rc::new(facets(category)));
    let mut selected = use_signal(BTreeMap::<&'static str, BTreeSet<String>>::new);
    let mut price_min = use_signal(String::new);
    let mut price_max = use_signal(String::new);
    // On by default: it only filters once a build exists (commerce §4).
    let mut only_compatible = use_signal(|| true);
    let mut sort = use_signal(|| SortBy::PriceAsc);
    let mut show_filters = use_signal(|| false);
    let mut compare = use_signal(Vec::<ProductId>::new);

    let has_build = !build.view.read().is_empty();
    let defs = facet_defs.clone();
    let visible = use_memo(move || {
        let selected = selected.read();
        let min = parse_euros(&price_min.read());
        let max = parse_euros(&price_max.read());
        let only_compatible = only_compatible() && !build.view.read().is_empty();
        let mut list: Vec<&'static Product> = products
            .iter()
            .filter(|p| {
                defs.iter().all(|f| match selected.get(f.key) {
                    Some(values) if !values.is_empty() => {
                        (f.values)(p).iter().any(|v| values.contains(v))
                    }
                    _ => true,
                })
            })
            .filter(|p| min.is_none_or(|m| p.price >= m) && max.is_none_or(|m| p.price <= m))
            .filter(|p| !only_compatible || build.fit(p).is_none_or(|f| f.is_compatible()))
            .collect();
        match sort() {
            SortBy::PriceAsc => list.sort_by_key(|p| p.price),
            SortBy::PriceDesc => list.sort_by_key(|p| std::cmp::Reverse(p.price)),
            SortBy::Name => list.sort_by(|a, b| a.name.cmp(&b.name)),
        }
        list
    });

    // "Compatible with my build" counts only while it filters (there is a build).
    let active_filters = selected.read().values().map(BTreeSet::len).sum::<usize>()
        + usize::from(!price_min.read().is_empty())
        + usize::from(!price_max.read().is_empty())
        + usize::from(only_compatible() && has_build);
    // Both "Clear filters" buttons reset the same things.
    let mut clear_filters = move || {
        selected.set(BTreeMap::new());
        only_compatible.set(false);
        price_min.set(String::new());
        price_max.set(String::new());
    };

    rsx! {
        PageHeader {
            title: category_plural(category).to_string(),
            subtitle: "{products.len()} parts. Every one is checked against your build.",
        }
        div { class: "grid gap-6 lg:grid-cols-[15rem_1fr]",
            // Filters.
            div { class: "flex flex-col gap-3",
                button {
                    class: "btn btn-sm lg:hidden",
                    aria_expanded: "{show_filters}",
                    onclick: move |_| show_filters.toggle(),
                    Icon { name: IconName::Sliders }
                    if active_filters > 0 { "Filters ({active_filters})" } else { "Filters" }
                }
                aside {
                    class: "flex flex-col gap-5",
                    class: if !show_filters() { "hidden lg:flex" },
                    aria_label: "Filters",
                    if has_build {
                        label { class: "flex items-center justify-between gap-2 rounded-box border border-base-300 p-3 text-sm",
                            span { class: "flex flex-col",
                                span { class: "font-medium", "Compatible with my build" }
                                span { class: "text-xs text-base-content/60", "Hide parts with errors" }
                            }
                            input {
                                r#type: "checkbox",
                                class: "toggle toggle-primary toggle-sm",
                                checked: only_compatible(),
                                onchange: move |e| only_compatible.set(e.checked()),
                            }
                        }
                    }
                    fieldset { class: "fieldset",
                        legend { class: "fieldset-legend", "Price (€)" }
                        div { class: "flex items-center gap-2",
                            input {
                                r#type: "number",
                                class: "input input-sm w-full",
                                placeholder: "Min",
                                min: "0",
                                aria_label: "Minimum price in euros",
                                value: "{price_min}",
                                oninput: move |e| price_min.set(e.value()),
                            }
                            span { "–" }
                            input {
                                r#type: "number",
                                class: "input input-sm w-full",
                                placeholder: "Max",
                                min: "0",
                                aria_label: "Maximum price in euros",
                                value: "{price_max}",
                                oninput: move |e| price_max.set(e.value()),
                            }
                        }
                    }
                    for f in facet_defs.iter() {
                        FacetGroup {
                            key: "{f.key}",
                            label: f.label,
                            options: facet_options(products, f),
                            selected: selected.read().get(f.key).cloned().unwrap_or_default(),
                            on_toggle: {
                                let key = f.key;
                                move |value: String| {
                                    let mut map = selected.write();
                                    let set = map.entry(key).or_default();
                                    if !set.remove(&value) {
                                        set.insert(value);
                                    }
                                }
                            },
                        }
                    }
                    if active_filters > 0 {
                        button {
                            class: "btn btn-ghost btn-sm self-start",
                            onclick: move |_| clear_filters(),
                            "Clear filters"
                        }
                    }
                }
            }

            // Results.
            div { class: "flex min-w-0 flex-col gap-4",
                div { class: "flex items-center justify-between gap-2 text-sm",
                    span { class: "text-base-content/60", "{visible.read().len()} shown" }
                    label { class: "flex items-center gap-2",
                        span { class: "text-base-content/60", "Sort" }
                        select {
                            class: "select select-sm w-auto",
                            aria_label: "Sort products",
                            onchange: move |e| sort.set(match e.value().as_str() {
                                "price_desc" => SortBy::PriceDesc,
                                "name" => SortBy::Name,
                                _ => SortBy::PriceAsc,
                            }),
                            option { value: "price_asc", "Price: low to high" }
                            option { value: "price_desc", "Price: high to low" }
                            option { value: "name", "Name" }
                        }
                    }
                }
                if visible.read().is_empty() {
                    EmptyState { message: "No parts match these filters.", icon: IconName::Sliders,
                        button {
                            class: "btn btn-sm",
                            onclick: move |_| clear_filters(),
                            "Clear filters"
                        }
                    }
                } else {
                    div { class: "grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3",
                        for product in visible.read().iter().copied() {
                            ProductCard {
                                key: "{product.id}",
                                product,
                                compare: rsx! {
                                    CompareCheck { id: product.id.clone(), compare }
                                },
                            }
                        }
                    }
                }
            }
        }

        if !compare.read().is_empty() {
            div { class: "sticky bottom-4 z-10 mt-6 flex justify-center",
                div { class: "flex items-center gap-3 rounded-box border border-base-300 bg-base-100 px-4 py-2 shadow-sm",
                    span { class: "text-sm", "{compare.read().len()} of {MAX_COMPARE} selected" }
                    button { class: "btn btn-ghost btn-sm", onclick: move |_| compare.set(Vec::new()), "Clear" }
                    Link {
                        class: if compare.read().len() < 2 { "btn btn-primary btn-sm btn-disabled" } else { "btn btn-primary btn-sm" },
                        to: Route::Compare { ids: compare.read().iter().map(ProductId::as_str).collect::<Vec<_>>().join(",") },
                        Icon { name: IconName::Compare }
                        "Compare"
                    }
                }
            }
        }
    }
}

/// `(value, count)` for every value of `f` in `products`, in natural order.
fn facet_options(products: &[Product], f: &Facet) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for p in products {
        for v in (f.values)(p) {
            *counts.entry(v).or_default() += 1;
        }
    }
    let mut options: Vec<(String, usize)> = counts.into_iter().collect();
    options.sort_by_key(|(v, _)| sort_key(v));
    options
}

fn parse_euros(text: &str) -> Option<Money> {
    let euros: f64 = text.trim().replace(',', ".").parse().ok()?;
    (euros >= 0.0).then(|| Money::from_cents((euros * 100.0).round() as i64))
}

#[component]
fn FacetGroup(
    label: &'static str,
    options: Vec<(String, usize)>,
    selected: BTreeSet<String>,
    on_toggle: EventHandler<String>,
) -> Element {
    if options.len() < 2 {
        return rsx! {};
    }
    rsx! {
        fieldset { class: "fieldset",
            legend { class: "fieldset-legend", "{label}" }
            for (value , count) in options {
                label { key: "{value}", class: "label cursor-pointer justify-start gap-2 text-sm text-base-content",
                    input {
                        r#type: "checkbox",
                        class: "checkbox checkbox-sm checkbox-primary",
                        checked: selected.contains(&value),
                        onchange: {
                            let value = value.clone();
                            move |_| on_toggle.call(value.clone())
                        },
                    }
                    span { class: "flex-1", "{value}" }
                    span { class: "text-xs text-base-content/50 tabular-nums", "{count}" }
                }
            }
        }
    }
}

#[component]
fn CompareCheck(id: ProductId, compare: Signal<Vec<ProductId>>) -> Element {
    let checked = compare.read().contains(&id);
    let full = compare.read().len() >= MAX_COMPARE;
    rsx! {
        label { class: "label cursor-pointer gap-1.5 text-xs",
            input {
                r#type: "checkbox",
                class: "checkbox checkbox-xs",
                checked,
                disabled: !checked && full,
                onchange: move |_| {
                    let mut list = compare.write();
                    if let Some(at) = list.iter().position(|x| *x == id) {
                        list.remove(at);
                    } else if list.len() < MAX_COMPARE {
                        list.push(id.clone());
                    }
                },
            }
            "Compare"
        }
    }
}
