//! The demo controls' own look (design.md, "Demo controls style"): amber, dashed, a flask icon.
//!
//! Deliberately built from Tailwind's amber palette rather than a theme colour, so a simulation
//! lever never looks like shop UI. Everything that is a lever, not shop behaviour, goes through
//! these components: the drawer, its sections and its buttons.

use dioxus::prelude::*;

use crate::browser;
use crate::ui::icons::{Icon, IconName};

/// A button inside the demo controls.
pub const DEMO_BUTTON: &str = "btn btn-sm border-amber-600/50 bg-white/70 text-amber-950 shadow-none hover:border-amber-700 hover:bg-amber-100 dark:border-amber-400/50 dark:bg-amber-900/40 dark:text-amber-50 dark:hover:bg-amber-900/70";

/// The drawer's open state survives reloads and order changes (a per-viewer convenience).
const OPEN_KEY: &str = "techdepo.demo-drawer.open";

/// Set on `<html>` while the drawer is open.
pub const DRAWER_ATTRIBUTE: &str = "data-demo-open";

/// The floating flask button and the right-side drawer it opens.
#[component]
pub fn DemoDrawer(
    /// What the drawer drives, under the title (default: "Simulate what happens outside the
    /// shop").
    #[props(default)]
    subtitle: Option<String>,
    children: Element,
) -> Element {
    let mut open = use_signal(|| browser::load_string(OPEN_KEY).as_deref() == Some("1"));
    let mut set_open = move |value: bool| {
        open.set(value);
        browser::save_string(OPEN_KEY, if value { "1" } else { "0" });
    };
    // `data-demo-open` on <html> lets the layout make room for the drawer from `lg` on instead
    // of hiding content under it (see `layouts::sidebar`); below `lg` it is a modal with a
    // backdrop.
    use_effect(move || browser::set_root_attribute(DRAWER_ATTRIBUTE, open().then_some("")));
    use_drop(|| browser::set_root_attribute(DRAWER_ATTRIBUTE, None));
    let subtitle = subtitle.unwrap_or_else(|| "Simulate what happens outside the shop".to_owned());

    rsx! {
        if !open() {
            button {
                r#type: "button",
                class: "btn fixed bottom-5 right-5 z-30 rounded-full border-2 border-dashed border-amber-600 bg-amber-50 text-amber-950 shadow-sm hover:bg-amber-100 dark:border-amber-500 dark:bg-amber-950 dark:text-amber-50 dark:hover:bg-amber-900",
                aria_controls: "demo-drawer",
                aria_expanded: "false",
                onclick: move |_| set_open(true),
                Icon { name: IconName::Flask, class: "size-5" }
                "Demo controls"
            }
        } else {
            div {
                class: "fixed inset-0 z-40 bg-base-content/20 lg:hidden",
                onclick: move |_| set_open(false),
            }
            aside {
                id: "demo-drawer",
                aria_label: "Demo controls",
                class: "fixed inset-y-0 right-0 z-50 flex w-full max-w-sm flex-col overflow-y-auto border-l-2 border-dashed border-amber-600 bg-amber-50 text-amber-950 dark:border-amber-500 dark:bg-[#1f1608] dark:text-amber-50",
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Escape {
                        set_open(false);
                    }
                },
                header { class: "sticky top-0 z-10 flex items-start gap-3 border-b border-dashed border-amber-600/50 bg-amber-50 p-4 dark:bg-[#1f1608]",
                    div { class: "rounded-lg border border-amber-600/40 bg-amber-100 p-2 text-amber-800 dark:bg-amber-900/50 dark:text-amber-200",
                        Icon { name: IconName::Flask, class: "size-5" }
                    }
                    div { class: "flex-1",
                        h2 { class: "font-semibold", "Demo controls" }
                        p { class: "text-xs text-amber-900/70 dark:text-amber-100/70", "{subtitle}" }
                    }
                    button {
                        r#type: "button",
                        class: "btn btn-ghost btn-sm btn-square text-amber-950 hover:bg-amber-100 dark:text-amber-50 dark:hover:bg-amber-900/60",
                        aria_label: "Close demo controls",
                        autofocus: true,
                        onclick: move |_| set_open(false),
                        Icon { name: IconName::X, class: "size-4" }
                    }
                }
                div { class: "flex flex-col gap-5 p-4 pb-10", {children} }
            }
        }
    }
}

/// A dashed amber card on a real page: demo controls and simulation truths (hidden outcomes,
/// armed faults), never mistaken for shop data (design.md, principle 5).
#[component]
pub fn DemoFrame(children: Element) -> Element {
    rsx! {
        section { class: "flex flex-col gap-3 rounded-box border-2 border-dashed border-amber-600 bg-amber-50 p-4 text-amber-950 dark:border-amber-500 dark:bg-[#1f1608] dark:text-amber-50",
            {children}
        }
    }
}

/// One titled block of the drawer.
#[component]
pub fn DemoSection(
    title: String,
    #[props(default)] icon: Option<IconName>,
    children: Element,
) -> Element {
    rsx! {
        section { class: "flex flex-col gap-2",
            h3 { class: "flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wider text-amber-800 dark:text-amber-300",
                if let Some(icon) = icon {
                    Icon { name: icon, class: "size-3.5" }
                }
                "{title}"
            }
            {children}
        }
    }
}
