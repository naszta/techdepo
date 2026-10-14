//! `Muhaha` (service): the operator's tools, across every org and on app-wide settings.
//!
//! The web worker calls it only for an operator (`Access::Operator`: an `org:admin` of an org
//! with `OrgFeature::Muhaha`), and the inputs carry the operator's user ID. Org changes go
//! through the [`Org`](crate::Org) object, so they serialize with the org's own changes:
//!
//! | Handler | Does |
//! |---|---|
//! | `reset_all` | `Org/reset_demo` for every org, one after another, oldest first |
//! | `clear_org` | `Org/clear_data`: one org's operational data, stock kept |
//! | `reseed_org` | `Org/reseed_stock`: one org's stock back to the catalog defaults |
//! | `models` | the model settings in effect, the defaults, the providers with credentials |
//! | `update_models` | replaces (or forgets) the model settings in `app_settings` |
//!
//! New model settings apply to invocations that start afterwards: every consumer reads them
//! once, journaled ([`crate::models`]).

use std::sync::Arc;

use restate_sdk::prelude::*;
use techdepo_api_types::admin::DemoResetResponse;
use techdepo_api_types::ingress::{
    OperatorInput, OrgTargetInput, ResetDemoInput, UpdateModelsInput, keys,
};
use techdepo_api_types::muhaha::{
    ModelsResponse, OrgResetResult, ProviderStatus, ReseedResponse, ResetAllResponse,
};
use techdepo_db::app_settings::StoredModels;
use techdepo_domain::{ModelProvider, ModelSettings, ModelSlot, OrgId};

use crate::Deps;
use crate::errors::{invalid, sql};
use crate::org::OrgClient;
use crate::util;

/// The `Muhaha` service.
pub struct Muhaha {
    deps: Arc<Deps>,
}

impl Muhaha {
    /// The service over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }

    /// The response for `stored` settings (or none).
    fn response(&self, stored: Option<StoredModels>) -> ModelsResponse {
        let defaults = self.deps.models.defaults();
        let available = self.deps.models.available();
        let (settings, updated_at, updated_by) = match stored {
            Some(StoredModels {
                settings: Some(settings),
                updated_at,
                updated_by,
            }) => (Some(settings), Some(updated_at), updated_by),
            _ => (None, None, None),
        };
        ModelsResponse {
            stored: settings.is_some(),
            settings: settings.unwrap_or_else(|| defaults.clone()),
            defaults,
            providers: ModelProvider::ALL
                .iter()
                .map(|&provider| ProviderStatus {
                    provider,
                    available: available.contains(&provider),
                })
                .collect(),
            updated_at,
            updated_by,
        }
    }

    /// Refuses a model on a provider this deployment cannot reach: it would fail every call.
    fn check_available(&self, settings: &ModelSettings) -> Result<(), HandlerError> {
        let available = self.deps.models.available();
        for &slot in ModelSlot::ALL {
            let Some(choice) = settings.get(slot) else {
                continue;
            };
            if !available.contains(&choice.provider) {
                let how = choice
                    .provider
                    .secrets_hint()
                    .map_or_else(String::new, |s| format!(" (set {s})"));
                return Err(invalid(
                    &format!("settings.{}", slot.as_str()),
                    format!(
                        "{} is not configured in this deployment{how}",
                        choice.provider.label()
                    ),
                ));
            }
        }
        Ok(())
    }
}

#[restate_sdk::service(name = "Muhaha")]
impl Muhaha {
    /// Resets every org (`Org/reset_demo`), one after another, oldest first. An org deleted
    /// meanwhile is reported without counts.
    #[handler]
    async fn reset_all(
        &self,
        ctx: Context<'_>,
        input: Json<OperatorInput>,
    ) -> Result<Json<ResetAllResponse>, HandlerError> {
        let by = input.into_inner().by;
        let db = self.deps.db.clone();
        let ids: Json<Vec<OrgId>> = ctx
            .run(|| async move {
                techdepo_db::orgs::all_ids(db.sql())
                    .await
                    .map(Json)
                    .map_err(sql)
            })
            .name("list-orgs")
            .await?;
        let mut orgs = Vec::new();
        for org_id in ids.into_inner() {
            let reset = ctx
                .object_client::<OrgClient>(keys::org(&org_id))
                .reset_demo(Json(ResetDemoInput { by: by.clone() }))
                .call()
                .await;
            let reset = match reset {
                Ok(done) => Some(done.into_inner()),
                Err(e) if e.code() == 404 => None,
                Err(e) => return Err(e.into()),
            };
            orgs.push(OrgResetResult { org_id, reset });
        }
        Ok(Json(ResetAllResponse { orgs }))
    }

