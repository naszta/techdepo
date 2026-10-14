use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;

/// An empty block placeholder styled with daisyUI's `skeleton` class.
///
/// Size, shape, layout and accessibility semantics belong to the caller.
/// Classes passed by the caller concatenate with the placeholder's own; every
/// other attribute the caller passes overrides the placeholder's.
#[component]
pub fn Skeleton(#[props(extends = GlobalAttributes)] attributes: Vec<Attribute>) -> Element {
    let base = attributes!(div { class: "skeleton" });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged }
    }
}

/// Text whose glyphs are masked by daisyUI's animated skeleton gradient.
///
/// The caller supplies the real text that gives the placeholder its shape and
/// owns whether it is exposed to assistive technology. Classes passed by the
/// caller concatenate with this component's own; every other attribute the
/// caller passes overrides the component's.
#[component]
pub fn SkeletonText(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(span {
        class: "skeleton skeleton-text",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        span { ..merged, {children} }
    }
}
