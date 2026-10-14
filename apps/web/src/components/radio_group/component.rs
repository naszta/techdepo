use std::{
    cell::Cell,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};

use dioxus::core::{AttributeValue, ListenerCallback};
use dioxus::document;
use dioxus::prelude::*;
use dioxus_field::{
    Binding, ChangeOrigin, FieldControlOptions, FieldMeta, FieldSurface, merge_attributes,
    use_binding, use_field_meta, use_focus_registration,
};
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::radio_group;

static NEXT_GROUP_ID: AtomicUsize = AtomicUsize::new(0);

fn next_group_id() -> usize {
    NEXT_GROUP_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy)]
struct RadioGroupFieldMeta(Signal<FieldMeta>);

#[derive(Clone, Copy)]
struct RadioGroupForm {
    selected: Memo<Option<String>>,
    name: Signal<Option<String>>,
    disabled: Signal<bool>,
}

#[derive(Clone)]
struct RadioGroupFocusScope {
    generation: Rc<Cell<u64>>,
    report_exit: Callback<()>,
}

impl RadioGroupFocusScope {
    fn focus_in(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
    }

    fn focus_out(&self) {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        let scope = self.clone();
        spawn(async move {
            let mut next_task = document::eval("setTimeout(() => dioxus.send(true), 0);");
            let _: Result<bool, _> = next_task.recv().await;
            if scope.generation.get() == generation {
                scope.generation.set(generation.wrapping_add(1));
                scope.report_exit.call(());
            }
        });
    }
}

/// daisyUI's colour axis for a radio item.
///
/// [`RadioItemColor::Default`] emits no class at all, which is daisyUI's own
/// uncoloured radio rather than a synonym for [`RadioItemColor::Neutral`].
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum RadioItemColor {
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

impl RadioItemColor {
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
            Self::Neutral => "radio-neutral",
            Self::Primary => "radio-primary",
            Self::Secondary => "radio-secondary",
            Self::Accent => "radio-accent",
            Self::Info => "radio-info",
            Self::Success => "radio-success",
            Self::Warning => "radio-warning",
            Self::Error => "radio-error",
        }
    }
}

/// Whether [`RadioGroup`] emits the utilities that lay its items out.
///
/// daisyUI has no class for the group at all (its own examples are a stack of
/// radios inside whatever the page already had) so the utilities that make a
/// group read as one are emitted here instead. They follow the orientation the
/// primitive reports rather than a prop of their own, through a variant on the
/// `data-orientation` attribute it already sets.
///
/// That inverts the usual convention: [`RadioGroupAppearance::Default`] emits
/// classes and [`RadioGroupAppearance::None`] emits nothing. A utility this
/// component emits only ties with a caller's, and a tie is settled by
/// generated-stylesheet order rather than by the class attribute, so switching
/// ours off is the way to win it (ADR-0004).
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum RadioGroupAppearance {
    #[default]
    Default,
    None,
}

impl RadioGroupAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::None];

    /// The Tailwind utilities for this value, as complete string literals so
    /// Tailwind's scanner can see them.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "flex gap-3 data-[orientation=vertical]:flex-col",
            Self::None => "",
        }
    }
}

