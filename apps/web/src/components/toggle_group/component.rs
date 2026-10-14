use std::collections::HashSet;

use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;
use dioxus_primitives::toggle_group;

/// daisyUI's colour axis for a toggle item, which is the button's own.
///
/// The class strings are the button component's, duplicated rather than
/// depended on: the registry uses no cross-component dependencies, and the
/// Tailwind contract already requires every one of these to be a literal in the
/// file that emits it.
///
/// [`ToggleItemColor::Default`] emits no class at all, which is daisyUI's
/// uncoloured button rather than a synonym for [`ToggleItemColor::Neutral`].
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum ToggleItemColor {
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

impl ToggleItemColor {
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
            Self::Neutral => "btn-neutral",
            Self::Primary => "btn-primary",
            Self::Secondary => "btn-secondary",
            Self::Accent => "btn-accent",
            Self::Info => "btn-info",
            Self::Success => "btn-success",
            Self::Warning => "btn-warning",
            Self::Error => "btn-error",
        }
    }
}

/// daisyUI's size axis for a toggle item, which is the button's own and is
/// duplicated for the reason [`ToggleItemColor`] records.
///
/// [`ToggleItemSize::Default`] emits no class, which renders at the same size
/// as daisyUI's explicit `btn-md`.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum ToggleItemSize {
    Xs,
    Sm,
    #[default]
    Default,
    Lg,
    Xl,
}

impl ToggleItemSize {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Xs, Self::Sm, Self::Default, Self::Lg, Self::Xl];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Xs => "btn-xs",
            Self::Sm => "btn-sm",
            Self::Default => "",
            Self::Lg => "btn-lg",
            Self::Xl => "btn-xl",
        }
    }
}

/// Which items are pressed, as [`ToggleGroup`] holds it for the items to read.
///
/// This is the second half of the lift ADR-0006 describes. daisyUI marks a
/// pressed button with `btn-active` and matches no ARIA attribute at all: the
/// `aria-pressed` the primitive sets on every item appears nowhere in its
/// stylesheet, so a pressed item would otherwise render identically to every
/// other one. The primitive's own context is private, so an item cannot read
/// the set from it; the group provides this alongside and each item looks
/// itself up by index.
#[derive(Copy, Clone)]
struct Pressed {
    indices: Memo<HashSet<usize>>,
}

/// A group of toggle buttons, carrying daisyUI's `join` classes.
///
/// The pressed set is **lifted** (ADR-0006): the group seeds a signal from
/// `default_pressed`, always hands the primitive a controlled set, and
/// intercepts the change callback, which leaves a controlled caller and an
/// uncontrolled one both working, and this component the only writer.
///
/// The orientation class is emitted for both values rather than only for the
/// one that departs from daisyUI's default, following ADR-0008. Here it does
/// more than name what is already true: the primitive's group is **vertical by
/// default** (up and down are the arrow keys it listens for) while an
/// unclassed `.join` is a row. Emitting the class keeps what the row looks like
/// and what the keyboard does from disagreeing.
///
/// [`ToggleItem`]s must stay direct children of this element. daisyUI rounds a
/// joined row's ends through `:scope > :first-child` and `:scope > :last-child`
/// and pulls the shared borders together with a negative margin on `.join-item`,
/// so an item inside a wrapper is an item daisyUI cannot find.
///
/// Classes passed by the caller concatenate with the group's own; every other
/// attribute the caller passes overrides them.
#[component]
pub fn ToggleGroup(
    /// The controlled set of pressed items, by index. `Some` makes the group
    /// controlled.
    #[props(default)]
    pressed: ReadSignal<Option<HashSet<usize>>>,
    /// Which items are pressed when the group is not controlled.
    #[props(default)]
    default_pressed: HashSet<usize>,
    /// Called when the pressed set changes.
    #[props(default)]
    on_pressed_change: Callback<HashSet<usize>>,
    /// Whether more than one item may be pressed at once. With this off the
    /// group behaves like a set of radios that all look like buttons.
    #[props(default)]
    allow_multiple_pressed: ReadSignal<bool>,
    /// Whether every item in the group is disabled.
    #[props(default)]
    disabled: ReadSignal<bool>,
    /// Whether the group runs across rather than down, which is both how it is
    /// laid out and which arrow keys move through it. The default repeats the
    /// primitive's own, since a prop declared here has to carry one, and the
    /// primitive's own is a column.
    #[props(default)]
    horizontal: ReadSignal<bool>,
    /// Whether arrow-key navigation wraps around at the ends of the group. The
    /// default repeats the primitive's own.
    #[props(default = ReadSignal::new(Signal::new(true)))]
    roving_loop: ReadSignal<bool>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let mut uncontrolled = use_signal(|| default_pressed.clone());

    // Read eagerly rather than only when the other is absent, so that this
    // component subscribes to whichever of them is driving and re-renders,
    // which is what puts the modifier class on an item and takes it off again.
    let current = use_memo(move || match pressed() {
        Some(pressed) => pressed,
        None => uncontrolled(),
    });
    use_context_provider(|| Pressed { indices: current });
    let controlled = use_memo(move || Some(current()));

    // Tier 2 on the layout rather than on state: daisyUI's own orientation
    // class, emitted from Rust as a complete literal so that Tailwind's scanner
    // sees it too.
    let orientation = if horizontal() {
        "join-horizontal"
    } else {
        "join-vertical"
    };

    let base = attributes!(div {
        class: "join {orientation}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        toggle_group::ToggleGroup {
            pressed: controlled,
            on_pressed_change: move |pressed: HashSet<usize>| {
                uncontrolled.set(pressed.clone());
                on_pressed_change.call(pressed);
            },
            allow_multiple_pressed,
            disabled,
            horizontal,
            roving_loop,
            attributes: merged,
            {children}
        }
    }
}

