//! The classifications that answer a question we asked (part of `xtask eval classify`):
//!
//! - **Follow-ups** (`follow_ups`): a ticket waits on a route's question (`order_number`,
//!   `clarification`; demo scenario 7, ask-and-resume). The reply is classified as `Router/route`
//!   classifies it (`triage_questions(Some(pending))`, body only) and decided with
//!   `router::decide`: resume the route that asked, route it as a new request, escalate or close.
//! - **Offer replies** (`offer_replies`): the customer answers the lost-parcel offer (demo
//!   scenarios 4 and 5). The reply is classified as `Investigation` interprets it
//!   (`investigation::reply_questions`: the verdict plus the urgent screen) and decided with
//!   `investigation::reply_step` in round 1: refund, reship, declined (a person), ask again, or
//!   escalate (a request for a person, a legal threat).
//!
//! An offer reply that ends in a refund or a replacement the customer did not choose moves
//! goods or money: those count as *wrong actions*, held to their own maximum.

use std::fmt::Write as _;

use serde::Deserialize;
use techdepo_api_types::vocab::{Awaiting, TicketRoute};
use techdepo_domain::{Intent, OrgSettings};
use techdepo_ports::{Classification, Classifier};
use techdepo_services::investigation::{OfferStep, Verdict, reply_questions, reply_step};
use techdepo_services::notifications::templates::OfferKind;
use techdepo_services::support::contract::PendingQuestion;
use techdepo_services::support::router::{
    Decision, Triage, decide, triage_questions, urgent_causes,
};

use super::OneOrMany;
use super::stats::{Md, frac, ratio};

/// The intent probability the pending question was asked with (only echoed back on resume).
const ASKED_P: f64 = 0.9;

/// The question a follow-up answers (or not).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asked {
    /// The intent the route that asked handles.
    intent: Intent,
    /// What it asked for.
    expects: Awaiting,
}

/// A customer message while a route's question is pending.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FollowUp {
    id: String,
    #[serde(default)]
    tags: Vec<String>,
    asked: Asked,
    body: String,
    outcome: OneOrMany<FollowUpExpected>,
    /// For `route`: the new request's intent(s).
    #[serde(default)]
    intent: Option<OneOrMany<Intent>>,
}

/// What the router should do with a follow-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowUpExpected {
    /// It answers the question: back to the route that asked.
    Resume,
    /// Not an answer, a new request: route it as such.
    Route,
    /// A person.
    Escalate,
    /// Nothing to answer.
    Close,
}

impl FollowUpExpected {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Resume => "resume",
            Self::Route => "route",
            Self::Escalate => "escalate",
            Self::Close => "close",
        }
    }
}

/// A reply to the lost-parcel offer.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfferReply {
    id: String,
    #[serde(default)]
    tags: Vec<String>,
    offer: OfferKind,
    body: String,
    step: OneOrMany<StepExpected>,
}

/// What follows an offer reply (round 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepExpected {
    /// Pre-approved refund.
    Refund,
    /// Replacement order.
    Reship,
    /// Every offer declined: a person.
    Declined,
    /// Ask again ("which one?").
    Clarify,
    /// A person at once (wants a person, legal threat).
    Escalate,
}

impl StepExpected {
    fn of(step: OfferStep) -> Self {
        match step {
            OfferStep::Refund => Self::Refund,
            OfferStep::Reship => Self::Reship,
            OfferStep::Declined => Self::Declined,
            OfferStep::Clarify => Self::Clarify,
            OfferStep::Escalate => Self::Escalate,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Refund => "refund",
            Self::Reship => "reship",
            Self::Declined => "declined",
            Self::Clarify => "clarify",
            Self::Escalate => "escalate",
        }
    }

    const fn acts(self) -> bool {
        matches!(self, Self::Refund | Self::Reship)
    }
}

