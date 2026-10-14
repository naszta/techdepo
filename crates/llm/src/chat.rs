//! [`Llm`] over any allama [`ChatProvider`]: Workers AI through the AI binding on Workers, any
//! provider `allama-providers` builds for native evals, allama's mocks in tests.

use allama::{ChatProvider, ContentBlock, Message};
use async_trait::async_trait;
use techdepo_ports::{
    ChatMessage, ChatRequest, ChatResponse, FinishReason, Llm, LlmError, ToolCall, Usage,
};

/// A chat model reached through allama.
///
/// Workers AI takes no `tool_choice`, so none is ever sent: the agent loop finishes on plain
/// text instead (support.md §5).
#[derive(Debug, Clone)]
pub struct AllamaLlm<P> {
    provider: P,
    model: String,
}

impl<P: ChatProvider> AllamaLlm<P> {
    /// `provider`, calling `model` unless a request names another.
    pub fn new(provider: P, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
        }
    }

    /// The provider.
    pub fn provider(&self) -> &P {
        &self.provider
    }
}

#[async_trait]
impl<P> Llm for AllamaLlm<P>
where
    P: ChatProvider + Send + Sync,
{
    fn default_model(&self) -> &str {
        &self.model
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let model = request.model.as_deref().unwrap_or(&self.model);
        let allama_request = to_allama(model, request);
        let response = self
            .provider
            .chat(&allama_request)
            .await
            .map_err(llm_error)?;
        Ok(from_allama(model, &response))
    }
}

/// Our request as allama's. System messages become system prompts (in order); tool calls are
/// replayed as tool-call blocks with JSON-encoded arguments.
pub(crate) fn to_allama(model: &str, request: &ChatRequest) -> allama::ChatRequest {
    let mut out = allama::ChatRequest::new(model);
    for message in &request.messages {
        match message {
            ChatMessage::System { content } => out = out.system(content.as_str()),
            ChatMessage::User { content } => out.push_message(Message::user(content.as_str())),
            ChatMessage::Assistant {
                content,
                tool_calls,
            } => {
                let mut blocks = Vec::with_capacity(tool_calls.len() + 1);
                if let Some(text) = content.as_deref().filter(|t| !t.is_empty()) {
                    blocks.push(ContentBlock::text(text));
                }
                blocks.extend(tool_calls.iter().map(|call| {
                    let arguments = match &call.arguments {
                        serde_json::Value::String(raw) => raw.clone(),
                        other => other.to_string(),
                    };
                    ContentBlock::from(allama::ToolCall::new(&call.id, &call.name, arguments))
                }));
                out.push_message(Message::assistant_blocks(blocks));
            }
            ChatMessage::Tool {
                tool_call_id,
                name,
                content,
            } => out.push_message(Message::tool_result(
                tool_call_id.as_str(),
                name.as_str(),
                content.as_str(),
            )),
        }
    }
    if !request.tools.is_empty() {
        out = out.tools(request.tools.iter().map(|tool| {
            allama::ToolDefinition::new(&tool.name, tool.parameters.clone())
                .description(&tool.description)
        }));
    }
    if let Some(max) = request.max_tokens {
        out = out.max_output_tokens(max);
    }
    if let Some(temperature) = request.temperature {
        out = out.temperature(f64::from(temperature));
    }
    out
}

/// allama's response as ours. String-encoded arguments are parsed; unparseable ones stay a
/// JSON string so the loop can report the error back to the model.
pub(crate) fn from_allama(model: &str, response: &allama::ChatResponse) -> ChatResponse {
    let tool_calls: Vec<ToolCall> = response
        .tool_calls()
        .map(|call| ToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: parse_arguments(&call.arguments),
        })
        .collect();
    let finish_reason = match &response.finish_reason {
        Some(allama::FinishReason::Stop) => FinishReason::Stop,
        Some(allama::FinishReason::ToolCalls) => FinishReason::ToolCalls,
        Some(allama::FinishReason::Length | allama::FinishReason::ContextWindow) => {
            FinishReason::Length
        }
        Some(allama::FinishReason::ContentFilter | allama::FinishReason::Refusal) => {
            FinishReason::ContentFilter
        }
        Some(_) => FinishReason::Other,
        None if !tool_calls.is_empty() => FinishReason::ToolCalls,
        None => FinishReason::Stop,
    };
    ChatResponse {
        model: response.model.clone().unwrap_or_else(|| model.to_owned()),
        text: response.text(),
        tool_calls,
        usage: response.usage.as_ref().map(usage).unwrap_or_default(),
        finish_reason,
    }
}

