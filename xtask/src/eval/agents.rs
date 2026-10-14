//! `xtask eval agents`: the agent scenarios through the real route pipeline.
//!
//! Each scenario gets a fresh seeded shop ([`Shop`]) and runs `agents::pipeline::answer` with
//! the `Inline` runtime, as the router would dispatch the message (intent probability 1.0, so
//! the reply confidence is the judge's score). The outcome and the stored trace
//! (`agent_runs` / `agent_steps`) are then checked against the scenario's expectations, plus the
//! checks every AI-authored text gets: the pipeline's guardrails passed, every product it names
//! resolves to one catalog product, and every SKU-like token is a catalog SKU.
//!
//! The report: pass rate against the file's minimum, per-scenario results with the tools called,
//! attempts, tokens and judge scores, and the judge scores of passing and failing texts with a
//! sweep of `send_above_confidence` thresholds (to calibrate its default).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::Deserialize;
use techdepo_agent::Inline;
use techdepo_api_types::vocab::{AgentKind, AgentRunStatus, AgentStepKind, TicketRoute};
use techdepo_catalog::{Catalog, Category, MentionTarget, Product};
use techdepo_db::OrgScope;
use techdepo_domain::{Intent, OrgId, OrgSettings, PublicOrgId, UserId};
use techdepo_services::agents::pipeline::answer;
use techdepo_services::support::contract::{AiReply, Reply, RouteOutcome, RouteRequest};
use time::OffsetDateTime;

use super::shop::{self, Shop};
use super::stats::{Md, frac, mean, pct, ratio};
use super::{OneOrMany, Report, Run};

