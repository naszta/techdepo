//! The human tasks inbox: one queue for every decision a person has to make (support.md §8).
//!
//! `HumanTasks/decide` is the only path that resolves a task's awakeable, so the browser never
//! sees awakeable IDs. Only refund approvals and investigation decisions have one (a workflow
//! waits on them, see [`HumanTaskKind::has_waiter`]); reply reviews, escalations and declined
//! offers belong to a ticket, and their decision goes to `SupportTicket/task_decided`.

use serde::{Deserialize, Serialize};
use techdepo_domain::{
    HumanTaskId, Intent, InvestigationId, LostOutcome, Money, RefundId, TicketId, TrackingNumber,
    UserId,
};
use time::OffsetDateTime;

use crate::common::{CustomerRef, Evidence, OrderRef, TicketRef};
use crate::error::{ApiError, FieldError, Validate, check_text};
use crate::ids::AgentRunId;
use crate::support::{Classification, EscalationCause, MAX_MESSAGE_CHARS, ReplyConfidence};
use crate::vocab::{DecisionKind, HumanTaskKind, HumanTaskStatus, RefundReason};

/// Longest decision note, in characters.
pub const MAX_NOTE_CHARS: usize = 2_000;

/// `GET /api/admin/tasks` query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskListQuery {
    /// Status filter; defaults to `pending`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<HumanTaskStatus>,
    /// Kind filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<HumanTaskKind>,
    /// Page cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A task in the inbox list (oldest pending first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanTaskSummary {
    /// Task ID.
    pub id: HumanTaskId,
    /// Kind.
    pub kind: HumanTaskKind,
    /// Status.
    pub status: HumanTaskStatus,
    /// Title, e.g. `Approve €229.90 refund for TD-1042`.
    pub title: String,
    /// One-line summary.
    pub summary: String,
    /// Related order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<OrderRef>,
    /// Related ticket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<TicketRef>,
    /// Customer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer: Option<CustomerRef>,
    /// SLA deadline; the workflow nudges the admin when it passes and keeps waiting.
    #[serde(with = "time::serde::rfc3339")]
    pub sla_due_at: OffsetDateTime,
    /// The SLA has passed.
    pub overdue: bool,
    /// Created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Kind-specific context of a task (`human_tasks.context_json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskContext {
    /// Approve or reject a refund.
    ApproveRefund {
        /// The refund.
        refund_id: RefundId,
        /// Amount.
        amount: Money,
        /// Reason.
        reason: RefundReason,
        /// Free-text reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason_text: Option<String>,
    },
    /// Choose what to offer after a "no information" trace.
    InvestigationDecision {
        /// The investigation.
        investigation_id: InvestigationId,
        /// Tracking number.
        tracking_number: TrackingNumber,
        /// What the carrier said.
        carrier_result: LostOutcome,
        /// Order total, the most a refund can be.
        refundable: Money,
    },
    /// Review an AI-authored reply.
    ReviewReply {
        /// Subject of the draft.
        subject: String,
        /// The draft.
        draft: String,
        /// Confidence breakdown.
        confidence: ReplyConfidence,
        /// The agent trace that wrote it.
        agent_run_id: AgentRunId,
    },
    /// A ticket handed to a person.
    Escalation {
        /// Why, in words.
        reason: String,
        /// The router's causes, when the router escalated.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        causes: Vec<EscalationCause>,
        /// The classification, when there is one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        classification: Option<Classification>,
        /// The trace of the agent that escalated, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_run_id: Option<AgentRunId>,
        /// A new customer message arrived while a person had the ticket.
        #[serde(default)]
        new_customer_message: bool,
    },
    /// The customer declined every offer.
    OffersDeclined {
        /// The investigation.
        investigation_id: InvestigationId,
        /// The customer's last reply, as an excerpt.
        last_reply: String,
    },
}

impl TaskContext {
    /// The task kind this context belongs to.
    pub const fn kind(&self) -> HumanTaskKind {
        match self {
            Self::ApproveRefund { .. } => HumanTaskKind::ApproveRefund,
            Self::InvestigationDecision { .. } => HumanTaskKind::InvestigationDecision,
            Self::ReviewReply { .. } => HumanTaskKind::ReviewReply,
            Self::Escalation { .. } => HumanTaskKind::Escalation,
            Self::OffersDeclined { .. } => HumanTaskKind::OffersDeclined,
        }
    }
}

/// A decision with its details (`human_tasks.decision_json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HumanDecision {
    /// Approve the refund.
    Approve,
    /// Reject the refund; the reason goes into the customer's email.
    Reject {
        /// Why.
        reason: String,
    },
    /// Offer a refund.
    OfferRefund,
    /// Offer a replacement.
    OfferReship,
    /// Offer both.
    OfferBoth,
    /// Send this reply instead of an offer.
    CustomReply {
        /// Reply body.
        body: String,
    },
    /// Send the AI draft as is.
    Send,
    /// Send this edited version.
    EditAndSend {
        /// Edited body.
        body: String,
    },
    /// Discard the draft; the ticket goes to a person.
    Discard,
    /// Reply to the customer.
    Reply {
        /// Reply body.
        body: String,
        /// Resolve the ticket after replying.
        #[serde(default)]
        close: bool,
    },
    /// Hand the ticket back to the AI: the unanswered messages are routed again, under
    /// `intent` when the person gave one.
    HandBack {
        /// What the customer wants (never `other`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intent: Option<Intent>,
    },
    /// Close the ticket without replying.
    Close,
}

/// Whether a ticket can be handed back to the AI under `intent`: `other` has no route.
pub const fn hand_back_intent(intent: Intent) -> bool {
    !matches!(intent, Intent::Other)
}

