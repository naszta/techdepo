//! Support tickets as the customer sees them, the "Send as email" form, and the AI artefacts
//! stored on tickets and traces (classification, reply confidence, guardrails, agent runs).

use serde::{Deserialize, Serialize};
use techdepo_domain::{
    Actor, Channel, Intent, InvestigationId, OrderNumber, ReplyDisposition, ReplyPolicy, TicketId,
    TicketStatus, labeled_enum,
};
use time::OffsetDateTime;

use crate::common::OrderRef;
use crate::error::{ApiError, Validate, check_client_key, check_text};
use crate::ids::AgentRunId;
use crate::vocab::{
    AgentKind, AgentOutcomeKind, AgentRunStatus, AgentStepKind, Awaiting, Direction, MessageStatus,
};

/// Longest message body accepted from the web form, in characters.
pub const MAX_MESSAGE_CHARS: usize = 10_000;

// ---------------------------------------------------------------------------------------------
// Tickets (customer view)
// ---------------------------------------------------------------------------------------------

/// `GET /api/tickets` query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketListQuery {
    /// Only tickets in this status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TicketStatus>,
    /// Page cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A ticket in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketSummary {
    /// Ticket ID, e.g. `T-7F2M9Q`.
    pub id: TicketId,
    /// Subject of the first message.
    pub subject: String,
    /// Status.
    pub status: TicketStatus,
    /// Channel of the first message.
    pub channel: Channel,
    /// The order it is about, once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<OrderRef>,
    /// Messages the customer can see.
    pub message_count: u32,
    /// Latest message.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub last_message_at: Option<OffsetDateTime>,
    /// Opened.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// One message of a ticket's conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketMessage {
    /// Position in the ticket, from 1.
    pub seq: u32,
    /// From or to the customer.
    pub direction: Direction,
    /// Customer for inbound; AI, human or system for outbound (the actor tag in the thread).
    pub author: Actor,
    /// Email or web form.
    pub channel: Channel,
    /// `received`/`sent`, or `draft`/`discarded` (admin only).
    pub status: MessageStatus,
    /// Subject.
    pub subject: String,
    /// Plain-text body (quoted history and signatures stripped for inbound mail).
    pub body_text: String,
    /// HTML body of outbound mail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_html: Option<String>,
    /// Created (received, or drafted).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Sent.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub sent_at: Option<OffsetDateTime>,
}

/// `GET /api/tickets/{id}`: the customer's view of a ticket (no drafts, no AI internals).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketDetail {
    /// Header.
    pub ticket: TicketSummary,
    /// Messages, oldest first.
    pub messages: Vec<TicketMessage>,
    /// Address that continues this ticket by email (`support+<org>.<ticket>@…`).
    pub reply_address: String,
}

/// `POST /api/support/messages`: the "Send as email" form. Enters the same pipeline as a
/// real email (`SupportTicket/inbound` with `channel = web`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendSupportMessageRequest {
    /// Client-generated key; also the basis of the message's `Message-ID`.
    pub idempotency_key: String,
    /// Continue this ticket; absent opens a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_id: Option<TicketId>,
    /// Subject, 1–200 characters.
    pub subject: String,
    /// Body, 1–10 000 characters.
    pub body: String,
}

impl Validate for SendSupportMessageRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        check_client_key(&mut errors, "idempotency_key", &self.idempotency_key);
        check_text(&mut errors, "subject", &self.subject, 1, 200);
        check_text(&mut errors, "body", &self.body, 1, MAX_MESSAGE_CHARS);
        ApiError::check(errors)
    }
}

/// Response of `POST /api/support/messages`. Processing is asynchronous: poll the ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendSupportMessageResponse {
    /// The ticket the message went to.
    pub ticket_id: TicketId,
    /// The message's `Message-ID` (without angle brackets).
    pub message_id: String,
}

// ---------------------------------------------------------------------------------------------
// Classification and routing (stored on tickets, shown in admin)
// ---------------------------------------------------------------------------------------------

