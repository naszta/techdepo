//! The loop.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use techdepo_api_types::vocab::AgentStepKind;
use techdepo_ports::{ChatMessage, ChatRequest, ChatResponse, FinishReason, Llm, ToolCall, Usage};

use crate::Outcome;
use crate::budget::{Budget, BudgetCheck, Unlimited};
use crate::runtime::{Runtime, StepError, now_ms};
use crate::terminal::{self, is_terminal, terminal_definitions};
use crate::tool::{ToolError, ToolSet};
use crate::trace::{Trace, TraceStep, llm_input};

/// Model calls per run unless the agent says otherwise (support.md §5).
pub const DEFAULT_MAX_STEPS: u32 = 6;

/// An agent: who it is and how it may run. Its tools are passed to [`run`] separately, because
/// they are bound to the caller's per-run context.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentDef {
    /// Name, e.g. `product_advisor` (the trace's agent).
    pub name: String,
    /// The system prompt.
    pub system_prompt: String,
    /// Model override; `None` uses the [`Llm`]'s default.
    pub model: Option<String>,
    /// Model calls before the run gives up and escalates.
    pub max_steps: u32,
    /// Output token limit per call.
    pub max_output_tokens: Option<u32>,
    /// Sampling temperature.
    pub temperature: Option<f32>,
}

impl AgentDef {
    /// An agent with the default step limit and the model's defaults.
    pub fn new(name: impl Into<String>, system_prompt: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            system_prompt: system_prompt.into(),
            model: None,
            max_steps: DEFAULT_MAX_STEPS,
            max_output_tokens: None,
            temperature: None,
        }
    }
}

/// What the loop calls out to: the model and the budget.
#[derive(Clone)]
pub struct AgentEnv {
    /// The chat model.
    pub llm: Arc<dyn Llm>,
    /// Checked before, and fed after, every model call.
    pub budget: Arc<dyn Budget>,
}

impl AgentEnv {
    /// `llm` with no budget.
    pub fn new(llm: Arc<dyn Llm>) -> Self {
        Self {
            llm,
            budget: Arc::new(Unlimited),
        }
    }

    /// Enforces `budget`.
    #[must_use]
    pub fn with_budget(mut self, budget: Arc<dyn Budget>) -> Self {
        self.budget = budget;
        self
    }
}

impl std::fmt::Debug for AgentEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentEnv")
            .field("model", &self.llm.default_model())
            .finish_non_exhaustive()
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentRun {
    /// What to do next.
    pub outcome: Outcome,
    /// What happened.
    pub trace: Trace,
    /// The conversation, system prompt first, up to and including the last model turn (and the
    /// terminal call's acknowledgement). To give the agent feedback (a guardrail violation),
    /// append a user message and pass it to [`run`] again.
    pub messages: Vec<ChatMessage>,
}

/// The journaled result of one model call: the budget check, the call and the usage record
/// are one step.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum LlmStep {
    Answered {
        response: ChatResponse,
        started_at_ms: i64,
        duration_ms: u32,
    },
    OverBudget {
        reason: String,
    },
}

