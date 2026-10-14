use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;

/// daisyUI's size axis for a card, which sizes its body and title.
///
/// [`CardSize::Default`] emits no class, which renders at the same size as
/// daisyUI's explicit `card-md`.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum CardSize {
    Xs,
    Sm,
    #[default]
    Default,
    Lg,
    Xl,
}

impl CardSize {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Xs, Self::Sm, Self::Default, Self::Lg, Self::Xl];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Xs => "card-xs",
            Self::Sm => "card-sm",
            Self::Default => "",
            Self::Lg => "card-lg",
            Self::Xl => "card-xl",
        }
    }
}

/// daisyUI's border axis for a card.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum CardBorder {
    #[default]
    Default,
    Solid,
    Dashed,
}

impl CardBorder {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::Solid, Self::Dashed];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Solid => "card-border",
            Self::Dashed => "card-dash",
        }
    }
}

/// daisyUI's layout axis for a card.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum CardLayout {
    #[default]
    Default,
    Side,
    ImageFull,
}

impl CardLayout {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::Side, Self::ImageFull];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Side => "card-side",
            Self::ImageFull => "image-full",
        }
    }
}

/// A titled box with a body and actions, carrying daisyUI's `card` classes.
///
/// Classes passed by the caller concatenate with the card's own; every other
/// attribute the caller passes overrides the card's.
#[component]
pub fn Card(
    /// daisyUI's size axis, which sizes the body and title.
    #[props(default)]
    size: CardSize,
    /// daisyUI's border axis; the default is borderless.
    #[props(default)]
    border: CardBorder,
    /// daisyUI's layout axis, which is where an image sits relative to the
    /// body.
    #[props(default)]
    layout: CardLayout,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let size = size.class();
    let border = border.class();
    let layout = layout.class();

    let base = attributes!(div {
        class: "card {size} {border} {layout}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// The padded content inside a [`Card`], carrying daisyUI's `card-body` class.
#[component]
pub fn CardBody(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div { class: "card-body" });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// A [`Card`]'s heading, carrying daisyUI's `card-title` class.
#[component]
pub fn CardTitle(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(h2 {
        class: "card-title"
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        h2 { ..merged, {children} }
    }
}

/// The row of controls in a [`Card`], carrying daisyUI's `card-actions` class.
#[component]
pub fn CardActions(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div {
        class: "card-actions"
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}
