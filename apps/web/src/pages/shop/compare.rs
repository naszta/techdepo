//! Compare two or three products side by side; rows that differ are highlighted.

use dioxus::prelude::*;
use techdepo_api_types::Product;

use crate::pages::PageHeader;
use crate::routes::Route;
use crate::state::build::catalog;
use crate::state::use_build;
use crate::ui::product::{ProductTile, category_plural, spec_rows};
use crate::ui::{EmptyState, FitBadge, IconName, MoneyText};

#[component]
pub fn Compare(ids: String) -> Element {
    let build = use_build();
    let products: Vec<&'static Product> = ids
        .split(',')
        .filter_map(|id| catalog().get(id.trim()))
        .take(3)
        .collect();

    if products.len() < 2 {
        return rsx! {
            PageHeader { title: "Compare" }
            EmptyState { message: "Pick two or three parts to compare.", icon: IconName::Compare,
                Link { to: Route::Home {}, class: "btn btn-primary btn-sm", "Browse parts" }
            }
        };
    }

    // Union of spec labels, in first-seen order.
    let specs: Vec<Vec<(&'static str, String)>> = products.iter().map(|p| spec_rows(p)).collect();
    let mut labels: Vec<&'static str> = Vec::new();
    for rows in &specs {
        for (label, _) in rows {
            if !labels.contains(label) {
                labels.push(label);
            }
        }
    }
    let value = |i: usize, label: &str| {
        specs[i]
            .iter()
            .find(|(l, _)| *l == label)
            .map_or_else(|| "—".to_owned(), |(_, v)| v.clone())
    };
    let back = products[0].category;

    rsx! {
        PageHeader {
            title: "Compare",
            actions: rsx! {
                Link { to: Route::Category { category: back.as_str().to_string() }, class: "btn btn-ghost btn-sm",
                    "Back to {category_plural(back)}"
                }
            },
        }
        div { class: "overflow-x-auto rounded-box border border-base-300",
            table { class: "table table-fixed min-w-[40rem]",
                thead {
                    tr {
                        th { class: "w-40" }
                        for product in products.iter().copied() {
                            th { key: "{product.id}", class: "align-top",
                                div { class: "flex flex-col gap-2 font-normal",
                                    ProductTile { category: product.category, brand: Some(product.brand.clone()), class: "aspect-[4/3] w-full" }
                                    Link { to: Route::Product { id: product.id.to_string() }, class: "font-semibold text-base-content hover:text-primary",
                                        "{product.name}"
                                    }
                                    MoneyText { amount: product.price, class: "text-lg font-semibold text-base-content" }
                                    if let Some(fit) = build.fit(product) {
                                        FitBadge { fit, small: true }
                                    }
                                    button { class: "btn btn-sm btn-outline", onclick: move |_| build.place(product), "Add to build" }
                                }
                            }
                        }
                    }
                }
                tbody {
                    for label in labels {
                        {
                            let values: Vec<String> = (0..products.len()).map(|i| value(i, label)).collect();
                            let differs = values.iter().any(|v| *v != values[0]);
                            rsx! {
                                tr { key: "{label}", class: if differs { "bg-primary/5" },
                                    th { class: "font-medium text-base-content/70", "{label}" }
                                    for (i , v) in values.into_iter().enumerate() {
                                        td { key: "{i}", class: "tabular-nums", "{v}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        p { class: "mt-3 text-sm text-base-content/60", "Highlighted rows differ." }
    }
}
