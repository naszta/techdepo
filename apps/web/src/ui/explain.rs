//! Explainability as designed screens, not debug dumps (design.md, principle 6): probability
//! bars, the reply-confidence breakdown, agent trace steps and pretty-printed JSON.

use dioxus::prelude::*;
use techdepo_api_types::support::{AgentStep, Classification, ReplyConfidence};
use techdepo_api_types::vocab::AgentStepKind;
use techdepo_domain::{ReplyDisposition, ReplyPolicy};

use crate::components::badge::{Badge, BadgeAppearance, BadgeColor, BadgeSize};
use crate::ui::format::datetime;
use crate::ui::icons::{Icon, IconName};

/// One bar of [`ProbabilityBars`].
#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    /// What the probability is of.
    pub label: String,
    /// `0.0..=1.0`.
    pub p: f64,
}

impl Bar {
    /// A bar.
    pub fn new(label: impl Into<String>, p: f64) -> Self {
        Self {
            label: label.into(),
            p,
        }
    }
}

/// `0.82`: probabilities are shown with two decimals everywhere, like the settings sliders.
pub fn prob(p: f64) -> String {
    format!("{:.2}", p.clamp(0.0, 1.0))
}

/// Small horizontal bars, one per value, with an optional threshold marker. The first bar is
/// emphasised when `top` is set (the winning intent).
#[component]
pub fn ProbabilityBars(
    bars: Vec<Bar>,
    #[props(default)] threshold: Option<f64>,
    #[props(default)] top: bool,
) -> Element {
    rsx! {
        ul { class: "flex flex-col gap-1.5",
            for (i , bar) in bars.into_iter().enumerate() {
                li { key: "{i}", class: "grid grid-cols-[minmax(0,9rem)_1fr_2.5rem] items-center gap-2",
                    span {
                        class: "truncate text-xs",
                        class: if top && i == 0 { "font-semibold" } else { "text-base-content/70" },
                        title: "{bar.label}",
                        "{bar.label}"
                    }
                    div { class: "relative",
                        progress {
                            class: "progress h-1.5 w-full",
                            class: if top && i == 0 { "progress-primary" } else { "text-base-content/40" },
                            value: "{bar.p.clamp(0.0, 1.0) * 100.0}",
                            max: "100",
                            aria_label: "{bar.label}",
                        }
                        if let Some(t) = threshold {
                            span {
                                class: "absolute -top-0.5 -bottom-0.5 w-px bg-base-content/60",
                                style: "left: {t.clamp(0.0, 1.0) * 100.0}%",
                                title: "Threshold {prob(t)}",
                            }
                        }
                    }
                    span { class: "text-right font-mono text-xs tabular-nums", "{prob(bar.p)}" }
                }
            }
        }
    }
}

/// The router's classification: every intent's probability against the org threshold, the
/// side questions, and why it escalated.
#[component]
pub fn ClassificationView(classification: Classification) -> Element {
    let c = classification;
    let bars: Vec<Bar> = c
        .probabilities
        .iter()
        .map(|p| Bar::new(p.intent.label(), p.p))
        .collect();
    let mut side = vec![
        Bar::new("Contains a request", c.contains_request),
        Bar::new("Wants a person", c.wants_human),
        Bar::new("Legal threat", c.legal_threat),
    ];
    if let Some(p) = c.answers_our_question {
        side.push(Bar::new("Answers our question", p));
    }
    rsx! {
        div { class: "flex flex-col gap-3",
            ProbabilityBars { bars, threshold: c.threshold, top: true }
            p { class: "text-xs text-base-content/60",
                "Threshold {prob(c.threshold)} · margin {prob(c.margin)} · "
                span { class: "font-mono", "{c.model}" }
            }
            if c.escalation.is_empty() {
                p { class: "flex items-center gap-1.5 text-xs",
                    Icon { name: IconName::Route, class: "size-3.5 text-success" }
                    "Confident enough to route automatically."
                }
            } else {
                div { class: "flex flex-wrap items-center gap-1.5 text-xs",
                    Icon { name: IconName::Hand, class: "size-3.5 text-warning" }
                    "Escalated:"
                    for cause in c.escalation.iter() {
                        Badge { key: "{cause.as_str()}", color: BadgeColor::Warning, appearance: BadgeAppearance::Soft, size: BadgeSize::Sm, "{cause.label()}" }
                    }
                }
            }
            details { class: "text-xs",
                summary { class: "cursor-pointer text-base-content/60 hover:text-base-content", "Side questions" }
                div { class: "mt-2", ProbabilityBars { bars: side } }
            }
            if !c.order_numbers.is_empty() {
                p { class: "text-xs text-base-content/60",
                    "Order numbers found: "
                    span { class: "font-mono", {c.order_numbers.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")} }
                }
            }
        }
    }
}

/// The policy's name in the settings' words.
pub const fn reply_policy_label(policy: ReplyPolicy) -> &'static str {
    match policy {
        ReplyPolicy::AlwaysSend => "Always send",
        ReplyPolicy::AlwaysReview => "Always review",
        ReplyPolicy::SendAboveConfidence => "Send above confidence",
    }
}

/// How a reply confidence was computed: `min(p(intent), judge)` and what the policy did.
#[component]
pub fn ConfidenceBreakdown(confidence: ReplyConfidence) -> Element {
    let c = confidence;
    let bars = vec![
        Bar::new("Intent probability", c.intent_p),
        Bar::new("Answers the question", c.judge.answers_question),
        Bar::new("Supported by tools", c.judge.supported_by_tools),
        Bar::new("Tone appropriate", c.judge.tone_appropriate),
    ];
    let threshold = (c.policy == ReplyPolicy::SendAboveConfidence).then_some(c.threshold);
    let (color, icon, outcome) = match c.disposition {
        ReplyDisposition::Send => (BadgeColor::Success, IconName::Send, "Sent automatically"),
        ReplyDisposition::Review => (BadgeColor::Warning, IconName::Hand, "Held for review"),
    };
    rsx! {
        div { class: "flex flex-col gap-3",
            ProbabilityBars { bars, threshold }
            div { class: "flex flex-wrap items-center justify-between gap-2 border-t border-base-300 pt-2",
                div { class: "flex items-baseline gap-2",
                    span { class: "text-xs text-base-content/60", "Confidence = lowest" }
                    span { class: "font-mono text-base font-semibold tabular-nums", "{prob(c.confidence)}" }
                    if let Some(t) = threshold {
                        span { class: "text-xs text-base-content/60", "vs threshold {prob(t)}" }
                    }
                }
                Badge { color, appearance: BadgeAppearance::Soft, size: BadgeSize::Sm, class: "gap-1",
                    Icon { name: icon, class: "size-3" }
                    "{outcome}"
                }
            }
            p { class: "text-xs text-base-content/60", "Policy at the time: {reply_policy_label(c.policy)}." }
        }
    }
}

/// Pretty-printed JSON in mono, scrollable.
#[component]
pub fn JsonBlock(value: serde_json::Value, #[props(default)] label: Option<String>) -> Element {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    rsx! {
        div { class: "flex min-w-0 flex-col gap-1",
            if let Some(label) = label {
                span { class: "text-xs font-medium uppercase tracking-wider text-base-content/50", "{label}" }
            }
            pre { class: "max-h-96 overflow-auto rounded-field border border-base-300 bg-base-200 p-3 font-mono text-xs leading-relaxed whitespace-pre",
                "{text}"
            }
        }
    }
}

