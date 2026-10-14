use dioxus::core::AttributeValue;
use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;
use dioxus_primitives::navbar;

/// daisyUI's colour axis for a navbar trigger, which is the button's own.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum NavbarTriggerColor {
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

impl NavbarTriggerColor {
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

    /// The daisyUI class name for this value.
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

/// daisyUI's size axis for a navbar trigger.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum NavbarTriggerSize {
    Xs,
    Sm,
    #[default]
    Default,
    Lg,
    Xl,
}

impl NavbarTriggerSize {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Xs, Self::Sm, Self::Default, Self::Lg, Self::Xl];

    /// The daisyUI class name for this value.
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

/// Whether [`NavbarTrigger`] emits the Bridged utilities that mark it open.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum NavbarTriggerOpenAppearance {
    #[default]
    Default,
    None,
}

impl NavbarTriggerOpenAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::None];

    /// The Tailwind variants for this value.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => {
                "group-data-[state=open]:ring-2 group-data-[state=open]:ring-offset-2 group-data-[state=open]:ring-offset-base-100"
            }
            Self::None => "",
        }
    }
}

/// daisyUI's size axis for the menu inside a navbar popup.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum NavbarMenuSize {
    Xs,
    Sm,
    #[default]
    Default,
    Lg,
    Xl,
}

impl NavbarMenuSize {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Xs, Self::Sm, Self::Default, Self::Lg, Self::Xl];

    /// The daisyUI class name for this value.
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

/// Whether [`NavbarContent`] emits the utilities that draw its floating box.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum NavbarContentAppearance {
    #[default]
    Default,
    None,
}

impl NavbarContentAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::None];

    /// The Tailwind utilities for this value.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "bg-base-100 rounded-box shadow-sm",
            Self::None => "",
        }
    }
}

/// A daisyUI navbar backed by the primitive's roving focus and menu state.
#[component]
pub fn Navbar(
    /// Whether the whole bar is disabled, which leaves every nav and item in it
    /// inert.
    #[props(default)]
    disabled: ReadSignal<bool>,
    /// Whether arrow-key navigation wraps around at the ends of the bar. The
    /// default repeats the primitive's own, since a prop declared here has to
    /// carry one.
    #[props(default = ReadSignal::new(Signal::new(true)))]
    roving_loop: ReadSignal<bool>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    use_context_provider(|| NavbarDisabledContext(disabled));

    let base = attributes!(div { class: "navbar" });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        navbar::Navbar { disabled, roving_loop, attributes: merged, {children} }
    }
}

/// The start-aligned region of a [`Navbar`].
#[component]
pub fn NavbarStart(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div {
        class: "navbar-start"
    });
    let merged = merge_attributes(vec![base, attributes]);
    rsx! { div { ..merged, {children} } }
}

/// The centered region of a [`Navbar`].
#[component]
pub fn NavbarCenter(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div {
        class: "navbar-center"
    });
    let merged = merge_attributes(vec![base, attributes]);
    rsx! { div { ..merged, {children} } }
}

/// The end-aligned region of a [`Navbar`].
#[component]
pub fn NavbarEnd(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div {
        class: "navbar-end"
    });
    let merged = merge_attributes(vec![base, attributes]);
    rsx! { div { ..merged, {children} } }
}

/// One dropdown navigation item: its trigger and popup.
#[component]
pub fn NavbarNav(
    /// Where this dropdown falls in the bar's keyboard navigation order, which
    /// it shares with the bar's direct items.
    index: ReadSignal<usize>,
    /// Whether this dropdown is disabled, which leaves its trigger inert and the
    /// popup unopenable.
    #[props(default)]
    disabled: ReadSignal<bool>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div {
        class: "group dropdown dropdown-hover dropdown-bottom dropdown-start",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        navbar::NavbarNav { index, disabled, attributes: merged, {children} }
    }
}

