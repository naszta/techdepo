//! Terminal tools: calling one ends the run with an [`Outcome`] other than a reply.
//!
//! Workers AI takes no `tool_choice`, so the model cannot be forced to call anything; a plain
//! text answer is the natural way to finish (`Reply`), and these three tools are how it says
//! anything else.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use techdepo_api_types::vocab::Awaiting;
use techdepo_domain::Intent;
use techdepo_ports::ToolDefinition;

use crate::Outcome;
use crate::tool::{ToolError, parse_args, schema_for};

/// `ask_customer`.
pub const ASK_CUSTOMER: &str = "ask_customer";
/// `escalate_to_human`.
pub const ESCALATE_TO_HUMAN: &str = "escalate_to_human";
/// `reroute`.
pub const REROUTE: &str = "reroute";

/// What the agent needs from the customer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Expects {
    /// An order number such as TD-1042.
    OrderNumber,
    /// Any other missing detail or clarification.
    Clarification,
}

impl From<Expects> for Awaiting {
    fn from(value: Expects) -> Self {
        match value {
            Expects::OrderNumber => Self::OrderNumber,
            Expects::Clarification => Self::Clarification,
        }
    }
}

/// Arguments of `ask_customer`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
pub struct AskCustomerArgs {
    /// The question to email the customer, written to them directly.
    pub question: String,
    /// What the answer should provide.
    pub expects: Expects,
}

/// Arguments of `escalate_to_human`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
pub struct EscalateArgs {
    /// Why a person must handle this, for the staff member who picks it up.
    pub reason: String,
}

/// Arguments of `reroute`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
pub struct RerouteArgs {
    /// What the customer actually wants.
    pub intent: Intent,
}

/// Whether `name` is a terminal tool.
pub fn is_terminal(name: &str) -> bool {
    matches!(name, ASK_CUSTOMER | ESCALATE_TO_HUMAN | REROUTE)
}

/// The terminal tools' definitions, appended to every agent's tools.
pub fn terminal_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: ASK_CUSTOMER.to_owned(),
            description: "Ask the customer for missing information and wait for their reply. \
                Use it only when you cannot answer without it."
                .to_owned(),
            parameters: schema_for::<AskCustomerArgs>(),
        },
        ToolDefinition {
            name: ESCALATE_TO_HUMAN.to_owned(),
            description: "Hand the conversation to a staff member: the request is out of \
                scope, needs a decision you cannot make, or the customer asks for a person."
                .to_owned(),
            parameters: schema_for::<EscalateArgs>(),
        },
        ToolDefinition {
            name: REROUTE.to_owned(),
            description: "The customer wants something done that another team handles (cancel \
                an order, a refund or return, a billing problem, order status). You cannot do \
                it yourself; reroute instead of promising anything."
                .to_owned(),
            parameters: schema_for::<RerouteArgs>(),
        },
    ]
}

/// The outcome of terminal tool `name`, or why its arguments are unusable.
pub(crate) fn outcome(name: &str, args: Value) -> Result<Outcome, ToolError> {
    match name {
        ASK_CUSTOMER => {
            let args: AskCustomerArgs = parse_args(args)?;
            non_empty(&args.question, "question")?;
            Ok(Outcome::AskCustomer {
                question: args.question.trim().to_owned(),
                expects: args.expects.into(),
            })
        }
        ESCALATE_TO_HUMAN => {
            let args: EscalateArgs = parse_args(args)?;
            non_empty(&args.reason, "reason")?;
            Ok(Outcome::Escalate {
                reason: args.reason.trim().to_owned(),
            })
        }
        REROUTE => {
            let args: RerouteArgs = parse_args(args)?;
            Ok(Outcome::Reroute {
                intent: args.intent,
            })
        }
        other => Err(ToolError::InvalidArguments(format!(
            "`{other}` is not a terminal tool"
        ))),
    }
}

fn non_empty(value: &str, field: &str) -> Result<(), ToolError> {
    if value.trim().is_empty() {
        return Err(ToolError::InvalidArguments(format!("`{field}` is empty")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_terminal_calls() {
        assert_eq!(
            outcome(
                ASK_CUSTOMER,
                json!({ "question": " Which order? ", "expects": "order_number" })
            ),
            Ok(Outcome::AskCustomer {
                question: "Which order?".into(),
                expects: Awaiting::OrderNumber
            })
        );
        assert_eq!(
            outcome(REROUTE, json!({ "intent": "cancel_order" })),
            Ok(Outcome::Reroute {
                intent: Intent::CancelOrder
            })
        );
        assert!(outcome(REROUTE, json!({ "intent": "launch_rockets" })).is_err());
        assert!(outcome(ESCALATE_TO_HUMAN, json!({ "reason": "  " })).is_err());
        assert!(
            outcome(
                ASK_CUSTOMER,
                json!({ "question": "?", "expects": "offer_reply" })
            )
            .is_err(),
            "agents cannot wait on offers"
        );

        let defs = terminal_definitions();
        assert!(defs.iter().all(|d| is_terminal(&d.name)));
        let intents = &defs[2].parameters["properties"]["intent"];
        assert!(intents.to_string().contains("cancel_order"), "{intents}");
    }
}
