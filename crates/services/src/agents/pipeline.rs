//! One agent route run, from the route request to the outcome for the ticket (support.md §5–§6):
//!
//! 1. **Agent** (`techdepo_agent::run`): the conversation in, an outcome out; every model call a
//!    journaled step behind the org's daily token budget, every D1 read a journaled tool step.
//! 2. **Guardrails** on AI-authored text (a reply or a question). On failure the agent gets one
//!    retry with the violations as feedback (a second trace, `attempt = 2`); a second failure is
//!    reported with its failing guardrails, which the ticket never sends (it escalates).
//! 3. **Judge** (one classifier pass) on text that passed: the reply confidence is
//!    `min(p(intent), judge score)`, stored with its breakdown (`confidence_json`).
//! 4. **Trace:** every attempt's model and tool calls plus the guardrail results go to
//!    `agent_runs` / `agent_steps` with the judge's pass and its `llm_usage`, in one journaled
//!    batch (`record-trace`).
//!
//! Generic over the agent loop's [`Runtime`], so the whole pipeline runs natively in tests
//! (`Inline`, SQLite) and journaled in the services (`Journaled`).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use techdepo_agent::{AgentEnv, AgentRun, Outcome, Runtime, StepError, ToolSet, Trace};
use techdepo_api_types::AgentRunId;
use techdepo_api_types::ingress::derive_id;
use techdepo_api_types::support::{GuardrailResult, JudgeScores, ReplyConfidence};
use techdepo_api_types::vocab::{AgentKind, AgentRunStatus, AgentStepKind, Direction, TicketRoute};
use techdepo_db::agent_runs::{NewAgentRun, NewAgentStep};
use techdepo_db::{OrgScope, Statement};
use techdepo_domain::{Intent, ModelSlot, OrgSettings};
use techdepo_llm::UsageLedger;
use techdepo_ports::{ChatMessage, Classification, Classifier, Usage};
use time::OffsetDateTime;

use super::tools::{self, ToolCx};
use super::{guardrails, judge, prompts};
use crate::Deps;
use crate::support::contract::{AiReply, Reply, RouteOutcome, RouteRequest};
use crate::support::conversation::truncate;

/// Earlier messages of the conversation the agent sees.
const HISTORY: usize = 8;
/// Characters of each earlier message.
const MAX_HISTORY_CHARS: usize = 1_500;
/// Agent runs per request: the first, and one retry after failed guardrails.
const MAX_ATTEMPTS: u32 = 2;

/// What the run decided, for `SupportTicket/route_result`.
#[derive(Debug, Clone, PartialEq)]
pub struct Answered {
    /// The outcome.
    pub outcome: RouteOutcome,
    /// The trace of the deciding attempt.
    pub agent_run_id: AgentRunId,
}

/// One agent attempt and what the guardrails said about it.
struct Attempt {
    id: AgentRunId,
    number: u32,
    run: AgentRun,
    /// `None` when the outcome has no AI-authored text (escalate, reroute).
    guardrails: Option<Vec<GuardrailResult>>,
}

impl Attempt {
    fn text(&self) -> Option<&str> {
        match &self.run.outcome {
            Outcome::Reply { body } => Some(body),
            Outcome::AskCustomer { question, .. } => Some(question),
            Outcome::Escalate { .. } | Outcome::Reroute { .. } => None,
        }
    }

    fn passed(&self) -> bool {
        self.guardrails.as_deref().is_none_or(guardrails::passed)
    }
}

/// The judge's pass as journaled.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JudgePass {
    /// The model asked (the trace's model when the call failed).
    #[serde(default)]
    model: String,
    started_at_ms: i64,
    duration_ms: u32,
    result: Result<Classification, String>,
}

/// The route of `agent`.
pub fn route_of(agent: AgentKind) -> TicketRoute {
    match agent {
        AgentKind::ProductAdvisor => TicketRoute::ProductAdvisor,
        _ => TicketRoute::OrderAssistant,
    }
}

