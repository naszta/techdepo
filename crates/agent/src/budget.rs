//! The budget hook: checked before every model call, fed the usage after it.

use async_trait::async_trait;
use techdepo_ports::Usage;

use crate::runtime::StepError;

/// Whether another model call is allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetCheck {
    /// Go ahead.
    Allowed,
    /// The budget is spent; the agent escalates with `reason`.
    Exhausted {
        /// Shown in the escalation, e.g. "daily AI budget of 200000 tokens used up".
        reason: String,
    },
}

/// A token budget, already bound to its scope (one org, one UTC day).
///
/// Both methods run inside the model call's journaled step, so they may do I/O (D1). A failed
/// `record` does not fail the call: the tokens are spent either way, and budgets tolerate the
/// rare miss (they also tolerate double counts from retried steps).
#[async_trait]
pub trait Budget: Send + Sync {
    /// Called before each model call.
    async fn check(&self, model: &str) -> Result<BudgetCheck, StepError>;

    /// Called after each successful model call with what it used.
    async fn record(&self, model: &str, usage: Usage) -> Result<(), StepError>;
}

/// No budget: every call is allowed and nothing is recorded.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unlimited;

#[async_trait]
impl Budget for Unlimited {
    async fn check(&self, _model: &str) -> Result<BudgetCheck, StepError> {
        Ok(BudgetCheck::Allowed)
    }

    async fn record(&self, _model: &str, _usage: Usage) -> Result<(), StepError> {
        Ok(())
    }
}
