use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;

/// daisyUI's direction axis for a timeline.
///
/// Both values emit an explicit class so the direction is carried by the
/// element rather than inferred from daisyUI's horizontal default.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TimelineDirection {
    #[default]
    Horizontal,
    Vertical,
}

impl TimelineDirection {
    /// Every value of this Axis, in the order the Preview renders them.
    pub const ALL: &'static [Self] = &[Self::Horizontal, Self::Vertical];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Horizontal => "timeline-horizontal",
            Self::Vertical => "timeline-vertical",
        }
    }
}

/// Whether a timeline places every event on the same side of its line.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TimelineCompact {
    #[default]
    Default,
    Compact,
}

impl TimelineCompact {
    /// Every value of this Axis, in the order the Preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::Compact];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Compact => "timeline-compact",
        }
    }
}

/// Whether timeline markers snap toward the start of their items.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TimelineSnap {
    #[default]
    Default,
    Icon,
}

impl TimelineSnap {
    /// Every value of this Axis, in the order the Preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::Icon];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Icon => "timeline-snap-icon",
        }
    }
}

/// daisyUI's shared appearance axis for timeline start and end content.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TimelineContentAppearance {
    #[default]
    Default,
    Box,
}

impl TimelineContentAppearance {
    /// Every value of this Axis, in the order the Preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::Box];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Box => "timeline-box",
        }
    }
}

/// A chronological list carrying daisyUI's `timeline` classes.
///
/// [`TimelineItem`] elements must be direct children so daisyUI can lay them
/// out. Classes passed by the caller concatenate with the timeline's own;
/// every other attribute the caller passes overrides the timeline's.
#[component]
pub fn Timeline(
    /// daisyUI's direction axis.
    #[props(default)]
    direction: TimelineDirection,
    /// Whether every event sits on the same side of the line.
    #[props(default)]
    compact: TimelineCompact,
    /// Whether markers snap toward the start of their items.
    #[props(default)]
    snap: TimelineSnap,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let direction = direction.class();
    let compact = compact.class();
    let snap = snap.class();

    let base = attributes!(ul {
        class: "timeline {direction} {compact} {snap}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        ul { ..merged, {children} }
    }
}

/// One event in a [`Timeline`], rendered as its direct `li` child.
#[component]
pub fn TimelineItem(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(li {});
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        li { ..merged, {children} }
    }
}

/// Content on the start side of a [`TimelineItem`].
#[component]
pub fn TimelineStart(
    /// daisyUI's appearance axis, which draws the content as a box or leaves it
    /// bare.
    #[props(default)]
    appearance: TimelineContentAppearance,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let appearance = appearance.class();
    let base = attributes!(div {
        class: "timeline-start {appearance}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// Content on the line through a [`TimelineItem`], usually a marker icon.
#[component]
pub fn TimelineMiddle(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div {
        class: "timeline-middle",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// Content on the end side of a [`TimelineItem`].
#[component]
pub fn TimelineEnd(
    /// daisyUI's appearance axis, which draws the content as a box or leaves it
    /// bare.
    #[props(default)]
    appearance: TimelineContentAppearance,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let appearance = appearance.class();
    let base = attributes!(div {
        class: "timeline-end {appearance}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// A decorative line segment connecting adjacent [`TimelineItem`] elements.
///
/// Place this direct `hr` child before an item's content for its incoming
/// segment, after its content for its outgoing segment, or in both positions
/// for a middle item. Callers decide whether the line is decorative and can
/// hide it from assistive technology with `aria-hidden="true"`.
#[component]
pub fn TimelineConnector(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let base = attributes!(hr {});
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        hr { ..merged }
    }
}