/// Runs `def` on `conversation` until it replies, asks, escalates or reroutes.
///
/// `conversation` is the customer's side (user messages: the ticket thread, context the caller
/// adds), or a previous [`AgentRun::messages`] plus feedback; a system prompt is prepended
/// unless it already starts with one. Each model call is the journaled step `llm-<n>`, where
/// `n` counts the conversation's model turns, so a continued conversation gets fresh names.
///
/// Never fails: model failures (after the runtime's retries), a spent budget, a fatal tool
/// error and the step limit all end in [`Outcome::Escalate`], with [`Trace::error`] set.
pub async fn run<R, Cx>(
    rt: &R,
    env: &AgentEnv,
    def: &AgentDef,
    tools: &ToolSet<Cx>,
    cx: &Cx,
    conversation: Vec<ChatMessage>,
) -> AgentRun
where
    R: Runtime,
    Cx: ?Sized + Sync,
{
    let mut messages = conversation;
    if !matches!(messages.first(), Some(ChatMessage::System { .. })) {
        messages.insert(0, ChatMessage::system(def.system_prompt.clone()));
    }
    let model = def
        .model
        .clone()
        .unwrap_or_else(|| env.llm.default_model().to_owned());
    let mut definitions = tools.definitions();
    definitions.extend(terminal_definitions());
    let first_turn = messages
        .iter()
        .filter(|m| matches!(m, ChatMessage::Assistant { .. }))
        .count();

    let mut run = Run {
        trace: Trace {
            agent: def.name.clone(),
            model: model.clone(),
            system_prompt: def.system_prompt.clone(),
            ..Trace::default()
        },
        messages,
    };
    let mut seen = 0;

    for step in 0..def.max_steps {
        let journal_name = format!("llm-{}", first_turn + step as usize);
        let request = ChatRequest {
            model: def.model.clone(),
            messages: run.messages.clone(),
            tools: definitions.clone(),
            max_tokens: def.max_output_tokens,
            temperature: def.temperature,
        };
        let input = llm_input(&run.messages, seen);
        seen = run.messages.len();

        let (llm, budget, call_model) = (env.llm.clone(), env.budget.clone(), model.clone());
        let result = rt
            .step::<LlmStep>(
                &journal_name,
                Box::new(move || Box::pin(llm_step(llm, budget, call_model, request))),
            )
            .await;

        let response = match result {
            Ok(LlmStep::Answered {
                response,
                started_at_ms,
                duration_ms,
            }) => {
                // Only answered calls count: a refused budget check or a call that failed
                // after its retries cost nothing the trace could show.
                run.trace.llm_calls += 1;
                if step == 0 {
                    run.trace.model = response.model.clone();
                }
                run.trace.push(TraceStep {
                    idx: 0,
                    kind: AgentStepKind::LlmCall,
                    name: response.model.clone(),
                    journal_name: Some(journal_name),
                    input,
                    output: serde_json::to_value(&response).unwrap_or(Value::Null),
                    is_error: false,
                    usage: response.usage,
                    started_at_ms,
                    duration_ms,
                });
                response
            }
            Ok(LlmStep::OverBudget { reason }) => {
                run.llm_failure(&model, journal_name, input, &reason);
                return run.escalate(reason);
            }
            Err(error) => {
                let reason = format!("the model is unavailable: {}", error.message);
                run.llm_failure(&model, journal_name, input, &reason);
                return run.escalate(reason);
            }
        };

        if response.tool_calls.is_empty() {
            return run.answer(&response);
        }

        run.messages.push(ChatMessage::assistant(&response));
        for (i, call) in response.tool_calls.iter().enumerate() {
            if is_terminal(&call.name) {
                match terminal::outcome(&call.name, call.arguments.clone()) {
                    Ok(outcome) => {
                        let output = serde_json::to_value(&outcome).unwrap_or(Value::Null);
                        run.tool_step(call, now_ms(), 0, Ok(output));
                        run.messages.push(ChatMessage::tool_result(
                            call,
                            json!({ "ok": true }).to_string(),
                        ));
                        // Every call of the response gets its result (chat APIs reject an
                        // assistant message with unanswered tool calls, and a guardrail retry
                        // continues these messages); the later ones are skipped, not run.
                        for skipped in &response.tool_calls[i + 1..] {
                            run.skipped_step(skipped);
                        }
                        return run.finish(outcome);
                    }
                    Err(error) => run.tool_step(call, now_ms(), 0, Err(&error)),
                }
                continue;
            }

            let started = now_ms();
            let result = tools.dispatch(cx, &call.name, call.arguments.clone()).await;
            let duration = elapsed_ms(started);
            match result {
                Ok(value) => run.tool_step(call, started, duration, Ok(value)),
                Err(ToolError::Fatal(message)) => {
                    let error = ToolError::Fatal(message);
                    run.tool_step(call, started, duration, Err(&error));
                    return run.escalate(format!("tool `{}` failed: {error}", call.name));
                }
                Err(error) => run.tool_step(call, started, duration, Err(&error)),
            }
        }
    }

    let reason = format!(
        "step limit reached ({} model calls without an answer)",
        def.max_steps
    );
    run.escalate(reason)
}

