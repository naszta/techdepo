//! The product page: tile and summary on the left; price, stock, Add to cart and Add to build
//! on the right with the compatibility box; tabs for Overview, Specs, Compatibility and FAQ;
//! then "works well with".
//!
//! Product data and content are compiled in; stock and "works well with" come from
//! `GET /api/products/{id}` when signed in (computed locally otherwise).

use dioxus::prelude::*;
use techdepo_api_types::catalog::ProductContentDto;
use techdepo_api_types::endpoints::GetProduct;
use techdepo_api_types::{Product, ProductId};
use techdepo_compat::{NOT_CHECKED, RuleId, check_pair};

use crate::api::use_api;
use crate::components::tabs::{TabContent, TabTrigger, Tabs, TabsAppearance};
use crate::components::toast::{ToastOptions, use_toast};
use crate::routes::Route;
use crate::session::use_session;
use crate::state::build::{Fit, catalog};
use crate::state::{use_build, use_cart, use_stock};
use crate::ui::markdown::Markdown;
use crate::ui::product::{
    ProductCard, ProductTile, category_plural, key_specs, product_image, spec_rows,
};
use crate::ui::{EmptyState, FitBadge, Icon, IconName, MoneyText, Mono, SeverityIcon, StockBadge};

/// Up to this many "works well with" products per other category.
const WORKS_WELL_PER_CATEGORY: usize = 2;

#[component]
pub fn ProductPage(id: String) -> Element {
    match catalog().get(id.as_str()) {
        Some(product) => rsx! { ProductView { key: "{product.id}", product } },
        None => rsx! {
            EmptyState { message: "We couldn't find that part.", icon: IconName::Search,
                Link { to: Route::Home {}, class: "btn btn-primary btn-sm", "Back to the shop" }
            }
        },
    }
}

/// Compatible products from other categories in the same tier, cheapest first (the API's
/// definition, computed locally from pair checks).
fn works_well_with_local(product: &'static Product) -> Vec<ProductId> {
    let mut out = Vec::new();
    for &category in techdepo_api_types::Category::ALL {
        if category == product.category {
            continue;
        }
        let mut candidates: Vec<&Product> = catalog()
            .by_category(category)
            .iter()
            .filter(|other| other.tier == product.tier)
            .filter(|other| {
                let report = check_pair(product, other);
                !report.checked_rules.is_empty() && report.issues.is_empty()
            })
            .collect();
        candidates.sort_by_key(|p| p.price);
        out.extend(
            candidates
                .into_iter()
                .take(WORKS_WELL_PER_CATEGORY)
                .map(|p| p.id.clone()),
        );
    }
    out
}

