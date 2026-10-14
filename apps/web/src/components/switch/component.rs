use dioxus::core::{AttributeValue, ListenerCallback};
use dioxus::prelude::*;
use dioxus_field::{
    Binding, ChangeOrigin, FieldContext, FieldControlOptions, FieldMeta, FieldSurface,
    merge_attributes, use_binding, use_field_meta, use_focus_registration,
};
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::switch;
use std::rc::Rc;

use crate::components::field::{
    Field, FieldAppearance, FieldDescription, FieldDescriptionAppearance, FieldError,
    FieldErrorAppearance, FieldLabel, FieldRow, FieldRowAppearance,
};

/// daisyUI's colour axis for a switch, which is the colour it fills with once
/// it is on.
///
/// [`SwitchColor::Default`] emits no class at all, which is daisyUI's own
/// uncoloured toggle rather than a synonym for [`SwitchColor::Neutral`].
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum SwitchColor {
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

impl SwitchColor {
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
            Self::Neutral => "toggle-neutral",
            Self::Primary => "toggle-primary",
            Self::Secondary => "toggle-secondary",
            Self::Accent => "toggle-accent",
            Self::Info => "toggle-info",
            Self::Success => "toggle-success",
            Self::Warning => "toggle-warning",
            Self::Error => "toggle-error",
        }
    }
}