/// The button that opens a navbar popup.
#[component]
pub fn NavbarTrigger(
    /// Whether to emit the Bridged utilities that ring the trigger while its
    /// popup is open.
    #[props(default)]
    open_appearance: NavbarTriggerOpenAppearance,
    /// daisyUI's colour axis.
    #[props(default)]
    color: NavbarTriggerColor,
    /// daisyUI's size axis.
    #[props(default)]
    size: NavbarTriggerSize,
    #[props(extends = GlobalAttributes)]
    #[props(extends = button)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let open_appearance = open_appearance.class();
    let color = color.class();
    let size = size.class();
    let base = attributes!(button {
        class: "btn {color} {size} {open_appearance}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        navbar::NavbarTrigger { attributes: merged, {children} }
    }
}

// `GlobalAttributes` and `button` both define `autofocus`, so naming it on `NavbarTrigger` would
// be ambiguous (E0034, DioxusLabs/dioxus#5271). Inherent methods win method resolution and forward
// to the element's own attribute, as `button { .. }` resolves it. Unused, they would warn in an
// app that installs this Component. Remove once upstream resolves the conflict (ADR-0034).
#[allow(dead_code)]
impl<F> NavbarTriggerPropsBuilder<F>
where
    Self: dioxus_elements::extensions::ButtonExtension,
{
    /// The `button` element's `autofocus` attribute.
    pub fn autofocus(self, value: impl dioxus::dioxus_core::IntoAttributeValue) -> Self {
        dioxus_elements::extensions::ButtonExtension::autofocus(self, value)
    }
}

/// The dropdown popup, with a presentational list for daisyUI's `menu` class.
#[component]
pub fn NavbarContent(
    /// Whether to emit the utilities that draw the popup's floating box.
    #[props(default)]
    appearance: NavbarContentAppearance,
    /// daisyUI's size axis for the menu, which sizes the items.
    #[props(default)]
    size: NavbarMenuSize,
    /// The id of this element. Declared rather than left to the attribute list,
    /// because the primitive generates one and then looks the element up by it;
    /// an id that arrived as an attribute would be written over the one it is
    /// looking for.
    #[props(default)]
    id: ReadSignal<Option<String>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    use_context_provider(|| NavbarContentContext);

    let appearance = appearance.class();
    let size = size.class();
    let base = attributes!(div {
        class: "dropdown-content {appearance}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        navbar::NavbarContent { id, attributes: merged,
            ul { role: "none", class: "menu {size}", {children} }
        }
    }
}

/// A navbar link, wrapped in the presentational list item daisyUI styles.
#[component]
pub fn NavbarItem(
    /// What this item is worth, which is what `on_select` is called with.
    value: String,
    /// Where this item falls in the keyboard navigation order: its popup's own
    /// when it is inside one, otherwise the bar's, shared with the dropdowns.
    index: ReadSignal<usize>,
    /// Whether this item is disabled, which marks its list item with daisyUI's
    /// `menu-disabled` and keeps the link from navigating.
    #[props(default)]
    disabled: ReadSignal<bool>,
    /// Called with this item's value when it is selected.
    #[props(default)]
    on_select: Callback<String>,
    /// A class added to the anchor while the route `to` names is the active one.
    #[props(default)]
    active_class: Option<String>,
    /// Whether the link opens `to` in a new tab. This does not change whether
    /// the item counts as active.
    #[props(default)]
    new_tab: bool,
    /// Called when the anchor is clicked, after the link's own behaviour unless
    /// `onclick_only` replaces it. Declared rather than left to the attribute
    /// list because `extends` reaches attributes only, never event handlers.
    #[props(default)]
    onclick: Option<EventHandler<MouseEvent>>,
    /// Called when the anchor element is mounted.
    #[props(default)]
    onmounted: Option<EventHandler<MountedEvent>>,
    /// Whether `onclick` runs instead of the link's own navigation rather than
    /// after it. A disabled item is put on this path whatever the caller passed,
    /// which is what keeps it from navigating.
    #[props(default)]
    onclick_only: bool,
    /// The anchor's `rel` attribute, which defaults to `noopener noreferrer` for
    /// an external target.
    #[props(default)]
    rel: Option<String>,
    /// Where the link navigates to, roughly the anchor's `href`.
    #[props(into)]
    to: NavigationTarget,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let in_content = try_use_context::<NavbarContentContext>().is_some();
    let root_disabled = try_use_context::<NavbarDisabledContext>();
    let is_disabled = disabled()
        || root_disabled
            .map(|context| (context.0)())
            .unwrap_or_default();
    let state = if disabled() { "menu-disabled" } else { "" };
    let mut attributes = attributes;
    let class = take_class(&mut attributes);
    let item = rsx! {
        navbar::NavbarItem {
            value,
            index,
            disabled,
            on_select,
            class: Some(class),
            active_class,
            new_tab,
            onclick,
            onmounted,
            onclick_only: onclick_only || is_disabled,
            rel,
            to,
            attributes,
            {children}
        }
    };

    if in_content {
        rsx! {
            li { role: "none", class: "{state}", {item} }
        }
    } else {
        item
    }
}

#[derive(Clone, Copy)]
struct NavbarContentContext;

#[derive(Clone, Copy)]
struct NavbarDisabledContext(ReadSignal<bool>);

fn take_class(attributes: &mut Vec<Attribute>) -> String {
    let class = attributes.iter().position(|attribute| {
        attribute.name == "class" && matches!(attribute.value, AttributeValue::Text(_))
    });

    match class.map(|index| attributes.remove(index).value) {
        Some(AttributeValue::Text(class)) => class,
        _ => String::new(),
    }
}