#[component]
fn ProductView(product: &'static Product) -> Element {
    let api = use_api();
    let session = use_session();
    let build = use_build();
    let cart = use_cart();
    let stock_store = use_stock();
    let toasts = use_toast();
    let nav = navigator();
    let mut quantity = use_signal(|| 1u32);
    let mut adding = use_signal(|| false);

    // Server detail (stock, works well with) once signed in.
    let detail = use_resource(move || async move {
        if !session.is_signed_in() {
            return None;
        }
        api.get::<GetProduct>(&[product.id.as_str()]).await.ok()
    });
    let content = catalog()
        .content(&product.id)
        .map(ProductContentDto::from)
        .unwrap_or_default();
    let stock = detail
        .read()
        .as_ref()
        .and_then(|d| d.as_ref().map(|d| d.stock))
        .or_else(|| stock_store.get(&product.id));
    let works_well: Vec<ProductId> = detail
        .read()
        .as_ref()
        .and_then(|d| d.as_ref().map(|d| d.works_well_with.clone()))
        .unwrap_or_else(|| works_well_with_local(product));
    let fit = build.fit(product);
    let in_build = build.contains(&product.id);
    let out_of_stock = stock.is_some_and(|s| s.stock == 0);

    let add_to_cart = move |_| {
        if !session.is_signed_in() {
            nav.push(Route::sign_in_to(format!("/products/{}", product.id)));
            return;
        }
        spawn(async move {
            adding.set(true);
            match cart.add(product.id.clone(), quantity()).await {
                Ok(_) => toasts.success(
                    format!("Added {} to your cart", product.name),
                    ToastOptions::new(),
                ),
                Err(e) => toasts.error(e.message, ToastOptions::new()),
            }
            adding.set(false);
        });
    };
    let add_to_build = move |_| {
        build.place(product);
        toasts.success(
            format!("{} is in your build", product.name),
            ToastOptions::new().description("Open the PC builder to see the full report."),
        );
    };

    rsx! {
        div { class: "breadcrumbs mb-4 text-sm",
            ul {
                li { Link { to: Route::Home {}, "Home" } }
                li {
                    Link { to: Route::Category { category: product.category.as_str().to_string() },
                        "{category_plural(product.category)}"
                    }
                }
                li { span { class: "text-base-content/60", "{product.name}" } }
            }
        }

        div { class: "grid gap-8 lg:grid-cols-[1fr_22rem]",
            // Left: tile and summary.
            div { class: "flex flex-col gap-6 sm:flex-row",
                ProductTile {
                    category: product.category,
                    brand: Some(product.brand.clone()),
                    image: product_image(product),
                    class: "aspect-square w-full self-start sm:w-64 shrink-0",
                    icon_class: "size-20",
                }
                div { class: "flex flex-col gap-3",
                    span { class: "text-xs font-semibold uppercase tracking-[0.18em] text-base-content/60", "{product.brand}" }
                    h1 { class: "text-3xl font-semibold tracking-tight", "{product.name}" }
                    p { class: "text-base-content/70", "{product.summary}" }
                    ul { class: "flex flex-wrap gap-2",
                        for spec in key_specs(product) {
                            li { class: "badge badge-outline", "{spec}" }
                        }
                    }
                    div { class: "text-sm text-base-content/60",
                        "SKU "
                        Mono { "{product.sku}" }
                    }
                }
            }

            // Right: price and actions.
            div { class: "flex flex-col gap-4",
                div { class: "card card-border border-base-300 bg-base-100",
                    div { class: "card-body gap-4",
                        div { class: "flex items-baseline justify-between",
                            MoneyText { amount: product.price, class: "text-3xl font-semibold" }
                            span { class: "text-xs text-base-content/60", "incl. VAT" }
                        }
                        div { class: "min-h-6",
                            if let Some(stock) = stock {
                                StockBadge { stock }
                            } else if !session.is_signed_in() {
                                span { class: "text-sm text-base-content/60", "Sign in to see stock" }
                            }
                        }
                        div { class: "flex gap-2",
                            div { class: "join",
                                button {
                                    class: "btn join-item btn-square",
                                    aria_label: "One less",
                                    disabled: quantity() <= 1,
                                    onclick: move |_| quantity.set(quantity().saturating_sub(1).max(1)),
                                    Icon { name: IconName::Minus }
                                }
                                span { class: "join-item grid w-10 place-items-center border-y border-base-300 tabular-nums",
                                    aria_live: "polite",
                                    "{quantity}"
                                }
                                button {
                                    class: "btn join-item btn-square",
                                    aria_label: "One more",
                                    disabled: quantity() >= 99,
                                    onclick: move |_| quantity.set((quantity() + 1).min(99)),
                                    Icon { name: IconName::Plus }
                                }
                            }
                            button {
                                class: "btn btn-primary flex-1",
                                disabled: adding() || out_of_stock,
                                onclick: add_to_cart,
                                if adding() {
                                    span { class: "loading loading-spinner loading-sm" }
                                } else {
                                    Icon { name: IconName::Cart }
                                }
                                "Add to cart"
                            }
                        }
                        button { class: "btn btn-outline", onclick: add_to_build,
                            Icon { name: IconName::Wrench }
                            if in_build { "In your build · replace" } else { "Add to build" }
                        }
                    }
                }
                CompatBox { product, fit: fit.clone() }
            }
        }

        // Tabs.
        div { class: "mt-10",
            Tabs { appearance: TabsAppearance::Border, default_value: "overview".to_string(),
                TabTrigger { value: "overview".to_string(), index: 0usize, "Overview" }
                TabContent { value: "overview".to_string(), index: 0usize, class: "pt-6",
                    OverviewTab { content: content.clone() }
                }
                TabTrigger { value: "specs".to_string(), index: 1usize, "Specs" }
                TabContent { value: "specs".to_string(), index: 1usize, class: "pt-6",
                    SpecTable { product }
                }
                TabTrigger { value: "compat".to_string(), index: 2usize, "Compatibility" }
                TabContent { value: "compat".to_string(), index: 2usize, class: "pt-6",
                    CompatTab { product, fit }
                }
                TabTrigger { value: "faq".to_string(), index: 3usize, "FAQ" }
                TabContent { value: "faq".to_string(), index: 3usize, class: "pt-6",
                    FaqTab { content }
                }
            }
        }

        if !works_well.is_empty() {
            section { class: "mt-12",
                h2 { class: "mb-4 text-lg font-semibold", "Works well with" }
                div { class: "grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4",
                    for other in works_well.iter().filter_map(|id| catalog().get(id)).take(8) {
                        ProductCard { key: "{other.id}", product: other }
                    }
                }
            }
        }
    }
}

/// The compatibility box next to the buy buttons.
#[component]
fn CompatBox(product: &'static Product, fit: Option<Fit>) -> Element {
    let Some(fit) = fit else {
        return rsx! {
            div { class: "rounded-box border border-dashed border-base-300 p-4 text-sm text-base-content/70",
                "Add parts to a build and we'll check this one against them: sockets, memory, clearances and power."
            }
        };
    };
    let issues = fit.issues.clone();
    rsx! {
        div { class: "rounded-box border border-base-300 p-4 flex flex-col gap-3",
            div { class: "flex items-center justify-between gap-2",
                span { class: "text-sm font-medium", "With your build" }
                FitBadge { fit }
            }
            if issues.is_empty() {
                p { class: "text-sm text-base-content/70", "No conflicts with the parts in your build." }
            }
            ul { class: "flex flex-col gap-2",
                for issue in issues {
                    li { class: "flex gap-2 text-sm",
                        SeverityIcon { severity: issue.severity, class: "size-4 mt-0.5" }
                        span { "{issue.message}" }
                    }
                }
            }
            Link { to: Route::Builder {}, class: "link link-primary text-sm", "Open the builder" }
        }
    }
}

