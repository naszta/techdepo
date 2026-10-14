//! The scripted chat model behind `LLM_BACKEND=mock`.
//!
//! [`MockLlm`] plays the support agents offline. On every call it re-reads the conversation
//! (the customer's messages, the tools on offer, the tool results so far) and does what a
//! careful model would do next:
//!
//! - **Exits first.** A request for a person or a legal threat → `escalate_to_human`; an
//!   injection attempt → `escalate_to_human`; a request to *do* something (cancel, refund,
//!   double charge, "where is my order") → `reroute` with the matching intent.
//! - **`ProductAdvisor`** (has `search_products`): searches every product the customer names,
//!   then for a compatibility question calls `check_compatibility` on what it found, and on an
//!   `incompatible` verdict `find_alternatives` for the part the customer owns ("my …") in a
//!   similar price band (and `get_product` when the alternatives come back as bare IDs). The
//!   answer quotes names, prices and issue messages from the tool results only, so the
//!   grounding and compatibility-claim guardrails pass.
//! - **`OrderAssistant`** (has `get_order` / `list_my_orders` / `list_policies`): looks up the
//!   order named (or the only order on the account, else asks for the number), its tracking or
//!   invoices when asked, and the matching policy section for policy questions.
//!
//! # Tool contract
//!
//! The mock calls tools by the names in support.md §5 and reads their parameter names from the
//! schemas it is given, trying the usual spellings (`text`/`query`, `product_ids`/`parts`,
//! `order_number`, `slug`, `replace_category`/`category`, `price_band`). It reads results
//! loosely: the first array of rows anywhere in the result; rows with `id`, `name`, `price`
//! (cents, like `Money`) or `price_cents`, `category`; a compat report with `verdict` and
//! `issues[].message`/`severity`; orders with `number`, `status`, `total`, `items[].name`;
//! documents with `slug`, `title`, `content`. Tool errors arrive as `{ "error": … }`.

use std::collections::BTreeSet;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use techdepo_domain::{Intent, Money};
use techdepo_ports::{
    ChatMessage, ChatRequest, ChatResponse, FinishReason, Llm, LlmError, ToolCall, Usage,
};

use super::estimate_tokens;
use super::text::{any, is_injection, normalize, order_numbers, product_mentions, sentences};

/// The scripted agent model for `LLM_BACKEND=mock`.
#[derive(Debug, Clone, Copy, Default)]
pub struct MockLlm;

impl MockLlm {
    /// The model name it reports.
    pub const MODEL: &'static str = "mock-agent";
}

#[async_trait]
impl Llm for MockLlm {
    fn default_model(&self) -> &str {
        Self::MODEL
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        if request.messages.is_empty() {
            return Err(LlmError::InvalidRequest("no messages".into()));
        }
        let conversation = Conversation::read(request);
        let action = decide(&conversation);
        let input_chars = serde_json::to_string(&request.messages).map_or(0, |s| s.len());
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| Self::MODEL.to_owned());
        Ok(match action {
            Action::Reply(text) => ChatResponse {
                model,
                usage: Usage {
                    input_tokens: estimate_tokens(input_chars),
                    output_tokens: estimate_tokens(text.len()),
                },
                text: Some(text),
                tool_calls: vec![],
                finish_reason: FinishReason::Stop,
            },
            Action::Calls(calls) => {
                let tool_calls: Vec<ToolCall> = calls
                    .into_iter()
                    .enumerate()
                    .map(|(i, (name, arguments))| ToolCall {
                        id: format!("mock-{}-{i}", conversation.turn),
                        name,
                        arguments,
                    })
                    .collect();
                let output_chars = serde_json::to_string(&tool_calls).map_or(0, |s| s.len());
                ChatResponse {
                    model,
                    text: None,
                    tool_calls,
                    usage: Usage {
                        input_tokens: estimate_tokens(input_chars),
                        output_tokens: estimate_tokens(output_chars),
                    },
                    finish_reason: FinishReason::ToolCalls,
                }
            }
        })
    }
}

/// The next move.
enum Action {
    Reply(String),
    Calls(Vec<(String, Value)>),
}

