//! Deep links into the Restate UI from admin pages (local development: the API exposes
//! `restate_ui_url` only for a local Restate).

use dioxus::prelude::*;
use techdepo_api_types::restate_ui::RestateUiLink;

use crate::session::use_session;
use crate::ui::{Icon, IconName};

/// The Restate UI base URL, when the API exposes one.
pub fn use_restate_ui() -> Option<String> {
    use_session()
        .config
        .read()
        .as_ref()
        .and_then(|c| c.restate_ui_url.clone())
}

/// "Restate ↗": opens `link` in the Restate UI in a new tab; renders nothing without a
/// Restate UI URL.
#[component]
pub fn RestateLink(
    link: RestateUiLink,
    #[props(default = "Restate".to_owned())] label: String,
    #[props(default)] class: String,
) -> Element {
    let Some(base) = use_restate_ui() else {
        return rsx! {};
    };
    let title = match &link {
        RestateUiLink::Invocation(_) => "Open the invocation in the Restate UI",
        RestateUiLink::Object { .. } => {
            "Open the virtual object (state and invocations) in the Restate UI"
        }
        RestateUiLink::Workflow { .. } => "Open the workflow run in the Restate UI",
    };
    rsx! {
        a {
            class: "link link-hover inline-flex items-center gap-0.5 {class}",
            href: link.url(&base),
            target: "_blank",
            rel: "noopener",
            title,
            "{label}"
            Icon { name: IconName::ExternalLink, class: "size-3" }
        }
    }
}
