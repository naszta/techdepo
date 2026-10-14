use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::dropdown_menu;
use dioxus_primitives::merge_attributes;

/// daisyUI's side axis for a dropdown, which is the side of the trigger
/// the menu opens on.
///
/// Every value emits a class, including the default one: the inverse of the
/// usual convention, and for a reason of this component's own (ADR-0008).
/// daisyUI's unclassed dropdown places the menu wherever it would have fallen
/// in flow, which is under the trigger only for as long as the trigger is the
/// one thing written before it; `dropdown-bottom` pins it under the element
/// instead. The difference is invisible in the markup this component renders
/// today and stops being invisible the moment a caller writes anything else
/// inside [`DropdownMenu`], so the explicit class is emitted rather than
/// relied upon.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum DropdownMenuSide {
    /// Above the trigger.
    Top,
    /// Under the trigger, which is where daisyUI puts an unplaced dropdown.
    #[default]
    Bottom,
    /// To the left of the trigger, in either writing direction; daisyUI's
    /// horizontal placements are physical rather than logical.
    Left,
    /// To the right of the trigger, mirroring [`DropdownMenuSide::Left`].
    Right,
}

impl DropdownMenuSide {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Top, Self::Bottom, Self::Left, Self::Right];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Top => "dropdown-top",
            Self::Bottom => "dropdown-bottom",
            Self::Left => "dropdown-left",
            Self::Right => "dropdown-right",
        }
    }
}

/// daisyUI's align axis for a dropdown, which is where the menu sits along
/// the side [`DropdownMenuSide`] opened it on.
///
/// The two compose the way daisyUI's own classes do: an alignment on a
/// vertical placement moves the menu across the trigger, and on a horizontal
/// one it moves the menu up and down it.
///
/// Every value emits a class here too, for the reason
/// [`DropdownMenuSide`] records.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum DropdownMenuAlign {
    /// The inline start edge, which follows the writing direction: the left in
    /// a left-to-right document, the right in a right-to-left one. This is
    /// where daisyUI puts an unaligned dropdown.
    #[default]
    Start,
    /// Centred on the trigger.
    Center,
    /// The inline end edge, mirroring [`DropdownMenuAlign::Start`].
    End,
}

impl DropdownMenuAlign {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Start, Self::Center, Self::End];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Start => "dropdown-start",
            Self::Center => "dropdown-center",
            Self::End => "dropdown-end",
        }
    }
}

/// daisyUI's size axis for the menu inside a dropdown, which sizes the items
/// rather than the box they are in.
///
/// [`DropdownMenuSize::Default`] emits no class, which renders at the same
/// size as daisyUI's explicit `menu-md`.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum DropdownMenuSize {
    Xs,
    Sm,
    #[default]
    Default,
    Lg,
    Xl,
}

impl DropdownMenuSize {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Xs, Self::Sm, Self::Default, Self::Lg, Self::Xl];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Xs => "menu-xs",
            Self::Sm => "menu-sm",
            Self::Default => "",
            Self::Lg => "menu-lg",
            Self::Xl => "menu-xl",
        }
    }
}

/// Whether [`DropdownMenuContent`] emits the utilities that draw the box the
/// menu sits in.
///
/// daisyUI's `dropdown-content` only positions the element: the fill, the
/// corners and the shadow in its own examples are Tailwind utilities on the
/// same element. They are emitted here instead, which inverts the usual
/// convention: [`DropdownMenuContentAppearance::Default`] emits classes and
/// [`DropdownMenuContentAppearance::None`] emits nothing. A utility this
/// component emits only ties with a caller's, and a tie is settled by
/// generated-stylesheet order rather than by the class attribute, so switching
/// ours off is the way to win it (ADR-0004).
///
/// Three of daisyUI's example utilities are deliberately not here. Its `p-2`
/// is what `.menu` already pads by, and here the two elements would pad in
/// turn rather than agree. Its `z-1` is beneath the `z-index` daisyUI's own
/// `dropdown-content` rule already sets. And its `w-52` is a width for the
/// menu it was written around rather than for every menu: a menu with no
/// width sizes to its items, and a caller who wants one adds it through
/// `class`, which reaches the items because the list is stretched to the box.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum DropdownMenuContentAppearance {
    #[default]
    Default,
    None,
}

impl DropdownMenuContentAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::None];

    /// The Tailwind utilities for this value, as complete string literals so
    /// Tailwind's scanner can see them.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "bg-base-100 rounded-box shadow-sm",
            Self::None => "",
        }
    }
}

