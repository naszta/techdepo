use dioxus::prelude::*;
use dioxus_field::{
    Binding, ChangeOrigin, FieldContext, FieldControlOptions, FieldMeta, FieldSurface,
    merge_attributes, use_binding, use_field_meta, use_focus_registration,
};
use dioxus_primitives::dioxus_attributes::attributes;
use std::rc::Rc;

use crate::components::field::{
    Field, FieldAppearance, FieldDescription, FieldDescriptionAppearance, FieldError,
    FieldErrorAppearance, FieldLabel,
};

/// daisyUI's colour axis for a textarea.
///
/// [`TextareaColor::Default`] emits no class, which is daisyUI's uncoloured
/// textarea rather than a synonym for [`TextareaColor::Neutral`].
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TextareaColor {
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

impl TextareaColor {
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
            Self::Neutral => "textarea-neutral",
            Self::Primary => "textarea-primary",
            Self::Secondary => "textarea-secondary",
            Self::Accent => "textarea-accent",
            Self::Info => "textarea-info",
            Self::Success => "textarea-success",
            Self::Warning => "textarea-warning",
            Self::Error => "textarea-error",
        }
    }
}

/// daisyUI's size axis for a textarea.
///
/// [`TextareaSize::Default`] emits no class and renders at the same size as
/// daisyUI's explicit `textarea-md`.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TextareaSize {
    Xs,
    Sm,
    #[default]
    Default,
    Lg,
    Xl,
}

impl TextareaSize {
    /// Every value of this axis, from smallest to largest.
    pub const ALL: &'static [Self] = &[Self::Xs, Self::Sm, Self::Default, Self::Lg, Self::Xl];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Xs => "textarea-xs",
            Self::Sm => "textarea-sm",
            Self::Default => "",
            Self::Lg => "textarea-lg",
            Self::Xl => "textarea-xl",
        }
    }
}

/// daisyUI's appearance axis for a textarea.
///
/// [`TextareaAppearance::Default`] emits no class, which is daisyUI's bordered
/// textarea rather than a named style.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TextareaAppearance {
    #[default]
    Default,
    Ghost,
}

impl TextareaAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::Ghost];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Ghost => "textarea-ghost",
        }
    }
}

/// A native multi-line text field styled with daisyUI's `textarea` class.
///
/// Producer-defined invalidity emits `textarea-error` when no colour is passed.
/// Binding, metadata, and focus resolve from explicit props, Field Context,
/// then standalone state.
///
/// Classes passed by the caller concatenate with the textarea's own; every
/// other attribute the caller passes overrides the textarea's. Event handlers
/// are explicit because Dioxus' extended attributes do not include them.
#[component]
pub fn Textarea(
    /// An explicit colour, or `None` to derive error colour from Field metadata.
    #[props(default)]
    color: Option<TextareaColor>,
    /// daisyUI's size axis.
    #[props(default)]
    size: TextareaSize,
    /// daisyUI's appearance axis.
    #[props(default)]
    appearance: TextareaAppearance,
    /// An explicit Field binding, which wins over Field Context.
    binding: Option<Binding<String>>,
    /// Explicit Field metadata, which wins over Field Context.
    meta: Option<FieldMeta>,
    /// The value rendered by the textarea.
    #[props(default)]
    value: Option<ReadSignal<String>>,
    /// Called with the textarea's value after user input.
    on_change: Option<EventHandler<String>>,
    /// Called when the native `change` event ends the interaction unit.
    on_commit: Option<EventHandler<()>>,
    /// Called after focus leaves the native textarea.
    on_focus_exit: Option<EventHandler<()>>,
    /// Whether the native textarea is required.
    #[props(default)]
    required: Option<bool>,
    /// Whether the native textarea is disabled.
    #[props(default)]
    disabled: Option<bool>,
    #[props(extends = GlobalAttributes)]
    #[props(extends = textarea)]
    attributes: Vec<Attribute>,
) -> Element {
    let binding = use_binding(binding, String::new());
    let meta = use_field_meta(meta);
    let color = color.map_or_else(
        || {
            if meta.invalid() { "textarea-error" } else { "" }
        },
        TextareaColor::class,
    );
    let size = size.class();
    let appearance = appearance.class();
    let binding_value = binding.read;
    let resolved_value = value.unwrap_or(binding_value);
    let mut focus_exit_reported = use_signal(|| false);
    let mut control: Signal<Option<Rc<MountedData>>> = use_signal(|| None);
    let focus_control = use_callback(move |()| {
        if let Some(control) = control() {
            spawn(async move {
                let _ = control.set_focus(true).await;
            });
        }
    });
    use_focus_registration(focus_control);

    let base = attributes!(textarea {
        class: "textarea {color} {size} {appearance}",
    });
    let meta_attributes = meta.attributes_for(
        &FieldControlOptions::new()
            .disabled(disabled)
            .required(required)
            .surface(FieldSurface::NATIVE),
    );
    let merged = merge_attributes(vec![meta_attributes, base, attributes]);
    let change_binding = binding.clone();
    let commit_binding = binding.clone();
    let focus_exit_binding = binding;

    rsx! {
        textarea {
            value: resolved_value,
            onmounted: move |event: MountedEvent| control.set(Some(event.data())),
            onfocusin: move |_| focus_exit_reported.set(false),
            oninput: move |event| {
                let next = event.value();
                change_binding.write(next.clone(), ChangeOrigin::User);
                if let Some(handler) = &on_change {
                    handler.call(next);
                }
            },
            onchange: move |_| {
                commit_binding.commit();
                if let Some(handler) = &on_commit {
                    handler.call(());
                }
            },
            onfocusout: move |_| {
                if focus_exit_reported() {
                    return;
                }
                focus_exit_reported.set(true);
                focus_exit_binding.focus_exit();
                if let Some(handler) = &on_focus_exit {
                    handler.call(());
                }
            },
            ..merged,
        }
    }
}

