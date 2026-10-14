//! The contract between `SupportTicket`, the `Router` and the routes (support.md §2–§4).
//!
//! ```text
//! SupportTicket ──Router/route(RouteTicketInput)──▶ Router ──<Route>/run(RouteRequest)──▶ route
//!       ▲            (one-way)                       │          (one-way, by name)          │
//!       └──── SupportTicket/classified (call) ───────┘                                      │
//!       └──────────────── SupportTicket/route_result(RouteResult) (one-way) ◀───────────────┘
//! ```
//!
//! - A route is a Restate **service** with one handler, `run(RouteRequest)`, named in
//!   [`route_target`]. It does its work (calling workflows such as `RefundWorkflow` or
//!   `Investigation`, or running an agent) and reports exactly one [`RouteOutcome`] back with
//!   `SupportTicket/route_result`, echoing the request's `generation`. The ticket ignores results
//!   of an older generation (a person took over meanwhile).
//! - Routes never send email and never touch the ticket's state: the outcome says *what* to tell
//!   the customer ([`Reply`]), the ticket decides *whether and how* (templates are always sent;
//!   AI-authored text goes through the guardrail result and the org's reply policy).
//! - A workflow that waits for the customer (`Investigation`'s offer) registers itself as the
//!   ticket's waiter with [`RouteOutcome::AskCustomer`] and an awakeable; the ticket resolves it
//!   with a [`CustomerReply`] on the next inbound message (or on timeout, or when a person takes
//!   over). Use a fresh awakeable per turn.
//! - Workflows that finish long after their route (refunds) report with `generation: None`:
//!   their reply is delivered whatever happened to the ticket since.
//!
//! Tenant scoping comes from here, not from the model: `user_id`, `org_id` and the order
//! candidates are filled by code (`Router` from D1), and [`RouteRequest::order`] is always one of
//! the customer's own orders. Order numbers that are not the customer's are listed in
//! [`RouteRequest::foreign_order_numbers`] only so a route can say it cannot help with them.

use restate_sdk::context::RequestTarget;
use serde::{Deserialize, Serialize};
use techdepo_api_types::AgentRunId;
use techdepo_api_types::support::{EscalationCause, GuardrailResult, JudgeScores};
use techdepo_api_types::vocab::{Awaiting, Direction, TicketRoute};
use techdepo_domain::{
    Actor, Intent, InvestigationId, OrderId, OrderNumber, OrderStatus, OrgId, PublicOrgId,
    RefundId, TicketId, UserId,
};
use time::OffsetDateTime;

use crate::notifications::templates::SupportTemplate;
use crate::support::RefundOutcome;

/// One of the customer's orders, as the router found it in D1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderBrief {
    /// Order ID.
    pub order_id: OrderId,
    /// Order number.
    pub number: OrderNumber,
    /// Status when the router looked.
    pub status: OrderStatus,
}

/// One message of the conversation, for routes and agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationMessage {
    /// Position in the ticket, from 1.
    pub seq: u32,
    /// From or to the customer.
    pub direction: Direction,
    /// Customer, AI, a person or the system.
    pub author: Actor,
    /// Subject.
    pub subject: String,
    /// Plain text (inbound: quoted history stripped).
    pub body: String,
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// A question a route asked and the ticket is waiting on (no workflow behind it): the next
/// customer message goes back to the same route if the classifier says it answers it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingQuestion {
    /// The route that asked.
    pub route: TicketRoute,
    /// The intent it was handling.
    pub intent: Intent,
    /// That intent's probability.
    pub intent_p: f64,
    /// What the answer should provide.
    pub expects: Awaiting,
}

/// `Router/route` input: a snapshot of the ticket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteTicketInput {
    /// Org.
    pub org_id: OrgId,
    /// Its public ID.
    pub public_org_id: PublicOrgId,
    /// Ticket.
    pub ticket_id: TicketId,
    /// The customer.
    pub user_id: UserId,
    /// Customer's address.
    pub customer_email: String,
    /// Customer's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_name: Option<String>,
    /// The ticket's routing generation; results of another generation are ignored.
    pub generation: u32,
    /// The conversation's subject.
    pub subject: String,
    /// The customer messages to handle now, oldest first.
    pub new_messages: Vec<ConversationMessage>,
    /// The conversation so far (most recent messages), oldest first.
    pub conversation: Vec<ConversationMessage>,
    /// The question the ticket waits on, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingQuestion>,
    /// Skip classification and route as this intent (an agent's `reroute`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forced_intent: Option<Intent>,
    /// The forced intent's probability (the previous classification's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forced_intent_p: Option<f64>,
    /// The order the ticket is already about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<OrderBrief>,
}

impl RouteTicketInput {
    /// The text to classify: the new messages' bodies, with the subject for a ticket's first
    /// message (later subjects are just `Re: …` of the first).
    pub fn text(&self) -> String {
        let first = self
            .conversation
            .iter()
            .all(|m| self.new_messages.iter().any(|n| n.seq == m.seq));
        let mut parts = Vec::new();
        if first && !self.subject.trim().is_empty() {
            parts.push(format!("Subject: {}", self.subject.trim()));
        }
        for m in &self.new_messages {
            let body = m.body.trim();
            if !body.is_empty() {
                parts.push(body.to_owned());
            }
        }
        if parts.is_empty() {
            parts.push(self.subject.trim().to_owned());
        }
        parts.join("\n\n")
    }
}

