use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;

/// daisyUI's inline placement axis for an [`IndicatorItem`].
///
/// Every value emits a class, the default included. daisyUI's unclassed item
/// uses the physical right edge, while explicit `indicator-end` follows the
/// writing direction under RTL.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum IndicatorInline {
    Start,
    Center,
    /// The inline end edge, and daisyUI's own default.
    #[default]
    End,
}

impl IndicatorInline {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Start, Self::Center, Self::End];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Start => "indicator-start",
            Self::Center => "indicator-center",
            Self::End => "indicator-end",
        }
    }
}

/// daisyUI's block placement axis for an [`IndicatorItem`].
///
/// Every value emits a class, the default included, so both dimensions of an
/// item's position are explicit.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum IndicatorBlock {
    /// The block start edge, and daisyUI's own default.
    #[default]
    Top,
    Middle,
    Bottom,
}

impl IndicatorBlock {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Top, Self::Middle, Self::Bottom];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Top => "indicator-top",
            Self::Middle => "indicator-middle",
            Self::Bottom => "indicator-bottom",
        }
    }
}

/// A relatively positioned container carrying daisyUI's `indicator` class.
///
/// Write one or more [`IndicatorItem`] parts beside the content they decorate.
/// Classes passed by the caller concatenate with the container's own; every
/// other attribute the caller passes overrides the container's.
#[component]
pub fn Indicator(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div { class: "indicator" });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// Overlaid caller content inside an [`Indicator`], carrying daisyUI's
/// `indicator-item` and placement classes.
///
/// Badge or status classes passed by the caller concatenate onto this same
/// element. A control remains a child so it keeps its native element.
#[component]
pub fn IndicatorItem(
    /// daisyUI's inline placement axis, which follows the writing direction.
    #[props(default)]
    inline: IndicatorInline,
    /// daisyUI's block placement axis.
    #[props(default)]
    block: IndicatorBlock,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Option<Element>,
) -> Element {
    let inline = inline.class();
    let block = block.class();

    let base = attributes!(div {
        class: "indicator-item {inline} {block}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}