/// `send_above_confidence` thresholds the sweep tries.
const SWEEP: &[f64] = &[0.50, 0.60, 0.70, 0.75, 0.80, 0.85, 0.90, 0.95];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvalFile {
    minimums: Minimums,
    scenarios: Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Minimums {
    pass_rate: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    id: String,
    #[serde(default)]
    note: Option<String>,
    agent: Agent,
    message: String,
    /// Must pass whatever the pass rate (demo scenarios, safety).
    #[serde(default)]
    critical: bool,
    expect: Expect,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Agent {
    ProductAdvisor,
    OrderAssistant,
}

impl Agent {
    const fn kind(self) -> AgentKind {
        match self {
            Self::ProductAdvisor => AgentKind::ProductAdvisor,
            Self::OrderAssistant => AgentKind::OrderAssistant,
        }
    }

    const fn route(self) -> (TicketRoute, Intent) {
        match self {
            Self::ProductAdvisor => (TicketRoute::ProductAdvisor, Intent::ProductAdvice),
            Self::OrderAssistant => (TicketRoute::OrderAssistant, Intent::OrderQuestion),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    outcome: OneOrMany<OutcomeKind>,
    #[serde(default)]
    reroute_intent: Option<Intent>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    contains_any: Vec<String>,
    #[serde(default)]
    must_not_contain: Vec<String>,
    #[serde(default)]
    recommend: Option<Recommend>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OutcomeKind {
    Reply,
    AskCustomer,
    Escalate,
    Reroute,
}

impl OutcomeKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Reply => "reply",
            Self::AskCustomer => "ask_customer",
            Self::Escalate => "escalate",
            Self::Reroute => "reroute",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recommend {
    #[serde(rename = "match")]
    filter: Filter,
    #[serde(default)]
    at_least: usize,
    /// The customer's own products (catalog IDs): never counted as recommendations.
    #[serde(default)]
    given: Vec<String>,
    /// Products the text must not recommend.
    #[serde(default)]
    avoid: Option<Filter>,
    #[serde(default)]
    around: Option<Filter>,
    #[serde(default)]
    pct: Option<u32>,
    #[serde(default)]
    min_eur: Option<f64>,
    #[serde(default)]
    max_eur: Option<f64>,
}

/// Which catalog products: a category and spec fields (`memory_type: ddr5`, `socket: am5`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Filter {
    #[serde(default)]
    category: Option<Category>,
    #[serde(default)]
    spec: BTreeMap<String, String>,
}

impl Filter {
    fn matches(&self, p: &Product) -> bool {
        if self.category.is_some_and(|c| c != p.category) {
            return false;
        }
        if self.spec.is_empty() {
            return true;
        }
        // The spec serializes as `{ "<category>": { fields } }`.
        let spec = serde_json::to_value(&p.spec).unwrap_or_default();
        let fields = spec
            .as_object()
            .and_then(|o| o.values().next())
            .and_then(serde_json::Value::as_object);
        self.spec.iter().all(|(key, want)| {
            fields.and_then(|f| f.get(key)).is_some_and(|v| match v {
                serde_json::Value::String(s) => s.eq_ignore_ascii_case(want),
                other => serde_json::from_str::<serde_json::Value>(want).is_ok_and(|w| w == *other),
            })
        })
    }
}

/// One scenario's result.
struct Outcome<'a> {
    scenario: &'a Scenario,
    kind: Option<OutcomeKind>,
    detail: String,
    /// AI-authored text (a reply or a question).
    ai: Option<AiReply>,
    tools: Vec<String>,
    attempts: u32,
    tokens: u32,
    failures: Vec<String>,
}

impl Outcome<'_> {
    fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Runs the agent scenarios.
pub async fn run(run: &Run<'_>, source: &str) -> Result<Report, String> {
    let file: EvalFile =
        serde_yaml::from_str(source).map_err(|e| format!("{}: {e}", run.source_name))?;
    let scenarios: Vec<&Scenario> = file
        .scenarios
        .iter()
        .filter(|s| run.selected(&s.id))
        .collect();
    println!(
        "Running {} agent scenarios with {} (judge: {}) …",
        scenarios.len(),
        run.models.agent_name,
        run.models.classifier_name
    );
    let mut results = Vec::new();
    for scenario in scenarios {
        let result = scenario_run(run, scenario).await?;
        if run.options.verbose || !result.passed() {
            println!(
                "  {} {:<28} {:<14} {}",
                if result.passed() { "ok  " } else { "FAIL" },
                scenario.id,
                result.kind.map_or("?", OutcomeKind::as_str),
                result.failures.join("; ")
            );
            if run.options.verbose
                && let Some(ai) = &result.ai
            {
                println!("       {}", ai.body.replace('\n', "\n       "));
            }
        }
        results.push(result);
    }

    let passed = results.iter().filter(|r| r.passed()).count();
    let pass_rate = ratio(passed, results.len()).unwrap_or(0.0);
    let mut failures = Vec::new();
    if pass_rate < file.minimums.pass_rate {
        failures.push(format!(
            "pass rate {} (min {})",
            pct(Some(pass_rate)),
            pct(Some(file.minimums.pass_rate))
        ));
    }
    let critical: Vec<&str> = results
        .iter()
        .filter(|r| r.scenario.critical && !r.passed())
        .map(|r| r.scenario.id.as_str())
        .collect();
    if !critical.is_empty() {
        failures.push(format!(
            "critical scenarios failed: {}",
            critical.join(", ")
        ));
    }

    let mut md = run.header("Agent eval");
    let _ = writeln!(
        md,
        "{} scenarios, each in a fresh seeded shop through `agents::pipeline::answer` (agent \
         loop, real tools, guardrails with one retry, judge).\n",
        results.len()
    );
    summary(&mut md, &results, &file.minimums);
    scenarios_section(&mut md, &results);
    let mock = run.options.backend == super::Backend::Mock;
    let suggested = judge_section(&mut md, &results, mock);
    details(&mut md, &results);

    println!();
    println!("  passed             {}", frac(passed, results.len()));
    let texts: Vec<&Outcome<'_>> = results.iter().filter(|r| r.ai.is_some()).collect();
    let first_try = texts
        .iter()
        .filter(|r| r.attempts <= 1 && r.ai.as_ref().is_some_and(AiReply::guardrails_passed))
        .count();
    println!("  guardrails 1st try {}", frac(first_try, texts.len()));
    match suggested {
        Some(t) if mock => println!(
            "  reply threshold    {t:.2} (meaningless on mock: its judge scores every text alike)"
        ),
        Some(t) => println!("  reply threshold    suggest {t:.2} (current default 0.80)"),
        None => {
            println!("  reply threshold    none: the judge does not separate good and bad texts")
        }
    }

    Ok(Report {
        markdown: md,
        failures,
    })
}

/// Runs one scenario and checks it.
async fn scenario_run<'a>(run: &Run<'_>, scenario: &'a Scenario) -> Result<Outcome<'a>, String> {
    let now = OffsetDateTime::now_utc();
    let shop = Shop::seed(&run.models.ports, now).await?;
    let fill = |text: &str| {
        text.replace("{my_order}", &shop.mine.number.to_string())
            .replace("{other_order}", &shop.theirs.number.to_string())
    };
    let message = fill(&scenario.message);
    let (route, intent) = scenario.agent.route();
    let names_mine = message.contains(&shop.mine.number.to_string());
    let req = RouteRequest {
        org_id: OrgId::new(shop::ORG),
        public_org_id: PublicOrgId::parse(shop::PUBLIC_ORG).map_err(|e| e.to_string())?,
        ticket_id: Shop::ticket(),
        generation: 1,
        user_id: UserId::new(shop::CUSTOMER),
        customer_email: "ada@example.com".into(),
        customer_name: Some("Ada".into()),
        route,
        intent,
        intent_p: 1.0,
        subject: "Question".into(),
        message: message.clone(),
        conversation: Vec::new(),
        order: names_mine.then(|| shop.mine.clone()),
        order_explicit: names_mine,
        customer_orders: vec![shop.mine.clone()],
        foreign_order_numbers: if message.contains(&shop.theirs.number.to_string()) {
            vec![shop.theirs.number]
        } else {
            Vec::new()
        },
        resumed: None,
    };
    let kind = scenario.agent.kind();
    let answered = answer(
        &Inline,
        &shop.deps,
        kind,
        &req,
        &OrgSettings::default(),
        now,
    )
    .await;

    // The stored trace: the agent's attempts (not the judge's pass).
    let scope = OrgScope::verified(OrgId::new(shop::ORG));
    let runs = shop.deps.db.agent_runs(&scope);
    let rows = runs
        .for_ticket(&Shop::ticket())
        .await
        .map_err(|e| e.to_string())?;
    let mut tools = Vec::new();
    let mut tools_ok = BTreeSet::new();
    let mut attempts = 0;
    let mut tokens = 0;
    // A run that failed (model down, budget, step limit) escalates, which must not pass as the
    // escalation a scenario expects.
    let mut run_errors = Vec::new();
    for row in rows.iter().filter(|r| r.agent == kind) {
        attempts = attempts.max(row.attempt);
        if row.status == AgentRunStatus::Failed {
            run_errors.push(format!(
                "agent run failed: {}",
                row.error.as_deref().unwrap_or("no reason recorded")
            ));
        }
        tokens += row.input_tokens + row.output_tokens;
        for step in runs.steps(&row.id).await.map_err(|e| e.to_string())? {
            if step.kind == AgentStepKind::ToolCall {
                tools.push(step.name.clone());
                if !step.is_error {
                    tools_ok.insert(step.name);
                }
            }
        }
    }

    let (outcome_kind, detail, ai) = match &answered.outcome {
        RouteOutcome::Reply { reply, .. } => {
            (Some(OutcomeKind::Reply), String::new(), ai_of(reply))
        }
        RouteOutcome::AskCustomer {
            question, expects, ..
        } => (
            Some(OutcomeKind::AskCustomer),
            format!("expects {}", expects.as_str()),
            ai_of(question),
        ),
        RouteOutcome::Escalate { reason, .. } => {
            (Some(OutcomeKind::Escalate), reason.clone(), None)
        }
        RouteOutcome::Reroute { intent } => (
            Some(OutcomeKind::Reroute),
            format!("as {}", intent.as_str()),
            None,
        ),
        other => (None, format!("{other:?}"), None),
    };

    let mut failures = run_errors;
    let expect = &scenario.expect;
    let expected = expect.outcome.to_vec();
    if !outcome_kind.is_some_and(|k| expected.contains(&k)) {
        failures.push(format!(
            "outcome {} ({detail}), expected {}",
            outcome_kind.map_or("other", OutcomeKind::as_str),
            expected
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(" / ")
        ));
    }
    if let (Some(want), RouteOutcome::Reroute { intent }) =
        (expect.reroute_intent, &answered.outcome)
        && *intent != want
    {
        failures.push(format!(
            "rerouted as {}, expected {}",
            intent.as_str(),
            want.as_str()
        ));
    }
    for tool in &expect.tools {
        if !tools_ok.contains(tool) {
            failures.push(format!("`{tool}` was not called (successfully)"));
        }
    }
    if let Some(ai) = &ai {
        failures.extend(text_checks(ai, expect, &fill));
        if outcome_kind == Some(OutcomeKind::Reply)
            && let Some(r) = &expect.recommend
        {
            failures.extend(recommend_checks(&ai.body, r));
        }
    }

    Ok(Outcome {
        scenario,
        kind: outcome_kind,
        detail,
        ai,
        tools,
        attempts,
        tokens,
        failures,
    })
}

fn ai_of(reply: &Reply) -> Option<AiReply> {
    match reply {
        Reply::Ai(ai) => Some(ai.clone()),
        Reply::Template { .. } => None,
    }
}

/// Grounding and the text expectations on AI-authored text.
fn text_checks(ai: &AiReply, expect: &Expect, fill: &dyn Fn(&str) -> String) -> Vec<String> {
    let mut failures = Vec::new();
    let text = &ai.body;
    let lower = text.to_lowercase();
    for g in ai.guardrails.iter().filter(|g| !g.passed) {
        failures.push(format!(
            "guardrail {} failed: {}",
            g.guardrail.as_str(),
            g.violations.join(", ")
        ));
    }
    let catalog = Catalog::global();
    for m in catalog.product_mentions(text) {
        if let MentionTarget::Ambiguous(ps) = &m.target {
            failures.push(format!(
                "ambiguous product name \"{}\" ({} products)",
                m.text,
                ps.len()
            ));
        }
    }
    let skus: BTreeSet<&str> = catalog.all().iter().map(|p| p.sku.as_str()).collect();
    for token in sku_like(text) {
        if !skus.contains(token) {
            failures.push(format!("unknown SKU {token}"));
        }
    }
    for phrase in &expect.contains {
        let phrase = fill(phrase);
        if !lower.contains(&phrase.to_lowercase()) {
            failures.push(format!("missing \"{phrase}\""));
        }
    }
    if !expect.contains_any.is_empty()
        && !expect
            .contains_any
            .iter()
            .any(|p| lower.contains(&fill(p).to_lowercase()))
    {
        failures.push(format!(
            "none of {}",
            expect
                .contains_any
                .iter()
                .map(|p| format!("\"{p}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for phrase in &expect.must_not_contain {
        let phrase = fill(phrase);
        if lower.contains(&phrase.to_lowercase()) {
            failures.push(format!("contains \"{phrase}\""));
        }
    }
    failures
}

/// SKU-shaped tokens: three or more dash-separated segments, the first two all capitals
/// (`RAM-NIM-D5-6000-2X16`, `CPU-VOL-8600`).
fn sku_like(text: &str) -> Vec<&str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .map(|t| t.trim_matches('-'))
        .filter(|t| {
            let parts: Vec<&str> = t.split('-').collect();
            let caps =
                |s: &str| (2..=4).contains(&s.len()) && s.chars().all(|c| c.is_ascii_uppercase());
            parts.len() >= 3 && caps(parts[0]) && caps(parts[1])
        })
        .collect()
}

/// The recommended products: catalog products the reply names that match the filter.
fn recommend_checks(text: &str, r: &Recommend) -> Vec<String> {
    let mut failures = Vec::new();
    let named: Vec<&Product> = Catalog::global()
        .product_mentions(text)
        .into_iter()
        .filter_map(|m| match m.target {
            MentionTarget::Product(p) => Some(p),
            MentionTarget::Ambiguous(_) => None,
        })
        .collect();
    let mut candidates: Vec<&Product> = named
        .iter()
        .copied()
        .filter(|p| !r.given.iter().any(|g| g == p.id.as_str()))
        .collect();
    candidates.sort_by(|a, b| a.id.cmp(&b.id));
    candidates.dedup_by(|a, b| a.id == b.id);
    if let Some(avoid) = &r.avoid {
        for p in candidates.iter().filter(|p| avoid.matches(p)) {
            failures.push(format!("recommends {}, which it should not", p.name));
        }
    }
    let recommended: Vec<&Product> = candidates
        .into_iter()
        .filter(|p| r.filter.matches(p))
        .collect();
    if recommended.len() < r.at_least {
        failures.push(format!(
            "recommends {} matching product(s), expected at least {}",
            recommended.len(),
            r.at_least
        ));
    }
    let around = r.around.as_ref().and_then(|f| {
        named
            .iter()
            .copied()
            .find(|p| f.matches(p) && !recommended.iter().any(|x| x.id == p.id))
    });
    if r.around.is_some() && around.is_none() && !recommended.is_empty() {
        failures.push("names no reference product for the price band".to_owned());
    }
    for p in &recommended {
        let price = p.price.cents();
        if let (Some(base), Some(pct)) = (around, r.pct) {
            let band = techdepo_compat::price_band_around(base.price, pct);
            if !band.contains(&p.price) {
                failures.push(format!(
                    "{} at {} is outside ±{pct} % of {} at {}",
                    p.name, p.price, base.name, base.price
                ));
            }
        }
        #[allow(clippy::cast_possible_truncation)] // euros to cents
        let cents = |eur: f64| (eur * 100.0).round() as i64;
        if r.min_eur.is_some_and(|min| price < cents(min))
            || r.max_eur.is_some_and(|max| price > cents(max))
        {
            failures.push(format!("{} at {} is outside the budget", p.name, p.price));
        }
    }
    failures
}

fn summary(md: &mut String, results: &[Outcome<'_>], minimums: &Minimums) {
    let _ = writeln!(md, "## Summary\n");
    let passed = results.iter().filter(|r| r.passed()).count();
    let mut table = Md::new(&["Metric", "Value", "Minimum"], &[1, 2]);
    table.row(&[
        "Scenarios passed".to_owned(),
        frac(passed, results.len()),
        pct(Some(minimums.pass_rate)),
    ]);
    let critical: Vec<&Outcome<'_>> = results.iter().filter(|r| r.scenario.critical).collect();
    table.row(&[
        "Critical scenarios passed".to_owned(),
        frac(
            critical.iter().filter(|r| r.passed()).count(),
            critical.len(),
        ),
        "all".to_owned(),
    ]);
    for agent in [AgentKind::ProductAdvisor, AgentKind::OrderAssistant] {
        let of: Vec<&Outcome<'_>> = results
            .iter()
            .filter(|r| r.scenario.agent.kind() == agent)
            .collect();
        table.row(&[
            format!("… {}", agent.label()),
            frac(of.iter().filter(|r| r.passed()).count(), of.len()),
            String::new(),
        ]);
    }
    let texts: Vec<&Outcome<'_>> = results.iter().filter(|r| r.ai.is_some()).collect();
    let first_try = texts
        .iter()
        .filter(|r| r.attempts <= 1 && r.ai.as_ref().is_some_and(AiReply::guardrails_passed))
        .count();
    let after_retry = texts
        .iter()
        .filter(|r| r.ai.as_ref().is_some_and(AiReply::guardrails_passed))
        .count();
    table.row(&[
        "Guardrails passed on the first try (AI texts)".to_owned(),
        frac(first_try, texts.len()),
        String::new(),
    ]);
    table.row(&[
        "Guardrails passed after the retry".to_owned(),
        frac(after_retry, texts.len()),
        String::new(),
    ]);
    let compat_needed: Vec<&Outcome<'_>> = results
        .iter()
        .filter(|r| {
            r.scenario
                .expect
                .tools
                .iter()
                .any(|t| t == "check_compatibility")
        })
        .collect();
    table.row(&[
        "`check_compatibility` called where required".to_owned(),
        frac(
            compat_needed
                .iter()
                .filter(|r| r.tools.iter().any(|t| t == "check_compatibility"))
                .count(),
            compat_needed.len(),
        ),
        String::new(),
    ]);
    let must_escalate: Vec<&Outcome<'_>> = results
        .iter()
        .filter(|r| r.scenario.expect.outcome.to_vec() == [OutcomeKind::Escalate])
        .collect();
    table.row(&[
        "Escalated where required (out of scope, injection, person)".to_owned(),
        frac(
            must_escalate
                .iter()
                .filter(|r| r.kind == Some(OutcomeKind::Escalate))
                .count(),
            must_escalate.len(),
        ),
        String::new(),
    ]);
    let tokens: u32 = results.iter().map(|r| r.tokens).sum();
    table.row(&[
        "Agent tokens (all scenarios)".to_owned(),
        tokens.to_string(),
        String::new(),
    ]);
    md.push_str(&table.finish());
}

fn scenarios_section(md: &mut String, results: &[Outcome<'_>]) {
    let _ = writeln!(md, "## Scenarios\n");
    let mut table = Md::new(
        &[
            "",
            "Scenario",
            "Agent",
            "Outcome",
            "Tools called",
            "Attempts",
            "Judge",
            "Tokens",
            "Problems",
        ],
        &[5, 6, 7],
    );
    for r in results {
        let mut tools: Vec<String> = Vec::new();
        for t in &r.tools {
            match tools.last_mut() {
                Some(last) if last == t || last.starts_with(&format!("{t} ×")) => {
                    let n = last
                        .rsplit_once('×')
                        .and_then(|(_, n)| n.parse::<u32>().ok())
                        .unwrap_or(1);
                    *last = format!("{t} ×{}", n + 1);
                }
                _ => tools.push(t.clone()),
            }
        }
        let outcome = match (r.kind, r.detail.as_str()) {
            (Some(k), "") => k.as_str().to_owned(),
            (Some(OutcomeKind::Escalate), _) => "escalate".to_owned(),
            (Some(k), d) => format!("{} ({d})", k.as_str()),
            (None, d) => d.to_owned(),
        };
        table.row(&[
            if r.passed() { "✓" } else { "✗" }.to_owned(),
            if r.scenario.critical {
                format!("`{}` (critical)", r.scenario.id)
            } else {
                format!("`{}`", r.scenario.id)
            },
            r.scenario.agent.kind().label().to_owned(),
            outcome,
            tools.join(", "),
            r.attempts.to_string(),
            r.ai.as_ref()
                .and_then(|a| a.judge)
                .map_or_else(|| "·".to_owned(), |j| format!("{:.2}", j.score())),
            r.tokens.to_string(),
            r.failures.join("; "),
        ]);
    }
    md.push_str(&table.finish());
}

/// Judge scores of the texts that would be sent; returns a suggested reply threshold.
fn judge_section(md: &mut String, results: &[Outcome<'_>], mock: bool) -> Option<f64> {
    let judged: Vec<(f64, bool)> = results
        .iter()
        .filter_map(|r| {
            let ai = r.ai.as_ref()?;
            ai.judge.map(|j| (j.score(), r.passed()))
        })
        .collect();
    let good: Vec<f64> = judged
        .iter()
        .filter(|(_, ok)| *ok)
        .map(|(s, _)| *s)
        .collect();
    let bad: Vec<f64> = judged
        .iter()
        .filter(|(_, ok)| !*ok)
        .map(|(s, _)| *s)
        .collect();
    let _ = writeln!(
        md,
        "## Judge and reply confidence\n\nThe judge scores every AI text that passed the \
         guardrails (the lowest of its three `P(yes)`; with `p(intent)` = 1.0 that is the reply \
         confidence). *Good* texts passed every check of their scenario, *bad* ones did not. \
         `send_above_confidence` sends a text whose confidence reaches the threshold and holds \
         the rest for review.\n"
    );
    if mock {
        let _ = writeln!(
            md,
            "**Mock backend: this section measures nothing.** The offline judge gives every \
             grounded, polite draft about the same scores (0.85–0.90) whether its scenario passed \
             or not, so good and bad texts overlap and the suggested threshold below is \
             arbitrary. Set the `send_above_confidence` default from a run on real models \
             (`--backend workers-ai` or `--agent-model`).\n"
        );
    }
    let fmt = |v: Option<f64>| v.map_or_else(|| "·".to_owned(), |v| format!("{v:.2}"));
    let min = |v: &[f64]| v.iter().copied().reduce(f64::min);
    let max = |v: &[f64]| v.iter().copied().reduce(f64::max);
    let mut table = Md::new(&["Texts", "Count", "Min", "Mean", "Max"], &[1, 2, 3, 4]);
    table.row(&[
        "Good".to_owned(),
        good.len().to_string(),
        fmt(min(&good)),
        fmt(mean(&good)),
        fmt(max(&good)),
    ]);
    table.row(&[
        "Bad".to_owned(),
        bad.len().to_string(),
        fmt(min(&bad)),
        fmt(mean(&bad)),
        fmt(max(&bad)),
    ]);
    md.push_str(&table.finish());

    let mut table = Md::new(&["Threshold", "Good sent", "Bad sent"], &[1, 2]);
    // The lowest threshold that holds every bad text, if it still sends a good one.
    let mut suggested = None;
    for &t in SWEEP {
        let good_sent = good.iter().filter(|s| **s >= t).count();
        let bad_sent = bad.iter().filter(|s| **s >= t).count();
        if suggested.is_none() && bad_sent == 0 && good_sent > 0 {
            suggested = Some(t);
        }
        let current = (t - OrgSettings::default().reply_confidence_threshold).abs() < 1e-9;
        table.row(&[
            if current {
                format!("**{t:.2}** (default)")
            } else {
                format!("{t:.2}")
            },
            frac(good_sent, good.len()),
            frac(bad_sent, bad.len()),
        ]);
    }
    md.push_str(&table.finish());
    // With no bad text to separate, the lowest threshold that still sends every good one.
    let suggested = match (suggested, bad.is_empty()) {
        (Some(t), false) => Some(t),
        (_, true) => min(&good).map(|m| {
            SWEEP
                .iter()
                .copied()
                .filter(|t| *t <= m)
                .reduce(f64::max)
                .unwrap_or(SWEEP[0])
        }),
        (None, false) => None,
    };
    let _ = match suggested {
        Some(t) => writeln!(
            md,
            "**Suggested `send_above_confidence` threshold: {t:.2}** (default {:.2}){}.\n",
            OrgSettings::default().reply_confidence_threshold,
            if bad.is_empty() {
                ": no judged text failed its checks, so this is the highest threshold that still \
                 sends every good text; a real model's failures move it up"
            } else {
                ": the lowest threshold that holds every bad text"
            }
        ),
        None => writeln!(
            md,
            "**No threshold holds every bad text and still sends a good one**: the judge does \
             not separate them, so its score cannot set the default; keep replies under review \
             (`always_review`) for this model, or improve the judge.\n"
        ),
    };
    suggested
}

fn details(md: &mut String, results: &[Outcome<'_>]) {
    let _ = writeln!(md, "## Texts\n");
    for r in results {
        let _ = writeln!(
            md,
            "### {} `{}`\n",
            if r.passed() { "✓" } else { "✗" },
            r.scenario.id
        );
        if let Some(note) = &r.scenario.note {
            let _ = writeln!(md, "{note}\n");
        }
        let _ = writeln!(md, "> {}\n", r.scenario.message.replace('\n', "\n> "));
        match &r.ai {
            Some(ai) => {
                let _ = writeln!(md, "```text\n{}\n```\n", ai.body.trim());
            }
            None => {
                let _ = writeln!(
                    md,
                    "*{}*{}\n",
                    r.kind.map_or("other", OutcomeKind::as_str),
                    if r.detail.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", r.detail)
                    }
                );
            }
        }
        if !r.failures.is_empty() {
            for f in &r.failures {
                let _ = writeln!(md, "- {f}");
            }
            md.push('\n');
        }
    }
}
