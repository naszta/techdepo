//! `xtask eval classify`: the triage eval set through the router's own questions and decision.
//!
//! Every case is classified as `Router/route` classifies a ticket's first message
//! (`RouteTicketInput::text`: "Subject: …" and the body; `router::triage_questions(None)`), read
//! with `Triage::read` and decided with `router::decide` on the default org settings. A
//! classifier that keeps failing escalates, as in the router (real backends retry retryable
//! errors, `super::retry`); a failure on the first case stops the run.
//!
//! The file's `follow_ups` and `offer_replies` cover the classifications that answer our own
//! questions (`super::replies`): a reply while a route's question is pending, and a reply to the
//! lost-parcel offer.
//!
//! The report: headline metrics against the file's minimums, results per tag, the confusion
//! matrix (expected × top intent), calibration (accuracy of the top intent per probability
//! bucket, and the expected calibration error) and a sweep of the escalation threshold with a
//! suggested value: the lowest threshold whose dispatched tickets stay within the allowed
//! wrong-route rate, which keeps the most tickets automatic.

use std::fmt::Write as _;

use serde::Deserialize;
use techdepo_api_types::support::EscalationCause;
use techdepo_domain::{Intent, OrgSettings};
use techdepo_services::support::router::{Decision, Triage, decide, triage_questions};

use super::stats::{Md, calibration, ece, frac, mean, pct, ratio};
use super::{OneOrMany, Report, Run, replies};

/// Calibration bucket edges.
const EDGES: &[f64] = &[0.0, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0];
/// Thresholds the sweep tries.
const SWEEP: &[f64] = &[
    0.40, 0.45, 0.50, 0.55, 0.60, 0.65, 0.70, 0.75, 0.80, 0.85, 0.90, 0.95,
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvalFile {
    minimums: Minimums,
    cases: Vec<Case>,
    /// Replies while a route's question is pending.
    #[serde(default)]
    follow_ups: Vec<replies::FollowUp>,
    /// Replies to the lost-parcel offer.
    #[serde(default)]
    offer_replies: Vec<replies::OfferReply>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Minimums {
    intent_accuracy: f64,
    decision_accuracy: f64,
    escalation_recall: f64,
    max_wrong_route_rate: f64,
    follow_up_accuracy: f64,
    offer_reply_accuracy: f64,
    max_offer_wrong_action_rate: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    subject: String,
    body: String,
    intent: OneOrMany<Intent>,
    outcome: OneOrMany<Expected>,
}

impl Case {
    /// The text the router classifies for a ticket's first message.
    fn text(&self) -> String {
        let body = self.body.trim();
        match self.subject.trim() {
            "" => body.to_owned(),
            subject => format!("Subject: {subject}\n\n{body}"),
        }
    }
}

/// What the router should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Expected {
    Route,
    Escalate,
    Close,
}

impl Expected {
    fn of(decision: &Decision) -> Self {
        match decision {
            Decision::Dispatch { .. } => Self::Route,
            Decision::Escalate { .. } => Self::Escalate,
            Decision::Close => Self::Close,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Escalate => "escalate",
            Self::Close => "close",
        }
    }
}

/// One classified case.
struct Row<'a> {
    case: &'a Case,
    intents: Vec<Intent>,
    outcomes: Vec<Expected>,
    /// The triage, or why the classifier failed.
    triage: Result<Triage, String>,
    /// The fallback model answered instead of the primary classifier.
    fallback: bool,
}

impl Row<'_> {
    fn top(&self) -> Option<(Intent, f64)> {
        self.triage.as_ref().ok().map(Triage::top)
    }

    fn intent_correct(&self) -> bool {
        self.top().is_some_and(|(i, _)| self.intents.contains(&i))
    }

    /// The router's decision under `settings` (a failed classifier escalates).
    fn decide(&self, settings: &OrgSettings) -> Decision {
        match &self.triage {
            Ok(t) => decide(settings, t, None),
            Err(_) => Decision::Escalate { causes: Vec::new() },
        }
    }

    fn must_escalate(&self) -> bool {
        self.outcomes.iter().all(|o| *o == Expected::Escalate)
    }

    fn must_route(&self) -> bool {
        self.outcomes.iter().all(|o| *o == Expected::Route)
    }

    /// Dispatched to a route for an intent the customer did not express: the route acts on
    /// something nobody asked for.
    fn wrong_route(&self, decision: &Decision) -> bool {
        matches!(decision, Decision::Dispatch { intent, .. } if !self.intents.contains(intent))
    }

    /// Dispatched to one of the customer's intents although a person should have taken it
    /// (several requests, a vague or angry message): part of it may go unanswered.
    fn missed_escalation(&self, decision: &Decision) -> bool {
        matches!(decision, Decision::Dispatch { intent, .. }
            if self.intents.contains(intent) && !self.outcomes.contains(&Expected::Route))
    }

    fn decision_correct(&self, decision: &Decision) -> bool {
        self.outcomes.contains(&Expected::of(decision)) && !self.wrong_route(decision)
    }
}

