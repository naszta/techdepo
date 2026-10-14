use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;

/// daisyUI's colour axis for a badge.
///
/// [`BadgeColor::Default`] emits no class, which is daisyUI's uncoloured badge
/// rather than a synonym for [`BadgeColor::Neutral`].
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum BadgeColor {
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

impl BadgeColor {
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
            Self::Neutral => "badge-neutral",
            Self::Primary => "badge-primary",
            Self::Secondary => "badge-secondary",
            Self::Accent => "badge-accent",
            Self::Info => "badge-info",
            Self::Success => "badge-success",
            Self::Warning => "badge-warning",
            Self::Error => "badge-error",
        }
    }
}

/// daisyUI's size axis for a badge.
///
/// [`BadgeSize::Default`] emits no class, which renders at the same size as
/// daisyUI's explicit `badge-md`.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum BadgeSize {
    Xs,
    Sm,
    #[default]
    Default,
    Lg,
    Xl,
}

impl BadgeSize {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Xs, Self::Sm, Self::Default, Self::Lg, Self::Xl];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Xs => "badge-xs",
            Self::Sm => "badge-sm",
            Self::Default => "",
            Self::Lg => "badge-lg",
            Self::Xl => "badge-xl",
        }
    }
}

/// daisyUI's appearance axis for a badge.
///
/// [`BadgeAppearance::Default`] emits no class, which is daisyUI's filled
/// badge rather than a synonym for one of its named styles.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum BadgeAppearance {
    #[default]
    Default,
    Outline,
    Dash,
    Soft,
    Ghost,
}

impl BadgeAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[
        Self::Default,
        Self::Outline,
        Self::Dash,
        Self::Soft,
        Self::Ghost,
    ];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Outline => "badge-outline",
            Self::Dash => "badge-dash",
            Self::Soft => "badge-soft",
            Self::Ghost => "badge-ghost",
        }
    }
}

/// A small label for a count, status or tag, styled with daisyUI's `badge`
/// classes.
///
/// Classes passed by the caller concatenate with the badge's own; every other
/// attribute the caller passes overrides the badge's.
#[component]
pub fn Badge(
    /// daisyUI's colour axis.
    #[props(default)]
    color: BadgeColor,
    /// daisyUI's size axis.
    #[props(default)]
    size: BadgeSize,
    /// daisyUI's appearance axis.
    #[props(default)]
    appearance: BadgeAppearance,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let color = color.class();
    let size = size.class();
    let appearance = appearance.class();

    let base = attributes!(span {
        class: "badge {color} {size} {appearance}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        span { ..merged, {children} }
    }
}
