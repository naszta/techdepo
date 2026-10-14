//! The support pipeline (support.md): intake lands in [`ticket::SupportTicket`], the
//! [`router::Router`] classifies and dispatches to the routes, which report back through the
//! [`contract`].
//!
//! Commerce-side components call the ticket through the functions here only:
//!
//! - [`task_decided`]: `HumanTasks/decide` forwards a ticket-owned task's decision (reply review,
//!   escalation, offers declined) to `SupportTicket/task_decided`.
//! - [`refund_finished`]: `RefundWorkflow` reports its outcome to the ticket that asked for the
//!   refund (`SupportTicket/route_result` with `RefundFinished`); the ticket emails the customer
//!   in the ticket's thread (`refund_done` / `refund_rejected`).
//! - [`refund_failed`]: a `RefundWorkflow` that failed terminally hands its ticket to a person
//!   (`route_result` with `Escalate`), so the ticket never buffers behind a dead workflow.

pub mod contract;
pub mod conversation;
pub mod router;
pub mod routes;
pub mod ticket;

use restate_sdk::context::RequestTarget;
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::ingress::keys;
use techdepo_api_types::tasks::HumanDecision;
use techdepo_api_types::vocab::{HumanTaskKind, RefundStatus};
use techdepo_domain::{HumanTaskId, Money, OrgId, RefundId, TicketId, UserId};

/// `SupportTicket/task_decided` input: a person decided a ticket-owned task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDecidedInput {
    /// Org.
    pub org_id: OrgId,
    /// Task.
    pub task_id: HumanTaskId,
    /// Its kind (`review_reply`, `escalation`, `offers_declined`).
    pub kind: HumanTaskKind,
    /// The decision.
    pub decision: HumanDecision,
    /// Note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Who decided.
    pub by: UserId,
}

/// Hands a ticket-owned task's decision to its ticket (one-way).
pub fn task_decided(ctx: &Context<'_>, org: &OrgId, ticket: &TicketId, input: TaskDecidedInput) {
    let target = RequestTarget::object("SupportTicket", keys::ticket(org, ticket), "task_decided");
    let _ = ctx.request::<_, ()>(target, Json(input)).send();
}

/// How a refund ended, for the ticket that asked for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundOutcome {
    /// Refund.
    pub refund_id: RefundId,
    /// `completed` or `rejected`.
    pub status: RefundStatus,
    /// Amount.
    pub amount: Money,
    /// The rejection reason (goes into the customer's email).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Reports a refund's outcome to `ticket` (one-way). Returns whether the ticket takes care of
/// telling the customer (always: it emails `refund_done` / `refund_rejected` in its thread).
pub fn refund_finished(
    ctx: &WorkflowContext<'_>,
    org: &OrgId,
    ticket: &TicketId,
    outcome: &RefundOutcome,
) -> bool {
    let result = contract::RouteResult {
        org_id: org.clone(),
        generation: None,
        route: None,
        order: None,
        agent_run_id: None,
        outcome: contract::RouteOutcome::RefundFinished {
            outcome: outcome.clone(),
        },
    };
    let _ = ctx
        .request::<_, ()>(contract::route_result_target(org, ticket), Json(result))
        .send();
    true
}

/// Reports a refund that failed for good to `ticket` (one-way): the ticket escalates.
pub fn refund_failed(ctx: &WorkflowContext<'_>, org: &OrgId, ticket: &TicketId, reason: String) {
    let result = contract::RouteResult {
        org_id: org.clone(),
        generation: None,
        route: None,
        order: None,
        agent_run_id: None,
        outcome: contract::RouteOutcome::Escalate {
            reason,
            causes: Vec::new(),
        },
    };
    let _ = ctx
        .request::<_, ()>(contract::route_result_target(org, ticket), Json(result))
        .send();
}