/// The outer element of a dropdown menu, carrying daisyUI's `dropdown`
/// classes.
///
/// This is where the open state is **lifted** (ADR-0006), and here the lift is
/// mandatory rather than cosmetic: daisyUI hides `.dropdown-content` outright
/// unless `dropdown-open` is on this element, and matches no attribute the
/// primitive sets. So the class has to be emitted from Rust, and this
/// component has to know the state to emit it. It seeds a signal from
/// `default_open`, always hands the primitive a controlled value, and
/// intercepts the change callback, which leaves a controlled caller and an
/// uncontrolled one both working and this component the only writer.
///
/// Classes passed by the caller concatenate with this element's own; every
/// other attribute the caller passes overrides them.
#[component]
pub fn DropdownMenu(
    /// daisyUI's side axis.
    #[props(default)]
    side: DropdownMenuSide,
    /// daisyUI's align axis.
    #[props(default)]
    align: DropdownMenuAlign,
    /// The controlled open state of the menu.
    #[props(default)]
    open: ReadSignal<Option<bool>>,
    /// The state the menu starts in when it is not controlled. The primitive
    /// closes a menu that nothing in it is focused, so a menu that has to open
    /// with the page is one a caller controls; see the component's
    /// documentation.
    #[props(default)]
    default_open: bool,
    /// Called when the open state changes.
    #[props(default)]
    on_open_change: Callback<bool>,
    /// Whether the menu is disabled, which leaves the trigger inert and the
    /// menu unopenable.
    #[props(default)]
    disabled: ReadSignal<bool>,
    /// Whether arrow-key navigation wraps around at the ends of the menu. The
    /// default repeats the primitive's own, since a prop declared here has to
    /// carry one.
    #[props(default = ReadSignal::new(Signal::new(true)))]
    roving_loop: ReadSignal<bool>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let mut uncontrolled = use_signal(|| default_open);

    // Both are read here rather than inside the markup, and eagerly rather
    // than only when the other is absent, so that this component subscribes to
    // whichever of them is driving and re-renders, which is what puts the
    // modifier class below on the element and takes it off again.
    let is_open = open().unwrap_or(uncontrolled());

    let side = side.class();
    let align = align.class();
    // Tier 2, and the whole reason the state is lifted: daisyUI's own modifier
    // class, emitted from Rust as a complete literal so that Tailwind's
    // scanner sees it too.
    let state = if is_open { "dropdown-open" } else { "" };

    let base = attributes!(div {
        class: "dropdown {side} {align} {state}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        dropdown_menu::DropdownMenu {
            open: Some(is_open),
            on_open_change: move |open| {
                uncontrolled.set(open);
                on_open_change.call(open);
            },
            disabled,
            roving_loop,
            attributes: merged,
            {children}
        }
    }
}

/// The control a dropdown menu opens from, carrying daisyUI's `btn` class.
///
/// The button's own axes are not repeated here: a colour or a size reaches
/// this element through `class`, which concatenates, and a trigger that is not
/// a button at all is what `as` is for. That prop is the primitive's, passed
/// through: it hands the whole merged attribute list, this component's class
/// included, to a callback that renders the element in this one's place. The
/// part's children do not travel with it, because the primitive's `as` path
/// does not carry them; an element rendered that way brings its own.
///
/// This part takes no `id` prop, unlike the parts of every other component
/// here, because the primitive's trigger has none to pass one to: it generates
/// an id, puts it on the element, and points the menu's `aria-labelledby` at
/// it. An `id` from the caller lands on the element over that one and leaves
/// the menu named after an element that is no longer there, so a trigger a
/// caller needs to address is one they reach through [`DropdownMenu`]'s own id
/// instead.
///
/// A trigger rendered through `as` should be focusable by being the kind of
/// element that already is, rather than by carrying a `tabindex`: daisyUI
/// takes the pointer events off a `[tabindex]` first child of an open dropdown
/// (which is how its own CSS-only dropdown closes on a second click) and a
/// trigger that stops answering clicks the moment it has focus never opens
/// twice.
#[component]
pub fn DropdownMenuTrigger(
    /// Renders the trigger as an element of the caller's rather than as a
    /// button, with every attribute this component and the primitive would
    /// have put on it.
    #[props(default)]
    r#as: Option<Callback<Vec<Attribute>, Element>>,
    #[props(extends = GlobalAttributes)]
    #[props(extends = button)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(button { class: "btn" });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        dropdown_menu::DropdownMenuTrigger { r#as, attributes: merged, {children} }
    }
}

