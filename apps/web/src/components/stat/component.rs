use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;

/// daisyUI's direction axis for a statistics layout.
///
/// Both values emit a class so the layout direction is explicit rather than
/// inferred from the absence of a modifier.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum StatsDirection {
    #[default]
    Horizontal,
    Vertical,
}

impl StatsDirection {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Horizontal, Self::Vertical];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Horizontal => "stats-horizontal",
            Self::Vertical => "stats-vertical",
        }
    }
}

/// A group of statistics, carrying daisyUI's `stats` classes.
///
/// [`Stat`] elements must be direct children so daisyUI can place them and draw
/// dividers between them.
///
/// Classes passed by the caller concatenate with the stats' own; every other
/// attribute the caller passes overrides the stats'.
#[component]
pub fn Stats(
    /// daisyUI's direction axis.
    #[props(default)]
    direction: StatsDirection,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let direction = direction.class();

    let base = attributes!(div {
        class: "stats {direction}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// One statistic in [`Stats`], carrying daisyUI's `stat` class.
///
/// The statistic's compound parts must be direct children so their grid
/// placement applies.
#[component]
pub fn Stat(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div { class: "stat" });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// A statistic's label, carrying daisyUI's `stat-title` class.
#[component]
pub fn StatTitle(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div {
        class: "stat-title"
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// A statistic's primary value, carrying daisyUI's `stat-value` class.
#[component]
pub fn StatValue(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div {
        class: "stat-value"
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// Supporting text for a statistic, carrying daisyUI's `stat-desc` class.
#[component]
pub fn StatDescription(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div { class: "stat-desc" });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// A figure beside a statistic, carrying daisyUI's `stat-figure` class.
#[component]
pub fn StatFigure(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div {
        class: "stat-figure"
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// Controls belonging to a statistic, carrying daisyUI's `stat-actions` class.
#[component]
pub fn StatActions(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div {
        class: "stat-actions"
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}
