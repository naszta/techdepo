use dioxus::core::AttributeValue;
use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;
use dioxus_primitives::tabs;

/// daisyUI's appearance axis for a set of tabs, which decides how a tab is
/// drawn and how it meets its panel.
///
/// [`TabsAppearance::Default`] emits no class: a plain row of tabs, with the
/// active one told apart by its text colour alone. The other three are the
/// looks that need the panel to be a sibling of the tab, which is what
/// ADR-0003 rearranges the tree for.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TabsAppearance {
    #[default]
    Default,
    /// A rule under the active tab.
    Border,
    /// The active tab lifted out of the row and joined to its panel.
    Lift,
    /// The row drawn as a filled box, with the active tab as a raised pill.
    Box,
}

impl TabsAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::Border, Self::Lift, Self::Box];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Border => "tabs-border",
            Self::Lift => "tabs-lift",
            Self::Box => "tabs-box",
        }
    }
}

/// daisyUI's size axis for a set of tabs.
///
/// [`TabsSize::Default`] emits no class, which renders at the same size as
/// daisyUI's explicit `tabs-md`.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TabsSize {
    Xs,
    Sm,
    #[default]
    Default,
    Lg,
    Xl,
}

impl TabsSize {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Xs, Self::Sm, Self::Default, Self::Lg, Self::Xl];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Xs => "tabs-xs",
            Self::Sm => "tabs-sm",
            Self::Default => "",
            Self::Lg => "tabs-lg",
            Self::Xl => "tabs-xl",
        }
    }
}

/// Whether [`TabContent`] emits the utilities that give a panel a surface to
/// sit on.
///
/// daisyUI's `tab-content` draws a border but leaves it transparent and paints
/// no background, so the classes it uses in its own examples are emitted here
/// instead; without them the lifted and boxed appearances have nothing to
/// join the active tab to. That inverts the usual convention:
/// [`TabContentAppearance::Default`] emits classes and
/// [`TabContentAppearance::None`] emits nothing, because a utility this
/// component emits only ties with a caller's, and a tie is settled by
/// generated-stylesheet order rather than by the class attribute. Switching
/// ours off is the way to win it (ADR-0004).
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TabContentAppearance {
    #[default]
    Default,
    None,
}

impl TabContentAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::None];

    /// The Tailwind utilities for this value, as complete string literals so
    /// Tailwind's scanner can see them.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "border-base-300 bg-base-100 p-6",
            Self::None => "",
        }
    }
}

