//! `Org` (virtual object, keyed by org ID): lazy provisioning, settings, stock, demo reset and
//! expiry. The operator's [`crate::Muhaha`] also clears an org's data (`clear_data`) or reseeds
//! its stock (`reseed_stock`) through it, so those serialize with the org's other changes.
//!
//! **Expiry** (`ORG_EXPIRY_DAYS`, off by default): provisioning stamps `orgs.expires_at` and
//! schedules a delayed `Org/expire` for that instant, a durable Restate timer instead of a cron.
//! When it fires, the org and everything it owns is deleted; its next authenticated request
//! provisions a fresh, empty shop (new public ID, new support address). The deadline is fixed at
//! provisioning: changing the setting later moves no existing deadline, and turning it off
//! spares an org whose timer fires afterwards. Orgs provisioned while it was off never expire.

use std::sync::Arc;
use std::time::Duration;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::admin::DemoResetResponse;
use techdepo_api_types::catalog::StockEntry;
use techdepo_api_types::ingress::{
    ProvisionOrgInput, ResetDemoInput, SetStockInput, UpdateSettingsInput,
};
use techdepo_api_types::session::OrgInfo;
use techdepo_catalog::{CATALOG_VERSION, Catalog};
use techdepo_db::inventory::MAX_STOCK;
use techdepo_db::{OrgScope, views};
use techdepo_domain::{OrgId, OrgSettings, PublicOrgId, SettingsError};
use techdepo_ports::ConstraintKind;
use time::OffsetDateTime;

use crate::Deps;
use crate::errors::{internal, invalid, not_found, sql};
use crate::util;

/// Longest org name stored.
const MAX_NAME_CHARS: usize = 100;

/// Longest expiry `ORG_EXPIRY_DAYS` accepts (ten years).
const MAX_EXPIRY_DAYS: f64 = 3650.0;

/// Parses `ORG_EXPIRY_DAYS`: days after provisioning that a demo org is deleted. Empty or `0`
/// is off; fractions are allowed (`0.01` is about a quarter of an hour, for trying it out) and
/// round down to whole seconds.
pub fn parse_expiry_days(value: &str) -> Result<Option<Duration>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let days: f64 = value
        .parse()
        .map_err(|_| format!("ORG_EXPIRY_DAYS must be a number of days, not `{value}`"))?;
    if !(0.0..=MAX_EXPIRY_DAYS).contains(&days) {
        return Err(format!(
            "ORG_EXPIRY_DAYS must be between 0 (off) and {MAX_EXPIRY_DAYS}"
        ));
    }
    // Bounded above, so the cast cannot overflow.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let secs = (days * 86_400.0) as u64;
    Ok((secs > 0).then(|| Duration::from_secs(secs)))
}

/// Input of `Org/expire`: the deadline the timer was set for. A timer whose deadline is no
/// longer the org's (re-provisioned, or expiry cleared) does nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpireOrgInput {
    /// `orgs.expires_at` when the timer was scheduled.
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

/// What `Org/expire` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpireOutcome {
    /// The org and its data were deleted.
    Deleted,
    /// Expiry was turned off since: the deadline was cleared and the org kept.
    Kept,
    /// The org is gone, or its deadline is another timer's.
    Stale,
    /// The deadline is still ahead (clock skew): scheduled again.
    NotYet,
}

/// What a provisioning step found or created.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Provisioned {
    info: OrgInfo,
    /// This call inserted the org (and so schedules its expiry).
    created: bool,
}

/// `at - now` as a timer delay (zero when `at` has passed).
fn delay_until(at: OffsetDateTime, now: OffsetDateTime) -> Duration {
    Duration::try_from(at - now).unwrap_or_default()
}

/// What a reset wipes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wipe {
    /// Operational data and stock (`reset_demo`).
    Both,
    /// Operational data only (`clear_data`).
    Data,
    /// Stock only (`reseed_stock`).
    Stock,
}

