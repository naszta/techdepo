//! Who is calling: the verified identity of a request, the org it acts in, and the member
//! cache (`docs/plan/api.md`, "Authentication and access").
//!
//! - **`AUTH_MODE=clerk`:** `ClerkAuthLayer` verified the session JWT; org ID and role come
//!   from its org claims. A token without an active org is `403`.
//! - **`AUTH_MODE=dev`:** the bearer token is `dev.<base64url(JSON DevClaims)>`, unsigned,
//!   minted by `POST /api/dev/login`: `org_dev_<hash(org name)>`, `user_dev_<hash(email)>`.
//!
//! Every authenticated request then resolves a [`Caller`]: the `orgs` row (provisioned through
//! `Org/provision` the first time an org is seen) and the `org_members` cache entry, refreshed
//! when the role, email or name changed or `last_seen_at` is older than
//! [`MEMBER_REFRESH_SECS`]. That upsert is the one D1 write the web worker does itself.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use techdepo_api_types::ingress::{CustomerInput, OrgProvision, ProvisionOrgInput, keys};
use techdepo_api_types::session::{Me, SupportMailbox, UserInfo};
use techdepo_api_types::vocab::{MemberSource, OrgRole};
use techdepo_api_types::{ApiError, CustomerRef, ErrorCode, OrgId, UserId};
use techdepo_db::OrgScope;
use techdepo_db::orgs::{MemberUpsert, OrgRow};
use techdepo_db::views;
use techdepo_domain::fnv1a64;
use time::OffsetDateTime;

use crate::app::App;
use crate::error::{ApiResult, DbResultExt};

/// The member cache is rewritten at most this often per member when nothing changed.
pub const MEMBER_REFRESH_SECS: i64 = 300;

/// Default shop name of the dev login.
pub const DEV_ORG_NAME: &str = "Dev Shop";

/// What a dev token says (`AUTH_MODE=dev` only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevClaims {
    /// `org_dev_<hash>`.
    pub org_id: OrgId,
    /// Shop name.
    pub org_name: String,
    /// `user_dev_<hash>`.
    pub user_id: UserId,
    /// Email (lower case).
    pub email: String,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Role.
    pub role: OrgRole,
}

const DEV_TOKEN_PREFIX: &str = "dev.";

impl DevClaims {
    /// Claims for a dev login: IDs derived from the (normalized) org name and email.
    pub fn new(email: &str, name: Option<String>, org_name: Option<&str>, role: OrgRole) -> Self {
        let email = email.trim().to_lowercase();
        let org_name = org_name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(DEV_ORG_NAME)
            .to_owned();
        Self {
            org_id: OrgId::new(format!(
                "org_dev_{:016x}",
                fnv1a64(org_name.to_lowercase().as_bytes())
            )),
            user_id: UserId::new(format!("user_dev_{:016x}", fnv1a64(email.as_bytes()))),
            org_name,
            email,
            name: name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()),
            role,
        }
    }

    /// The bearer token.
    pub fn token(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        format!("{DEV_TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
    }

    /// Reads a bearer token; `None` if it is not a dev token.
    pub fn from_token(token: &str) -> Option<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(token.strip_prefix(DEV_TOKEN_PREFIX)?)
            .ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

/// A verified identity, before the org and member cache are resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// Active org.
    pub org_id: OrgId,
    /// User.
    pub user_id: UserId,
    /// Role in the org.
    pub role: OrgRole,
    /// Email, when the credential carries it (dev tokens).
    pub email: Option<String>,
    /// Name, when the credential carries it.
    pub name: Option<String>,
    /// Org name, when the credential carries it.
    pub org_name: Option<String>,
    /// Where the identity came from.
    pub source: MemberSource,
}

impl From<DevClaims> for Identity {
    fn from(c: DevClaims) -> Self {
        Self {
            org_id: c.org_id,
            user_id: c.user_id,
            role: c.role,
            email: Some(c.email),
            name: c.name,
            org_name: Some(c.org_name),
            source: MemberSource::Dev,
        }
    }
}

/// An authenticated caller acting in its org.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The org (provisioned).
    pub org: OrgRow,
    /// User.
    pub user_id: UserId,
    /// Role.
    pub role: OrgRole,
    /// Email (lower case).
    pub email: String,
    /// Display name.
    pub name: Option<String>,
}

impl Caller {
    /// The org ID.
    pub fn org_id(&self) -> &OrgId {
        &self.org.id
    }

    /// The scope every query of this request uses.
    pub fn scope(&self) -> OrgScope {
        OrgScope::verified(self.org.id.clone())
    }

