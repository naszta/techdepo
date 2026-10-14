use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;

/// daisyUI's colour axis for a status indicator.
///
/// [`StatusColor::Default`] emits no modifier, which is daisyUI's uncoloured
/// status rather than a synonym for [`StatusColor::Neutral`].
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum StatusColor {
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

impl StatusColor {
    /// Every value of this Axis, in the order the Preview renders them.
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
            Self::Neutral => "status-neutral",
            Self::Primary => "status-primary",
            Self::Secondary => "status-secondary",
            Self::Accent => "status-accent",
            Self::Info => "status-info",
            Self::Success => "status-success",
            Self::Warning => "status-warning",
            Self::Error => "status-error",
        }
    }
}

/// daisyUI's size axis for a status indicator.
///
/// [`StatusSize::Default`] emits no modifier and renders at the same medium
/// size as daisyUI's explicit `status-md`.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum StatusSize {
    Xs,
    Sm,
    #[default]
    Default,
    Lg,
    Xl,
}

impl StatusSize {
    /// Every value of this Axis, from smallest to largest.
    pub const ALL: &'static [Self] = &[Self::Xs, Self::Sm, Self::Default, Self::Lg, Self::Xl];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Xs => "status-xs",
            Self::Sm => "status-sm",
            Self::Default => "",
            Self::Lg => "status-lg",
            Self::Xl => "status-xl",
        }
    }
}

/// An empty status indicator styled with daisyUI's `status` classes.
///
/// The meaning, accessibility semantics and any animation belong to the
/// caller. Classes passed by the caller concatenate with the indicator's own;
/// every other attribute the caller passes overrides the indicator's.
#[component]
pub fn Status(
    /// daisyUI's colour axis.
    #[props(default)]
    color: StatusColor,
    /// daisyUI's size axis.
    #[props(default)]
    size: StatusSize,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let color = color.class();
    let size = size.class();

    let base = attributes!(span {
        class: "status {color} {size}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        span { ..merged }
    }
}