impl Org {
    /// One batch: the org's operational data deleted and/or its stock restored to the catalog
    /// defaults (which also records the catalog version it was seeded from). `404` for an
    /// unknown org.
    async fn wipe(
        &self,
        ctx: &ObjectContext<'_>,
        wipe: Wipe,
        step: &'static str,
    ) -> Result<Json<DemoResetResponse>, HandlerError> {
        let scope = OrgScope::verified(OrgId::new(ctx.key()));
        let now = util::now(ctx).await?;
        let db = self.deps.db.clone();
        let response: Json<DemoResetResponse> = ctx
            .run(|| async move {
                let orgs = db.orgs(&scope);
                if orgs.get().await.map_err(sql)?.is_none() {
                    return Err(not_found("org"));
                }
                let mut response = DemoResetResponse::default();
                let mut statements = Vec::new();
                if wipe != Wipe::Stock {
                    let (orders, tickets, tasks, emails) =
                        orgs.demo_data_counts().await.map_err(sql)?;
                    response = DemoResetResponse {
                        orders,
                        tickets,
                        tasks,
                        emails,
                        products_restocked: 0,
                    };
                    statements.extend(orgs.reset_demo_statements());
                }
                if wipe != Wipe::Data {
                    let restock = db
                        .inventory(&scope)
                        .restore_defaults_statements(Catalog::global(), now);
                    response.products_restocked = u32::try_from(restock.len()).unwrap_or(u32::MAX);
                    statements.extend(restock);
                    statements.push(orgs.set_catalog_version_statement(CATALOG_VERSION, now));
                }
                db.batch(&statements).await.map_err(sql)?;
                Ok(Json(response))
            })
            .name(step)
            .await?;
        Ok(response)
    }
}

/// The `Org` virtual object.
pub struct Org {
    deps: Arc<Deps>,
}

impl Org {
    /// The object over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }
}

#[restate_sdk::object(name = "Org")]
impl Org {
    /// Provisions the org on its first authenticated request: a public ID from Restate's
    /// deterministic randomness, default settings, and inventory seeded from the catalog.
    /// Idempotent: an existing org is returned (and its inventory topped up with products added
    /// to the catalog since).
    #[handler]
    async fn provision(
        &self,
        mut ctx: ObjectContext<'_>,
        input: Json<ProvisionOrgInput>,
    ) -> Result<Json<OrgInfo>, HandlerError> {
        let input = input.into_inner();
        let scope = OrgScope::verified(OrgId::new(ctx.key()));
        let now = util::now(&ctx).await?;
        let expires_at = self.deps.config.org_expiry.map(|d| now + d);
        let name: String = match input.name.trim() {
            "" => "My shop".to_owned(),
            n => n.chars().take(MAX_NAME_CHARS).collect(),
        };
        // A clash on the public ID (36^8 space) is retried with a fresh one.
        for _ in 0..3 {
            let bytes = ctx.rand_uuid().into_bytes();
            let mut entropy = [0_u8; PublicOrgId::LEN];
            entropy.copy_from_slice(&bytes[..PublicOrgId::LEN]);
            let public_id = PublicOrgId::from_entropy(entropy);
            let db = self.deps.db.clone();
            let support = self.deps.config.support_mailbox();
            let (scope, name) = (scope.clone(), name.clone());
            let provisioned: Json<Option<Provisioned>> = ctx
                .run(|| async move {
                    let orgs = db.orgs(&scope);
                    if let Some(row) = orgs.get().await.map_err(sql)? {
                        if row.catalog_version.as_deref() != Some(CATALOG_VERSION) {
                            let mut statements =
                                db.inventory(&scope).seed_statements(Catalog::global(), now);
                            statements
                                .push(orgs.set_catalog_version_statement(CATALOG_VERSION, now));
                            db.batch(&statements).await.map_err(sql)?;
                        }
                        return Ok(Json(Some(Provisioned {
                            info: views::org_info(&row, &support),
                            created: false,
                        })));
                    }
                    let mut statements = vec![
                        orgs.insert_statement(
                            &public_id,
                            &name,
                            &OrgSettings::default(),
                            CATALOG_VERSION,
                            now,
                            expires_at,
                        )
                        .map_err(sql)?,
                    ];
                    statements.extend(db.inventory(&scope).seed_statements(Catalog::global(), now));
                    match db.batch(&statements).await {
                        Ok(_) => {}
                        Err(e) if e.is_constraint(ConstraintKind::Unique) => return Ok(Json(None)),
                        Err(e) => return Err(sql(e)),
                    }
                    let row = orgs
                        .get()
                        .await
                        .map_err(sql)?
                        .ok_or_else(|| internal("org row missing after provisioning"))?;
                    Ok(Json(Some(Provisioned {
                        info: views::org_info(&row, &support),
                        created: true,
                    })))
                })
                .name("provision")
                .await?;
            if let Some(Provisioned { info, created }) = provisioned.into_inner() {
                if let (true, Some(at)) = (created, info.expires_at) {
                    let _ = ctx
                        .object_client::<OrgClient>(ctx.key())
                        .expire(Json(ExpireOrgInput { expires_at: at }))
                        .send_after(delay_until(at, now));
                }
                return Ok(Json(info));
            }
        }
        Err(internal("could not allocate a public org ID"))
    }