fn call(name: &str, args: Value) -> Action {
    Action::Calls(vec![(name.to_owned(), args)])
}

fn escalate(reason: &str) -> Action {
    call("escalate_to_human", json!({ "reason": reason }))
}

fn ask(question: &str, expects: &str) -> Action {
    call(
        "ask_customer",
        json!({ "question": question, "expects": expects }),
    )
}

/// A tool call the model made and what came back.
struct ToolResult {
    name: String,
    args: Value,
    value: Value,
    is_error: bool,
}

/// The conversation as the mock reads it.
struct Conversation<'a> {
    request: &'a ChatRequest,
    /// The customer's messages, joined.
    question: String,
    /// Tool calls and their results, in order.
    results: Vec<ToolResult>,
    /// Model turns so far (for unique call IDs).
    turn: usize,
}

impl<'a> Conversation<'a> {
    fn read(request: &'a ChatRequest) -> Self {
        let mut question = Vec::new();
        let mut pending: Vec<&ToolCall> = Vec::new();
        let mut results = Vec::new();
        let mut turn = 0;
        for message in &request.messages {
            match message {
                ChatMessage::System { .. } => {}
                ChatMessage::User { content } => question.push(content.as_str()),
                ChatMessage::Assistant { tool_calls, .. } => {
                    turn += 1;
                    pending.extend(tool_calls);
                }
                ChatMessage::Tool {
                    tool_call_id,
                    name,
                    content,
                } => {
                    let args = pending
                        .iter()
                        .find(|c| &c.id == tool_call_id)
                        .map(|c| c.arguments.clone())
                        .unwrap_or(Value::Null);
                    let value = serde_json::from_str(content)
                        .unwrap_or_else(|_| Value::String(content.clone()));
                    let is_error = value.get("error").is_some();
                    results.push(ToolResult {
                        name: name.clone(),
                        args,
                        value,
                        is_error,
                    });
                }
            }
        }
        Self {
            request,
            question: question.join("\n"),
            results,
            turn,
        }
    }

    fn has_tool(&self, name: &str) -> bool {
        self.request.tools.iter().any(|t| t.name == name)
    }

    /// The parameter of `tool` called one of `candidates` (the first spelling if none is).
    fn param(&self, tool: &str, candidates: &[&str]) -> String {
        let properties = self
            .request
            .tools
            .iter()
            .find(|t| t.name == tool)
            .and_then(|t| t.parameters.get("properties"))
            .and_then(Value::as_object);
        candidates
            .iter()
            .find(|c| properties.is_some_and(|p| p.contains_key(**c)))
            .or(candidates.first())
            .map(|c| (*c).to_owned())
            .unwrap_or_default()
    }

    /// The schema of parameter `param` of `tool`.
    fn param_schema(&self, tool: &str, param: &str) -> Option<&Value> {
        self.request
            .tools
            .iter()
            .find(|t| t.name == tool)?
            .parameters
            .get("properties")?
            .get(param)
    }

    fn last(&self, tool: &str) -> Option<&ToolResult> {
        self.results
            .iter()
            .rev()
            .find(|r| r.name == tool && !r.is_error)
    }

    /// The successful result of `tool` called with `args[key] == value`.
    fn result_for(&self, tool: &str, key: &str, value: &str) -> Option<&ToolResult> {
        self.results
            .iter()
            .find(|r| r.name == tool && r.args.get(key).and_then(Value::as_str) == Some(value))
    }
}

