use std::{ops::Range, rc::Rc};

use dioxus::core::{AttributeValue, ListenerCallback};
use dioxus::prelude::*;
use dioxus_field::{
    Binding, ChangeOrigin, FieldContext, FieldControlOptions, FieldMeta, FieldSurface,
    ValiditySurface, merge_attributes, use_binding, use_field_meta, use_focus_registration,
};
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::slider;

use crate::components::field::{
    Field, FieldAppearance, FieldDescription, FieldDescriptionAppearance, FieldError,
    FieldErrorAppearance, FieldLabel,
};

const POINTER_CAPTURE_JS: &str = r#"
    const installed = Symbol.for('dioxus-daisyui-components.slider.pointer-capture');
    const roots = document.querySelectorAll('[data-slider-pointer-capture="true"]');
    for (const root of roots) {
        if (root[installed]) continue;
        root[installed] = true;
        root.addEventListener('pointerdown', event => {
            if (event.isPrimary && event.button === 0) {
                root.setPointerCapture(event.pointerId);
            }
        }, { capture: true });
    }
    dioxus.send(true);
"#;

const FOCUS_EXIT_MACROTASK_JS: &str = "setTimeout(() => dioxus.send(true), 0);";

#[derive(Clone, Copy)]
struct SliderFieldContext {
    disabled: Signal<bool>,
    first_thumb: Signal<Option<Rc<MountedData>>>,
    second_thumb: Signal<Option<Rc<MountedData>>>,
    pointer_change_pending: Signal<bool>,
    keyboard_change_pending: Signal<bool>,
    change_sequence: Signal<u64>,
    rendered_sequence: CopyValue<u64>,
    commit: Callback<()>,
}

fn is_slider_key(key: &Key) -> bool {
    matches!(
        key,
        Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight
    )
}

fn focus_slider_thumb(thumb: Signal<Option<Rc<MountedData>>>) {
    if let Some(control) = thumb() {
        spawn(async move {
            let _ = control.set_focus(true).await;
        });
    }
}

/// The root props [`SliderRoot`] and [`RangeSliderRoot`] resolve identically,
/// bundled so [`use_slider_root`] can resolve them once for both.
struct SliderRootOptions<T: 'static> {
    color: Option<SliderColor>,
    size: SliderSize,
    appearance: SliderAppearance,
    binding: Option<Binding<T>>,
    meta: Option<FieldMeta>,
    value: ReadSignal<Option<T>>,
    default_value: T,
    required: Option<bool>,
    disabled: Option<bool>,
    name: Option<String>,
    on_commit: Option<EventHandler<()>>,
    on_focus_exit: Option<EventHandler<()>>,
    attributes: Vec<Attribute>,
}

/// What a slider root renders from once its Field integration is resolved.
struct ResolvedSliderRoot<T: 'static> {
    binding: Binding<T>,
    value: Memo<Option<T>>,
    disabled: bool,
    first_thumb: Signal<Option<Rc<MountedData>>>,
    second_thumb: Signal<Option<Rc<MountedData>>>,
    pointer_change_pending: Signal<bool>,
    change_sequence: Signal<u64>,
    attributes: Vec<Attribute>,
}