    /// Wipes one org's operational data (`Org/clear_data`); its stock stays.
    #[handler]
    async fn clear_org(
        &self,
        ctx: Context<'_>,
        input: Json<OrgTargetInput>,
    ) -> Result<Json<DemoResetResponse>, HandlerError> {
        let input = input.into_inner();
        Ok(ctx
            .object_client::<OrgClient>(keys::org(&input.org_id))
            .clear_data(Json(ResetDemoInput { by: input.by }))
            .call()
            .await?)
    }

    /// Restores one org's stock to the catalog defaults (`Org/reseed_stock`).
    #[handler]
    async fn reseed_org(
        &self,
        ctx: Context<'_>,
        input: Json<OrgTargetInput>,
    ) -> Result<Json<ReseedResponse>, HandlerError> {
        let input = input.into_inner();
        let done = ctx
            .object_client::<OrgClient>(keys::org(&input.org_id))
            .reseed_stock(Json(ResetDemoInput { by: input.by }))
            .call()
            .await?
            .into_inner();
        Ok(Json(ReseedResponse {
            products_restocked: done.products_restocked,
        }))
    }

    /// The model settings in effect, the deployment's defaults and the providers it can reach.
    #[handler]
    async fn models(
        &self,
        ctx: Context<'_>,
        _input: Json<OperatorInput>,
    ) -> Result<Json<ModelsResponse>, HandlerError> {
        let db = self.deps.db.clone();
        let stored = ctx
            .run(|| async move {
                db.app_settings()
                    .models()
                    .await
                    .map(|s| Json(s.map(Stored::from)))
                    .map_err(sql)
            })
            .name("load-models")
            .await?
            .into_inner();
        Ok(Json(self.response(stored.map(Into::into))))
    }

    /// Replaces the model settings (validated again here, and every provider must be
    /// reachable), or forgets them with `None`.
    #[handler]
    async fn update_models(
        &self,
        ctx: Context<'_>,
        input: Json<UpdateModelsInput>,
    ) -> Result<Json<ModelsResponse>, HandlerError> {
        let UpdateModelsInput { by, settings } = input.into_inner();
        if let Some(settings) = &settings {
            if let Err(e) = settings.validate() {
                return Err(invalid(
                    &format!("settings.{}", e.slot().as_str()),
                    e.to_string(),
                ));
            }
            self.check_available(settings)?;
        }
        let now = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        let (to_save, saved_by) = (settings.clone(), by.clone());
        ctx.run(|| async move {
            let app = db.app_settings();
            let statement = match &to_save {
                Some(settings) => app
                    .set_models_statement(settings, &saved_by, now)
                    .map_err(sql)?,
                None => app.clear_models_statement(),
            };
            db.batch(&[statement]).await.map(|_| ()).map_err(sql)
        })
        .name("save-models")
        .await?;
        Ok(Json(self.response(settings.map(|settings| StoredModels {
            settings: Some(settings),
            updated_at: now,
            updated_by: Some(by),
        }))))
    }
}

/// [`StoredModels`] as the journal keeps it.
#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    settings: Option<ModelSettings>,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: time::OffsetDateTime,
    updated_by: Option<techdepo_domain::UserId>,
}

impl From<StoredModels> for Stored {
    fn from(s: StoredModels) -> Self {
        Self {
            settings: s.settings,
            updated_at: s.updated_at,
            updated_by: s.updated_by,
        }
    }
}

impl From<Stored> for StoredModels {
    fn from(s: Stored) -> Self {
        Self {
            settings: s.settings,
            updated_at: s.updated_at,
            updated_by: s.updated_by,
        }
    }
}
