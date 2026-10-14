//! Ports: the traits TechDepó's services depend on instead of concrete infrastructure.
//!
//! `crates/services` is generic over these traits (all but [`MemberDirectory`], which only
//! mail-in's sender check uses) and must build for both `wasm32` (production, on Workers) and
//! native (unit tests, later e2e tests). The adapters live elsewhere:
//!
//! | Port | Production adapter | Test / local adapter |
//! |---|---|---|
//! | [`SqlExecutor`] | D1 (`crates/db`, wasm) | rusqlite (post-plan) |
//! | [`Llm`] | Workers AI, Anthropic, OpenAI, Azure OpenAI, Gemini or OpenRouter through allama (`crates/llm`) | `crates/llm` `mock`, `testing::ScriptedLlm` |
//! | [`Classifier`] | Workers AI Clef (`crates/llm`) | `crates/llm` `mock` (keyword-based) |
//! | [`Models`] | `crates/llm` `ModelRegistry`: the providers with credentials | services `FixedModels` |
//! | [`MemberDirectory`] | Clerk Backend API + KV cache (`apps/mail-in`) | D1 `org_members` (`AUTH_MODE=dev`), `testing::StaticDirectory` |
//! | [`Mailer`] | Resend over Fetch | Mailpit HTTP API, `testing::RecordingMailer` |
//!
//! Time is not a port: handlers use Restate's journaled clock (`restate_ext`'s `ctx.now()`),
//! and pure code takes time as a parameter. Randomness comes from `ctx.rand()`. The carrier is
//! not a port either: it is simulated in D1 by the `Carrier` service.
//!
//! # `Send` bounds: port futures are `Send` on every target
//!
//! Services call ports inside Restate's `ctx.run(|| async { … })`, and `ctx.run` requires the
//! closure's future to be `Send`. So **every port trait is declared with plain
//! `#[async_trait]`** (whose futures are `Send`) **on every target, including `wasm32`**. Every
//! port also requires `Send + Sync`, so services can hold ports as `Arc<dyn Port>` and pick a
//! backend at runtime (for example `LLM_BACKEND=mock`).
//!
//! JS-backed futures on Workers (D1, Fetch, Workers AI) are `!Send`, and so are the JS handles
//! behind them. **The wasm adapters make them `Send`**: they keep JS handles in
//! `send_wrapper::SendWrapper` and wrap each JS future in `SendWrapper` (or
//! `worker::send::SendFuture`) before awaiting it:
//!
//! ```ignore
//! pub struct D1Executor(send_wrapper::SendWrapper<worker::D1Database>);
//!
//! #[async_trait::async_trait]
//! impl SqlExecutor for D1Executor {
//!     async fn execute(&self, sql: &str, params: &[Param]) -> Result<u64, SqlError> {
//!         let stmt = self.0.prepare(sql).bind(&to_js(params))?;   // sync, no await
//!         let result = SendWrapper::new(stmt.run()).await?;       // the !Send JS future, wrapped
//!         Ok(rows_written(&result))
//!     }
//!     // …
//! }
//! ```
//!
//! This is sound because a Worker isolate is single-threaded: `SendWrapper` panics only if its
//! value is touched from a different thread, which cannot happen there. Rules that follow:
//!
//! - Service code never sees a JS type and never wraps anything in `SendWrapper`.
//! - No `?Send` variants exist. A `cfg`-gated `?Send` on wasm32 would make every `ctx.run`
//!   around a port call fail to compile on wasm only, which is exactly the drift the native
//!   `cargo check` of `crates/services` is meant to prevent.
//! - Adapters do the conversion between JS values and the port types (for example D1's numbers,
//!   see [`sql`]), so ports stay plain Rust and serde.
//!
//! # Errors and retries
//!
//! Every port error has `is_retryable()`. Inside `ctx.run`, a retryable error should be returned
//! as a normal error, so Restate retries it with the handler's retry policy. A non-retryable one
//! should become a terminal error.

pub mod classifier;
pub mod directory;
pub mod llm;
pub mod mailer;
pub mod models;
pub mod sql;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use classifier::{
    Answer, AnswerValue, ChoiceOption, Classification, Classifier, ClassifierError,
    OptionProbability, Question, QuestionKind,
};
pub use directory::{DirectoryError, Member, MemberDirectory, MemberRole};
pub use llm::{
    ChatMessage, ChatRequest, ChatResponse, FinishReason, Llm, LlmError, ToolCall, ToolDefinition,
    Usage,
};
pub use mailer::{Attachment, Header, MailError, Mailbox, Mailer, OutboundEmail};
pub use models::Models;
pub use sql::{
    BatchResult, ConstraintKind, Param, Row, SqlError, SqlExecutor, SqlExecutorExt, Statement,
};

/// Re-exported so adapters implement the ports with the same macro version.
pub use async_trait::async_trait;
