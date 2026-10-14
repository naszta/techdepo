//! Products and per-org stock (commerce.md §7). The catalog and its prices are global and
//! read-only; stock is editable inline (0–9999, `Org/set_stock`), and low or empty stock is
//! highlighted.

use dioxus::prelude::*;
use techdepo_api_types::Category;
use techdepo_api_types::Validate;
use techdepo_api_types::admin::{InventoryItem, InventoryQuery, MAX_STOCK, SetStockRequest};
use techdepo_api_types::catalog::StockLevel;
use techdepo_api_types::endpoints::{GetInventory, SetStock};

use super::common::{FilterBar, SelectFilter, TableWrap, ToggleFilter, options, parsed};
use crate::api::use_api;
use crate::components::toast::{ToastOptions, use_toast};
use crate::pages::PageHeader;
use crate::routes::Route;
use crate::ui::format::ago;
use crate::ui::{EmptyState, ErrorAlert, IconName, MoneyText, Mono, PageSkeleton, StockBadge};

#[component]
pub fn AdminInventory() -> Element {
    let api = use_api();
    let category = use_signal(String::new);
    let low_only = use_signal(|| false);
    let mut inventory = use_resource(move || async move {
        let query = InventoryQuery {
            category: parsed::<Category>(&category()),
            low_only: low_only().then_some(true),
        };
        api.query::<GetInventory>(&[], &query).await
    });

    rsx! {
        PageHeader {
            title: "Inventory",
            subtitle: "Stock is per shop and editable; products and prices are the global catalog.",
        }
        FilterBar {
            SelectFilter { label: "Category", value: category, options: options(Category::ALL, Category::as_str, Category::label) }
            ToggleFilter { label: "Low or out of stock", value: low_only }
        }
        match &*inventory.read() {
            None => rsx! { PageSkeleton { rows: 3 } },
            Some(Err(error)) => rsx! { ErrorAlert { error: error.clone(), on_retry: move |_| inventory.restart() } },
            Some(Ok(list)) if list.items.is_empty() => rsx! {
                EmptyState { message: "Nothing here: every product is well stocked.", icon: IconName::Boxes }
            },
            Some(Ok(list)) => {
                let low = list.items.iter().filter(|i| i.stock.level != StockLevel::InStock).count();
                rsx! {
                    p { class: "mb-2 text-base-content/60", "{list.items.len()} products · {low} low or out of stock" }
                    TableWrap {
                        table { class: "table table-sm",
                            thead {
                                tr {
                                    th { "Product" }
                                    th { "Category" }
                                    th { class: "text-right", "Price" }
                                    th { "Level" }
                                    th { "Stock" }
                                    th { class: "text-right", "Reserved" }
                                    th { class: "text-right", "Default" }
                                    th { "Updated" }
                                }
                            }
                            tbody {
                                for item in list.items.iter().cloned() {
                                    StockRow { key: "{item.product_id}", item }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn StockRow(item: InventoryItem) -> Element {
    let api = use_api();
    let toasts = use_toast();
    // The row's own copy, so a save shows at once without reloading the whole table.
    let mut current = use_signal(|| item.clone());
    let mut draft = use_signal(|| item.stock.stock.to_string());
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);
    // A reload (filters, demo reset) brings fresh data for the same row.
    use_effect(use_reactive!(|(item,)| {
        draft.set(item.stock.stock.to_string());
        current.set(item);
    }));
    let row = current();
    let changed = draft().trim() != row.stock.stock.to_string();
    let low = row.stock.level != StockLevel::InStock;

    let mut save = move || {
        let parsed = draft().trim().parse::<u32>();
        let request = match parsed {
            Ok(stock) => SetStockRequest { stock },
            Err(_) => {
                error.set(Some(format!("A whole number from 0 to {MAX_STOCK}")));
                return;
            }
        };
        if let Err(e) = request.validate() {
            error.set(
                e.fields
                    .first()
                    .map(|f| f.message.clone())
                    .or(Some(e.message)),
            );
            return;
        }
        error.set(None);
        let product = current.peek().product_id.to_string();
        spawn(async move {
            saving.set(true);
            match api.send::<SetStock>(&[product.as_str()], &request).await {
                Ok(updated) => {
                    toasts.success(
                        format!("{}: stock set to {}", updated.name, updated.stock.stock),
                        ToastOptions::new(),
                    );
                    draft.set(updated.stock.stock.to_string());
                    current.set(updated);
                }
                Err(e) => {
                    error.set(Some(e.message.clone()));
                    toasts.error(e.message, ToastOptions::new());
                }
            }
            saving.set(false);
        });
    };

    rsx! {
        tr { class: if low { "bg-warning/5" } else { "hover:bg-base-200" },
            td { class: "max-w-72",
                div { class: "flex flex-col",
                    Link { to: Route::Product { id: row.product_id.to_string() }, class: "link link-hover truncate font-medium", "{row.name}" }
                    span { class: "text-xs text-base-content/60", Mono { "{row.sku}" } " · {row.tier.label()}" }
                }
            }
            td { class: "whitespace-nowrap", "{row.category.label()}" }
            td { class: "text-right", MoneyText { amount: row.price } }
            td { StockBadge { stock: row.stock } }
            td {
                form {
                    class: "flex items-center gap-1.5",
                    // Our own message, not the browser's tooltip, for out-of-range stock.
                    novalidate: true,
                    onsubmit: move |e| {
                        e.prevent_default();
                        save();
                    },
                    input {
                        r#type: "number",
                        min: "0",
                        max: "{MAX_STOCK}",
                        class: "input input-sm w-20 tabular-nums",
                        class: if error().is_some() { "input-error" },
                        aria_label: "Stock of {row.name}",
                        value: "{draft}",
                        oninput: move |e| draft.set(e.value()),
                    }
                    if changed {
                        button { r#type: "submit", class: "btn btn-primary btn-xs", disabled: saving(),
                            if saving() { span { class: "loading loading-spinner loading-xs" } }
                            "Save"
                        }
                        button {
                            r#type: "button",
                            class: "btn btn-ghost btn-xs",
                            onclick: move |_| {
                                draft.set(current.peek().stock.stock.to_string());
                                error.set(None);
                            },
                            "Undo"
                        }
                    }
                }
                if let Some(message) = error() {
                    span { class: "text-xs text-error", "{message}" }
                }
            }
            td { class: "text-right tabular-nums", "{row.reserved}" }
            td { class: "text-right tabular-nums text-base-content/60", "{row.default_stock}" }
            td { class: "whitespace-nowrap text-base-content/60", "{ago(row.updated_at)}" }
        }
    }
}
