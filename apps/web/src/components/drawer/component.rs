use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use dioxus::core::AttributeValue;
use dioxus::document;
use dioxus::prelude::*;
use dioxus_primitives::dialog;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;

static NEXT_DRAWER_ID: AtomicUsize = AtomicUsize::new(0);

/// daisyUI's placement axis for a drawer.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum DrawerPlacement {
    /// The inline start edge: left in a left-to-right document and right in a
    /// right-to-left one.
    #[default]
    Start,
    /// The inline end edge, mirroring [`DrawerPlacement::Start`].
    End,
}

impl DrawerPlacement {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Start, Self::End];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Start => "",
            Self::End => "drawer-end",
        }
    }
}

#[derive(Clone, Copy)]
struct DrawerContext {
    open: Memo<bool>,
    set_open: Callback<bool>,
    side_id: Signal<String>,
    phase: Signal<DrawerPhase>,
    active_trigger: Signal<Option<Rc<MountedData>>>,
}

#[derive(Clone, Copy, PartialEq)]
enum DrawerPhase {
    Closed,
    Opening,
    Open,
    Closing,
}

impl DrawerContext {
    fn is_open(self) -> bool {
        (self.open)()
    }

    fn set_open(self, open: bool) {
        self.set_open.call(open);
    }

    fn is_primitive_open(self) -> bool {
        (self.phase)() != DrawerPhase::Closed
    }
}

/// The always-mounted drawer grid, carrying daisyUI's `drawer` classes.
///
/// This component owns open state and projects it into an inert hidden
/// `.drawer-toggle`. daisyUI reads that checkbox through a sibling selector,
/// while [`DrawerTrigger`] and the dialog Primitive inside [`DrawerSide`] read
/// the same state from context.
#[component]
pub fn Drawer(
    /// Which inline edge the side enters from.
    #[props(default)]
    placement: DrawerPlacement,
    /// The controlled open state. `Some` makes the drawer controlled.
    #[props(default)]
    open: ReadSignal<Option<bool>>,
    /// Whether the drawer starts open when it is not controlled.
    #[props(default)]
    default_open: bool,
    /// Called when the drawer opens or closes.
    #[props(default)]
    on_open_change: Callback<bool>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let mut phase = use_signal(|| {
        if open().unwrap_or(default_open) {
            DrawerPhase::Open
        } else {
            DrawerPhase::Closed
        }
    });
    let is_open = use_memo(move || {
        let controlled = open();
        let uncontrolled = matches!(phase(), DrawerPhase::Opening | DrawerPhase::Open);
        controlled.unwrap_or(uncontrolled)
    });

    use_effect(move || {
        if let Some(controlled) = open() {
            let current = phase();
            if controlled && matches!(current, DrawerPhase::Closed | DrawerPhase::Closing) {
                phase.set(DrawerPhase::Opening);
            } else if !controlled && matches!(current, DrawerPhase::Opening | DrawerPhase::Open) {
                phase.set(DrawerPhase::Closing);
            }
        }
    });

    use_effect(move || {
        let current = phase();
        let next = match current {
            DrawerPhase::Opening => DrawerPhase::Open,
            DrawerPhase::Closing => DrawerPhase::Closed,
            _ => return,
        };
        spawn(async move {
            let mut after_mount = document::eval(
                "requestAnimationFrame(() => requestAnimationFrame(() => dioxus.send(true)));",
            );
            let _: Result<bool, _> = after_mount.recv().await;
            if phase() == current {
                phase.set(next);
            }
        });
    });

    let active_trigger: Signal<Option<Rc<MountedData>>> = use_signal(|| None);
    let mut previous_phase = use_signal(|| phase.cloned());
    use_effect(move || {
        let current = phase();
        let previous = previous_phase();
        if previous != current {
            if previous != DrawerPhase::Closed
                && current == DrawerPhase::Closed
                && let Some(trigger) = active_trigger()
            {
                spawn(async move {
                    let mut after_pointer =
                        document::eval("setTimeout(() => dioxus.send(true), 0);");
                    let _: Result<bool, _> = after_pointer.recv().await;
                    let _ = trigger.set_focus(true).await;
                });
            }
            previous_phase.set(current);
        }
    });
    let set_open = use_callback(move |next| {
        if open().is_none() {
            phase.set(if next {
                DrawerPhase::Opening
            } else {
                DrawerPhase::Closing
            });
        }
        on_open_change.call(next);
    });
    let side_id = use_signal(|| {
        let id = NEXT_DRAWER_ID.fetch_add(1, Ordering::Relaxed);
        format!("drawer-side-{id}")
    });
    use_context_provider(|| DrawerContext {
        open: is_open,
        set_open,
        side_id,
        phase,
        active_trigger,
    });

    let placement = placement.class();
    let base = attributes!(div {
        class: "drawer {placement}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged,
            input {
                r#type: "checkbox",
                class: "drawer-toggle",
                checked: phase() == DrawerPhase::Open,
                disabled: true,
                tabindex: "-1",
                aria_hidden: "true",
            }
            {children}
        }
    }
}