/// The trace ID of `agent`'s `attempt` on this request: derived, so a retried invocation
/// upserts the same rows.
fn run_id(req: &RouteRequest, kind: AgentKind, attempt: &str) -> AgentRunId {
    AgentRunId::new(derive_id(
        "run",
        &[
            req.org_id.as_str(),
            &req.ticket_id.to_string(),
            &req.generation.to_string(),
            kind.as_str(),
            attempt,
        ],
    ))
}

/// The conversation as chat messages: recent earlier messages (customer → user, ours →
/// assistant), then the new customer text.
pub fn conversation(req: &RouteRequest) -> Vec<ChatMessage> {
    let trailing = req
        .conversation
        .iter()
        .rev()
        .take_while(|m| m.direction == Direction::Inbound)
        .count();
    let earlier = &req.conversation[..req.conversation.len() - trailing];
    let earlier = &earlier[earlier.len().saturating_sub(HISTORY)..];
    let mut out: Vec<ChatMessage> = earlier
        .iter()
        .map(|m| {
            let body = truncate(m.body.trim(), MAX_HISTORY_CHARS);
            match m.direction {
                Direction::Inbound => ChatMessage::user(body),
                Direction::Outbound => ChatMessage::Assistant {
                    content: Some(body),
                    tool_calls: Vec::new(),
                },
            }
        })
        .collect();
    let mut text = String::new();
    if earlier.is_empty() && !req.subject.trim().is_empty() {
        text.push_str(&format!("Subject: {}\n\n", req.subject.trim()));
    }
    text.push_str(req.message.trim());
    out.push(ChatMessage::user(text));
    out
}

