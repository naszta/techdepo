//! `SupportTicket` (virtual object, keyed `<org>:<ticket>`): the conversation state machine
//! (support.md §2). It holds no LLM logic: it reacts to events, decides where each message goes,
//! and is the one place that sends support email, applies the AI reply policy and the reply rate
//! limit, and creates the ticket's human tasks.
//!
//! Every handler finishes quickly. The router and the routes run as their own invocations and
//! report back (`classified`, `route_result`); workflows that wait for the customer hold an
//! awakeable the ticket resolves (the [`Waiter`]); reminders are delayed calls to `timeout`.
//! Nothing ever waits while holding the object's lock. The state and the pure decisions on it
//! live in [`super::conversation`]; this module runs their effects.
//!
//! | Handler | Caller | What |
//! |---|---|---|
//! | `inbound` | mail-in, the web form (ingress, one-way) | append the message, then route it, buffer it (screened by `Router/screen` when it waits behind a workflow or a question supersedes it), hand it to the waiting workflow or to the person who has the ticket (refreshing their tasks) |
//! | `classified` | `Router` | store the classification and route (if this generation is still current) |
//! | `route_result` | routes, agents, workflows | apply a [`RouteOutcome`], then drain the buffer |
//! | `timeout` | itself (delayed) | remind once, then close (or tell the waiting workflow there was no reply) |
//! | `human` | admin (ingress) | reply, take over, hand back, close; cancels the ticket's pending tasks |
//! | `task_decided` | `HumanTasks/decide` | a reviewed draft (send, edit and send, discard) or an escalation decision |
//! | `get` | anyone internal (shared) | the state, without awakeable IDs |
//!
//! **D1:** every change rewrites the `tickets` row from the state and appends `ticket_messages`
//! in one batch (`save-ticket`). The object is the read model's only writer and assigns `seq`.
//!
//! **Outbound mail** is rendered here (`notifications::render_ticket_email`, so the stored
//! message is exactly what is sent) and handed to `Notifications/ticket` one-way; `Email/send`
//! retries delivery on its own. Code-authored templates are always sent; AI-authored text is sent
//! only when its guardrails passed, and then per the org's reply policy, as a review draft
//! otherwise. Automatic replies are limited to [`MAX_AUTO_REPLIES_PER_HOUR`] per ticket (loop
//! protection); over the limit the ticket escalates instead.

use std::sync::Arc;
use std::time::Duration;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::AgentRunId;
use techdepo_api_types::admin::TicketAction;
use techdepo_api_types::ingress::{HumanTicketInput, InboundMessage, TicketState, derive_id};
use techdepo_api_types::support::{Classification, EscalationCause, ReplyConfidence};
use techdepo_api_types::tasks::{HumanDecision, TaskContext};
use techdepo_api_types::vocab::{
    Awaiting, Direction, EmailStatus, HumanTaskKind, MessageStatus, TicketRoute,
};
use techdepo_db::email_log::NewEmail;
use techdepo_db::tickets::NewTicketMessage;
use techdepo_db::{Db, OrgScope, Statement};
use techdepo_domain::{
    Actor, Assignee, Channel, HumanTaskId, Intent, MAX_SETTINGS_DURATION_SECS, OrderNumber, OrgId,
    OrgSettings, ReplyDisposition, TicketId, TicketStatus, UserId,
};
use time::OffsetDateTime;

use super::contract::{
    AiReply, ConversationMessage, CustomerReply, OrderBrief, Reply, RouteOutcome, RouteResult,
    RouteTicketInput, ScreenInput, WorkflowRef,
};
use super::conversation::{
    AfterSend, Draft, MAX_BODY_CHARS, MessageMeta, Next, OpenTask, TicketData, Waiter, truncate,
};
use super::router::RouterClient;
use super::{RefundOutcome, TaskDecidedInput};
use crate::errors::{check_key_org, not_found, sql};
use crate::human_tasks::{CreateTaskInput, HumanTasksClient, RefreshTaskInput, TaskRefInput};
use crate::notifications::templates::SupportTemplate;
use crate::notifications::{
    NotificationsClient, TicketNoticeInput, addressing, render_ticket_email,
};
use crate::{Config, Deps, util};

const STATE: &str = "ticket";

/// Agent reroutes in a row before the ticket escalates.
const MAX_REROUTES: u32 = 2;

/// `SupportTicket/classified` input (from `Router`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassifiedInput {
    /// Org.
    pub org_id: OrgId,
    /// The routing generation classified.
    pub generation: u32,
    /// The classification (absent for a reroute or when the classifier failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Classification>,
    /// The intent being handled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<Intent>,
    /// Its probability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_p: Option<f64>,
    /// The route chosen (`human` when escalated; absent when closed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<TicketRoute>,
    /// The customer's order the message is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<OrderBrief>,
    /// The first order number mentioned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_number: Option<OrderNumber>,
}

/// `SupportTicket/timeout` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeoutInput {
    /// Org.
    pub org_id: OrgId,
    /// The timer generation it was armed for.
    pub generation: u32,
}

/// Side effects collected by a handler, executed after the state is saved.
#[derive(Default)]
struct Effects {
    statements: Vec<Statement>,
    notices: Vec<TicketNoticeInput>,
    wake: Vec<(String, CustomerReply)>,
    cancel_tasks: Vec<HumanTaskId>,
    route: Option<RouteTicketInput>,
    screen: Option<ScreenInput>,
    refresh_tasks: Vec<RefreshTaskInput>,
    timer: Option<Duration>,
}

/// How a reply went.
#[derive(Debug, Clone, PartialEq)]
enum Delivered {
    Sent,
    Drafted,
    Blocked(String),
}

