//! The agent loop's [`Runtime`] over Restate: every step is a named `ctx.run`.

use std::time::Duration;

use restate_sdk::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use techdepo_agent::{Runtime, StepError, StepFn};

/// Journals each step with `ctx.run(..).name(name)`: on retry or replay the model is not
/// called again (and a D1-reading tool not re-read).
///
/// Retryable failures (a model outage, throttling, a busy D1) are retried a few times with
/// backoff; then the step fails for good and the loop escalates. A step never retries forever:
/// the ticket must reach a person if the model stays down.
pub struct Journaled<'a, 'ctx> {
    ctx: &'a Context<'ctx>,
}

impl<'a, 'ctx> Journaled<'a, 'ctx> {
    /// The runtime over `ctx`.
    pub fn new(ctx: &'a Context<'ctx>) -> Self {
        Self { ctx }
    }
}

/// Attempts per step before it fails for good.
const MAX_ATTEMPTS: u32 = 5;

impl Runtime for Journaled<'_, '_> {
    async fn step<T>(&self, name: &str, step: StepFn<T>) -> Result<T, StepError>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
    {
        self.ctx
            .run(move || async move {
                match step().await {
                    Ok(value) => Ok(Json(value)),
                    Err(e) if e.retryable => Err(HandlerError::from(e.message)),
                    Err(e) => Err(TerminalError::new(e.message).into()),
                }
            })
            .name(name.to_owned())
            .retry_policy(
                RunRetryPolicy::default()
                    .initial_delay(Duration::from_millis(500))
                    .exponentiation_factor(2.0)
                    .max_delay(Duration::from_secs(10))
                    .max_attempts(MAX_ATTEMPTS),
            )
            .await
            .map(Json::into_inner)
            .map_err(|e| StepError::terminal(e.message().to_owned()))
    }
}
