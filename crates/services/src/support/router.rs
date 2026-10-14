//! `Router` (service, ingress-private): classification and routing (support.md §3).
//!
//! `SupportTicket` sends it a snapshot ([`RouteTicketInput`], one-way) whenever customer messages
//! need handling. Three steps:
//!
//! 1. **Deterministic extraction** (`load-orders`): order numbers from the text
//!    (`OrderNumber::find_all`), matched against the customer's own orders in D1. Only those
//!    can become the route's order; other numbers are passed on as foreign.
//! 2. **Classification** (`classify`): one `Classifier` call answers every question of
//!    [`triage_questions`] in a single pass. The call and its result are recorded as a `router`
//!    trace (`agent_runs`), and the classifier gets a few attempts before the ticket escalates.
//! 3. **Decision** ([`decide`], pure): escalate on a request for a person, a legal threat, the
//!    `other` intent, a top intent below the org threshold or too close to the runner-up; close
//!    "thanks!" without a reply; re-run the route that asked a question when the message answers
//!    it; otherwise dispatch the intent's route.
//!
//! The router then tells the ticket (`SupportTicket/classified`, which stores the probabilities
//! and the route for the admin's "why" panel and says whether this generation is still
//! current), and either starts the route by name (`<Route>/run`, [`contract::route_target`]) or
//! reports the escalation or close itself (`SupportTicket/route_result`). A route that is not
//! deployed escalates, and so does a routing pass that fails terminally.
//!
//! `screen` is the cheap pass for messages the ticket does not triage now (buffered behind a
//! workflow, or superseded by a question sent after them): only [`screen_questions`], and a
//! request for a person or a legal threat escalates the ticket at once.

use std::sync::Arc;
use std::time::Duration;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::AgentRunId;
use techdepo_api_types::ingress::api_error_from_terminal;
use techdepo_api_types::support::{Classification, EscalationCause, IntentProbability};
use techdepo_api_types::vocab::{AgentKind, AgentRunStatus, AgentStepKind, Awaiting, TicketRoute};
use techdepo_db::OrgScope;
use techdepo_db::agent_runs::{NewAgentRun, NewAgentStep};
use techdepo_db::orders::OrderFilter;
use techdepo_domain::{Intent, ModelSlot, OrderNumber, OrderStatus, OrgId, OrgSettings, TicketId};
use techdepo_ports::{ChoiceOption, Question};
use time::OffsetDateTime;

use super::contract::{
    self, OrderBrief, PendingQuestion, RouteOutcome, RouteRequest, RouteResult, RouteTicketInput,
    ScreenInput,
};
use super::routes::now;
use super::ticket::{ClassifiedInput, SupportTicketClient};
use crate::Deps;
use crate::errors::sql;
use crate::util::mint;

/// `P(yes)` at or above which a yes/no question counts as yes.
pub const YES: f64 = 0.5;

/// The questions of one triage pass. `answers_our_question` only when the ticket waits on one.
pub fn triage_questions(pending: Option<&PendingQuestion>) -> Vec<Question> {
    let mut questions = vec![
        Question::choice(
            "intent",
            "What does the customer want from the shop's support team? Pick the closest category.",
            Intent::ALL
                .iter()
                .map(|i| ChoiceOption::new(i.as_str(), i.description()))
                .collect(),
        ),
        Question::noul(
            "contains_request",
            "Does the message ask the shop to do or answer something (more than a thank-you or an acknowledgement)?",
        ),
        Question::noul(
            "wants_human",
            "Does the customer explicitly ask to talk to a human person instead of an automated system?",
        ),
        Question::noul(
            "legal_threat",
            "Does the message threaten legal action, a lawyer, a court case, a chargeback or a complaint to an authority?",
        ),
    ];
    if let Some(p) = pending {
        let asked = match p.expects {
            Awaiting::OrderNumber => "the number of the order the request is about",
            Awaiting::OfferReply => "whether they want the refund or the replacement we offered",
            Awaiting::Clarification => "more details about their request",
        };
        questions.push(Question::noul(
            "answers_our_question",
            format!("We asked the customer for {asked}. Does this message give us that?"),
        ));
    }
    questions
}