/// A person's action (the admin's ticket actions and escalation decisions).
#[derive(Debug, Clone, PartialEq)]
enum HumanAct {
    Reply { body: String, close: bool },
    TakeOver,
    HandBack { intent: Option<Intent> },
    Close,
}

impl From<TicketAction> for HumanAct {
    fn from(a: TicketAction) -> Self {
        match a {
            TicketAction::Reply { body, close } => Self::Reply { body, close },
            TicketAction::TakeOver => Self::TakeOver,
            TicketAction::HandBack { intent } => Self::HandBack { intent },
            TicketAction::Close => Self::Close,
        }
    }
}

/// One sentence for the reminder.
fn waiting_for(expects: Awaiting) -> &'static str {
    match expects {
        Awaiting::OrderNumber => {
            "the number of the order your message is about (it looks like TD-1042)."
        }
        Awaiting::OfferReply => "whether you would like the refund or the replacement.",
        Awaiting::Clarification => "a few more details about your request.",
    }
}

fn delay(secs: u64) -> Duration {
    Duration::from_secs(secs.clamp(1, MAX_SETTINGS_DURATION_SECS))
}

/// The `SupportTicket` virtual object.
pub struct SupportTicket {
    deps: Arc<Deps>,
}

impl SupportTicket {
    /// The object over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }

    fn config(&self) -> &Config {
        &self.deps.config
    }

    async fn load(ctx: &ObjectContext<'_>, org: &OrgId) -> Result<TicketData, HandlerError> {
        check_key_org(ctx.key(), org)?;
        match ctx.get::<Json<TicketData>>(STATE).await? {
            Some(state) if state.0.org_id == *org => Ok(state.into_inner()),
            _ => Err(not_found("ticket")),
        }
    }

    async fn settings(
        &self,
        ctx: &ObjectContext<'_>,
        org: &OrgId,
    ) -> Result<OrgSettings, HandlerError> {
        let db = self.deps.db.clone();
        let scope = OrgScope::verified(org.clone());
        let settings: Json<OrgSettings> = ctx
            .run(|| async move { db.orgs(&scope).settings().await.map(Json).map_err(sql) })
            .name("load-settings")
            .await?;
        Ok(settings.into_inner())
    }

    fn db(&self) -> &Db {
        &self.deps.db
    }

    /// Saves the state (D1 batch, then the object state) and runs the side effects.
    async fn finish(
        &self,
        ctx: &ObjectContext<'_>,
        s: &mut TicketData,
        fx: Effects,
        at: OffsetDateTime,
    ) -> Result<(), HandlerError> {
        s.updated_at = at;
        let db = self.deps.db.clone();
        let scope = OrgScope::verified(s.org_id.clone());
        let mut statements = vec![
            db.tickets(&scope)
                .save_statement(&s.record())
                .map_err(sql)?,
        ];
        statements.extend(fx.statements);
        // The ticket row first: messages and tasks reference it.
        ctx.run(|| async move {
            db.batch(&statements).await.map_err(sql)?;
            Ok(())
        })
        .name("save-ticket")
        .await?;
        ctx.set(STATE, Json(s.clone()));
        for notice in fx.notices {
            let _ = ctx
                .service_client::<NotificationsClient>()
                .ticket(Json(notice))
                .send();
        }
        for (awakeable, reply) in fx.wake {
            ctx.resolve_awakeable(&awakeable, Json(reply));
        }
        for task_id in fx.cancel_tasks {
            let _ = ctx
                .service_client::<HumanTasksClient>()
                .cancel(Json(TaskRefInput {
                    org_id: s.org_id.clone(),
                    task_id,
                }))
                .send();
        }
        for refresh in fx.refresh_tasks {
            let _ = ctx
                .service_client::<HumanTasksClient>()
                .refresh(Json(refresh))
                .send();
        }
        if let Some(route) = fx.route {
            let _ = ctx
                .service_client::<RouterClient>()
                .route(Json(route))
                .send();
        }
        if let Some(screen) = fx.screen {
            let _ = ctx
                .service_client::<RouterClient>()
                .screen(Json(screen))
                .send();
        }
        if let Some(after) = fx.timer {
            let _ = ctx
                .object_client::<SupportTicketClient>(ctx.key())
                .timeout(Json(TimeoutInput {
                    org_id: s.org_id.clone(),
                    generation: s.timer_gen,
                }))
                .send_after(after);
        }
        Ok(())
    }

    /// Renders and records an outbound message, queues its email. Returns its `seq`.
    #[allow(clippy::too_many_arguments)]
    fn send_message(
        &self,
        s: &mut TicketData,
        fx: &mut Effects,
        template: SupportTemplate,
        author: Actor,
        author_user: Option<&UserId>,
        agent_run_id: Option<&AgentRunId>,
        automatic: bool,
        at: OffsetDateTime,
    ) -> Result<u32, HandlerError> {
        let seq = s.next_seq;
        s.next_seq += 1;
        if template != SupportTemplate::Escalated {
            s.answered_through = seq;
        }
        let notice = s.notice(seq, template);
        let (email, rendered) = render_ticket_email(self.config(), &s.public_org_id, &notice)?;
        let scope = OrgScope::verified(s.org_id.clone());
        fx.statements
            .push(self.db().tickets(&scope).insert_message_statement(
                &s.ticket_id,
                &NewTicketMessage {
                    seq,
                    direction: Direction::Outbound,
                    author,
                    author_user_id: author_user.cloned(),
                    channel: Channel::Email,
                    status: MessageStatus::Sent,
                    subject: rendered.subject.clone(),
                    body_text: rendered.text.clone(),
                    body_html: Some(rendered.html.clone()),
                    message_id: Some(email.message_id.clone()),
                    email_log_id: None,
                    agent_run_id: agent_run_id.map(|r| r.as_str().to_owned()),
                    created_at: at,
                    sent_at: Some(at),
                },
            ));
        s.messages.push(MessageMeta {
            message: ConversationMessage {
                seq,
                direction: Direction::Outbound,
                author,
                subject: rendered.subject,
                body: truncate(&rendered.text, MAX_BODY_CHARS),
                at,
            },
            message_id: Some(email.message_id),
        });
        if automatic {
            s.auto_replies.push(at.unix_timestamp() * 1000);
        }
        s.last_message_at = Some(at);
        fx.notices.push(notice);
        Ok(seq)
    }

