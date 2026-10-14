use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;
use dioxus_primitives::progress;

/// daisyUI's colour axis for a progress bar.
///
/// Every one of these classes sets nothing but `color`, which is what makes the
/// axis survive the split ADR-0012 records: the track is `currentColor` mixed
/// down to a fifth, and the fill this component draws is `currentColor` outright.
///
/// [`ProgressColor::Default`] emits no class, which leaves the bar at the
/// page's own text colour: daisyUI's uncoloured progress rather than a synonym
/// for [`ProgressColor::Neutral`].
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum ProgressColor {
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

impl ProgressColor {
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
            Self::Neutral => "progress-neutral",
            Self::Primary => "progress-primary",
            Self::Secondary => "progress-secondary",
            Self::Accent => "progress-accent",
            Self::Info => "progress-info",
            Self::Success => "progress-success",
            Self::Warning => "progress-warning",
            Self::Error => "progress-error",
        }
    }
}

/// Whether [`ProgressIndicator`] emits the utilities that draw the fill.
///
/// This is the axis ADR-0012 exists for. daisyUI draws a progress bar's filled
/// part in `::-webkit-progress-value` and `::-moz-progress-bar`, which are
/// pseudo-elements of a native `progress` element and of nothing else, so on
/// the `div` the primitive renders there is no daisyUI class that fills
/// anything, and the fill is Tailwind utilities instead.
///
/// That inverts the usual convention: [`ProgressIndicatorAppearance::Default`]
/// emits classes and [`ProgressIndicatorAppearance::None`] emits nothing. A
/// utility this component emits only ties with a caller's, and a tie is settled
/// by generated-stylesheet order rather than by the class attribute, so
/// switching ours off is the way to win it (ADR-0004).
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum ProgressIndicatorAppearance {
    #[default]
    Default,
    None,
}

impl ProgressIndicatorAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::None];

    /// The Tailwind utilities for this value, as complete string literals so
    /// Tailwind's scanner can see them.
    ///
    /// The width is the percentage the primitive works out and publishes as a
    /// custom property on the root, so the fill follows the value without this
    /// component doing the arithmetic a second time. The fallback is what an
    /// indeterminate bar gets, and it is `0%` rather than nothing: a `var()`
    /// that resolves to nothing leaves the width at `auto`, which on a block
    /// child is the whole track: a bar with no value at all, drawn full.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "block h-full w-[var(--progress-value,0%)] bg-current",
            Self::None => "",
        }
    }
}

/// The track of a progress bar, carrying daisyUI's `progress` classes.
///
/// There is no state to bridge here: a progress bar is never open, checked or
/// selected, and its value is a number rather than a state daisyUI has a class
/// for. What this component does instead is split daisyUI's own class: the
/// track applies to any element, the fill does not, and ADR-0012 records why
/// the fill is drawn by [`ProgressIndicator`] with utilities.
///
/// The primitive puts the value in the accessibility tree as `aria-valuenow`
/// against `aria-valuemin` and `aria-valuemax`, and publishes it as a
/// `--progress-value` percentage for the fill to be sized from. A bar with no
/// value is indeterminate: it announces itself as such, and daisyUI's own
/// indeterminate styling cannot reach it; see the component's documentation.
///
/// Classes passed by the caller concatenate with the track's own; every other
/// attribute the caller passes overrides them.
#[component]
pub fn ProgressRoot(
    /// daisyUI's colour axis.
    #[props(default)]
    color: ProgressColor,
    /// How far along the bar is, out of `max`. `None` is a bar that is running
    /// but cannot say how far along it is.
    value: ReadSignal<Option<f64>>,
    /// What a full bar is worth. The default repeats the primitive's own, since
    /// a prop declared here has to carry one.
    #[props(default = ReadSignal::new(Signal::new(100.0)))]
    max: ReadSignal<f64>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let color = color.class();

    let base = attributes!(div {
        class: "progress {color}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        progress::Progress { value, max, attributes: merged, {children} }
    }
}

/// The filled part of a progress bar, drawn with utilities because daisyUI's
/// own fill cannot be drawn here (ADR-0012).
///
/// It is sized from the `--progress-value` percentage the primitive sets on the
/// track and painted with `currentColor`, which is the one thing daisyUI's
/// colour classes do set, so the colour axis reaches the fill through the same
/// property daisyUI uses to reach the native one.
///
/// It must stay inside [`ProgressRoot`]: the custom property it is sized from
/// is inherited from that element, and an indicator outside one has no value to
/// draw.
#[component]
pub fn ProgressIndicator(
    /// Whether to emit the utilities that draw the fill.
    #[props(default)]
    appearance: ProgressIndicatorAppearance,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let appearance = appearance.class();

    let base = attributes!(div {
        class: "{appearance}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        progress::ProgressIndicator { attributes: merged, {children} }
    }
}

/// A progress bar, as one component rather than as [`ProgressRoot`] wrapped
/// around [`ProgressIndicator`].
///
/// The collapse is legal because daisyUI's progress has nothing between the two
/// and no caller content can go there: its markup is a single element whose
/// fill is drawn by the browser.
///
/// **Caller attributes land on the track**, which is the opposite of where the
/// dialog puts them and is the right way round here: the track is the element a
/// caller sizes, positions and rounds, where the fill's width *is* the value and
/// is not the caller's to set. Reaching the fill means dropping to the parts,
/// or switching its utilities off and writing your own (ADR-0012).
#[component]
pub fn Progress(
    /// daisyUI's colour axis.
    #[props(default)]
    color: ProgressColor,
    /// Whether the fill emits the utilities that draw it.
    #[props(default)]
    appearance: ProgressIndicatorAppearance,
    /// How far along the bar is, out of `max`. `None` is a bar that is running
    /// but cannot say how far along it is.
    value: ReadSignal<Option<f64>>,
    /// What a full bar is worth. The default repeats the primitive's own.
    #[props(default = ReadSignal::new(Signal::new(100.0)))]
    max: ReadSignal<f64>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    rsx! {
        ProgressRoot { color, value, max, attributes,
            ProgressIndicator { appearance }
        }
    }
}