    /// `org:admin`.
    pub fn is_admin(&self) -> bool {
        self.role.is_admin()
    }

    /// The `Customer` object's identity.
    pub fn customer(&self) -> CustomerInput {
        CustomerInput {
            org_id: self.org.id.clone(),
            user_id: self.user_id.clone(),
        }
    }

    /// `Customer` key.
    pub fn customer_key(&self) -> String {
        keys::customer(&self.org.id, &self.user_id)
    }

    /// The caller as a customer link.
    pub fn customer_ref(&self) -> CustomerRef {
        CustomerRef {
            user_id: self.user_id.clone(),
            email: self.email.clone(),
            name: self.name.clone(),
        }
    }

    /// `/api/me`.
    pub fn me(&self, support: &SupportMailbox) -> Me {
        Me {
            user: UserInfo {
                id: self.user_id.clone(),
                email: self.email.clone(),
                name: self.name.clone(),
            },
            org: views::org_info(&self.org, support),
            role: self.role,
        }
    }
}

/// Resolves `identity` into a [`Caller`]: provisions the org if it is new and refreshes the
/// member cache.
pub async fn resolve(app: &App, identity: Identity, now: OffsetDateTime) -> ApiResult<Caller> {
    let scope = OrgScope::verified(identity.org_id.clone());
    let orgs = app.db.orgs(&scope);
    let org = match orgs.get().await.db()? {
        Some(org) => org,
        None => provision(app, &identity).await?,
    };
    let member = orgs.member(&identity.user_id).await.db()?;
    let (email, name) = match (&identity.email, &member) {
        (Some(email), _) => (email.clone(), identity.name.clone()),
        (None, Some(m)) => (m.email.clone(), m.name.clone()),
        (None, None) => {
            let profile = match &app.clerk {
                Some(clerk) => clerk.user(identity.user_id.as_str()).await,
                None => None,
            };
            let profile = profile.ok_or_else(|| {
                ApiError::new(
                    ErrorCode::Unavailable,
                    "Could not load your profile. Try again in a moment.",
                )
            })?;
            (profile.email, profile.name)
        }
    };
    let stale = member.as_ref().is_none_or(|m| {
        m.role != identity.role
            || m.email != email
            || (name.is_some() && m.name != name)
            || (now - m.last_seen_at).whole_seconds() > MEMBER_REFRESH_SECS
    });
    if stale {
        let upsert = MemberUpsert {
            user_id: identity.user_id.clone(),
            email: email.clone(),
            name: name.clone(),
            role: identity.role,
            source: identity.source,
        };
        orgs.upsert_member(&upsert, now).await.db()?;
    }
    Ok(Caller {
        org,
        user_id: identity.user_id,
        role: identity.role,
        email,
        name: name.or_else(|| member.and_then(|m| m.name)),
    })
}

/// `Org/provision`, then the fresh `orgs` row.
async fn provision(app: &App, identity: &Identity) -> ApiResult<OrgRow> {
    let name = match (&identity.org_name, &app.clerk) {
        (Some(name), _) => Some(name.clone()),
        (None, Some(clerk)) => clerk.org_name(identity.org_id.as_str()).await,
        (None, None) => None,
    }
    .unwrap_or_else(|| identity.org_id.as_str().to_owned());
    let key = keys::org(&identity.org_id);
    app.ingress
        .call::<OrgProvision>(Some(&key), &ProvisionOrgInput { name }, None)
        .await?;
    let scope = OrgScope::verified(identity.org_id.clone());
    app.db.orgs(&scope).get().await.db()?.ok_or_else(|| {
        ApiError::new(
            ErrorCode::Unavailable,
            "Your shop is still being set up. Try again in a moment.",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_tokens_round_trip() {
        let claims = DevClaims::new(
            " Jane@Example.com ",
            Some("Jane".into()),
            None,
            OrgRole::Admin,
        );
        assert_eq!(claims.email, "jane@example.com");
        assert_eq!(claims.org_name, DEV_ORG_NAME);
        assert!(claims.org_id.as_str().starts_with("org_dev_"));
        assert!(claims.user_id.as_str().starts_with("user_dev_"));
        let token = claims.token();
        assert!(token.starts_with("dev."));
        assert_eq!(DevClaims::from_token(&token), Some(claims.clone()));
        assert_eq!(DevClaims::from_token("eyJhbGciOi.x.y"), None);
        // The org ID depends on the name only, case-insensitively.
        let other = DevClaims::new("bob@example.com", None, Some("dev shop"), OrgRole::Member);
        assert_eq!(other.org_id, claims.org_id);
        assert_ne!(other.user_id, claims.user_id);
    }
}