/// The numbers [`decide`] works from, read from a classifier answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Triage {
    /// Every intent's probability, most probable first.
    pub probabilities: Vec<IntentProbability>,
    /// `P(the message contains a request)`.
    pub contains_request: f64,
    /// `P(wants a person)`.
    pub wants_human: f64,
    /// `P(legal threat)`.
    pub legal_threat: f64,
    /// `P(answers our question)`, when asked.
    pub answers_our_question: Option<f64>,
}

impl Triage {
    /// Reads the answers to [`triage_questions`]. Missing answers count as "no" (and an
    /// unknown intent as `other`), which escalates rather than guesses.
    pub fn read(c: &techdepo_ports::Classification) -> Self {
        let mut probabilities: Vec<IntentProbability> = match c.answer("intent") {
            Some(techdepo_ports::AnswerValue::Choice { probabilities, .. }) => probabilities
                .iter()
                .filter_map(|o| {
                    o.option
                        .parse::<Intent>()
                        .ok()
                        .map(|intent| IntentProbability {
                            intent,
                            p: o.p.clamp(0.0, 1.0),
                        })
                })
                .collect(),
            _ => Vec::new(),
        };
        if probabilities.is_empty() {
            probabilities.push(IntentProbability {
                intent: Intent::Other,
                p: 0.0,
            });
        }
        probabilities.sort_by(|a, b| b.p.total_cmp(&a.p));
        Self {
            probabilities,
            contains_request: c.yes("contains_request").unwrap_or(1.0),
            wants_human: c.yes("wants_human").unwrap_or(0.0),
            legal_threat: c.yes("legal_threat").unwrap_or(0.0),
            answers_our_question: c.yes("answers_our_question"),
        }
    }

    /// The top intent and its probability.
    pub fn top(&self) -> (Intent, f64) {
        self.probabilities
            .first()
            .map_or((Intent::Other, 0.0), |p| (p.intent, p.p))
    }

    /// The runner-up's probability.
    pub fn runner_up(&self) -> f64 {
        self.probabilities.get(1).map_or(0.0, |p| p.p)
    }
}

/// What the router decided.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Start `route`.
    Dispatch {
        /// The route.
        route: TicketRoute,
        /// The intent it handles.
        intent: Intent,
        /// Its probability.
        intent_p: f64,
        /// Set when the message answers the route's own question.
        resumed: Option<Awaiting>,
    },
    /// Hand to a person.
    Escalate {
        /// Why.
        causes: Vec<EscalationCause>,
    },
    /// Nothing to answer: resolve without a reply.
    Close,
}

/// The questions of a screen (`Router/screen`): only what escalates right away.
pub fn screen_questions() -> Vec<Question> {
    triage_questions(None)
        .into_iter()
        .filter(|q| q.id == "wants_human" || q.id == "legal_threat")
        .collect()
}

/// The causes that escalate whatever else the message says: a request for a person, a legal
/// threat.
pub fn urgent_causes(t: &Triage) -> Vec<EscalationCause> {
    let mut causes = Vec::new();
    if t.wants_human >= YES {
        causes.push(EscalationCause::WantsHuman);
    }
    if t.legal_threat >= YES {
        causes.push(EscalationCause::LegalThreat);
    }
    causes
}