/// Runs `agent` on `req` (support.md §5–§6). Never fails: anything that goes wrong ends in an
/// escalation.
pub async fn answer<R: Runtime>(
    rt: &R,
    deps: &Deps,
    agent: AgentKind,
    req: &RouteRequest,
    settings: &OrgSettings,
    now: OffsetDateTime,
) -> Answered {
    let def = prompts::definition(agent, req.customer_name.as_deref(), now.date());
    let models = crate::models::stepped(rt, deps).await;
    let slot = match agent {
        AgentKind::ProductAdvisor => ModelSlot::ProductAdvisor,
        _ => ModelSlot::OrderAssistant,
    };
    let ledger = UsageLedger::new(deps.db.executor(), deps.config.llm_budget);
    let env = AgentEnv::new(crate::models::chat(deps, &models, slot))
        .with_budget(Arc::new(ledger.for_org(req.org_id.clone(), now.date())));
    let scope = OrgScope::verified(req.org_id.clone());
    let cx = ToolCx::new(
        rt,
        deps.db.clone(),
        scope.clone(),
        req.user_id.clone(),
        req.ticket_id.clone(),
    );
    let tools: ToolSet<ToolCx<'_, R>> = match agent {
        AgentKind::ProductAdvisor => tools::product_advisor(),
        _ => tools::order_assistant(),
    };
    let hosts = guardrails::allowed_hosts(&deps.config);

    let mut attempts: Vec<Attempt> = Vec::new();
    let mut messages = conversation(req);
    // The tool calls of every attempt so far: a retry continues the same conversation, so the
    // model may rely on what the first attempt's tools returned without calling them again.
    let mut evidence = Trace::default();
    for number in 1..=MAX_ATTEMPTS {
        let run = techdepo_agent::run(rt, &env, &def, &tools, &cx, messages).await;
        evidence.steps.extend(run.trace.steps.iter().cloned());
        let mut attempt = Attempt {
            id: run_id(req, agent, &number.to_string()),
            number,
            guardrails: None,
            run,
        };
        attempt.guardrails = attempt
            .text()
            .map(|text| guardrails::check(text, &evidence, &hosts));
        let retry = !attempt.passed() && number < MAX_ATTEMPTS;
        messages = attempt.run.messages.clone();
        if let Some(results) = attempt.guardrails.as_deref().filter(|_| retry) {
            messages.push(ChatMessage::user(guardrails::feedback(results)));
        }
        attempts.push(attempt);
        if !retry {
            break;
        }
    }
    let last = attempts
        .last()
        .unwrap_or_else(|| unreachable!("one attempt at least"));

    // The judge, on text that may be sent.
    let judge_id = run_id(req, agent, "judge");
    let judge_pass = match last.text() {
        Some(text) if last.passed() => {
            let input = judge::text(&req.message, &evidence, text);
            let classifier = crate::models::classifier(deps, &models, ModelSlot::Judge);
            let model = models.judge.model.clone();
            Some((input.clone(), run_judge(rt, classifier, model, input).await))
        }
        _ => None,
    };
    let scores = judge_pass
        .as_ref()
        .and_then(|(_, pass)| pass.result.as_ref().ok().map(judge::scores));

    let ai = |body: String| AiReply {
        body,
        intent_p: req.intent_p,
        judge: scores,
        guardrails: last.guardrails.clone().unwrap_or_default(),
        agent_run_id: last.id.clone(),
    };
    let confidence = last.text().filter(|_| last.passed()).map(|text| {
        let c = ai(text.to_owned()).confidence();
        ReplyConfidence {
            intent_p: req.intent_p,
            judge: scores.unwrap_or(JudgeScores {
                answers_question: 0.0,
                supported_by_tools: 0.0,
                tone_appropriate: 0.0,
            }),
            confidence: c,
            policy: settings.reply_policy,
            threshold: settings.reply_confidence_threshold,
            disposition: settings.reply_disposition(c),
        }
    });

    let outcome = route_outcome(agent, last.run.outcome.clone(), ai);

    // The trace: every attempt, the judge, its usage.
    let mut statements = Vec::new();
    let runs = deps.db.agent_runs(&scope);
    for a in &attempts {
        let is_last = a.number == last.number;
        statements.extend(attempt_statements(
            &runs,
            req,
            agent,
            a,
            if is_last { confidence } else { None },
            now,
        ));
    }
    if let Some((input, pass)) = &judge_pass {
        statements.extend(judge_statements(
            &runs, req, &judge_id, input, pass, confidence, now,
        ));
    }
    let db = deps.db.clone();
    // Best effort: the outcome goes to the ticket either way.
    let _ = rt
        .step::<()>(
            "record-trace",
            Box::new(move || {
                Box::pin(async move {
                    db.batch(&statements)
                        .await
                        .map(|_| ())
                        .map_err(|e| StepError {
                            retryable: e.is_retryable(),
                            message: e.to_string(),
                        })
                })
            }),
        )
        .await;

    Answered {
        outcome,
        agent_run_id: last.id.clone(),
    }
}

/// One journaled classifier pass of the judge.
async fn run_judge<R: Runtime>(
    rt: &R,
    classifier: Arc<dyn Classifier>,
    model: String,
    input: String,
) -> JudgePass {
    let asked = model.clone();
    let step = rt
        .step::<JudgePass>(
            "judge",
            Box::new(move || {
                Box::pin(async move {
                    let started = now_ms();
                    let result = classifier.classify(&input, &judge::questions()).await;
                    let duration_ms = elapsed(started);
                    match result {
                        Ok(c) => Ok(JudgePass {
                            model: model.clone(),
                            started_at_ms: started,
                            duration_ms,
                            result: Ok(c),
                        }),
                        Err(e) if e.is_retryable() => Err(StepError::retryable(e.to_string())),
                        Err(e) => Ok(JudgePass {
                            model: model.clone(),
                            started_at_ms: started,
                            duration_ms,
                            result: Err(e.to_string()),
                        }),
                    }
                })
            }),
        )
        .await;
    step.unwrap_or_else(|e| JudgePass {
        model: asked,
        started_at_ms: now_ms(),
        duration_ms: 0,
        result: Err(e.message),
    })
}