/// A switch styled with daisyUI's `toggle` classes.
///
/// The checked state needs no bridging: daisyUI's rule is
/// `.toggle:checked, .toggle[aria-checked=true]`, and the primitive sets
/// `aria-checked` on the `button` it renders. Colour is the same rule again,
/// `.toggle-primary[aria-checked=true]`, so nothing here emits a class for
/// state either. Producer-defined invalidity emits `toggle-error` when no
/// colour is passed. Binding, metadata, and focus resolve from explicit props,
/// Field Context, then standalone state.
///
/// **There is no size axis**, because daisyUI's size classes cannot reach this
/// element: every one of them is written
/// `.toggle-sm[type=checkbox], .toggle-sm:has([type=checkbox])`, and the
/// primitive renders a `button` with the hidden form input beside it rather
/// than inside it. The component's documentation records what that costs and
/// what a caller can do about it.
///
/// The primitive's thumb part is not exposed and neither are children:
/// daisyUI draws the knob itself, from a `::before` on the toggle, and the
/// marks it lays out on either side of that knob are swapped by
/// `.toggle:has(:checked)`, which this markup cannot satisfy for the same
/// reason the sizes cannot match. A caller who wants either uses the primitive
/// directly.
///
/// Classes passed by the caller concatenate with the switch's own; every other
/// attribute the caller passes overrides the switch's.
///
/// Like the checkbox, this takes no `extends = button` list: the primitive
/// renders the `button` element and owns the attributes worth reaching
/// (submitted value and `disabled`) as props of its own, so extending them
/// here would offer a second, conflicting way to set them.
#[component]
pub fn Switch(
    /// An explicit colour, or `None` to derive error colour from Field metadata.
    #[props(default)]
    color: Option<SwitchColor>,
    /// An explicit Field binding, which wins over Field Context.
    binding: Option<Binding<bool>>,
    /// Explicit Field metadata, which wins over Field Context.
    meta: Option<FieldMeta>,
    /// The controlled value of the switch.
    #[props(default)]
    value: ReadSignal<Option<bool>>,
    /// The state the switch starts in when it is not controlled.
    #[props(default)]
    default_value: bool,
    /// Whether the switch is required in a form.
    #[props(default)]
    required: Option<bool>,
    /// Whether the switch is disabled.
    #[props(default)]
    disabled: Option<bool>,
    /// The name of the switch, used in forms.
    #[props(default)]
    name: Option<String>,
    /// The submitted value of the switch. The default repeats the
    /// primitive's own, since a prop declared here has to carry one.
    #[props(default = ReadSignal::new(Signal::new(String::from("on"))))]
    form_value: ReadSignal<String>,
    /// Called with the switch's value after user interaction.
    on_change: Option<EventHandler<bool>>,
    /// Called after every change ends its interaction unit.
    on_commit: Option<EventHandler<()>>,
    /// Called after focus leaves the switch button.
    on_focus_exit: Option<EventHandler<()>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let binding = use_binding(binding, default_value);
    let meta = use_field_meta(meta);
    let color = color.map_or_else(
        || {
            if meta.invalid() { "toggle-error" } else { "" }
        },
        SwitchColor::class,
    );
    let binding_value = binding.read;
    let checked = use_memo(move || {
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
    let mut control: Signal<Option<Rc<MountedData>>> = use_signal(|| None);
    let focus_control = use_callback(move |()| {
        if let Some(control) = control() {
            spawn(async move {
                let _ = control.set_focus(true).await;
            });
        }
    });
    use_focus_registration(focus_control);

    // `button` rather than an input of some kind: the primitive renders a
    // `button` with `role="switch"`, and this list ends up spread onto it, so
    // that is the element the attribute has to be namespaced for.
    let base = attributes!(button {
        class: "toggle {color}",
    });
    let meta_attributes = meta.attributes_for(
        &FieldControlOptions::new()
            .disabled(disabled)
            .required(required)
            .name(name.map(Rc::from))
            .surface(FieldSurface::BUTTON_WIDGET),
    );
    let change_binding = binding.clone();
    let commit_binding = binding.clone();
    let focus_exit_binding = binding;
    let mut merged = merge_attributes(vec![meta_attributes, base, attributes]);
    let caller_focus_out = take_event_listener(&mut merged, "onfocusout");
    let interaction = attributes!(button {
        onmounted: move |event: MountedEvent| control.set(Some(event.data())),
        onfocusout: move |event: FocusEvent| {
            if let Some(listener) = &caller_focus_out {
                listener.call(event.into_any());
            }
            focus_exit_binding.focus_exit();
            if let Some(handler) = &on_focus_exit {
                handler.call(());
            }
        },
    });
    let merged = merge_attributes(vec![merged, interaction]);

    rsx! {
        switch::Switch {
            checked,
            default_checked: default_value,
            required: resolved_required,
            disabled: resolved_disabled,
            // The pinned Primitive omits `required` from its hidden checkbox.
            // Leave that input unnamed and render the form participant below.
            name: String::new(),
            value: form_value,
            on_checked_change: move |next| {
                change_binding.write(next, ChangeOrigin::User);
                if let Some(handler) = &on_change {
                    handler.call(next);
                }
                commit_binding.commit();
                if let Some(handler) = &on_commit {
                    handler.call(());
                }
                if let Some(control) = control() {
                    spawn(async move {
                        let _ = control.set_focus(true).await;
                    });
                }
            },
            attributes: merged,
        }
        input {
            type: "checkbox",
            aria_hidden: "true",
            tabindex: "-1",
            name: resolved_name,
            value: form_value,
            checked: checked().unwrap_or(default_value),
            required: resolved_required,
            disabled: resolved_disabled,
            // Hiding this form participant is structural behavior. Match the
            // Primitive's input rather than relying on Tailwind source discovery.
            style: "transform: translateX(-100%); position: absolute; pointer-events: none; opacity: 0; margin: 0; width: 0; height: 0;",
        }
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

/// The common Field composition for a switch.
///
/// This Composition sugar intentionally has no children. Use [`Field`] and its
/// Compound parts for inline or custom layouts, or when content or attributes
/// must land between the parts. Global attributes and caller classes are
/// forwarded to [`Switch`].
#[component]
pub fn SwitchField(
    /// The context supplied to the switch and every Field part.
    #[props(into)]
    context: FieldContext,
    /// The switch's visible label.
    label: String,
    /// Supporting text rendered between the switch and its error region.
    #[props(default)]
    description: Option<String>,
    /// An explicit colour, or `None` to derive error colour from Field metadata.
    #[props(default)]
    color: Option<SwitchColor>,
    /// Whether the surrounding Field emits its default layout utilities.
    #[props(default)]
    field_appearance: FieldAppearance,
    /// Whether the control-and-label row emits its default layout utilities.
    #[props(default)]
    row_appearance: FieldRowAppearance,
    /// Whether supporting text emits its default wrapping utilities.
    #[props(default)]
    description_appearance: FieldDescriptionAppearance,
    /// Whether the error region emits its default semantic colour.
    #[props(default)]
    error_appearance: FieldErrorAppearance,
    /// An explicit Field binding, which wins over `context` for the switch.
    binding: Option<Binding<bool>>,
    /// Explicit Field metadata, which wins over `context` for the switch.
    meta: Option<FieldMeta>,
    /// The controlled value of the switch.
    #[props(default)]
    value: ReadSignal<Option<bool>>,
    /// The state the switch starts in when it is not controlled.
    #[props(default)]
    default_value: bool,
    /// Whether the switch is required in a form.
    #[props(default)]
    required: Option<bool>,
    /// Whether the switch is disabled.
    #[props(default)]
    disabled: Option<bool>,
    /// The name of the switch, used in forms.
    #[props(default)]
    name: Option<String>,
    /// The submitted value of the switch.
    #[props(default = ReadSignal::new(Signal::new(String::from("on"))))]
    form_value: ReadSignal<String>,
    /// Called with the switch's value after user interaction.
    on_change: Option<EventHandler<bool>>,
    /// Called after every change ends its interaction unit.
    on_commit: Option<EventHandler<()>>,
    /// Called after focus leaves the switch button.
    on_focus_exit: Option<EventHandler<()>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    rsx! {
        Field { context, appearance: field_appearance,
            FieldRow { appearance: row_appearance,
                Switch {
                    color,
                    binding,
                    meta,
                    value,
                    default_value,
                    required,
                    disabled,
                    name,
                    form_value,
                    on_change,
                    on_commit,
                    on_focus_exit,
                    attributes,
                }
                FieldLabel { {label} }
            }
            if let Some(description) = description {
                FieldDescription { appearance: description_appearance, {description} }
            }
            FieldError { appearance: error_appearance }
        }
    }
}
