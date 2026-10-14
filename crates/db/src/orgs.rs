//! `orgs` (one webshop instance per Clerk org) and the `org_members` identity cache.

use serde::{Deserialize, Deserializer};
use techdepo_api_types::vocab::{MemberSource, OrgRole};
use techdepo_domain::{OrgFeature, OrgId, OrgSettings, PublicOrgId, UserId, known_features};
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutor, SqlExecutorExt, params};
use time::OffsetDateTime;

use crate::{PageRequest, SqlError, Statement, json_param, repository, stmt};

/// An `orgs` row.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct OrgRow {
    /// Clerk org ID.
    pub id: OrgId,
    /// Email tag.
    pub public_id: PublicOrgId,
    /// Shop name.
    pub name: String,
    /// Settings (missing fields default).
    #[serde(rename = "settings_json", with = "column::json_text")]
    pub settings: OrgSettings,
    /// Catalog version inventory was last seeded from.
    pub catalog_version: Option<String>,
    /// Provisioned at.
    #[serde(with = "column::millis")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "column::millis")]
    pub updated_at: OffsetDateTime,
    /// When `Org/expire` deletes the org (`ORG_EXPIRY_DAYS`); `None` when it never expires.
    #[serde(default, with = "column::millis::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// Features switched on for the org (`0004`), known ones only.
    #[serde(rename = "features_json", deserialize_with = "features")]
    pub features: Vec<OrgFeature>,
}

fn features<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<OrgFeature>, D::Error> {
    let names: Vec<String> = column::json_text::deserialize(deserializer)?;
    Ok(known_features(&names))
}

/// An `org_members` row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MemberRow {
    /// Clerk user ID.
    pub user_id: UserId,
    /// Lower-cased email.
    pub email: String,
    /// Display name.
    pub name: Option<String>,
    /// Role.
    pub role: OrgRole,
    /// Where the row came from.
    pub source: MemberSource,
    /// First seen.
    #[serde(with = "column::millis")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "column::millis")]
    pub updated_at: OffsetDateTime,
    /// Last authenticated request.
    #[serde(with = "column::millis")]
    pub last_seen_at: OffsetDateTime,
}

/// A member as seen on an authenticated request (web worker identity cache).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberUpsert {
    /// Clerk user ID.
    pub user_id: UserId,
    /// Email (lower-cased on write).
    pub email: String,
    /// Display name.
    pub name: Option<String>,
    /// Role.
    pub role: OrgRole,
    /// Source.
    pub source: MemberSource,
}

/// Looks an org up by its public ID (mail-in resolving `support+<public_id>@…`). This is the one
/// unscoped org lookup: its result is what a scope is then built from.
pub async fn find_by_public_id(
    sql: &dyn SqlExecutor,
    public_id: &PublicOrgId,
) -> Result<Option<OrgRow>, SqlError> {
    sql.query_first_as(
        "SELECT * FROM orgs WHERE public_id = ?1",
        &params![public_id],
    )
    .await
}

/// One org as the operator sees it (the Muhaha page): the row's summary and its data volume.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OrgOverviewRow {
    /// Clerk org ID.
    pub id: OrgId,
    /// Email tag.
    pub public_id: PublicOrgId,
    /// Shop name.
    pub name: String,
    /// Catalog version inventory was last seeded from.
    pub catalog_version: Option<String>,
    /// Provisioned at.
    #[serde(with = "column::millis")]
    pub created_at: OffsetDateTime,
    /// When the org expires, if it does.
    #[serde(default, with = "column::millis::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// Cached members.
    pub members: u32,
    /// Orders.
    pub orders: u32,
    /// Tickets.
    pub tickets: u32,
}

/// Every org, newest first, for the operator. Unscoped like [`find_by_public_id`]: only the
/// Muhaha routes (operator access) and the `Muhaha` service call it.
pub async fn list_all(sql: &dyn SqlExecutor) -> Result<Vec<OrgOverviewRow>, SqlError> {
    sql.query_as(
        "SELECT o.id, o.public_id, o.name, o.catalog_version, o.created_at, o.expires_at, \
         (SELECT count(*) FROM org_members m WHERE m.org_id = o.id) AS members, \
         (SELECT count(*) FROM orders r WHERE r.org_id = o.id) AS orders, \
         (SELECT count(*) FROM tickets t WHERE t.org_id = o.id) AS tickets \
         FROM orgs o ORDER BY o.created_at DESC, o.id",
        &[],
    )
    .await
}

