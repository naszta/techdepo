//! Agent and classifier traces (`agent_runs`, `agent_steps`): the read side for the admin
//! ticket page and its "Trace" tab, and the statements the support pipeline (router, agents,
//! judge) writes them with ([`AgentRuns::insert_run_statement`], one per step
//! [`AgentRuns::insert_step_statement`]; idempotent on their keys).

use serde::Deserialize;
use techdepo_api_types::AgentRunId;
use techdepo_api_types::support::{
    AgentOutcome, AgentRunDetail, AgentRunSummary, AgentStep, GuardrailResult, ReplyConfidence,
};
use techdepo_api_types::vocab::{AgentKind, AgentOutcomeKind, AgentRunStatus, AgentStepKind};
use techdepo_domain::{InvestigationId, TicketId};
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutorExt, params};
use time::OffsetDateTime;

use crate::{Param, SqlError, Statement, json_param, repository, stmt};

/// An `agent_runs` row.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AgentRunRow {
    /// Trace ID.
    pub id: AgentRunId,
    /// Ticket.
    pub ticket_id: Option<TicketId>,
    /// Investigation (reply interpretation).
    pub investigation_id: Option<InvestigationId>,
    /// Which component.
    pub agent: AgentKind,
    /// Model.
    pub model: String,
    /// State.
    pub status: AgentRunStatus,
    /// Outcome kind.
    pub outcome: Option<AgentOutcomeKind>,
    /// Outcome.
    #[serde(rename = "outcome_json", default, with = "column::json_text::option")]
    pub outcome_detail: Option<AgentOutcome>,
    /// System prompt.
    pub system_prompt: Option<String>,
    /// Prompt tokens.
    pub input_tokens: u32,
    /// Completion tokens.
    pub output_tokens: u32,
    /// Steps.
    pub step_count: u32,
    /// 1, or 2 for the guardrail retry.
    pub attempt: u32,
    /// Reply confidence.
    pub reply_confidence: Option<f64>,
    /// Confidence breakdown.
    #[serde(
        rename = "confidence_json",
        default,
        with = "column::json_text::option"
    )]
    pub confidence: Option<ReplyConfidence>,
    /// Guardrail results.
    #[serde(
        rename = "guardrails_json",
        default,
        with = "column::json_text::option"
    )]
    pub guardrails: Option<Vec<GuardrailResult>>,
    /// Failure message.
    pub error: Option<String>,
    /// Started.
    #[serde(with = "column::millis")]
    pub started_at: OffsetDateTime,
    /// Finished.
    #[serde(default, with = "column::millis::option")]
    pub finished_at: Option<OffsetDateTime>,
}

impl AgentRunRow {
    /// The trace in a list.
    pub fn summary(&self) -> AgentRunSummary {
        AgentRunSummary {
            id: self.id.clone(),
            agent: self.agent,
            model: self.model.clone(),
            status: self.status,
            outcome: self.outcome,
            ticket_id: self.ticket_id.clone(),
            investigation_id: self.investigation_id.clone(),
            attempt: self.attempt,
            step_count: self.step_count,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            reply_confidence: self.reply_confidence,
            started_at: self.started_at,
            finished_at: self.finished_at,
        }
    }

    /// The full trace with its steps.
    pub fn detail(&self, steps: &[AgentStepRow]) -> AgentRunDetail {
        AgentRunDetail {
            run: self.summary(),
            system_prompt: self.system_prompt.clone(),
            outcome: self.outcome_detail.clone(),
            confidence: self.confidence,
            guardrails: self.guardrails.clone().unwrap_or_default(),
            error: self.error.clone(),
            steps: steps.iter().map(AgentStepRow::step).collect(),
        }
    }
}

/// An `agent_steps` row.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AgentStepRow {
    /// Position, from 0.
    pub idx: u32,
    /// Kind.
    pub kind: AgentStepKind,
    /// Model, tool or guardrail name.
    pub name: String,
    /// The `ctx.run` name.
    pub journal_name: Option<String>,
    /// Input.
    #[serde(rename = "input_json", default, with = "column::json_text::option")]
    pub input: Option<serde_json::Value>,
    /// Output.
    #[serde(rename = "output_json", default, with = "column::json_text::option")]
    pub output: Option<serde_json::Value>,
    /// Failed.
    #[serde(with = "column::bool_int")]
    pub is_error: bool,
    /// Prompt tokens.
    pub input_tokens: u32,
    /// Completion tokens.
    pub output_tokens: u32,
    /// Started.
    #[serde(with = "column::millis")]
    pub started_at: OffsetDateTime,
    /// Duration.
    pub duration_ms: u32,
}

impl AgentStepRow {
    /// The DTO.
    pub fn step(&self) -> AgentStep {
        AgentStep {
            idx: self.idx,
            kind: self.kind,
            name: self.name.clone(),
            journal_name: self.journal_name.clone(),
            input: self.input.clone(),
            output: self.output.clone(),
            is_error: self.is_error,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            started_at: self.started_at,
            duration_ms: self.duration_ms,
        }
    }
}