// `GlobalAttributes` and `textarea` both define `autofocus` and `spellcheck`, so naming one on
// `Textarea` would be ambiguous (E0034, DioxusLabs/dioxus#5271). Inherent methods win method
// resolution and forward to the element's own attribute, as `textarea { .. }` resolves it. Unused,
// they would warn in an app that installs this Component. Remove once upstream resolves the
// conflict (ADR-0034).
#[allow(dead_code)]
impl<F> TextareaPropsBuilder<F>
where
    Self: dioxus_elements::extensions::TextareaExtension,
{
    /// The `textarea` element's `autofocus` attribute.
    pub fn autofocus(self, value: impl dioxus::dioxus_core::IntoAttributeValue) -> Self {
        dioxus_elements::extensions::TextareaExtension::autofocus(self, value)
    }

    /// The `textarea` element's `spellcheck` attribute.
    pub fn spellcheck(self, value: impl dioxus::dioxus_core::IntoAttributeValue) -> Self {
        dioxus_elements::extensions::TextareaExtension::spellcheck(self, value)
    }
}

/// The common Field composition for a native multi-line text field.
///
/// This Composition sugar intentionally has no children. Use [`Field`] and its
/// Compound parts when content or attributes must land between the parts. Native
/// textarea attributes and caller classes are forwarded to [`Textarea`].
#[component]
pub fn TextareaField(
    /// The context supplied to the textarea and every Field part.
    #[props(into)]
    context: FieldContext,
    /// The textarea's visible label.
    label: String,
    /// Supporting text rendered between the textarea and its error region.
    #[props(default)]
    description: Option<String>,
    /// An explicit colour, or `None` to derive error colour from Field metadata.
    #[props(default)]
    color: Option<TextareaColor>,
    /// daisyUI's size axis.
    #[props(default)]
    size: TextareaSize,
    /// daisyUI's appearance axis.
    #[props(default)]
    appearance: TextareaAppearance,
    /// Whether the surrounding Field emits its default layout utilities.
    #[props(default)]
    field_appearance: FieldAppearance,
    /// Whether supporting text emits its default wrapping utilities.
    #[props(default)]
    description_appearance: FieldDescriptionAppearance,
    /// Whether the error region emits its default semantic colour.
    #[props(default)]
    error_appearance: FieldErrorAppearance,
    /// An explicit Field binding, which wins over `context` for the textarea.
    binding: Option<Binding<String>>,
    /// Explicit Field metadata, which wins over `context` for the textarea.
    meta: Option<FieldMeta>,
    /// The value rendered by the textarea.
    #[props(default)]
    value: Option<ReadSignal<String>>,
    /// Called with the textarea's value after user input.
    on_change: Option<EventHandler<String>>,
    /// Called when the native `change` event ends the interaction unit.
    on_commit: Option<EventHandler<()>>,
    /// Called after focus leaves the native textarea.
    on_focus_exit: Option<EventHandler<()>>,
    /// Whether the native textarea is required.
    #[props(default)]
    required: Option<bool>,
    /// Whether the native textarea is disabled.
    #[props(default)]
    disabled: Option<bool>,
    #[props(extends = GlobalAttributes)]
    #[props(extends = textarea)]
    attributes: Vec<Attribute>,
) -> Element {
    rsx! {
        Field { context, appearance: field_appearance,
            FieldLabel { {label} }
            Textarea {
                color,
                size,
                appearance,
                binding,
                meta,
                value,
                on_change,
                on_commit,
                on_focus_exit,
                required,
                disabled,
                attributes,
            }
            if let Some(description) = description {
                FieldDescription { appearance: description_appearance, {description} }
            }
            FieldError { appearance: error_appearance }
        }
    }
}

// `GlobalAttributes` and `textarea` both define `autofocus` and `spellcheck`, so naming one on
// `TextareaField` would be ambiguous (E0034, DioxusLabs/dioxus#5271). Inherent methods win method
// resolution and forward to the element's own attribute, as `textarea { .. }` resolves it. Unused,
// they would warn in an app that installs this Component. Remove once upstream resolves the
// conflict (ADR-0034).
#[allow(dead_code)]
impl<F> TextareaFieldPropsBuilder<F>
where
    Self: dioxus_elements::extensions::TextareaExtension,
{
    /// The `textarea` element's `autofocus` attribute.
    pub fn autofocus(self, value: impl dioxus::dioxus_core::IntoAttributeValue) -> Self {
        dioxus_elements::extensions::TextareaExtension::autofocus(self, value)
    }

    /// The `textarea` element's `spellcheck` attribute.
    pub fn spellcheck(self, value: impl dioxus::dioxus_core::IntoAttributeValue) -> Self {
        dioxus_elements::extensions::TextareaExtension::spellcheck(self, value)
    }
}