/// Resolves the Field integration both roots share: binding and metadata
/// precedence, the error colour, commit and Focus Exit plumbing, the thumb
/// focus request, and the merged root attribute list.
fn use_slider_root<T: Clone + PartialEq + 'static>(
    props: SliderRootOptions<T>,
) -> ResolvedSliderRoot<T> {
    let binding = use_binding(props.binding, props.default_value);
    let meta = use_field_meta(props.meta);
    let color = props.color.map_or_else(
        || {
            if meta.invalid() { "range-error" } else { "" }
        },
        SliderColor::class,
    );
    let size = props.size.class();
    let appearance = props.appearance.class();
    let binding_value = binding.read;
    let value = props.value;
    let resolved_value = use_memo(move || {
        Some(match value() {
            Some(value) => value,
            None => binding_value(),
        })
    });
    let resolved_disabled = props.disabled.unwrap_or_else(|| meta.disabled());
    let mut disabled = use_signal(|| resolved_disabled);
    if *disabled.peek() != resolved_disabled {
        disabled.set(resolved_disabled);
    }
    let pointer_change_pending = use_signal(|| false);
    let keyboard_change_pending = use_signal(|| false);
    let change_sequence = use_signal(|| 0u64);
    let focus_sequence = use_hook(|| CopyValue::new(0u64));
    let mut rendered_sequence = use_hook(|| CopyValue::new(0u64));
    rendered_sequence.set(change_sequence());
    let first_thumb: Signal<Option<Rc<MountedData>>> = use_signal(|| None);
    let second_thumb: Signal<Option<Rc<MountedData>>> = use_signal(|| None);
    let focus_control = use_callback(move |()| {
        focus_slider_thumb(first_thumb);
    });
    use_focus_registration(focus_control);
    let commit_binding = binding.clone();
    let on_commit = props.on_commit;
    let commit = use_callback(move |()| {
        commit_binding.commit();
        if let Some(handler) = &on_commit {
            handler.call(());
        }
    });
    let focus_exit_binding = binding.clone();
    let on_focus_exit = props.on_focus_exit;
    let focus_exit = use_callback(move |()| {
        focus_exit_binding.focus_exit();
        if let Some(handler) = &on_focus_exit {
            handler.call(());
        }
    });
    use_context_provider(move || SliderFieldContext {
        disabled,
        first_thumb,
        second_thumb,
        pointer_change_pending,
        keyboard_change_pending,
        change_sequence,
        rendered_sequence,
        commit,
    });

    let base = attributes!(div {
        class: "range {color} {size} {appearance}",
    });
    let meta_attributes = meta.attributes_for(
        &FieldControlOptions::new()
            .disabled(props.disabled)
            .required(props.required)
            .name(props.name.map(Rc::from))
            .surface(FieldSurface {
                validity: ValiditySurface::Omit,
                ..FieldSurface::ARIA_WIDGET
            }),
    );
    let mut caller_attributes = props.attributes;
    let caller_focus_in = take_event_listener(&mut caller_attributes, "onfocusin");
    let caller_focus_out = take_event_listener(&mut caller_attributes, "onfocusout");
    let mut pointerup_pending = pointer_change_pending;
    let mut pointercancel_pending = pointer_change_pending;
    let mut focusin_sequence = focus_sequence;
    let mut focusout_sequence = focus_sequence;
    let release = attributes!(div {
        "data-slider-pointer-capture": "true",
        onmounted: move |_| {
            spawn(async move {
                let mut setup = document::eval(POINTER_CAPTURE_JS);
                let _: Result<bool, _> = setup.recv().await;
            });
        },
        onpointerup: move |_| {
            if !resolved_disabled && pointerup_pending() {
                pointerup_pending.set(false);
                commit.call(());
            }
        },
        onpointercancel: move |_| {
            if !resolved_disabled && pointercancel_pending() {
                pointercancel_pending.set(false);
                commit.call(());
            }
        },
        onfocusin: move |event: FocusEvent| {
            if let Some(listener) = &caller_focus_in {
                listener.call(event.into_any());
            }
            focusin_sequence.set(focusin_sequence.cloned().wrapping_add(1));
        },
        onfocusout: move |event: FocusEvent| {
            if let Some(listener) = &caller_focus_out {
                listener.call(event.into_any());
            }
            let sequence = focusout_sequence.cloned().wrapping_add(1);
            focusout_sequence.set(sequence);
            spawn(async move {
                let mut after_focus = document::eval(FOCUS_EXIT_MACROTASK_JS);
                let _: Result<bool, _> = after_focus.recv().await;
                if focusout_sequence.cloned() == sequence {
                    focus_exit.call(());
                }
            });
        },
    });
    let attributes = merge_attributes(vec![meta_attributes, base, release, caller_attributes]);

    ResolvedSliderRoot {
        binding,
        value: resolved_value,
        disabled: resolved_disabled,
        first_thumb,
        second_thumb,
        pointer_change_pending,
        change_sequence,
        attributes,
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

/// daisyUI's colour axis for a slider.
///
/// Every value of it sets two custom properties and nothing else:
/// `.range-primary` is `color: var(--color-primary); --range-thumb:
/// var(--color-primary-content)`, which is what lets the parts this component
/// draws take their colour from daisyUI rather than from a second mapping of
/// the registry's own (ADR-0016).
///
/// [`SliderColor::Default`] emits no class, which is daisyUI's uncoloured range:
/// a track and a fill in the page's own text colour.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum SliderColor {
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

impl SliderColor {
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
            Self::Neutral => "range-neutral",
            Self::Primary => "range-primary",
            Self::Secondary => "range-secondary",
            Self::Accent => "range-accent",
            Self::Info => "range-info",
            Self::Success => "range-success",
            Self::Warning => "range-warning",
            Self::Error => "range-error",
        }
    }
}

/// daisyUI's size axis for a slider, which is a thumb size everything else is
/// measured from.
///
/// `.range-lg` sets `--range-thumb-size` and nothing else; daisyUI's own rules
/// take the control's height from it and the native track's from half of it, and
/// the utilities this component draws the parts with do the same (ADR-0016).
///
/// [`SliderSize::Default`] emits no class, which renders at the same size as
/// daisyUI's explicit `range-md`.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum SliderSize {
    Xs,
    Sm,
    #[default]
    Default,
    Lg,
    Xl,
}

impl SliderSize {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Xs, Self::Sm, Self::Default, Self::Lg, Self::Xl];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Xs => "range-xs",
            Self::Sm => "range-sm",
            Self::Default => "",
            Self::Lg => "range-lg",
            Self::Xl => "range-xl",
        }
    }
}

/// Whether the root emits the utilities that make it the box the parts are laid
/// out in.
///
/// daisyUI's `range` gives this element its height, its width and its corners,
/// but nothing that positions anything inside it; a native range has no inside.
/// So the positioning context is emitted here, together with the muting daisyUI
/// writes as `.range:disabled`, which cannot fire on an element that is not a
/// form control.
///
/// That inverts the usual convention: [`SliderAppearance::Default`] emits
/// classes and [`SliderAppearance::None`] emits nothing. A utility this
/// component emits only ties with a caller's, and a tie is settled by
/// generated-stylesheet order rather than by the class attribute, so switching
/// ours off is the way to win it (ADR-0004).
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum SliderAppearance {
    #[default]
    Default,
    None,
}