fn decide(c: &Conversation<'_>) -> Action {
    let q = normalize(&c.question);

    if any(
        &q,
        &[
            "lawyer",
            "attorney",
            "legal action",
            "sue you",
            "court",
            "real person",
            "talk to a human",
            "speak to a human",
            "talk to someone",
            "speak to someone",
            "manager",
        ],
    ) {
        return escalate("The customer asks for a person or raises a legal matter.");
    }
    if is_injection(&c.question) {
        return escalate(
            "The message tries to instruct the assistant or asks about another customer's order.",
        );
    }
    if c.has_tool("reroute")
        && let Some(intent) = action_request(&q)
    {
        return call("reroute", json!({ "intent": intent }));
    }
    if let Some(failed) = c.results.last().filter(|r| r.is_error) {
        let message = failed
            .value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        if failed.name.contains("order") || failed.name.contains("tracking") {
            return ask(
                "I couldn't find that order on your account. Could you send me the order \
                 number (it looks like TD-1042)? You can find it under My orders.",
                "order_number",
            );
        }
        return escalate(&format!("The `{}` tool failed: {message}", failed.name));
    }

    if c.has_tool("search_products") || c.has_tool("check_compatibility") {
        advisor(c, &q)
    } else if c.has_tool("get_order") || c.has_tool("list_my_orders") || c.has_tool("list_policies")
    {
        assistant(c, &q)
    } else {
        Action::Reply(
            "Thank you for your message. We have looked into it and will follow up if we \
             need anything else from you."
                .to_owned(),
        )
    }
}

/// The intent of a request to *do* something, which agents cannot do. Questions about how
/// things work ("can I cancel?") are answered instead.
fn action_request(q: &str) -> Option<&'static str> {
    let asks_how = any(
        q,
        &[
            "can i",
            "could i",
            "is it possible",
            "how do i",
            "how can i",
            "policy",
            "how long",
            "how many days",
        ],
    );
    if asks_how {
        return None;
    }
    let intent = if any(
        q,
        &[
            "charged twice",
            "charged me twice",
            "double charge",
            "duplicate charge",
            "wrong amount",
        ],
    ) {
        Intent::BillingProblem
    } else if any(q, &["cancel", "stornieren", "lemond"]) {
        Intent::CancelOrder
    } else if any(
        q,
        &[
            "refund",
            "money back",
            "return it",
            "return my",
            "send it back",
            "want to return",
        ],
    ) {
        Intent::RefundOrReturn
    } else if any(
        q,
        &[
            "where is my",
            "where's my",
            "hasn't arrived",
            "has not arrived",
            "not arrived",
            "not received",
        ],
    ) {
        Intent::OrderStatus
    } else {
        return None;
    };
    Some(intent.as_str())
}

// ---------------------------------------------------------------------------------------------
// ProductAdvisor
// ---------------------------------------------------------------------------------------------

/// A catalog row from a tool result.
#[derive(Debug, Clone)]
struct Product {
    id: String,
    name: String,
    price: Option<Money>,
    category: Option<String>,
}

impl Product {
    fn from_row(row: &Map<String, Value>) -> Option<Self> {
        let id = row
            .get("id")
            .or_else(|| row.get("product_id"))
            .and_then(Value::as_str)?
            .to_owned();
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_owned();
        Some(Self {
            id,
            name,
            price: price(row),
            category: row
                .get("category")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        })
    }

    fn with_price(&self) -> String {
        match self.price {
            Some(price) => format!("{} ({price})", self.name),
            None => self.name.clone(),
        }
    }
}

fn price(row: &Map<String, Value>) -> Option<Money> {
    money(row.get("price_cents").or_else(|| row.get("price"))?)
}

/// An amount: cents (how `Money` serializes), `{ "cents": … }`, or text such as `€84.90`.
fn money(value: &Value) -> Option<Money> {
    match value {
        Value::Number(n) => n.as_i64().map(Money::from_cents),
        Value::String(s) => {
            let digits: String = s
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            let (euros, cents) = digits.split_once('.').unwrap_or((&digits, "0"));
            let euros: i64 = euros.parse().ok()?;
            let cents: i64 = format!("{cents:0<2}").get(..2)?.parse().ok()?;
            Some(Money::from_cents(euros * 100 + cents))
        }
        Value::Object(o) => o
            .get("cents")
            .and_then(Value::as_i64)
            .map(Money::from_cents),
        _ => None,
    }
}

