use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::label;
use dioxus_primitives::merge_attributes;

/// daisyUI's appearance axis for a label, which is which of its label classes
/// this element carries.
///
/// They are three values of one axis rather than three components because they
/// are classes for the same element, and no two of them go together: `.label`
/// is the caption beside a control, `.fieldset-label` is its deprecated legacy
/// form for a caption inside a fieldset, and `.floating-label` is the one that
/// *wraps* a control and moves its own text as the field fills.
///
/// [`LabelAppearance::None`] emits nothing, for a caption that is styled by the
/// caller or by whatever it is written inside. It is the only value that emits
/// no class, which makes this axis the inverted shape ADR-0004 describes rather
/// than the usual one: there is no such thing as an unclassed daisyUI label,
/// so a `Default` that emitted nothing would be a fourth look rather than
/// daisyUI's own.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum LabelAppearance {
    /// daisyUI's `label`: the muted caption beside a control.
    #[default]
    Label,
    /// daisyUI's deprecated `fieldset-label`, retained for existing callers.
    Fieldset,
    /// daisyUI's `floating-label`, for a label that wraps its control and
    /// carries its own `span`.
    Floating,
    /// No class at all.
    None,
}

impl LabelAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Label, Self::Fieldset, Self::Floating, Self::None];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Label => "label",
            Self::Fieldset => "fieldset-label",
            Self::Floating => "floating-label",
            Self::None => "",
        }
    }
}

/// A caption naming a form control, styled with one of daisyUI's label classes.
///
/// There is no state to bridge. The primitive renders a `label` whose `for`
/// attribute points at the control it names, and that is the whole of its
/// behaviour: no ARIA state, no keyboard handling, no open state. What it buys
/// over writing the element by hand is that the association is a required prop
/// rather than an optional one, so a caption that names nothing does not
/// compile.
///
/// The class this element carries is the whole of what daisyUI needs for the
/// cases where a label sits inside daisyUI's `input` or `select` group: daisyUI
/// matches the ancestor itself, through `.label:is(.input>*,.select>*)`, so
/// nothing extra is emitted for it.
///
/// Classes passed by the caller concatenate with this element's own; every other
/// attribute the caller passes overrides them.
#[component]
pub fn Label(
    /// Which of daisyUI's labels this is.
    #[props(default)]
    appearance: LabelAppearance,
    /// The id of the control this label names, which is what the `for`
    /// attribute points at. Required by the primitive rather than defaulted,
    /// which is the point of using it.
    html_for: ReadSignal<String>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let appearance = appearance.class();

    let base = attributes!(label {
        class: "{appearance}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        label::Label { html_for, attributes: merged, {children} }
    }
}
