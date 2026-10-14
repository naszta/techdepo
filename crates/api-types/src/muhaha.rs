//! The Muhaha page (`/api/admin/muhaha/*`, [`Access::Operator`](crate::Access::Operator)): the
//! operator's tools across every org, and the app-wide model settings.
//!
//! The destructive calls take the same typed confirmation as the demo reset
//! ([`crate::admin::DemoResetRequest`]).

use serde::{Deserialize, Serialize};
use techdepo_domain::{ModelProvider, ModelSettings, OrgId, PublicOrgId, UserId};
use time::OffsetDateTime;

use crate::admin::DemoResetResponse;
use crate::error::{ApiError, Validate};

/// One org, as `GET /api/admin/muhaha/orgs` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuhahaOrg {
    /// Clerk org ID.
    pub id: OrgId,
    /// Email tag.
    pub public_id: PublicOrgId,
    /// Shop name.
    pub name: String,
    /// Provisioned at.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// When the org expires, if it does.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// Cached members.
    pub members: u32,
    /// Orders.
    pub orders: u32,
    /// Tickets.
    pub tickets: u32,
    /// Whether its stock was seeded from the catalog this deployment serves; otherwise
    /// products added since have no stock until it is reseeded.
    pub catalog_current: bool,
    /// The caller's own org.
    pub current: bool,
}

/// `GET /api/admin/muhaha/orgs`: every org, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuhahaOrgList {
    /// The orgs.
    pub orgs: Vec<MuhahaOrg>,
}

/// What `POST /api/admin/muhaha/reset` did to one org.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgResetResult {
    /// The org.
    pub org_id: OrgId,
    /// Rows deleted and products restocked; `None` when the org was gone by the time its turn
    /// came.
    pub reset: Option<DemoResetResponse>,
}

/// `POST /api/admin/muhaha/reset`: every org reset, one after another, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetAllResponse {
    /// One entry per org.
    pub orgs: Vec<OrgResetResult>,
}

impl ResetAllResponse {
    /// The totals over every org.
    pub fn total(&self) -> DemoResetResponse {
        self.orgs
            .iter()
            .filter_map(|o| o.reset)
            .fold(DemoResetResponse::default(), |t, r| DemoResetResponse {
                orders: t.orders + r.orders,
                tickets: t.tickets + r.tickets,
                tasks: t.tasks + r.tasks,
                emails: t.emails + r.emails,
                products_restocked: t.products_restocked + r.products_restocked,
            })
    }
}

/// `POST /api/admin/muhaha/orgs/{org_id}/reseed`: every catalog product back at its default
/// stock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReseedResponse {
    /// Products whose stock was restored (or added).
    pub products_restocked: u32,
}

/// A provider and whether the deployment can reach it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStatus {
    /// Provider.
    pub provider: ModelProvider,
    /// It has credentials (a Worker secret) or a binding.
    pub available: bool,
}

/// `GET`/`PUT /api/admin/muhaha/models`: the model settings in effect and what can be chosen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelsResponse {
    /// The settings in effect: the stored ones, else the defaults.
    pub settings: ModelSettings,
    /// Whether settings are stored (otherwise the defaults apply).
    pub stored: bool,
    /// The deployment's defaults (`LLM_BACKEND`, `AI_*`).
    pub defaults: ModelSettings,
    /// Every provider, available or not.
    pub providers: Vec<ProviderStatus>,
    /// When the stored settings were saved.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
    /// Who saved them.
    #[serde(default)]
    pub updated_by: Option<UserId>,
}

/// `PUT /api/admin/muhaha/models`: replace the model settings; `None` forgets them, so the
/// deployment's defaults apply again. Running workflows keep the models they started with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateModelsRequest {
    /// The new settings.
    pub settings: Option<ModelSettings>,
}

impl Validate for UpdateModelsRequest {
    fn validate(&self) -> Result<(), ApiError> {
        match &self.settings {
            Some(settings) => settings.validate().map_err(|e| {
                ApiError::invalid_field(format!("settings.{}", e.slot().as_str()), e.to_string())
            }),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use techdepo_domain::{ModelChoice, ModelSlot};

    use super::*;

    #[test]
    fn model_updates_are_validated_per_slot() {
        assert!(UpdateModelsRequest { settings: None }.validate().is_ok());
        let mut settings = ModelSettings::mock();
        settings.set(
            ModelSlot::OfferReply,
            Some(ModelChoice::new(ModelProvider::Gemini, "")),
        );
        let err = UpdateModelsRequest {
            settings: Some(settings),
        }
        .validate()
        .unwrap_err();
        assert_eq!(err.fields[0].field, "settings.offer_reply");
    }

    #[test]
    fn reset_totals_skip_missing_orgs() {
        let one = DemoResetResponse {
            orders: 2,
            tickets: 1,
            tasks: 0,
            emails: 3,
            products_restocked: 40,
        };
        let all = ResetAllResponse {
            orgs: vec![
                OrgResetResult {
                    org_id: OrgId::new("org_a"),
                    reset: Some(one),
                },
                OrgResetResult {
                    org_id: OrgId::new("org_b"),
                    reset: None,
                },
                OrgResetResult {
                    org_id: OrgId::new("org_c"),
                    reset: Some(one),
                },
            ],
        };
        assert_eq!(all.total().orders, 4);
        assert_eq!(all.total().products_restocked, 80);
    }
}