/// The decision for a triage (support.md §3 step 3).
pub fn decide(settings: &OrgSettings, t: &Triage, pending: Option<&PendingQuestion>) -> Decision {
    let mut causes = urgent_causes(t);
    if !causes.is_empty() {
        return Decision::Escalate { causes };
    }
    if let Some(p) = pending
        && t.answers_our_question.is_some_and(|yes| yes >= YES)
    {
        return Decision::Dispatch {
            route: p.route,
            intent: p.intent,
            intent_p: p.intent_p,
            resumed: Some(p.expects),
        };
    }
    if t.contains_request < YES {
        return Decision::Close;
    }
    let (intent, top) = t.top();
    if intent == Intent::Other {
        causes.push(EscalationCause::OtherIntent);
    }
    if top < settings.escalation_threshold {
        causes.push(EscalationCause::LowConfidence);
    }
    if top - t.runner_up() < settings.escalation_margin {
        causes.push(EscalationCause::SmallMargin);
    }
    if causes.is_empty() {
        Decision::Dispatch {
            route: TicketRoute::for_intent(intent),
            intent,
            intent_p: top,
            resumed: None,
        }
    } else {
        Decision::Escalate { causes }
    }
}

/// The escalation reason shown on the task, e.g. `Low confidence (top intent 0.52)`.
pub fn escalation_reason(causes: &[EscalationCause], t: Option<&Triage>) -> String {
    let words: Vec<String> = causes
        .iter()
        .map(|c| match (c, t) {
            (EscalationCause::LowConfidence | EscalationCause::SmallMargin, Some(t)) => {
                let (intent, p) = t.top();
                format!(
                    "{} (top intent {} at {p:.2}, runner-up {:.2})",
                    c.label(),
                    intent.as_str(),
                    t.runner_up()
                )
            }
            _ => c.label().to_owned(),
        })
        .collect();
    words.join("; ")
}

/// The orders a message can be about.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OrderMatch {
    /// The first mentioned number that is one of the customer's orders.
    pub explicit: Option<OrderBrief>,
    /// Mentioned numbers that are not the customer's.
    pub foreign: Vec<OrderNumber>,
    /// Every mentioned number.
    pub mentioned: Vec<OrderNumber>,
}

/// Matches the order numbers in `text` against the customer's orders.
pub fn match_orders(text: &str, customer_orders: &[OrderBrief]) -> OrderMatch {
    let mentioned = OrderNumber::find_all(text);
    let mut out = OrderMatch {
        mentioned: mentioned.clone(),
        ..OrderMatch::default()
    };
    for number in mentioned {
        match customer_orders.iter().find(|o| o.number == number) {
            Some(order) if out.explicit.is_none() => out.explicit = Some(order.clone()),
            Some(_) => {}
            None => out.foreign.push(number),
        }
    }
    out
}

/// What `load-orders` journals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Facts {
    orders: Vec<OrderBrief>,
    settings: OrgSettings,
}

/// The `Router` service.
pub struct Router {
    deps: Arc<Deps>,
}

impl Router {
    /// The service over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }
}

#[restate_sdk::service(name = "Router", ingress_private = true)]
impl Router {
    /// Classifies the ticket's new messages and starts the route (or escalates, or closes). A
    /// terminal failure reports an escalation, so the ticket never waits on a dead router.
    #[handler]
    async fn route(
        &self,
        mut ctx: Context<'_>,
        input: Json<RouteTicketInput>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        if let Err(e) = self.dispatch(&mut ctx, &input).await {
            let error = api_error_from_terminal(e.code(), e.message());
            let result = RouteResult {
                org_id: input.org_id.clone(),
                generation: Some(input.generation),
                route: None,
                order: None,
                agent_run_id: None,
                outcome: RouteOutcome::Escalate {
                    reason: format!("Routing failed: {}", error.message),
                    causes: Vec::new(),
                },
            };
            let _ = ctx
                .request::<_, ()>(
                    contract::route_result_target(&input.org_id, &input.ticket_id),
                    Json(result),
                )
                .send();
        }
        Ok(())
    }