    /// Replaces the settings (validated again here).
    #[handler]
    async fn update_settings(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<UpdateSettingsInput>,
    ) -> Result<Json<OrgSettings>, HandlerError> {
        let mut input = input.into_inner();
        // Holidays are a set: stored in date order, once each.
        input.settings.holidays.sort_unstable();
        input.settings.holidays.dedup();
        if let Err(e) = input.settings.validate() {
            let field = match &e {
                SettingsError::NotAProbability { field, .. }
                | SettingsError::ZeroDuration(field)
                | SettingsError::DurationTooLong(field) => {
                    format!("settings.{field}")
                }
                SettingsError::VatRate(_) => "settings.vat_rate_bp".to_owned(),
                SettingsError::TooManyHolidays(_) => "settings.holidays".to_owned(),
                SettingsError::Fault(fault) => format!("settings.faults.{}", fault.field()),
            };
            return Err(invalid(&field, e.to_string()));
        }
        let scope = OrgScope::verified(OrgId::new(ctx.key()));
        let now = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        let settings = input.settings.clone();
        let updated: u64 = ctx
            .run(|| async move {
                db.orgs(&scope)
                    .update_settings(&settings, now)
                    .await
                    .map_err(sql)
            })
            .name("update-settings")
            .await?;
        if updated == 0 {
            return Err(not_found("org"));
        }
        Ok(Json(input.settings))
    }

    /// Sets one product's stock (0–9999).
    #[handler]
    async fn set_stock(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<SetStockInput>,
    ) -> Result<Json<StockEntry>, HandlerError> {
        let input = input.into_inner();
        if Catalog::global().get(&input.product_id).is_none() {
            return Err(not_found(format!("product {}", input.product_id)));
        }
        if input.stock > MAX_STOCK {
            return Err(invalid("stock", format!("At most {MAX_STOCK}")));
        }
        let scope = OrgScope::verified(OrgId::new(ctx.key()));
        let now = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        let (product, stock) = (input.product_id.clone(), input.stock);
        ctx.run(|| async move {
            let statement = db.inventory(&scope).set_statement(&product, stock, now);
            match db.batch(&[statement]).await {
                Ok(_) => Ok(()),
                Err(e) if e.is_constraint(ConstraintKind::ForeignKey) => Err(not_found("org")),
                Err(e) => Err(sql(e)),
            }
        })
        .name("set-stock")
        .await?;
        Ok(Json(views::stock_entry(input.product_id, input.stock)))
    }

