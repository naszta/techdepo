//! Guardrails: plain code run on every AI-authored text (an agent's reply or question) before
//! anything can be sent (support.md §5). Each returns a [`GuardrailResult`]; a failure gives the
//! agent one retry with [`feedback`], then the ticket escalates. The text is never sent when one
//! fails, whatever the reply policy.
//!
//! 1. **Grounding:** every product name in the text is one product (the full name, or a prefix
//!    only one product has: `Catalog::product_mentions`), not a made-up variant of one; a price
//!    next to a product is that product's catalog price; every other price comes from a tool
//!    result; every SKU exists.
//! 2. **Compatibility claims:** a text that says parts do (or do not) work together needs a
//!    `check_compatibility` call in the trace covering the parts it names (or alternatives that
//!    `find_alternatives` returned, which the engine checked), with a matching verdict.
//! 3. **Format:** not empty, not too long, no internal IDs, no template placeholders, links only
//!    to the shop's own domain.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex_lite::Regex;
use serde_json::Value;
use techdepo_agent::Trace;
use techdepo_api_types::support::{Guardrail, GuardrailResult};
use techdepo_catalog::{Catalog, MentionTarget, ProductMention};
use techdepo_domain::{Money, ProductId};

use super::tools::catalog::lookup;

/// Longest AI-authored text sent, in characters.
pub const MAX_TEXT_CHARS: usize = 2_000;

/// How far after a product name a price is taken to be that product's, in bytes.
const PRICE_REACH: usize = 60;

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|e| unreachable!("invalid guardrail regex: {e}"))
}

/// `€1,435.90`, `€ 69`, `69.90 €`, `69,90 EUR`.
static PRICE: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"€\s?(\d{1,3}(?:,\d{3})+|\d+)(?:\.(\d{1,2}))?|\b(\d+)(?:[.,](\d{2}))?\s?(?:€|EUR\b|euros?\b)",
    )
});

/// SKU-shaped tokens, e.g. `RAM-NIM-D5-5200-2X16`.
static SKU: LazyLock<Regex> =
    LazyLock::new(|| regex(r"\b[A-Z]{2,5}-[A-Z]{2,4}-[A-Z0-9]+(?:-[A-Z0-9]+)*\b"));

/// Internal identifiers (`ord_…`, `run_…`, Clerk's `org_…`/`user_…`, provider references).
static INTERNAL_ID: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"\b(?:adr|bld|eml|ref|ch|inv|ord|pay|re|run|shp|tsk|inq|org|user|cus)_[A-Za-z0-9_]{6,}\b",
    )
});

/// Links.
static LINK: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?i)\b(?:https?://|www\.)[^\s<>()\[\]{}'\x22]+"));

/// Email addresses (mail clients make them links).
static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?i)\b[a-z0-9._%+-]+@((?:[a-z0-9-]+\.)+[a-z]{2,24})\b"));

/// Bare domains, optionally with a path (`example.com/verify`; mail clients make them links). The
/// last label is letters only, so prices, versions and sizes (`€69.90`, `PCIe 4.0`, `3.5mm`) are
/// not domains.
static BARE_DOMAIN: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"(?i)\b(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,24}\b(?:/[^\s<>()\[\]{}'\x22]*)?")
});

fn result(guardrail: Guardrail, violations: Vec<String>) -> GuardrailResult {
    GuardrailResult {
        guardrail,
        passed: violations.is_empty(),
        violations,
    }
}

/// Runs every guardrail on `text` (written in a run with `trace`). `hosts`: the shop's own
/// domains, which links may point to.
pub fn check(text: &str, trace: &Trace, hosts: &[String]) -> Vec<GuardrailResult> {
    let mentions = Catalog::global().product_mentions(text);
    vec![
        grounding(text, &mentions, trace),
        compatibility_claims(text, &mentions, trace),
        format(text, hosts),
    ]
}

/// Whether every result passed.
pub fn passed(results: &[GuardrailResult]) -> bool {
    results.iter().all(|r| r.passed)
}

/// The retry's feedback: what failed and how to fix it.
pub fn feedback(results: &[GuardrailResult]) -> String {
    let mut out = String::from("Your reply was NOT sent: it failed the shop's automatic checks:\n");
    for v in results
        .iter()
        .filter(|r| !r.passed)
        .flat_map(|r| &r.violations)
    {
        out.push_str(&format!("- {v}\n"));
    }
    out.push_str(
        "Write the reply again. Use product names and prices exactly as the tools returned \
         them, check parts with check_compatibility before saying whether they fit, and leave \
         out internal IDs and links to other websites. If you cannot answer reliably, call \
         escalate_to_human.",
    );
    out
}

