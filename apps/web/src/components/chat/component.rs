use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;

/// daisyUI's required logical placement Axis for a chat message.
///
/// Both values emit an explicit class so daisyUI can place the message and
/// orient the bubble tail.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum ChatPlacement {
    #[default]
    Start,
    End,
}

impl ChatPlacement {
    /// Every value of this Axis, in the order the Preview renders them.
    pub const ALL: &'static [Self] = &[Self::Start, Self::End];

    /// The daisyUI class name for this value, as a complete string literal so
    /// Tailwind's scanner can see it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Start => "chat-start",
            Self::End => "chat-end",
        }
    }
}

/// daisyUI's colour Axis for a [`ChatBubble`].
///
/// [`ChatBubbleColor::Default`] emits no modifier and keeps daisyUI's base
/// bubble colours rather than acting as a synonym for neutral.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum ChatBubbleColor {
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

impl ChatBubbleColor {
    /// Every value of this Axis, in the order the Preview renders them.
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
            Self::Neutral => "chat-bubble-neutral",
            Self::Primary => "chat-bubble-primary",
            Self::Secondary => "chat-bubble-secondary",
            Self::Accent => "chat-bubble-accent",
            Self::Info => "chat-bubble-info",
            Self::Success => "chat-bubble-success",
            Self::Warning => "chat-bubble-warning",
            Self::Error => "chat-bubble-error",
        }
    }
}

/// One chat message carrying daisyUI's `chat` and required placement classes.
///
/// Keep [`ChatImage`], [`ChatHeader`], [`ChatBubble`], and [`ChatFooter`] as
/// direct children so daisyUI can place them as grid items and draw the bubble
/// tail. Classes passed by the caller concatenate with the message's own;
/// every other caller attribute overrides the message's.
#[component]
pub fn Chat(
    /// daisyUI's placement axis, which is required: daisyUI has no unplaced
    /// chat message.
    placement: ChatPlacement,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let placement = placement.class();
    let base = attributes!(div {
        class: "chat {placement}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// A chat author's image, carrying daisyUI's `chat-image` class.
#[component]
pub fn ChatImage(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div {
        class: "chat-image"
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// Text above a chat bubble, carrying daisyUI's `chat-header` class.
#[component]
pub fn ChatHeader(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div {
        class: "chat-header",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// A message bubble carrying daisyUI's structural and colour classes.
#[component]
pub fn ChatBubble(
    /// daisyUI's colour axis.
    #[props(default)]
    color: ChatBubbleColor,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let color = color.class();
    let base = attributes!(div {
        class: "chat-bubble {color}",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}

/// Text below a chat bubble, carrying daisyUI's `chat-footer` class.
#[component]
pub fn ChatFooter(
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let base = attributes!(div {
        class: "chat-footer",
    });
    let merged = merge_attributes(vec![base, attributes]);

    rsx! {
        div { ..merged, {children} }
    }
}