/// The section's results, for the summary and the minimums.
pub struct Results {
    /// Follow-ups decided right.
    pub follow_ups_correct: usize,
    /// Follow-ups.
    pub follow_ups: usize,
    /// Offer replies decided right.
    pub offers_correct: usize,
    /// Offer replies.
    pub offers: usize,
    /// Offer replies that ended in a refund or replacement nobody chose.
    pub wrong_actions: usize,
    /// Answers given by another model than the primary classifier (the fallback).
    pub fallback_answers: usize,
}

impl FollowUp {
    /// The case ID.
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl OfferReply {
    /// The case ID.
    pub fn id(&self) -> &str {
        &self.id
    }
}

fn pending(asked: &Asked) -> PendingQuestion {
    PendingQuestion {
        route: TicketRoute::for_intent(asked.intent),
        intent: asked.intent,
        intent_p: ASKED_P,
        expects: asked.expects,
    }
}

fn follow_up_outcome(decision: &Decision) -> FollowUpExpected {
    match decision {
        Decision::Dispatch {
            resumed: Some(_), ..
        } => FollowUpExpected::Resume,
        Decision::Dispatch { .. } => FollowUpExpected::Route,
        Decision::Escalate { .. } => FollowUpExpected::Escalate,
        Decision::Close => FollowUpExpected::Close,
    }
}

/// The step `Investigation` takes on a round-1 reply classified as `c`.
fn offer_reply_step(offer: OfferKind, c: &Classification, threshold: f64) -> (OfferStep, String) {
    let urgent = !urgent_causes(&Triage::read(c)).is_empty();
    let (verdict, p) = c
        .answer("verdict")
        .and_then(|a| {
            let choice = c.choice("verdict")?;
            Some((
                Verdict::parse(choice),
                a.probability_of(choice).unwrap_or(0.0),
            ))
        })
        .unwrap_or((Verdict::Unclear, 0.0));
    (
        reply_step(offer, verdict, p, threshold, 1, urgent),
        format!(
            "{} {p:.2}{}",
            verdict.as_str(),
            if urgent { ", urgent" } else { "" }
        ),
    )
}

/// Classifies and reports both sections; `primary`: the primary classifier's model name.
pub async fn run(
    md: &mut String,
    classifier: &dyn Classifier,
    primary: &str,
    follow_ups: &[&FollowUp],
    offers: &[&OfferReply],
    settings: &OrgSettings,
    verbose: bool,
) -> Results {
    let mut results = Results {
        follow_ups_correct: 0,
        follow_ups: follow_ups.len(),
        offers_correct: 0,
        offers: offers.len(),
        wrong_actions: 0,
        fallback_answers: 0,
    };

    let _ = writeln!(
        md,
        "## Follow-ups (a question is pending)\n\nThe customer writes while a route waits for its \
         question's answer: classified with `answers_our_question` and decided with \
         `router::decide` (resume the route, route a new request, escalate, close).\n"
    );
    let mut table = Md::new(
        &[
            "",
            "Case",
            "Tags",
            "Asked",
            "Expected",
            "p(answers)",
            "Decision",
        ],
        &[5],
    );
    for case in follow_ups {
        let pending = pending(&case.asked);
        let classified = classifier
            .classify(case.body.trim(), &triage_questions(Some(&pending)))
            .await;
        let expected = case.outcome.to_vec();
        let (ok, p, decision) = match &classified {
            Ok(c) => {
                results.fallback_answers += usize::from(c.model != primary);
                let triage = Triage::read(c);
                let d = decide(settings, &triage, Some(&pending));
                let outcome = follow_up_outcome(&d);
                let intent_ok = match (&d, &case.intent) {
                    (
                        Decision::Dispatch {
                            intent,
                            resumed: None,
                            ..
                        },
                        Some(want),
                    ) => want.to_vec().contains(intent),
                    _ => true,
                };
                let shown = match &d {
                    Decision::Dispatch { intent, .. } => {
                        format!("{} ({})", outcome.as_str(), intent.as_str())
                    }
                    _ => outcome.as_str().to_owned(),
                };
                (
                    expected.contains(&outcome) && intent_ok,
                    triage
                        .answers_our_question
                        .map_or_else(String::new, |p| format!("{p:.2}")),
                    shown,
                )
            }
            Err(e) => (false, String::new(), format!("error: {e}")),
        };
        results.follow_ups_correct += usize::from(ok);
        if verbose || !ok {
            println!(
                "  {} {:<28} {}",
                if ok { "ok  " } else { "MISS" },
                case.id,
                decision
            );
        }
        table.row(&[
            if ok { "✓" } else { "✗" }.to_owned(),
            format!("`{}`", case.id),
            case.tags.join(", "),
            format!(
                "{} ({})",
                case.asked.expects.as_str(),
                case.asked.intent.as_str()
            ),
            expected
                .iter()
                .map(|o| o.as_str())
                .collect::<Vec<_>>()
                .join(" / "),
            p,
            decision,
        ]);
    }
    let _ = writeln!(
        md,
        "Decided right: {}.\n",
        frac(results.follow_ups_correct, results.follow_ups)
    );
    md.push_str(&table.finish());

    let _ = writeln!(
        md,
        "## Offer replies (lost parcel, round 1)\n\nThe customer answers the offer: one pass \
         with the verdict and the urgent screen, then `investigation::reply_step` at the \
         default threshold {:.2}. A *wrong action* is a refund or replacement the customer did \
         not choose.\n",
        settings.escalation_threshold
    );
    let mut table = Md::new(
        &["", "Case", "Tags", "Offer", "Expected", "Verdict", "Step"],
        &[],
    );
    for case in offers {
        let classified = classifier
            .classify(case.body.trim(), &reply_questions(case.offer))
            .await;
        let expected = case.step.to_vec();
        let (ok, wrong, verdict, step) = match &classified {
            Ok(c) => {
                results.fallback_answers += usize::from(c.model != primary);
                let (step, verdict) =
                    offer_reply_step(case.offer, c, settings.escalation_threshold);
                let step = StepExpected::of(step);
                let ok = expected.contains(&step);
                (ok, !ok && step.acts(), verdict, step.as_str().to_owned())
            }
            Err(e) => (false, false, String::new(), format!("error: {e}")),
        };
        results.offers_correct += usize::from(ok);
        results.wrong_actions += usize::from(wrong);
        if verbose || !ok {
            println!(
                "  {} {:<28} {}",
                if ok { "ok  " } else { "MISS" },
                case.id,
                step
            );
        }
        table.row(&[
            if ok {
                "✓"
            } else if wrong {
                "✗ wrong action"
            } else {
                "✗"
            }
            .to_owned(),
            format!("`{}`", case.id),
            case.tags.join(", "),
            offer_name(case.offer).to_owned(),
            expected
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(" / "),
            verdict,
            step,
        ]);
    }
    let _ = writeln!(
        md,
        "Decided right: {}; wrong actions: {}.\n",
        frac(results.offers_correct, results.offers),
        frac(results.wrong_actions, results.offers)
    );
    md.push_str(&table.finish());
    results
}

const fn offer_name(offer: OfferKind) -> &'static str {
    match offer {
        OfferKind::Refund => "refund",
        OfferKind::Reship => "reship",
        OfferKind::Both => "both",
    }
}

impl Results {
    /// Share of follow-ups decided right.
    pub fn follow_up_accuracy(&self) -> Option<f64> {
        ratio(self.follow_ups_correct, self.follow_ups)
    }

    /// Share of offer replies decided right.
    pub fn offer_accuracy(&self) -> Option<f64> {
        ratio(self.offers_correct, self.offers)
    }

    /// Share of offer replies that ended in an action nobody chose.
    pub fn wrong_action_rate(&self) -> Option<f64> {
        ratio(self.wrong_actions, self.offers)
    }
}
