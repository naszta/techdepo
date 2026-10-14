//! Org membership lookups: the [`MemberDirectory`] port.
//!
//! mail-in verifies that an inbound email's sender is a member of the org named in the
//! `support+<org>@` address before anything else happens (support.md §1). The admin Customers
//! page lists members.
//!
//! - `AUTH_MODE=clerk`: the Clerk Backend API (users by org and email), cached in KV for about
//!   10 minutes.
//! - `AUTH_MODE=dev`: the D1 `org_members` table, filled by the dev login.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use techdepo_domain::{OrgId, UserId};

/// A member's role in the org (Clerk's role keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemberRole {
    /// `org:admin`: may use `/admin` and the admin APIs.
    #[serde(rename = "org:admin")]
    Admin,
    /// `org:member`: a customer.
    #[serde(rename = "org:member")]
    Member,
}

impl MemberRole {
    /// Clerk's role key.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "org:admin",
            Self::Member => "org:member",
        }
    }

    /// Parses Clerk's role key. Unknown custom roles are treated as plain members.
    pub fn from_clerk(role: &str) -> Self {
        if role == Self::Admin.as_str() {
            Self::Admin
        } else {
            Self::Member
        }
    }
}

/// An org member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// Clerk user ID (or the dev user ID).
    pub user_id: UserId,
    /// Primary email address, lower-cased.
    pub email: String,
    /// Display name, if known.
    pub name: Option<String>,
    /// Role in the org.
    pub role: MemberRole,
}

/// A directory lookup failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DirectoryError {
    /// The directory could not be reached. Retryable. mail-in must not reject the email on
    /// this error: it returns a temporary failure so the sending server retries.
    #[error("member directory unavailable: {0}")]
    Unavailable(String),
    /// Misconfiguration (missing secret, bad credentials). Not retryable.
    #[error("member directory misconfigured: {0}")]
    Config(String),
}

impl DirectoryError {
    /// Whether retrying the same lookup may succeed.
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

/// Looks up org members.
#[async_trait]
pub trait MemberDirectory: Send + Sync {
    /// The member of `org` whose (verified) email is `email`, matched case-insensitively, or
    /// `None` if there is no such member.
    async fn is_member(&self, org: &OrgId, email: &str) -> Result<Option<Member>, DirectoryError>;

    /// Every member of `org`.
    async fn list_members(&self, org: &OrgId) -> Result<Vec<Member>, DirectoryError>;
}

#[async_trait]
impl<T: MemberDirectory + ?Sized> MemberDirectory for Arc<T> {
    async fn is_member(&self, org: &OrgId, email: &str) -> Result<Option<Member>, DirectoryError> {
        (**self).is_member(org, email).await
    }

    async fn list_members(&self, org: &OrgId) -> Result<Vec<Member>, DirectoryError> {
        (**self).list_members(org).await
    }
}
