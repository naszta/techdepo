//! `LLM_BACKEND=mock`: a deterministic classifier and a scripted chat model, so every demo
//! scenario runs offline, without a Cloudflare account.
//!
//! Neither is a stub: both read the text they are given. [`MockClassifier`] scores keywords
//! and turns the scores into calibrated-looking probabilities (vague, angry or multi-intent
//! messages come out uncertain, so escalation can be demoed). [`MockLlm`] plays the agents: it
//! reads the conversation and the tool results so far and makes the tool calls a good model
//! would, then answers from what the tools returned (so the guardrails pass).

mod chat;
mod classifier;
mod text;

pub use chat::MockLlm;
pub use classifier::MockClassifier;

/// A rough token count (about four characters per token), for usage accounting.
pub(crate) fn estimate_tokens(chars: usize) -> u32 {
    u32::try_from(chars.div_ceil(4)).unwrap_or(u32::MAX)
}