impl SliderAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::None];

    /// The Tailwind utilities for this value, as complete string literals so
    /// Tailwind's scanner can see them.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => {
                "relative data-[disabled=true]:cursor-not-allowed data-[disabled=true]:opacity-30"
            }
            Self::None => "",
        }
    }
}

/// Whether [`SliderTrack`] emits the utilities that draw the groove.
///
/// daisyUI draws the groove in `::-webkit-slider-runnable-track` and
/// `::-moz-range-track`, which exist on a native input and nowhere else
/// (ADR-0016). What is emitted here is the same declarations, reading daisyUI's
/// own `--range-bg` and `--range-thumb-size` rather than naming a colour or a
/// height of this registry's own.
///
/// The groove is inset by half a thumb at each end, which is where a native
/// range puts the ends of the thumb's travel, so the fill and the thumb line up
/// with the value the same way daisyUI's do.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum SliderTrackAppearance {
    #[default]
    Default,
    None,
}

impl SliderTrackAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::None];

    /// The Tailwind utilities for this value, as complete string literals so
    /// Tailwind's scanner can see them.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => {
                "absolute top-1/2 inset-x-[calc(var(--range-thumb-size)/2)] h-[calc(var(--range-thumb-size)/2)] -translate-y-1/2 rounded-[var(--radius-selector)] border border-current bg-[color:var(--range-bg)]"
            }
            Self::None => "",
        }
    }
}

/// Whether [`SliderRange`] emits the utilities that draw the filled part.
///
/// daisyUI fills a native range with an inset box shadow on the thumb, painted
/// in `--range-progress`, which is `currentColor`. The fill here is
/// `bg-current`, so it is the same colour by the same route and the colour axis
/// needs no second mapping (ADR-0016, and ADR-0012 before it).
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum SliderRangeAppearance {
    #[default]
    Default,
    None,
}

impl SliderRangeAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::None];

    /// The Tailwind utilities for this value, as complete string literals so
    /// Tailwind's scanner can see them.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "absolute inset-y-0 rounded-[var(--radius-selector)] bg-current",
            Self::None => "",
        }
    }
}

/// Whether [`SliderThumb`] emits the utilities that draw the handle.
///
/// daisyUI draws it in `::-webkit-slider-thumb` and `::-moz-range-thumb`
/// (ADR-0016), sized by `--range-thumb-size`, filled with `--range-thumb` and
/// bordered by `--range-p` of `currentColor`. Those are the declarations emitted
/// here, minus the box shadow that draws the fill; that is [`SliderRange`]'s
/// job, because the primitive gives it an element of its own.
///
/// The focus ring is daisyUI's `.range:focus-visible`, moved to the element that
/// actually takes focus: the thumb is a `button`, where a native range is one
/// control that focuses as a whole.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum SliderThumbAppearance {
    #[default]
    Default,
    None,
}

impl SliderThumbAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::None];

    /// The Tailwind utilities for this value, as complete string literals so
    /// Tailwind's scanner can see them.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => {
                "absolute top-1/2 size-[var(--range-thumb-size)] -translate-x-1/2 -translate-y-1/2 rounded-[var(--radius-selector)] border-[length:var(--range-p)] border-current bg-[color:var(--range-thumb)] focus-visible:outline-2 focus-visible:outline-offset-2"
            }
            Self::None => "",
        }
    }
}