#[component]
fn OverviewTab(content: ProductContentDto) -> Element {
    rsx! {
        div { class: "grid gap-8 lg:grid-cols-[1fr_20rem]",
            div { class: "flex flex-col gap-6 max-w-prose",
                Markdown { text: content.overview.clone() }
                if !content.who_its_for.is_empty() {
                    div { class: "flex flex-col gap-2",
                        h3 { class: "font-semibold", "Who it's for" }
                        Markdown { text: content.who_its_for.clone() }
                    }
                }
            }
            div { class: "flex flex-col gap-4",
                div { class: "rounded-box bg-base-200 p-4",
                    h3 { class: "mb-2 font-semibold", "Pros" }
                    ul { class: "flex flex-col gap-1.5 text-sm",
                        for pro in content.pros.iter() {
                            li { class: "flex gap-2",
                                Icon { name: IconName::Check, class: "size-4 mt-0.5 text-success" }
                                span { "{pro}" }
                            }
                        }
                    }
                }
                div { class: "rounded-box bg-base-200 p-4",
                    h3 { class: "mb-2 font-semibold", "Cons" }
                    ul { class: "flex flex-col gap-1.5 text-sm",
                        for con in content.cons.iter() {
                            li { class: "flex gap-2",
                                Icon { name: IconName::Minus, class: "size-4 mt-0.5 text-base-content/60" }
                                span { "{con}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The full spec table (two columns).
#[component]
pub fn SpecTable(product: &'static Product) -> Element {
    rsx! {
        div { class: "overflow-x-auto max-w-2xl",
            table { class: "table",
                tbody {
                    for (label , value) in spec_rows(product) {
                        tr { key: "{label}",
                            th { class: "w-1/3 font-medium text-base-content/70", "{label}" }
                            td { class: "tabular-nums", "{value}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CompatTab(product: &'static Product, fit: Option<Fit>) -> Element {
    let rules: Vec<RuleId> = RuleId::ALL
        .iter()
        .copied()
        .filter(|r| r.categories().contains(&product.category))
        .collect();
    rsx! {
        div { class: "grid gap-8 lg:grid-cols-2",
            div { class: "flex flex-col gap-3",
                h3 { class: "font-semibold", "Against your build" }
                match fit {
                    None => rsx! {
                        p { class: "text-base-content/70",
                            "Your build is empty. Add this part, or start in the "
                            Link { to: Route::Builder {}, class: "link link-primary", "PC builder" }
                            ", and every other part is checked against it."
                        }
                    },
                    Some(fit) if fit.issues.is_empty() => rsx! {
                        div { class: "alert alert-success alert-soft",
                            Icon { name: IconName::CircleCheck, class: "size-5" }
                            span { "No conflicts with the parts in your build." }
                        }
                    },
                    Some(fit) => rsx! {
                        ul { class: "flex flex-col gap-3",
                            for issue in fit.issues {
                                li { class: "flex gap-3 rounded-box border border-base-300 p-3",
                                    SeverityIcon { severity: issue.severity, class: "size-5 shrink-0" }
                                    div { class: "flex flex-col gap-1",
                                        span { class: "text-sm", "{issue.message}" }
                                        span { class: "text-xs text-base-content/60", "Rule: {issue.rule.description()}" }
                                    }
                                }
                            }
                        }
                    },
                }
            }
            div { class: "flex flex-col gap-3",
                h3 { class: "font-semibold", "What we check for this part" }
                ul { class: "flex flex-col gap-1.5 text-sm",
                    for rule in rules {
                        li { class: "flex gap-2",
                            SeverityIcon { severity: rule.severity(), class: "size-4 mt-0.5" }
                            span { "{rule.description()}" }
                        }
                    }
                }
                details { class: "collapse collapse-arrow border border-base-300 text-sm",
                    summary { class: "collapse-title font-medium", "What we don't check" }
                    div { class: "collapse-content flex flex-col gap-2",
                        for item in NOT_CHECKED {
                            p {
                                span { class: "font-medium", "{item.title}. " }
                                span { class: "text-base-content/70", "{item.detail}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn FaqTab(content: ProductContentDto) -> Element {
    rsx! {
        div { class: "flex max-w-3xl flex-col gap-2",
            for (i , item) in content.faq.into_iter().enumerate() {
                details { key: "{i}", class: "collapse collapse-arrow border border-base-300", open: i == 0,
                    summary { class: "collapse-title font-medium", "{item.question}" }
                    div { class: "collapse-content", Markdown { text: item.answer } }
                }
            }
        }
    }
}
