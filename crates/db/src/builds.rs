//! Saved PC builds (`builds`), per user.

use serde::Deserialize;
use techdepo_api_types::BuildId;
use techdepo_domain::{ProductId, UserId};
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutorExt, params};
use time::OffsetDateTime;

use crate::{SqlError, Statement, json_param, repository, stmt};

/// A `builds` row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BuildRow {
    /// Build ID.
    pub id: BuildId,
    /// Owner.
    pub user_id: UserId,
    /// Name.
    pub name: String,
    /// Parts in slot order (may include products no longer in the catalog).
    #[serde(rename = "product_ids_json", with = "column::json_text")]
    pub product_ids: Vec<ProductId>,
    /// Created.
    #[serde(with = "column::millis")]
    pub created_at: OffsetDateTime,
    /// Last saved.
    #[serde(with = "column::millis")]
    pub updated_at: OffsetDateTime,
}

repository! {
    /// Saved builds.
    Builds
}

impl<'a> Builds<'a> {
    /// The user's builds, most recently saved first.
    pub async fn list(&self, user: &UserId) -> Result<Vec<BuildRow>, SqlError> {
        self.sql
            .query_as(
                "SELECT * FROM builds WHERE org_id = ?1 AND user_id = ?2 ORDER BY updated_at DESC, id DESC",
                &params![self.org(), user],
            )
            .await
    }

    /// A build of the user's (`None` if missing or someone else's).
    pub async fn get(&self, user: &UserId, id: &BuildId) -> Result<Option<BuildRow>, SqlError> {
        self.sql
            .query_first_as(
                "SELECT * FROM builds WHERE org_id = ?1 AND id = ?2 AND user_id = ?3",
                &params![self.org(), id.as_str(), user],
            )
            .await
    }

    /// Whether a build with this ID exists in the org at all (to tell "someone else's" apart).
    pub async fn exists(&self, id: &BuildId) -> Result<bool, SqlError> {
        Ok(self
            .sql
            .query_first(
                "SELECT 1 AS one FROM builds WHERE org_id = ?1 AND id = ?2",
                &params![self.org(), id.as_str()],
            )
            .await?
            .is_some())
    }

    /// Creates or replaces a build of the user's. Never takes over another user's row.
    pub fn save_statement(
        &self,
        user: &UserId,
        id: &BuildId,
        name: &str,
        product_ids: &[ProductId],
        at: OffsetDateTime,
    ) -> Result<Statement, SqlError> {
        Ok(stmt(
            "INSERT INTO builds (org_id, id, user_id, name, product_ids_json, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6) ON CONFLICT (org_id, id) DO UPDATE SET \
             name = excluded.name, product_ids_json = excluded.product_ids_json, updated_at = excluded.updated_at \
             WHERE builds.user_id = excluded.user_id",
            vec![
                self.org().into(),
                id.as_str().into(),
                user.into(),
                name.into(),
                json_param(product_ids)?,
                at.into(),
            ],
        ))
    }

    /// Deletes a build of the user's; returns whether it existed.
    pub async fn delete(&self, user: &UserId, id: &BuildId) -> Result<bool, SqlError> {
        let n = self
            .sql
            .execute(
                "DELETE FROM builds WHERE org_id = ?1 AND id = ?2 AND user_id = ?3",
                &params![self.org(), id.as_str(), user],
            )
            .await?;
        Ok(n > 0)
    }
}