    /// Screens messages that are not triaged now (buffered behind a workflow, or superseded by
    /// a question) for a request for a person or a legal threat; either escalates the ticket
    /// right away (support.md §8). A classifier that keeps failing changes nothing: the messages
    /// are triaged when the workflow reports.
    #[handler]
    async fn screen(
        &self,
        mut ctx: Context<'_>,
        input: Json<ScreenInput>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        let classified = self
            .classify(
                &mut ctx,
                &input.org_id,
                &input.ticket_id,
                &input.text,
                screen_questions(),
                "screen",
            )
            .await?;
        let Some((_, _, c)) = classified else {
            return Ok(());
        };
        let triage = Triage::read(&c);
        let causes = urgent_causes(&triage);
        if causes.is_empty() {
            return Ok(());
        }
        let result = RouteResult {
            org_id: input.org_id.clone(),
            generation: None,
            route: None,
            order: None,
            agent_run_id: None,
            outcome: RouteOutcome::Escalate {
                reason: escalation_reason(&causes, None),
                causes,
            },
        };
        let _ = ctx
            .request::<_, ()>(
                contract::route_result_target(&input.org_id, &input.ticket_id),
                Json(result),
            )
            .send();
        Ok(())
    }
}

impl Router {
    async fn dispatch(
        &self,
        ctx: &mut Context<'_>,
        input: &RouteTicketInput,
    ) -> Result<(), TerminalError> {
        let db = self.deps.db.clone();
        let (org, user) = (input.org_id.clone(), input.user_id.clone());
        let facts: Json<Facts> = ctx
            .run(|| async move {
                let scope = OrgScope::verified(org);
                let settings = db.orgs(&scope).settings().await.map_err(sql)?;
                let filter = OrderFilter {
                    user_id: Some(user),
                    ..OrderFilter::default()
                };
                let (rows, _) = db
                    .orders(&scope)
                    .list(&filter, &techdepo_db::PageRequest::first())
                    .await
                    .map_err(sql)?;
                Ok(Json(Facts {
                    orders: rows
                        .into_iter()
                        .map(|o| OrderBrief {
                            order_id: o.id,
                            number: o.number,
                            status: o.status,
                        })
                        .collect(),
                    settings,
                }))
            })
            .name("load-orders")
            .await?;
        let Facts { orders, settings } = facts.into_inner();
        let text = input.text();
        let matched = match_orders(&text, &orders);

        let (decision, classification, triage) = match input.forced_intent {
            Some(intent) => (
                Decision::Dispatch {
                    route: TicketRoute::for_intent(intent),
                    intent,
                    intent_p: input.forced_intent_p.unwrap_or(0.0),
                    resumed: None,
                },
                None,
                None,
            ),
            None => match self
                .classify(
                    ctx,
                    &input.org_id,
                    &input.ticket_id,
                    &text,
                    triage_questions(input.pending.as_ref()),
                    "classify",
                )
                .await?
            {
                Some((run_id, at, c)) => {
                    let triage = Triage::read(&c);
                    let decision = decide(&settings, &triage, input.pending.as_ref());
                    let classification =
                        api_classification(&c, &triage, &decision, &settings, &matched, run_id, at);
                    (decision, Some(classification), Some(triage))
                }
                None => (Decision::Escalate { causes: Vec::new() }, None, None),
            },
        };

        let (route, intent, intent_p) = match &decision {
            Decision::Dispatch {
                route,
                intent,
                intent_p,
                ..
            } => (*route, Some(*intent), Some(*intent_p)),
            Decision::Escalate { .. } => (
                TicketRoute::Human,
                triage.as_ref().map(|t| t.top().0),
                triage.as_ref().map(|t| t.top().1),
            ),
            Decision::Close => (
                TicketRoute::Human,
                triage.as_ref().map(|t| t.top().0),
                triage.as_ref().map(|t| t.top().1),
            ),
        };
        let order = matched.explicit.clone().or_else(|| input.order.clone());
        let current: Json<bool> = ctx
            .object_client::<SupportTicketClient>(techdepo_api_types::ingress::keys::ticket(
                &input.org_id,
                &input.ticket_id,
            ))
            .classified(Json(ClassifiedInput {
                org_id: input.org_id.clone(),
                generation: input.generation,
                classification,
                intent,
                intent_p,
                route: (!matches!(decision, Decision::Close)).then_some(route),
                order: order.clone(),
                order_number: matched.mentioned.first().copied(),
            }))
            .call()
            .await?;
        if !current.into_inner() {
            return Ok(());
        }

        let report = |outcome: RouteOutcome| RouteResult {
            org_id: input.org_id.clone(),
            generation: Some(input.generation),
            route: Some(route),
            order: order.clone(),
            agent_run_id: None,
            outcome,
        };
        let result = match decision {
            Decision::Dispatch {
                route,
                intent,
                intent_p,
                resumed,
            } => match contract::route_target(route) {
                Some(service) => {
                    let request = RouteRequest {
                        org_id: input.org_id.clone(),
                        public_org_id: input.public_org_id.clone(),
                        ticket_id: input.ticket_id.clone(),
                        generation: input.generation,
                        user_id: input.user_id.clone(),
                        customer_email: input.customer_email.clone(),
                        customer_name: input.customer_name.clone(),
                        route,
                        intent,
                        intent_p,
                        subject: input.subject.clone(),
                        message: input
                            .new_messages
                            .iter()
                            .map(|m| m.body.trim())
                            .collect::<Vec<_>>()
                            .join("\n\n"),
                        conversation: input.conversation.clone(),
                        order_explicit: matched.explicit.is_some(),
                        order,
                        customer_orders: orders
                            .into_iter()
                            .filter(|o| o.status != OrderStatus::Cancelled)
                            .collect(),
                        foreign_order_numbers: matched.foreign,
                        resumed,
                    };
                    let _ = ctx
                        .request::<_, ()>(contract::route_request_target(service), Json(request))
                        .send();
                    return Ok(());
                }
                None => report(RouteOutcome::Escalate {
                    reason: format!("{} is not available yet", route.label()),
                    causes: Vec::new(),
                }),
            },
            Decision::Escalate { causes } => report(RouteOutcome::Escalate {
                reason: if causes.is_empty() {
                    "The classifier could not be reached".to_owned()
                } else {
                    escalation_reason(&causes, triage.as_ref())
                },
                causes,
            }),
            Decision::Close => report(RouteOutcome::Close),
        };
        let _ = ctx
            .request::<_, ()>(
                contract::route_result_target(&input.org_id, &input.ticket_id),
                Json(result),
            )
            .send();
        Ok(())
    }

