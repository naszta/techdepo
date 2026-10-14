//! What a run did, step by step: the source of `agent_runs` / `agent_steps`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use techdepo_api_types::vocab::AgentStepKind;
use techdepo_ports::{ChatMessage, Usage};

/// One step: a model call or a tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceStep {
    /// Position in the run, from 0 (`agent_steps.idx` when the run is the trace's only part).
    pub idx: u32,
    /// `llm_call` or `tool_call`.
    pub kind: AgentStepKind,
    /// The model or the tool.
    pub name: String,
    /// The journaled step's name (`llm-<n>`), for model calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_name: Option<String>,
    /// Model calls: `{ digest, messages, new }` (a digest of the whole prompt, its length, and
    /// the messages added since the previous call). Tool calls: the arguments.
    pub input: Value,
    /// Model calls: the response. Tool calls: the result, or `{ "error": … }`.
    pub output: Value,
    /// The step failed (a tool error fed back to the model, or a failed model call).
    pub is_error: bool,
    /// Tokens (model calls).
    pub usage: Usage,
    /// Started, Unix milliseconds (live clock; for display only).
    pub started_at_ms: i64,
    /// Duration in milliseconds.
    pub duration_ms: u32,
}

/// A whole run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    /// The agent's name.
    pub agent: String,
    /// The model that answered (the first response's), or the configured one.
    pub model: String,
    /// The system prompt.
    pub system_prompt: String,
    /// Steps, in order.
    pub steps: Vec<TraceStep>,
    /// Tokens over every model call.
    pub usage: Usage,
    /// Model calls that answered (a call the budget refused, or one that failed after its
    /// retries, is not counted).
    pub llm_calls: u32,
    /// Why the run ended abnormally (model or tool failure, budget, step limit), if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Trace {
    pub(crate) fn push(&mut self, mut step: TraceStep) {
        step.idx = u32::try_from(self.steps.len()).unwrap_or(u32::MAX);
        self.usage += step.usage;
        self.steps.push(step);
    }

    /// The tool calls named `name`, with their arguments and results (e.g. the guardrail's
    /// "the trace has a `check_compatibility` call covering these parts").
    pub fn tool_calls<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a TraceStep> + 'a {
        self.steps
            .iter()
            .filter(move |s| s.kind == AgentStepKind::ToolCall && s.name == name)
    }
}

/// The input recorded for a model call: a stable digest of the prompt, its length, and what is
/// new since the previous call (so the trace holds every message once).
pub(crate) fn llm_input(messages: &[ChatMessage], seen: usize) -> Value {
    let encoded = serde_json::to_vec(messages).unwrap_or_default();
    serde_json::json!({
        "digest": format!("{:016x}", techdepo_domain::fnv1a64(&encoded)),
        "messages": messages.len(),
        "new": messages.get(seen..).unwrap_or_default(),
    })
}
