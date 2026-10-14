//! The "Trace" tab: every agent run of the ticket (router, agents, judge, reply interpreter)
//! with its steps as a vertical timeline of collapsible rows (LLM calls, tool calls, classifier
//! passes, guardrails), each with duration, tokens and its JSON input and output.
//!
//! Loaded only when the tab opens. A ticket without AI involvement simply has no runs.

use dioxus::prelude::*;
use techdepo_api_types::endpoints::GetTicketTrace;
use techdepo_api_types::support::{AgentOutcome, AgentRunDetail};

use crate::api::use_api;
use crate::ui::explain::{ConfidenceBreakdown, TraceStep, prob};
use crate::ui::format::datetime;
use crate::ui::{
    ActorTag, AgentRunStatusBadge, EmptyState, ErrorAlert, Icon, IconName, Mono, PageSkeleton,
    RetryBadge, Tag,
};

#[component]
pub fn TicketTraceTab(id: String, runs: usize) -> Element {
    let api = use_api();
    // `runs` (from the ticket) changes when a new run lands, which re-reads the trace.
    let mut trace = use_resource(use_reactive!(|(id, runs)| async move {
        let _ = runs;
        api.get::<GetTicketTrace>(&[id.as_str()]).await
    }));
    match &*trace.read() {
        None => rsx! { PageSkeleton { rows: 2 } },
        Some(Err(error)) => {
            rsx! { ErrorAlert { error: error.clone(), on_retry: move |_| trace.restart() } }
        }
        Some(Ok(trace)) if trace.runs.is_empty() => rsx! {
            EmptyState {
                message: "No AI traces yet. They appear once the support pipeline classifies or answers this ticket.",
                icon: IconName::Activity,
            }
        },
        Some(Ok(trace)) => rsx! {
            ol { class: "flex flex-col gap-4",
                for run in trace.runs.iter().cloned() {
                    li { key: "{run.run.id}", RunCard { run } }
                }
            }
        },
    }
}

#[component]
fn RunCard(run: AgentRunDetail) -> Element {
    let r = run.run.clone();
    let elapsed = r
        .finished_at
        .map(|f| (f - r.started_at).whole_milliseconds());
    rsx! {
        section { class: "rounded-box border border-base-300 bg-base-100",
            header { class: "flex flex-wrap items-center gap-2 border-b border-base-300 px-4 py-3",
                ActorTag { actor: techdepo_api_types::Actor::Ai }
                span { class: "font-semibold", "{r.agent.label()}" }
                Mono { class: "text-xs text-base-content/60", "{r.model}" }
                AgentRunStatusBadge { status: r.status }
                if let Some(outcome) = r.outcome {
                    Tag { "→ {outcome.label()}" }
                }
                if r.attempt > 1 {
                    RetryBadge { attempt: r.attempt }
                }
                span { class: "flex-1" }
                span { class: "font-mono text-xs tabular-nums text-base-content/60",
                    "{r.step_count} steps · {r.input_tokens} / {r.output_tokens} tok"
                    if let Some(ms) = elapsed { " · {ms} ms" }
                    if let Some(c) = r.reply_confidence { " · confidence {prob(c)}" }
                }
            }
            div { class: "flex flex-col gap-4 p-4",
                p { class: "text-xs text-base-content/60",
                    "Started {datetime(r.started_at)} · "
                    Mono { "{r.id}" }
                }
                if let Some(error) = run.error.clone() {
                    div { role: "alert", class: "alert alert-error alert-soft py-2",
                        Icon { name: IconName::CircleX, class: "size-4" }
                        "{error}"
                    }
                }
                if let Some(outcome) = run.outcome.clone() {
                    OutcomeView { outcome }
                }
                if !run.guardrails.is_empty() {
                    div { class: "flex flex-col gap-1.5",
                        h4 { class: "text-xs font-medium uppercase tracking-wider text-base-content/50", "Guardrails" }
                        ul { class: "flex flex-col gap-1",
                            for g in run.guardrails.iter().cloned() {
                                li { key: "{g.guardrail.as_str()}", class: "flex flex-col gap-0.5",
                                    span { class: "inline-flex items-center gap-1.5",
                                        if g.passed {
                                            Icon { name: IconName::CircleCheck, class: "size-3.5 text-success", label: "Passed".to_string() }
                                        } else {
                                            Icon { name: IconName::CircleX, class: "size-3.5 text-error", label: "Failed".to_string() }
                                        }
                                        "{g.guardrail.label()}"
                                        span { class: "text-xs text-base-content/60", if g.passed { "passed" } else { "failed" } }
                                    }
                                    for (i , v) in g.violations.iter().enumerate() {
                                        span { key: "{i}", class: "pl-5 text-xs text-base-content/70", "· {v}" }
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(confidence) = run.confidence {
                    details { class: "rounded-field border border-base-300 p-3",
                        summary { class: "cursor-pointer font-medium", "Reply confidence breakdown" }
                        div { class: "mt-3 max-w-md", ConfidenceBreakdown { confidence } }
                    }
                }
                if let Some(prompt) = run.system_prompt.clone() {
                    details { class: "rounded-field border border-base-300 p-3",
                        summary { class: "cursor-pointer font-medium", "System prompt" }
                        pre { class: "mt-2 max-h-80 overflow-auto whitespace-pre-wrap font-mono text-xs leading-relaxed", "{prompt}" }
                    }
                }
                div { class: "flex flex-col gap-1.5",
                    h4 { class: "text-xs font-medium uppercase tracking-wider text-base-content/50", "Steps" }
                    if run.steps.is_empty() {
                        p { class: "text-base-content/60", "No steps recorded." }
                    }
                    ol { class: "ml-2 flex flex-col gap-2 border-l border-base-300 pl-4",
                        for step in run.steps.iter().cloned() {
                            li { key: "{step.idx}", class: "relative",
                                span { class: "absolute -left-[1.3rem] top-3 size-2 rounded-full bg-base-300" }
                                TraceStep { step }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn OutcomeView(outcome: AgentOutcome) -> Element {
    let (title, text) = match outcome {
        AgentOutcome::Reply { body } => ("Decided to reply", body),
        AgentOutcome::AskCustomer { question, expects } => (
            "Asked the customer",
            format!(
                "{question}\n\n(expects: {})",
                expects.label().to_lowercase()
            ),
        ),
        AgentOutcome::Escalate { reason } => ("Escalated to a person", reason),
        AgentOutcome::Reroute { intent } => ("Rerouted", format!("as {}", intent.label())),
    };
    rsx! {
        div { class: "flex flex-col gap-1",
            h4 { class: "text-xs font-medium uppercase tracking-wider text-base-content/50", "Outcome: {title}" }
            p { class: "whitespace-pre-wrap rounded-field border border-base-300 bg-base-200/50 p-3", "{text}" }
        }
    }
}