/// A set of tabs styled with daisyUI's `tabs` classes.
///
/// This element carries both the `tabs` class and the `tablist` role, and
/// [`TabTrigger`] and [`TabContent`] interleave as its direct children:
/// trigger, its panel, the next trigger, its panel, which is daisyUI's own
/// tabs markup. The primitive's `TabList` part is **not exposed**, because its
/// element would stand between every selector daisyUI reaches a panel with and
/// the panel it reaches (ADR-0003). What that costs in ARIA is recorded in the
/// component's documentation and is the sharpest trade-off in this registry.
///
/// The active tab needs no class of its own: daisyUI matches the
/// `aria-selected` attribute the primitive already sets.
///
/// Classes passed by the caller concatenate with this element's own; every
/// other attribute the caller passes overrides them.
#[component]
pub fn Tabs(
    /// daisyUI's appearance axis.
    #[props(default)]
    appearance: TabsAppearance,
    /// daisyUI's size axis.
    #[props(default)]
    size: TabsSize,
    /// The controlled value of the active tab.
    #[props(default)]
    value: ReadSignal<Option<String>>,
    /// The tab that is active when the tabs are not controlled.
    #[props(default)]
    default_value: String,
    /// Called when the active tab changes.
    #[props(default)]
    on_value_change: Callback<String>,
    /// Whether every tab is disabled.
    #[props(default)]
    disabled: ReadSignal<bool>,
    /// Whether arrow-key navigation wraps around at the ends of the row. The
    /// default repeats the primitive's own, since a prop declared here has to
    /// carry one.
    #[props(default = ReadSignal::new(Signal::new(true)))]
    roving_loop: ReadSignal<bool>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let appearance = appearance.class();
    let size = size.class();

    let base = attributes!(div {
        class: "tabs {appearance} {size}",
        // The role the primitive's `TabList` would have carried, moved out to
        // the element that part was dropped from.
        role: "tablist",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        tabs::Tabs {
            value,
            default_value,
            on_value_change,
            disabled,
            // Pinned rather than passed through. The primitive's orientation
            // decides which arrow keys navigate, and daisyUI has no vertical
            // tabs at all, so a vertical set would announce and navigate one
            // way while rendering as a horizontal row. Documented as
            // unsupported instead.
            horizontal: true,
            roving_loop,
            attributes: merged,
            {children}
        }
    }
}

/// One tab, carrying daisyUI's `tab` class.
///
/// It must stay a direct child of [`Tabs`], and its own [`TabContent`] must be
/// the element straight after it: daisyUI styles a tab through
/// `.tab:is(.tabs > .tab)` and reveals its panel through an adjacent-sibling
/// rule from the active one, so a wrapper around either would leave the tab
/// unstyled and the panel invisible.
///
/// The active tab is **Tier 1**. daisyUI's own rule is
/// `.tab-active, [aria-selected=true], [aria-current=true], [aria-current=page]`
/// and the primitive sets `aria-selected`, so nothing here emits a class for
/// it. Nor for the disabled state: the primitive sets the `disabled` attribute
/// on the `button` it renders, which daisyUI matches directly.
#[component]
pub fn TabTrigger(
    /// The tab this trigger activates, matching the `value` of its
    /// [`TabContent`].
    value: String,
    /// Where this tab falls in the keyboard navigation order, and which panel
    /// it points `aria-controls` at.
    index: ReadSignal<usize>,
    /// Whether this tab is disabled.
    #[props(default)]
    disabled: ReadSignal<bool>,
    /// The id of this element. Declared rather than left to the attribute
    /// list, because the primitive takes one as a prop of its own and puts it
    /// on the element; an id arriving as an attribute would meet the one that
    /// is already there.
    #[props(default)]
    id: Option<String>,
    #[props(extends = GlobalAttributes)]
    #[props(extends = button)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(button { class: "tab" });
    let mut merged = merge_attributes(vec![base, attributes]);

    let class = take_class(&mut merged);

    rsx! {
        tabs::TabTrigger {
            value,
            index,
            disabled,
            id,
            class: Some(class),
            attributes: merged,
            {children}
        }
    }
}

// `GlobalAttributes` and `button` both define `autofocus`, so naming it on `TabTrigger` would be
// ambiguous (E0034, DioxusLabs/dioxus#5271). Inherent methods win method resolution and forward to
// the element's own attribute, as `button { .. }` resolves it. Unused, they would warn in an app
// that installs this Component. Remove once upstream resolves the conflict (ADR-0034).
#[allow(dead_code)]
impl<F> TabTriggerPropsBuilder<F>
where
    Self: dioxus_elements::extensions::ButtonExtension,
{
    /// The `button` element's `autofocus` attribute.
    pub fn autofocus(self, value: impl dioxus::dioxus_core::IntoAttributeValue) -> Self {
        dioxus_elements::extensions::ButtonExtension::autofocus(self, value)
    }
}

/// The panel one tab reveals, carrying daisyUI's `tab-content` class.
///
/// Every panel is hidden by daisyUI until the tab immediately before it is the
/// active one, which is the rule the whole tree is arranged around, so a
/// panel has to be written straight after its own [`TabTrigger`], as a direct
/// child of [`Tabs`].
#[component]
pub fn TabContent(
    /// Whether to emit the utilities that give the panel a surface.
    #[props(default)]
    appearance: TabContentAppearance,
    /// The tab this panel belongs to, matching the `value` of its
    /// [`TabTrigger`].
    value: String,
    /// Which tab this panel belongs to, matching the `index` of its
    /// [`TabTrigger`].
    index: ReadSignal<usize>,
    /// The id of this element. Declared rather than left to the attribute
    /// list, because the primitive generates one and then points the trigger's
    /// `aria-controls` at it; an id arriving as an attribute would replace the
    /// one the trigger is still naming.
    #[props(default)]
    id: ReadSignal<Option<String>>,
    #[props(extends = GlobalAttributes)]
    #[props(extends = div)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let appearance = appearance.class();

    let base = attributes!(div {
        class: "tab-content {appearance}",
    });
    let mut merged = merge_attributes(vec![base, attributes]);

    let class = take_class(&mut merged);

    rsx! {
        tabs::TabContent {
            value,
            index,
            id,
            class: Some(class),
            attributes: merged,
            {children}
        }
    }
}

/// Takes the class out of a merged attribute list, so that it can be passed to
/// a primitive that takes one as a prop of its own.
///
/// `merge_attributes` has already concatenated the caller's class with this
/// component's by the time this runs, so there is exactly one to take, as
/// long as it is text, which is the only kind of class `rsx!` produces and the
/// only kind that could have been concatenated in the first place. Anything
/// else is left where it is, to travel on as an attribute.
fn take_class(attributes: &mut Vec<Attribute>) -> String {
    let class = attributes.iter().position(|attribute| {
        attribute.name == "class" && matches!(attribute.value, AttributeValue::Text(_))
    });

    match class.map(|index| attributes.remove(index).value) {
        Some(AttributeValue::Text(class)) => class,
        _ => String::new(),
    }
}