    /// Creates a ticket-owned task and remembers it.
    async fn create_task(
        &self,
        ctx: &ObjectContext<'_>,
        s: &mut TicketData,
        context: TaskContext,
        title: String,
        summary: String,
    ) -> Result<HumanTaskId, HandlerError> {
        let kind = context.kind();
        let id: Json<HumanTaskId> = ctx
            .service_client::<HumanTasksClient>()
            .create(Json(CreateTaskInput {
                org_id: s.org_id.clone(),
                context,
                title: truncate(&title, 140),
                summary: truncate(&summary, 280),
                evidence: Vec::new(),
                awakeable_id: None,
                ticket_id: Some(s.ticket_id.clone()),
                order_id: s.order.as_ref().map(|o| o.order_id.clone()),
                refund_id: None,
                investigation_id: None,
            }))
            .call()
            .await?;
        let id = id.into_inner();
        s.open_tasks.push(OpenTask {
            id: id.clone(),
            kind,
        });
        Ok(id)
    }

    /// Tells the customer a person took the ticket over from the AI (support.md §8), within the
    /// automatic reply limit.
    fn acknowledge_escalation(
        &self,
        s: &mut TicketData,
        fx: &mut Effects,
        from_ai: bool,
        at: OffsetDateTime,
    ) -> Result<(), HandlerError> {
        if from_ai && s.may_auto_reply(at) {
            self.send_message(
                s,
                fx,
                SupportTemplate::Escalated,
                Actor::System,
                None,
                None,
                true,
                at,
            )?;
        }
        Ok(())
    }

    /// Hands the ticket to a person (support.md §8).
    #[allow(clippy::too_many_arguments)]
    async fn escalate(
        &self,
        ctx: &ObjectContext<'_>,
        s: &mut TicketData,
        fx: &mut Effects,
        reason: String,
        causes: Vec<EscalationCause>,
        agent_run_id: Option<AgentRunId>,
        at: OffsetDateTime,
    ) -> Result<(), HandlerError> {
        self.acknowledge_escalation(s, fx, s.assignee == Assignee::Ai, at)?;
        withdraw_waiter(s, fx);
        s.timer_gen += 1;
        s.route_active = false;
        s.status = TicketStatus::WithHuman;
        s.assignee = Assignee::Human;
        s.resolved_at = None;
        s.escalation_reason = Some(reason.clone());
        if s.open_tasks
            .iter()
            .any(|t| t.kind == HumanTaskKind::Escalation)
        {
            return Ok(());
        }
        let context = TaskContext::Escalation {
            reason: reason.clone(),
            classification: (!causes.is_empty())
                .then(|| s.classification.clone())
                .flatten(),
            causes,
            agent_run_id,
            new_customer_message: false,
        };
        let title = format!("Ticket {}: {reason}", s.ticket_id);
        let summary = s.subject.clone();
        self.create_task(ctx, s, context, title, summary).await?;
        Ok(())
    }