/// A finished (or running) trace to store.
#[derive(Debug, Clone, PartialEq)]
pub struct NewAgentRun {
    /// Trace ID (`run_…`).
    pub id: AgentRunId,
    /// Ticket.
    pub ticket_id: Option<TicketId>,
    /// Investigation.
    pub investigation_id: Option<InvestigationId>,
    /// Which component.
    pub agent: AgentKind,
    /// Model.
    pub model: String,
    /// State (`finished_at` is set exactly when not `running`).
    pub status: AgentRunStatus,
    /// Outcome, for agents.
    pub outcome: Option<AgentOutcome>,
    /// System prompt.
    pub system_prompt: Option<String>,
    /// Prompt tokens.
    pub input_tokens: u32,
    /// Completion tokens.
    pub output_tokens: u32,
    /// Steps.
    pub step_count: u32,
    /// 1, or 2 for the guardrail retry.
    pub attempt: u32,
    /// Reply confidence breakdown.
    pub confidence: Option<ReplyConfidence>,
    /// Guardrail results.
    pub guardrails: Option<Vec<GuardrailResult>>,
    /// Failure message.
    pub error: Option<String>,
    /// Started.
    pub started_at: OffsetDateTime,
    /// Finished.
    pub finished_at: Option<OffsetDateTime>,
}

/// One step of a trace to store.
#[derive(Debug, Clone, PartialEq)]
pub struct NewAgentStep {
    /// Position, from 0.
    pub idx: u32,
    /// Kind.
    pub kind: AgentStepKind,
    /// Model, tool or guardrail name.
    pub name: String,
    /// The `ctx.run` name.
    pub journal_name: Option<String>,
    /// Input.
    pub input: Option<serde_json::Value>,
    /// Output.
    pub output: Option<serde_json::Value>,
    /// Failed.
    pub is_error: bool,
    /// Prompt tokens.
    pub input_tokens: u32,
    /// Completion tokens.
    pub output_tokens: u32,
    /// Started.
    pub started_at: OffsetDateTime,
    /// Duration.
    pub duration_ms: u32,
}

fn json_or_null<T: serde::Serialize>(value: Option<&T>) -> Result<Param, SqlError> {
    value.map_or(Ok(Param::Null), json_param)
}

repository! {
    /// Agent and classifier traces.
    AgentRuns
}

impl<'a> AgentRuns<'a> {
    /// The ticket's traces, oldest first.
    pub async fn for_ticket(&self, ticket: &TicketId) -> Result<Vec<AgentRunRow>, SqlError> {
        self.sql
            .query_as(
                "SELECT * FROM agent_runs WHERE org_id = ?1 AND ticket_id = ?2 ORDER BY started_at, id",
                &params![self.org(), ticket],
            )
            .await
    }

    /// A trace's steps, in order.
    pub async fn steps(&self, run: &AgentRunId) -> Result<Vec<AgentStepRow>, SqlError> {
        self.sql
            .query_as(
                "SELECT idx, kind, name, journal_name, input_json, output_json, is_error, input_tokens, \
                 output_tokens, started_at, duration_ms FROM agent_steps WHERE org_id = ?1 AND run_id = ?2 ORDER BY idx",
                &params![self.org(), run.as_str()],
            )
            .await
    }

    /// Stores a trace (an upsert: a running trace is updated when it finishes).
    pub fn insert_run_statement(&self, run: &NewAgentRun) -> Result<Statement, SqlError> {
        Ok(stmt(
            "INSERT INTO agent_runs (org_id, id, ticket_id, investigation_id, agent, model, status, outcome, outcome_json, \
             system_prompt, input_tokens, output_tokens, step_count, attempt, reply_confidence, confidence_json, \
             guardrails_json, error, started_at, finished_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20) \
             ON CONFLICT (org_id, id) DO UPDATE SET status = excluded.status, outcome = excluded.outcome, \
             outcome_json = excluded.outcome_json, input_tokens = excluded.input_tokens, \
             output_tokens = excluded.output_tokens, step_count = excluded.step_count, \
             reply_confidence = excluded.reply_confidence, confidence_json = excluded.confidence_json, \
             guardrails_json = excluded.guardrails_json, error = excluded.error, finished_at = excluded.finished_at",
            vec![
                self.org().into(),
                run.id.as_str().into(),
                run.ticket_id.as_ref().into(),
                run.investigation_id.as_ref().into(),
                run.agent.as_str().into(),
                run.model.as_str().into(),
                run.status.as_str().into(),
                run.outcome.as_ref().map(|o| o.kind().as_str()).into(),
                json_or_null(run.outcome.as_ref())?,
                run.system_prompt.clone().into(),
                run.input_tokens.into(),
                run.output_tokens.into(),
                run.step_count.into(),
                run.attempt.max(1).into(),
                run.confidence.map(|c| c.confidence.clamp(0.0, 1.0)).into(),
                json_or_null(run.confidence.as_ref())?,
                json_or_null(run.guardrails.as_ref())?,
                run.error.clone().into(),
                run.started_at.into(),
                run.finished_at.into(),
            ],
        ))
    }

    /// Stores one step of a trace (idempotent on its index).
    pub fn insert_step_statement(
        &self,
        run: &AgentRunId,
        step: &NewAgentStep,
    ) -> Result<Statement, SqlError> {
        Ok(stmt(
            "INSERT INTO agent_steps (org_id, run_id, idx, kind, name, journal_name, input_json, output_json, is_error, \
             input_tokens, output_tokens, started_at, duration_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) ON CONFLICT DO NOTHING",
            vec![
                self.org().into(),
                run.as_str().into(),
                step.idx.into(),
                step.kind.as_str().into(),
                step.name.as_str().into(),
                step.journal_name.clone().into(),
                json_or_null(step.input.as_ref())?,
                json_or_null(step.output.as_ref())?,
                step.is_error.into(),
                step.input_tokens.into(),
                step.output_tokens.into(),
                step.started_at.into(),
                step.duration_ms.into(),
            ],
        ))
    }
}