    /// Wipes the org's orders, tickets, invoices, tasks, traces and email log, and restores the
    /// default stock (see `migrations/README.md`).
    #[handler]
    async fn reset_demo(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<ResetDemoInput>,
    ) -> Result<Json<DemoResetResponse>, HandlerError> {
        let _by = input.into_inner().by;
        self.wipe(&ctx, Wipe::Both, "reset-demo").await
    }

    /// The operator's half of the reset: wipes the org's operational data like `reset_demo`,
    /// and leaves the stock alone (`Muhaha/clear_org`).
    #[handler(ingress_private = true)]
    async fn clear_data(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<ResetDemoInput>,
    ) -> Result<Json<DemoResetResponse>, HandlerError> {
        let _by = input.into_inner().by;
        self.wipe(&ctx, Wipe::Data, "clear-data").await
    }

    /// The other half: every catalog product back at its default stock, products added to the
    /// catalog since included (`Muhaha/reseed_org`).
    #[handler(ingress_private = true)]
    async fn reseed_stock(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<ResetDemoInput>,
    ) -> Result<Json<DemoResetResponse>, HandlerError> {
        let _by = input.into_inner().by;
        self.wipe(&ctx, Wipe::Stock, "reseed-stock").await
    }

    /// The expiry timer (`ORG_EXPIRY_DAYS`): deletes the org and all its data in one batch, if
    /// `expires_at` is still the org's deadline and expiry is still on.
    #[handler(ingress_private = true)]
    async fn expire(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<ExpireOrgInput>,
    ) -> Result<Json<ExpireOutcome>, HandlerError> {
        let deadline = input.into_inner().expires_at;
        let scope = OrgScope::verified(OrgId::new(ctx.key()));
        let now = util::now(&ctx).await?;
        let enabled = self.deps.config.org_expiry.is_some();
        let db = self.deps.db.clone();
        let outcome: Json<ExpireOutcome> = ctx
            .run(|| async move {
                let orgs = db.orgs(&scope);
                let Some(row) = orgs.get().await.map_err(sql)? else {
                    return Ok(Json(ExpireOutcome::Stale));
                };
                if row.expires_at != Some(deadline) {
                    return Ok(Json(ExpireOutcome::Stale));
                }
                if !enabled {
                    db.batch(&[orgs.set_expiry_statement(None, now)])
                        .await
                        .map_err(sql)?;
                    return Ok(Json(ExpireOutcome::Kept));
                }
                if now < deadline {
                    return Ok(Json(ExpireOutcome::NotYet));
                }
                db.batch(&orgs.delete_statements()).await.map_err(sql)?;
                Ok(Json(ExpireOutcome::Deleted))
            })
            .name("expire")
            .await?;
        let outcome = outcome.into_inner();
        if outcome == ExpireOutcome::NotYet {
            let _ = ctx
                .object_client::<OrgClient>(ctx.key())
                .expire(Json(ExpireOrgInput {
                    expires_at: deadline,
                }))
                .send_after(delay_until(deadline, now));
        }
        Ok(Json(outcome))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_days_parse() {
        assert_eq!(parse_expiry_days(""), Ok(None));
        assert_eq!(parse_expiry_days(" 0 "), Ok(None));
        assert_eq!(
            parse_expiry_days("30"),
            Ok(Some(Duration::from_secs(30 * 86_400)))
        );
        assert_eq!(
            parse_expiry_days("0.01"),
            Ok(Some(Duration::from_secs(864)))
        );
        assert!(parse_expiry_days("-1").is_err());
        assert!(parse_expiry_days("NaN").is_err());
        assert!(parse_expiry_days("30d").is_err());
        assert!(parse_expiry_days("99999").is_err());
    }

    #[test]
    fn delays_never_go_negative() {
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::days(1);
        assert_eq!(
            delay_until(now + time::Duration::minutes(2), now),
            Duration::from_secs(120)
        );
        assert_eq!(
            delay_until(now - time::Duration::minutes(2), now),
            Duration::ZERO
        );
    }
}