/// The first array of rows anywhere in `value` (breadth-first), e.g. `[…]`,
/// `{ "products": […] }` or `{ "fixes": [{ "candidates": […] }] }`.
fn find_array(value: &Value) -> Option<&Vec<Value>> {
    let mut queue = vec![value];
    while !queue.is_empty() {
        let mut next = Vec::new();
        for v in queue {
            match v {
                Value::Array(items) if items.iter().any(|i| i.is_object() || i.is_string()) => {
                    // An array of wrapper objects without IDs (fixes) is looked into.
                    if items.iter().any(|i| {
                        i.is_string()
                            || i.get("id").is_some()
                            || i.get("product_id").is_some()
                            || i.get("slug").is_some()
                            || i.get("number").is_some()
                            || i.get("order_number").is_some()
                    }) {
                        return Some(items);
                    }
                    next.extend(items);
                }
                Value::Object(map) => next.extend(map.values()),
                _ => {}
            }
        }
        queue = next;
    }
    None
}

fn products(value: &Value) -> Vec<Product> {
    find_array(value)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_object)
                .filter_map(Product::from_row)
                .collect()
        })
        .unwrap_or_default()
}

fn advisor(c: &Conversation<'_>, q: &str) -> Action {
    let mentions = product_mentions(&c.question);
    let compat = any(
        q,
        &[
            "compatible",
            "compatibility",
            "work with",
            "works with",
            "fit",
            "go with",
            "pair with",
            "support",
        ],
    );
    let search_key = c.param("search_products", &["text", "query", "q", "name"]);

    // 1. Look up every product the customer names.
    let pending: Vec<&String> = mentions
        .iter()
        .filter(|m| c.result_for("search_products", &search_key, m).is_none())
        .collect();
    if !pending.is_empty() && c.has_tool("search_products") {
        return Action::Calls(
            pending
                .into_iter()
                .map(|m| {
                    (
                        "search_products".to_owned(),
                        json!({ search_key.clone(): m }),
                    )
                })
                .collect(),
        );
    }

    if mentions.is_empty() {
        return browse(c, q, &search_key);
    }

    let found: Vec<(&String, Option<Product>)> = mentions
        .iter()
        .map(|m| {
            let first = c
                .result_for("search_products", &search_key, m)
                .and_then(|r| products(&r.value).into_iter().next());
            (m, first)
        })
        .collect();
    if let Some((missing, _)) = found.iter().find(|(_, p)| p.is_none()) {
        return ask(
            &format!(
                "I couldn't find \"{missing}\" in our catalog. Could you send me the exact \
                 product name, or a link to it in our shop?"
            ),
            "clarification",
        );
    }
    let found: Vec<(&String, Product)> = found
        .into_iter()
        .filter_map(|(m, p)| p.map(|p| (m, p)))
        .collect();

    if !compat {
        return describe(c, &found);
    }
    if found.len() < 2 {
        return ask(
            "Which other part should I check it against? Please send me its exact name.",
            "clarification",
        );
    }
    compatibility(c, q, &found)
}

/// No product named: search by category, or ask what they are looking for.
fn browse(c: &Conversation<'_>, q: &str, search_key: &str) -> Action {
    let category = [
        ("memory", &["ram", "memory", "ddr4", "ddr5"][..]),
        ("cpu", &["cpu", "processor"][..]),
        ("gpu", &["gpu", "graphics card", "video card"][..]),
        ("motherboard", &["motherboard", "mainboard", "board"][..]),
        ("psu", &["psu", "power supply"][..]),
        ("cooler", &["cooler", "cooling"][..]),
        ("storage", &["ssd", "nvme", "storage", "hard drive"][..]),
        ("case", &["case", "chassis"][..]),
    ]
    .into_iter()
    .find(|(_, words)| any(q, words))
    .map(|(category, _)| category);

    let Some(category) = category else {
        return ask(
            "Happy to help! Which kind of part are you looking for, and roughly what budget \
             do you have in mind?",
            "clarification",
        );
    };
    let Some(result) = c.last("search_products") else {
        let key = c.param("search_products", &["category", "text"]);
        let mut args = json!({ key.clone(): category });
        if key != search_key && q.contains("ddr5") {
            args[search_key] = json!("DDR5");
        }
        return call("search_products", args);
    };
    let rows = products(&result.value);
    if rows.is_empty() {
        return ask(
            "I couldn't find a matching product. Could you tell me more about what you need?",
            "clarification",
        );
    }
    let list: Vec<String> = rows.iter().take(3).map(Product::with_price).collect();
    Action::Reply(format!(
        "Here are some options from our catalog: {}. Let me know if you would like me to check \
         any of them against the rest of your build.",
        list.join("; ")
    ))
}

