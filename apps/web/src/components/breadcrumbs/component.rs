use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;

/// A breadcrumb navigation landmark carrying daisyUI's `breadcrumbs` class.
///
/// The caller must name the landmark with `aria-label` or `aria-labelledby`.
/// Classes passed by the caller concatenate with the component's own; every
/// other attribute the caller passes overrides the component's.
#[component]
pub fn Breadcrumbs(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(nav {
        class: "breadcrumbs"
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        nav { ..merged, {children} }
    }
}

/// The ordered list directly inside [`Breadcrumbs`].
#[component]
pub fn BreadcrumbsList(
    #[props(extends = GlobalAttributes)]
    #[props(extends = ol)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let merged = merge_attributes(vec![attributes]);

    rsx! {
        ol { ..merged, {children} }
    }
}

/// One list item directly inside a [`BreadcrumbsList`].
///
/// Write links and current-page text as this part's direct children so
/// daisyUI's child selectors and generated separators reach them.
#[component]
pub fn BreadcrumbsItem(
    #[props(extends = GlobalAttributes)]
    #[props(extends = li)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let merged = merge_attributes(vec![attributes]);

    rsx! {
        li { ..merged, {children} }
    }
}
