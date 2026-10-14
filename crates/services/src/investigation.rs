//! `Investigation` (workflow, keyed by investigation ID): a parcel that should have arrived by
//! now (support.md §4). Started by the `WhereIsMyOrder` route (with a ticket) or by an admin
//! from the order page (without one).
//!
//! ```text
//! run(StartInvestigationInput):
//!   ctx.run open-investigation    insert `investigations` (open); the facts (parcel, refundable, threshold)
//!   Order/record_event            investigation_opened (sets investigation_open)
//!   Poller(5s, 5 min) over Carrier/open_trace       the first call opens the trace; the carrier
//!                                                   answers after a short while (retried by Restate
//!                                                   while the carrier API is down)
//!   found → close (resurfaced), the ticket replies "found, arriving <date>" → end
//!   no information (or no answer in time):
//!     HumanTasks/create            investigation_decision, carrying this workflow's awakeable
//!     await awakeable              offer_refund | offer_reship | offer_both | custom_reply
//!     custom_reply → the ticket sends it → end
//!     no ticket → refund / replacement right away (the person decided); offer_both → escalated
//!     loop round 1..=2:
//!       the ticket sends the offer (round 2: "sorry, which one?") and registers a fresh
//!       awakeable as its waiter (AskCustomer); the ticket resolves it with the next customer
//!       message, `no_reply` after the reminder, or `withdrawn` when a person takes over
//!       verdict = one Clef choice {accept_refund, accept_reship, decline, unclear} (`offer_step`),
//!                 asked with the router's screen (wants a person, legal threat → escalate,
//!                 unless declined: that goes to a person anyway, `reply_step`)
//!       accept_refund → plan (nothing left / already under way → escalate), then
//!                       RefundWorkflow (pre-approved: a person made the offer), awaited; its
//!                       result email closes the ticket
//!       accept_reship → Order/place (a store-paid replacement, ships like any order)
//!       decline       → the ticket goes to a person with an `offers_declined` task
//!       unclear / low p → ask again; after round 2 escalate
//! ```
//!
//! The workflow writes its own `investigations` row (the only writer) and reports to the ticket
//! with `SupportTicket/route_result` and no generation (always applied). The carrier call is a
//! fixed step, not an agent's tool call: there is nothing for an LLM to decide.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use restate_ext::ContextClockExt;
use restate_ext::poll::{PollOutcome, Poller};
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::AgentRunId;
use techdepo_api_types::ingress::{
    InvestigationFinal, PlacedOrder, StartInvestigationInput, StartRefundInput,
    api_error_from_terminal, derive_id,
};
use techdepo_api_types::tasks::{HumanDecision, TaskContext};
use techdepo_api_types::vocab::{
    AgentKind, AgentRunStatus, AgentStepKind, Awaiting, InvestigationDecision,
    InvestigationOutcome, InvestigationStatus, OrderEventKind, RefKind, RefundReason, RefundStatus,
    TraceStatus,
};
use techdepo_api_types::{Evidence, ingress::keys};
use techdepo_db::OrgScope;
use techdepo_db::agent_runs::{NewAgentRun, NewAgentStep};
use techdepo_db::investigations::{InvestigationUpdate, NewInvestigation};
use techdepo_db::orders::{NewOrder, NewOrderItem};
use techdepo_db::refunds::plan_refund;
use techdepo_domain::{
    Actor, FaultToggles, InvestigationId, LostOutcome, ModelSlot, Money, OrderId, OrderNumber,
    OrderStatus, OrgId, RefundId, ShipmentId, TicketId, TrackingNumber, UserId, VirtualClock,
    WorkCalendar,
};
use techdepo_ports::{ChoiceOption, Classifier, Question};
use time::{Date, OffsetDateTime};

use crate::Deps;
use crate::carrier::{CarrierClient, TraceResult, TrackInput};
use crate::errors::{not_found, not_now, sql};
use crate::human_tasks::{CreateTaskInput, HumanTasksClient, TaskResolution};
use crate::notifications::templates::{OfferKind, SupportTemplate};
use crate::order::{OrderClient, PlaceOrderInput, RecordEventInput};
use crate::refund::RefundWorkflowClient;
use crate::support::contract::{
    self, CustomerReply, Reply, RouteOutcome, RouteResult, WorkflowRef,
};
use crate::support::router;
use crate::util;

/// How often the carrier is asked about the trace.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// How long to wait for the carrier's answer before a person decides without it.
pub const POLL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Offer rounds before the investigation escalates (support.md §4).
pub const MAX_ROUNDS: u32 = 2;

/// What the customer's reply to the offer means (one Clef choice).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// They want the refund.
    AcceptRefund,
    /// They want the replacement.
    AcceptReship,
    /// They want neither.
    Decline,
    /// Cannot tell.
    Unclear,
}

impl Verdict {
    /// Every verdict, in the order the classifier is asked.
    pub const ALL: [Self; 4] = [
        Self::AcceptRefund,
        Self::AcceptReship,
        Self::Decline,
        Self::Unclear,
    ];

    /// Option ID.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AcceptRefund => "accept_refund",
            Self::AcceptReship => "accept_reship",
            Self::Decline => "decline",
            Self::Unclear => "unclear",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::AcceptRefund => "The customer accepts the refund (wants their money back).",
            Self::AcceptReship => "The customer accepts the replacement (wants a new parcel sent).",
            Self::Decline => "The customer declines every offer.",
            Self::Unclear => "The reply does not say which offer the customer accepts.",
        }
    }

    /// The verdict with this option ID (`unclear` for anything else).
    pub fn parse(option: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|v| v.as_str() == option)
            .unwrap_or(Self::Unclear)
    }
}

/// What follows a reply to the offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferStep {
    /// Refund (pre-approved).
    Refund,
    /// Send a replacement.
    Reship,
    /// Every offer declined: a person takes over.
    Declined,
    /// Ask again.
    Clarify,
    /// Unclear twice: a person takes over.
    Escalate,
}

