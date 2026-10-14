//! The ticket's conversation state ([`TicketData`]) and the pure decisions on it: where new
//! messages go ([`TicketData::next_step`]), what a routing generation hands the router
//! ([`TicketData::start_routing`]), the reply rate limit, and the `tickets` row it mirrors.
//! `SupportTicket` ([`super::ticket`]) runs the effects.

use serde::{Deserialize, Serialize};
use techdepo_api_types::AgentRunId;
use techdepo_api_types::ingress::InboundMessage;
use techdepo_api_types::support::Classification;
use techdepo_api_types::vocab::{Awaiting, Direction, HumanTaskKind, TicketRoute};
use techdepo_db::tickets::TicketRecord;
use techdepo_domain::{
    Actor, Assignee, Channel, HumanTaskId, Intent, OrderNumber, OrgId, PublicOrgId, TicketId,
    TicketStatus, UserId,
};
use techdepo_ports::Mailbox;
use time::OffsetDateTime;

use super::contract::{
    ConversationMessage, CustomerReply, OrderBrief, PendingQuestion, RouteTicketInput, ScreenInput,
    WorkflowRef,
};
use crate::notifications::TicketNoticeInput;
use crate::notifications::templates::{SupportTemplate, TicketFacts};

/// Automatic replies (templates and AI) per ticket and rolling hour; more escalate (support.md
/// §9 loop protection). A person's replies do not count.
pub const MAX_AUTO_REPLIES_PER_HOUR: usize = 6;
/// Longest message body kept in the object's state (the full text is in D1).
pub(super) const MAX_BODY_CHARS: usize = 4_000;
/// Messages of the conversation handed to the router and routes.
const CONVERSATION_WINDOW: usize = 30;
/// A message as the object remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageMeta {
    /// The message.
    #[serde(flatten)]
    pub message: ConversationMessage,
    /// `Message-ID` (inbound, or outbound once sent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

/// Someone waits for the customer's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Waiter {
    /// A workflow's awakeable (resolved with a [`CustomerReply`]); absent: the route that asked
    /// is re-run with the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awakeable_id: Option<String>,
    /// What the answer should provide.
    pub expects: Awaiting,
    /// The route that asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<TicketRoute>,
    /// The intent it handled, and its probability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<(Intent, f64)>,
    /// The reminder went out.
    #[serde(default)]
    pub reminded: bool,
    /// The first message number that answers the question: messages received before it was
    /// sent (buffered while the route or workflow worked) are not answers to it.
    #[serde(default)]
    pub answers_from: u32,
}

/// What happens after a reviewed draft is sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AfterSend {
    /// Resolve the ticket.
    pub resolve: bool,
    /// Wait for the customer (the draft was a question).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiter: Option<Waiter>,
}

/// An AI draft held for review (`review_reply` task).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    /// The review task.
    pub task_id: HumanTaskId,
    /// Its message number.
    pub seq: u32,
    /// The draft text.
    pub body: String,
    /// The trace that wrote it.
    pub agent_run_id: AgentRunId,
    /// What sending it leads to.
    pub after: AfterSend,
}

/// A pending ticket-owned human task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenTask {
    /// Task.
    pub id: HumanTaskId,
    /// Kind (`escalation`, `review_reply`, `offers_declined`).
    pub kind: HumanTaskKind,
}