/// What the ticket gets for the agent's `outcome`.
fn route_outcome(
    agent: AgentKind,
    outcome: Outcome,
    ai: impl Fn(String) -> AiReply,
) -> RouteOutcome {
    match outcome {
        Outcome::Reply { body } => RouteOutcome::Reply {
            reply: Reply::Ai(ai(body)),
            resolve: true,
        },
        Outcome::AskCustomer { question, expects } => RouteOutcome::AskCustomer {
            question: Reply::Ai(ai(question)),
            expects,
            awakeable_id: None,
        },
        Outcome::Escalate { reason } => RouteOutcome::Escalate {
            reason: format!("{}: {reason}", agent.label()),
            causes: Vec::new(),
        },
        // Back to the same route, or to no route: a person has to look.
        Outcome::Reroute { intent }
            if intent == Intent::Other || TicketRoute::for_intent(intent) == route_of(agent) =>
        {
            RouteOutcome::Escalate {
                reason: format!(
                    "{}: could not handle the request (wanted to reroute it as {})",
                    agent.label(),
                    intent.as_str()
                ),
                causes: Vec::new(),
            }
        }
        Outcome::Reroute { intent } => RouteOutcome::Reroute { intent },
    }
}

/// Live wall clock in Unix milliseconds (inside journaled steps, for the trace only).
fn now_ms() -> i64 {
    i64::try_from(crate::util::wall_clock().unix_timestamp_nanos() / 1_000_000).unwrap_or(0)
}

fn elapsed(started_ms: i64) -> u32 {
    u32::try_from(now_ms().saturating_sub(started_ms).max(0)).unwrap_or(u32::MAX)
}

fn from_ms(ms: i64, fallback: OffsetDateTime) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).unwrap_or(fallback)
}

/// When a trace started and finished, from its steps (live clock inside the journaled steps),
/// or `now` without steps.
fn span(trace: &Trace, now: OffsetDateTime) -> (OffsetDateTime, OffsetDateTime) {
    let start = trace
        .steps
        .iter()
        .map(|s| s.started_at_ms)
        .filter(|&ms| ms > 0)
        .min();
    let end = trace
        .steps
        .iter()
        .map(|s| s.started_at_ms + i64::from(s.duration_ms))
        .filter(|&ms| ms > 0)
        .max();
    match (start, end) {
        (Some(s), Some(e)) => (from_ms(s, now), from_ms(e.max(s), now)),
        _ => (now, now),
    }
}