    /// Tells the customer something, by template or AI text (guardrails, reply policy, rate
    /// limit). `after` says what follows a sent (or later approved) reply.
    #[allow(clippy::too_many_arguments)]
    async fn deliver(
        &self,
        ctx: &ObjectContext<'_>,
        s: &mut TicketData,
        fx: &mut Effects,
        settings: &OrgSettings,
        reply: Reply,
        after: &AfterSend,
        at: OffsetDateTime,
    ) -> Result<Delivered, HandlerError> {
        match reply {
            Reply::Template { template } => {
                if !s.may_auto_reply(at) {
                    return Ok(Delivered::Blocked(
                        "Automatic reply limit reached for this ticket".to_owned(),
                    ));
                }
                self.send_message(s, fx, template, Actor::System, None, None, true, at)?;
                Ok(Delivered::Sent)
            }
            Reply::Ai(ai) => self.deliver_ai(ctx, s, fx, settings, ai, after, at).await,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn deliver_ai(
        &self,
        ctx: &ObjectContext<'_>,
        s: &mut TicketData,
        fx: &mut Effects,
        settings: &OrgSettings,
        ai: AiReply,
        after: &AfterSend,
        at: OffsetDateTime,
    ) -> Result<Delivered, HandlerError> {
        if !ai.guardrails_passed() {
            let violations: Vec<String> = ai
                .guardrails
                .iter()
                .filter(|g| !g.passed)
                .flat_map(|g| g.violations.iter().cloned())
                .collect();
            return Ok(Delivered::Blocked(format!(
                "The AI reply failed its guardrails: {}",
                violations.join("; ")
            )));
        }
        let confidence = ai.confidence();
        let disposition = settings.reply_disposition(confidence);
        s.reply_confidence = Some(confidence);
        if disposition == ReplyDisposition::Send {
            if !s.may_auto_reply(at) {
                return Ok(Delivered::Blocked(
                    "Automatic reply limit reached for this ticket".to_owned(),
                ));
            }
            self.send_message(
                s,
                fx,
                SupportTemplate::Reply { body: ai.body },
                Actor::Ai,
                None,
                Some(&ai.agent_run_id),
                true,
                at,
            )?;
            return Ok(Delivered::Sent);
        }
        // A review draft: stored, not sent, with a `review_reply` task.
        let seq = s.next_seq;
        s.next_seq += 1;
        let notice = s.notice(
            seq,
            SupportTemplate::Reply {
                body: ai.body.clone(),
            },
        );
        let (_, rendered) = render_ticket_email(self.config(), &s.public_org_id, &notice)?;
        let scope = OrgScope::verified(s.org_id.clone());
        fx.statements
            .push(self.db().tickets(&scope).insert_message_statement(
                &s.ticket_id,
                &NewTicketMessage {
                    seq,
                    direction: Direction::Outbound,
                    author: Actor::Ai,
                    author_user_id: None,
                    channel: Channel::Email,
                    status: MessageStatus::Draft,
                    subject: rendered.subject.clone(),
                    body_text: rendered.text,
                    body_html: Some(rendered.html),
                    message_id: None,
                    email_log_id: None,
                    agent_run_id: Some(ai.agent_run_id.as_str().to_owned()),
                    created_at: at,
                    sent_at: None,
                },
            ));
        let confidence = ReplyConfidence {
            intent_p: ai.intent_p,
            judge: ai
                .judge
                .unwrap_or(techdepo_api_types::support::JudgeScores {
                    answers_question: 0.0,
                    supported_by_tools: 0.0,
                    tone_appropriate: 0.0,
                }),
            confidence,
            policy: settings.reply_policy,
            threshold: settings.reply_confidence_threshold,
            disposition,
        };
        let title = format!("Review the AI reply on ticket {}", s.ticket_id);
        let summary = ai.body.clone();
        let context = TaskContext::ReviewReply {
            subject: rendered.subject,
            draft: ai.body.clone(),
            confidence,
            agent_run_id: ai.agent_run_id.clone(),
        };
        let task_id = self.create_task(ctx, s, context, title, summary).await?;
        s.drafts.push(Draft {
            task_id,
            seq,
            body: ai.body,
            agent_run_id: ai.agent_run_id,
            after: after.clone(),
        });
        s.status = TicketStatus::WithHuman;
        Ok(Delivered::Drafted)
    }

    /// Applies what follows a sent reply.
    fn after_sent(
        s: &mut TicketData,
        fx: &mut Effects,
        settings: &OrgSettings,
        after: AfterSend,
        at: OffsetDateTime,
    ) {
        match after.waiter {
            Some(mut waiter) => {
                // Only messages received from now on answer the question.
                waiter.answers_from = s.next_seq;
                s.status = TicketStatus::AwaitingCustomer;
                s.waiter = Some(waiter);
                s.timer_gen += 1;
                fx.timer = Some(delay(settings.reminder_after_secs));
            }
            None if after.resolve => s.resolve(at),
            None => s.status = TicketStatus::WithRoute,
        }
    }

    /// Applies a route's (or workflow's) outcome.
    #[allow(clippy::too_many_arguments)]
    async fn apply(
        &self,
        ctx: &ObjectContext<'_>,
        s: &mut TicketData,
        fx: &mut Effects,
        settings: &OrgSettings,
        outcome: RouteOutcome,
        agent_run_id: Option<AgentRunId>,
        late: bool,
        at: OffsetDateTime,
    ) -> Result<(), HandlerError> {
        // A person has the ticket, or a late workflow report arrives while a route works on a
        // newer message: deliver facts, but leave the status to them.
        let with_human = s.assignee == Assignee::Human || (late && s.route_active);
        match outcome {
            RouteOutcome::Reply { reply, resolve } => {
                if resolve {
                    s.workflows.clear();
                }
                let after = AfterSend {
                    resolve: resolve && !with_human,
                    waiter: None,
                };
                // A person has the ticket: only code-authored facts still go out.
                if with_human && matches!(reply, Reply::Ai(_)) {
                    return Ok(());
                }
                match self
                    .deliver(ctx, s, fx, settings, reply, &after, at)
                    .await?
                {
                    Delivered::Sent if !with_human => Self::after_sent(s, fx, settings, after, at),
                    Delivered::Sent | Delivered::Drafted => {}
                    Delivered::Blocked(reason) => {
                        self.escalate(ctx, s, fx, reason, Vec::new(), agent_run_id, at)
                            .await?;
                    }
                }
            }
            RouteOutcome::AskCustomer {
                question,
                expects,
                awakeable_id,
            } => {
                if with_human {
                    if let Some(id) = awakeable_id {
                        fx.wake.push((id, CustomerReply::Withdrawn));
                    }
                    return Ok(());
                }
                let after = AfterSend {
                    resolve: false,
                    waiter: Some(Waiter {
                        awakeable_id: awakeable_id.clone(),
                        expects,
                        route: s.route,
                        intent: s.intent,
                        reminded: false,
                        answers_from: 0,
                    }),
                };
                match self
                    .deliver(ctx, s, fx, settings, question, &after, at)
                    .await?
                {
                    Delivered::Sent => Self::after_sent(s, fx, settings, after, at),
                    Delivered::Drafted => {}
                    Delivered::Blocked(reason) => {
                        if let Some(id) = awakeable_id {
                            fx.wake.push((id, CustomerReply::Withdrawn));
                        }
                        self.escalate(ctx, s, fx, reason, Vec::new(), agent_run_id, at)
                            .await?;
                    }
                }
            }
            RouteOutcome::WaitingOnWorkflow { workflow, reply } => {
                if !s.workflows.contains(&workflow) {
                    s.workflows.push(workflow);
                }
                if !with_human {
                    s.status = TicketStatus::WithRoute;
                }
                if let Some(reply) = reply {
                    let after = AfterSend {
                        resolve: false,
                        waiter: None,
                    };
                    if let Delivered::Blocked(reason) = self
                        .deliver(ctx, s, fx, settings, reply, &after, at)
                        .await?
                    {
                        self.escalate(ctx, s, fx, reason, Vec::new(), agent_run_id, at)
                            .await?;
                    }
                }
            }
            RouteOutcome::Escalate { reason, causes } => {
                s.workflows.clear();
                self.escalate(ctx, s, fx, reason, causes, agent_run_id, at)
                    .await?;
            }
            RouteOutcome::Close => {
                if !with_human {
                    s.resolve(at);
                }
            }
            RouteOutcome::Reroute { intent } => {
                if with_human {
                    return Ok(());
                }
                s.reroutes += 1;
                if s.reroutes > MAX_REROUTES {
                    self.escalate(
                        ctx,
                        s,
                        fx,
                        "The assistant kept handing the ticket between routes".to_owned(),
                        Vec::new(),
                        agent_run_id,
                        at,
                    )
                    .await?;
                } else {
                    let p = s.intent.map_or(0.0, |(_, p)| p);
                    // Re-route the messages in flight under the new intent.
                    s.unhandled.extend(std::mem::take(&mut s.routing));
                    fx.route = Some(s.start_routing(Some((intent, p))));
                }
            }
            RouteOutcome::OffersDeclined {
                investigation_id,
                last_reply,
            } => {
                s.workflows
                    .retain(|w| *w != WorkflowRef::Investigation(investigation_id.clone()));
                self.offers_declined(ctx, s, fx, investigation_id, last_reply, at)
                    .await?;
            }
            RouteOutcome::RefundFinished { outcome } => {
                self.refund_finished(ctx, s, fx, settings, outcome, with_human, at)
                    .await?;
            }
        }
        Ok(())
    }

    /// The customer declined every offer of an investigation: a person takes the ticket, with an
    /// `offers_declined` task (support.md §8).
    async fn offers_declined(
        &self,
        ctx: &ObjectContext<'_>,
        s: &mut TicketData,
        fx: &mut Effects,
        investigation_id: techdepo_domain::InvestigationId,
        last_reply: String,
        at: OffsetDateTime,
    ) -> Result<(), HandlerError> {
        let reason = "The customer declined every offer".to_owned();
        self.acknowledge_escalation(s, fx, s.assignee == Assignee::Ai, at)?;
        withdraw_waiter(s, fx);
        s.timer_gen += 1;
        s.route_active = false;
        s.status = TicketStatus::WithHuman;
        s.assignee = Assignee::Human;
        s.resolved_at = None;
        s.escalation_reason = Some(reason.clone());
        let context = TaskContext::OffersDeclined {
            investigation_id,
            last_reply: truncate(&last_reply, 500),
        };
        let title = format!("Ticket {}: {reason}", s.ticket_id);
        let summary = s.subject.clone();
        self.create_task(ctx, s, context, title, summary).await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn refund_finished(
        &self,
        ctx: &ObjectContext<'_>,
        s: &mut TicketData,
        fx: &mut Effects,
        settings: &OrgSettings,
        outcome: RefundOutcome,
        with_human: bool,
        at: OffsetDateTime,
    ) -> Result<(), HandlerError> {
        s.workflows
            .retain(|w| *w != WorkflowRef::Refund(outcome.refund_id.clone()));
        let db = self.deps.db.clone();
        let scope = OrgScope::verified(s.org_id.clone());
        let id = outcome.refund_id.clone();
        type Facts = (
            OrderNumber,
            bool,
            Option<techdepo_domain::InvoiceNumber>,
            Option<techdepo_domain::InvestigationId>,
        );
        let facts: Json<Option<Facts>> = ctx
            .run(|| async move {
                let Some(refund) = db.refunds(&scope).get(&id).await.map_err(sql)? else {
                    return Ok(Json(None));
                };
                let total = db
                    .orders(&scope)
                    .get(&refund.order_id)
                    .await
                    .map_err(sql)?
                    .map(|o| o.total);
                let credit_note = match &refund.credit_note_id {
                    Some(cn) => db
                        .invoices(&scope)
                        .get(cn)
                        .await
                        .map_err(sql)?
                        .map(|i| i.number),
                    None => None,
                };
                Ok(Json(Some((
                    refund.order_number,
                    // A duplicate charge's refund leaves the order paid, whatever its amount.
                    total == Some(refund.amount)
                        && refund.reason
                            != techdepo_api_types::vocab::RefundReason::DuplicateCharge,
                    credit_note,
                    refund.investigation_id,
                ))))
            })
            .name("load-refund")
            .await?;
        let Some((order_number, full, credit_note, investigation)) = facts.into_inner() else {
            return Err(not_found("refund"));
        };
        // The refund an investigation's accepted offer started ends that investigation too.
        if let Some(investigation) = investigation {
            s.workflows
                .retain(|w| *w != WorkflowRef::Investigation(investigation.clone()));
        }
        let template = if outcome.status == techdepo_api_types::vocab::RefundStatus::Completed {
            SupportTemplate::RefundDone {
                order_number,
                amount: outcome.amount,
                full,
                credit_note,
            }
        } else {
            SupportTemplate::RefundRejected {
                order_number,
                amount: outcome.amount,
                reason: outcome
                    .reason
                    .clone()
                    .unwrap_or_else(|| "Not approved".to_owned()),
            }
        };
        let after = AfterSend {
            resolve: !with_human && s.workflows.is_empty(),
            waiter: None,
        };
        match self
            .deliver(ctx, s, fx, settings, Reply::template(template), &after, at)
            .await?
        {
            Delivered::Sent if after.resolve => s.resolve(at),
            Delivered::Blocked(reason) if !with_human => {
                self.escalate(ctx, s, fx, reason, Vec::new(), None, at)
                    .await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Carries out [`TicketData::next_step`].
    async fn run_next(
        &self,
        ctx: &ObjectContext<'_>,
        s: &mut TicketData,
        fx: &mut Effects,
    ) -> Result<(), HandlerError> {
        let step = s.next_step();
        if !step.screen.is_empty() {
            fx.screen = Some(s.screen_request(&step.screen));
        }
        match step.next {
            Next::Nothing | Next::Buffer => {}
            Next::Route => fx.route = Some(s.start_routing(None)),
            Next::Wake {
                awakeable_id,
                reply,
            } => fx.wake.push((awakeable_id, reply)),
            Next::NotifyHuman => {
                if !s.open_tasks.is_empty() {
                    // The person's tasks say the customer wrote again (support.md §2).
                    let excerpt = s
                        .messages
                        .iter()
                        .rev()
                        .find(|m| m.message.direction == Direction::Inbound)
                        .map(|m| m.message.body.trim().to_owned())
                        .unwrap_or_default();
                    let summary = truncate(&format!("New customer message: {excerpt}"), 280);
                    fx.refresh_tasks
                        .extend(s.open_tasks.iter().map(|t| RefreshTaskInput {
                            org_id: s.org_id.clone(),
                            task_id: t.id.clone(),
                            summary: summary.clone(),
                        }));
                } else {
                    let context = TaskContext::Escalation {
                        reason: "New customer message".to_owned(),
                        causes: vec![EscalationCause::HumanAssigned],
                        classification: None,
                        agent_run_id: None,
                        new_customer_message: true,
                    };
                    let title = format!("Ticket {}: new customer message", s.ticket_id);
                    let summary = s.subject.clone();
                    self.create_task(ctx, s, context, title, summary).await?;
                }
            }
        }
        Ok(())
    }

    /// A person acts on the ticket: cancels its pending tasks and drafts, stops any waiting
    /// workflow, and invalidates routes in flight.
    async fn act(
        &self,
        s: &mut TicketData,
        fx: &mut Effects,
        act: HumanAct,
        by: &UserId,
        at: OffsetDateTime,
    ) -> Result<(), HandlerError> {
        let scope = OrgScope::verified(s.org_id.clone());
        fx.cancel_tasks.extend(s.open_tasks.drain(..).map(|t| t.id));
        for draft in std::mem::take(&mut s.drafts) {
            fx.statements
                .push(self.db().tickets(&scope).settle_draft_statement(
                    &s.ticket_id,
                    draft.seq,
                    MessageStatus::Discarded,
                    Actor::Ai,
                    None,
                    None,
                    None,
                    None,
                ));
        }
        s.generation += 1;
        s.route_active = false;
        s.routing.clear();
        s.timer_gen += 1;
        match act {
            HumanAct::Reply { body, close } => {
                withdraw_waiter(s, fx);
                self.send_message(
                    s,
                    fx,
                    SupportTemplate::Reply { body },
                    Actor::Human,
                    Some(by),
                    None,
                    false,
                    at,
                )?;
                s.assignee = Assignee::Human;
                s.unhandled.clear();
                if close {
                    s.resolve(at);
                } else {
                    s.status = TicketStatus::WithHuman;
                    s.resolved_at = None;
                }
            }
            HumanAct::TakeOver => {
                withdraw_waiter(s, fx);
                s.assignee = Assignee::Human;
                s.unhandled.clear();
                if s.status != TicketStatus::Resolved {
                    s.status = TicketStatus::WithHuman;
                }
            }
            HumanAct::HandBack { intent } => {
                // The AI answers what is still unanswered (under the person's intent), or
                // resumes with the customer's next message; a workflow that still waited for the
                // customer is told it will get no answer.
                withdraw_waiter(s, fx);
                fx.route = s.hand_back(intent);
            }
            HumanAct::Close => {
                withdraw_waiter(s, fx);
                s.unhandled.clear();
                s.resolve(at);
            }
        }
        Ok(())
    }

    /// Sends (or discards) a reviewed draft.
    #[allow(clippy::too_many_arguments)]
    async fn settle_draft(
        &self,
        ctx: &ObjectContext<'_>,
        s: &mut TicketData,
        fx: &mut Effects,
        draft: Draft,
        decision: &HumanDecision,
        by: &UserId,
        at: OffsetDateTime,
    ) -> Result<(), HandlerError> {
        let scope = OrgScope::verified(s.org_id.clone());
        let tickets = self.db().tickets(&scope);
        let (body, author, author_user) = match decision {
            HumanDecision::Send => (draft.body.clone(), Actor::Ai, None),
            HumanDecision::EditAndSend { body } => (body.clone(), Actor::Human, Some(by)),
            _ => {
                fx.statements.push(tickets.settle_draft_statement(
                    &s.ticket_id,
                    draft.seq,
                    MessageStatus::Discarded,
                    Actor::Ai,
                    None,
                    None,
                    None,
                    None,
                ));
                // Discarding hands the ticket to a person.
                withdraw_draft_waiter(&draft, fx);
                s.assignee = Assignee::Human;
                s.status = TicketStatus::WithHuman;
                return Ok(());
            }
        };
        let notice = s.notice(draft.seq, SupportTemplate::Reply { body });
        let (email, rendered) = render_ticket_email(self.config(), &s.public_org_id, &notice)?;
        fx.statements.push(tickets.settle_draft_statement(
            &s.ticket_id,
            draft.seq,
            MessageStatus::Sent,
            author,
            author_user,
            Some((&rendered.text, Some(&rendered.html))),
            Some(&email.message_id),
            Some(at),
        ));
        s.messages.push(MessageMeta {
            message: ConversationMessage {
                seq: draft.seq,
                direction: Direction::Outbound,
                author,
                subject: rendered.subject,
                body: truncate(&rendered.text, MAX_BODY_CHARS),
                at,
            },
            message_id: Some(email.message_id),
        });
        s.messages.sort_by_key(|m| m.message.seq);
        s.answered_through = s.answered_through.max(draft.seq);
        s.last_message_at = Some(at);
        fx.notices.push(notice);
        if s.drafts.is_empty() && s.assignee == Assignee::Ai {
            let settings = self.settings(ctx, &s.org_id).await?;
            Self::after_sent(s, fx, &settings, draft.after, at);
        }
        Ok(())
    }
}

/// Stops a waiting workflow (a person took over or closed the ticket).
fn withdraw_waiter(s: &mut TicketData, fx: &mut Effects) {
    if let Some(w) = s.waiter.take()
        && let Some(id) = w.awakeable_id
    {
        fx.wake.push((id, CustomerReply::Withdrawn));
    }
}

fn withdraw_draft_waiter(draft: &Draft, fx: &mut Effects) {
    if let Some(id) = draft
        .after
        .waiter
        .as_ref()
        .and_then(|w| w.awakeable_id.clone())
    {
        fx.wake.push((id, CustomerReply::Withdrawn));
    }
}

#[restate_sdk::object(name = "SupportTicket")]
impl SupportTicket {
    /// A customer message (mail-in or the web form).
    #[handler]
    async fn inbound(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<InboundMessage>,
    ) -> Result<(), HandlerError> {
        let msg = input.into_inner();
        let rest = check_key_org(ctx.key(), &msg.org_id)?;
        if TicketId::parse(rest).ok().as_ref() != Some(&msg.ticket_id) {
            return Err(not_found("ticket"));
        }
        let at = util::now(&ctx).await?;
        let mut s = match ctx.get::<Json<TicketData>>(STATE).await? {
            Some(state) => {
                let state = state.into_inner();
                if state.org_id != msg.org_id || state.user_id != msg.user_id {
                    // Another member's ticket: never mix conversations.
                    return Err(not_found("ticket"));
                }
                state
            }
            None => TicketData::open(&msg, at),
        };
        if s.has_message(&msg.message_id) {
            return Ok(());
        }
        let seq = s.push_inbound(&msg);
        let scope = OrgScope::verified(s.org_id.clone());
        let mut fx = Effects::default();
        let email_log_id = match &msg.email_log_id {
            Some(id) => id.as_str().to_owned(),
            None => {
                // The web form has no mail-in: log it here, like an accepted email.
                let id = derive_id("eml", &[msg.org_id.as_str(), "inbound", &msg.message_id]);
                fx.statements.push(
                    self.db()
                        .email_log(&scope)
                        .insert_statement(&NewEmail {
                            id: id.clone(),
                            direction: Some(Direction::Inbound),
                            channel: Some(msg.channel),
                            status: Some(EmailStatus::Accepted),
                            message_id: Some(msg.message_id.clone()),
                            in_reply_to: msg.in_reply_to.clone(),
                            references: msg.references.clone(),
                            from_address: msg.customer_email.to_lowercase(),
                            from_name: msg.customer_name.clone(),
                            to_address: addressing::ticket_address(
                                self.config(),
                                &s.public_org_id,
                                &s.ticket_id,
                            ),
                            subject: msg.subject.clone(),
                            text_body: Some(msg.body_text.clone()),
                            user_id: Some(msg.user_id.clone()),
                            ticket_id: Some(s.ticket_id.clone()),
                            created_at_ms: msg.received_at.unix_timestamp() * 1000,
                            ..NewEmail::default()
                        })
                        .map_err(sql)?,
                );
                id
            }
        };
        let message = NewTicketMessage {
            seq,
            direction: Direction::Inbound,
            author: Actor::Customer,
            author_user_id: Some(msg.user_id.clone()),
            channel: msg.channel,
            status: MessageStatus::Received,
            subject: msg.subject.clone(),
            body_text: msg.body_text.clone(),
            body_html: None,
            message_id: Some(msg.message_id.clone()),
            email_log_id: Some(email_log_id),
            agent_run_id: None,
            created_at: msg.received_at,
            sent_at: None,
        };
        let message = self
            .db()
            .tickets(&scope)
            .insert_message_statement(&s.ticket_id, &message);
        fx.statements.insert(0, message);
        // A task created now (a person has the ticket) references a ticket row that already
        // exists: only an existing ticket can be with a person.
        self.run_next(&ctx, &mut s, &mut fx).await?;
        self.finish(&ctx, &mut s, fx, at).await
    }

    /// The router classified a generation: store it. Returns whether it is still current.
    #[handler(ingress_private = true)]
    async fn classified(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<ClassifiedInput>,
    ) -> Result<Json<bool>, HandlerError> {
        let input = input.into_inner();
        let mut s = Self::load(&ctx, &input.org_id).await?;
        if input.generation != s.generation || s.status != TicketStatus::Routing {
            return Ok(Json(false));
        }
        let at = util::now(&ctx).await?;
        if let Some(c) = input.classification {
            s.classification = Some(c);
        }
        if let Some(intent) = input.intent {
            s.intent = Some((intent, input.intent_p.unwrap_or(0.0)));
        }
        if input.route.is_some() {
            s.route = input.route;
        }
        if let Some(order) = input.order {
            s.order_number = Some(order.number);
            s.order = Some(order);
        } else if input.order_number.is_some() {
            s.order_number = input.order_number;
        }
        s.status = TicketStatus::WithRoute;
        self.finish(&ctx, &mut s, Effects::default(), at).await?;
        Ok(Json(true))
    }

    /// A route, agent or workflow reports (support.md §2 `route_result`).
    #[handler(ingress_private = true)]
    async fn route_result(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<RouteResult>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        let mut s = Self::load(&ctx, &input.org_id).await?;
        if let Some(generation) = input.generation
            && (generation != s.generation || !s.route_active)
        {
            // A person acted, or a newer generation runs: this result is stale.
            return Ok(());
        }
        let at = util::now(&ctx).await?;
        let settings = self.settings(&ctx, &s.org_id).await?;
        let mut fx = Effects::default();
        if input.generation.is_some() {
            s.route_active = false;
            if !matches!(input.outcome, RouteOutcome::Reroute { .. }) {
                s.routing.clear();
            }
            if input.route.is_some() {
                s.route = input.route;
            }
            if let Some(order) = input.order {
                s.order_number = Some(order.number);
                s.order = Some(order);
            }
        }
        let late = input.generation.is_none();
        self.apply(
            &ctx,
            &mut s,
            &mut fx,
            &settings,
            input.outcome,
            input.agent_run_id,
            late,
            at,
        )
        .await?;
        self.run_next(&ctx, &mut s, &mut fx).await?;
        self.finish(&ctx, &mut s, fx, at).await
    }

    /// A reminder timer fired: remind once, then close (or tell the waiting workflow).
    #[handler(ingress_private = true)]
    async fn timeout(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<TimeoutInput>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        let mut s = Self::load(&ctx, &input.org_id).await?;
        if input.generation != s.timer_gen || s.status != TicketStatus::AwaitingCustomer {
            return Ok(());
        }
        let Some(mut waiter) = s.waiter.clone() else {
            return Ok(());
        };
        let at = util::now(&ctx).await?;
        let settings = self.settings(&ctx, &s.org_id).await?;
        let mut fx = Effects::default();
        if waiter.reminded {
            match waiter.awakeable_id.take() {
                Some(id) => {
                    // The workflow decides what silence means (it escalates).
                    fx.wake.push((id, CustomerReply::NoReply));
                    s.waiter = None;
                    s.timer_gen += 1;
                    s.status = TicketStatus::WithRoute;
                }
                None => {
                    if s.may_auto_reply(at) {
                        self.send_message(
                            &mut s,
                            &mut fx,
                            SupportTemplate::Closed,
                            Actor::System,
                            None,
                            None,
                            true,
                            at,
                        )?;
                    }
                    s.resolve(at);
                }
            }
        } else {
            if s.may_auto_reply(at) {
                self.send_message(
                    &mut s,
                    &mut fx,
                    SupportTemplate::Reminder {
                        waiting_for: waiting_for(waiter.expects).to_owned(),
                    },
                    Actor::System,
                    None,
                    None,
                    true,
                    at,
                )?;
            }
            waiter.reminded = true;
            s.waiter = Some(waiter);
            s.timer_gen += 1;
            fx.timer = Some(delay(settings.auto_close_after_secs));
        }
        self.finish(&ctx, &mut s, fx, at).await
    }

    /// A person acts on the ticket from the admin page.
    #[handler]
    async fn human(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<HumanTicketInput>,
    ) -> Result<Json<TicketState>, HandlerError> {
        let input = input.into_inner();
        let mut s = Self::load(&ctx, &input.org_id).await?;
        let at = util::now(&ctx).await?;
        let mut fx = Effects::default();
        self.act(&mut s, &mut fx, input.action.into(), &input.by, at)
            .await?;
        self.finish(&ctx, &mut s, fx, at).await?;
        Ok(Json(TicketState {
            status: s.status,
            assignee: s.assignee,
        }))
    }

    /// A decision on one of the ticket's tasks (`HumanTasks/decide`).
    #[handler(ingress_private = true)]
    async fn task_decided(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<TaskDecidedInput>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        let mut s = Self::load(&ctx, &input.org_id).await?;
        if !s.open_tasks.iter().any(|t| t.id == input.task_id) {
            // Already answered by acting on the ticket.
            return Ok(());
        }
        s.open_tasks.retain(|t| t.id != input.task_id);
        let at = util::now(&ctx).await?;
        let mut fx = Effects::default();
        match input.kind {
            HumanTaskKind::ReviewReply => {
                let Some(pos) = s.drafts.iter().position(|d| d.task_id == input.task_id) else {
                    return Ok(());
                };
                let draft = s.drafts.remove(pos);
                self.settle_draft(&ctx, &mut s, &mut fx, draft, &input.decision, &input.by, at)
                    .await?;
                self.run_next(&ctx, &mut s, &mut fx).await?;
            }
            HumanTaskKind::Escalation | HumanTaskKind::OffersDeclined => {
                let act = match input.decision {
                    HumanDecision::Reply { body, close } => HumanAct::Reply { body, close },
                    HumanDecision::HandBack { intent } => HumanAct::HandBack { intent },
                    HumanDecision::Close => HumanAct::Close,
                    _ => return Ok(()),
                };
                self.act(&mut s, &mut fx, act, &input.by, at).await?;
            }
            HumanTaskKind::ApproveRefund | HumanTaskKind::InvestigationDecision => {
                return Ok(());
            }
        }
        self.finish(&ctx, &mut s, fx, at).await
    }

    /// The ticket's state (without awakeable IDs); `None` for an unknown ticket.
    #[handler(ingress_private = true)]
    async fn get(
        &self,
        ctx: SharedObjectContext<'_>,
    ) -> Result<Json<Option<TicketData>>, HandlerError> {
        Ok(Json(
            ctx.get::<Json<TicketData>>(STATE)
                .await?
                .map(|s| s.into_inner().redacted()),
        ))
    }
}