/// The offer a person chose.
pub fn offer_kind(decision: &HumanDecision) -> Option<OfferKind> {
    match decision {
        HumanDecision::OfferRefund => Some(OfferKind::Refund),
        HumanDecision::OfferReship => Some(OfferKind::Reship),
        HumanDecision::OfferBoth => Some(OfferKind::Both),
        _ => None,
    }
}

/// The decision recorded on the investigation.
pub fn decision_of(decision: &HumanDecision) -> Option<InvestigationDecision> {
    match decision {
        HumanDecision::OfferRefund => Some(InvestigationDecision::OfferRefund),
        HumanDecision::OfferReship => Some(InvestigationDecision::OfferReship),
        HumanDecision::OfferBoth => Some(InvestigationDecision::OfferBoth),
        HumanDecision::CustomReply { .. } => Some(InvestigationDecision::CustomReply),
        _ => None,
    }
}

/// Interprets the verdict on `offer` (support.md §4): an accepted offer that was made, or a
/// decline, at or above `threshold`; anything else is asked again once, then escalated. Accepting
/// an offer that was not made counts as unclear.
pub fn offer_step(
    offer: OfferKind,
    verdict: Verdict,
    p: f64,
    threshold: f64,
    round: u32,
) -> OfferStep {
    let refund = matches!(offer, OfferKind::Refund | OfferKind::Both);
    let reship = matches!(offer, OfferKind::Reship | OfferKind::Both);
    let step = match verdict {
        Verdict::AcceptRefund if refund => Some(OfferStep::Refund),
        Verdict::AcceptReship if reship => Some(OfferStep::Reship),
        Verdict::Decline => Some(OfferStep::Declined),
        _ => None,
    };
    match step {
        Some(step) if p >= threshold => step,
        _ if round < MAX_ROUNDS => OfferStep::Clarify,
        _ => OfferStep::Escalate,
    }
}

/// [`offer_step`] for a reply that may also ask for a person or threaten legal action
/// (`urgent`): that escalates at once, unless the reply already goes to a person (a decline gets
/// its `offers_declined` task, which says more than a plain escalation).
pub fn reply_step(
    offer: OfferKind,
    verdict: Verdict,
    p: f64,
    threshold: f64,
    round: u32,
    urgent: bool,
) -> OfferStep {
    match offer_step(offer, verdict, p, threshold, round) {
        step @ (OfferStep::Declined | OfferStep::Escalate) => step,
        _ if urgent => OfferStep::Escalate,
        step => step,
    }
}

/// The one classifier question about the customer's reply.
pub fn verdict_question(offer: OfferKind) -> Question {
    let offered = match offer {
        OfferKind::Refund => "a refund",
        OfferKind::Reship => "a free replacement parcel",
        OfferKind::Both => "a choice between a refund and a free replacement parcel",
    };
    Question::choice(
        "verdict",
        format!(
            "The shop's carrier lost the customer's parcel and the shop offered {offered}. \
             Which offer does the customer's reply accept?"
        ),
        Verdict::ALL
            .iter()
            .map(|v| ChoiceOption::new(v.as_str(), v.description()))
            .collect(),
    )
}

/// The questions about a reply to the offer: the verdict, plus the router's urgent screen.
pub fn reply_questions(offer: OfferKind) -> Vec<Question> {
    let mut questions = vec![verdict_question(offer)];
    questions.extend(router::screen_questions());
    questions
}

/// What [`Investigation::interpret`] read from a reply.
struct Interpreted {
    verdict: Verdict,
    p: f64,
    /// A request for a person or a legal threat.
    urgent: Vec<techdepo_api_types::support::EscalationCause>,
    run: AgentRunId,
}

/// What `open-investigation` journals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Opened {
    number: OrderNumber,
    user_id: UserId,
    shipment_id: Option<ShipmentId>,
    tracking_number: Option<TrackingNumber>,
    clock: VirtualClock,
    shipped_on: Option<Date>,
    refundable: Money,
    threshold: f64,
    /// The org's workday calendar (absent in journals written before holidays were
    /// configurable: no holidays).
    #[serde(default)]
    calendar: WorkCalendar,
}

/// The `Investigation` workflow.
pub struct Investigation {
    deps: Arc<Deps>,
}

/// One investigation's identity, passed around the steps.
struct Case<'a> {
    id: InvestigationId,
    org: &'a OrgId,
    order: &'a OrderId,
    ticket: Option<&'a TicketId>,
    /// Reads the customer's replies to offers: the `offer_reply` model of the settings read
    /// at the start of the run, so the whole workflow uses one model.
    offers: Arc<dyn Classifier>,
    /// That model's ID, for the trace of a failed call.
    offers_model: String,
}

impl Case<'_> {
    fn scope(&self) -> OrgScope {
        OrgScope::verified(self.org.clone())
    }

    /// Reports to the ticket (no generation: always applied).
    fn tell(&self, ctx: &WorkflowContext<'_>, outcome: RouteOutcome, run: Option<AgentRunId>) {
        let Some(ticket) = self.ticket else {
            return;
        };
        let result = RouteResult {
            org_id: self.org.clone(),
            generation: None,
            route: None,
            order: None,
            agent_run_id: run,
            outcome,
        };
        let _ = ctx
            .request::<_, ()>(
                contract::route_result_target(self.org, ticket),
                Json(result),
            )
            .send();
    }
}

fn template_reply(template: SupportTemplate, resolve: bool) -> RouteOutcome {
    RouteOutcome::Reply {
        reply: Reply::template(template),
        resolve,
    }
}

fn escalate(reason: impl Into<String>) -> RouteOutcome {
    RouteOutcome::Escalate {
        reason: reason.into(),
        causes: Vec::new(),
    }
}

