use dioxus::prelude::*;

use crate::routes::Route;
use crate::ui::{EmptyState, IconName};

#[component]
pub fn NotFound(segments: Vec<String>) -> Element {
    let path = format!("/{}", segments.join("/"));
    rsx! {
        EmptyState { message: "There is nothing at {path}.", icon: IconName::Search,
            Link { to: Route::Home {}, class: "btn btn-primary btn-sm", "Go to the shop" }
        }
    }
}
