//! The light/dark switch in every header (design.md, "Theme").
//!
//! The first visit follows `prefers-color-scheme` (daisyUI's `prefersdark` theme). A click sets
//! `data-theme` on `<html>` and remembers the choice; the inline script in `index.html`
//! re-applies it before the WASM loads, so there is no flash of the wrong theme.

use dioxus::prelude::*;

use crate::browser;
use crate::ui::icons::{Icon, IconName};

const THEME_KEY: &str = "techdepo.theme";
const LIGHT: &str = "techdepo";
const DARK: &str = "techdepo-dark";

fn current_is_dark() -> bool {
    match browser::load_string(THEME_KEY).as_deref() {
        Some(DARK) => true,
        Some(LIGHT) => false,
        _ => browser::prefers_dark(),
    }
}

/// A 40 px icon button: a moon in light mode, a sun in dark mode.
#[component]
pub fn ThemeToggle() -> Element {
    let mut dark = use_signal(current_is_dark);
    let target = if dark() { "light" } else { "dark" };

    rsx! {
        button {
            r#type: "button",
            class: "btn btn-ghost btn-square size-10 swap swap-rotate",
            class: if dark() { "swap-active" },
            aria_label: "Switch to {target} theme",
            title: "Switch to {target} theme",
            onclick: move |_| {
                let next = !dark();
                let theme = if next { DARK } else { LIGHT };
                browser::set_root_attribute("data-theme", Some(theme));
                browser::save_string(THEME_KEY, theme);
                dark.set(next);
            },
            // `swap-off` shows in light mode, `swap-on` in dark mode.
            span { class: "swap-off", Icon { name: IconName::Moon, class: "size-5" } }
            span { class: "swap-on", Icon { name: IconName::Sun, class: "size-5" } }
        }
    }
}