/// A slider with one thumb, carrying daisyUI's `range` classes.
///
/// There is no value state to bridge: daisyUI has no class for which number the
/// slider holds. Producer-defined invalidity does use Tier 2 bridging, emitting
/// `range-error` when no colour is passed. The component otherwise splits
/// daisyUI's own class three ways: `range` and both axes stay here, and the
/// groove, fill and handle use utilities that read daisyUI's custom properties.
/// ADR-0016 records why.
///
/// The primitive gives this element `role="group"` and makes it the surface a
/// pointer drives: a press anywhere on it moves the nearest thumb to that
/// position, which is what a native range does too.
///
/// [`SliderTrack`] must stay a direct child, and [`SliderRange`] and
/// [`SliderThumb`] inside it: the inline percentages the primitive positions the
/// last two with are percentages of the element they are in.
///
/// Classes passed by the caller concatenate with this element's own; every other
/// attribute the caller passes overrides them.
#[component]
pub fn SliderRoot(
    /// An explicit colour, or `None` to derive error colour from Field metadata.
    #[props(default)]
    color: Option<SliderColor>,
    /// daisyUI's size axis.
    #[props(default)]
    size: SliderSize,
    /// Whether to emit the utilities that lay the parts out.
    #[props(default)]
    appearance: SliderAppearance,
    /// An explicit Field binding, which wins over Field Context.
    binding: Option<Binding<f64>>,
    /// Explicit Field metadata, which wins over Field Context.
    meta: Option<FieldMeta>,
    /// The controlled value. `Some` makes the slider controlled.
    #[props(default)]
    value: ReadSignal<Option<f64>>,
    /// The value the slider starts at when it is not controlled.
    #[props(default)]
    default_value: f64,
    /// What the slider is worth at its start. The default repeats the
    /// primitive's own, since a prop declared here has to carry one.
    #[props(default = ReadSignal::new(Signal::new(0.0)))]
    min: ReadSignal<f64>,
    /// What it is worth at its end. The default repeats the primitive's own.
    #[props(default = ReadSignal::new(Signal::new(100.0)))]
    max: ReadSignal<f64>,
    /// How far one arrow key moves it. The default repeats the primitive's own.
    #[props(default = ReadSignal::new(Signal::new(1.0)))]
    step: ReadSignal<f64>,
    /// Whether the slider is required according to its producer.
    #[props(default)]
    required: Option<bool>,
    /// Whether the slider is disabled, which leaves it inert.
    #[props(default)]
    disabled: Option<bool>,
    /// The Field name override. The `div[role=group]` root omits native names.
    #[props(default)]
    name: Option<String>,
    /// Whether the slider runs across rather than down. The default repeats the
    /// primitive's own; see the component's documentation for what daisyUI does
    /// and does not have for a vertical one.
    #[props(default = true)]
    horizontal: bool,
    /// Whether the value runs from the end towards the start.
    #[props(default)]
    inverted: bool,
    /// Called with the new value whenever user interaction changes it.
    on_change: Option<EventHandler<f64>>,
    /// Called when pointer or arrow-key release ends the interaction unit.
    on_commit: Option<EventHandler<()>>,
    /// Called after focus leaves the complete slider root subtree.
    on_focus_exit: Option<EventHandler<()>>,
    /// What the thumb is named in the accessibility tree. A slider with no label
    /// is announced as "slider" and nothing else.
    #[props(default)]
    label: ReadSignal<Option<String>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let ResolvedSliderRoot {
        binding,
        value: resolved_value,
        disabled: resolved_disabled,
        first_thumb,
        second_thumb: _,
        mut pointer_change_pending,
        mut change_sequence,
        attributes: merged,
    } = use_slider_root(SliderRootOptions {
        color,
        size,
        appearance,
        binding,
        meta,
        value,
        default_value,
        required,
        disabled,
        name,
        on_commit,
        on_focus_exit,
        attributes,
    });

    rsx! {
        slider::Slider {
            value: resolved_value,
            default_value,
            min,
            max,
            step,
            disabled: resolved_disabled,
            horizontal,
            inverted,
            on_value_change: move |next: f64| {
                let current = resolved_value().expect("resolved slider value is always set");
                if next == current {
                    return;
                }
                pointer_change_pending.set(true);
                change_sequence += 1;
                binding.write(next, ChangeOrigin::User);
                if let Some(handler) = &on_change {
                    handler.call(next);
                }
                focus_slider_thumb(first_thumb);
            },
            label,
            attributes: merged,
            {children}
        }
    }
}

/// A slider with two thumbs, carrying the same daisyUI classes
/// [`SliderRoot`] does.
///
/// It shares every part: the groove, the fill and the handles are the same
/// components, and the fill spans between the two thumbs rather than from the
/// start. daisyUI has nothing for a two-thumb range (a native input cannot be
/// one) so this is the shape the primitive adds, wearing daisyUI's clothes.
#[component]
pub fn RangeSliderRoot(
    /// An explicit colour, or `None` to derive error colour from Field metadata.
    #[props(default)]
    color: Option<SliderColor>,
    /// daisyUI's size axis.
    #[props(default)]
    size: SliderSize,
    /// Whether to emit the utilities that lay the parts out.
    #[props(default)]
    appearance: SliderAppearance,
    /// An explicit Field binding, which wins over Field Context.
    binding: Option<Binding<Range<f64>>>,
    /// Explicit Field metadata, which wins over Field Context.
    meta: Option<FieldMeta>,
    /// The controlled value. `Some` makes the slider controlled.
    #[props(default)]
    value: ReadSignal<Option<Range<f64>>>,
    /// The span the slider starts at when it is not controlled. The default
    /// repeats the primitive's own.
    #[props(default = 0.0..100.0)]
    default_value: Range<f64>,
    /// What the slider is worth at its start. The default repeats the
    /// primitive's own.
    #[props(default = ReadSignal::new(Signal::new(0.0)))]
    min: ReadSignal<f64>,
    /// What it is worth at its end. The default repeats the primitive's own.
    #[props(default = ReadSignal::new(Signal::new(100.0)))]
    max: ReadSignal<f64>,
    /// How far one arrow key moves it. The default repeats the primitive's own.
    #[props(default = ReadSignal::new(Signal::new(1.0)))]
    step: ReadSignal<f64>,
    /// Whether the slider is required according to its producer.
    #[props(default)]
    required: Option<bool>,
    /// Whether the slider is disabled, which leaves it inert.
    #[props(default)]
    disabled: Option<bool>,
    /// The Field name override. The `div[role=group]` root omits native names.
    #[props(default)]
    name: Option<String>,
    /// Whether the slider runs across rather than down.
    #[props(default = true)]
    horizontal: bool,
    /// Whether the value runs from the end towards the start.
    #[props(default)]
    inverted: bool,
    /// Called with the new span whenever user interaction changes it.
    on_change: Option<EventHandler<Range<f64>>>,
    /// Called when pointer or arrow-key release ends the interaction unit.
    on_commit: Option<EventHandler<()>>,
    /// Called after focus leaves the complete range-slider root, including both thumbs.
    on_focus_exit: Option<EventHandler<()>>,
    /// What the thumbs are named in the accessibility tree.
    #[props(default)]
    label: ReadSignal<Option<String>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let ResolvedSliderRoot {
        binding,
        value: resolved_value,
        disabled: resolved_disabled,
        first_thumb,
        second_thumb,
        mut pointer_change_pending,
        mut change_sequence,
        attributes: merged,
    } = use_slider_root(SliderRootOptions {
        color,
        size,
        appearance,
        binding,
        meta,
        value,
        default_value: default_value.clone(),
        required,
        disabled,
        name,
        on_commit,
        on_focus_exit,
        attributes,
    });

    rsx! {
        slider::RangeSlider {
            value: resolved_value,
            default_value,
            min,
            max,
            step,
            disabled: resolved_disabled,
            horizontal,
            inverted,
            on_value_change: move |next: Range<f64>| {
                let current = resolved_value().expect("resolved range slider value is always set");
                if next == current {
                    return;
                }
                let active_thumb = if next.start != current.start {
                    first_thumb
                } else {
                    second_thumb
                };
                pointer_change_pending.set(true);
                change_sequence += 1;
                binding.write(next.clone(), ChangeOrigin::User);
                if let Some(handler) = &on_change {
                    handler.call(next);
                }
                focus_slider_thumb(active_thumb);
            },
            label,
            attributes: merged,
            {children}
        }
    }
}