/// A question about products, not their compatibility: details of the first one, if a tool
/// gives them.
fn describe(c: &Conversation<'_>, found: &[(&String, Product)]) -> Action {
    let key = c.param("get_product", &["id", "product_id"]);
    let first = &found[0].1;
    if c.has_tool("get_product") && c.result_for("get_product", &key, &first.id).is_none() {
        return call("get_product", json!({ key: first.id }));
    }
    let summary = c
        .result_for("get_product", &key, &first.id)
        .and_then(|r| {
            r.value
                .get("summary")
                .or_else(|| r.value.get("description"))
                .and_then(Value::as_str)
        })
        .map(|s| format!(" {s}"))
        .unwrap_or_default();
    let others: Vec<String> = found[1..].iter().map(|(_, p)| p.with_price()).collect();
    let also = if others.is_empty() {
        String::new()
    } else {
        format!(" I also found: {}.", others.join("; "))
    };
    Action::Reply(format!(
        "The {} is in our catalog.{summary}{also}",
        first.with_price()
    ))
}

fn compatibility(c: &Conversation<'_>, q: &str, found: &[(&String, Product)]) -> Action {
    let ids: Vec<&str> = found.iter().map(|(_, p)| p.id.as_str()).collect();
    let Some(report) = c.last("check_compatibility") else {
        let key = c.param(
            "check_compatibility",
            &["product_ids", "ids", "parts", "products"],
        );
        return call("check_compatibility", json!({ key: ids }));
    };
    let report = &report.value;
    let verdict = report
        .get("verdict")
        .and_then(Value::as_str)
        .unwrap_or("compatible");
    let issues = |severity: &str| -> Vec<String> {
        report
            .get("issues")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|i| {
                i.get("severity")
                    .and_then(Value::as_str)
                    .is_none_or(|s| s == severity)
            })
            .filter_map(|i| i.get("message").and_then(Value::as_str))
            .map(|m| m.trim_end_matches('.').to_owned())
            .collect()
    };
    let names = found
        .iter()
        .map(|(_, p)| format!("the {}", p.name))
        .collect::<Vec<_>>()
        .join(" and ");

    match verdict {
        "incompatible" => {
            let errors = issues("error");
            let why = if errors.is_empty() {
                String::new()
            } else {
                format!(": {}", errors.join("; "))
            };
            let mut reply = format!("Unfortunately, {names} won't work together{why}.");
            match alternative(c, q, found) {
                Ok(Some((replaced, kept, alt))) => reply.push_str(&format!(
                    " If you'd like to keep the {}, the {} would replace the {} and fits a \
                     similar budget.",
                    kept.name,
                    alt.with_price(),
                    replaced.name
                )),
                Ok(None) => {}
                Err(next) => return next,
            }
            reply.push_str(" I checked this with our compatibility checker.");
            Action::Reply(reply)
        }
        "warnings" => Action::Reply(format!(
            "Yes, {names} work together, with one thing to keep in mind: {}. I checked this \
             with our compatibility checker.",
            issues("warning").join("; ")
        )),
        _ => Action::Reply(format!(
            "Yes, {names} are compatible: our compatibility checker found no problems with \
             this combination."
        )),
    }
}