/// The body of one journaled model call.
async fn llm_step(
    llm: Arc<dyn Llm>,
    budget: Arc<dyn Budget>,
    model: String,
    request: ChatRequest,
) -> Result<LlmStep, StepError> {
    if let BudgetCheck::Exhausted { reason } = budget.check(&model).await? {
        return Ok(LlmStep::OverBudget { reason });
    }
    let started_at_ms = now_ms();
    let response = llm.chat(&request).await.map_err(|e| StepError {
        message: e.to_string(),
        retryable: e.is_retryable(),
    })?;
    let duration_ms = elapsed_ms(started_at_ms);
    // Best effort: the tokens are spent either way (see `Budget`).
    let _ = budget.record(&response.model, response.usage).await;
    Ok(LlmStep::Answered {
        response,
        started_at_ms,
        duration_ms,
    })
}

fn elapsed_ms(started: i64) -> u32 {
    u32::try_from(now_ms().saturating_sub(started).max(0)).unwrap_or(u32::MAX)
}

/// A run in progress.
struct Run {
    trace: Trace,
    messages: Vec<ChatMessage>,
}

impl Run {
    /// Records a tool call that was not run because an earlier call of the same response ended
    /// the run, and answers it. It is marked as an error: it returned nothing, so it is never
    /// evidence for a reply.
    fn skipped_step(&mut self, call: &ToolCall) {
        let output = json!({ "skipped": true, "reason": "the run ended at an earlier call" });
        self.messages
            .push(ChatMessage::tool_result(call, output.to_string()));
        self.trace.push(TraceStep {
            idx: 0,
            kind: AgentStepKind::ToolCall,
            name: call.name.clone(),
            journal_name: None,
            input: call.arguments.clone(),
            output,
            is_error: true,
            usage: Usage::default(),
            started_at_ms: now_ms(),
            duration_ms: 0,
        });
    }

    /// Records a tool call and feeds its result (or error) back to the model.
    fn tool_step(
        &mut self,
        call: &ToolCall,
        started_at_ms: i64,
        duration_ms: u32,
        result: Result<Value, &ToolError>,
    ) {
        let (output, is_error) = match result {
            Ok(value) => (value, false),
            Err(error) => (json!({ "error": error.to_string() }), true),
        };
        // Terminal calls end the run; their acknowledgement is pushed by the caller.
        if !is_terminal(&call.name) || is_error {
            self.messages
                .push(ChatMessage::tool_result(call, output.to_string()));
        }
        self.trace.push(TraceStep {
            idx: 0,
            kind: AgentStepKind::ToolCall,
            name: call.name.clone(),
            journal_name: None,
            input: call.arguments.clone(),
            output,
            is_error,
            usage: Usage::default(),
            started_at_ms,
            duration_ms,
        });
    }

    fn llm_failure(&mut self, model: &str, journal_name: String, input: Value, reason: &str) {
        self.trace.push(TraceStep {
            idx: 0,
            kind: AgentStepKind::LlmCall,
            name: model.to_owned(),
            journal_name: Some(journal_name),
            input,
            output: json!({ "error": reason }),
            is_error: true,
            usage: Usage::default(),
            started_at_ms: now_ms(),
            duration_ms: 0,
        });
    }