/// The groove the thumb runs along, drawn with utilities because daisyUI's own
/// groove cannot be drawn here (ADR-0016).
///
/// It reads daisyUI's `--range-bg` and `--range-thumb-size`, so a colour or a
/// size set on the root reaches it without this component knowing what either
/// of them is worth.
///
/// It must stay inside a slider root: the custom properties it is drawn from are
/// inherited from that element, and the percentages its children are positioned
/// with are percentages of this one.
#[component]
pub fn SliderTrack(
    /// Whether to emit the utilities that draw the groove.
    #[props(default)]
    appearance: SliderTrackAppearance,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let appearance = appearance.class();
    let field = try_consume_context::<SliderFieldContext>();

    let base = attributes!(div {
        class: "{appearance}",
    });
    let field_attributes = field.map_or_else(Vec::new, |field| {
        let mut pointer_change_pending = field.pointer_change_pending;
        let mut keyboard_change_pending = field.keyboard_change_pending;
        attributes!(div {
            onpointerdown: move |_| {
                pointer_change_pending.set(false);
                keyboard_change_pending.set(false);
            },
            onkeydown: move |event: KeyboardEvent| {
                if is_slider_key(&event.key()) {
                    keyboard_change_pending
                        .set(*field.change_sequence.peek() != field.rendered_sequence.cloned());
                }
            },
        })
    });
    let merged = merge_attributes(vec![base, field_attributes, attributes]);

    rsx! {
        slider::SliderTrack { attributes: merged, {children} }
    }
}