/// The part to replace (the one the customer calls "my …", else the last one named), the part
/// kept, and a compatible alternative. `Err` is the tool call that gets it.
fn alternative<'p>(
    c: &Conversation<'_>,
    q: &str,
    found: &'p [(&String, Product)],
) -> Result<Option<(&'p Product, &'p Product, Product)>, Action> {
    if !c.has_tool("find_alternatives") {
        return Ok(None);
    }
    let replace_at = found
        .iter()
        .position(|(m, _)| q.contains(&format!("my {}", m.to_lowercase())))
        .unwrap_or(found.len() - 1);
    let replaced = &found[replace_at].1;
    let kept = &found[if replace_at == 0 { 1 } else { 0 }].1;

    let Some(result) = c.last("find_alternatives") else {
        return Err(call(
            "find_alternatives",
            alternatives_args(c, found, replaced),
        ));
    };
    let candidates = products(&result.value);
    let first = match candidates.into_iter().next() {
        Some(p) if p.name != p.id => p,
        Some(bare) => {
            // Bare IDs: look the first one up.
            let key = c.param("get_product", &["id", "product_id"]);
            match c.result_for("get_product", &key, &bare.id) {
                Some(r) => r
                    .value
                    .as_object()
                    .and_then(Product::from_row)
                    .unwrap_or(bare),
                None if c.has_tool("get_product") => {
                    return Err(call("get_product", json!({ key: bare.id })));
                }
                None => return Ok(None),
            }
        }
        None => {
            let ids = find_array(&result.value)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let Some(id) = ids else { return Ok(None) };
            let key = c.param("get_product", &["id", "product_id"]);
            match c.result_for("get_product", &key, &id) {
                Some(r) => match r.value.as_object().and_then(Product::from_row) {
                    Some(p) => p,
                    None => return Ok(None),
                },
                None if c.has_tool("get_product") => {
                    return Err(call("get_product", json!({ key: id })));
                }
                None => return Ok(None),
            }
        }
    };
    Ok(Some((replaced, kept, first)))
}

fn alternatives_args(
    c: &Conversation<'_>,
    found: &[(&String, Product)],
    replaced: &Product,
) -> Value {
    let tool = "find_alternatives";
    let parts = c.param(tool, &["parts", "product_ids", "ids", "build"]);
    let category = c.param(tool, &["replace_category", "category", "slot", "replace"]);
    let band = c.param(tool, &["price_band", "price_range", "band"]);
    let ids: Vec<&str> = found.iter().map(|(_, p)| p.id.as_str()).collect();
    let mut args = json!({
        parts: ids,
        category: replaced.category.clone().unwrap_or_else(|| replaced.id.clone()),
    });
    let schema = c.param_schema(tool, &band);
    let kind = schema.and_then(|s| s.get("type")).and_then(Value::as_str);
    let price = replaced.price.map_or(0, Money::cents);
    let band_value =
        if let Some(options) = schema.and_then(|s| s.get("enum")).and_then(Value::as_array) {
            options
                .iter()
                .find(|o| {
                    o.as_str()
                        .is_some_and(|s| s.contains("same") || s.contains("similar"))
                })
                .or(options.first())
                .cloned()
        } else {
            match kind {
                Some("integer" | "number") => Some(json!(25)),
                Some("string") => Some(json!("similar")),
                Some("object") | None => {
                    let props = schema
                        .and_then(|s| s.get("properties"))
                        .and_then(Value::as_object);
                    let key = |candidates: &[&str]| {
                        candidates
                            .iter()
                            .find(|k| props.is_some_and(|p| p.contains_key(**k)))
                            .or(candidates.first())
                            .map(|k| (*k).to_owned())
                            .unwrap_or_default()
                    };
                    (price > 0).then(|| {
                        json!({
                            key(&["min_cents", "min", "price_min"]): price * 3 / 4,
                            key(&["max_cents", "max", "price_max"]): price * 5 / 4,
                        })
                    })
                }
                Some(_) => None,
            }
        };
    if let (Some(value), Value::Object(map)) = (band_value, &mut args) {
        map.insert(band, value);
    }
    args
}

// ---------------------------------------------------------------------------------------------
// OrderAssistant
// ---------------------------------------------------------------------------------------------

const POLICY_WORDS: &[&str] = &[
    "ship to",
    "do you ship",
    "deliver to",
    "delivery address",
    "change the address",
    "change my address",
    "return",
    "warranty",
    "refund",
    "cancel",
    "payment",
    "pay by",
    "invoice",
    "vat",
    "policy",
    "how long",
    "shipping",
];

