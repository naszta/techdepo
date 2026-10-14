use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;
use dioxus_primitives::tooltip;
use dioxus_primitives::{ContentAlign, ContentSide};

/// daisyUI's colour axis for a tooltip, which is what the bubble is filled
/// with.
///
/// [`TooltipColor::Default`] emits no class at all, and unlike every other
/// component here it *is* the neutral one: daisyUI fills an unclassed tooltip
/// with `--color-neutral` and ships no `tooltip-neutral` to ask for it by name,
/// so this axis has no neutral value of its own.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TooltipColor {
    #[default]
    Default,
    Primary,
    Secondary,
    Accent,
    Info,
    Success,
    Warning,
    Error,
}

impl TooltipColor {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[
        Self::Default,
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
            Self::Primary => "tooltip-primary",
            Self::Secondary => "tooltip-secondary",
            Self::Accent => "tooltip-accent",
            Self::Info => "tooltip-info",
            Self::Success => "tooltip-success",
            Self::Warning => "tooltip-warning",
            Self::Error => "tooltip-error",
        }
    }
}

/// daisyUI's side axis for a tooltip, which is the side of the trigger the
/// bubble opens on.
///
/// Every value emits a class, including the default one: the inverse of the
/// usual convention, and for a reason of this component's own that the
/// dropdown's ADR-0008 rhymes with. daisyUI's base rule places the bubble where
/// `tooltip-top` places it, so the class looks redundant; the tail does not
/// follow. It is a pseudo-element of this element with no position in the base
/// rule at all, and only a placement class puts it anywhere, so an unplaced
/// tooltip draws its tail somewhere else entirely.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TooltipSide {
    /// Above the trigger, which is where daisyUI puts an unplaced tooltip.
    #[default]
    Top,
    /// Under the trigger.
    Bottom,
    /// To the left of the trigger, in either writing direction; daisyUI's
    /// horizontal placements are physical rather than logical.
    Left,
    /// To the right of the trigger, mirroring [`TooltipSide::Left`].
    Right,
}

impl TooltipSide {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Top, Self::Bottom, Self::Left, Self::Right];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Top => "tooltip-top",
            Self::Bottom => "tooltip-bottom",
            Self::Left => "tooltip-left",
            Self::Right => "tooltip-right",
        }
    }

    /// What the primitive is told, so that the `data-side` it reports says the
    /// same thing daisyUI did.
    const fn side(self) -> ContentSide {
        match self {
            Self::Top => ContentSide::Top,
            Self::Bottom => ContentSide::Bottom,
            Self::Left => ContentSide::Left,
            Self::Right => ContentSide::Right,
        }
    }
}

/// daisyUI's align axis for a tooltip, which is where the bubble sits along
/// the side [`TooltipSide`] opened it on.
///
/// Every value emits a class here too, for the reason [`TooltipSide`]
/// records: the tail is placed by these classes as much as the bubble is.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TooltipAlign {
    /// The inline start edge, which follows the writing direction: the left in
    /// a left-to-right document, the right in a right-to-left one.
    Start,
    /// Centred on the trigger, which is where daisyUI puts an unaligned
    /// tooltip.
    #[default]
    Center,
    /// The inline end edge, mirroring [`TooltipAlign::Start`].
    End,
}

impl TooltipAlign {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Start, Self::Center, Self::End];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Start => "tooltip-start",
            Self::Center => "tooltip-center",
            Self::End => "tooltip-end",
        }
    }

    /// What the primitive is told, so that the `data-align` it reports says the
    /// same thing daisyUI did.
    const fn align(self) -> ContentAlign {
        match self {
            Self::Start => ContentAlign::Start,
            Self::Center => ContentAlign::Center,
            Self::End => ContentAlign::End,
        }
    }
}

/// Where the bubble was placed, as [`Tooltip`] holds it for [`TooltipContent`]
/// to read.
///
/// daisyUI places the bubble from classes on the element the two axes are props
/// of, and the primitive reports the placement as `data-side` and `data-align`
/// on the bubble itself: two elements, one decision. Passing it down keeps the
/// attributes truthful without asking a caller to say the same thing twice.
///
/// It is provided as a memo rather than as a value, because a context is
/// created once and these are props: a caller who moves a tooltip would
/// otherwise move the bubble and leave the attributes saying where it used to
/// be.
#[derive(Copy, Clone, PartialEq)]
struct Placement {
    side: TooltipSide,
    align: TooltipAlign,
}