// `GlobalAttributes` and `button` both define `autofocus`, so naming it on `DropdownMenuTrigger`
// would be ambiguous (E0034, DioxusLabs/dioxus#5271). Inherent methods win method resolution and
// forward to the element's own attribute, as `button { .. }` resolves it. Unused, they would warn
// in an app that installs this Component. Remove once upstream resolves the conflict (ADR-0034).
#[allow(dead_code)]
impl<F> DropdownMenuTriggerPropsBuilder<F>
where
    Self: dioxus_elements::extensions::ButtonExtension,
{
    /// The `button` element's `autofocus` attribute.
    pub fn autofocus(self, value: impl dioxus::dioxus_core::IntoAttributeValue) -> Self {
        dioxus_elements::extensions::ButtonExtension::autofocus(self, value)
    }
}

/// The box a dropdown menu's items sit in, carrying daisyUI's
/// `dropdown-content` class and holding the `menu` list.
///
/// This is the split ADR-0005 exists for. daisyUI puts both classes on one
/// `ul`, which is impossible here: the primitive's content element is a
/// hardcoded `div` with no `as` prop of its own, and every visual rule `.menu`
/// has for an item is written against a literal `li`. So the positioning and
/// the box go on the primitive's element, and `menu` goes on a list rendered
/// inside it: each class on the element whose layout it was written to drive.
///
/// The list is marked presentational, as [`DropdownMenuItem`]'s own wrapper is:
/// the primitive gives this element the `listbox` role and the items the
/// `option` role, and a plain list between them would break the ownership a
/// screen reader announces option counts and positions from.
///
/// Classes passed by the caller concatenate with the box's own, which is the
/// element worth reaching: a width or a fill belongs on the box rather than
/// on the list inside it.
#[component]
pub fn DropdownMenuContent(
    /// Whether to emit the utilities that draw the box.
    #[props(default)]
    appearance: DropdownMenuContentAppearance,
    /// daisyUI's size axis for the menu, which sizes the items.
    #[props(default)]
    size: DropdownMenuSize,
    /// The id of this element. Declared rather than left to the attribute
    /// list, because the primitive generates one and then looks the element up
    /// by it; an id that arrived as an attribute would be written over the
    /// one it is looking for.
    #[props(default)]
    id: ReadSignal<Option<String>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let appearance = appearance.class();
    let size = size.class();

    let base = attributes!(div {
        class: "dropdown-content {appearance}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        dropdown_menu::DropdownMenuContent { id, attributes: merged,
            // The list is stretched to the box rather than left to size
            // itself, which is the one place the split has to be papered over:
            // `.menu` is `width: fit-content`, and on daisyUI's single element
            // that is the box's width as well, where here it would leave a
            // caller's width on the box with a shrink-wrapped menu inside it.
            // With no width on the box the two are the same thing anyway,
            // since the box shrink-wraps the list in turn.
            ul { role: "none", class: "menu w-full {size}", {children} }
        }
    }
}

/// One item of a dropdown menu, wrapped in the list item daisyUI's `menu`
/// styles it through.
///
/// Nothing is emitted on the item itself: `.menu` reaches it as the child of
/// an `li`, and both the hover highlight and the keyboard one come with that,
/// the second because daisyUI's rule matches `:focus-visible` as well as its
/// own class, and the primitive moves real DOM focus onto this element.
///
/// The wrapper is marked presentational for the reason
/// [`DropdownMenuContent`]'s list is, and costs nothing behaviourally: items
/// register with the primitive's focus collection by the `index` they are
/// given rather than by where they sit in the DOM.
///
/// Everything the caller passes travels to the item rather than to the
/// wrapper, which is the element daisyUI styles and the primitive gives the
/// `option` role to. There is nothing to merge it with and nothing merges it:
/// this part emits no class on the item at all, so a caller's arrives as the
/// only one.
#[component]
pub fn DropdownMenuItem<T: Clone + PartialEq + 'static>(
    /// What this item is worth, which is what `on_select` is called with.
    value: ReadSignal<T>,
    /// Where this item falls in the keyboard navigation order.
    index: ReadSignal<usize>,
    /// Whether this item is disabled.
    #[props(default)]
    disabled: ReadSignal<bool>,
    /// Called with this item's value when it is selected.
    #[props(default)]
    on_select: Callback<T>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    // Tier 2 on the disabled state, on the wrapper rather than on the item:
    // daisyUI mutes a disabled item through `.menu-disabled` on the list item
    // or a `disabled` attribute on the item, and the primitive sets neither:
    // it reports the state as `data-disabled`, which daisyUI matches nowhere.
    let state = if disabled() { "menu-disabled" } else { "" };

    rsx! {
        li { role: "none", class: "{state}",
            dropdown_menu::DropdownMenuItem::<T> {
                value,
                index,
                disabled,
                on_select,
                attributes,
                {children}
            }
        }
    }
}