fn assistant(c: &Conversation<'_>, q: &str) -> Action {
    let mut numbers = order_numbers(&c.question);
    let about_order = !numbers.is_empty()
        || any(
            q,
            &[
                "my order",
                "my invoice",
                "my parcel",
                "my package",
                "my delivery",
                "my last order",
            ],
        );
    let about_policy = any(q, POLICY_WORDS);
    let mut parts = Vec::new();

    if about_order && c.has_tool("get_order") {
        if numbers.is_empty() {
            match only_order(c) {
                Ok(number) => numbers.push(number),
                Err(next) => return next,
            }
        }
        let number = &numbers[0];
        let key = c.param("get_order", &["order_number", "number", "order"]);
        let mut calls = Vec::new();
        if c.result_for("get_order", &key, number).is_none() {
            calls.push(("get_order".to_owned(), json!({ key.clone(): number })));
        }
        let wants_tracking = any(
            q,
            &["track", "where", "arriv", "deliver", "shipped", "parcel"],
        );
        if wants_tracking && c.has_tool("get_tracking") {
            let key = c.param("get_tracking", &["order_number", "number", "order"]);
            if c.result_for("get_tracking", &key, number).is_none() {
                calls.push(("get_tracking".to_owned(), json!({ key: number })));
            }
        }
        if q.contains("invoice") && c.has_tool("get_invoices") {
            let key = c.param("get_invoices", &["order_number", "number", "order"]);
            if c.result_for("get_invoices", &key, number).is_none() {
                calls.push(("get_invoices".to_owned(), json!({ key: number })));
            }
        }
        if !calls.is_empty() {
            return Action::Calls(calls);
        }
        parts.push(order_summary(c, number));
    }

    if (about_policy || parts.is_empty()) && c.has_tool("list_policies") {
        match policy(c, q) {
            Ok(Some(text)) => parts.push(text),
            Ok(None) => {}
            Err(next) => return next,
        }
    }

    if parts.is_empty() {
        return escalate(
            "I could not find anything in the order data or policies that answers this.",
        );
    }
    Action::Reply(parts.join("\n\n"))
}

/// The customer's only order, or the call / question that gets one.
fn only_order(c: &Conversation<'_>) -> Result<String, Action> {
    if !c.has_tool("list_my_orders") {
        return Err(ask_order_number());
    }
    let Some(result) = c.last("list_my_orders") else {
        return Err(call("list_my_orders", json!({})));
    };
    let orders: Vec<String> = find_array(&result.value)
        .into_iter()
        .flatten()
        .filter_map(|o| o.get("number").or_else(|| o.get("order_number")))
        .filter_map(|n| match n {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(format!("TD-{n}")),
            _ => None,
        })
        .collect();
    match orders.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(Action::Reply(
            "I couldn't find any orders on your account yet. If you ordered with a different \
             email address, please reply from that one."
                .to_owned(),
        )),
        _ => Err(ask_order_number()),
    }
}

fn ask_order_number() -> Action {
    ask(
        "Which order do you mean? Please send me the order number (it looks like TD-1042); \
         you can find it under My orders.",
        "order_number",
    )
}