/// One intent's probability.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IntentProbability {
    /// Intent.
    pub intent: Intent,
    /// Probability in `0.0..=1.0`.
    pub p: f64,
}

labeled_enum! {
    /// Why the router escalated instead of dispatching.
    pub enum EscalationCause {
        /// Top intent below the org threshold.
        LowConfidence => ("low_confidence", "Low confidence"),
        /// Top intent too close to the runner-up.
        SmallMargin => ("small_margin", "Ambiguous intent"),
        /// The intent is `other`.
        OtherIntent => ("other_intent", "No matching route"),
        /// The customer asked for a person.
        WantsHuman => ("wants_human", "Asked for a person"),
        /// Legal threat.
        LegalThreat => ("legal_threat", "Legal threat"),
        /// The ticket is assigned to a person.
        HumanAssigned => ("human_assigned", "Assigned to a person"),
    }
}

/// The router's classification of a ticket's latest message (`tickets.classification_json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    /// Classifier model.
    pub model: String,
    /// Top intent.
    pub intent: Intent,
    /// Every intent's probability, most probable first.
    pub probabilities: Vec<IntentProbability>,
    /// `P(the message contains a request)`; "thanks!" closes the ticket without a reply.
    pub contains_request: f64,
    /// `P(the customer wants a person)`.
    pub wants_human: f64,
    /// `P(legal threat)`.
    pub legal_threat: f64,
    /// `P(the message answers our pending question)`, when the ticket was awaiting one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers_our_question: Option<f64>,
    /// Order numbers found by the regex.
    pub order_numbers: Vec<OrderNumber>,
    /// The threshold and margin that applied (org settings at the time).
    pub threshold: f64,
    /// Margin that applied.
    pub margin: f64,
    /// Why it was escalated; empty means it was dispatched.
    pub escalation: Vec<EscalationCause>,
    /// The trace of the classifier call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_run_id: Option<AgentRunId>,
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub classified_at: OffsetDateTime,
}

impl Classification {
    /// The top intent's probability.
    pub fn top_probability(&self) -> f64 {
        self.probabilities.first().map_or(0.0, |p| p.p)
    }
}

// ---------------------------------------------------------------------------------------------
// Reply confidence and guardrails (support.md §5–§6)
// ---------------------------------------------------------------------------------------------

/// The judge's three yes/no answers, as `P(yes)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct JudgeScores {
    /// Does the draft answer the question?
    pub answers_question: f64,
    /// Is every claim supported by the tool results?
    pub supported_by_tools: f64,
    /// Is the tone appropriate?
    pub tone_appropriate: f64,
}

impl JudgeScores {
    /// The judge score: the lowest of the three.
    pub fn score(&self) -> f64 {
        self.answers_question
            .min(self.supported_by_tools)
            .min(self.tone_appropriate)
    }
}

/// How the reply confidence was computed and what the reply policy did with it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReplyConfidence {
    /// `p(intent)` from triage.
    pub intent_p: f64,
    /// The judge's answers.
    pub judge: JudgeScores,
    /// `min(intent_p, judge.score())`.
    pub confidence: f64,
    /// The org's reply policy at the time.
    pub policy: ReplyPolicy,
    /// The threshold, for `send_above_confidence`.
    pub threshold: f64,
    /// Sent or held for review.
    pub disposition: ReplyDisposition,
}

labeled_enum! {
    /// A code guardrail run on an agent reply before anything is sent.
    pub enum Guardrail {
        /// Every product name, SKU and price exists in the catalog with the right price.
        Grounding => ("grounding", "Grounding"),
        /// Compatibility claims are backed by a matching `check_compatibility` call.
        CompatibilityClaims => ("compatibility_claims", "Compatibility claims"),
        /// Length, no internal IDs, links only to our domain.
        Format => ("format", "Format"),
    }
}

/// One guardrail result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardrailResult {
    /// Which guardrail.
    pub guardrail: Guardrail,
    /// Passed.
    pub passed: bool,
    /// Violations, fed back to the agent on retry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub violations: Vec<String>,
}