async fn now(ctx: &WorkflowContext<'_>) -> Result<OffsetDateTime, TerminalError> {
    Ok(util::to_offset(ctx.now().await?))
}

impl Investigation {
    /// The workflow over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }

    /// Records progress on the `investigations` row.
    async fn record(
        &self,
        ctx: &WorkflowContext<'_>,
        case: &Case<'_>,
        name: &str,
        update: InvestigationUpdate,
    ) -> Result<(), TerminalError> {
        let at = now(ctx).await?;
        let db = self.deps.db.clone();
        let (scope, id) = (case.scope(), case.id.clone());
        ctx.run(|| async move {
            db.batch(&[db.investigations(&scope).update_statement(&id, &update, at)])
                .await
                .map_err(sql)?;
            Ok(())
        })
        .name(name)
        .await
    }

    /// Closes the investigation (and the order's flag).
    async fn close(
        &self,
        ctx: &WorkflowContext<'_>,
        case: &Case<'_>,
        status: InvestigationStatus,
        outcome: InvestigationOutcome,
        mut update: InvestigationUpdate,
        summary: String,
    ) -> Result<InvestigationOutcome, TerminalError> {
        update.status = Some(status);
        update.outcome = Some(outcome);
        self.record(ctx, case, "close-investigation", update)
            .await?;
        ctx.object_client::<OrderClient>(case.order.as_str())
            .record_event(Json(RecordEventInput {
                org_id: case.org.clone(),
                kind: OrderEventKind::InvestigationClosed,
                actor: Actor::System,
                actor_user_id: None,
                summary,
                reference: Some((RefKind::Investigation, case.id.to_string())),
                data: serde_json::json!({ "outcome": outcome.as_str() }),
                refund_pending: None,
                investigation_open: Some(false),
                invocation_id: Some(ctx.invocation_id().to_owned()),
            }))
            .call()
            .await?;
        Ok(outcome)
    }