/// `Router/screen` input: customer messages that are not triaged now (support.md §8), checked
/// only for a request for a person or a legal threat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenInput {
    /// Org.
    pub org_id: OrgId,
    /// Ticket.
    pub ticket_id: TicketId,
    /// The messages screened.
    pub seqs: Vec<u32>,
    /// Their bodies, joined.
    pub text: String,
}

/// `<Route>/run` input: everything a route needs, filled by code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteRequest {
    /// Org.
    pub org_id: OrgId,
    /// Its public ID.
    pub public_org_id: PublicOrgId,
    /// Ticket (report back to `SupportTicket/<org>:<ticket>/route_result`).
    pub ticket_id: TicketId,
    /// Echo this in [`RouteResult::generation`].
    pub generation: u32,
    /// The customer.
    pub user_id: UserId,
    /// Customer's address.
    pub customer_email: String,
    /// Customer's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_name: Option<String>,
    /// This route.
    pub route: TicketRoute,
    /// The intent being handled.
    pub intent: Intent,
    /// Its probability from triage (the reply confidence starts from it).
    pub intent_p: f64,
    /// The conversation's subject.
    pub subject: String,
    /// The new customer text (all unhandled messages).
    pub message: String,
    /// The conversation so far, oldest first.
    pub conversation: Vec<ConversationMessage>,
    /// The order: one the customer named explicitly in this message, else the one the ticket is
    /// already about. Always the customer's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<OrderBrief>,
    /// Whether [`order`](Self::order) was named in the new message (routes that **act** on an
    /// order require an explicit number: support.md §3 "Missing information").
    pub order_explicit: bool,
    /// The customer's orders, newest first (cancelled ones excluded). Read-only routes may infer
    /// the order when there is exactly one, and must say which one they used.
    pub customer_orders: Vec<OrderBrief>,
    /// Order numbers mentioned that are not the customer's. Never act on them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_order_numbers: Vec<OrderNumber>,
    /// Set when this run continues the route's own question (`AskCustomer`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed: Option<Awaiting>,
}

/// An AI-authored reply (an agent's answer or question), with what the reply policy needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiReply {
    /// The text (plain, blank lines between paragraphs).
    pub body: String,
    /// `p(intent)` from triage.
    pub intent_p: f64,
    /// The judge's scores; absent means no judge ran, which never sends automatically under
    /// `send_above_confidence`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<JudgeScores>,
    /// The guardrail results. Any failure means the text is never sent (the ticket escalates).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guardrails: Vec<GuardrailResult>,
    /// The agent trace that wrote it.
    pub agent_run_id: AgentRunId,
}

impl AiReply {
    /// Every guardrail passed.
    pub fn guardrails_passed(&self) -> bool {
        self.guardrails.iter().all(|g| g.passed)
    }

    /// `min(p(intent), judge score)`; `0` without a judge.
    pub fn confidence(&self) -> f64 {
        let judge = self.judge.map_or(0.0, |j| j.score());
        self.intent_p.min(judge).clamp(0.0, 1.0)
    }
}

/// What to tell the customer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    /// A code-authored template: always sent (subject to the per-ticket rate limit).
    Template {
        /// The template and its facts.
        template: SupportTemplate,
    },
    /// AI-authored text: guardrails and the org's reply policy decide (support.md §6).
    Ai(AiReply),
}

impl Reply {
    /// A template reply.
    pub fn template(template: SupportTemplate) -> Self {
        Self::Template { template }
    }
}

/// A workflow a route started for the ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum WorkflowRef {
    /// `RefundWorkflow`.
    Refund(RefundId),
    /// `Investigation`.
    Investigation(InvestigationId),
}

/// What a route (or a workflow) decided.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteOutcome {
    /// Tell the customer; `resolve` closes the ticket afterwards, otherwise it stays with the
    /// route (a workflow will report again).
    Reply {
        /// The reply.
        reply: Reply,
        /// Resolve the ticket.
        resolve: bool,
    },
    /// Ask the customer and wait for the answer.
    AskCustomer {
        /// The question.
        question: Reply,
        /// What the answer should provide.
        expects: Awaiting,
        /// A waiting workflow's awakeable, resolved with a [`CustomerReply`]. Absent: the route
        /// is re-run when the answer comes (if it answers the question).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        awakeable_id: Option<String>,
    },
    /// A workflow carries on (refund approval, investigation); optionally tell the customer.
    WaitingOnWorkflow {
        /// The workflow.
        workflow: WorkflowRef,
        /// What to say now.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply: Option<Reply>,
    },
    /// Hand the ticket to a person.
    Escalate {
        /// Why, in words (shown on the task).
        reason: String,
        /// The router's causes, when it was the router.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        causes: Vec<EscalationCause>,
    },
    /// Nothing to answer ("thanks!"): resolve without replying.
    Close,
    /// Route the ticket as another intent (agents have no write tools).
    Reroute {
        /// The intent.
        intent: Intent,
    },
    /// The customer declined every offer of an investigation: hand the ticket to a person with
    /// an `offers_declined` task (the investigation is over).
    OffersDeclined {
        /// The investigation.
        investigation_id: InvestigationId,
        /// The customer's last reply, as an excerpt.
        last_reply: String,
    },
    /// A refund the ticket asked for finished (`RefundWorkflow` via `support::refund_finished`).
    RefundFinished {
        /// How it ended.
        outcome: RefundOutcome,
    },
}