/// The object's state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TicketData {
    /// Org.
    pub org_id: OrgId,
    /// Its public ID (reply addresses).
    pub public_org_id: PublicOrgId,
    /// Ticket.
    pub ticket_id: TicketId,
    /// The customer.
    pub user_id: UserId,
    /// Customer's address.
    pub customer_email: String,
    /// Customer's name.
    pub customer_name: Option<String>,
    /// Subject of the first message.
    pub subject: String,
    /// Channel of the first message.
    pub channel: Channel,
    /// Status.
    pub status: TicketStatus,
    /// AI or a person.
    pub assignee: Assignee,
    /// The conversation (drafts are added once sent).
    pub messages: Vec<MessageMeta>,
    /// Next message number.
    pub next_seq: u32,
    /// Routing generation: bumped when routing starts or a person acts; older results are stale.
    pub generation: u32,
    /// A route of the current generation is working.
    pub route_active: bool,
    /// Inbound messages the current route handles.
    pub routing: Vec<u32>,
    /// Inbound messages not handled yet (the buffer).
    pub unhandled: Vec<u32>,
    /// Latest classification.
    pub classification: Option<Classification>,
    /// Intent being handled, and its probability.
    pub intent: Option<(Intent, f64)>,
    /// Route.
    pub route: Option<TicketRoute>,
    /// The order it is about.
    pub order: Option<OrderBrief>,
    /// The order number mentioned, even if not one of the customer's.
    pub order_number: Option<OrderNumber>,
    /// Who waits for the customer.
    pub waiter: Option<Waiter>,
    /// Workflows working for the ticket.
    pub workflows: Vec<WorkflowRef>,
    /// Timer generation: a `timeout` of another generation is stale.
    pub timer_gen: u32,
    /// Automatic replies in the last hour (epoch ms).
    pub auto_replies: Vec<i64>,
    /// Pending ticket-owned tasks.
    pub open_tasks: Vec<OpenTask>,
    /// Drafts held for review.
    pub drafts: Vec<Draft>,
    /// Agent reroutes since the last customer message.
    pub reroutes: u32,
    /// Why it was escalated.
    pub escalation_reason: Option<String>,
    /// Latest AI reply confidence.
    pub reply_confidence: Option<f64>,
    /// Highest message number already screened for a request for a person or a legal threat.
    #[serde(default)]
    pub screened_through: u32,
    /// Our latest sent reply that answered the customer (the escalation acknowledgement answers
    /// nothing): customer messages after it are unanswered, which is what a hand-back routes.
    #[serde(default)]
    pub answered_through: u32,
    /// Opened.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// Latest message.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_message_at: Option<OffsetDateTime>,
    /// Resolved.
    #[serde(with = "time::serde::rfc3339::option")]
    pub resolved_at: Option<OffsetDateTime>,
}

/// What to do with the unhandled messages.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// Nothing to handle.
    Nothing,
    /// Keep them buffered (a route, a workflow or a review is under way).
    Buffer,
    /// Classify and route them.
    Route,
    /// Hand them to the waiting workflow.
    Wake {
        /// Its awakeable.
        awakeable_id: String,
        /// The answer.
        reply: CustomerReply,
    },
    /// A person has the ticket: make sure they have a task about it.
    NotifyHuman,
}

/// [`TicketData::next_step`]'s answer: what to do, and which messages to screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// What to do with the unhandled messages.
    pub next: Next,
    /// Messages that are not triaged now (buffered behind a workflow, or superseded by a
    /// question sent after them): the router screens them for a request for a person or a
    /// legal threat (`Router/screen`), which escalate right away (support.md §8).
    pub screen: Vec<u32>,
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_owned(),
    }
}

impl TicketData {
    /// A new ticket opened by `msg`.
    pub fn open(msg: &InboundMessage, at: OffsetDateTime) -> Self {
        Self {
            org_id: msg.org_id.clone(),
            public_org_id: msg.public_org_id.clone(),
            ticket_id: msg.ticket_id.clone(),
            user_id: msg.user_id.clone(),
            customer_email: msg.customer_email.to_lowercase(),
            customer_name: msg.customer_name.clone(),
            subject: msg.subject.trim().to_owned(),
            channel: msg.channel,
            status: TicketStatus::New,
            assignee: Assignee::Ai,
            messages: Vec::new(),
            next_seq: 1,
            generation: 0,
            route_active: false,
            routing: Vec::new(),
            unhandled: Vec::new(),
            classification: None,
            intent: None,
            route: None,
            order: None,
            order_number: None,
            waiter: None,
            workflows: Vec::new(),
            timer_gen: 0,
            auto_replies: Vec::new(),
            open_tasks: Vec::new(),
            drafts: Vec::new(),
            reroutes: 0,
            escalation_reason: None,
            reply_confidence: None,
            screened_through: 0,
            answered_through: 0,
            created_at: at,
            updated_at: at,
            last_message_at: None,
            resolved_at: None,
        }
    }