    /// Insert the row and gather the facts; then flag the order.
    async fn open(
        &self,
        ctx: &WorkflowContext<'_>,
        case: &Case<'_>,
        input: &StartInvestigationInput,
    ) -> Result<Opened, TerminalError> {
        let at = now(ctx).await?;
        let db = self.deps.db.clone();
        let (scope, id, req) = (case.scope(), case.id.clone(), input.clone());
        let opened: Json<Opened> = ctx
            .run(|| async move {
                let orders = db.orders(&scope);
                let order = orders
                    .get(&req.order_id)
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| not_found("order"))?;
                let investigations = db.investigations(&scope);
                let existing = investigations.get(&id).await.map_err(sql)?;
                if existing.is_none() && order.status != OrderStatus::Shipped {
                    return Err(not_now(format!(
                        "{} can only be investigated while shipped",
                        order.number
                    )));
                }
                let shipment = db
                    .shipments(&scope)
                    .for_order(&order.id)
                    .await
                    .map_err(sql)?;
                let payments = db
                    .payments(&scope)
                    .for_order(&order.id)
                    .await
                    .map_err(sql)?;
                let refunds = db.refunds(&scope).for_order(&order.id).await.map_err(sql)?;
                let refundable =
                    plan_refund(&payments, &refunds, RefundReason::LostParcel, None, None)
                        .map_or(order.total, |p| p.amount);
                let settings = db.orgs(&scope).settings().await.map_err(sql)?;
                let clock = orders.clock(&order.id).await.map_err(sql)?;
                if existing.is_none() {
                    let record = NewInvestigation {
                        id: id.clone(),
                        order_id: order.id.clone(),
                        ticket_id: req.ticket_id.clone(),
                        shipment_id: shipment.as_ref().map(|s| s.id.clone()),
                        opened_by: req.opened_by,
                        opened_by_user_id: req.opened_by_user.clone(),
                        note: req.note.clone(),
                        created_at: at,
                    };
                    db.batch(&[investigations.insert_statement(&record)])
                        .await
                        .map_err(sql)?;
                }
                Ok(Json(Opened {
                    number: order.number,
                    user_id: order.user_id,
                    shipment_id: shipment.as_ref().map(|s| s.id.clone()),
                    tracking_number: shipment.as_ref().map(|s| s.tracking_number.clone()),
                    clock,
                    shipped_on: shipment.as_ref().map(|s| s.shipped_on),
                    refundable,
                    threshold: settings.escalation_threshold,
                    calendar: settings.calendar(),
                }))
            })
            .name("open-investigation")
            .await?;
        let opened = opened.into_inner();
        ctx.object_client::<OrderClient>(case.order.as_str())
            .record_event(Json(RecordEventInput {
                org_id: case.org.clone(),
                kind: OrderEventKind::InvestigationOpened,
                actor: input.opened_by,
                actor_user_id: input.opened_by_user.clone(),
                summary: match case.ticket {
                    Some(t) => format!("Carrier investigation opened (ticket {t})"),
                    None => "Carrier investigation opened".to_owned(),
                },
                reference: Some((RefKind::Investigation, case.id.to_string())),
                data: serde_json::json!({ "note": input.note }),
                refund_pending: None,
                investigation_open: Some(true),
                invocation_id: Some(ctx.invocation_id().to_owned()),
            }))
            .call()
            .await?;
        Ok(opened)
    }

    /// Opens the trace and polls the carrier until it answers (or the time is up).
    async fn trace(
        &self,
        ctx: &WorkflowContext<'_>,
        case: &Case<'_>,
        opened: &Opened,
    ) -> Result<Option<TraceResult>, TerminalError> {
        let sim_now = opened.clock.now(now(ctx).await?);
        let poller = Poller::builder(POLL_INTERVAL)
            .timeout(POLL_TIMEOUT)
            .build()?;
        let input = TrackInput {
            org_id: case.org.clone(),
            order_id: case.order.clone(),
            sim_now,
        };
        let polled = poller
            .poll(ctx, || {
                let input = input.clone();
                async move {
                    // A failed call stops polling with its (terminal) error.
                    let called = ctx
                        .service_client::<CarrierClient>()
                        .open_trace(Json(input))
                        .call()
                        .await;
                    Ok(match called {
                        Ok(result) if result.0.status == TraceStatus::Open => {
                            ControlFlow::Continue(result.into_inner())
                        }
                        Ok(result) => ControlFlow::Break(Ok(result.into_inner())),
                        Err(e) => ControlFlow::Break(Err(e)),
                    })
                }
            })
            .await
            .map_err(|e| {
                let error: &(dyn std::error::Error + Send + Sync) = e.as_ref();
                TerminalError::new(error.to_string())
            })?;
        match polled {
            PollOutcome::Finished(result) => result.map(Some),
            PollOutcome::Failed { .. } => Ok(None),
        }
    }

    /// Asks the classifier what the reply means, and (in the same pass) whether it asks for a
    /// person or threatens legal action; records a `reply_interpreter` trace. A classifier that
    /// keeps failing counts as unclear.
    async fn interpret(
        &self,
        ctx: &WorkflowContext<'_>,
        case: &Case<'_>,
        offer: OfferKind,
        text: &str,
        round: u32,
    ) -> Result<Interpreted, TerminalError> {
        let run_id = AgentRunId::new(derive_id(
            "run",
            &[
                case.org.as_str(),
                case.id.as_str(),
                "reply",
                &round.to_string(),
            ],
        ));
        let started = now(ctx).await?;
        let questions = reply_questions(offer);
        let classifier = case.offers.clone();
        let (q, t) = (questions.clone(), text.to_owned());
        let step = format!("interpret-reply-{round}");
        let result: Result<Json<techdepo_ports::Classification>, TerminalError> = ctx
            .run(|| async move {
                match classifier.classify(&t, &q).await {
                    Ok(c) => Ok(Json(c)),
                    Err(e) if e.is_retryable() => Err(HandlerError::from(e)),
                    Err(e) => Err(TerminalError::new(e.to_string()).into()),
                }
            })
            .name(&step)
            .retry_policy(
                RunRetryPolicy::default()
                    .initial_delay(Duration::from_millis(500))
                    .max_attempts(4),
            )
            .await;
        let finished = now(ctx).await?;
        let (classification, error) = match result {
            Ok(c) => (Some(c.into_inner()), None),
            Err(e) => (None, Some(e.message().to_owned())),
        };
        let (verdict, p) = classification
            .as_ref()
            .and_then(|c| {
                let answer = c.answer("verdict")?;
                let choice = c.choice("verdict")?;
                Some((
                    Verdict::parse(choice),
                    answer.probability_of(choice).unwrap_or(0.0),
                ))
            })
            .unwrap_or((Verdict::Unclear, 0.0));
        let urgent = classification
            .as_ref()
            .map(|c| router::urgent_causes(&router::Triage::read(c)))
            .unwrap_or_default();
        let usage = classification.as_ref().and_then(|c| c.usage);
        let run = NewAgentRun {
            id: run_id.clone(),
            ticket_id: case.ticket.cloned(),
            investigation_id: Some(case.id.clone()),
            agent: AgentKind::ReplyInterpreter,
            model: classification
                .as_ref()
                .map_or_else(|| case.offers_model.clone(), |c| c.model.clone()),
            status: if classification.is_some() {
                AgentRunStatus::Completed
            } else {
                AgentRunStatus::Failed
            },
            outcome: None,
            system_prompt: None,
            input_tokens: usage.map_or(0, |u| u.input_tokens),
            output_tokens: usage.map_or(0, |u| u.output_tokens),
            step_count: 1,
            attempt: 1,
            confidence: None,
            guardrails: None,
            error: error.clone(),
            started_at: started,
            finished_at: Some(finished),
        };
        let trace_step = NewAgentStep {
            idx: 0,
            kind: AgentStepKind::Classification,
            name: run.model.clone(),
            journal_name: Some(step),
            input: Some(serde_json::json!({ "text": text, "questions": questions })),
            output: match (&classification, &error) {
                (Some(c), _) => serde_json::to_value(c).ok(),
                (None, Some(e)) => Some(serde_json::json!({ "error": e })),
                (None, None) => None,
            },
            is_error: classification.is_none(),
            input_tokens: run.input_tokens,
            output_tokens: run.output_tokens,
            started_at: started,
            duration_ms: u32::try_from((finished - started).whole_milliseconds().max(0))
                .unwrap_or(u32::MAX),
        };
        let db = self.deps.db.clone();
        let scope = case.scope();
        // The classifier's tokens count towards the org's daily budget (recorded, not gated).
        let usage = usage.filter(|u| u.total() > 0).map(|u| {
            techdepo_llm::UsageLedger::record_statement(
                scope.org_id(),
                finished.date(),
                &run.model,
                u,
            )
        });
        ctx.run(|| async move {
            let runs = db.agent_runs(&scope);
            let mut statements = vec![
                runs.insert_run_statement(&run).map_err(sql)?,
                runs.insert_step_statement(&run.id, &trace_step)
                    .map_err(sql)?,
            ];
            statements.extend(usage);
            db.batch(&statements).await.map_err(sql)?;
            Ok(())
        })
        .name(format!("record-interpretation-{round}"))
        .await?;
        Ok(Interpreted {
            verdict,
            p,
            urgent,
            run: run_id,
        })
    }

    /// Refunds the lost order (pre-approved: a person made the offer) and waits for the refund.
    /// It is planned first: when nothing is left to refund (an admin refunded the order while the
    /// offer was out) or a refund is already under way, a person takes over instead. With a
    /// ticket, the ticket waits on the refund, whose result email resolves it.
    async fn refund(
        &self,
        ctx: &WorkflowContext<'_>,
        case: &Case<'_>,
        decided_by: &UserId,
    ) -> Result<InvestigationOutcome, TerminalError> {
        let refund_id = RefundId::new(derive_id(
            "ref",
            &[case.org.as_str(), case.id.as_str(), "lost_parcel"],
        ));
        let db = self.deps.db.clone();
        let (scope, order) = (case.scope(), case.order.clone());
        let blocked: Json<Option<String>> = ctx
            .run(|| async move {
                let payments = db.payments(&scope).for_order(&order).await.map_err(sql)?;
                let refunds = db.refunds(&scope).for_order(&order).await.map_err(sql)?;
                Ok(Json(
                    plan_refund(&payments, &refunds, RefundReason::LostParcel, None, None)
                        .err()
                        .map(|e| e.message),
                ))
            })
            .name("plan-refund")
            .await?;
        if let Some(why) = blocked.into_inner() {
            let reason = format!("The accepted lost-parcel refund cannot start: {why}");
            case.tell(ctx, escalate(reason.clone()), None);
            return self
                .close(
                    ctx,
                    case,
                    InvestigationStatus::Escalated,
                    InvestigationOutcome::Escalated,
                    InvestigationUpdate::default(),
                    reason,
                )
                .await;
        }
        // Before the refund starts, so the ticket knows about it before it reports.
        case.tell(
            ctx,
            RouteOutcome::WaitingOnWorkflow {
                workflow: WorkflowRef::Refund(refund_id.clone()),
                reply: None,
            },
            None,
        );
        let refunded = ctx
            .workflow_client::<RefundWorkflowClient>(keys::refund(&refund_id))
            .run(Json(StartRefundInput {
                org_id: case.org.clone(),
                order_id: case.order.clone(),
                amount: None,
                reason: RefundReason::LostParcel,
                reason_text: Some(format!(
                    "Parcel lost in transit (investigation {})",
                    case.id
                )),
                evidence: Vec::new(),
                pre_approved: true,
                requested_by: Actor::Human,
                requested_by_user: Some(decided_by.clone()),
                charge_payment_id: None,
                ticket_id: case.ticket.cloned(),
                investigation_id: Some(case.id.clone()),
            }))
            .call()
            .await;
        // The refund told the ticket how it ended (`RefundFinished`, or an escalation when it
        // failed); a refused payment of a promised refund also goes to a person.
        let status = refunded.map(|r| r.into_inner().status);
        if matches!(status, Ok(s) if s != RefundStatus::Completed) {
            case.tell(
                ctx,
                escalate("The accepted lost-parcel refund could not be paid"),
                None,
            );
        }
        let (status, outcome, summary) = match status {
            Ok(RefundStatus::Completed) => (
                InvestigationStatus::Resolved,
                InvestigationOutcome::Refunded,
                "Investigation closed: the lost parcel is refunded".to_owned(),
            ),
            Ok(_) => (
                InvestigationStatus::Escalated,
                InvestigationOutcome::Escalated,
                "Investigation escalated: the lost-parcel refund could not be paid".to_owned(),
            ),
            Err(e) => (
                InvestigationStatus::Escalated,
                InvestigationOutcome::Escalated,
                format!(
                    "Investigation escalated: the lost-parcel refund failed: {}",
                    api_error_from_terminal(e.code(), e.message()).message
                ),
            ),
        };
        self.close(
            ctx,
            case,
            status,
            outcome,
            InvestigationUpdate {
                refund_id: Some(refund_id),
                ..InvestigationUpdate::default()
            },
            summary,
        )
        .await
    }

    /// Places the store-paid replacement order. `Err` carries why it could not be placed.
    async fn reship(
        &self,
        ctx: &WorkflowContext<'_>,
        case: &Case<'_>,
    ) -> Result<Result<PlacedOrder, String>, TerminalError> {
        let replacement = OrderId::new(derive_id(
            "ord",
            &[case.org.as_str(), case.id.as_str(), "replacement"],
        ));
        let db = self.deps.db.clone();
        let (scope, original, id) = (case.scope(), case.order.clone(), replacement.clone());
        let order: Json<NewOrder> = ctx
            .run(|| async move {
                let orders = db.orders(&scope);
                let row = orders
                    .get(&original)
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| not_found("order"))?;
                let items = orders.items(&original).await.map_err(sql)?;
                Ok(Json(NewOrder {
                    id,
                    user_id: row.user_id,
                    customer_email: row.customer_email,
                    customer_name: row.customer_name,
                    items: items
                        .into_iter()
                        .map(|i| NewOrderItem {
                            product_id: i.product_id,
                            sku: i.sku,
                            name: i.name,
                            category: i.category,
                            unit_price: i.unit_price,
                            quantity: i.quantity,
                        })
                        .collect(),
                    shipping: Money::ZERO,
                    vat_rate: row.vat_rate,
                    payment_method: techdepo_api_types::vocab::PaymentMethod::Store,
                    shipping_address: row.shipping_address,
                    billing_address: row.billing_address,
                    compat_override: row.compat_override,
                    replacement_for: Some(original),
                }))
            })
            .name("load-replacement")
            .await?;
        let placed = ctx
            .object_client::<OrderClient>(replacement.as_str())
            .place(Json(PlaceOrderInput {
                org_id: case.org.clone(),
                order: order.into_inner(),
                faults: FaultToggles::default(),
                actor: Actor::System,
                actor_user_id: None,
            }))
            .call()
            .await;
        let placed = match placed {
            Ok(placed) => placed.into_inner(),
            Err(e) => return Ok(Err(api_error_from_terminal(e.code(), e.message()).message)),
        };
        ctx.object_client::<OrderClient>(case.order.as_str())
            .record_event(Json(RecordEventInput {
                org_id: case.org.clone(),
                kind: OrderEventKind::ReplacementCreated,
                actor: Actor::System,
                actor_user_id: None,
                summary: format!(
                    "Replacement order {} placed (paid by the store)",
                    placed.number
                ),
                reference: Some((RefKind::Order, placed.order_id.to_string())),
                data: serde_json::json!({ "replacement": placed.number }),
                refund_pending: None,
                investigation_open: None,
                invocation_id: Some(ctx.invocation_id().to_owned()),
            }))
            .call()
            .await?;
        Ok(Ok(placed))
    }

    async fn reship_and_close(
        &self,
        ctx: &WorkflowContext<'_>,
        case: &Case<'_>,
        number: OrderNumber,
    ) -> Result<InvestigationOutcome, TerminalError> {
        match self.reship(ctx, case).await? {
            Ok(placed) => {
                case.tell(
                    ctx,
                    template_reply(
                        SupportTemplate::ReplacementPlaced {
                            order_number: number,
                            replacement: placed.number,
                        },
                        true,
                    ),
                    None,
                );
                self.close(
                    ctx,
                    case,
                    InvestigationStatus::Resolved,
                    InvestigationOutcome::Reshipped,
                    InvestigationUpdate {
                        replacement_order_id: Some(placed.order_id),
                        ..InvestigationUpdate::default()
                    },
                    format!("Investigation closed: replacement {} placed", placed.number),
                )
                .await
            }
            Err(why) => {
                let reason = format!("The replacement could not be placed: {why}");
                case.tell(ctx, escalate(reason.clone()), None);
                self.close(
                    ctx,
                    case,
                    InvestigationStatus::Escalated,
                    InvestigationOutcome::Escalated,
                    InvestigationUpdate::default(),
                    reason,
                )
                .await
            }
        }
    }

    /// The whole investigation; `Err` is a terminal failure (the caller escalates).
    async fn investigate(
        &self,
        ctx: &WorkflowContext<'_>,
        case: &Case<'_>,
        input: &StartInvestigationInput,
        opened_flag: &mut bool,
    ) -> Result<InvestigationOutcome, TerminalError> {
        let opened = self.open(ctx, case, input).await?;
        *opened_flag = true;
        let n = opened.number;
        self.record(
            ctx,
            case,
            "record-tracing",
            InvestigationUpdate {
                status: Some(InvestigationStatus::Tracing),
                ..InvestigationUpdate::default()
            },
        )
        .await?;

        // -- the carrier's trace --------------------------------------------------------------
        let traced = self.trace(ctx, case, &opened).await?;
        if let Some(found) = traced
            .as_ref()
            .filter(|t| t.status == TraceStatus::Resurfaced)
        {
            let eta = found.shipment.eta;
            case.tell(
                ctx,
                template_reply(
                    SupportTemplate::ParcelFound {
                        order_number: n,
                        eta,
                    },
                    true,
                ),
                None,
            );
            return self
                .close(
                    ctx,
                    case,
                    InvestigationStatus::Resolved,
                    InvestigationOutcome::Resurfaced,
                    InvestigationUpdate {
                        carrier_result: Some(LostOutcome::Resurfaces),
                        ..InvestigationUpdate::default()
                    },
                    format!("Investigation closed: the carrier found the parcel, expected {eta}"),
                )
                .await;
        }

        // -- no information: a person decides what to offer ----------------------------------
        let tracking = opened
            .tracking_number
            .clone()
            .unwrap_or_else(|| TrackingNumber::new("unknown"));
        let sim_today = opened.clock.now(now(ctx).await?).date();
        let workdays = opened
            .shipped_on
            .map(|shipped| opened.calendar.workdays_between(shipped, sim_today));
        let mut evidence = Vec::new();
        if let Some(shipment_id) = &opened.shipment_id {
            evidence.push(Evidence::CarrierTrace {
                shipment_id: shipment_id.clone(),
                tracking_number: tracking.clone(),
                status: traced.as_ref().map_or(TraceStatus::Open, |t| t.status),
                outcome: traced.as_ref().and_then(|t| t.outcome),
            });
        }
        evidence.push(Evidence::OrderState {
            order_number: n,
            status: OrderStatus::Shipped,
            workdays_since_ship: workdays,
        });
        self.record(
            ctx,
            case,
            "record-no-information",
            InvestigationUpdate {
                status: Some(InvestigationStatus::AwaitingDecision),
                carrier_result: Some(LostOutcome::Gone),
                ..InvestigationUpdate::default()
            },
        )
        .await?;
        let (awakeable_id, decision) = ctx.awakeable::<Json<TaskResolution>>();
        let task_id = ctx
            .service_client::<HumanTasksClient>()
            .create(Json(CreateTaskInput {
                org_id: case.org.clone(),
                context: TaskContext::InvestigationDecision {
                    investigation_id: case.id.clone(),
                    tracking_number: tracking,
                    carrier_result: LostOutcome::Gone,
                    refundable: opened.refundable,
                },
                title: format!("Lost parcel: what do we offer for {n}?"),
                summary: if traced.is_some() {
                    "The carrier has no information on the parcel".to_owned()
                } else {
                    "The carrier did not answer the trace in time".to_owned()
                },
                evidence,
                awakeable_id: Some(awakeable_id),
                ticket_id: case.ticket.cloned(),
                order_id: Some(case.order.clone()),
                refund_id: None,
                investigation_id: Some(case.id.clone()),
            }))
            .call()
            .await?
            .into_inner();
        self.record(
            ctx,
            case,
            "link-task",
            InvestigationUpdate {
                human_task_id: Some(task_id),
                ..InvestigationUpdate::default()
            },
        )
        .await?;
        let resolution = decision.await?.into_inner();
        self.record(
            ctx,
            case,
            "record-decision",
            InvestigationUpdate {
                decision: decision_of(&resolution.decision),
                ..InvestigationUpdate::default()
            },
        )
        .await?;

        let offer = match (&resolution.decision, offer_kind(&resolution.decision)) {
            (HumanDecision::CustomReply { body }, _) => {
                case.tell(
                    ctx,
                    template_reply(SupportTemplate::Reply { body: body.clone() }, true),
                    None,
                );
                return self
                    .close(
                        ctx,
                        case,
                        InvestigationStatus::Resolved,
                        InvestigationOutcome::CustomReply,
                        InvestigationUpdate::default(),
                        "Investigation closed with a personal reply".to_owned(),
                    )
                    .await;
            }
            (_, Some(offer)) => offer,
            (other, None) => {
                return Err(TerminalError::new(format!(
                    "unexpected investigation decision {:?}",
                    other.kind()
                )));
            }
        };

        // -- without a ticket there is nobody to ask: the person's decision is the outcome -----
        if case.ticket.is_none() {
            return match offer {
                OfferKind::Refund => self.refund(ctx, case, &resolution.decided_by).await,
                OfferKind::Reship => self.reship_and_close(ctx, case, n).await,
                OfferKind::Both => {
                    self.close(
                        ctx,
                        case,
                        InvestigationStatus::Escalated,
                        InvestigationOutcome::Escalated,
                        InvestigationUpdate::default(),
                        "Investigation closed: no ticket to ask the customer which offer they \
                         prefer; contact them directly"
                            .to_owned(),
                    )
                    .await
                }
            };
        }

        // -- the conversation: offer, interpret, act -----------------------------------------
        let refund_amount =
            matches!(offer, OfferKind::Refund | OfferKind::Both).then_some(opened.refundable);
        for round in 1..=MAX_ROUNDS {
            self.record(
                ctx,
                case,
                &format!("record-offer-{round}"),
                InvestigationUpdate {
                    status: Some(InvestigationStatus::AwaitingCustomer),
                    rounds: Some(round),
                    ..InvestigationUpdate::default()
                },
            )
            .await?;
            // A fresh awakeable per turn: the ticket resolves it with the customer's answer.
            let (awakeable_id, answer) = ctx.awakeable::<Json<CustomerReply>>();
            let question = if round == 1 {
                SupportTemplate::Offer {
                    order_number: n,
                    kind: offer,
                    refund_amount,
                }
            } else {
                SupportTemplate::OfferUnclear {
                    order_number: n,
                    kind: offer,
                    refund_amount,
                }
            };
            case.tell(
                ctx,
                RouteOutcome::AskCustomer {
                    question: Reply::template(question),
                    expects: Awaiting::OfferReply,
                    awakeable_id: Some(awakeable_id),
                },
                None,
            );
            let message = match answer.await?.into_inner() {
                CustomerReply::Message { message, .. } => message,
                CustomerReply::NoReply => {
                    let reason = "No reply to the lost-parcel offer after the reminder";
                    case.tell(ctx, escalate(reason), None);
                    return self
                        .close(
                            ctx,
                            case,
                            InvestigationStatus::Escalated,
                            InvestigationOutcome::Escalated,
                            InvestigationUpdate::default(),
                            format!("Investigation escalated: {}", reason.to_lowercase()),
                        )
                        .await;
                }
                CustomerReply::Withdrawn => {
                    return self
                        .close(
                            ctx,
                            case,
                            InvestigationStatus::Escalated,
                            InvestigationOutcome::Escalated,
                            InvestigationUpdate::default(),
                            "Investigation handed over: a person took over the ticket".to_owned(),
                        )
                        .await;
                }
            };
            let Interpreted {
                verdict,
                p,
                urgent,
                run,
            } = self
                .interpret(ctx, case, offer, &message.body, round)
                .await?;
            let step = reply_step(
                offer,
                verdict,
                p,
                opened.threshold,
                round,
                !urgent.is_empty(),
            );
            if step == OfferStep::Escalate && !urgent.is_empty() {
                // A request for a person or a legal threat is never acted on automatically
                // (support.md §8).
                let reason = router::escalation_reason(&urgent, None);
                case.tell(
                    ctx,
                    RouteOutcome::Escalate {
                        reason: reason.clone(),
                        causes: urgent,
                    },
                    Some(run),
                );
                return self
                    .close(
                        ctx,
                        case,
                        InvestigationStatus::Escalated,
                        InvestigationOutcome::Escalated,
                        InvestigationUpdate::default(),
                        format!("Investigation escalated: {}", reason.to_lowercase()),
                    )
                    .await;
            }
            match step {
                OfferStep::Refund => return self.refund(ctx, case, &resolution.decided_by).await,
                OfferStep::Reship => return self.reship_and_close(ctx, case, n).await,
                OfferStep::Declined => {
                    case.tell(
                        ctx,
                        RouteOutcome::OffersDeclined {
                            investigation_id: case.id.clone(),
                            last_reply: message.body.chars().take(500).collect(),
                        },
                        Some(run),
                    );
                    return self
                        .close(
                            ctx,
                            case,
                            InvestigationStatus::Escalated,
                            InvestigationOutcome::Declined,
                            InvestigationUpdate::default(),
                            "Investigation closed: the customer declined every offer".to_owned(),
                        )
                        .await;
                }
                OfferStep::Clarify => {}
                OfferStep::Escalate => {
                    let reason = "Two unclear replies to the lost-parcel offer";
                    case.tell(ctx, escalate(reason), Some(run));
                    return self
                        .close(
                            ctx,
                            case,
                            InvestigationStatus::Escalated,
                            InvestigationOutcome::Escalated,
                            InvestigationUpdate::default(),
                            format!("Investigation escalated: {}", reason.to_lowercase()),
                        )
                        .await;
                }
            }
        }
        Err(TerminalError::new(
            "the offer conversation ended without an outcome",
        ))
    }
}