/// A group of radio items, of which one is chosen.
///
/// The group is the primitive's: it owns the value, moves the selection with
/// the arrow keys, keeps one tab stop for the whole group, and skips disabled
/// items on the way. daisyUI styles the items rather than the group, so what is
/// emitted here is layout and nothing else, behind an appearance axis that
/// switches it off.
///
/// Classes passed by the caller concatenate with the group's own; every other
/// public attribute the caller passes overrides them. `data-field-group` is
/// reserved for the generated locator used by Field focus requests.
///
/// Field metadata belongs to the group root: it describes the one value and
/// its ARIA relationships. Items only read invalidity from that metadata when
/// deciding their omitted colour.
#[component]
pub fn RadioGroup(
    /// Whether to emit the utilities that lay the items out.
    #[props(default)]
    appearance: RadioGroupAppearance,
    /// An explicit Field binding, which wins over Field Context.
    binding: Option<Binding<String>>,
    /// Explicit Field metadata, which wins over Field Context.
    meta: Option<FieldMeta>,
    /// The controlled value of the group, which is the value of the chosen
    /// item.
    #[props(default)]
    value: ReadSignal<Option<String>>,
    /// The value chosen when the group is not controlled.
    #[props(default)]
    default_value: String,
    /// Called with the chosen value after user interaction.
    on_change: Option<EventHandler<String>>,
    /// Called after every change ends its interaction unit.
    on_commit: Option<EventHandler<()>>,
    /// Called after focus leaves every item owned by the group.
    on_focus_exit: Option<EventHandler<()>>,
    /// Whether every item in the group is disabled.
    #[props(default)]
    disabled: Option<bool>,
    /// Whether the group is announced as required.
    #[props(default)]
    required: Option<bool>,
    /// The Field name override. The `div[role=radiogroup]` root omits it and
    /// registry-owned hidden radio inputs use it for form submission.
    #[props(default)]
    name: Option<String>,
    /// Whether the group is laid out in a row, which is also which arrow keys
    /// move the selection.
    #[props(default)]
    horizontal: ReadSignal<bool>,
    /// Whether arrow-key navigation wraps around at the ends of the group. The
    /// default repeats the primitive's own, since a prop declared here has to
    /// carry one.
    #[props(default = ReadSignal::new(Signal::new(true)))]
    roving_loop: ReadSignal<bool>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let binding = use_binding(binding, default_value.clone());
    let meta = use_field_meta(meta);
    let focus_exit_binding = binding.clone();
    let report_exit = use_callback(move |()| {
        focus_exit_binding.focus_exit();
        if let Some(handler) = &on_focus_exit {
            handler.call(());
        }
    });
    use_context_provider(|| RadioGroupFocusScope {
        generation: Rc::new(Cell::new(0)),
        report_exit,
    });
    let field_meta = use_context_provider(|| RadioGroupFieldMeta(Signal::new(meta)));
    let mut field_meta_value = field_meta.0;
    let meta_changed = {
        let current = field_meta_value.peek();
        *current != meta
    };
    if meta_changed {
        field_meta_value.set(meta);
    }
    let appearance = appearance.class();
    let binding_value = binding.read;
    let selected = use_memo(move || {
        Some(match value() {
            Some(value) => value,
            None => binding_value(),
        })
    });
    let resolved_required = required.unwrap_or_else(|| meta.required());
    let resolved_disabled = disabled.unwrap_or_else(|| meta.disabled());
    let resolved_name = name
        .clone()
        .or_else(|| meta.name().map(|name| name.to_string()));
    let form = use_context_provider(|| RadioGroupForm {
        selected,
        name: Signal::new(resolved_name.clone()),
        disabled: Signal::new(resolved_disabled),
    });
    let mut form_name = form.name;
    if *form_name.peek() != resolved_name {
        form_name.set(resolved_name);
    }
    let mut form_disabled = form.disabled;
    if *form_disabled.peek() != resolved_disabled {
        form_disabled.set(resolved_disabled);
    }
    let group_uid = use_hook(next_group_id);
    let focus_group = use_callback(move |()| {
        if resolved_disabled {
            return;
        }
        spawn(async move {
            let script = format!(
                r#"
                const group = document.querySelector('[data-field-group="{group_uid}"]');
                const item = group?.querySelector(
                    '[role="radio"][tabindex="0"]:not(:disabled)'
                );
                item?.focus();
                dioxus.send(item != null && document.activeElement === item);
                "#,
            );
            let mut focus_item = document::eval(&script);
            // Preserve delivery until FocusRequest can report it:
            // https://github.com/sagikazarmark/dioxus-field/issues/3
            let _: Result<bool, _> = focus_item.recv().await;
        });
    });
    use_focus_registration(focus_group);

    let base = attributes!(div {
        class: "{appearance}",
        tabindex: "-1",
    });
    let meta_attributes = meta.attributes_for(
        &FieldControlOptions::new()
            .disabled(disabled)
            .required(required)
            .name(name.map(Rc::from))
            .surface(FieldSurface::ARIA_WIDGET),
    );
    let focus_locator = attributes!(div {
        "data-field-group": "{group_uid}",
    });
    let merged = merge_attributes(vec![meta_attributes, base, attributes, focus_locator]);
    let change_binding = binding.clone();
    let commit_binding = binding;

    rsx! {
        radio_group::RadioGroup {
            value: selected,
            default_value,
            on_value_change: move |next: String| {
                change_binding.write(next.clone(), ChangeOrigin::User);
                if let Some(handler) = &on_change {
                    handler.call(next);
                }
                commit_binding.commit();
                if let Some(handler) = &on_commit {
                    handler.call(());
                }
            },
            disabled: resolved_disabled,
            required: resolved_required,
            horizontal,
            roving_loop,
            attributes: merged,
            {children}
        }
    }
}

