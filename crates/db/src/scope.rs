use serde::{Deserialize, Serialize};
use techdepo_domain::OrgId;

/// The org every query of a repository is confined to.
///
/// Build it only from a **verified** identity: the org claim of a verified session (web
/// worker), the org of a verified inbound email (mail-in), or the `org_id` of a Restate handler
/// input after the handler checked its key against it (services). Never from a request body or
/// a URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OrgScope {
    org_id: OrgId,
}

impl OrgScope {
    /// The scope of a verified org.
    pub fn verified(org_id: OrgId) -> Self {
        Self { org_id }
    }

    /// The org ID.
    pub fn org_id(&self) -> &OrgId {
        &self.org_id
    }
}
