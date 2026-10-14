use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;

/// A native form group carrying daisyUI's `fieldset` class.
///
/// The browser owns the group's form association and disabled behaviour. In
/// particular, disabling the fieldset disables its descendant controls except
/// those inside its first [`FieldsetLegend`]. The `form` attribute associates
/// the fieldset itself; it does not implicitly associate descendant controls
/// with a form they are not otherwise in.
///
/// Classes passed by the caller concatenate with the fieldset's own. Native
/// fieldset attributes, including `disabled`, `form`, and `name`, are merged
/// onto the same element and are omitted when the caller does not supply them.
#[component]
pub fn Fieldset(
    #[props(extends = GlobalAttributes)]
    #[props(extends = fieldset)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(fieldset { class: "fieldset" });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        fieldset { ..merged, {children} }
    }
}

/// The caption for a [`Fieldset`], carrying daisyUI's `fieldset-legend` class.
///
/// Keep this as the fieldset's first direct child. The browser uses the first
/// direct `legend` as the group's caption and exempts its descendants from the
/// fieldset's disabled state.
#[component]
pub fn FieldsetLegend(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(legend {
        class: "fieldset-legend"
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        legend { ..merged, {children} }
    }
}