fn parse_arguments(raw: &str) -> serde_json::Value {
    if raw.trim().is_empty() {
        return serde_json::Value::Object(serde_json::Map::new());
    }
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()))
}

/// allama's usage as ours (saturating; missing counts are zero).
pub(crate) fn usage(usage: &allama::Usage) -> Usage {
    let clamp = |n: Option<u64>| n.map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
    Usage {
        input_tokens: clamp(usage.input_tokens),
        output_tokens: clamp(usage.output_tokens),
    }
}

/// allama's error as ours: what allama calls retryable stays retryable (Restate retries it).
pub(crate) fn llm_error(error: allama::Error) -> LlmError {
    if let allama::Error::RateLimited { .. } = &error {
        return LlmError::RateLimited {
            retry_after_secs: error
                .retry_after()
                .map(|d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX)),
        };
    }
    if error.is_retryable() {
        return LlmError::Unavailable(error.to_string());
    }
    match error {
        allama::Error::InvalidRequest { .. }
        | allama::Error::Unsupported { .. }
        | allama::Error::ContextLengthExceeded { .. }
        | allama::Error::ModelNotFound { .. }
        | allama::Error::Configuration { .. }
        | allama::Error::Auth { .. }
        | allama::Error::QuotaExhausted { .. }
        | allama::Error::Rejected { .. } => LlmError::InvalidRequest(error.to_string()),
        other => LlmError::InvalidResponse(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use allama::mock::MockProvider;
    use futures::executor::block_on;
    use serde_json::json;
    use techdepo_ports::ToolDefinition;

    use super::*;

    fn request() -> ChatRequest {
        let call = ToolCall {
            id: "c1".into(),
            name: "get_product".into(),
            arguments: json!({ "id": "kestrel-b650-aero" }),
        };
        ChatRequest {
            model: None,
            messages: vec![
                ChatMessage::system("You advise."),
                ChatMessage::user("Is the Kestrel B650 Aero any good?"),
                ChatMessage::Assistant {
                    content: None,
                    tool_calls: vec![call.clone()],
                },
                ChatMessage::tool_result(&call, r#"{"name":"Kestrel B650 Aero"}"#),
            ],
            tools: vec![ToolDefinition {
                name: "get_product".into(),
                description: "Specs of one product.".into(),
                parameters: json!({ "type": "object", "properties": { "id": { "type": "string" } } }),
            }],
            max_tokens: Some(512),
            temperature: Some(0.2),
        }
    }

    #[test]
    fn converts_requests() {
        let out = to_allama("@cf/meta/llama", &request());
        assert_eq!(out.model, "@cf/meta/llama");
        assert_eq!(out.system.len(), 1);
        assert_eq!(out.messages.len(), 3);
        let calls: Vec<_> = out.messages[1]
            .as_assistant()
            .unwrap()
            .tool_calls()
            .collect();
        assert_eq!(calls[0].arguments, r#"{"id":"kestrel-b650-aero"}"#);
        assert_eq!(out.messages[2].as_tool().unwrap().tool_call_id, "c1");
        assert_eq!(
            out.tools[0].description.as_deref(),
            Some("Specs of one product.")
        );
        assert_eq!(out.tool_choice, None, "Workers AI takes no tool_choice");
        assert_eq!(out.max_output_tokens, Some(512));
    }

    #[test]
    fn converts_responses_through_a_provider() {
        let provider = MockProvider::build(|b| {
            b.tool_calls([
                allama::ToolCall::new("c1", "search_products", r#"{"text":"Nimbus DDR4"}"#),
                allama::ToolCall::new("c2", "check_compatibility", "not json"),
            ])
            .text("Here you go.")
        });
        let llm = AllamaLlm::new(provider, "mock-model");
        let first = block_on(llm.chat(&request())).unwrap();
        assert_eq!(first.tool_calls.len(), 2);
        assert_eq!(
            first.tool_calls[0].arguments,
            json!({ "text": "Nimbus DDR4" })
        );
        assert_eq!(first.tool_calls[1].arguments, json!("not json"));
        assert_eq!(first.finish_reason, FinishReason::ToolCalls);

        let second = block_on(llm.chat(&request())).unwrap();
        assert_eq!(second.text.as_deref(), Some("Here you go."));
        assert!(second.tool_calls.is_empty());

        let exhausted = block_on(llm.chat(&request())).unwrap_err();
        assert!(!exhausted.is_retryable(), "{exhausted:?}");
    }
}
