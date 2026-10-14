//! The models behind `ports::Llm` and `ports::Classifier` (architecture.md §6).
//!
//! | Backend (`LLM_BACKEND`) | Chat (agents) | Classification (triage, offers, judge) |
//! |---|---|---|
//! | `workers-ai` | [`AllamaLlm`] over allama-cloudflare-worker (`@cf/meta/llama-3.3-70b-instruct-fp8-fast`) | [`DecisionClassifier`] on Clef, falling back to [`StructuredClassifier`] (JSON mode) |
//! | `mock` | [`MockLlm`]: scripted agent behaviour | [`MockClassifier`]: keywords → calibrated-looking probabilities |
//!
//! Everything here talks to allama through its provider traits (`ChatProvider`,
//! `DecisionProvider`), so the same adapters run on Workers AI through the AI binding (wasm,
//! [`LlmConfig::build`]), on Workers AI over HTTP for native evals (allama-openai-compat and
//! allama-systemone, through [`LlmConfig::with_providers`]), and on allama's mocks in tests.
//! allama has no agent loop on purpose; `techdepo-agent` runs it.
//!
//! The table is the deployment's *defaults*. Which model answers each task is the operator's
//! model settings (`domain::ModelSettings`, the Muhaha page): [`ModelRegistry`] is the
//! `ports::Models` port that turns them into adapters, over every provider the Worker can reach
//! ([`LlmConfig::registry`] on wasm): Workers AI through the binding, and Anthropic, OpenAI,
//! Azure OpenAI, Gemini and OpenRouter when their secrets are set on the Worker, over the Fetch
//! API.
//! Classification on a decision model (Clef) is calibrated; on any other model it is JSON mode.
//!
//! [`budget`] keeps `llm_usage` and gives the agent loop its per-org daily [`OrgBudget`].
//!
//! The adapters only talk to the model. Journaling (`ctx.run`), traces and budget checks are
//! the caller's (see `techdepo-agent`).

pub mod budget;
mod chat;
mod config;
mod decision;
pub mod mock;
mod registry;
mod structured;
#[cfg(target_arch = "wasm32")]
mod worker;

pub use budget::{BudgetLimits, DailyUsage, OrgBudget, UsageLedger};
pub use chat::AllamaLlm;
pub use config::{Backend, ConfigError, LlmConfig, LlmPorts};
pub use decision::DecisionClassifier;
pub use mock::{MockClassifier, MockLlm};
pub use registry::ModelRegistry;
pub use structured::{StructuredClassifier, WithFallback};
#[cfg(target_arch = "wasm32")]
pub use worker::WorkerFetch;
