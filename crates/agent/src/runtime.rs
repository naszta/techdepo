//! Durable steps: the seam between the loop and the journal.

use std::future::Future;
use std::pin::Pin;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// A step's work: an owned, `Send` future (it may outlive the borrow of the loop, as Restate's
/// `ctx.run` closures must).
pub type StepFuture<T> = Pin<Box<dyn Future<Output = Result<T, StepError>> + Send + 'static>>;

/// Produces a step's work when the runtime decides to run it (never, on replay).
pub type StepFn<T> = Box<dyn FnOnce() -> StepFuture<T> + Send + 'static>;

/// A step failed.
///
/// Inside a durable runtime a retryable error is retried (Restate: a plain error from
/// `ctx.run`), a non-retryable one fails the step for good (a terminal error). What reaches the
/// loop is final either way.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct StepError {
    /// What went wrong.
    pub message: String,
    /// Whether running the step again may succeed.
    pub retryable: bool,
}

impl StepError {
    /// A transient failure (network, throttling, a busy database).
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }

    /// A failure that will not go away by retrying.
    pub fn terminal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }
}

/// Runs named, journaled steps.
///
/// The services implement it over Restate's context, roughly:
///
/// ```ignore
/// impl Runtime for Journaled<'_, '_> {
///     async fn step<T>(&self, name: &str, step: StepFn<T>) -> Result<T, StepError>
///     where
///         T: Serialize + DeserializeOwned + Send + 'static,
///     {
///         self.ctx
///             .run(move || async move {
///                 match step().await {
///                     Ok(value) => Ok(Json(value)),
///                     Err(e) if e.retryable => Err(HandlerError::from(e.message)),
///                     Err(e) => Err(TerminalError::new(e.message).into()),
///                 }
///             })
///             .name(name.to_owned())
///             .await
///             .map(|Json(value)| value)
///             .map_err(|e| StepError::terminal(e.message()))
///     }
/// }
/// ```
///
/// A step's result must be serializable because the journal stores it; on replay the runtime
/// returns the stored value without calling `step`.
pub trait Runtime: Sync {
    /// Runs `step` as the journaled step `name` (or returns its journaled result).
    fn step<T>(
        &self,
        name: &str,
        step: StepFn<T>,
    ) -> impl Future<Output = Result<T, StepError>> + Send
    where
        T: Serialize + DeserializeOwned + Send + 'static;
}

/// Runs every step directly, with no journal: unit tests, evals, anything not durable.
///
/// Retryable failures are returned like terminal ones (nothing retries them).
#[derive(Debug, Clone, Copy, Default)]
pub struct Inline;

impl Runtime for Inline {
    fn step<T>(
        &self,
        _name: &str,
        step: StepFn<T>,
    ) -> impl Future<Output = Result<T, StepError>> + Send
    where
        T: Serialize + DeserializeOwned + Send + 'static,
    {
        step()
    }
}

/// Live wall clock in Unix milliseconds, for trace timestamps and latencies only (never
/// branched on). Called inside journaled steps, so replays see the recorded values.
pub(crate) fn now_ms() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}
