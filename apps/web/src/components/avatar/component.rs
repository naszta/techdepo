use dioxus::prelude::*;
use dioxus_primitives::avatar;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;

/// The primitive's own state, re-exported so that a caller needs one import.
///
/// It is what the change callback reports, and it is the state this component
/// mirrors to decide whether daisyUI's placeholder class belongs on the root.
pub use dioxus_primitives::avatar::AvatarState;

/// daisyUI's status axis, which is the dot drawn in the corner of an avatar.
///
/// [`AvatarStatus::Default`] emits no class and draws no dot, which is an
/// avatar that says nothing about whether the person behind it is around.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum AvatarStatus {
    #[default]
    Default,
    Online,
    Offline,
}

impl AvatarStatus {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::Online, Self::Offline];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Online => "avatar-online",
            Self::Offline => "avatar-offline",
        }
    }
}

/// Whether [`AvatarFrame`] emits the utilities that give it a size and a shape.
///
/// daisyUI has no class for either: its own examples write `w-24` and
/// `rounded-full` on this element, and the only thing `.avatar > div` itself
/// sets is the square aspect ratio and the clipping. The pair is emitted here
/// so that an avatar looks like one on install.
///
/// That inverts the usual convention: [`AvatarFrameAppearance::Default`] emits
/// classes and [`AvatarFrameAppearance::None`] emits nothing. A utility this
/// component emits only ties with a caller's, and a tie is settled by
/// generated-stylesheet order rather than by the class attribute, so a caller
/// who wants a *smaller* avatar switches ours off rather than trying to
/// out-rank it (ADR-0004).
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum AvatarFrameAppearance {
    #[default]
    Default,
    None,
}

impl AvatarFrameAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::None];

    /// The Tailwind utilities for this value, as complete string literals so
    /// Tailwind's scanner can see them.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => "w-16 rounded-full",
            Self::None => "",
        }
    }
}

/// Whether [`AvatarFallback`] emits the utilities that paint the placeholder.
///
/// daisyUI's `avatar-placeholder` centres what is inside the frame and paints
/// nothing; the fill and the text colour in its own examples are utilities on
/// the frame. They are emitted on the fallback instead, the one element that
/// is only ever there when there is no image, so that a frame carrying a
/// loaded image is not painted behind it.
///
/// The inverse convention of the axes above, for the reason
/// [`AvatarFrameAppearance`] records.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum AvatarFallbackAppearance {
    #[default]
    Default,
    None,
}

impl AvatarFallbackAppearance {
    /// Every value of this axis, in the order the preview renders them.
    pub const ALL: &'static [Self] = &[Self::Default, Self::None];

    /// The Tailwind utilities for this value, as complete string literals so
    /// Tailwind's scanner can see them.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Default => {
                "flex size-full items-center justify-center bg-neutral text-neutral-content"
            }
            Self::None => "",
        }
    }
}

/// The outer element of an avatar, carrying daisyUI's `avatar` classes.
///
/// This is where the placeholder state is bridged, and it is **Tier 2**:
/// daisyUI centres a placeholder's content through `avatar-placeholder` on this
/// element and matches nothing the primitive sets: not the `data-state` it
/// puts here, and not the image's own load events.
///
/// The state is **mirrored rather than lifted** (ADR-0011), and for a plainer
/// reason than the accordion item's: there is no state to lift. Whether an
/// avatar has an image is decided by the browser (a `load` or an `error` on
/// the image element), and the primitive offers no controlled prop for it,
/// because there is nothing a caller could control it with. So this component
/// seeds a signal with the state the primitive starts in and updates it from the
/// change callback the primitive already fires.
///
/// Classes passed by the caller concatenate with the root's own; every other
/// attribute the caller passes overrides them.
#[component]
pub fn AvatarRoot(
    /// daisyUI's status axis.
    #[props(default)]
    status: AvatarStatus,
    /// Called when the image loads.
    #[props(default)]
    on_load: Option<EventHandler<()>>,
    /// Called when the image fails to load.
    #[props(default)]
    on_error: Option<EventHandler<()>>,
    /// Called when the avatar's state changes, whichever way it changed.
    #[props(default)]
    on_state_change: Option<EventHandler<AvatarState>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    // Seeded with the state the primitive itself starts in, so that an avatar
    // that never loads anything is a placeholder on the first render rather
    // than one callback later, and so that one whose image loads has the class
    // taken off it rather than never put on.
    let mut state = use_signal(|| AvatarState::Empty);

    let status = status.class();
    // Tier 2, and the whole reason the state is mirrored: daisyUI's own
    // modifier class, emitted from Rust as a complete literal so that
    // Tailwind's scanner sees it too.
    let placeholder = match state() {
        AvatarState::Error | AvatarState::Empty => "avatar-placeholder",
        AvatarState::Loading | AvatarState::Loaded => "",
    };

    let base = attributes!(span {
        class: "avatar {status} {placeholder}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        avatar::Avatar {
            on_load,
            on_error,
            on_state_change: move |next: AvatarState| {
                state.set(next);
                if let Some(handler) = &on_state_change {
                    handler.call(next);
                }
            },
            attributes: merged,
            {children}
        }
    }
}