/// Totals for one set of decisions.
#[derive(Debug, Default, Clone, Copy)]
struct Totals {
    cases: usize,
    intent_correct: usize,
    decision_correct: usize,
    must_escalate: usize,
    escalated_of_must: usize,
    must_route: usize,
    routed_right_of_must: usize,
    dispatched: usize,
    wrong_route: usize,
    missed_escalation: usize,
    errors: usize,
}

fn totals<'r>(rows: impl IntoIterator<Item = &'r Row<'r>>, settings: &OrgSettings) -> Totals {
    let mut t = Totals::default();
    for row in rows {
        let d = row.decide(settings);
        t.cases += 1;
        t.intent_correct += usize::from(row.intent_correct());
        t.decision_correct += usize::from(row.decision_correct(&d));
        t.errors += usize::from(row.triage.is_err());
        if row.must_escalate() {
            t.must_escalate += 1;
            t.escalated_of_must += usize::from(matches!(d, Decision::Escalate { .. }));
        }
        if row.must_route() {
            t.must_route += 1;
            t.routed_right_of_must += usize::from(row.decision_correct(&d));
        }
        if matches!(d, Decision::Dispatch { .. }) {
            t.dispatched += 1;
            t.wrong_route += usize::from(row.wrong_route(&d));
            t.missed_escalation += usize::from(row.missed_escalation(&d));
        }
    }
    t
}