fn order_summary(c: &Conversation<'_>, number: &str) -> String {
    let key = c.param("get_order", &["order_number", "number", "order"]);
    let order = c.result_for("get_order", &key, number).map(|r| &r.value);
    let field = |v: Option<&Value>, keys: &[&str]| -> Option<String> {
        let v = v?;
        keys.iter().find_map(|k| match v.get(*k)? {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
    };
    let status = field(order, &["status_label", "status"])
        .map(|s| s.replace('_', " "))
        .unwrap_or_else(|| "being processed".to_owned());
    let mut out = format!("Your order {number} is currently {status}.");
    if let Some(total) = order
        .and_then(|o| o.get("total_cents").or_else(|| o.get("total")))
        .and_then(money)
    {
        out.push_str(&format!(" The total was {total}."));
    }
    let items: Vec<String> = order
        .and_then(|o| o.get("items").or_else(|| o.get("lines")))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|i| {
            i.get("name")
                .or_else(|| i.get("product_name"))
                .and_then(Value::as_str)
        })
        .map(ToOwned::to_owned)
        .collect();
    if !items.is_empty() {
        out.push_str(&format!(" It contains: {}.", items.join(", ")));
    }
    let tracking_key = c.param("get_tracking", &["order_number", "number", "order"]);
    if let Some(tracking) = c
        .result_for("get_tracking", &tracking_key, number)
        .map(|r| &r.value)
    {
        let state = field(Some(tracking), &["status_label", "status", "state"]);
        let eta = field(Some(tracking), &["eta", "estimated_delivery", "expected"]);
        match (state, eta) {
            (Some(state), Some(eta)) => out.push_str(&format!(
                " The carrier reports: {}; expected delivery {eta}.",
                state.replace('_', " ")
            )),
            (Some(state), None) => {
                out.push_str(&format!(
                    " The carrier reports: {}.",
                    state.replace('_', " ")
                ));
            }
            _ => {}
        }
    }
    let invoices_key = c.param("get_invoices", &["order_number", "number", "order"]);
    if let Some(invoices) = c.result_for("get_invoices", &invoices_key, number) {
        let numbers: Vec<String> = find_array(&invoices.value)
            .into_iter()
            .flatten()
            .filter_map(|i| i.get("number").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect();
        if numbers.is_empty() {
            out.push_str(" There is no invoice for it yet.");
        } else {
            out.push_str(&format!(
                " Its invoice: {}; you can download it from the order page.",
                numbers.join(", ")
            ));
        }
    }
    out
}

/// The relevant policy passage, or the call that gets it.
fn policy(c: &Conversation<'_>, q: &str) -> Result<Option<String>, Action> {
    let Some(list) = c.last("list_policies") else {
        return Err(call("list_policies", json!({})));
    };
    let words = keywords(q);
    let best = find_array(&list.value)
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let slug = p.get("slug").and_then(Value::as_str)?;
            let about = format!(
                "{slug} {} {}",
                p.get("title").and_then(Value::as_str).unwrap_or_default(),
                p.get("summary").and_then(Value::as_str).unwrap_or_default()
            );
            Some((overlap(&words, &about), slug))
        })
        .filter(|(score, _)| *score > 0)
        .max_by_key(|(score, _)| *score)
        .map(|(_, slug)| slug.to_owned());
    let Some(slug) = best else { return Ok(None) };

    let key = c.param("read_policy", &["slug", "name", "id"]);
    let Some(doc) = c.result_for("read_policy", &key, &slug) else {
        return Err(call("read_policy", json!({ key: slug })));
    };
    let content = doc
        .value
        .get("content")
        .or_else(|| doc.value.get("body"))
        .and_then(Value::as_str)
        .or_else(|| doc.value.as_str())
        .unwrap_or_default();
    let title = doc
        .value
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(&slug);

    // The section whose heading and text share the most words with the question.
    let section = content
        .split("\n## ")
        .skip(1)
        .max_by_key(|s| {
            overlap(&words, s) * 2 + overlap(&words, s.lines().next().unwrap_or_default()) * 3
        })
        .unwrap_or(content);
    let body: String = section
        .lines()
        .skip_while(|l| !l.trim().is_empty())
        .filter(|l| !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ");
    let passage: Vec<&str> = sentences(&body).into_iter().take(3).collect();
    if passage.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "From our {} policy: {}",
        title.to_lowercase(),
        passage.join(" ")
    )))
}

fn keywords(q: &str) -> BTreeSet<String> {
    const STOP: &[&str] = &[
        "what", "when", "where", "which", "with", "your", "have", "this", "that", "there", "about",
        "would", "could", "should", "does", "please", "thanks", "order", "hello",
    ];
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 4 && !STOP.contains(w))
        .map(|w| w.trim_end_matches('s').to_owned())
        .collect()
}

fn overlap(words: &BTreeSet<String>, text: &str) -> usize {
    let text = normalize(text);
    words.iter().filter(|w| text.contains(w.as_str())).count()
}

#[cfg(test)]
mod tests;