/// The element daisyUI's avatar is actually drawn on, which the primitive does
/// not render (ADR-0013).
///
/// Every rule that gives an avatar its shape is written `.avatar > div`: the
/// square aspect ratio and the clipping are daisyUI's, and the size and the
/// corner radius are utilities in its own examples, so an image dropped
/// straight into the primitive's root would be styled by none of them. This
/// element is that `div`, rendered here rather than asked of the caller.
///
/// It must stay a direct child of [`AvatarRoot`], which is what the selector
/// says, and the image and the fallback go inside it.
#[component]
pub fn AvatarFrame(
    /// Whether to emit the utilities that give the frame a size and a shape.
    #[props(default)]
    appearance: AvatarFrameAppearance,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let appearance = appearance.class();

    let base = attributes!(div {
        class: "{appearance}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// The image an avatar shows while it has one.
///
/// Nothing is emitted here, and nothing needs to be: daisyUI's `.avatar img`
/// sizes the image to the frame and crops it to fill, and the primitive writes
/// the same three declarations inline. The primitive also unmounts this element
/// when the image fails, which is what reveals the fallback.
#[component]
pub fn AvatarImage(
    /// Where the image is loaded from. An empty source is an avatar with no
    /// image rather than a broken one.
    src: String,
    /// What the image is announced as. An avatar whose alt text is empty is
    /// announced by the name on the root instead.
    #[props(default)]
    alt: Option<String>,
    /// The id of this element. Declared rather than left to the attribute list,
    /// because the primitive generates one and then looks the element up by it
    /// to catch an image that was already in the cache; an id arriving as an
    /// attribute would replace the one it is looking for.
    #[props(default)]
    id: ReadSignal<Option<String>>,
    #[props(extends = GlobalAttributes)]
    #[props(extends = img)]
    attributes: Vec<Attribute>,
) -> Element {
    rsx! {
        avatar::AvatarImage { src, alt, id, attributes }
    }
}

// `GlobalAttributes` and `img` both define `height` and `width`, so naming one on `AvatarImage`
// would be ambiguous (E0034, DioxusLabs/dioxus#5271). Inherent methods win method resolution and
// forward to the element's own attribute, as `img { .. }` resolves it. Unused, they would warn in
// an app that installs this Component. Remove once upstream resolves the conflict (ADR-0034).
#[allow(dead_code)]
impl<F> AvatarImagePropsBuilder<F>
where
    Self: dioxus_elements::extensions::ImgExtension,
{
    /// The `img` element's `height` attribute.
    pub fn height(self, value: impl dioxus::dioxus_core::IntoAttributeValue) -> Self {
        dioxus_elements::extensions::ImgExtension::height(self, value)
    }

    /// The `img` element's `width` attribute.
    pub fn width(self, value: impl dioxus::dioxus_core::IntoAttributeValue) -> Self {
        dioxus_elements::extensions::ImgExtension::width(self, value)
    }
}

/// What an avatar shows when it has no image: initials, an icon, whatever the
/// caller writes.
///
/// The primitive renders this only while the avatar is empty or has failed, so
/// it is not hidden behind a loaded image; it is not there at all. The
/// utilities it emits fill the frame and paint it, which is what daisyUI's own
/// placeholder examples put on the frame; they are on this element instead so
/// that a frame with an image in it is left unpainted.
#[component]
pub fn AvatarFallback(
    /// Whether to emit the utilities that paint the placeholder.
    #[props(default)]
    appearance: AvatarFallbackAppearance,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let appearance = appearance.class();

    let base = attributes!(span {
        class: "{appearance}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        avatar::AvatarFallback { attributes: merged, {children} }
    }
}

/// An avatar, as one component rather than as [`AvatarRoot`] wrapped around
/// [`AvatarFrame`].
///
/// The collapse is legal because daisyUI's avatar has nothing between those two
/// elements and no caller content can go there: its markup is the avatar, the
/// frame, and whatever is inside the frame.
///
/// **Caller attributes land on the frame**, which is the element worth
/// reaching: the size, the corner radius and any border are all written there,
/// on both sides of daisyUI's `.avatar > div`. The root is reached through
/// `status`, or by dropping to the parts.
///
/// One attribute is routed back out to the root rather than left on the frame:
/// the primitive gives the root `role="img"`, and an image's name is the
/// author's to give; content inside it is not used for one. So `label` is a
/// prop of its own, the way [`DialogTitle`](crate::components::dialog)'s id is,
/// and an avatar that needs `aria-labelledby` instead drops to the parts.
#[component]
pub fn Avatar(
    /// What the avatar is announced as, which lands on the root.
    #[props(default)]
    label: Option<String>,
    /// daisyUI's status axis, which styles the outer element.
    #[props(default)]
    status: AvatarStatus,
    /// Whether the frame emits the utilities that size and shape it.
    #[props(default)]
    appearance: AvatarFrameAppearance,
    /// Called when the image loads.
    #[props(default)]
    on_load: Option<EventHandler<()>>,
    /// Called when the image fails to load.
    #[props(default)]
    on_error: Option<EventHandler<()>>,
    /// Called when the avatar's state changes.
    #[props(default)]
    on_state_change: Option<EventHandler<AvatarState>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        AvatarRoot {
            aria_label: label,
            status,
            on_load,
            on_error,
            on_state_change,
            AvatarFrame { appearance, attributes, {children} }
        }
    }
}