/// `SupportTicket/route_result` input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteResult {
    /// Org.
    pub org_id: OrgId,
    /// The request's generation; `None` for a workflow reporting later (always applied).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u32>,
    /// The route that reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<TicketRoute>,
    /// The order the route worked on (linked to the ticket).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<OrderBrief>,
    /// The agent trace, if an agent decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_run_id: Option<AgentRunId>,
    /// What to do.
    pub outcome: RouteOutcome,
}

/// What a workflow waiting on the customer gets from its awakeable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CustomerReply {
    /// The customer answered.
    Message {
        /// The message (and any other unhandled ones, joined).
        message: ConversationMessage,
        /// Order numbers it mentions.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        order_numbers: Vec<OrderNumber>,
    },
    /// No answer after the reminder: the workflow decides (escalate).
    NoReply,
    /// A person took the ticket over or closed it: stop waiting.
    Withdrawn,
}

/// The Restate service behind a route, invoked by name (`<service>/run`), or `None` while the
/// route is not deployed (the router escalates instead). A route is a service with one handler:
/// `run(Json<RouteRequest>) -> ()`, reporting through `SupportTicket/route_result`.
pub const fn route_target(route: TicketRoute) -> Option<&'static str> {
    match route {
        TicketRoute::CancelOrder => Some(super::routes::CANCEL_ORDER),
        TicketRoute::WhereIsMyOrder => Some(super::routes::WHERE_IS_MY_ORDER),
        TicketRoute::BillingIssue => Some(super::routes::BILLING_ISSUE),
        TicketRoute::RefundRequest => Some(super::routes::REFUND_REQUEST),
        TicketRoute::ProductAdvisor => Some(crate::agents::PRODUCT_ADVISOR),
        TicketRoute::OrderAssistant => Some(crate::agents::ORDER_ASSISTANT),
        TicketRoute::Human => None,
    }
}

/// The handler every route service implements.
pub const ROUTE_HANDLER: &str = "run";

/// The request target of `route`'s service.
pub fn route_request_target(service: &'static str) -> RequestTarget {
    RequestTarget::service(service, ROUTE_HANDLER)
}

/// `SupportTicket/route_result` of `ticket`, for routes and workflows.
pub fn route_result_target(org: &OrgId, ticket: &TicketId) -> RequestTarget {
    RequestTarget::object(
        "SupportTicket",
        techdepo_api_types::ingress::keys::ticket(org, ticket),
        "route_result",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(seq: u32, body: &str) -> ConversationMessage {
        ConversationMessage {
            seq,
            direction: Direction::Inbound,
            author: Actor::Customer,
            subject: "Order".into(),
            body: body.into(),
            at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn input(
        conversation: Vec<ConversationMessage>,
        new: Vec<ConversationMessage>,
    ) -> RouteTicketInput {
        RouteTicketInput {
            org_id: OrgId::new("org_1"),
            public_org_id: PublicOrgId::parse("k3x9q2ab").unwrap(),
            ticket_id: TicketId::parse("T-7F2M9Q").unwrap(),
            user_id: UserId::new("user_1"),
            customer_email: "a@example.com".into(),
            customer_name: None,
            generation: 1,
            subject: "Cancel my order".into(),
            new_messages: new,
            conversation,
            pending: None,
            forced_intent: None,
            forced_intent_p: None,
            order: None,
        }
    }

    #[test]
    fn the_subject_counts_only_on_the_first_message() {
        let first = input(vec![msg(1, "please")], vec![msg(1, "please")]);
        assert_eq!(first.text(), "Subject: Cancel my order\n\nplease");
        let later = input(
            vec![msg(1, "please"), msg(2, "thanks!")],
            vec![msg(2, "thanks!")],
        );
        assert_eq!(later.text(), "thanks!");
    }

    #[test]
    fn ai_confidence_needs_a_judge() {
        let mut reply = AiReply {
            body: "Hi".into(),
            intent_p: 0.9,
            judge: None,
            guardrails: Vec::new(),
            agent_run_id: AgentRunId::new("run_1"),
        };
        assert_eq!(reply.confidence(), 0.0);
        reply.judge = Some(JudgeScores {
            answers_question: 0.95,
            supported_by_tools: 0.85,
            tone_appropriate: 0.99,
        });
        assert!((reply.confidence() - 0.85).abs() < 1e-9);
        assert!(reply.guardrails_passed());
    }
}