/// Runs the classification eval.
pub async fn run(run: &Run<'_>, source: &str) -> Result<Report, String> {
    let file: EvalFile =
        serde_yaml::from_str(source).map_err(|e| format!("{}: {e}", run.source_name))?;
    let classifier = run.models.ports.classifier.clone();
    let questions = triage_questions(None);
    let settings = OrgSettings::default();

    let cases: Vec<&Case> = file.cases.iter().filter(|c| run.selected(&c.id)).collect();
    println!(
        "Classifying {} cases with {} …",
        cases.len(),
        run.models.classifier_name
    );
    // Which model answered each case: the fallback's (self-reported) confidence is not Clef's
    // probability, so its answers are counted and kept out of the calibration.
    let primary = run
        .models
        .classifier_name
        .split(' ')
        .next()
        .unwrap_or_default()
        .to_owned();
    let mut rows = Vec::new();
    for case in cases {
        let classified = classifier.classify(&case.text(), &questions).await;
        let fallback = classified.as_ref().is_ok_and(|c| c.model != primary);
        let triage = classified
            .map(|c| Triage::read(&c))
            .map_err(|e| e.to_string());
        // A misconfigured backend (credentials, model name) fails every case: stop at the first.
        if let (Err(e), true) = (&triage, rows.is_empty()) {
            return Err(format!("the classifier fails on the first case: {e}"));
        }
        let row = Row {
            case,
            intents: case.intent.to_vec(),
            outcomes: case.outcome.to_vec(),
            triage,
            fallback,
        };
        let d = row.decide(&settings);
        let ok = row.intent_correct() && row.decision_correct(&d);
        if run.options.verbose || !ok {
            println!(
                "  {} {:<28} {:<36} {}",
                if ok { "ok  " } else { "MISS" },
                case.id,
                top_cell(&row),
                decision_cell(&d)
            );
        }
        rows.push(row);
    }

    let t = totals(&rows, &settings);
    let intent_accuracy = ratio(t.intent_correct, t.cases);
    let decision_accuracy = ratio(t.decision_correct, t.cases);
    let escalation_recall = ratio(t.escalated_of_must, t.must_escalate);
    let wrong_route_rate = ratio(t.wrong_route, t.dispatched).or(Some(0.0));

    let mut failures = Vec::new();
    let m = &file.minimums;
    let mut check = |name: &str, value: Option<f64>, min: f64, at_most: bool| {
        // Nothing to measure (no such cases) is not a failure.
        let v = value.unwrap_or(if at_most { 0.0 } else { 1.0 });
        let ok = if at_most { v <= min } else { v >= min };
        if !ok {
            failures.push(format!(
                "{name} {} ({} {})",
                pct(Some(v)),
                if at_most { "max" } else { "min" },
                pct(Some(min))
            ));
        }
        (if ok { "ok" } else { "**below**" }).to_owned()
    };

    let mut md = run.header("Classification eval");
    let _ = writeln!(
        md,
        "{} cases; router settings: threshold {:.2}, margin {:.2}. Each case is classified as the \
         router classifies a ticket's first message and decided with `router::decide`.\n",
        t.cases, settings.escalation_threshold, settings.escalation_margin
    );
    let _ = writeln!(md, "## Summary\n");
    let mut table = Md::new(&["Metric", "Value", "Minimum", ""], &[1, 2]);
    let status = check("intent accuracy", intent_accuracy, m.intent_accuracy, false);
    table.row(&[
        "Intent accuracy (top intent)",
        &frac(t.intent_correct, t.cases),
        &pct(Some(m.intent_accuracy)),
        &status,
    ]);
    let status = check(
        "decision accuracy",
        decision_accuracy,
        m.decision_accuracy,
        false,
    );
    table.row(&[
        "Decision accuracy (route / escalate / close)",
        &frac(t.decision_correct, t.cases),
        &pct(Some(m.decision_accuracy)),
        &status,
    ]);
    let status = check(
        "escalation recall",
        escalation_recall,
        m.escalation_recall,
        false,
    );
    table.row(&[
        "Escalation recall (must reach a person)",
        &frac(t.escalated_of_must, t.must_escalate),
        &pct(Some(m.escalation_recall)),
        &status,
    ]);
    let status = check(
        "wrong-route rate",
        wrong_route_rate,
        m.max_wrong_route_rate,
        true,
    );
    table.row(&[
        "Wrong-route rate (dispatched to an intent not expressed)",
        &frac(t.wrong_route, t.dispatched),
        &format!("≤ {}", pct(Some(m.max_wrong_route_rate))),
        &status,
    ]);
    table.row(&[
        "Dispatched although a person was needed (to one of its intents)",
        &frac(t.missed_escalation, t.dispatched),
        "",
        "",
    ]);
    table.row(&[
        "Automation (clear cases routed right)",
        &frac(t.routed_right_of_must, t.must_route),
        "",
        "",
    ]);
    table.row(&["Classifier errors", &t.errors.to_string(), "", ""]);
    let fallback = rows.iter().filter(|r| r.fallback).count();
    table.row(&[
        "Answered by the fallback model (not in the calibration)",
        &fallback.to_string(),
        "",
        "",
    ]);
    md.push_str(&table.finish());
    if fallback > 0 {
        let _ = writeln!(
            md,
            "**{fallback} answers came from the fallback model** (Clef could not answer them):              its confidence is self-reported, so the calibration leaves them out and the              threshold sweep is not a clean Clef measurement. Run again before tuning.\n"
        );
    }

    by_tag(&mut md, &rows, &settings);
    confusion(&mut md, &rows);
    // The threshold only matters for a routable top intent (`other` always escalates).
    let points: Vec<(f64, bool)> = rows
        .iter()
        .filter(|r| !r.fallback)
        .filter_map(|r| {
            r.top()
                .filter(|(intent, _)| *intent != Intent::Other)
                .map(|(_, p)| (p, r.intent_correct()))
        })
        .collect();
    calibration_section(&mut md, &points);
    let suggested = sweep(&mut md, &rows, &settings, m);

    // The classifications that answer our own questions.
    let follow_ups: Vec<&replies::FollowUp> = file
        .follow_ups
        .iter()
        .filter(|c| run.selected(c.id()))
        .collect();
    let offers: Vec<&replies::OfferReply> = file
        .offer_replies
        .iter()
        .filter(|c| run.selected(c.id()))
        .collect();
    println!(
        "Classifying {} follow-ups and {} offer replies …",
        follow_ups.len(),
        offers.len()
    );
    let answered = replies::run(
        &mut md,
        classifier.as_ref(),
        &primary,
        &follow_ups,
        &offers,
        &settings,
        run.options.verbose,
    )
    .await;
    let follow_up_status = check(
        "follow-up accuracy",
        answered.follow_up_accuracy(),
        m.follow_up_accuracy,
        false,
    );
    let offer_status = check(
        "offer-reply accuracy",
        answered.offer_accuracy(),
        m.offer_reply_accuracy,
        false,
    );
    let wrong_status = check(
        "offer wrong-action rate",
        answered.wrong_action_rate().or(Some(0.0)),
        m.max_offer_wrong_action_rate,
        true,
    );
    let _ = writeln!(md, "## Follow-ups and offer replies: summary\n");
    let mut table = Md::new(&["Metric", "Value", "Minimum", ""], &[1, 2]);
    table.row(&[
        "Follow-ups decided right (resume / route / escalate / close)",
        &frac(answered.follow_ups_correct, answered.follow_ups),
        &pct(Some(m.follow_up_accuracy)),
        &follow_up_status,
    ]);
    table.row(&[
        "Offer replies decided right",
        &frac(answered.offers_correct, answered.offers),
        &pct(Some(m.offer_reply_accuracy)),
        &offer_status,
    ]);
    table.row(&[
        "Offer replies acted on wrongly (a refund or replacement not chosen)",
        &frac(answered.wrong_actions, answered.offers),
        &format!("≤ {}", pct(Some(m.max_offer_wrong_action_rate))),
        &wrong_status,
    ]);
    table.row(&[
        "Answered by the fallback model",
        &answered.fallback_answers.to_string(),
        "",
        "",
    ]);
    md.push_str(&table.finish());
    cases_section(&mut md, &rows, &settings);

    println!();
    println!("  intent accuracy    {}", frac(t.intent_correct, t.cases));
    println!("  decision accuracy  {}", frac(t.decision_correct, t.cases));
    println!(
        "  escalation recall  {}",
        frac(t.escalated_of_must, t.must_escalate)
    );
    println!("  wrong-route rate   {}", frac(t.wrong_route, t.dispatched));
    println!(
        "  calibration error  {}",
        pct(ece(&calibration(&points, EDGES)))
    );
    println!(
        "  follow-ups         {}",
        frac(answered.follow_ups_correct, answered.follow_ups)
    );
    println!(
        "  offer replies      {} (wrong actions {})",
        frac(answered.offers_correct, answered.offers),
        answered.wrong_actions
    );
    if fallback > 0 {
        println!("  fallback answers   {fallback} (left out of the calibration)");
    }
    match suggested {
        Some(s) => println!(
            "  threshold          suggest {s:.2} (current default {:.2})",
            settings.escalation_threshold
        ),
        None => println!("  threshold          no threshold meets the wrong-route limit"),
    }

    Ok(Report {
        markdown: md,
        failures,
    })
}

