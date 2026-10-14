//! Prices and identifiers.

use dioxus::prelude::*;
use techdepo_api_types::Money;

/// A price in EUR with tabular figures (`€1,435.90`).
#[component]
pub fn MoneyText(amount: Money, #[props(default)] class: String) -> Element {
    rsx! {
        span { class: "tabular-nums whitespace-nowrap {class}", "{amount}" }
    }
}

/// An identifier in Geist Mono: SKUs, order and ticket numbers, IDs.
#[component]
pub fn Mono(#[props(default)] class: String, children: Element) -> Element {
    rsx! {
        span { class: "font-mono text-[0.92em] {class}", {children} }
    }
}

/// A long generated identifier (`pay_4da90be173d5…`) shortened to its prefix and first hex
/// digits, with the full value on hover; keeps tables and notes from overflowing.
#[component]
pub fn ShortId(id: String, #[props(default)] class: String) -> Element {
    let short = match id.char_indices().nth(16) {
        Some((cut, _)) => format!("{}…", &id[..cut]),
        None => id.clone(),
    };
    rsx! {
        span { class: "font-mono text-[0.92em] whitespace-nowrap {class}", title: "{id}", "{short}" }
    }
}