    /// One classifier pass (`journal` names its journal entries), journaled and recorded as a
    /// `router` trace. `None` when the classifier failed for good.
    async fn classify(
        &self,
        ctx: &mut Context<'_>,
        org: &OrgId,
        ticket: &TicketId,
        text: &str,
        questions: Vec<Question>,
        journal: &str,
    ) -> Result<Option<(AgentRunId, OffsetDateTime, techdepo_ports::Classification)>, TerminalError>
    {
        let models = ctx
            .run(crate::models::read(&self.deps))
            .name(crate::models::STEP)
            .await?
            .into_inner();
        let classifier = crate::models::classifier(&self.deps, &models, ModelSlot::Triage);
        let asked = models.triage.model;
        let run_id = AgentRunId::new(mint("run", ctx.rand_uuid()));
        let started = now(ctx).await?;
        let (q, t) = (questions.clone(), text.to_owned());
        let result: Result<Json<techdepo_ports::Classification>, TerminalError> = ctx
            .run(|| async move {
                match classifier.classify(&t, &q).await {
                    Ok(c) => Ok(Json(c)),
                    Err(e) if e.is_retryable() => Err(HandlerError::from(e)),
                    Err(e) => Err(TerminalError::new(e.to_string()).into()),
                }
            })
            .name(journal)
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
        let usage = classification.as_ref().and_then(|c| c.usage);
        let run = NewAgentRun {
            id: run_id.clone(),
            ticket_id: Some(ticket.clone()),
            investigation_id: None,
            agent: AgentKind::Router,
            model: classification
                .as_ref()
                .map_or_else(|| asked.clone(), |c| c.model.clone()),
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
        let step = NewAgentStep {
            idx: 0,
            kind: AgentStepKind::Classification,
            name: run.model.clone(),
            journal_name: Some(journal.to_owned()),
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
        let org = org.clone();
        // The classifier's tokens count towards the org's daily budget (recorded, not gated).
        let usage = usage.filter(|u| u.total() > 0).map(|u| {
            techdepo_llm::UsageLedger::record_statement(&org, finished.date(), &run.model, u)
        });
        ctx.run(|| async move {
            let scope = OrgScope::verified(org);
            let runs = db.agent_runs(&scope);
            let mut statements = vec![
                runs.insert_run_statement(&run).map_err(sql)?,
                runs.insert_step_statement(&run.id, &step).map_err(sql)?,
            ];
            statements.extend(usage);
            db.batch(&statements).await.map_err(sql)?;
            Ok(())
        })
        .name(match journal {
            "classify" => "record-classification".to_owned(),
            other => format!("record-{other}"),
        })
        .await?;
        Ok(classification.map(|c| (run_id, finished, c)))
    }
}

/// The classification as stored on the ticket and shown in admin.
fn api_classification(
    c: &techdepo_ports::Classification,
    t: &Triage,
    decision: &Decision,
    settings: &OrgSettings,
    matched: &OrderMatch,
    run_id: AgentRunId,
    at: OffsetDateTime,
) -> Classification {
    Classification {
        model: c.model.clone(),
        intent: t.top().0,
        probabilities: t.probabilities.clone(),
        contains_request: t.contains_request,
        wants_human: t.wants_human,
        legal_threat: t.legal_threat,
        answers_our_question: t.answers_our_question,
        order_numbers: matched.mentioned.clone(),
        threshold: settings.escalation_threshold,
        margin: settings.escalation_margin,
        escalation: match decision {
            Decision::Escalate { causes } => causes.clone(),
            _ => Vec::new(),
        },
        agent_run_id: Some(run_id),
        classified_at: at,
    }
}

#[cfg(test)]
mod tests {
    use techdepo_domain::OrderId;
    use techdepo_ports::{Answer, AnswerValue, OptionProbability};

    use super::*;

    fn classification(
        intents: &[(&str, f64)],
        yes: &[(&str, f64)],
    ) -> techdepo_ports::Classification {
        let mut answers = vec![Answer {
            question_id: "intent".into(),
            value: AnswerValue::Choice {
                choice: intents[0].0.into(),
                probabilities: intents
                    .iter()
                    .map(|(o, p)| OptionProbability {
                        option: (*o).into(),
                        p: *p,
                    })
                    .collect(),
            },
        }];
        for (id, p) in yes {
            answers.push(Answer {
                question_id: (*id).into(),
                value: AnswerValue::Noul {
                    yes: *p,
                    no: 1.0 - p,
                },
            });
        }
        techdepo_ports::Classification {
            model: "test".into(),
            answers,
            usage: None,
        }
    }

    fn triage(intents: &[(&str, f64)], yes: &[(&str, f64)]) -> Triage {
        Triage::read(&classification(intents, yes))
    }

    const QUIET: &[(&str, f64)] = &[
        ("contains_request", 0.95),
        ("wants_human", 0.05),
        ("legal_threat", 0.02),
    ];

    #[test]
    fn confident_intents_dispatch_to_their_route() {
        let s = OrgSettings::default();
        let t = triage(&[("cancel_order", 0.9), ("refund_or_return", 0.05)], QUIET);
        assert_eq!(
            decide(&s, &t, None),
            Decision::Dispatch {
                route: TicketRoute::CancelOrder,
                intent: Intent::CancelOrder,
                intent_p: 0.9,
                resumed: None,
            }
        );
    }

    #[test]
    fn uncertainty_and_people_escalate() {
        let s = OrgSettings::default();
        let low = triage(&[("order_status", 0.6), ("other", 0.3)], QUIET);
        assert_eq!(
            decide(&s, &low, None),
            Decision::Escalate {
                causes: vec![EscalationCause::LowConfidence]
            }
        );
        let close = triage(&[("cancel_order", 0.75), ("refund_or_return", 0.65)], QUIET);
        assert_eq!(
            decide(&s, &close, None),
            Decision::Escalate {
                causes: vec![EscalationCause::SmallMargin]
            }
        );
        let other = triage(&[("other", 0.95)], QUIET);
        assert_eq!(
            decide(&s, &other, None),
            Decision::Escalate {
                causes: vec![EscalationCause::OtherIntent]
            }
        );
        let angry = triage(
            &[("order_status", 0.95)],
            &[
                ("contains_request", 0.9),
                ("wants_human", 0.92),
                ("legal_threat", 0.93),
            ],
        );
        assert_eq!(
            decide(&s, &angry, None),
            Decision::Escalate {
                causes: vec![EscalationCause::WantsHuman, EscalationCause::LegalThreat]
            }
        );
        assert!(escalation_reason(&[EscalationCause::LowConfidence], Some(&low)).contains("0.60"));
    }

    #[test]
    fn thanks_closes_and_answers_resume_the_route() {
        let s = OrgSettings::default();
        let thanks = triage(
            &[("other", 0.5), ("order_status", 0.3)],
            &[("contains_request", 0.07)],
        );
        assert_eq!(decide(&s, &thanks, None), Decision::Close);

        let pending = PendingQuestion {
            route: TicketRoute::CancelOrder,
            intent: Intent::CancelOrder,
            intent_p: 0.88,
            expects: Awaiting::OrderNumber,
        };
        let answer = triage(
            &[("other", 0.4), ("cancel_order", 0.35)],
            &[("contains_request", 0.6), ("answers_our_question", 0.94)],
        );
        assert_eq!(
            decide(&s, &answer, Some(&pending)),
            Decision::Dispatch {
                route: TicketRoute::CancelOrder,
                intent: Intent::CancelOrder,
                intent_p: 0.88,
                resumed: Some(Awaiting::OrderNumber),
            }
        );
        // Not an answer: re-triaged like any message.
        let unrelated = triage(
            &[("product_advice", 0.9), ("other", 0.05)],
            &[("contains_request", 0.95), ("answers_our_question", 0.2)],
        );
        assert!(matches!(
            decide(&s, &unrelated, Some(&pending)),
            Decision::Dispatch {
                route: TicketRoute::ProductAdvisor,
                resumed: None,
                ..
            }
        ));
        assert_eq!(triage_questions(Some(&pending)).len(), 5);
        assert_eq!(triage_questions(None).len(), 4);
    }

    #[test]
    fn missing_answers_escalate_instead_of_guessing() {
        let t = Triage::read(&techdepo_ports::Classification {
            model: "test".into(),
            answers: Vec::new(),
            usage: None,
        });
        assert!(matches!(
            decide(&OrgSettings::default(), &t, None),
            Decision::Escalate { .. }
        ));
    }

    #[test]
    fn only_the_customers_own_orders_are_matched() {
        let mine = vec![
            OrderBrief {
                order_id: OrderId::new("ord_2"),
                number: OrderNumber::new(1042),
                status: OrderStatus::Paid,
            },
            OrderBrief {
                order_id: OrderId::new("ord_3"),
                number: OrderNumber::new(1043),
                status: OrderStatus::Shipped,
            },
        ];
        let m = match_orders("Refund TD-1 and td-1043, also TD-1042 please", &mine);
        assert_eq!(m.explicit.unwrap().number, OrderNumber::new(1043));
        assert_eq!(m.foreign, vec![OrderNumber::new(1)]);
        assert_eq!(m.mentioned.len(), 3);
        assert_eq!(match_orders("no numbers", &mine), OrderMatch::default());
    }
}