/// The filled part of the groove, drawn with utilities for the reason
/// [`SliderTrack`] records.
///
/// The primitive gives it the inline percentages that span it: from the start to
/// the value on a one-thumb slider, and between the two thumbs on a range. It is
/// painted with `bg-current`, which is the colour daisyUI fills a native range
/// with by the same route.
#[component]
pub fn SliderRange(
    /// Whether to emit the utilities that draw the fill.
    #[props(default)]
    appearance: SliderRangeAppearance,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let appearance = appearance.class();

    let base = attributes!(div {
        class: "{appearance}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        slider::SliderRange { attributes: merged, {children} }
    }
}

/// The handle, drawn with utilities for the reason [`SliderTrack`] records.
///
/// The primitive makes it a `button` with `role="slider"`, which is what carries
/// the value into the accessibility tree and what the arrow keys, Home and End
/// act on. daisyUI's focus ring is emitted here rather than on the root, because
/// this is the element that takes focus.
#[component]
pub fn SliderThumb(
    /// Which thumb this is on a [`RangeSliderRoot`]: `0` is the start and `1` is
    /// the end. A one-thumb slider leaves it alone.
    #[props(default)]
    index: Option<usize>,
    /// Whether to emit the utilities that draw the handle.
    #[props(default)]
    appearance: SliderThumbAppearance,
    #[props(extends = GlobalAttributes)]
    #[props(extends = button)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let appearance = appearance.class();
    let field = try_consume_context::<SliderFieldContext>();

    let base = attributes!(button {
        class: "{appearance}",
    });
    let disabled_attributes = if field.is_some_and(|field| (field.disabled)()) {
        attributes!(button {
            aria_disabled: "true",
            tabindex: "-1",
        })
    } else {
        Vec::new()
    };
    let field_attributes = field.map_or_else(Vec::new, |field| {
        let mut thumb = if index.unwrap_or(0) == 0 {
            field.first_thumb
        } else {
            field.second_thumb
        };
        let mut pointer_change_pending = field.pointer_change_pending;
        let mut keyboard_change_pending = field.keyboard_change_pending;
        attributes!(button {
            onpointerdown: move |_| {
                pointer_change_pending.set(false);
                keyboard_change_pending.set(false);
            },
            onmounted: move |event: MountedEvent| thumb.set(Some(event.data())),
            onmousedown: move |event: MouseEvent| {
                event.prevent_default();
                focus_slider_thumb(thumb);
            },
            ontouchstart: move |event: TouchEvent| {
                event.prevent_default();
                focus_slider_thumb(thumb);
            },
            onkeyup: move |event: KeyboardEvent| {
                if is_slider_key(&event.key()) {
                    pointer_change_pending.set(false);
                    if keyboard_change_pending() {
                        keyboard_change_pending.set(false);
                        field.commit.call(());
                    }
                }
            },
        })
    });
    let merged = merge_attributes(vec![
        base,
        disabled_attributes,
        field_attributes,
        attributes,
    ]);

    rsx! {
        slider::SliderThumb { index, attributes: merged, {children} }
    }
}

// `GlobalAttributes` and `button` both define `autofocus`, so naming it on `SliderThumb` would be
// ambiguous (E0034, DioxusLabs/dioxus#5271). Inherent methods win method resolution and forward to
// the element's own attribute, as `button { .. }` resolves it. Unused, they would warn in an app
// that installs this Component. Remove once upstream resolves the conflict (ADR-0034).
#[allow(dead_code)]
impl<F> SliderThumbPropsBuilder<F>
where
    Self: dioxus_elements::extensions::ButtonExtension,
{
    /// The `button` element's `autofocus` attribute.
    pub fn autofocus(self, value: impl dioxus::dioxus_core::IntoAttributeValue) -> Self {
        dioxus_elements::extensions::ButtonExtension::autofocus(self, value)
    }
}

/// A slider, as one component rather than as [`SliderRoot`] wrapped around the
/// three parts.
///
/// The collapse is legal: daisyUI's own range is a single element whose groove,
/// fill and handle are pseudo-elements, so there is nothing between the parts
/// and no caller content can go there.
///
/// **Caller attributes land on the root**, which is the element a caller sizes
/// and positions, and the one daisyUI's own class is written for. Reaching a
/// part means dropping to them.
#[component]
pub fn Slider(
    /// An explicit colour, or `None` to derive error colour from Field metadata.
    #[props(default)]
    color: Option<SliderColor>,
    /// daisyUI's size axis.
    #[props(default)]
    size: SliderSize,
    /// Whether the root emits the utilities that lay the parts out.
    #[props(default)]
    appearance: SliderAppearance,
    /// An explicit Field binding, which wins over Field Context.
    binding: Option<Binding<f64>>,
    /// Explicit Field metadata, which wins over Field Context.
    meta: Option<FieldMeta>,
    /// The controlled value. `Some` makes the slider controlled.
    #[props(default)]
    value: ReadSignal<Option<f64>>,
    /// The value the slider starts at when it is not controlled.
    #[props(default)]
    default_value: f64,
    /// What the slider is worth at its start.
    #[props(default = ReadSignal::new(Signal::new(0.0)))]
    min: ReadSignal<f64>,
    /// What it is worth at its end.
    #[props(default = ReadSignal::new(Signal::new(100.0)))]
    max: ReadSignal<f64>,
    /// How far one arrow key moves it.
    #[props(default = ReadSignal::new(Signal::new(1.0)))]
    step: ReadSignal<f64>,
    /// Whether the slider is required according to its producer.
    #[props(default)]
    required: Option<bool>,
    /// Whether the slider is disabled.
    #[props(default)]
    disabled: Option<bool>,
    /// The Field name override. The `div[role=group]` root omits native names.
    #[props(default)]
    name: Option<String>,
    /// Whether the slider runs across rather than down.
    #[props(default = true)]
    horizontal: bool,
    /// Whether the value runs from the end towards the start.
    #[props(default)]
    inverted: bool,
    /// Called with the new value whenever user interaction changes it.
    on_change: Option<EventHandler<f64>>,
    /// Called when pointer or arrow-key release ends the interaction unit.
    on_commit: Option<EventHandler<()>>,
    /// Called after focus leaves the complete slider root subtree.
    on_focus_exit: Option<EventHandler<()>>,
    /// What the thumb is named in the accessibility tree.
    #[props(default)]
    label: ReadSignal<Option<String>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    rsx! {
        SliderRoot {
            color,
            size,
            appearance,
            binding,
            meta,
            value,
            default_value,
            min,
            max,
            step,
            required,
            disabled,
            name,
            horizontal,
            inverted,
            on_change,
            on_commit,
            on_focus_exit,
            label,
            attributes,
            SliderTrack {
                SliderRange {}
                SliderThumb {}
            }
        }
    }
}

/// The common Field composition for a one-thumb slider.
///
/// This Composition sugar intentionally has no children. Use [`Field`] and its
/// Compound parts when content or attributes must land between the parts. The
/// visible label is also passed to [`Slider`] so its thumb has an accessible
/// name. Caller attributes and classes are forwarded to the slider root.
#[component]
pub fn SliderField(
    /// The context supplied to the slider and every Field part.
    #[props(into)]
    context: FieldContext,
    /// The slider's visible label and the accessible name of its thumb.
    label: String,
    /// Supporting text rendered between the slider and its error region.
    #[props(default)]
    description: Option<String>,
    /// An explicit colour, or `None` to derive error colour from Field metadata.
    #[props(default)]
    color: Option<SliderColor>,
    /// daisyUI's size axis.
    #[props(default)]
    size: SliderSize,
    /// Whether the slider root emits the utilities that lay the parts out.
    #[props(default)]
    appearance: SliderAppearance,
    /// Whether the surrounding Field emits its default layout utilities.
    #[props(default)]
    field_appearance: FieldAppearance,
    /// Whether supporting text emits its default wrapping utilities.
    #[props(default)]
    description_appearance: FieldDescriptionAppearance,
    /// Whether the error region emits its default semantic colour.
    #[props(default)]
    error_appearance: FieldErrorAppearance,
    /// An explicit Field binding, which wins over `context` for the slider.
    binding: Option<Binding<f64>>,
    /// Explicit Field metadata, which wins over `context` for the slider.
    meta: Option<FieldMeta>,
    /// The controlled value. `Some` makes the slider controlled.
    #[props(default)]
    value: ReadSignal<Option<f64>>,
    /// The value the slider starts at when it is not controlled.
    #[props(default)]
    default_value: f64,
    /// What the slider is worth at its start.
    #[props(default = ReadSignal::new(Signal::new(0.0)))]
    min: ReadSignal<f64>,
    /// What the slider is worth at its end.
    #[props(default = ReadSignal::new(Signal::new(100.0)))]
    max: ReadSignal<f64>,
    /// How far one arrow key moves it.
    #[props(default = ReadSignal::new(Signal::new(1.0)))]
    step: ReadSignal<f64>,
    /// Whether the slider is required according to its producer.
    #[props(default)]
    required: Option<bool>,
    /// Whether the slider is disabled.
    #[props(default)]
    disabled: Option<bool>,
    /// The Field name override. The slider root omits native names.
    #[props(default)]
    name: Option<String>,
    /// Whether the slider runs across rather than down.
    #[props(default = true)]
    horizontal: bool,
    /// Whether the value runs from the end towards the start.
    #[props(default)]
    inverted: bool,
    /// Called with the new value whenever user interaction changes it.
    on_change: Option<EventHandler<f64>>,
    /// Called when pointer or arrow-key release ends the interaction unit.
    on_commit: Option<EventHandler<()>>,
    /// Called after focus leaves the complete slider root subtree.
    on_focus_exit: Option<EventHandler<()>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let field_label = label.clone();

    rsx! {
        Field { context, appearance: field_appearance,
            FieldLabel { {field_label} }
            Slider {
                label,
                color,
                size,
                appearance,
                binding,
                meta,
                value,
                default_value,
                min,
                max,
                step,
                required,
                disabled,
                name,
                horizontal,
                inverted,
                on_change,
                on_commit,
                on_focus_exit,
                attributes,
            }
            if let Some(description) = description {
                FieldDescription { appearance: description_appearance, {description} }
            }
            FieldError { appearance: error_appearance }
        }
    }
}

/// A two-thumb slider, as one component rather than as [`RangeSliderRoot`]
/// wrapped around the parts.
///
/// The collapse is legal for the reason [`Slider`]'s is, and the two thumbs are
/// what the parts would have been written out as: index `0` for the start of the
/// span and index `1` for its end.
#[component]
pub fn RangeSlider(
    /// An explicit colour, or `None` to derive error colour from Field metadata.
    #[props(default)]
    color: Option<SliderColor>,
    /// daisyUI's size axis.
    #[props(default)]
    size: SliderSize,
    /// Whether the root emits the utilities that lay the parts out.
    #[props(default)]
    appearance: SliderAppearance,
    /// An explicit Field binding, which wins over Field Context.
    binding: Option<Binding<Range<f64>>>,
    /// Explicit Field metadata, which wins over Field Context.
    meta: Option<FieldMeta>,
    /// The controlled span. `Some` makes the slider controlled.
    #[props(default)]
    value: ReadSignal<Option<Range<f64>>>,
    /// The span the slider starts at when it is not controlled.
    #[props(default = 0.0..100.0)]
    default_value: Range<f64>,
    /// What the slider is worth at its start.
    #[props(default = ReadSignal::new(Signal::new(0.0)))]
    min: ReadSignal<f64>,
    /// What it is worth at its end.
    #[props(default = ReadSignal::new(Signal::new(100.0)))]
    max: ReadSignal<f64>,
    /// How far one arrow key moves it.
    #[props(default = ReadSignal::new(Signal::new(1.0)))]
    step: ReadSignal<f64>,
    /// Whether the slider is required according to its producer.
    #[props(default)]
    required: Option<bool>,
    /// Whether the slider is disabled.
    #[props(default)]
    disabled: Option<bool>,
    /// The Field name override. The `div[role=group]` root omits native names.
    #[props(default)]
    name: Option<String>,
    /// Whether the slider runs across rather than down.
    #[props(default = true)]
    horizontal: bool,
    /// Whether the value runs from the end towards the start.
    #[props(default)]
    inverted: bool,
    /// Called with the new span whenever user interaction changes it.
    on_change: Option<EventHandler<Range<f64>>>,
    /// Called when pointer or arrow-key release ends the interaction unit.
    on_commit: Option<EventHandler<()>>,
    /// Called after focus leaves the complete range-slider root, including both thumbs.
    on_focus_exit: Option<EventHandler<()>>,
    /// What the thumbs are named in the accessibility tree.
    #[props(default)]
    label: ReadSignal<Option<String>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    rsx! {
        RangeSliderRoot {
            color,
            size,
            appearance,
            binding,
            meta,
            value,
            default_value,
            min,
            max,
            step,
            required,
            disabled,
            name,
            horizontal,
            inverted,
            on_change,
            on_commit,
            on_focus_exit,
            label,
            attributes,
            SliderTrack {
                SliderRange {}
                SliderThumb { index: 0usize }
                SliderThumb { index: 1usize }
            }
        }
    }
}

/// The common Field composition for a two-thumb slider.
///
/// This Composition sugar intentionally has no children. Use [`Field`] and its
/// Compound parts when content or attributes must land between the parts. The
/// visible label is also passed to [`RangeSlider`], whose Primitive applies the
/// same accessible name to both thumbs. Caller attributes and classes are
/// forwarded to the slider root.
#[component]
pub fn RangeSliderField(
    /// The context supplied to the range slider and every Field part.
    #[props(into)]
    context: FieldContext,
    /// The visible label and the shared accessible name of both thumbs.
    label: String,
    /// Supporting text rendered between the range slider and its error region.
    #[props(default)]
    description: Option<String>,
    /// An explicit colour, or `None` to derive error colour from Field metadata.
    #[props(default)]
    color: Option<SliderColor>,
    /// daisyUI's size axis.
    #[props(default)]
    size: SliderSize,
    /// Whether the slider root emits the utilities that lay the parts out.
    #[props(default)]
    appearance: SliderAppearance,
    /// Whether the surrounding Field emits its default layout utilities.
    #[props(default)]
    field_appearance: FieldAppearance,
    /// Whether supporting text emits its default wrapping utilities.
    #[props(default)]
    description_appearance: FieldDescriptionAppearance,
    /// Whether the error region emits its default semantic colour.
    #[props(default)]
    error_appearance: FieldErrorAppearance,
    /// An explicit Field binding, which wins over `context` for the range slider.
    binding: Option<Binding<Range<f64>>>,
    /// Explicit Field metadata, which wins over `context` for the range slider.
    meta: Option<FieldMeta>,
    /// The controlled span. `Some` makes the range slider controlled.
    #[props(default)]
    value: ReadSignal<Option<Range<f64>>>,
    /// The span the range slider starts at when it is not controlled.
    #[props(default = 0.0..100.0)]
    default_value: Range<f64>,
    /// What the range slider is worth at its start.
    #[props(default = ReadSignal::new(Signal::new(0.0)))]
    min: ReadSignal<f64>,
    /// What the range slider is worth at its end.
    #[props(default = ReadSignal::new(Signal::new(100.0)))]
    max: ReadSignal<f64>,
    /// How far one arrow key moves it.
    #[props(default = ReadSignal::new(Signal::new(1.0)))]
    step: ReadSignal<f64>,
    /// Whether the range slider is required according to its producer.
    #[props(default)]
    required: Option<bool>,
    /// Whether the range slider is disabled.
    #[props(default)]
    disabled: Option<bool>,
    /// The Field name override. The slider root omits native names.
    #[props(default)]
    name: Option<String>,
    /// Whether the range slider runs across rather than down.
    #[props(default = true)]
    horizontal: bool,
    /// Whether the value runs from the end towards the start.
    #[props(default)]
    inverted: bool,
    /// Called with the new span whenever user interaction changes it.
    on_change: Option<EventHandler<Range<f64>>>,
    /// Called when pointer or arrow-key release ends the interaction unit.
    on_commit: Option<EventHandler<()>>,
    /// Called after focus leaves the complete range-slider root, including both thumbs.
    on_focus_exit: Option<EventHandler<()>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let field_label = label.clone();

    rsx! {
        Field { context, appearance: field_appearance,
            FieldLabel { {field_label} }
            RangeSlider {
                label,
                color,
                size,
                appearance,
                binding,
                meta,
                value,
                default_value,
                min,
                max,
                step,
                required,
                disabled,
                name,
                horizontal,
                inverted,
                on_change,
                on_commit,
                on_focus_exit,
                attributes,
            }
            if let Some(description) = description {
                FieldDescription { appearance: description_appearance, {description} }
            }
            FieldError { appearance: error_appearance }
        }
    }
}