/// Checks a hand-back's intent.
pub(crate) fn check_hand_back(errors: &mut Vec<FieldError>, field: &str, intent: Option<Intent>) {
    if intent.is_some_and(|i| !hand_back_intent(i)) {
        errors.push(FieldError::new(field, "Pick what the customer wants"));
    }
}

impl HumanDecision {
    /// The decision's kind.
    pub const fn kind(&self) -> DecisionKind {
        match self {
            Self::Approve => DecisionKind::Approve,
            Self::Reject { .. } => DecisionKind::Reject,
            Self::OfferRefund => DecisionKind::OfferRefund,
            Self::OfferReship => DecisionKind::OfferReship,
            Self::OfferBoth => DecisionKind::OfferBoth,
            Self::CustomReply { .. } => DecisionKind::CustomReply,
            Self::Send => DecisionKind::Send,
            Self::EditAndSend { .. } => DecisionKind::EditAndSend,
            Self::Discard => DecisionKind::Discard,
            Self::Reply { .. } => DecisionKind::Reply,
            Self::HandBack { .. } => DecisionKind::HandBack,
            Self::Close => DecisionKind::Close,
        }
    }

    /// Text the decision sends to the customer, if any.
    pub fn body(&self) -> Option<&str> {
        match self {
            Self::Reject { reason } => Some(reason),
            Self::CustomReply { body } | Self::EditAndSend { body } | Self::Reply { body, .. } => {
                Some(body)
            }
            _ => None,
        }
    }
}

/// The recorded decision of a decided task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedDecision {
    /// The decision.
    pub decision: HumanDecision,
    /// Free-text note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Who decided.
    pub decided_by: UserId,
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub decided_at: OffsetDateTime,
}

/// `GET /api/admin/tasks/{id}`: the task with its context, evidence and allowed decisions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanTaskDetail {
    /// Header.
    pub task: HumanTaskSummary,
    /// Kind-specific context.
    pub context: TaskContext,
    /// Evidence gathered by code.
    pub evidence: Vec<Evidence>,
    /// The decisions offered.
    pub allowed_decisions: Vec<DecisionKind>,
    /// Related ticket ID (for deep links even when the ticket summary is absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_id: Option<TicketId>,
    /// The decision, once decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<RecordedDecision>,
}

/// `POST /api/admin/tasks/{id}/decide`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecideTaskRequest {
    /// The decision; its kind must be one of the task's allowed decisions.
    pub decision: HumanDecision,
    /// Free-text note for the record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl DecideTaskRequest {
    /// Validates the request against the task's allowed decisions.
    pub fn validate_for(&self, kind: HumanTaskKind) -> Result<(), ApiError> {
        self.validate()?;
        if kind.allowed_decisions().contains(&self.decision.kind()) {
            Ok(())
        } else {
            Err(ApiError::invalid_field(
                "decision.kind",
                format!(
                    "`{}` is not a decision for this task",
                    self.decision.kind().as_str()
                ),
            ))
        }
    }
}

impl Validate for DecideTaskRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors: Vec<FieldError> = Vec::new();
        if let Some(body) = self.decision.body() {
            let field = match self.decision {
                HumanDecision::Reject { .. } => "decision.reason",
                _ => "decision.body",
            };
            check_text(&mut errors, field, body, 1, MAX_MESSAGE_CHARS);
        }
        if let HumanDecision::HandBack { intent } = self.decision {
            check_hand_back(&mut errors, "decision.intent", intent);
        }
        if let Some(note) = &self.note {
            check_text(&mut errors, "note", note, 0, MAX_NOTE_CHARS);
        }
        ApiError::check(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_wire_shape_and_kind() {
        let d: HumanDecision =
            serde_json::from_str(r#"{"kind":"reject","reason":"Outside policy"}"#).unwrap();
        assert_eq!(d.kind(), DecisionKind::Reject);
        let send: HumanDecision = serde_json::from_str(r#"{"kind":"send"}"#).unwrap();
        assert_eq!(send, HumanDecision::Send);
        // Every DecisionKind has a HumanDecision variant with the same wire name.
        for kind in DecisionKind::ALL {
            let json = serde_json::json!({ "kind": kind.as_str(), "reason": "x", "body": "x" });
            let decision: HumanDecision = serde_json::from_value(json).unwrap();
            assert_eq!(decision.kind(), *kind);
        }
    }

    #[test]
    fn decide_checks_allowed_decisions() {
        let req = DecideTaskRequest {
            decision: HumanDecision::Approve,
            note: None,
        };
        assert!(req.validate_for(HumanTaskKind::ApproveRefund).is_ok());
        assert!(req.validate_for(HumanTaskKind::ReviewReply).is_err());
        let empty = DecideTaskRequest {
            decision: HumanDecision::Reject {
                reason: String::new(),
            },
            note: None,
        };
        assert_eq!(
            empty.validate().unwrap_err().fields[0].field,
            "decision.reason"
        );
    }

    #[test]
    fn hand_back_takes_an_optional_intent() {
        let plain: HumanDecision = serde_json::from_str(r#"{"kind":"hand_back"}"#).unwrap();
        assert_eq!(plain, HumanDecision::HandBack { intent: None });
        let billing: HumanDecision =
            serde_json::from_str(r#"{"kind":"hand_back","intent":"billing_problem"}"#).unwrap();
        assert_eq!(
            billing,
            HumanDecision::HandBack {
                intent: Some(Intent::BillingProblem)
            }
        );
        assert_eq!(
            serde_json::to_string(&plain).unwrap(),
            r#"{"kind":"hand_back"}"#
        );
        let other = DecideTaskRequest {
            decision: HumanDecision::HandBack {
                intent: Some(Intent::Other),
            },
            note: None,
        };
        assert_eq!(
            other.validate().unwrap_err().fields[0].field,
            "decision.intent"
        );
    }
}
