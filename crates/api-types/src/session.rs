//! Runtime configuration, the dev login, and who the caller is.

use serde::{Deserialize, Serialize};
use techdepo_domain::{OrgFeature, OrgId, PublicOrgId, TicketId, UserId, labeled_enum};
use time::OffsetDateTime;

use crate::error::{ApiError, FieldError, Validate, check_text};
use crate::vocab::OrgRole;

labeled_enum! {
    /// How the web worker authenticates requests (`AUTH_MODE`).
    pub enum AuthMode {
        /// Clerk session JWTs (production, and local with a Clerk development instance).
        Clerk => ("clerk", "Clerk"),
        /// Unsigned dev tokens from `POST /api/dev/login`. Never in production.
        Dev => ("dev", "Dev login"),
    }
}

labeled_enum! {
    /// Which LLM backend the services use (`LLM_BACKEND`), so the UI can say "mock AI".
    pub enum LlmBackend {
        /// Deterministic keyword-based mock.
        Mock => ("mock", "Mock"),
        /// Workers AI (agents) and Clef (classification).
        WorkersAi => ("workers-ai", "Workers AI"),
    }
}

/// `GET /api/config`: what the SPA needs before sign-in. One SPA build works in every
/// environment because of this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// How to sign in.
    pub auth_mode: AuthMode,
    /// Clerk publishable key (`pk_…`); present when `auth_mode` is `clerk`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clerk_publishable_key: Option<String>,
    /// The domain of support addresses, e.g. `techdepo.dev`.
    pub support_domain: String,
    /// `techdepo_catalog::CATALOG_VERSION` of the deployed workers. The SPA compares it with
    /// its own compiled-in catalog and refetches `/api/catalog` when they differ.
    pub catalog_version: String,
    /// LLM backend in use.
    pub llm_backend: LlmBackend,
    /// Base URL of the Restate UI, for deep links from admin timelines (local dev only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restate_ui_url: Option<String>,
}

/// `POST /api/dev/login` (`AUTH_MODE=dev` only): signs in as any email, creating the dev org
/// and member on first use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevLoginRequest {
    /// Email address; also the identity (`user_dev_<hash>`).
    pub email: String,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Shop (org) name; the org ID is derived from it (`org_dev_<hash>`). Defaults to
    /// `Dev Shop`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_name: Option<String>,
    /// Role in that org. Defaults to admin, matching "one user is customer and admin".
    #[serde(default = "default_dev_role")]
    pub role: OrgRole,
}

const fn default_dev_role() -> OrgRole {
    OrgRole::Admin
}

impl Validate for DevLoginRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        check_email(&mut errors, "email", &self.email);
        if let Some(name) = &self.name {
            check_text(&mut errors, "name", name, 0, 100);
        }
        if let Some(org) = &self.org_name {
            check_text(&mut errors, "org_name", org, 1, 80);
        }
        ApiError::check(errors)
    }
}

/// Response of `POST /api/dev/login`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevLoginResponse {
    /// Send as `Authorization: Bearer <token>`. Unsigned; only accepted in `AUTH_MODE=dev`.
    pub token: String,
    /// The signed-in user and org.
    pub me: Me,
}

/// The signed-in user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserInfo {
    /// Clerk user ID.
    pub id: UserId,
    /// Primary email address.
    pub email: String,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The active org (webshop instance).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgInfo {
    /// Clerk org ID.
    pub id: OrgId,
    /// Short public ID used in the support address.
    pub public_id: PublicOrgId,
    /// Shop name.
    pub name: String,
    /// `support+<public_id>@<domain>`: where customers email.
    pub support_address: String,
    /// When the org was provisioned.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// When the demo org and all its data are deleted (`ORG_EXPIRY_DAYS`); `None`: never.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// Features switched on for the org (`orgs.features_json`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<OrgFeature>,
}

impl OrgInfo {
    /// Whether the org has `feature`.
    pub fn has(&self, feature: OrgFeature) -> bool {
        self.features.contains(&feature)
    }
}

/// `GET /api/me`. The first call for an unknown org provisions it (`Org/provision`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Me {
    /// The user.
    pub user: UserInfo,
    /// The active org.
    pub org: OrgInfo,
    /// The user's role in it.
    pub role: OrgRole,
}

impl Me {
    /// Whether the user may open `/admin`.
    pub const fn is_admin(&self) -> bool {
        self.role.is_admin()
    }
}

/// Where customers email support: `<local_part>+<public_id>@<domain>` (`SUPPORT_LOCAL_PART`,
/// `EMAIL_DOMAIN`). Each deployment has its own local part (`support` live, `support-dev` dev),
/// so its Email Routing rule reaches its own mail-in worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportMailbox {
    /// The local part before the `+` tag.
    pub local_part: String,
    /// The email domain.
    pub domain: String,
}

impl SupportMailbox {
    /// `<local_part>+<public_id>@<domain>`: the org's support address.
    pub fn address(&self, public_id: &PublicOrgId) -> String {
        format!("{}+{}@{}", self.local_part, public_id.as_str(), self.domain)
    }

    /// `<local_part>+<public_id>.<ticket code>@<domain>`: the `Reply-To` of a ticket's emails.
    pub fn ticket_reply(&self, public_id: &PublicOrgId, ticket: &TicketId) -> String {
        format!(
            "{}+{}.{}@{}",
            self.local_part,
            public_id.as_str(),
            ticket.code().to_ascii_lowercase(),
            self.domain
        )
    }
}

/// Pushes a field error unless `value` looks like an email address.
pub(crate) fn check_email(errors: &mut Vec<FieldError>, field: &str, value: &str) {
    let valid = value.len() <= 254
        && value.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        })
        && !value.contains(char::is_whitespace);
    if !valid {
        errors.push(FieldError::new(field, "Enter a valid email address"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        let org = PublicOrgId::parse("k3x9q2ab").unwrap();
        let ticket = TicketId::parse("T-7F2M9Q").unwrap();
        let live = SupportMailbox {
            local_part: "support".into(),
            domain: "techdepo.dev".into(),
        };
        assert_eq!(live.address(&org), "support+k3x9q2ab@techdepo.dev");
        assert_eq!(
            live.ticket_reply(&org, &ticket),
            "support+k3x9q2ab.7f2m9q@techdepo.dev"
        );
        let dev = SupportMailbox {
            local_part: "support-dev".into(),
            ..live
        };
        assert_eq!(dev.address(&org), "support-dev+k3x9q2ab@techdepo.dev");
    }

    #[test]
    fn dev_login_defaults_and_validation() {
        let req: DevLoginRequest = serde_json::from_str(r#"{"email":"jane@example.com"}"#).unwrap();
        assert_eq!(req.role, OrgRole::Admin);
        assert!(req.validate().is_ok());
        let bad = DevLoginRequest {
            email: "nope".into(),
            ..req
        };
        assert_eq!(bad.validate().unwrap_err().fields[0].field, "email");
    }
}
