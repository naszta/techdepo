//! The header search: filters the compiled-in catalog as you type (name, brand, tags, SKU).

use dioxus::prelude::*;
use techdepo_api_types::Product;

use crate::routes::Route;
use crate::state::build::catalog;
use crate::ui::money::MoneyText;
use crate::ui::product::category_icon;
use crate::ui::{Icon, IconName};

const MAX_RESULTS: usize = 6;

/// Products matching every word of `query`, best (name) matches first.
pub fn search(query: &str) -> Vec<&'static Product> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(usize, &'static Product)> = catalog()
        .all()
        .iter()
        .filter_map(|p| {
            let name = p.name.to_lowercase();
            let haystack = format!(
                "{name} {} {} {} {}",
                p.brand.to_lowercase(),
                p.sku.as_str().to_lowercase(),
                p.category.label().to_lowercase(),
                p.tags.join(" ")
            );
            words
                .iter()
                .all(|w| haystack.contains(w.as_str()))
                .then(|| {
                    (
                        words.iter().filter(|w| name.contains(w.as_str())).count(),
                        p,
                    )
                })
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.price.cmp(&b.1.price)));
    hits.into_iter().map(|(_, p)| p).collect()
}

/// The search field with its results as an ARIA combobox: the focus stays in the field, the
/// arrow keys move through the results (`aria-activedescendant`), Enter opens the active one
/// (or the first), Escape closes them. `id` must be unique on the page (the shop renders one in
/// the navbar and one under it on small screens).
#[component]
pub fn SearchBox(id: &'static str) -> Element {
    let mut query = use_signal(String::new);
    let mut open = use_signal(|| false);
    let mut active = use_signal(|| None::<usize>);
    let results = use_memo(move || search(&query.read()));
    let nav = navigator();

    let listbox = format!("{id}-results");
    let option_id = move |index: usize| format!("{id}-option-{index}");
    let shown = open() && !query.read().trim().is_empty();
    let count = results.read().len().min(MAX_RESULTS);
    let active_option = active().filter(|_| shown).map(option_id);
    let mut go = move |product: &'static Product| {
        nav.push(Route::Product {
            id: product.id.to_string(),
        });
        query.set(String::new());
        open.set(false);
        active.set(None);
    };

    rsx! {
        div { class: "relative w-full md:max-w-sm",
            label { class: "input input-sm w-full",
                Icon { name: IconName::Search, class: "size-4 opacity-60" }
                input {
                    id,
                    r#type: "search",
                    role: "combobox",
                    placeholder: "Search parts",
                    aria_label: "Search parts",
                    aria_autocomplete: "list",
                    aria_expanded: "{shown}",
                    aria_controls: "{listbox}",
                    aria_activedescendant: active_option,
                    autocomplete: "off",
                    value: "{query}",
                    oninput: move |e| {
                        query.set(e.value());
                        open.set(true);
                        active.set(None);
                    },
                    onfocusin: move |_| open.set(true),
                    onfocusout: move |_| {
                        // Let a click on a result land before closing.
                        spawn(async move {
                            gloo_timers::future::TimeoutFuture::new(150).await;
                            open.set(false);
                            active.set(None);
                        });
                    },
                    onkeydown: move |e| {
                        let step = |from: Option<usize>, down: bool| match (from, down) {
                            (None, true) => 0,
                            (None, false) => count - 1,
                            (Some(i), true) => (i + 1) % count,
                            (Some(i), false) => (i + count - 1) % count,
                        };
                        match e.key() {
                            Key::ArrowDown | Key::ArrowUp if count > 0 => {
                                e.prevent_default();
                                open.set(true);
                                let down = e.key() == Key::ArrowDown;
                                active.set(Some(step(active(), down)));
                            }
                            Key::Enter => {
                                let pick = results.read().get(active().unwrap_or(0)).copied();
                                if let Some(product) = pick {
                                    e.prevent_default();
                                    go(product);
                                }
                            }
                            Key::Escape => {
                                open.set(false);
                                active.set(None);
                            }
                            _ => {}
                        }
                    },
                }
            }
            if shown {
                ul {
                    id: "{listbox}",
                    role: "listbox",
                    aria_label: "Matching parts",
                    class: "menu absolute z-30 mt-1 w-full rounded-box border border-base-300 bg-base-100 p-1 shadow-sm",
                    if results.read().is_empty() {
                        li { class: "menu-disabled", role: "option", aria_disabled: "true",
                            span { "No parts match “{query}”" }
                        }
                    }
                    for (index, product) in results.read().iter().copied().take(MAX_RESULTS).enumerate() {
                        li {
                            key: "{product.id}",
                            id: option_id(index),
                            role: "option",
                            aria_selected: "{active() == Some(index)}",
                            Link {
                                to: Route::Product { id: product.id.to_string() },
                                // Not a tab stop: the field keeps the focus (combobox pattern).
                                tabindex: "-1",
                                class: if active() == Some(index) { "menu-active" } else { "" },
                                onclick: move |_| {
                                    query.set(String::new());
                                    open.set(false);
                                    active.set(None);
                                },
                                Icon { name: category_icon(product.category), class: "size-4 text-primary" }
                                span { class: "flex-1 truncate", "{product.name}" }
                                MoneyText { amount: product.price, class: "text-xs text-base-content/60" }
                            }
                        }
                    }
                }
            }
        }
    }
}