/// What an agent (or route) decided. Deterministic code acts on it: agents never send email or
/// move money themselves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentOutcome {
    /// Reply to the customer.
    Reply {
        /// The reply body (plain text / light Markdown).
        body: String,
    },
    /// Ask the customer for something and wait.
    AskCustomer {
        /// The question.
        question: String,
        /// What the answer should provide.
        expects: Awaiting,
    },
    /// Hand to a person.
    Escalate {
        /// Why.
        reason: String,
    },
    /// Send the ticket to the route for another intent (agents have no write tools).
    Reroute {
        /// The intent to route as.
        intent: Intent,
    },
}

impl AgentOutcome {
    /// The outcome's kind (the `agent_runs.outcome` column).
    pub const fn kind(&self) -> AgentOutcomeKind {
        match self {
            Self::Reply { .. } => AgentOutcomeKind::Reply,
            Self::AskCustomer { .. } => AgentOutcomeKind::AskCustomer,
            Self::Escalate { .. } => AgentOutcomeKind::Escalate,
            Self::Reroute { .. } => AgentOutcomeKind::Reroute,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Traces (admin "Trace" tab)
// ---------------------------------------------------------------------------------------------

/// A trace in a list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunSummary {
    /// Trace ID.
    pub id: AgentRunId,
    /// Which component.
    pub agent: AgentKind,
    /// Model.
    pub model: String,
    /// State.
    pub status: AgentRunStatus,
    /// Outcome kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AgentOutcomeKind>,
    /// Ticket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_id: Option<TicketId>,
    /// Investigation (reply interpretation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub investigation_id: Option<InvestigationId>,
    /// 1, or 2 for the guardrail retry.
    pub attempt: u32,
    /// Number of steps.
    pub step_count: u32,
    /// Prompt tokens.
    pub input_tokens: u32,
    /// Completion tokens.
    pub output_tokens: u32,
    /// Reply confidence, for agent replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_confidence: Option<f64>,
    /// Started.
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// Finished.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub finished_at: Option<OffsetDateTime>,
}

/// One step of a trace: an LLM call, a tool call, a classifier pass or a guardrail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStep {
    /// Position, from 0.
    pub idx: u32,
    /// Kind.
    pub kind: AgentStepKind,
    /// Model, tool or guardrail name.
    pub name: String,
    /// The `ctx.run` name, e.g. `llm-0` (journal cross-reference).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_name: Option<String>,
    /// Input (messages, tool arguments, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    /// Output (response, tool result, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    /// The step failed (errors go back to the model as tool results).
    pub is_error: bool,
    /// Prompt tokens.
    pub input_tokens: u32,
    /// Completion tokens.
    pub output_tokens: u32,
    /// Started.
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    /// Duration.
    pub duration_ms: u32,
}

/// A full trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunDetail {
    /// Header.
    pub run: AgentRunSummary,
    /// System prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// The outcome, for agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AgentOutcome>,
    /// Reply confidence breakdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<ReplyConfidence>,
    /// Guardrail results.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guardrails: Vec<GuardrailResult>,
    /// Failure message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Steps, in order.
    pub steps: Vec<AgentStep>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_wire_shape() {
        let outcome = AgentOutcome::AskCustomer {
            question: "Which order?".into(),
            expects: Awaiting::OrderNumber,
        };
        assert_eq!(outcome.kind(), AgentOutcomeKind::AskCustomer);
        assert_eq!(
            serde_json::to_value(&outcome).unwrap(),
            serde_json::json!({ "kind": "ask_customer", "question": "Which order?", "expects": "order_number" })
        );
    }

    #[test]
    fn judge_score_is_the_minimum() {
        let judge = JudgeScores {
            answers_question: 0.9,
            supported_by_tools: 0.7,
            tone_appropriate: 0.95,
        };
        assert!((judge.score() - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn web_message_validation() {
        let req = SendSupportMessageRequest {
            idempotency_key: "abcdefgh".into(),
            ticket_id: None,
            subject: "Where is my order?".into(),
            body: " ".into(),
        };
        assert_eq!(req.validate().unwrap_err().fields[0].field, "body");
    }
}