fn top_cell(row: &Row<'_>) -> String {
    match &row.triage {
        Ok(t) => {
            let (intent, p) = t.top();
            format!("{} {p:.2} (2nd {:.2})", intent.as_str(), t.runner_up())
        }
        Err(e) => format!("error: {e}"),
    }
}

fn decision_cell(d: &Decision) -> String {
    match d {
        Decision::Dispatch { intent, .. } => format!("route {}", intent.as_str()),
        Decision::Escalate { causes } if causes.is_empty() => "escalate".to_owned(),
        Decision::Escalate { causes } => format!("escalate ({})", causes_text(causes)),
        Decision::Close => "close".to_owned(),
    }
}

fn causes_text(causes: &[EscalationCause]) -> String {
    causes
        .iter()
        .map(|c| c.label().to_lowercase())
        .collect::<Vec<_>>()
        .join(", ")
}

fn by_tag(md: &mut String, rows: &[Row<'_>], settings: &OrgSettings) {
    let mut tags: Vec<&str> = rows
        .iter()
        .flat_map(|r| r.case.tags.iter().map(String::as_str))
        .collect();
    tags.sort_unstable();
    tags.dedup();
    let _ = writeln!(md, "## By tag\n");
    let mut table = Md::new(
        &[
            "Tag",
            "Cases",
            "Intent",
            "Decision",
            "Wrong route",
            "Dispatched, needed a person",
        ],
        &[1, 2, 3, 4, 5],
    );
    for tag in tags {
        let subset: Vec<&Row<'_>> = rows
            .iter()
            .filter(|r| r.case.tags.iter().any(|t| t == tag))
            .collect();
        let t = totals(subset.iter().copied(), settings);
        table.row(&[
            tag.to_owned(),
            t.cases.to_string(),
            frac(t.intent_correct, t.cases),
            frac(t.decision_correct, t.cases),
            t.wrong_route.to_string(),
            t.missed_escalation.to_string(),
        ]);
    }
    md.push_str(&table.finish());
}

fn confusion(md: &mut String, rows: &[Row<'_>]) {
    let _ = writeln!(
        md,
        "## Confusion matrix\n\nRows: expected intent (the predicted one when it is among \
         several expected); columns: top intent.\n"
    );
    let short = |i: Intent| match i {
        Intent::CancelOrder => "cancel",
        Intent::OrderStatus => "status",
        Intent::BillingProblem => "billing",
        Intent::RefundOrReturn => "refund",
        Intent::ProductAdvice => "advice",
        Intent::OrderQuestion => "question",
        Intent::Other => "other",
    };
    let mut header = vec!["expected \\ top"];
    header.extend(Intent::ALL.iter().map(|i| short(*i)));
    header.push("error");
    let right: Vec<usize> = (1..header.len()).collect();
    let mut table = Md::new(&header, &right);
    for &expected in Intent::ALL {
        let mut counts = vec![0usize; Intent::ALL.len() + 1];
        let mut any = false;
        for row in rows {
            let top = row.top().map(|(i, _)| i);
            let actual = match top {
                Some(i) if row.intents.contains(&i) => i,
                _ => row.intents[0],
            };
            if actual != expected {
                continue;
            }
            any = true;
            let col = top
                .and_then(|i| Intent::ALL.iter().position(|x| *x == i))
                .unwrap_or(Intent::ALL.len());
            counts[col] += 1;
        }
        if !any {
            continue;
        }
        let mut cells = vec![short(expected).to_owned()];
        cells.extend(counts.iter().enumerate().map(|(col, n)| {
            let diagonal = Intent::ALL.get(col) == Some(&expected);
            match (*n, diagonal) {
                (0, _) => "·".to_owned(),
                (n, true) => format!("**{n}**"),
                (n, false) => n.to_string(),
            }
        }));
        table.row(&cells);
    }
    md.push_str(&table.finish());
}

fn calibration_section(md: &mut String, points: &[(f64, bool)]) {
    let buckets = calibration(points, EDGES);
    let _ = writeln!(
        md,
        "## Calibration\n\nHow often the top intent is right, by its probability, over the {} \
         cases whose top intent has a route (`other` always escalates, whatever its \
         probability). A calibrated classifier's accuracy matches its mean probability in every \
         bucket. Expected calibration error: {}.\n",
        points.len(),
        pct(ece(&buckets))
    );
    let mut table = Md::new(
        &["p(top intent)", "Cases", "Mean p", "Accuracy", "Gap"],
        &[1, 2, 3, 4],
    );
    for b in &buckets {
        let acc = ratio(b.correct, b.ps.len());
        let conf = mean(&b.ps);
        table.row(&[
            format!("{:.2}–{:.2}", b.lo, b.hi),
            b.ps.len().to_string(),
            conf.map_or_else(|| "·".to_owned(), |c| format!("{c:.2}")),
            acc.map_or_else(
                || "·".to_owned(),
                |a| frac(b.correct, b.ps.len()) + &format!(" ({a:.2})"),
            ),
            match (acc, conf) {
                (Some(a), Some(c)) => format!("{:+.2}", a - c),
                _ => "·".to_owned(),
            },
        ]);
    }
    md.push_str(&table.finish());
}

/// The threshold sweep; returns the suggested threshold.
fn sweep(md: &mut String, rows: &[Row<'_>], settings: &OrgSettings, m: &Minimums) -> Option<f64> {
    let _ = writeln!(
        md,
        "## Threshold sweep\n\nThe router's decision with other escalation thresholds (margin \
         fixed at {:.2}). *Automated* counts clear cases routed right; *wrong route* counts \
         tickets dispatched for an intent the customer did not express.\n",
        settings.escalation_margin
    );
    let mut table = Md::new(
        &[
            "Threshold",
            "Dispatched",
            "Automated",
            "Wrong route",
            "Escalation recall",
            "Decision accuracy",
        ],
        &[1, 2, 3, 4, 5],
    );
    // (threshold, correct decisions) of every threshold within both limits.
    let mut within: Vec<(f64, usize)> = Vec::new();
    for &threshold in SWEEP {
        let s = OrgSettings {
            escalation_threshold: threshold,
            ..settings.clone()
        };
        let t = totals(rows, &s);
        let wrong = ratio(t.wrong_route, t.dispatched).unwrap_or(0.0);
        let recall = ratio(t.escalated_of_must, t.must_escalate).unwrap_or(1.0);
        if wrong <= m.max_wrong_route_rate && recall >= m.escalation_recall {
            within.push((threshold, t.decision_correct));
        }
        let current = (threshold - settings.escalation_threshold).abs() < 1e-9;
        table.row(&[
            if current {
                format!("**{threshold:.2}** (default)")
            } else {
                format!("{threshold:.2}")
            },
            t.dispatched.to_string(),
            frac(t.routed_right_of_must, t.must_route),
            frac(t.wrong_route, t.dispatched),
            frac(t.escalated_of_must, t.must_escalate),
            frac(t.decision_correct, t.cases),
        ]);
    }
    md.push_str(&table.finish());
    // The most correct decisions (automation and escalation alike) within both limits; of the
    // thresholds that tie, the middle one, so the suggestion does not hug the confidence of the
    // few cases on either edge.
    let best = within.iter().map(|(_, n)| *n).max();
    let plateau: Vec<f64> = within
        .iter()
        .filter(|(_, n)| Some(*n) == best)
        .map(|(t, _)| *t)
        .collect();
    let suggestion = plateau.get(plateau.len().saturating_sub(1) / 2).copied();
    let _ = match suggestion {
        Some(t) => writeln!(
            md,
            "**Suggested threshold: {t:.2}** (current default {:.2}): the most correct decisions \
             while the wrong-route rate stays within {} and the escalation recall at or above {} \
             (the middle of the thresholds that tie). With a set this small, also check the \
             calibration table: prefer a value at or above the first bucket whose accuracy \
             matches its probability.\n",
            settings.escalation_threshold,
            pct(Some(m.max_wrong_route_rate)),
            pct(Some(m.escalation_recall))
        ),
        None => writeln!(
            md,
            "**No threshold keeps the wrong-route rate within {} and the escalation recall at \
             or above {}**: the classifier (or the questions) needs work before the threshold \
             can be tuned.\n",
            pct(Some(m.max_wrong_route_rate)),
            pct(Some(m.escalation_recall))
        ),
    };
    suggestion
}

fn cases_section(md: &mut String, rows: &[Row<'_>], settings: &OrgSettings) {
    let _ = writeln!(md, "## Cases\n");
    let mut table = Md::new(
        &[
            "",
            "Case",
            "Tags",
            "Expected",
            "Top intent",
            "p",
            "2nd",
            "Flags",
            "Decision",
        ],
        &[5, 6],
    );
    for row in rows {
        let d = row.decide(settings);
        let ok = row.intent_correct() && row.decision_correct(&d);
        let expected = format!(
            "{} → {}",
            row.intents
                .iter()
                .map(|i| i.as_str())
                .collect::<Vec<_>>()
                .join(" / "),
            row.outcomes
                .iter()
                .map(|o| o.as_str())
                .collect::<Vec<_>>()
                .join(" / ")
        );
        let (top, p, second, flags) = match &row.triage {
            Ok(t) => {
                let (i, p) = t.top();
                let mut flags = Vec::new();
                if t.wants_human >= 0.5 {
                    flags.push(format!("human {:.2}", t.wants_human));
                }
                if t.legal_threat >= 0.5 {
                    flags.push(format!("legal {:.2}", t.legal_threat));
                }
                if t.contains_request < 0.5 {
                    flags.push(format!("request {:.2}", t.contains_request));
                }
                (
                    i.as_str().to_owned(),
                    format!("{p:.2}"),
                    format!("{:.2}", t.runner_up()),
                    flags.join(", "),
                )
            }
            Err(e) => (
                format!("error: {e}"),
                String::new(),
                String::new(),
                String::new(),
            ),
        };
        table.row(&[
            if ok { "✓" } else { "✗" }.to_owned(),
            format!("`{}`", row.case.id),
            row.case.tags.join(", "),
            expected,
            top,
            p,
            second,
            flags,
            decision_cell(&d),
        ]);
    }
    md.push_str(&table.finish());
}