/// One button of a toggle group, carrying daisyUI's `join-item` and `btn`
/// classes.
///
/// The pressed state is bridged here at **Tier 2**, from the set [`ToggleGroup`]
/// lifted: daisyUI's pressed button is `btn-active`, and `aria-pressed` (which
/// the primitive does set, on this very element) appears nowhere in daisyUI's
/// stylesheet. So the class is emitted from Rust and the state is read from the
/// group's context by index.
///
/// The disabled state needs no such thing: the primitive sets the `disabled`
/// attribute on the `button` and daisyUI's rule is `.btn:disabled`.
///
/// Everything the caller passes lands on the button itself, which is both the
/// element daisyUI styles and the one the primitive gives the behaviour to.
#[component]
pub fn ToggleItem(
    /// daisyUI's colour axis.
    #[props(default)]
    color: ToggleItemColor,
    /// daisyUI's size axis.
    #[props(default)]
    size: ToggleItemSize,
    /// Where this item falls in the group's order, which is both the keyboard
    /// navigation order and how the group identifies it in the pressed set.
    index: ReadSignal<usize>,
    /// Whether this item is disabled.
    #[props(default)]
    disabled: ReadSignal<bool>,
    #[props(extends = GlobalAttributes)]
    #[props(extends = button)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let pressed = use_context::<Pressed>();

    let color = color.class();
    let size = size.class();
    // Tier 2, and the whole reason the set is lifted.
    let state = if pressed.indices.read().contains(&index()) {
        "btn-active"
    } else {
        ""
    };

    let base = attributes!(button {
        class: "join-item btn {color} {size} {state}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        toggle_group::ToggleItem { index, disabled, attributes: merged, {children} }
    }
}

// `GlobalAttributes` and `button` both define `autofocus`, so naming it on `ToggleItem` would be
// ambiguous (E0034, DioxusLabs/dioxus#5271). Inherent methods win method resolution and forward to
// the element's own attribute, as `button { .. }` resolves it. Unused, they would warn in an app
// that installs this Component. Remove once upstream resolves the conflict (ADR-0034).
#[allow(dead_code)]
impl<F> ToggleItemPropsBuilder<F>
where
    Self: dioxus_elements::extensions::ButtonExtension,
{
    /// The `button` element's `autofocus` attribute.
    pub fn autofocus(self, value: impl dioxus::dioxus_core::IntoAttributeValue) -> Self {
        dioxus_elements::extensions::ButtonExtension::autofocus(self, value)
    }
}
