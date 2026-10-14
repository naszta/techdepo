//! Home: a short hero, the category grid, three reasons. Nothing else, except a one-line
//! pointer to the demo guide for signed-out visitors (the guide is the landing page).

use dioxus::prelude::*;
use techdepo_api_types::Category;

use crate::auth::SignedOut;
use crate::routes::Route;
use crate::state::build::catalog;
use crate::ui::product::{category_icon, category_plural};
use crate::ui::{Icon, IconName, MoneyText};

#[component]
pub fn Home() -> Element {
    rsx! {
        section { class: "flex flex-col items-start gap-6 py-8 sm:py-14",
            Link { to: Route::DemoGuide {}, class: "badge badge-soft badge-primary gap-1 hover:underline",
                "Demo shop · fictional brands · how it works"
                Icon { name: IconName::ArrowRight, class: "size-3" }
            }
            h1 { class: "max-w-3xl text-4xl font-semibold tracking-tight sm:text-5xl",
                "PC parts, explained. "
                span { class: "text-primary", "Builds that just work." }
            }
            p { class: "max-w-2xl text-lg text-base-content/70",
                "Every part comes with specs that matter and a compatibility check against your build, so you know why something fits before you order."
            }
            div { class: "flex flex-wrap gap-3",
                Link { to: Route::Builder {}, class: "btn btn-primary",
                    Icon { name: IconName::Wrench }
                    "Build a PC"
                }
                Link { to: Route::Category { category: Category::Cpu.as_str().to_string() }, class: "btn btn-outline",
                    "Browse parts"
                    Icon { name: IconName::ArrowRight }
                }
            }
            SignedOut {
                p { class: "flex items-start gap-2 text-sm text-base-content/70",
                    Icon { name: IconName::Flask, class: "mt-0.5 size-4 shrink-0 text-warning" }
                    span {
                        "First time here? This shop is a demo of code, workflows, AI agents and human decisions working together. "
                        Link { to: Route::DemoGuide {}, class: "link link-primary", "See how to try it" }
                        "."
                    }
                }
            }
        }

        section { class: "py-6", aria_labelledby: "categories",
            h2 { id: "categories", class: "mb-4 text-lg font-semibold", "Shop by category" }
            div { class: "grid grid-cols-2 gap-3 sm:grid-cols-4",
                for &category in Category::ALL {
                    CategoryTile { category }
                }
            }
        }

        section { class: "grid gap-6 py-10 md:grid-cols-3",
            Reason {
                icon: IconName::ShieldCheck,
                title: "Compatibility you can trust",
                text: "Sockets, memory, clearances, power and connectors are checked for every combination, and the reason is always spelled out.",
            }
            Reason {
                icon: IconName::Info,
                title: "Specs, explained",
                text: "Each part says who it's for, its pros and cons, and the questions people actually ask. No marketing fog.",
            }
            Reason {
                icon: IconName::LifeBuoy,
                title: "Support that knows your order",
                text: "Email us like you would any shop. Simple requests are handled at once; a person decides anything that moves money.",
            }
        }
    }
}

#[component]
fn CategoryTile(category: Category) -> Element {
    let products = catalog().by_category(category);
    let from = products.iter().map(|p| p.price).min();
    rsx! {
        Link {
            to: Route::Category { category: category.as_str().to_string() },
            class: "group flex flex-col gap-3 rounded-box border border-base-300 bg-base-100 p-4 transition-colors hover:border-primary/50",
            div { class: "grid size-12 place-items-center rounded-box bg-primary/8 text-primary",
                Icon { name: category_icon(category), class: "size-6 stroke-[1.5]" }
            }
            div { class: "flex flex-col",
                span { class: "font-medium group-hover:text-primary", "{category_plural(category)}" }
                span { class: "text-sm text-base-content/60",
                    "{products.len()} parts"
                    if let Some(from) = from {
                        " · from "
                        MoneyText { amount: from }
                    }
                }
            }
        }
    }
}

#[component]
fn Reason(icon: IconName, title: &'static str, text: &'static str) -> Element {
    rsx! {
        div { class: "flex flex-col gap-2",
            div { class: "text-primary", Icon { name: icon, class: "size-6" } }
            h3 { class: "font-semibold", "{title}" }
            p { class: "text-base-content/70", "{text}" }
        }
    }
}