const fn step_icon(kind: AgentStepKind) -> IconName {
    match kind {
        AgentStepKind::LlmCall => IconName::Sparkles,
        AgentStepKind::ToolCall => IconName::Wrench,
        AgentStepKind::Classification => IconName::Activity,
        AgentStepKind::Guardrail => IconName::ShieldCheck,
    }
}

/// One step of an agent trace: a collapsible row with duration and tokens, the JSON input and
/// output inside.
#[component]
pub fn TraceStep(step: AgentStep) -> Element {
    let tokens = step.input_tokens + step.output_tokens;
    rsx! {
        details { class: "group rounded-field border border-base-300 bg-base-100 open:bg-base-200/40",
            summary { class: "flex cursor-pointer list-none items-center gap-2 px-3 py-2 [&::-webkit-details-marker]:hidden",
                Icon { name: IconName::ChevronRight, class: "size-3.5 text-base-content/50 transition-transform group-open:rotate-90 motion-reduce:transition-none" }
                span { class: "w-6 text-right font-mono text-xs text-base-content/50", "{step.idx}" }
                span {
                    class: "grid size-6 place-items-center rounded-full border border-base-300 bg-base-100",
                    class: if step.is_error { "text-error" } else { "text-base-content/70" },
                    Icon { name: step_icon(step.kind), class: "size-3.5" }
                }
                span { class: "text-xs text-base-content/60", "{step.kind.label()}" }
                span { class: "min-w-0 flex-1 truncate font-mono text-xs font-medium", "{step.name}" }
                if step.is_error {
                    Badge { color: BadgeColor::Error, appearance: BadgeAppearance::Soft, size: BadgeSize::Sm, class: "gap-1",
                        Icon { name: IconName::CircleX, class: "size-3" }
                        "Error"
                    }
                }
                if tokens > 0 {
                    span { class: "hidden font-mono text-xs tabular-nums text-base-content/60 sm:inline",
                        title: "Prompt / completion tokens",
                        "{step.input_tokens} / {step.output_tokens} tok"
                    }
                }
                span { class: "w-16 text-right font-mono text-xs tabular-nums text-base-content/60", "{step.duration_ms} ms" }
            }
            div { class: "flex flex-col gap-3 border-t border-base-300 p-3",
                p { class: "text-xs text-base-content/60",
                    "Started {datetime(step.started_at)}"
                    if let Some(journal) = step.journal_name.clone() {
                        " · journal entry "
                        span { class: "font-mono", "{journal}" }
                    }
                }
                div { class: "grid gap-3 xl:grid-cols-2",
                    match step.input.clone() {
                        Some(value) => rsx! { JsonBlock { value, label: "Input" } },
                        None => rsx! { p { class: "text-xs text-base-content/60", "No input recorded." } },
                    }
                    match step.output.clone() {
                        Some(value) => rsx! { JsonBlock { value, label: "Output" } },
                        None => rsx! { p { class: "text-xs text-base-content/60", "No output recorded." } },
                    }
                }
            }
        }
    }
}
