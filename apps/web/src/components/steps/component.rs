use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;

/// daisyUI's direction axis for a [`Steps`] list.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum StepsDirection {
    #[default]
    Horizontal,
    Vertical,
}

impl StepsDirection {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Horizontal, Self::Vertical];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Horizontal => "steps-horizontal",
            Self::Vertical => "steps-vertical",
        }
    }
}

/// daisyUI's colour axis for a [`Step`] and the connector leading to it.
///
/// [`StepColor::Default`] emits no class, which keeps daisyUI's base colour.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum StepColor {
    #[default]
    Default,
    Neutral,
    Primary,
    Secondary,
    Accent,
    Info,
    Success,
    Warning,
    Error,
}

impl StepColor {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[
        Self::Default,
        Self::Neutral,
        Self::Primary,
        Self::Secondary,
        Self::Accent,
        Self::Info,
        Self::Success,
        Self::Warning,
        Self::Error,
    ];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Neutral => "step-neutral",
            Self::Primary => "step-primary",
            Self::Secondary => "step-secondary",
            Self::Accent => "step-accent",
            Self::Info => "step-info",
            Self::Success => "step-success",
            Self::Warning => "step-warning",
            Self::Error => "step-error",
        }
    }
}

/// An ordered process, rendered as daisyUI's `ul.steps`.
///
/// Put [`Step`] children next to one another without wrappers: daisyUI colours
/// connectors through adjacent-sibling selectors. Classes passed by the caller
/// concatenate with the list's own; every other attribute the caller passes
/// overrides the list's.
#[component]
pub fn Steps(
    /// daisyUI's direction axis.
    #[props(default)]
    direction: StepsDirection,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let direction = direction.class();
    let base = attributes!(ul {
        class: "steps {direction}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        ul { ..merged, {children} }
    }
}

/// One direct `li.step` inside [`Steps`].
///
/// Colour is visual only: the caller owns completion and current-step
/// semantics. A caller can replace the generated counter through
/// `data-content`, or by putting one [`StepIcon`] directly inside this part.
#[component]
pub fn Step(
    /// daisyUI's colour axis, which colours the step and the connector leading
    /// to it.
    #[props(default)]
    color: StepColor,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let color = color.class();
    let base = attributes!(li {
        class: "step {color}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        li { ..merged, {children} }
    }
}

/// Caller content replacing a [`Step`]'s generated counter.
///
/// This part must remain a direct child of `Step` for daisyUI's child selector
/// to style it. Classes passed by the caller concatenate with `step-icon`.
#[component]
pub fn StepIcon(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(span { class: "step-icon" });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        span { ..merged, {children} }
    }
}