    /// A plain answer: the reply, unless it is empty or was cut off.
    fn answer(mut self, response: &ChatResponse) -> AgentRun {
        let text = response.text.as_deref().map(str::trim).unwrap_or_default();
        let problem = match response.finish_reason {
            FinishReason::Length => Some("the reply was cut off at the token limit"),
            FinishReason::ContentFilter => Some("the reply was blocked by the content filter"),
            _ if text.is_empty() => Some("the model returned an empty reply"),
            _ => None,
        };
        if let Some(reason) = problem {
            return self.escalate(reason.to_owned());
        }
        self.messages.push(ChatMessage::assistant(response));
        self.finish(Outcome::Reply {
            body: text.to_owned(),
        })
    }

    fn escalate(mut self, reason: String) -> AgentRun {
        self.trace.error = Some(reason.clone());
        self.finish(Outcome::Escalate { reason })
    }

    fn finish(self, outcome: Outcome) -> AgentRun {
        AgentRun {
            outcome,
            trace: self.trace,
            messages: self.messages,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use futures::executor::block_on;
    use schemars::JsonSchema;
    use serde::Deserialize;
    use serde::de::DeserializeOwned;
    use techdepo_api_types::vocab::Awaiting;
    use techdepo_domain::Intent;
    use techdepo_ports::LlmError;
    use techdepo_ports::testing::ScriptedLlm;

    use super::*;
    use crate::runtime::{Inline, StepFn};
    use crate::tool::Tool;

    struct Cx {
        customer: &'static str,
    }

    #[derive(Deserialize, JsonSchema)]
    struct GetOrderArgs {
        order_number: String,
    }

    /// A read-only tool scoped by the context, never by its arguments.
    struct GetOrder;

    impl Tool<Cx> for GetOrder {
        const NAME: &'static str = "get_order";
        const DESCRIPTION: &'static str = "Looks up one of the customer's orders.";
        type Args = GetOrderArgs;

        async fn call(&self, cx: &Cx, args: GetOrderArgs) -> Result<Value, ToolError> {
            match args.order_number.as_str() {
                "TD-1042" => {
                    Ok(json!({ "number": "TD-1042", "status": "shipped", "customer": cx.customer }))
                }
                "TD-500" => Err(ToolError::Fatal("database gone".into())),
                other => Err(ToolError::NotFound(format!(
                    "{other} is not one of your orders"
                ))),
            }
        }
    }

    fn tools() -> ToolSet<Cx> {
        ToolSet::new().with(GetOrder)
    }

    fn def() -> AgentDef {
        AgentDef::new("order_assistant", "You answer questions about orders.")
    }

    fn call(id: &str, name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: args,
        }
    }

    fn calls(calls: Vec<ToolCall>) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            model: "scripted".into(),
            text: None,
            tool_calls: calls,
            usage: Usage {
                input_tokens: 100,
                output_tokens: 10,
            },
            finish_reason: FinishReason::ToolCalls,
        })
    }

    fn text(text: &str) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            model: "scripted".into(),
            text: Some(text.into()),
            tool_calls: vec![],
            usage: Usage {
                input_tokens: 150,
                output_tokens: 30,
            },
            finish_reason: FinishReason::Stop,
        })
    }

    fn assert_send<T: Send>(value: T) -> T {
        value
    }

    fn go(llm: &Arc<ScriptedLlm>, def: &AgentDef, conversation: Vec<ChatMessage>) -> AgentRun {
        let env = AgentEnv::new(llm.clone());
        let cx = Cx { customer: "cus_1" };
        let tools = tools();
        // Restate handler futures must be `Send`, and so must the loop's.
        block_on(assert_send(run(
            &Inline,
            &env,
            def,
            &tools,
            &cx,
            conversation,
        )))
    }

    #[test]
    fn dispatches_tools_and_replies() {
        let llm = Arc::new(ScriptedLlm::new([
            calls(vec![call(
                "c1",
                "get_order",
                json!({ "order_number": "TD-1042" }),
            )]),
            text("  TD-1042 has shipped.  "),
        ]));
        let done = go(&llm, &def(), vec![ChatMessage::user("Where is TD-1042?")]);

        assert_eq!(
            done.outcome,
            Outcome::Reply {
                body: "TD-1042 has shipped.".into()
            }
        );
        let requests = llm.requests();
        assert_eq!(requests.len(), 2);
        let names: Vec<_> = requests[0].tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["get_order", "ask_customer", "escalate_to_human", "reroute"]
        );
        assert!(
            matches!(&requests[0].messages[0], ChatMessage::System { content } if content.contains("orders"))
        );
        // The tool result went back to the model, answering the call by ID.
        let ChatMessage::Tool {
            tool_call_id,
            content,
            ..
        } = requests[1].messages.last().unwrap()
        else {
            panic!("expected a tool result last");
        };
        assert_eq!(tool_call_id, "c1");
        assert!(
            content.contains("cus_1"),
            "scoped by the context: {content}"
        );

        let kinds: Vec<_> = done
            .trace
            .steps
            .iter()
            .map(|s| (s.kind, s.name.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                (AgentStepKind::LlmCall, "scripted"),
                (AgentStepKind::ToolCall, "get_order"),
                (AgentStepKind::LlmCall, "scripted"),
            ]
        );
        assert_eq!(done.trace.steps[0].journal_name.as_deref(), Some("llm-0"));
        assert_eq!(done.trace.steps[2].journal_name.as_deref(), Some("llm-1"));
        assert_eq!(
            done.trace.steps[2].input["new"].as_array().unwrap().len(),
            2
        );
        assert_eq!(done.trace.usage.total(), 290);
        assert_eq!(done.trace.llm_calls, 2);
        assert_eq!(done.trace.error, None);
        assert_eq!(done.trace.tool_calls("get_order").count(), 1);
        assert_eq!(done.messages.len(), 5, "system, user, call, result, answer");
    }

    #[test]
    fn terminal_tools_end_the_run() {
        let llm = Arc::new(ScriptedLlm::new([calls(vec![
            call(
                "c1",
                "ask_customer",
                json!({ "question": "Which order?", "expects": "order_number" }),
            ),
            call("c2", "get_order", json!({ "order_number": "TD-1042" })),
        ])]));
        let done = go(&llm, &def(), vec![ChatMessage::user("Where is my order?")]);
        assert_eq!(
            done.outcome,
            Outcome::AskCustomer {
                question: "Which order?".into(),
                expects: Awaiting::OrderNumber
            }
        );
        // Later calls are not run, but traced as skipped and answered (chat APIs reject an
        // assistant message with unanswered tool calls).
        let skipped: Vec<_> = done.trace.tool_calls("get_order").collect();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].output["skipped"], true);
        assert!(skipped[0].is_error, "never evidence");
        let answered: Vec<&str> = done
            .messages
            .iter()
            .filter_map(|m| match m {
                ChatMessage::Tool { tool_call_id, .. } => Some(tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(answered, ["c1", "c2"]);

        let llm = Arc::new(ScriptedLlm::new([calls(vec![call(
            "c1",
            "reroute",
            json!({ "intent": "cancel_order" }),
        )])]));
        let done = go(&llm, &def(), vec![ChatMessage::user("Cancel it")]);
        assert_eq!(
            done.outcome,
            Outcome::Reroute {
                intent: Intent::CancelOrder
            }
        );

        let llm = Arc::new(ScriptedLlm::new([calls(vec![call(
            "c1",
            "escalate_to_human",
            json!({ "reason": "legal question" }),
        )])]));
        let done = go(&llm, &def(), vec![ChatMessage::user("My lawyer…")]);
        assert_eq!(
            done.outcome,
            Outcome::Escalate {
                reason: "legal question".into()
            }
        );
        assert_eq!(
            done.trace.error, None,
            "a chosen escalation is not an error"
        );
    }

    #[test]
    fn errors_go_back_to_the_model() {
        let llm = Arc::new(ScriptedLlm::new([
            calls(vec![
                call("c1", "get_order", json!({ "order_number": "TD-9" })),
                call("c2", "get_order", json!({ "number": 9 })),
                call("c3", "cancel_order", json!({})),
                call("c4", "reroute", json!({ "intent": "teleport" })),
            ]),
            text("I could not find that order."),
        ]));
        let done = go(&llm, &def(), vec![ChatMessage::user("Where is TD-9?")]);
        assert!(matches!(done.outcome, Outcome::Reply { .. }));

        let results: Vec<_> = llm.requests()[1]
            .messages
            .iter()
            .filter_map(|m| match m {
                ChatMessage::Tool { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 4, "every call is answered");
        assert!(results[0].contains("not one of your orders"));
        assert!(results[1].contains("invalid arguments"));
        assert!(results[2].contains("no tool named `cancel_order`"));
        assert!(results[3].contains("invalid arguments"));
        assert!(done.trace.steps[1..5].iter().all(|s| s.is_error));
    }

    #[test]
    fn fatal_tool_errors_escalate() {
        let llm = Arc::new(ScriptedLlm::new([calls(vec![call(
            "c1",
            "get_order",
            json!({ "order_number": "TD-500" }),
        )])]));
        let done = go(&llm, &def(), vec![ChatMessage::user("TD-500?")]);
        let Outcome::Escalate { reason } = &done.outcome else {
            panic!("expected an escalation, got {:?}", done.outcome);
        };
        assert!(reason.contains("database gone"));
        assert_eq!(done.trace.error.as_deref(), Some(reason.as_str()));
    }

    #[test]
    fn step_limit_escalates() {
        let looping = || {
            calls(vec![call(
                "c",
                "get_order",
                json!({ "order_number": "TD-1042" }),
            )])
        };
        let llm = Arc::new(ScriptedLlm::new([looping(), looping(), looping()]));
        let def = AgentDef {
            max_steps: 2,
            ..def()
        };
        let done = go(&llm, &def, vec![ChatMessage::user("?")]);
        assert!(
            matches!(&done.outcome, Outcome::Escalate { reason } if reason.contains("step limit"))
        );
        assert_eq!(llm.requests().len(), 2);
        assert_eq!(done.trace.llm_calls, 2);
    }

    #[test]
    fn unusable_answers_and_model_failures_escalate() {
        let llm = Arc::new(ScriptedLlm::new([Err(LlmError::Unavailable("503".into()))]));
        let done = go(&llm, &def(), vec![ChatMessage::user("?")]);
        assert!(matches!(&done.outcome, Outcome::Escalate { reason } if reason.contains("503")));
        assert!(done.trace.steps[0].is_error);

        let mut truncated = text("Your order is").unwrap();
        truncated.finish_reason = FinishReason::Length;
        let llm = Arc::new(ScriptedLlm::new([Ok(truncated)]));
        let done = go(&llm, &def(), vec![ChatMessage::user("?")]);
        assert!(
            matches!(&done.outcome, Outcome::Escalate { reason } if reason.contains("cut off"))
        );

        let llm = Arc::new(ScriptedLlm::new([text("   ")]));
        let done = go(&llm, &def(), vec![ChatMessage::user("?")]);
        assert!(matches!(&done.outcome, Outcome::Escalate { reason } if reason.contains("empty")));
    }

    /// Spends the budget after `allowed` calls and records what it is told.
    struct CountingBudget {
        allowed: usize,
        recorded: Mutex<Vec<(String, Usage)>>,
    }

    #[async_trait]
    impl Budget for CountingBudget {
        async fn check(&self, _model: &str) -> Result<BudgetCheck, StepError> {
            let used = self.recorded.lock().unwrap().len();
            Ok(if used < self.allowed {
                BudgetCheck::Allowed
            } else {
                BudgetCheck::Exhausted {
                    reason: "daily AI budget used up".into(),
                }
            })
        }

        async fn record(&self, model: &str, usage: Usage) -> Result<(), StepError> {
            self.recorded
                .lock()
                .unwrap()
                .push((model.to_owned(), usage));
            Ok(())
        }
    }

    #[test]
    fn budget_is_checked_before_every_call() {
        let llm = Arc::new(ScriptedLlm::new([
            calls(vec![call(
                "c1",
                "get_order",
                json!({ "order_number": "TD-1042" }),
            )]),
            text("never reached"),
        ]));
        let budget = Arc::new(CountingBudget {
            allowed: 1,
            recorded: Mutex::default(),
        });
        let env = AgentEnv::new(llm.clone()).with_budget(budget.clone());
        let cx = Cx { customer: "cus_1" };
        let done = block_on(run(
            &Inline,
            &env,
            &def(),
            &tools(),
            &cx,
            vec![ChatMessage::user("Where is TD-1042?")],
        ));
        assert_eq!(
            done.outcome,
            Outcome::Escalate {
                reason: "daily AI budget used up".into()
            }
        );
        assert_eq!(
            llm.requests().len(),
            1,
            "the second call never reached the model"
        );
        assert_eq!(done.trace.llm_calls, 1, "the refused call is not counted");
        assert_eq!(
            budget.recorded.lock().unwrap().as_slice(),
            [(
                "scripted".to_owned(),
                Usage {
                    input_tokens: 100,
                    output_tokens: 10
                }
            )]
        );
    }

    /// Records step names and replays journaled results, like a durable runtime would.
    #[derive(Default)]
    struct Journal {
        entries: Mutex<Vec<(String, Value)>>,
        replay: bool,
    }

    impl Runtime for Journal {
        async fn step<T>(&self, name: &str, step: StepFn<T>) -> Result<T, StepError>
        where
            T: Serialize + DeserializeOwned + Send + 'static,
        {
            if self.replay {
                let entries = self.entries.lock().unwrap();
                if let Some((_, value)) = entries.iter().find(|(n, _)| n == name) {
                    return Ok(serde_json::from_value(value.clone()).unwrap());
                }
            }
            let value = step().await?;
            self.entries
                .lock()
                .unwrap()
                .push((name.to_owned(), serde_json::to_value(&value).unwrap()));
            Ok(value)
        }
    }

    #[test]
    fn model_calls_are_journaled_and_replayed() {
        let script = || {
            [
                calls(vec![call(
                    "c1",
                    "get_order",
                    json!({ "order_number": "TD-1042" }),
                )]),
                text("TD-1042 has shipped."),
            ]
        };
        let llm = Arc::new(ScriptedLlm::new(script()));
        let mut journal = Journal::default();
        let env = AgentEnv::new(llm.clone());
        let cx = Cx { customer: "cus_1" };
        let conversation = vec![ChatMessage::user("Where is TD-1042?")];
        let first = block_on(run(
            &journal,
            &env,
            &def(),
            &tools(),
            &cx,
            conversation.clone(),
        ));
        let names: Vec<_> = journal
            .entries
            .lock()
            .unwrap()
            .iter()
            .map(|(n, _)| n.clone())
            .collect();
        assert_eq!(names, ["llm-0", "llm-1"]);

        // A retry of the invocation replays the journal: the model is not called again.
        journal.replay = true;
        let replayed = block_on(run(&journal, &env, &def(), &tools(), &cx, conversation));
        assert_eq!(replayed.outcome, first.outcome);
        assert_eq!(llm.requests().len(), 2);

        // Feedback continues the conversation under fresh step names.
        let llm = Arc::new(ScriptedLlm::new([text("Corrected answer.")]));
        let env = AgentEnv::new(llm);
        let mut messages = first.messages;
        messages.push(ChatMessage::user("Guardrail: do not mention internal IDs."));
        let journal = Journal::default();
        let retried = block_on(run(&journal, &env, &def(), &tools(), &cx, messages));
        assert!(matches!(retried.outcome, Outcome::Reply { .. }));
        assert_eq!(journal.entries.lock().unwrap()[0].0, "llm-2");
    }
}