/// Every org's ID, oldest first (the order `Muhaha/reset_all` resets them in).
pub async fn all_ids(sql: &dyn SqlExecutor) -> Result<Vec<OrgId>, SqlError> {
    #[derive(Deserialize)]
    struct R {
        id: OrgId,
    }
    let rows: Vec<R> = sql
        .query_as("SELECT id FROM orgs ORDER BY created_at, id", &[])
        .await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

repository! {
    /// The scope's own `orgs` row and its member cache.
    Orgs
}

impl<'a> Orgs<'a> {
    /// The org row, if provisioned.
    pub async fn get(&self) -> Result<Option<OrgRow>, SqlError> {
        self.sql
            .query_first_as("SELECT * FROM orgs WHERE id = ?1", &params![self.org()])
            .await
    }

    /// The org's settings (defaults when the org is not provisioned).
    pub async fn settings(&self) -> Result<OrgSettings, SqlError> {
        Ok(self.get().await?.map(|o| o.settings).unwrap_or_default())
    }

    /// Inserts the org row unless it exists (provisioning). A clash on `public_id` with another
    /// org fails with a `Unique` constraint error; pick another ID and retry. An existing org
    /// keeps its `expires_at`.
    pub fn insert_statement(
        &self,
        public_id: &PublicOrgId,
        name: &str,
        settings: &OrgSettings,
        catalog_version: &str,
        at: OffsetDateTime,
        expires_at: Option<OffsetDateTime>,
    ) -> Result<Statement, SqlError> {
        Ok(stmt(
            "INSERT INTO orgs (id, public_id, name, settings_json, catalog_version, created_at, updated_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7) ON CONFLICT (id) DO NOTHING",
            vec![
                self.org().into(),
                public_id.into(),
                name.into(),
                json_param(settings)?,
                catalog_version.into(),
                at.into(),
                expires_at.into(),
            ],
        ))
    }

    /// Sets (or with `None`, clears) when the org expires.
    pub fn set_expiry_statement(
        &self,
        expires_at: Option<OffsetDateTime>,
        at: OffsetDateTime,
    ) -> Statement {
        stmt(
            "UPDATE orgs SET expires_at = ?2, updated_at = ?3 WHERE id = ?1",
            params![self.org(), expires_at, at],
        )
    }

    /// Deletes the org and everything it owns (expiry), as one batch: first what
    /// [`Orgs::reset_demo_statements`] deletes, in its order, then the `orgs` row, which cascades
    /// to the rest (members, stock, carts, builds, addresses, sequences, LLM usage).
    pub fn delete_statements(&self) -> Vec<Statement> {
        let mut statements = self.reset_demo_statements();
        statements.push(stmt("DELETE FROM orgs WHERE id = ?1", params![self.org()]));
        statements
    }

    /// Replaces the settings.
    pub async fn update_settings(
        &self,
        settings: &OrgSettings,
        at: OffsetDateTime,
    ) -> Result<u64, SqlError> {
        self.sql
            .execute(
                "UPDATE orgs SET settings_json = ?2, updated_at = ?3 WHERE id = ?1",
                &[self.org().into(), json_param(settings)?, at.into()],
            )
            .await
    }

    /// Records the catalog version inventory was seeded from.
    pub fn set_catalog_version_statement(&self, version: &str, at: OffsetDateTime) -> Statement {
        stmt(
            "UPDATE orgs SET catalog_version = ?2, updated_at = ?3 WHERE id = ?1",
            params![self.org(), version, at],
        )
    }

    /// Counts of what [`Orgs::reset_demo_statements`] deletes: orders, tickets, tasks, emails.
    pub async fn demo_data_counts(&self) -> Result<(u32, u32, u32, u32), SqlError> {
        #[derive(Deserialize)]
        struct C {
            orders: u32,
            tickets: u32,
            tasks: u32,
            emails: u32,
        }
        let c: Option<C> = self
            .sql
            .query_first_as(
                "SELECT (SELECT count(*) FROM orders WHERE org_id = ?1) AS orders, \
                 (SELECT count(*) FROM tickets WHERE org_id = ?1) AS tickets, \
                 (SELECT count(*) FROM human_tasks WHERE org_id = ?1) AS tasks, \
                 (SELECT count(*) FROM email_log WHERE org_id = ?1) AS emails",
                &params![self.org()],
            )
            .await?;
        Ok(c.map_or((0, 0, 0, 0), |c| (c.orders, c.tickets, c.tasks, c.emails)))
    }

    /// Wipes the org's operational data, in the order `migrations/README.md` gives (some
    /// foreign keys deliberately have no cascade). Stock is restored separately
    /// ([`crate::inventory::Inventory::restore_defaults_statements`]); orgs, settings, members,
    /// carts, builds, addresses, LLM usage and both sequences are kept.
    pub fn reset_demo_statements(&self) -> Vec<Statement> {
        [
            "DELETE FROM human_tasks WHERE org_id = ?1",
            "DELETE FROM agent_runs WHERE org_id = ?1",
            "DELETE FROM refunds WHERE org_id = ?1",
            "DELETE FROM investigations WHERE org_id = ?1",
            "DELETE FROM tickets WHERE org_id = ?1",
            "DELETE FROM orders WHERE org_id = ?1",
            "DELETE FROM email_log WHERE org_id = ?1",
        ]
        .into_iter()
        .map(|sql| stmt(sql, params![self.org()]))
        .collect()
    }

    // -- members ------------------------------------------------------------------------------

    /// Upserts the caller into the member cache (web worker, every authenticated request).
    pub async fn upsert_member(
        &self,
        member: &MemberUpsert,
        at: OffsetDateTime,
    ) -> Result<u64, SqlError> {
        self.sql
            .execute(
                "INSERT INTO org_members (org_id, user_id, email, name, role, source, created_at, updated_at, last_seen_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?7) \
                 ON CONFLICT (org_id, user_id) DO UPDATE SET email = excluded.email, \
                 name = coalesce(excluded.name, org_members.name), role = excluded.role, \
                 source = excluded.source, updated_at = excluded.updated_at, last_seen_at = excluded.last_seen_at",
                &params![
                    self.org(),
                    &member.user_id,
                    member.email.to_lowercase(),
                    member.name.clone(),
                    member.role.as_str(),
                    member.source.as_str(),
                    at
                ],
            )
            .await
    }

    /// One member.
    pub async fn member(&self, user: &UserId) -> Result<Option<MemberRow>, SqlError> {
        self.sql
            .query_first_as(
                "SELECT * FROM org_members WHERE org_id = ?1 AND user_id = ?2",
                &params![self.org(), user],
            )
            .await
    }

    /// Every `org:admin` in the member cache, most recently seen first (who gets task nudges).
    pub async fn admins(&self) -> Result<Vec<MemberRow>, SqlError> {
        self.sql
            .query_as(
                "SELECT * FROM org_members WHERE org_id = ?1 AND role = 'org:admin' \
                 ORDER BY last_seen_at DESC, user_id DESC LIMIT 20",
                &params![self.org()],
            )
            .await
    }

    /// A member by email (case-insensitive), for sender verification in `AUTH_MODE=dev`.
    pub async fn member_by_email(&self, email: &str) -> Result<Option<MemberRow>, SqlError> {
        self.sql
            .query_first_as(
                "SELECT * FROM org_members WHERE org_id = ?1 AND email = ?2",
                &params![self.org(), email.to_lowercase()],
            )
            .await
    }

    /// Members, most recently seen first. `q` matches email or name.
    pub async fn members(
        &self,
        q: Option<&str>,
        page: &PageRequest,
    ) -> Result<(Vec<MemberRow>, Option<String>), SqlError> {
        let (key, id) = page.desc_bounds();
        let pattern = q.map(|q| format!("%{}%", q.to_lowercase()));
        let rows: Vec<MemberRow> = self
            .sql
            .query_as(
                "SELECT * FROM org_members WHERE org_id = ?1 \
                 AND (?2 IS NULL OR email LIKE ?2 OR lower(coalesce(name, '')) LIKE ?2) \
                 AND (last_seen_at < ?3 OR (last_seen_at = ?3 AND user_id < ?4)) \
                 ORDER BY last_seen_at DESC, user_id DESC LIMIT ?5",
                &params![self.org(), pattern, key, id, page.fetch()],
            )
            .await?;
        Ok(page.finish(rows, |r| {
            crate::Cursor::new(
                column::millis::to_millis(r.last_seen_at),
                r.user_id.as_str(),
            )
        }))
    }
}