/// The page content that remains mounted whether the drawer is open or closed.
#[component]
pub fn DrawerContent(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div {
        class: "drawer-content"
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// The real button that opens a drawer.
///
/// daisyUI's checkbox stays inert; this button reports the state and controlled
/// side through ARIA and changes the Registry-owned state instead.
#[component]
pub fn DrawerTrigger(
    #[props(extends = GlobalAttributes)]
    #[props(extends = button)]
    attributes: Vec<Attribute>,
    /// Called after a click has asked the drawer to open. `extends` reaches
    /// attributes only, never event handlers, so a handler has to be its own
    /// prop.
    onclick: Option<EventHandler<MouseEvent>>,
    /// Called once the button has mounted and been remembered as the element
    /// focus returns to when the side closes; its own prop for the same reason
    /// as `onclick`.
    onmounted: Option<EventHandler<MountedEvent>>,
    children: Element,
) -> Element {
    let mut context = use_context::<DrawerContext>();
    let mut element: Signal<Option<Rc<MountedData>>> = use_signal(|| None);
    let base = attributes!(button {
        r#type: "button",
        class: "btn drawer-button",
        aria_expanded: context.is_open().to_string(),
        aria_controls: (context.side_id)(),
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        button {
            onmounted: move |event: MountedEvent| {
                element.set(Some(event.data()));
                if let Some(handler) = &onmounted {
                    handler.call(event);
                }
            },
            onclick: move |event| {
                context.active_trigger.set(element());
                context.set_open(true);
                if let Some(handler) = &onclick {
                    handler.call(event);
                }
            },
            ..merged,
            {children}
        }
    }
}

// `GlobalAttributes` and `button` both define `autofocus`, so naming it on `DrawerTrigger` would
// be ambiguous (E0034, DioxusLabs/dioxus#5271). Inherent methods win method resolution and forward
// to the element's own attribute, as `button { .. }` resolves it. Unused, they would warn in an
// app that installs this Component. Remove once upstream resolves the conflict (ADR-0034).
#[allow(dead_code)]
impl<F> DrawerTriggerPropsBuilder<F>
where
    Self: dioxus_elements::extensions::ButtonExtension,
{
    /// The `button` element's `autofocus` attribute.
    pub fn autofocus(self, value: impl dioxus::dioxus_core::IntoAttributeValue) -> Self {
        dioxus_elements::extensions::ButtonExtension::autofocus(self, value)
    }
}

/// The animation-mounted side of a drawer, backed by the dialog Primitive.
///
/// This must follow [`DrawerContent`] under [`Drawer`], because daisyUI opens it
/// through `.drawer-toggle:checked ~ .drawer-side`.
#[component]
pub fn DrawerSide(
    /// The id [`DrawerTrigger`] points `aria-controls` at.
    #[props(default)]
    id: ReadSignal<Option<String>>,
    /// Whether the side traps focus while it is open.
    #[props(default = ReadSignal::new(Signal::new(true)))]
    is_modal: ReadSignal<bool>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let mut context = use_context::<DrawerContext>();
    let primitive_modal = use_memo(move || {
        is_modal() && (!context.is_open() || (context.phase)() == DrawerPhase::Open)
    });

    use_effect(move || {
        if let Some(id) = id() {
            context.side_id.set(id);
        }
    });

    let side_id = id().unwrap_or_else(|| (context.side_id)());
    let base = attributes!(div {
        class: "drawer-side"
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        dialog::DialogRoot {
            id: Some(side_id),
            is_modal: primitive_modal,
            open: Some(context.is_primitive_open()),
            on_open_change: move |open| context.set_open(open),
            attributes: merged,
            {children}
        }
    }
}

/// The dim that dismisses the drawer through the Primitive's outside boundary.
///
/// It must be a direct child of [`DrawerSide`].
#[component]
pub fn DrawerOverlay(#[props(extends = GlobalAttributes)] attributes: Vec<Attribute>) -> Element {
    let base = attributes!(div {
        class: "drawer-overlay",
        aria_hidden: "true",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged }
    }
}

/// The drawer's sliding panel and the Primitive's focus-trapping dialog.
///
/// It must be a direct non-overlay child of [`DrawerSide`], which makes the
/// element daisyUI slides the same one the Primitive gives dialog behavior.
#[component]
pub fn DrawerPanel(
    /// The id of the dialog panel.
    #[props(default)]
    id: ReadSignal<Option<String>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let mut merged = merge_attributes(vec![attributes]);
    let class = take_class(&mut merged);

    rsx! {
        dialog::DialogContent { id, class, attributes: merged, {children} }
    }
}

/// The heading that names a drawer's panel.
#[component]
pub fn DrawerTitle(
    /// The id the Primitive uses for `aria-labelledby`.
    #[props(default)]
    id: ReadSignal<Option<String>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        dialog::DialogTitle { id, attributes, {children} }
    }
}

/// The text that describes a drawer's panel.
#[component]
pub fn DrawerDescription(
    /// The id the Primitive uses for `aria-describedby`.
    #[props(default)]
    id: ReadSignal<Option<String>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        dialog::DialogDescription { id, attributes, {children} }
    }
}

fn take_class(attributes: &mut Vec<Attribute>) -> String {
    let class = attributes.iter().position(|attribute| {
        attribute.name == "class" && matches!(attribute.value, AttributeValue::Text(_))
    });

    match class.map(|index| attributes.remove(index).value) {
        Some(AttributeValue::Text(class)) => class,
        _ => String::new(),
    }
}