/// One item of a radio group, styled with daisyUI's `radio` classes.
///
/// The chosen state needs no bridging: daisyUI's rule is
/// `.radio:checked, .radio[aria-checked=true]`, and the primitive sets
/// `aria-checked` on the `button` it renders. Neither does the disabled state,
/// which the primitive puts on the element as the native attribute
/// `.radio:disabled` matches.
///
/// The item renders nothing inside itself (daisyUI draws the dot from a
/// `::before` on the element) so its label is the caller's, beside the item
/// rather than in it, and the item is named by `aria_label` or
/// `aria_labelledby`.
///
/// **There is no size axis**, for the reason the switch has none (ADR-0010).
/// daisyUI's `.radio-lg` sets the padding around the dot on any element, but
/// puts the size itself behind `.radio-lg[type=radio]`, so on the primitive's
/// `button` the classes would leave the control the same size with a smaller
/// dot in it, which is the opposite of what their names say.
///
/// Classes passed by the caller concatenate with the item's own; every other
/// attribute the caller passes overrides them.
#[component]
pub fn RadioItem(
    /// An explicit colour, or `None` to derive error colour from Field metadata.
    #[props(default)]
    color: Option<RadioItemColor>,
    /// What this item is worth, which is what the group's value becomes when
    /// it is chosen.
    value: ReadSignal<String>,
    /// Where this item falls in the keyboard navigation order.
    index: ReadSignal<usize>,
    /// Whether this item is disabled.
    #[props(default)]
    disabled: ReadSignal<bool>,
    /// The id of this element. Declared rather than left to the attribute list,
    /// because the primitive takes one as a prop of its own and puts it on the
    /// element; an id arriving as an attribute would meet the one already
    /// there.
    #[props(default)]
    id: Option<String>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let meta = *use_context::<RadioGroupFieldMeta>().0.read();
    let form = use_context::<RadioGroupForm>();
    let focus_scope = use_context::<RadioGroupFocusScope>();
    let color = color.map_or_else(
        || {
            if meta.invalid() { "radio-error" } else { "" }
        },
        RadioItemColor::class,
    );

    // `button` rather than an input of some kind: the primitive renders a
    // `button` with `role="radio"`, and this list ends up spread onto it, so
    // that is the element the attribute has to be namespaced for.
    let base = attributes!(button {
        class: "radio {color}",
    });
    let mut merged = merge_attributes(vec![base, attributes]);
    let caller_focus_in = take_event_listener(&mut merged, "onfocusin");
    let caller_focus_out = take_event_listener(&mut merged, "onfocusout");
    let focus_in_scope = focus_scope.clone();
    let focus_events = attributes!(button {
        onfocusin: move |event: FocusEvent| {
            if let Some(listener) = &caller_focus_in {
                listener.call(event.into_any());
            }
            focus_in_scope.focus_in();
        },
        onfocusout: move |event: FocusEvent| {
            if let Some(listener) = &caller_focus_out {
                listener.call(event.into_any());
            }
            focus_scope.focus_out();
        },
    });
    let mut merged = merge_attributes(vec![merged, focus_events]);

    let class = take_class(&mut merged);

    rsx! {
        radio_group::RadioItem {
            value,
            index,
            disabled,
            id,
            class: Some(class),
            attributes: merged,
        }
        if let Some(name) = (form.name)() {
            input {
                type: "radio",
                aria_hidden: "true",
                tabindex: "-1",
                name,
                value,
                checked: (form.selected)().is_some_and(|selected| selected == value()),
                disabled: (form.disabled)() || disabled(),
                // This participant carries native form state; the Primitive's
                // button remains the visible and keyboard-operable control.
                style: "transform: translateX(-100%); position: absolute; pointer-events: none; opacity: 0; margin: 0; width: 0; height: 0;",
            }
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

fn take_event_listener(attributes: &mut Vec<Attribute>, name: &str) -> Option<ListenerCallback> {
    let index = attributes.iter().position(|attribute| {
        attribute.name == name && matches!(attribute.value, AttributeValue::Listener(_))
    })?;
    match attributes.remove(index).value {
        AttributeValue::Listener(listener) => Some(listener),
        _ => unreachable!(),
    }
}