    /// Whether `message_id` is already in the conversation.
    pub fn has_message(&self, message_id: &str) -> bool {
        self.messages
            .iter()
            .any(|m| m.message_id.as_deref() == Some(message_id))
    }

    /// Appends an inbound message; returns its `seq`.
    pub fn push_inbound(&mut self, msg: &InboundMessage) -> u32 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.messages.push(MessageMeta {
            message: ConversationMessage {
                seq,
                direction: Direction::Inbound,
                author: Actor::Customer,
                subject: msg.subject.clone(),
                body: truncate(&msg.body_text, MAX_BODY_CHARS),
                at: msg.received_at,
            },
            message_id: Some(msg.message_id.clone()),
        });
        self.unhandled.push(seq);
        self.reroutes = 0;
        self.last_message_at = Some(msg.received_at);
        seq
    }

    /// What to do with the unhandled messages now (support.md §2 `inbound`; also how the buffer
    /// is drained after a route reports), and which of them to screen.
    pub fn next_step(&mut self) -> Step {
        let mut screen = self.supersede();
        let next = self.next_action();
        let behind_workflow = next == Next::Buffer
            && !self.route_active
            && self.drafts.is_empty()
            && self.status == TicketStatus::WithRoute
            && !self.workflows.is_empty();
        if behind_workflow {
            screen.extend(self.unhandled.iter().copied());
        }
        screen.sort_unstable();
        screen.dedup();
        screen.retain(|seq| *seq > self.screened_through);
        if let Some(last) = screen.last() {
            self.screened_through = *last;
        }
        Step { next, screen }
    }

    /// Drops the unhandled messages a question to the customer was sent after: they were
    /// buffered while the route or workflow worked, the question supersedes them (they stay in
    /// the conversation), and they are no answer to it. Returns them (to be screened).
    fn supersede(&mut self) -> Vec<u32> {
        let waiting = self.assignee == Assignee::Ai
            && !self.route_active
            && self.drafts.is_empty()
            && self.status == TicketStatus::AwaitingCustomer;
        let Some(from) = self
            .waiter
            .as_ref()
            .filter(|_| waiting)
            .map(|w| w.answers_from)
        else {
            return Vec::new();
        };
        let (older, answers) = self.unhandled.iter().partition(|seq| **seq < from);
        self.unhandled = answers;
        older
    }

    fn next_action(&mut self) -> Next {
        if self.unhandled.is_empty() {
            return Next::Nothing;
        }
        if self.assignee == Assignee::Human {
            self.status = TicketStatus::WithHuman;
            self.resolved_at = None;
            self.unhandled.clear();
            return Next::NotifyHuman;
        }
        if self.route_active || !self.drafts.is_empty() {
            return Next::Buffer;
        }
        match self.status {
            TicketStatus::AwaitingCustomer => {
                match self.waiter.as_ref().and_then(|w| w.awakeable_id.clone()) {
                    Some(awakeable_id) => {
                        let reply = self.take_unhandled_reply();
                        self.waiter = None;
                        self.timer_gen += 1;
                        self.status = TicketStatus::WithRoute;
                        Next::Wake {
                            awakeable_id,
                            reply,
                        }
                    }
                    None => Next::Route,
                }
            }
            TicketStatus::WithRoute if !self.workflows.is_empty() => Next::Buffer,
            _ => Next::Route,
        }
    }

    /// The screening request for messages `seqs`.
    pub fn screen_request(&self, seqs: &[u32]) -> ScreenInput {
        let text = self
            .messages
            .iter()
            .map(|m| &m.message)
            .filter(|m| seqs.contains(&m.seq))
            .map(|m| m.body.trim())
            .collect::<Vec<_>>()
            .join("\n\n");
        ScreenInput {
            org_id: self.org_id.clone(),
            ticket_id: self.ticket_id.clone(),
            seqs: seqs.to_vec(),
            text,
        }
    }

    /// The unhandled messages joined into one reply for a waiting workflow.
    fn take_unhandled_reply(&mut self) -> CustomerReply {
        let seqs = std::mem::take(&mut self.unhandled);
        let parts: Vec<&ConversationMessage> = self
            .messages
            .iter()
            .map(|m| &m.message)
            .filter(|m| seqs.contains(&m.seq))
            .collect();
        let body = parts
            .iter()
            .map(|m| m.body.trim())
            .collect::<Vec<_>>()
            .join("\n\n");
        let last = parts.last().map(|m| (*m).clone());
        let message = ConversationMessage {
            body: body.clone(),
            ..last.unwrap_or(ConversationMessage {
                seq: 0,
                direction: Direction::Inbound,
                author: Actor::Customer,
                subject: self.subject.clone(),
                body: String::new(),
                at: self.updated_at,
            })
        };
        CustomerReply::Message {
            order_numbers: OrderNumber::find_all(&body),
            message,
        }
    }

    /// Starts a routing generation for the unhandled (and in-flight) messages.
    pub fn start_routing(&mut self, forced: Option<(Intent, f64)>) -> RouteTicketInput {
        self.generation += 1;
        self.status = TicketStatus::Routing;
        self.route_active = true;
        self.timer_gen += 1;
        self.resolved_at = None;
        let pending = self
            .waiter
            .take()
            .filter(|w| w.awakeable_id.is_none())
            .and_then(|w| {
                let (intent, intent_p) = w.intent?;
                Some(PendingQuestion {
                    route: w.route?,
                    intent,
                    intent_p,
                    expects: w.expects,
                })
            });
        let unhandled = std::mem::take(&mut self.unhandled);
        self.routing.extend(unhandled);
        let new_messages = self
            .messages
            .iter()
            .filter(|m| self.routing.contains(&m.message.seq))
            .map(|m| m.message.clone())
            .collect();
        let start = self.messages.len().saturating_sub(CONVERSATION_WINDOW);
        RouteTicketInput {
            org_id: self.org_id.clone(),
            public_org_id: self.public_org_id.clone(),
            ticket_id: self.ticket_id.clone(),
            user_id: self.user_id.clone(),
            customer_email: self.customer_email.clone(),
            customer_name: self.customer_name.clone(),
            generation: self.generation,
            subject: self.subject.clone(),
            new_messages,
            conversation: self.messages[start..]
                .iter()
                .map(|m| m.message.clone())
                .collect(),
            pending,
            forced_intent: forced.map(|(i, _)| i),
            forced_intent_p: forced.map(|(_, p)| p),
            order: self.order.clone(),
        }
    }

    /// The customer's messages after our last answer: what nobody answered yet.
    fn unanswered(&self) -> Vec<u32> {
        self.messages
            .iter()
            .map(|m| &m.message)
            .filter(|m| m.direction == Direction::Inbound && m.seq > self.answered_through)
            .map(|m| m.seq)
            .collect()
    }

    /// A person hands the ticket back to the AI (support.md §2 "Hand back"): the unanswered
    /// messages are routed again at once, under the person's `intent` (forced, `p = 1`) or
    /// through the classifier. With nothing unanswered, or on a resolved ticket, it waits for the
    /// customer's next message (`New`). The caller already withdrew the waiter.
    pub fn hand_back(&mut self, intent: Option<Intent>) -> Option<RouteTicketInput> {
        self.assignee = Assignee::Ai;
        self.escalation_reason = None;
        self.workflows.clear();
        if self.status == TicketStatus::Resolved {
            return None;
        }
        self.unhandled = self.unanswered();
        if self.unhandled.is_empty() {
            self.status = TicketStatus::New;
            return None;
        }
        self.reroutes = 0;
        Some(self.start_routing(intent.map(|i| (i, 1.0))))
    }

    /// Whether another automatic reply fits the hourly limit (prunes the window).
    pub fn may_auto_reply(&mut self, at: OffsetDateTime) -> bool {
        let now = at.unix_timestamp() * 1000;
        self.auto_replies.retain(|t| now - t < 3_600_000);
        self.auto_replies.len() < MAX_AUTO_REPLIES_PER_HOUR
    }

    /// The message IDs of the thread, oldest first.
    pub(super) fn references(&self) -> Vec<String> {
        self.messages
            .iter()
            .filter_map(|m| m.message_id.clone())
            .collect()
    }

    /// The customer's latest message ID.
    pub(super) fn last_inbound_id(&self) -> Option<String> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.message.direction == Direction::Inbound)
            .and_then(|m| m.message_id.clone())
    }

    /// `Notifications/ticket` input for message `seq`.
    pub(super) fn notice(&self, seq: u32, template: SupportTemplate) -> TicketNoticeInput {
        TicketNoticeInput {
            org_id: self.org_id.clone(),
            ticket: TicketFacts {
                ticket_id: self.ticket_id.clone(),
                subject: self.subject.clone(),
                customer_name: self.customer_name.clone(),
            },
            to: Mailbox {
                email: self.customer_email.clone(),
                name: self.customer_name.clone(),
            },
            user_id: Some(self.user_id.clone()),
            seq,
            in_reply_to: self.last_inbound_id(),
            references: self.references(),
            order_id: self.order.as_ref().map(|o| o.order_id.clone()),
            template,
        }
    }

    /// The `tickets` row.
    pub fn record(&self) -> TicketRecord {
        TicketRecord {
            id: self.ticket_id.clone(),
            user_id: self.user_id.clone(),
            customer_email: self.customer_email.clone(),
            subject: self.subject.clone(),
            channel: self.channel,
            status: self.status,
            assignee: self.assignee,
            order_id: self.order.as_ref().map(|o| o.order_id.clone()),
            order_number: self.order.as_ref().map(|o| o.number).or(self.order_number),
            intent: self.intent.map(|(i, _)| i),
            intent_confidence: self.intent.map(|(_, p)| p),
            classification: self.classification.clone(),
            route: self.route,
            awaiting: match (self.status, &self.waiter) {
                (TicketStatus::AwaitingCustomer, Some(w)) => Some(w.expects),
                _ => None,
            },
            escalation_reason: self.escalation_reason.clone(),
            reply_confidence: self.reply_confidence,
            message_count: self.next_seq.saturating_sub(1),
            last_message_at: self.last_message_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
            resolved_at: self.resolved_at,
        }
    }

    /// Resolves the ticket.
    pub(super) fn resolve(&mut self, at: OffsetDateTime) {
        self.status = TicketStatus::Resolved;
        self.resolved_at = Some(at);
        self.waiter = None;
        self.workflows.clear();
        self.timer_gen += 1;
    }

    /// The state without awakeable IDs (they never leave the server).
    pub fn redacted(&self) -> Self {
        let mut s = self.clone();
        if let Some(w) = &mut s.waiter {
            w.awakeable_id = w.awakeable_id.as_ref().map(|_| "<redacted>".to_owned());
        }
        for d in &mut s.drafts {
            if let Some(w) = &mut d.after.waiter {
                w.awakeable_id = w.awakeable_id.as_ref().map(|_| "<redacted>".to_owned());
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    const AT: OffsetDateTime = datetime!(2026-10-14 10:00 UTC);

    fn inbound(message_id: &str, body: &str) -> InboundMessage {
        InboundMessage {
            org_id: OrgId::new("org_1"),
            public_org_id: PublicOrgId::parse("k3x9q2ab").unwrap(),
            ticket_id: TicketId::parse("T-7F2M9Q").unwrap(),
            user_id: UserId::new("user_1"),
            customer_email: "Ada@Example.com".into(),
            customer_name: Some("Ada".into()),
            channel: Channel::Email,
            message_id: message_id.into(),
            in_reply_to: None,
            references: Vec::new(),
            subject: "Cancel my order".into(),
            body_text: body.into(),
            received_at: AT,
            email_log_id: None,
        }
    }

    fn ticket() -> TicketData {
        let msg = inbound("m1@x", "Please cancel TD-1042");
        let mut s = TicketData::open(&msg, AT);
        s.push_inbound(&msg);
        s
    }

    #[test]
    fn a_new_message_is_routed_and_later_ones_are_buffered() {
        let mut s = ticket();
        assert_eq!(s.customer_email, "ada@example.com");
        assert_eq!(s.next_step().next, Next::Route);
        let route = s.start_routing(None);
        assert_eq!(route.generation, 1);
        assert_eq!(route.new_messages.len(), 1);
        assert_eq!(s.status, TicketStatus::Routing);
        assert!(s.unhandled.is_empty());

        s.push_inbound(&inbound("m2@x", "also, hurry"));
        assert!(s.has_message("m2@x"));
        assert_eq!(s.next_step().next, Next::Buffer, "a route is in flight");

        // The route reports: the buffer is routed next, as one generation.
        s.route_active = false;
        s.routing.clear();
        s.status = TicketStatus::Resolved;
        assert_eq!(s.next_step().next, Next::Route);
        let again = s.start_routing(None);
        assert_eq!(again.generation, 2);
        assert_eq!(again.new_messages[0].body, "also, hurry");
        assert_eq!(again.conversation.len(), 2);
        assert!(s.resolved_at.is_none(), "a reply reopens the ticket");
    }

    #[test]
    fn a_waiting_workflow_gets_the_answer() {
        let mut s = ticket();
        s.start_routing(None);
        s.route_active = false;
        s.routing.clear();
        s.status = TicketStatus::AwaitingCustomer;
        s.waiter = Some(Waiter {
            awakeable_id: Some("awk_1".into()),
            expects: Awaiting::OfferReply,
            route: Some(TicketRoute::WhereIsMyOrder),
            intent: Some((Intent::OrderStatus, 0.9)),
            reminded: false,
            answers_from: 0,
        });
        s.workflows.push(WorkflowRef::Investigation(
            techdepo_domain::InvestigationId::new("inq_1"),
        ));
        let gen_before = s.timer_gen;
        s.push_inbound(&inbound("m2@x", "Just send a new one for TD-1042"));
        match s.next_step().next {
            Next::Wake {
                awakeable_id,
                reply:
                    CustomerReply::Message {
                        message,
                        order_numbers,
                    },
            } => {
                assert_eq!(awakeable_id, "awk_1");
                assert_eq!(message.body, "Just send a new one for TD-1042");
                assert_eq!(order_numbers, vec![OrderNumber::new(1042)]);
            }
            other => panic!("expected a wake-up, got {other:?}"),
        }
        assert_eq!(s.status, TicketStatus::WithRoute);
        assert!(s.timer_gen > gen_before, "the reminder is cancelled");
        // Further messages wait for the workflow, screened once.
        s.push_inbound(&inbound("m3@x", "?"));
        assert_eq!(
            s.next_step(),
            Step {
                next: Next::Buffer,
                screen: vec![3]
            }
        );
        assert!(s.next_step().screen.is_empty(), "screened already");
    }

    #[test]
    fn messages_older_than_the_question_are_no_answer_to_it() {
        let mut s = ticket();
        s.start_routing(None);
        s.route_active = false;
        s.routing.clear();
        s.status = TicketStatus::WithRoute;
        s.workflows.push(WorkflowRef::Investigation(
            techdepo_domain::InvestigationId::new("inq_1"),
        ));
        // "Any news?" while the investigation waits on a person's decision: buffered, screened.
        s.push_inbound(&inbound("m2@x", "Any news?"));
        assert_eq!(s.next_step().screen, vec![2]);
        // The offer goes out (seq 3), then the ticket waits for the answer.
        s.next_seq += 1;
        s.status = TicketStatus::AwaitingCustomer;
        s.waiter = Some(Waiter {
            awakeable_id: Some("awk_1".into()),
            expects: Awaiting::OfferReply,
            route: Some(TicketRoute::WhereIsMyOrder),
            intent: Some((Intent::OrderStatus, 0.9)),
            reminded: false,
            answers_from: s.next_seq,
        });
        let step = s.next_step();
        assert_eq!(step.next, Next::Nothing, "the old message wakes nobody");
        assert!(step.screen.is_empty(), "already screened");
        assert!(s.unhandled.is_empty());
        assert!(s.waiter.is_some());
        s.push_inbound(&inbound("m4@x", "A refund please"));
        match s.next_step().next {
            Next::Wake {
                reply: CustomerReply::Message { message, .. },
                ..
            } => assert_eq!(message.body, "A refund please"),
            other => panic!("expected a wake-up, got {other:?}"),
        }
    }

    #[test]
    fn a_route_question_supersedes_messages_buffered_while_it_ran() {
        let mut s = ticket();
        s.start_routing(None);
        s.push_inbound(&inbound("m2@x", "also, hurry"));
        assert_eq!(s.next_step().next, Next::Buffer, "a route is in flight");
        // The route asks for the order number (seq 3).
        s.route_active = false;
        s.routing.clear();
        s.next_seq += 1;
        s.status = TicketStatus::AwaitingCustomer;
        s.waiter = Some(Waiter {
            awakeable_id: None,
            expects: Awaiting::OrderNumber,
            route: Some(TicketRoute::CancelOrder),
            intent: Some((Intent::CancelOrder, 0.91)),
            reminded: false,
            answers_from: s.next_seq,
        });
        let step = s.next_step();
        assert_eq!(step.next, Next::Nothing);
        assert_eq!(
            step.screen,
            vec![2],
            "screened instead of taken as the answer"
        );
        s.push_inbound(&inbound("m4@x", "TD-1042"));
        assert_eq!(s.next_step().next, Next::Route);
        let route = s.start_routing(None);
        assert!(route.pending.is_some());
        assert_eq!(route.new_messages.len(), 1);
        assert_eq!(route.new_messages[0].body, "TD-1042");
    }

    #[test]
    fn a_route_question_is_resumed_through_the_router() {
        let mut s = ticket();
        s.start_routing(None);
        s.route_active = false;
        s.routing.clear();
        s.status = TicketStatus::AwaitingCustomer;
        s.waiter = Some(Waiter {
            awakeable_id: None,
            expects: Awaiting::OrderNumber,
            route: Some(TicketRoute::CancelOrder),
            intent: Some((Intent::CancelOrder, 0.91)),
            reminded: true,
            answers_from: 0,
        });
        s.push_inbound(&inbound("m2@x", "TD-1042"));
        assert_eq!(s.next_step().next, Next::Route);
        let route = s.start_routing(None);
        let pending = route.pending.expect("the question is passed on");
        assert_eq!(pending.route, TicketRoute::CancelOrder);
        assert_eq!(pending.expects, Awaiting::OrderNumber);
        assert!(s.waiter.is_none());
    }

    #[test]
    fn a_person_gets_every_message_while_they_have_the_ticket() {
        let mut s = ticket();
        s.assignee = Assignee::Human;
        s.status = TicketStatus::Resolved;
        assert_eq!(s.next_step().next, Next::NotifyHuman);
        assert_eq!(s.status, TicketStatus::WithHuman);
        assert!(s.unhandled.is_empty());
        assert_eq!(s.next_step().next, Next::Nothing);
    }

    /// A sent outbound message; `answers`: a reply, not the escalation acknowledgement.
    fn sent(s: &mut TicketData, body: &str, answers: bool) {
        let seq = s.next_seq;
        s.next_seq += 1;
        s.messages.push(MessageMeta {
            message: ConversationMessage {
                seq,
                direction: Direction::Outbound,
                author: Actor::System,
                subject: "Re: Cancel my order".into(),
                body: body.into(),
                at: AT,
            },
            message_id: Some(format!("out-{seq}")),
        });
        if answers {
            s.answered_through = seq;
        }
    }

    /// "Please cancel", "which order?", "TD-1042", escalated for low confidence and
    /// acknowledged.
    fn escalated() -> TicketData {
        let mut s = ticket();
        s.start_routing(None);
        s.route_active = false;
        s.routing.clear();
        sent(&mut s, "Which order is this about?", true);
        s.push_inbound(&inbound("m3@x", "TD-1042"));
        s.start_routing(None);
        s.route_active = false;
        s.routing.clear();
        s.status = TicketStatus::WithHuman;
        s.assignee = Assignee::Human;
        s.escalation_reason = Some("Low confidence".into());
        sent(&mut s, "A colleague will look at this.", false);
        s
    }

    #[test]
    fn hand_back_routes_the_unanswered_messages_under_the_persons_intent() {
        let mut s = escalated();
        let route = s
            .hand_back(Some(Intent::CancelOrder))
            .expect("routed at once");
        assert_eq!(route.forced_intent, Some(Intent::CancelOrder));
        assert_eq!(route.forced_intent_p, Some(1.0));
        let bodies: Vec<&str> = route.new_messages.iter().map(|m| m.body.as_str()).collect();
        assert_eq!(bodies, ["TD-1042"], "the acknowledgement answered nothing");
        assert_eq!(
            route.conversation.len(),
            4,
            "the route sees the whole thread"
        );
        assert_eq!(s.status, TicketStatus::Routing);
        assert_eq!(s.assignee, Assignee::Ai);
        assert!(s.escalation_reason.is_none());
        assert_eq!(s.routing, vec![3]);
    }

    #[test]
    fn hand_back_without_an_intent_classifies_again() {
        let mut s = escalated();
        s.push_inbound(&inbound("m5@x", "Hello??"));
        s.unhandled.clear(); // the person had the ticket: NotifyHuman took it
        let route = s.hand_back(None).expect("routed at once");
        assert_eq!(route.forced_intent, None);
        let bodies: Vec<&str> = route.new_messages.iter().map(|m| m.body.as_str()).collect();
        assert_eq!(bodies, ["TD-1042", "Hello??"]);
    }

    #[test]
    fn hand_back_after_a_persons_reply_waits_for_the_customer() {
        let mut s = escalated();
        sent(&mut s, "Done, your order is cancelled.", true);
        assert!(s.hand_back(Some(Intent::CancelOrder)).is_none());
        assert_eq!(s.status, TicketStatus::New);
        assert_eq!(s.assignee, Assignee::Ai);
        assert_eq!(s.generation, 2, "nothing routed");
    }

    #[test]
    fn hand_back_of_a_resolved_ticket_waits_for_the_customer() {
        let mut s = escalated();
        s.resolve(AT);
        assert!(s.hand_back(Some(Intent::CancelOrder)).is_none());
        assert_eq!(s.status, TicketStatus::Resolved);
        assert_eq!(s.assignee, Assignee::Ai);
    }

    #[test]
    fn records_keep_the_awaiting_invariant() {
        let mut s = ticket();
        s.status = TicketStatus::AwaitingCustomer;
        assert_eq!(s.record().awaiting, None, "no waiter, no awaiting");
        s.waiter = Some(Waiter {
            awakeable_id: Some("secret".into()),
            expects: Awaiting::OrderNumber,
            route: None,
            intent: None,
            reminded: false,
            answers_from: 0,
        });
        let r = s.record();
        assert_eq!(r.awaiting, Some(Awaiting::OrderNumber));
        assert_eq!(r.message_count, 1);
        assert_eq!(
            s.redacted().waiter.unwrap().awakeable_id.as_deref(),
            Some("<redacted>")
        );
    }

    #[test]
    fn automatic_replies_are_rate_limited() {
        let mut s = ticket();
        for _ in 0..MAX_AUTO_REPLIES_PER_HOUR {
            assert!(s.may_auto_reply(AT));
            s.auto_replies.push(AT.unix_timestamp() * 1000);
        }
        assert!(!s.may_auto_reply(AT));
        assert!(s.may_auto_reply(AT + time::Duration::minutes(61)));
    }

    #[test]
    fn long_bodies_are_truncated_in_state() {
        assert_eq!(truncate("héllo", 2), "hé…");
        assert_eq!(truncate("hi", 5), "hi");
    }
}
