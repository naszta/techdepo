//! Loading, empty and error states, and the demo banner.

use dioxus::prelude::*;
use techdepo_api_types::{ApiError, ErrorCode};

use crate::routes::Route;
use crate::ui::icons::{Icon, IconName};

/// The thin, permanent banner at the very top: neutral, not alarming. It links the demo guide
/// (the landing page), so the explanation is one click away on every page.
#[component]
pub fn DemoBanner() -> Element {
    rsx! {
        div { class: "bg-base-200 border-b border-base-300 text-xs text-base-content/70 text-center py-1.5 px-4",
            "Demo shop – not a real store. Nothing you order here ships or costs money. "
            Link { to: Route::DemoGuide {}, class: "link link-hover font-medium text-base-content/90 whitespace-nowrap",
                "How this demo works"
            }
        }
    }
}

/// Skeleton blocks while a page loads (never a spinner over the whole page).
#[component]
pub fn PageSkeleton(#[props(default = 3)] rows: usize) -> Element {
    rsx! {
        div { class: "flex flex-col gap-4", aria_busy: "true",
            div { class: "skeleton h-8 w-1/3" }
            for _ in 0..rows {
                div { class: "skeleton h-24 w-full" }
            }
        }
    }
}

/// One sentence and one call to action.
#[component]
pub fn EmptyState(
    message: String,
    #[props(default = IconName::Package)] icon: IconName,
    children: Element,
) -> Element {
    rsx! {
        div { class: "flex flex-col items-center justify-center gap-4 rounded-box border border-dashed border-base-300 px-6 py-14 text-center",
            div { class: "rounded-full bg-base-200 p-3 text-base-content/60",
                Icon { name: icon, class: "size-6" }
            }
            p { class: "text-base-content/70", "{message}" }
            {children}
        }
    }
}

/// An API error as an alert; `on_retry` adds a retry button for retryable errors.
#[component]
pub fn ErrorAlert(error: ApiError, on_retry: Option<EventHandler<()>>) -> Element {
    let retryable = error.code.is_retryable() || error.code == ErrorCode::Internal;
    rsx! {
        div { role: "alert", class: "alert alert-error alert-soft",
            Icon { name: IconName::CircleX, class: "size-5" }
            div { class: "flex flex-col",
                span { class: "font-medium", "{error.code.label()}" }
                span { class: "text-sm", "{error.message}" }
            }
            if let (true, Some(retry)) = (retryable, on_retry) {
                button { class: "btn btn-sm", onclick: move |_| retry.call(()), "Try again" }
            }
        }
    }
}