/// The outer element of a tooltip, carrying daisyUI's `tooltip` classes.
///
/// This is where the open state is **lifted** (ADR-0006). daisyUI reveals a
/// bubble on `.tooltip-open`, on `:hover`, or on `:has(:focus-visible)`. The
/// last two agree with when the primitive opens on its own, so a tooltip that
/// is only ever pointed at or tabbed to would work without the lift, but a
/// controlled one, and one that opens with the page, reach neither. So the
/// class is emitted from Rust and this component owns the state to emit it: it
/// seeds a signal from `default_open`, always hands the primitive a controlled
/// value, and intercepts the change callback.
///
/// Classes passed by the caller concatenate with this element's own; every
/// other attribute the caller passes overrides them.
#[component]
pub fn Tooltip(
    /// daisyUI's colour axis.
    #[props(default)]
    color: TooltipColor,
    /// daisyUI's side axis.
    #[props(default)]
    side: TooltipSide,
    /// daisyUI's align axis.
    #[props(default)]
    align: TooltipAlign,
    /// The controlled open state of the tooltip.
    #[props(default)]
    open: ReadSignal<Option<bool>>,
    /// The state the tooltip starts in when it is not controlled.
    #[props(default)]
    default_open: bool,
    /// Called when the open state changes.
    #[props(default)]
    on_open_change: Callback<bool>,
    /// Whether the tooltip is disabled, which leaves the trigger alone and the
    /// bubble unopenable.
    #[props(default)]
    disabled: ReadSignal<bool>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let mut uncontrolled = use_signal(|| default_open);

    // Both are read here rather than inside the markup, and eagerly rather
    // than only when the other is absent, so that this component subscribes to
    // whichever of them is driving and re-renders, which is what puts the
    // modifier class below on the element and takes it off again.
    let is_open = open().unwrap_or(uncontrolled());

    let placed = use_memo(use_reactive!(|(side, align)| Placement { side, align }));
    use_context_provider(|| placed);

    let color = color.class();
    let side = side.class();
    let align = align.class();
    // Tier 2, and the reason the state is lifted: daisyUI's own modifier
    // class, emitted from Rust as a complete literal so that Tailwind's
    // scanner sees it too.
    let state = if is_open { "tooltip-open" } else { "" };

    let base = attributes!(div {
        class: "tooltip {color} {side} {align} {state}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        tooltip::Tooltip {
            open: Some(is_open),
            on_open_change: move |open| {
                uncontrolled.set(open);
                on_open_change.call(open);
            },
            disabled,
            attributes: merged,
            {children}
        }
    }
}

/// What a tooltip is attached to.
///
/// Nothing is emitted here: daisyUI's tooltip styles the bubble and positions it
/// against the element that carries `tooltip`, and has nothing to say about what
/// is inside. A trigger that should look like something is the caller's, which
/// is what `as` is for: the primitive hands the whole merged attribute list to
/// a callback that renders the element in this one's place, so a button can be
/// the trigger rather than sit inside one.
///
/// The primitive's default element is a `div` carrying `tabindex="0"`, which is
/// what makes a tooltip reachable by keyboard at all. A trigger rendered
/// through `as` should be focusable by being the kind of element that already
/// is, since the attribute list carries no `tabindex` of its own.
#[component]
pub fn TooltipTrigger(
    /// Renders the trigger as an element of the caller's rather than as the
    /// primitive's `div`, with every attribute this component and the primitive
    /// would have put on it.
    #[props(default)]
    r#as: Option<Callback<Vec<Attribute>, Element>>,
    /// The id of this element.
    #[props(default)]
    id: Option<String>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        tooltip::TooltipTrigger { r#as, id, attributes, {children} }
    }
}

/// The bubble itself, carrying daisyUI's `tooltip-content` class.
///
/// It must stay a direct child of [`Tooltip`]: every rule that paints,
/// positions or reveals the bubble reaches it through
/// `.tooltip > .tooltip-content`.
///
/// Where the bubble sits is not a prop here. daisyUI decides it from classes on
/// [`Tooltip`], so the axes are that component's, and the values travel down to
/// the primitive from there, which is what keeps the `data-side` and
/// `data-align` it reports saying the same thing daisyUI did.
#[component]
pub fn TooltipContent(
    /// The id of this element. Declared rather than left to the attribute list,
    /// because the primitive generates one and then points the trigger's
    /// `aria-describedby` at it; an id arriving as an attribute would leave
    /// the trigger described by an element that is no longer there.
    #[props(default)]
    id: ReadSignal<Option<String>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let side = use_context::<Memo<Placement>>()();

    let base = attributes!(div {
        class: "tooltip-content",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        tooltip::TooltipContent {
            id,
            side: side.side.side(),
            align: side.align.align(),
            attributes: merged,
            {children}
        }
    }
}