fn money(euros: &str, cents: Option<&str>) -> Option<Money> {
    let euros: i64 = euros.replace(',', "").parse().ok()?;
    let cents: i64 = match cents {
        Some(c) if c.len() == 1 => c.parse::<i64>().ok()? * 10,
        Some(c) => c.parse().ok()?,
        None => 0,
    };
    Some(Money::from_cents(euros * 100 + cents))
}

/// Every price in `text`, with its byte offset.
fn prices(text: &str) -> Vec<(usize, Money)> {
    PRICE
        .captures_iter(text)
        .filter_map(|c| {
            let at = c.get(0)?.start();
            let amount = match (c.get(1), c.get(3)) {
                (Some(e), _) => money(e.as_str(), c.get(2).map(|m| m.as_str())),
                (None, Some(e)) => money(e.as_str(), c.get(4).map(|m| m.as_str())),
                _ => None,
            }?;
            Some((at, amount))
        })
        .collect()
}

/// Amounts the tools returned: prices in their text, `*_cents` numbers.
fn tool_amounts(trace: &Trace) -> BTreeSet<Money> {
    fn walk(value: &Value, key: Option<&str>, out: &mut BTreeSet<Money>) {
        match value {
            Value::String(s) => out.extend(prices(s).into_iter().map(|(_, m)| m)),
            Value::Number(n) if key.is_some_and(|k| k.ends_with("_cents")) => {
                if let Some(c) = n.as_i64() {
                    out.insert(Money::from_cents(c));
                }
            }
            Value::Array(items) => items.iter().for_each(|v| walk(v, None, out)),
            Value::Object(map) => map.iter().for_each(|(k, v)| walk(v, Some(k), out)),
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    for step in tool_steps(trace) {
        walk(&step.output, None, &mut out);
    }
    out
}

/// The successful tool calls of `trace`.
fn tool_steps(trace: &Trace) -> impl Iterator<Item = &techdepo_agent::TraceStep> {
    trace.steps.iter().filter(|s| {
        s.kind == techdepo_api_types::vocab::AgentStepKind::ToolCall
            && !s.is_error
            && !techdepo_agent::is_terminal(&s.name)
    })
}

/// The byte offset just after `mention` in `text` (its words may be separated by any
/// whitespace there).
fn mention_end(text: &str, mention: &ProductMention<'_>) -> usize {
    let mut at = mention.offset;
    for word in mention.text.split(' ') {
        match text[at..].find(word) {
            Some(i) => at += i + word.len(),
            None => break,
        }
    }
    at
}

fn grounding(text: &str, mentions: &[ProductMention<'_>], trace: &Trace) -> GuardrailResult {
    let catalog = Catalog::global();
    let mut violations = Vec::new();
    let mut named_prices = BTreeSet::new();
    for (i, m) in mentions.iter().enumerate() {
        let product = match &m.target {
            MentionTarget::Product(p) => *p,
            MentionTarget::Ambiguous(products) => {
                let names: Vec<&str> = products.iter().map(|p| p.name.as_str()).collect();
                violations.push(format!(
                    "\"{}\" could be several products ({}); use the full product name",
                    m.text,
                    names.join(", ")
                ));
                continue;
            }
        };
        named_prices.insert(product.price);
        let end = mention_end(text, m);
        // A capitalised word right after the name: a variant we do not sell ("Kestrel B650
        // Turbo" is not the Kestrel B650 Aero).
        if let Some(next) = text[end..]
            .strip_prefix(' ')
            .and_then(|r| r.split_whitespace().next())
        {
            let word = next.trim_end_matches(|c: char| !c.is_alphanumeric());
            let mut chars = word.chars();
            if word != "I"
                && chars.next().is_some_and(|c| c.is_ascii_uppercase())
                && chars.clone().count() > 0
                && chars.all(|c| c.is_ascii_lowercase())
            {
                violations.push(format!(
                    "\"{} {word}\" is not a product in our catalog (did you mean the {}?)",
                    m.text, product.name
                ));
            }
        }
        // The price right after the name is that product's.
        let limit = mentions
            .get(i + 1)
            .map_or(text.len(), |n| n.offset)
            .min(end + PRICE_REACH)
            .max(end);
        let limit = (end..=limit.min(text.len()))
            .rev()
            .find(|&b| text.is_char_boundary(b))
            .unwrap_or(end);
        if let Some((_, price)) = prices(&text[end..limit]).first()
            && *price != product.price
        {
            violations.push(format!(
                "the {} costs {}, not {price}",
                product.name, product.price
            ));
        }
    }
    let known = tool_amounts(trace);
    for (_, price) in prices(text) {
        if !named_prices.contains(&price) && !known.contains(&price) {
            violations.push(format!(
                "{price} is not a catalog price or an amount the tools returned"
            ));
        }
    }
    let sku_prefixes: BTreeSet<&str> = catalog
        .all()
        .iter()
        .filter_map(|p| p.sku.as_str().split('-').next())
        .collect();
    for sku in SKU.find_iter(text).map(|m| m.as_str()) {
        let prefix = sku.split('-').next().unwrap_or_default();
        if sku_prefixes.contains(prefix) && !catalog.all().iter().any(|p| p.sku.as_str() == sku) {
            violations.push(format!("SKU {sku} is not in our catalog"));
        }
    }
    violations.dedup();
    result(Guardrail::Grounding, violations)
}

/// Phrases saying parts do not work together.
const NEGATIVE: &[&str] = &[
    "not compatible",
    "incompatible",
    "won't work",
    "will not work",
    "wouldn't work",
    "would not work",
    "doesn't work",
    "does not work",
    "don't work",
    "do not work",
    "won't fit",
    "will not fit",
    "doesn't fit",
    "does not fit",
    "not work together",
    "cannot be used",
    "can't be used",
    "not supported",
];

/// Phrases saying parts do work together.
const POSITIVE: &[&str] = &[
    "compatible",
    "works with",
    "work with",
    "will work",
    "would work",
    "work together",
    "works together",
    "will fit",
    "fits in",
    "fits into",
    "fits on",
    "is supported",
    "are supported",
];

/// The product IDs in a tool's arguments or result (strings that are catalog IDs or names).
fn ids_in(value: &Value, out: &mut BTreeSet<ProductId>) {
    match value {
        Value::String(s) => {
            if let Some(p) = lookup(s) {
                out.insert(p.id.clone());
            }
        }
        Value::Array(items) => items.iter().for_each(|v| ids_in(v, out)),
        Value::Object(map) => map.values().for_each(|v| ids_in(v, out)),
        _ => {}
    }
}

fn compatibility_claims(
    text: &str,
    mentions: &[ProductMention<'_>],
    trace: &Trace,
) -> GuardrailResult {
    let lower = text.to_lowercase().replace(['’', '‘'], "'");
    let negative = NEGATIVE.iter().any(|p| lower.contains(p));
    let mut rest = lower.clone();
    for p in NEGATIVE {
        rest = rest.replace(p, " ");
    }
    let positive = POSITIVE.iter().any(|p| rest.contains(p));
    if !negative && !positive {
        return result(Guardrail::CompatibilityClaims, Vec::new());
    }

    let mut checks: Vec<(BTreeSet<ProductId>, String)> = Vec::new();
    let mut alternatives = BTreeSet::new();
    for step in tool_steps(trace) {
        match step.name.as_str() {
            "check_compatibility" => {
                let mut parts = BTreeSet::new();
                ids_in(&step.input, &mut parts);
                let verdict = step.output["verdict"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                checks.push((parts, verdict));
            }
            "find_alternatives" => ids_in(&step.output["candidates"], &mut alternatives),
            _ => {}
        }
    }
    let mut violations = Vec::new();
    if checks.is_empty() {
        violations.push(
            "the reply says whether parts work together, but check_compatibility was never called"
                .to_owned(),
        );
        return result(Guardrail::CompatibilityClaims, violations);
    }
    let checked: BTreeSet<&ProductId> = checks.iter().flat_map(|(parts, _)| parts).collect();
    let mut uncovered: Vec<&str> = mentions
        .iter()
        .filter_map(|m| match &m.target {
            MentionTarget::Product(p) => Some(*p),
            MentionTarget::Ambiguous(_) => None,
        })
        .filter(|p| !checked.contains(&p.id) && !alternatives.contains(&p.id))
        .map(|p| p.name.as_str())
        .collect();
    uncovered.dedup();
    if !uncovered.is_empty() {
        violations.push(format!(
            "the reply makes a compatibility claim about {} without a check_compatibility call \
             covering it",
            uncovered.join(", ")
        ));
    }
    let incompatible = checks.iter().any(|(_, v)| v == "incompatible");
    if negative && !incompatible {
        violations.push(
            "the reply says parts do not work together, but check_compatibility found no \
             incompatibility"
                .to_owned(),
        );
    }
    if positive && !negative && incompatible && alternatives.is_empty() {
        violations.push(
            "the reply says parts work together, but check_compatibility found them incompatible"
                .to_owned(),
        );
    }
    result(Guardrail::CompatibilityClaims, violations)
}

fn host_of(link: &str) -> String {
    let rest = link
        .split_once("://")
        .map_or(link, |(_, rest)| rest)
        .trim_start_matches("www.");
    rest.split(['/', '?', '#', ':'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_lowercase()
}

fn format(text: &str, hosts: &[String]) -> GuardrailResult {
    let mut violations = Vec::new();
    let chars = text.chars().count();
    if text.trim().is_empty() {
        violations.push("the reply is empty".to_owned());
    } else if chars > MAX_TEXT_CHARS {
        violations.push(format!(
            "the reply is too long ({chars} characters, at most {MAX_TEXT_CHARS})"
        ));
    }
    let mut ids: Vec<&str> = INTERNAL_ID.find_iter(text).map(|m| m.as_str()).collect();
    ids.extend(
        Catalog::global()
            .all()
            .iter()
            .map(|p| p.id.as_str())
            .filter(|id| {
                text.match_indices(id).any(|(at, _)| {
                    let before = text[..at].chars().next_back();
                    let after = text[at + id.len()..].chars().next();
                    !before.is_some_and(|c| c.is_alphanumeric() || c == '-')
                        && !after.is_some_and(|c| c.is_alphanumeric() || c == '-')
                })
            }),
    );
    if !ids.is_empty() {
        violations.push(format!(
            "the reply shows internal identifiers ({}); name products and orders as customers see them",
            ids.join(", ")
        ));
    }
    let lower = text.to_lowercase();
    if [
        "{{",
        "}}",
        "[your name",
        "[customer",
        "<insert",
        "[insert",
        "[name]",
    ]
    .iter()
    .any(|p| lower.contains(p))
    {
        violations.push("the reply contains a template placeholder".to_owned());
    }
    // Links, email addresses and bare domains all become links in the customer's mail client:
    // each host once, and only our own.
    let mut seen = Vec::new();
    let found = EMAIL
        .captures_iter(text)
        .filter_map(|c| Some((c.get(1)?.as_str().to_lowercase(), c.get(0)?.as_str())))
        .chain(
            LINK.find_iter(text)
                .map(|m| (host_of(m.as_str()), m.as_str())),
        )
        .chain(
            BARE_DOMAIN
                .find_iter(text)
                .map(|m| (host_of(m.as_str()), m.as_str())),
        );
    for (host, shown) in found {
        if host.is_empty() || seen.contains(&host) {
            continue;
        }
        let ours = hosts
            .iter()
            .any(|h| host == *h || host.ends_with(&format!(".{h}")));
        if !ours {
            violations.push(format!("the reply links to another website ({shown})"));
        }
        seen.push(host);
    }
    result(Guardrail::Format, violations)
}

/// The domains links may point to: the shop's base URL and the support domain.
pub fn allowed_hosts(config: &crate::Config) -> Vec<String> {
    let mut hosts = vec![
        host_of(&config.app_base_url),
        config.support_domain.to_lowercase(),
    ];
    hosts.retain(|h| !h.is_empty());
    hosts.dedup();
    hosts
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use techdepo_agent::TraceStep;
    use techdepo_api_types::vocab::AgentStepKind;
    use techdepo_ports::Usage;

    use super::*;

    fn tool(name: &str, input: Value, output: Value) -> TraceStep {
        TraceStep {
            idx: 0,
            kind: AgentStepKind::ToolCall,
            name: name.into(),
            journal_name: None,
            input,
            output,
            is_error: false,
            usage: Usage::default(),
            started_at_ms: 0,
            duration_ms: 0,
        }
    }

    fn trace(steps: Vec<TraceStep>) -> Trace {
        Trace {
            steps,
            ..Trace::default()
        }
    }

    fn hosts() -> Vec<String> {
        allowed_hosts(&crate::Config::default())
    }

    fn violations(text: &str, t: &Trace) -> Vec<String> {
        check(text, t, &hosts())
            .into_iter()
            .flat_map(|r| r.violations)
            .collect()
    }

    fn scenario_8() -> Trace {
        trace(vec![
            tool(
                "check_compatibility",
                json!({ "product_ids": ["kestrel-b650-aero", "nimbus-ddr4-3200-32"] }),
                json!({ "verdict": "incompatible" }),
            ),
            tool(
                "find_alternatives",
                json!({ "parts": ["kestrel-b650-aero", "nimbus-ddr4-3200-32"], "replace_category": "memory" }),
                json!({ "candidates": [{ "id": "nimbus-ddr5-5200-32", "price": "€69.90" }] }),
            ),
        ])
    }

    #[test]
    fn a_grounded_answer_passes() {
        let text = "Unfortunately, the Kestrel B650 Aero and the Nimbus Cirrus DDR4-3200 32 GB \
                    (2x16 GB) won't work together: the board takes DDR5 memory. The Nimbus \
                    Stratus DDR5-5200 32 GB (2x16 GB) (€69.90) would replace the kit. See \
                    http://localhost:8787/products/x.";
        assert_eq!(violations(text, &scenario_8()), Vec::<String>::new());
    }

    #[test]
    fn wrong_prices_unknown_products_and_skus_fail() {
        let t = scenario_8();
        let v = violations(
            "The Nimbus Stratus DDR5-5200 32 GB (2x16 GB) costs €59.90.",
            &t,
        );
        assert!(
            v.iter().any(|v| v.contains("costs €69.90, not €59.90")),
            "{v:?}"
        );
        let v = violations("Try the Kestrel B650 Turbo instead.", &t);
        assert!(v.iter().any(|v| v.contains("not a product")), "{v:?}");
        let v = violations("The Halcyon HX 7070 is great.", &t);
        assert!(v.iter().any(|v| v.contains("several products")), "{v:?}");
        let v = violations("Order SKU RAM-NIM-D9-1 for €12.34.", &t);
        assert!(v.iter().any(|v| v.contains("SKU")), "{v:?}");
        assert!(v.iter().any(|v| v.contains("€12.34")), "{v:?}");
    }

    #[test]
    fn compatibility_claims_need_a_matching_check() {
        let v = violations(
            "Yes, the Kestrel B650 Aero and the Nimbus Cirrus DDR4-3200 32 GB (2x16 GB) are compatible.",
            &trace(Vec::new()),
        );
        assert!(v.iter().any(|v| v.contains("never called")), "{v:?}");
        let v = violations(
            "Yes, the Kestrel B650 Aero and the Nimbus Cirrus DDR4-3200 32 GB (2x16 GB) are compatible.",
            &trace(vec![tool(
                "check_compatibility",
                json!({ "product_ids": ["kestrel-b650-aero", "nimbus-ddr4-3200-32"] }),
                json!({ "verdict": "incompatible" }),
            )]),
        );
        assert!(
            v.iter().any(|v| v.contains("found them incompatible")),
            "{v:?}"
        );
        let v = violations(
            "The Kestrel B650 Aero works with the Lumen Apex DDR5-7200 32 GB (2x16 GB).",
            &scenario_8(),
        );
        assert!(v.iter().any(|v| v.contains("Lumen Apex")), "{v:?}");
    }

    #[test]
    fn format_rejects_ids_placeholders_and_foreign_links() {
        let t = trace(Vec::new());
        let v = violations("Your order ord_0123456789abcdef is on its way.", &t);
        assert!(
            v.iter().any(|v| v.contains("internal identifiers")),
            "{v:?}"
        );
        let v = violations("Have a look at kestrel-b650-aero.", &t);
        assert!(
            v.iter().any(|v| v.contains("internal identifiers")),
            "{v:?}"
        );
        let v = violations("Hi [Customer Name], see https://evil.example/x", &t);
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(
            violations(
                "Write to support@techdepo.dev or see https://shop.techdepo.dev/x",
                &t
            )
            .is_empty()
        );
        assert!(!violations(&"x".repeat(MAX_TEXT_CHARS + 1), &t).is_empty());
        let fb = feedback(&check("See https://evil.example", &t, &hosts()));
        assert!(fb.contains("evil.example") && fb.contains("NOT sent"));
    }

    #[test]
    fn bare_domains_and_addresses_are_links_too() {
        let t = trace(Vec::new());
        let v = violations(
            "Please confirm your card at techdepo-billing.com/verify.",
            &t,
        );
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("techdepo-billing.com/verify"), "{v:?}");
        let v = violations("Write to refunds@evil.example for the refund.", &t);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("refunds@evil.example"), "{v:?}");
        let v = violations(
            "Mail refunds@evil.example or visit https://evil.example/x",
            &t,
        );
        assert_eq!(v.len(), 1, "one violation per host: {v:?}");
        // Versions, sizes and abbreviations are not domains; our own domain is fine.
        let v = violations(
            "A 3.5mm jack and PCIe 4.0 cards, e.g. for order TD-1042. Write to \
             support@techdepo.dev or see shop.techdepo.dev/orders.",
            &t,
        );
        assert!(v.is_empty(), "{v:?}");
    }
}
