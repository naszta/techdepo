//! Chat with tool calling: the [`Llm`] port used by the agent loop (`crates/agent`).
//!
//! The types are provider-neutral and serde-serializable, because each call runs in its own
//! `ctx.run` and its [`ChatResponse`] is journaled. On retry or replay the model is not called
//! again.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::ops::{Add, AddAssign};
use std::sync::Arc;

/// One chat completion request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// Model override; `None` uses the adapter's configured model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The conversation so far, usually starting with a [`ChatMessage::System`] prompt.
    pub messages: Vec<ChatMessage>,
    /// Tools the model may call. Empty means plain chat.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    /// Output token limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

/// A message in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum ChatMessage {
    /// Instructions.
    System {
        /// Prompt text.
        content: String,
    },
    /// The customer's text (or the transcript the agent works on).
    User {
        /// Message text.
        content: String,
    },
    /// A previous model turn: text, tool calls, or both.
    Assistant {
        /// Text, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        /// Tool calls the model made.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    /// The result of a tool call, fed back to the model. Errors are reported here too.
    Tool {
        /// [`ToolCall::id`] this answers.
        tool_call_id: String,
        /// Tool name.
        name: String,
        /// Result, usually JSON text.
        content: String,
    },
}

impl ChatMessage {
    /// A system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }

    /// A user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
        }
    }

    /// The assistant turn for `response`, to append before the tool results.
    pub fn assistant(response: &ChatResponse) -> Self {
        Self::Assistant {
            content: response.text.clone(),
            tool_calls: response.tool_calls.clone(),
        }
    }

    /// A tool result for `call`.
    pub fn tool_result(call: &ToolCall, content: impl Into<String>) -> Self {
        Self::Tool {
            tool_call_id: call.id.clone(),
            name: call.name.clone(),
            content: content.into(),
        }
    }
}

/// A tool the model can call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Name, e.g. `check_compatibility`.
    pub name: String,
    /// What the tool does and when to use it.
    pub description: String,
    /// JSON Schema of the arguments object (generated with schemars).
    pub parameters: serde_json::Value,
}

/// A tool call made by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider call ID (adapters synthesize `call-<n>` when the provider has none).
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments as JSON. Adapters parse string-encoded arguments; unparseable arguments arrive
    /// as a JSON string so the loop can report the error back to the model.
    pub arguments: serde_json::Value,
}

/// Why the model stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// Finished its answer.
    Stop,
    /// Wants tool results.
    ToolCalls,
    /// Hit the token limit.
    Length,
    /// Blocked by a content filter.
    ContentFilter,
    /// Anything else the provider reported.
    Other,
}

/// Token accounting for one or more calls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Prompt tokens.
    pub input_tokens: u32,
    /// Completion tokens.
    pub output_tokens: u32,
}

impl Usage {
    /// Input plus output tokens.
    pub const fn total(self) -> u32 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

impl Add for Usage {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self {
            input_tokens: self.input_tokens.saturating_add(rhs.input_tokens),
            output_tokens: self.output_tokens.saturating_add(rhs.output_tokens),
        }
    }
}

impl AddAssign for Usage {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

/// The model's answer. Plain text with no tool calls means the agent is done (`Reply`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    /// The model that answered.
    pub model: String,
    /// Text, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Tool calls, in the order the model made them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Tokens used.
    pub usage: Usage,
    /// Why it stopped.
    pub finish_reason: FinishReason,
}

/// A chat call failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LlmError {
    /// Network failure, timeout or 5xx. Retryable.
    #[error("model unavailable: {0}")]
    Unavailable(String),
    /// Throttled by the provider. Retryable.
    #[error("rate limited")]
    RateLimited {
        /// Provider hint, if any.
        retry_after_secs: Option<u32>,
    },
    /// The provider rejected the request (bad schema, context too long, …). Not retryable.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// The provider answered with something that could not be understood. Not retryable.
    #[error("invalid response: {0}")]
    InvalidResponse(String),
}

impl LlmError {
    /// Whether retrying the same call may succeed.
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_) | Self::RateLimited { .. })
    }
}

/// A chat model with tool calling.
///
/// Budget checks (`llm_usage`) and tracing (`agent_steps`) are the caller's job, not the
/// adapter's: the adapter only talks to the model.
#[async_trait]
pub trait Llm: Send + Sync {
    /// The model used when [`ChatRequest::model`] is `None` (recorded in traces).
    fn default_model(&self) -> &str;

    /// Runs one completion.
    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError>;
}

#[async_trait]
impl<T: Llm + ?Sized> Llm for Arc<T> {
    fn default_model(&self) -> &str {
        (**self).default_model()
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        (**self).chat(request).await
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn message_wire_shape() {
        let call = ToolCall {
            id: "call-0".into(),
            name: "get_product".into(),
            arguments: json!({ "id": "voltaris-v5-8600" }),
        };
        let msg = ChatMessage::tool_result(&call, "{}");
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            json!({ "role": "tool", "tool_call_id": "call-0", "name": "get_product", "content": "{}" })
        );
        let user = serde_json::to_value(ChatMessage::user("hi")).unwrap();
        assert_eq!(user, json!({ "role": "user", "content": "hi" }));
    }

    #[test]
    fn usage_adds_up() {
        let mut total = Usage::default();
        total += Usage {
            input_tokens: 10,
            output_tokens: 5,
        };
        total += Usage {
            input_tokens: 1,
            output_tokens: 1,
        };
        assert_eq!(total.total(), 17);
        assert!(
            LlmError::RateLimited {
                retry_after_secs: None
            }
            .is_retryable()
        );
        assert!(!LlmError::InvalidRequest("x".into()).is_retryable());
    }
}
