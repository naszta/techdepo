//! The minimal durable agent loop (support.md §5).
//!
//! An agent is a system prompt, a [`ToolSet`] and a step limit ([`AgentDef`]). [`run`] calls the
//! model, runs the tools it asks for, feeds the results back, and stops when the model answers in
//! plain text ([`Outcome::Reply`]) or calls a terminal tool (`ask_customer`,
//! `escalate_to_human`, `reroute`). Anything that goes wrong (step limit, budget, a model that
//! keeps failing, a fatal tool error) ends in [`Outcome::Escalate`], never in an error: the
//! ticket always gets a next step.
//!
//! Agents never act. They return an [`Outcome`], and deterministic code (the ticket, the
//! guardrails, the reply policy) decides what happens with it.
//!
//! # Durability without Restate types
//!
//! The crate does not depend on `restate-sdk`. Each model call runs through a caller-provided
//! [`Runtime`], whose [`Runtime::step`] is one journaled step: in the services it is
//! `ctx.run(..).name(name)`, so on retry or replay the model is not called again and its cost
//! is paid once. [`Inline`] runs steps directly (unit tests, evals). This keeps the loop natively
//! testable with `ports::testing::ScriptedLlm`, and the services free to journal however they
//! like.
//!
//! Tools are generic over a caller-defined context (`Tool<Cx>`), so the services' tools can
//! hold a `&Context` and the tenant scope (support.md: "tenant scoping comes from code, never
//! from tool arguments"). A tool decides its own durability: read-only tools that touch D1 wrap
//! their work in `ctx.run`, pure tools (the compat engine, the compiled-in catalog) do not.
//!
//! # Budget
//!
//! Before every model call the loop asks a [`Budget`] (per-org daily tokens, `llm_usage`); the
//! check, the call and the usage record are one journaled step. [`Unlimited`] skips it.

mod budget;
mod run;
mod runtime;
mod terminal;
mod tool;
mod trace;

pub use budget::{Budget, BudgetCheck, Unlimited};
pub use run::{AgentDef, AgentEnv, AgentRun, DEFAULT_MAX_STEPS, run};
pub use runtime::{Inline, Runtime, StepError, StepFn, StepFuture};
pub use terminal::{
    ASK_CUSTOMER, AskCustomerArgs, ESCALATE_TO_HUMAN, EscalateArgs, Expects, REROUTE, RerouteArgs,
    is_terminal, terminal_definitions,
};
pub use tool::{Tool, ToolError, ToolSet, schema_for};
pub use trace::{Trace, TraceStep};

/// What an agent decided: [`techdepo_api_types::support::AgentOutcome`] (stored in
/// `agent_runs.outcome_json`).
pub use techdepo_api_types::support::AgentOutcome as Outcome;