/// `agent_runs` + `agent_steps` of one attempt: its model and tool calls, then one step per
/// guardrail.
fn attempt_statements(
    runs: &techdepo_db::agent_runs::AgentRuns<'_>,
    req: &RouteRequest,
    agent: AgentKind,
    a: &Attempt,
    confidence: Option<ReplyConfidence>,
    now: OffsetDateTime,
) -> Vec<Statement> {
    let trace = &a.run.trace;
    let (started, finished) = span(trace, now);
    let mut steps: Vec<NewAgentStep> = trace
        .steps
        .iter()
        .map(|s| NewAgentStep {
            idx: s.idx,
            kind: s.kind,
            name: s.name.clone(),
            journal_name: s.journal_name.clone(),
            input: Some(s.input.clone()),
            output: Some(s.output.clone()),
            is_error: s.is_error,
            input_tokens: s.usage.input_tokens,
            output_tokens: s.usage.output_tokens,
            started_at: from_ms(s.started_at_ms, now),
            duration_ms: s.duration_ms,
        })
        .collect();
    if let (Some(results), Some(text)) = (&a.guardrails, a.text()) {
        for g in results {
            steps.push(NewAgentStep {
                idx: u32::try_from(steps.len()).unwrap_or(u32::MAX),
                kind: AgentStepKind::Guardrail,
                name: g.guardrail.as_str().to_owned(),
                journal_name: None,
                input: Some(json!({ "text": text })),
                output: serde_json::to_value(g).ok(),
                is_error: !g.passed,
                input_tokens: 0,
                output_tokens: 0,
                started_at: finished,
                duration_ms: 0,
            });
        }
    }
    let failed_guardrails = !a.passed();
    let run = NewAgentRun {
        id: a.id.clone(),
        ticket_id: Some(req.ticket_id.clone()),
        investigation_id: None,
        agent,
        model: trace.model.clone(),
        status: if trace.error.is_some() {
            AgentRunStatus::Failed
        } else {
            AgentRunStatus::Completed
        },
        outcome: Some(a.run.outcome.clone()),
        system_prompt: Some(trace.system_prompt.clone()),
        input_tokens: trace.usage.input_tokens,
        output_tokens: trace.usage.output_tokens,
        step_count: u32::try_from(steps.len()).unwrap_or(u32::MAX),
        attempt: a.number,
        confidence,
        guardrails: a.guardrails.clone(),
        error: trace.error.clone().or_else(|| {
            failed_guardrails.then(|| {
                if a.number < MAX_ATTEMPTS {
                    "Guardrails failed; the agent was asked to try again".to_owned()
                } else {
                    "Guardrails failed twice; the reply is not sent".to_owned()
                }
            })
        }),
        started_at: started,
        finished_at: Some(finished),
    };
    let mut out = Vec::new();
    if let Ok(s) = runs.insert_run_statement(&run) {
        out.push(s);
    }
    out.extend(
        steps
            .iter()
            .filter_map(|s| runs.insert_step_statement(&run.id, s).ok()),
    );
    out
}

/// The judge's trace (one classification step) and its `llm_usage`.
fn judge_statements(
    runs: &techdepo_db::agent_runs::AgentRuns<'_>,
    req: &RouteRequest,
    id: &AgentRunId,
    input: &str,
    pass: &JudgePass,
    confidence: Option<ReplyConfidence>,
    now: OffsetDateTime,
) -> Vec<Statement> {
    let started = from_ms(pass.started_at_ms, now);
    let finished = started + time::Duration::milliseconds(i64::from(pass.duration_ms));
    let (model, usage, output, error) = match &pass.result {
        Ok(c) => (
            c.model.clone(),
            c.usage.unwrap_or_default(),
            serde_json::to_value(c).ok(),
            None,
        ),
        Err(e) => (
            pass.model.clone(),
            Usage::default(),
            Some(json!({ "error": e })),
            Some(e.clone()),
        ),
    };
    let run = NewAgentRun {
        id: id.clone(),
        ticket_id: Some(req.ticket_id.clone()),
        investigation_id: None,
        agent: AgentKind::Judge,
        model: model.clone(),
        status: if error.is_some() {
            AgentRunStatus::Failed
        } else {
            AgentRunStatus::Completed
        },
        outcome: None,
        system_prompt: None,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        step_count: 1,
        attempt: 1,
        confidence,
        guardrails: None,
        error: error.clone(),
        started_at: started,
        finished_at: Some(finished),
    };
    let step = NewAgentStep {
        idx: 0,
        kind: AgentStepKind::Classification,
        name: model.clone(),
        journal_name: Some("judge".to_owned()),
        input: Some(json!({ "text": input, "questions": judge::questions() })),
        output,
        is_error: error.is_some(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        started_at: started,
        duration_ms: pass.duration_ms,
    };
    let mut out = Vec::new();
    if let Ok(s) = runs.insert_run_statement(&run) {
        out.push(s);
    }
    if let Ok(s) = runs.insert_step_statement(id, &step) {
        out.push(s);
    }
    if usage.total() > 0 {
        out.push(UsageLedger::record_statement(
            &req.org_id,
            now.date(),
            &model,
            usage,
        ));
    }
    out
}
