//! The model settings in effect for an invocation (`domain::ModelSettings`, set by the operator
//! through [`crate::Muhaha`]): read from `app_settings` once, in a journaled step named
//! `model-settings`, falling back to the deployment's defaults (`ports::Models::defaults`).
//!
//! Journaling is the point: a retry or a replay sees the models it saw the first time, and a
//! workflow that reads them at its start ([`crate::Investigation`]) keeps them to the end,
//! however the operator changes the settings meanwhile. Each consumer picks its slot:
//!
//! | Slot | Read by |
//! |---|---|
//! | `product_advisor`, `order_assistant`, `judge` | the agent pipeline, per route request |
//! | `triage` | the [`crate::Router`], per classification |
//! | `offer_reply` | the [`crate::Investigation`] workflow, once at its start |
//! | `classifier_fallback` | wherever a classifier is built |

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use restate_sdk::prelude::*;
use techdepo_agent::{Runtime, StepError};
use techdepo_db::{Db, SqlError};
use techdepo_domain::{ModelSettings, ModelSlot};
use techdepo_ports::{Classifier, Llm, Models};

use crate::Deps;
use crate::errors::sql;

/// The journal name of the read.
pub const STEP: &str = "model-settings";

/// The stored settings, else `models`' defaults. Not journaled: call it inside a step.
pub async fn load(db: &Db, models: &dyn Models) -> Result<ModelSettings, SqlError> {
    db.app_settings().models_or(models.defaults()).await
}

/// The read as a `ctx.run` closure, for a Restate handler to journal as [`STEP`]:
///
/// ```ignore
/// let models = ctx.run(models::read(&self.deps)).name(models::STEP).await?.into_inner();
/// ```
pub fn read(deps: &Deps) -> impl FnOnce() -> Read + Send + 'static {
    let (db, models) = (deps.db.clone(), deps.models.clone());
    move || Box::pin(async move { load(&db, &*models).await.map(Json).map_err(sql) })
}

/// The future of [`read`].
pub type Read = Pin<Box<dyn Future<Output = Result<Json<ModelSettings>, HandlerError>> + Send>>;

/// The settings, read in the journaled step [`STEP`] of an agent [`Runtime`]. A read that
/// keeps failing falls back to the defaults (the failure is journaled, so a replay does the
/// same): the run goes on.
pub async fn stepped<R: Runtime>(rt: &R, deps: &Deps) -> ModelSettings {
    let (db, models) = (deps.db.clone(), deps.models.clone());
    rt.step::<ModelSettings>(
        STEP,
        Box::new(move || {
            Box::pin(async move {
                load(&db, &*models).await.map_err(|e| StepError {
                    retryable: e.is_retryable(),
                    message: e.to_string(),
                })
            })
        }),
    )
    .await
    .unwrap_or_else(|_| deps.models.defaults())
}

/// The chat model of `slot` (a chat slot; the agents).
pub fn chat(deps: &Deps, settings: &ModelSettings, slot: ModelSlot) -> Arc<dyn Llm> {
    let choice = settings
        .get(slot)
        .cloned()
        .unwrap_or_else(|| settings.product_advisor.clone());
    deps.models.chat(&choice)
}

/// The classifier of `slot` (a classifier slot), with the fallback if one is set.
pub fn classifier(deps: &Deps, settings: &ModelSettings, slot: ModelSlot) -> Arc<dyn Classifier> {
    let choice = settings
        .get(slot)
        .cloned()
        .unwrap_or_else(|| settings.triage.clone());
    deps.models
        .classifier(&choice, settings.classifier_fallback.as_ref())
}