#[restate_sdk::workflow(name = "Investigation")]
impl Investigation {
    /// Runs one investigation to its outcome.
    #[handler]
    async fn run(
        &self,
        ctx: WorkflowContext<'_>,
        input: Json<StartInvestigationInput>,
    ) -> Result<Json<InvestigationFinal>, HandlerError> {
        let input = input.into_inner();
        // A read that keeps failing uses the defaults (journaled as failed, so replays agree).
        let models = ctx
            .run(crate::models::read(&self.deps))
            .name(crate::models::STEP)
            .await
            .map_or_else(|_| self.deps.models.defaults(), Json::into_inner);
        let case = Case {
            id: InvestigationId::new(ctx.key()),
            org: &input.org_id,
            order: &input.order_id,
            ticket: input.ticket_id.as_ref(),
            offers: crate::models::classifier(&self.deps, &models, ModelSlot::OfferReply),
            offers_model: models.offer_reply.model.clone(),
        };
        let mut opened = false;
        match self.investigate(&ctx, &case, &input, &mut opened).await {
            Ok(outcome) => Ok(Json(InvestigationFinal { outcome })),
            Err(error) => {
                // Never leave the ticket waiting on a workflow that died.
                let message = api_error_from_terminal(error.code(), error.message()).message;
                case.tell(
                    &ctx,
                    escalate(format!("The carrier investigation failed: {message}")),
                    None,
                );
                if opened {
                    self.close(
                        &ctx,
                        &case,
                        InvestigationStatus::Escalated,
                        InvestigationOutcome::Escalated,
                        InvestigationUpdate::default(),
                        format!("Investigation failed: {message}"),
                    )
                    .await?;
                }
                Err(error.into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: f64 = 0.7;

    #[test]
    fn accepted_offers_act() {
        use OfferKind::{Both, Refund, Reship};
        assert_eq!(
            offer_step(Both, Verdict::AcceptRefund, 0.9, T, 1),
            OfferStep::Refund
        );
        assert_eq!(
            offer_step(Both, Verdict::AcceptReship, 0.9, T, 1),
            OfferStep::Reship
        );
        assert_eq!(
            offer_step(Refund, Verdict::AcceptRefund, 0.7, T, 2),
            OfferStep::Refund
        );
        assert_eq!(
            offer_step(Reship, Verdict::Decline, 0.8, T, 1),
            OfferStep::Declined
        );
    }

    #[test]
    fn unclear_or_unoffered_asks_once_then_escalates() {
        use OfferKind::{Both, Refund};
        assert_eq!(
            offer_step(Both, Verdict::Unclear, 0.95, T, 1),
            OfferStep::Clarify
        );
        assert_eq!(
            offer_step(Both, Verdict::Unclear, 0.95, T, 2),
            OfferStep::Escalate
        );
        assert_eq!(
            offer_step(Both, Verdict::AcceptReship, 0.6, T, 1),
            OfferStep::Clarify,
            "below the threshold"
        );
        assert_eq!(
            offer_step(Refund, Verdict::AcceptReship, 0.95, T, 1),
            OfferStep::Clarify,
            "a replacement was not offered"
        );
        assert_eq!(
            offer_step(Refund, Verdict::Decline, 0.5, T, 2),
            OfferStep::Escalate
        );
    }

    #[test]
    fn urgent_replies_are_never_acted_on() {
        use OfferKind::Both;
        assert_eq!(
            reply_step(Both, Verdict::AcceptRefund, 0.9, T, 1, true),
            OfferStep::Escalate
        );
        assert_eq!(
            reply_step(Both, Verdict::Unclear, 0.9, T, 1, true),
            OfferStep::Escalate
        );
        assert_eq!(
            reply_step(Both, Verdict::Decline, 0.9, T, 1, true),
            OfferStep::Declined,
            "a decline reaches a person with its own task"
        );
        assert_eq!(
            reply_step(Both, Verdict::AcceptRefund, 0.9, T, 1, false),
            OfferStep::Refund
        );
    }

    #[test]
    fn verdicts_and_decisions() {
        for v in Verdict::ALL {
            assert_eq!(Verdict::parse(v.as_str()), v);
        }
        assert_eq!(Verdict::parse("maybe"), Verdict::Unclear);
        assert_eq!(offer_kind(&HumanDecision::OfferBoth), Some(OfferKind::Both));
        assert_eq!(
            offer_kind(&HumanDecision::CustomReply { body: "x".into() }),
            None
        );
        assert_eq!(
            decision_of(&HumanDecision::CustomReply { body: "x".into() }),
            Some(InvestigationDecision::CustomReply)
        );
        let q = verdict_question(OfferKind::Reship);
        assert_eq!(q.id, "verdict");
    }

    #[test]
    fn the_mock_classifier_reads_offer_replies() {
        use futures::executor::block_on;
        use techdepo_ports::Classifier;
        let mock = techdepo_llm::MockClassifier;
        let verdict = |text: &str| {
            let c = block_on(mock.classify(text, &[verdict_question(OfferKind::Both)])).unwrap();
            let choice = c.choice("verdict").unwrap().to_owned();
            let p = c
                .answer("verdict")
                .unwrap()
                .probability_of(&choice)
                .unwrap();
            (Verdict::parse(&choice), p)
        };
        let (v, p) = verdict("Just send a new one please.");
        assert_eq!(v, Verdict::AcceptReship);
        assert_eq!(offer_step(OfferKind::Both, v, p, T, 1), OfferStep::Reship);
        assert_eq!(verdict("No thanks, neither.").0, Verdict::Decline);
    }

    #[test]
    fn a_reply_that_wants_a_person_or_threatens_is_caught() {
        use futures::executor::block_on;
        use techdepo_ports::Classifier;
        let mock = techdepo_llm::MockClassifier;
        let urgent = |text: &str| {
            let c = block_on(mock.classify(text, &reply_questions(OfferKind::Both))).unwrap();
            router::urgent_causes(&router::Triage::read(&c))
        };
        assert!(!urgent("I'm calling my lawyer about this.").is_empty());
        assert!(!urgent("Let me talk to a real person please.").is_empty());
        assert!(urgent("Just send a new one please.").is_empty());
    }
}
