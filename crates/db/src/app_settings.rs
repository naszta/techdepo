//! `app_settings`: the operator's app-wide settings (no org). The only key is `models`.

use serde::Deserialize;
use techdepo_domain::{ModelSettings, UserId};
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutor, SqlExecutorExt, params};
use time::OffsetDateTime;

use crate::{SqlError, Statement, json_param};

/// The stored model settings and who saved them.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredModels {
    /// The settings; `None` when the stored JSON no longer parses (a provider or field removed
    /// since), which callers treat as no settings.
    pub settings: Option<ModelSettings>,
    /// Saved at.
    pub updated_at: OffsetDateTime,
    /// Saved by.
    pub updated_by: Option<UserId>,
}

#[derive(Deserialize)]
struct Row {
    value_json: String,
    #[serde(with = "column::millis")]
    updated_at: OffsetDateTime,
    updated_by: Option<UserId>,
}

/// The app-wide settings. Not tenant data: only the `Muhaha` service writes it, and the
/// services read it (journaled) to pick their models.
#[derive(Clone, Copy)]
pub struct AppSettings<'a> {
    sql: &'a dyn SqlExecutor,
}

impl<'a> AppSettings<'a> {
    /// The settings over `sql`.
    pub fn new(sql: &'a dyn SqlExecutor) -> Self {
        Self { sql }
    }

    /// The stored model settings, if any.
    pub async fn models(&self) -> Result<Option<StoredModels>, SqlError> {
        let row: Option<Row> = self
            .sql
            .query_first_as(
                "SELECT value_json, updated_at, updated_by FROM app_settings WHERE key = 'models'",
                &[],
            )
            .await?;
        Ok(row.map(|r| StoredModels {
            settings: serde_json::from_str(&r.value_json).ok(),
            updated_at: r.updated_at,
            updated_by: r.updated_by,
        }))
    }

    /// The model settings in effect: the stored ones, else `defaults`.
    pub async fn models_or(&self, defaults: ModelSettings) -> Result<ModelSettings, SqlError> {
        Ok(self
            .models()
            .await?
            .and_then(|s| s.settings)
            .unwrap_or(defaults))
    }

    /// Replaces the model settings.
    pub fn set_models_statement(
        &self,
        settings: &ModelSettings,
        by: &UserId,
        at: OffsetDateTime,
    ) -> Result<Statement, SqlError> {
        Ok(Statement::new(
            "INSERT INTO app_settings (key, value_json, updated_at, updated_by) VALUES ('models', ?1, ?2, ?3) \
             ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json, \
             updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            vec![json_param(settings)?, at.into(), by.into()],
        ))
    }

    /// Forgets the model settings: the deployment's defaults apply again.
    pub fn clear_models_statement(&self) -> Statement {
        Statement::new("DELETE FROM app_settings WHERE key = 'models'", params![])
    }
}
