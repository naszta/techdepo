//! Sender verification: the `MemberDirectory` port (support.md §1 step 4).
//!
//! | `AUTH_MODE` | Directory |
//! |---|---|
//! | `dev` | [`D1Directory`]: the `org_members` cache the dev login fills |
//! | `clerk` | `ClerkDirectory` (wasm): the Clerk Backend API's org memberships, cached ~10 minutes in KV (`MEMBERS_CACHE`, optional) and in the isolate's memory |

use async_trait::async_trait;
use techdepo_api_types::vocab::OrgRole;
use techdepo_db::{Db, OrgScope, PageRequest};
use techdepo_domain::OrgId;
use techdepo_ports::{DirectoryError, Member, MemberDirectory, MemberRole};

/// How long a membership list is trusted.
pub const CACHE_TTL_SECS: u64 = 600;

fn role(role: OrgRole) -> MemberRole {
    match role {
        OrgRole::Admin => MemberRole::Admin,
        OrgRole::Member => MemberRole::Member,
    }
}

fn unavailable(e: impl std::fmt::Display) -> DirectoryError {
    DirectoryError::Unavailable(e.to_string())
}

/// `AUTH_MODE=dev`: members are the `org_members` rows the dev login wrote.
pub struct D1Directory {
    db: Db,
}

impl D1Directory {
    /// The directory over `db`.
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MemberDirectory for D1Directory {
    async fn is_member(&self, org: &OrgId, email: &str) -> Result<Option<Member>, DirectoryError> {
        let scope = OrgScope::verified(org.clone());
        let row = self
            .db
            .orgs(&scope)
            .member_by_email(email)
            .await
            .map_err(unavailable)?;
        Ok(row.map(|m| Member {
            user_id: m.user_id,
            email: m.email,
            name: m.name,
            role: role(m.role),
        }))
    }

    async fn list_members(&self, org: &OrgId) -> Result<Vec<Member>, DirectoryError> {
        let scope = OrgScope::verified(org.clone());
        let mut out = Vec::new();
        let mut page = PageRequest::first();
        loop {
            let (rows, next) = self
                .db
                .orgs(&scope)
                .members(None, &page)
                .await
                .map_err(unavailable)?;
            out.extend(rows.into_iter().map(|m| Member {
                user_id: m.user_id,
                email: m.email,
                name: m.name,
                role: role(m.role),
            }));
            match next {
                Some(cursor) => page = PageRequest::from_query(Some(&cursor), None),
                None => return Ok(out),
            }
        }
    }
}

/// Finds `email` among `members` (case-insensitive).
pub fn find_member(members: &[Member], email: &str) -> Option<Member> {
    let email = email.trim().to_lowercase();
    members.iter().find(|m| m.email == email).cloned()
}

/// Clerk's `GET /v1/organizations/{id}/memberships` response (the fields we read).
#[derive(Debug, serde::Deserialize)]
pub struct ClerkMemberships {
    /// One page of memberships.
    pub data: Vec<ClerkMembership>,
    /// All memberships.
    #[serde(default)]
    pub total_count: u64,
}

/// One membership.
#[derive(Debug, serde::Deserialize)]
pub struct ClerkMembership {
    /// `org:admin`, `org:member` or a custom role.
    pub role: String,
    /// The user.
    pub public_user_data: ClerkPublicUser,
}

/// The membership's user.
#[derive(Debug, serde::Deserialize)]
pub struct ClerkPublicUser {
    /// User ID.
    pub user_id: String,
    /// The primary identifier (the email address for email sign-ins).
    #[serde(default)]
    pub identifier: Option<String>,
    /// First name.
    #[serde(default)]
    pub first_name: Option<String>,
    /// Last name.
    #[serde(default)]
    pub last_name: Option<String>,
}

impl ClerkMemberships {
    /// The members with an email identifier.
    pub fn members(self) -> Vec<Member> {
        self.data
            .into_iter()
            .filter_map(|m| {
                let email = m
                    .public_user_data
                    .identifier
                    .filter(|i| i.contains('@'))?
                    .to_lowercase();
                let name = [m.public_user_data.first_name, m.public_user_data.last_name]
                    .into_iter()
                    .flatten()
                    .filter(|n| !n.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                Some(Member {
                    user_id: techdepo_domain::UserId::new(m.public_user_data.user_id),
                    email,
                    name: (!name.is_empty()).then_some(name),
                    role: MemberRole::from_clerk(&m.role),
                })
            })
            .collect()
    }
}

#[cfg(target_arch = "wasm32")]
pub use clerk::ClerkDirectory;

#[cfg(target_arch = "wasm32")]
mod clerk {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use async_trait::async_trait;
    use send_wrapper::SendWrapper;
    use techdepo_domain::OrgId;
    use techdepo_ports::{DirectoryError, Member, MemberDirectory};
    use worker::kv::KvStore;
    use worker::{Fetch, Headers, Method, Request, RequestInit};

    use super::{CACHE_TTL_SECS, ClerkMemberships, find_member, unavailable};

    thread_local! {
        /// Isolate-local cache: org → (expires at epoch ms, members).
        static MEMORY: RefCell<HashMap<String, (f64, Vec<Member>)>> = RefCell::new(HashMap::new());
    }

    /// `AUTH_MODE=clerk`: Clerk's org memberships, cached.
    pub struct ClerkDirectory {
        api_url: String,
        secret_key: String,
        kv: SendWrapper<Option<KvStore>>,
    }

    impl ClerkDirectory {
        /// A directory using `secret_key` against `api_url` (`https://api.clerk.com/v1`), with
        /// an optional KV cache.
        pub fn new(api_url: String, secret_key: String, kv: Option<KvStore>) -> Self {
            Self {
                api_url: api_url.trim_end_matches('/').to_owned(),
                secret_key,
                kv: SendWrapper::new(kv),
            }
        }

        async fn fetch(&self, org: &OrgId) -> Result<Vec<Member>, DirectoryError> {
            let url = format!(
                "{}/organizations/{}/memberships?limit=500",
                self.api_url,
                org.as_str()
            );
            let key = self.secret_key.clone();
            SendWrapper::new(async move {
                let headers = Headers::new();
                headers
                    .set("authorization", &format!("Bearer {key}"))
                    .map_err(unavailable)?;
                let mut init = RequestInit::new();
                init.with_method(Method::Get).with_headers(headers);
                let request = Request::new_with_init(&url, &init).map_err(unavailable)?;
                let mut response = Fetch::Request(request).send().await.map_err(unavailable)?;
                match response.status_code() {
                    200 => {}
                    401 | 403 => {
                        return Err(DirectoryError::Config(
                            "Clerk rejected CLERK_SECRET_KEY".to_owned(),
                        ));
                    }
                    // An unknown org has no members.
                    404 => return Ok(Vec::new()),
                    status => {
                        return Err(DirectoryError::Unavailable(format!(
                            "Clerk answered {status}"
                        )));
                    }
                }
                let body: ClerkMemberships = response.json().await.map_err(unavailable)?;
                Ok(body.members())
            })
            .await
        }

        async fn members(&self, org: &OrgId) -> Result<Vec<Member>, DirectoryError> {
            let now = worker::Date::now().as_millis() as f64;
            if let Some(members) = MEMORY.with(|m| {
                m.borrow()
                    .get(org.as_str())
                    .filter(|(expires, _)| *expires > now)
                    .map(|(_, members)| members.clone())
            }) {
                return Ok(members);
            }
            let cache_key = format!("members:{}", org.as_str());
            if let Some(kv) = self.kv.as_ref() {
                let get = SendWrapper::new(kv.get(&cache_key).json::<Vec<Member>>());
                if let Ok(Some(members)) = get.await {
                    remember(org, now, &members);
                    return Ok(members);
                }
            }
            let members = self.fetch(org).await?;
            remember(org, now, &members);
            if let Some(kv) = self.kv.as_ref()
                && let Ok(put) = kv.put(&cache_key, &members)
            {
                // A cache write failure only costs a refetch.
                let _ = SendWrapper::new(put.expiration_ttl(CACHE_TTL_SECS).execute()).await;
            }
            Ok(members)
        }
    }

    fn remember(org: &OrgId, now: f64, members: &[Member]) {
        #[allow(clippy::cast_precision_loss)]
        let expires = now + (CACHE_TTL_SECS * 1000) as f64;
        MEMORY.with(|m| {
            m.borrow_mut()
                .insert(org.as_str().to_owned(), (expires, members.to_vec()));
        });
    }

    #[async_trait]
    impl MemberDirectory for ClerkDirectory {
        async fn is_member(
            &self,
            org: &OrgId,
            email: &str,
        ) -> Result<Option<Member>, DirectoryError> {
            Ok(find_member(&self.members(org).await?, email))
        }

        async fn list_members(&self, org: &OrgId) -> Result<Vec<Member>, DirectoryError> {
            self.members(org).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clerk_memberships() {
        let body: ClerkMemberships = serde_json::from_value(serde_json::json!({
            "data": [
                { "role": "org:admin", "public_user_data": { "user_id": "user_1", "identifier": "Ada@Example.com", "first_name": "Ada", "last_name": "Lovelace" } },
                { "role": "org:member", "public_user_data": { "user_id": "user_2", "identifier": "+3612345678" } },
                { "role": "org:billing", "public_user_data": { "user_id": "user_3", "identifier": "c@example.com" } }
            ],
            "total_count": 3
        }))
        .unwrap();
        let members = body.members();
        assert_eq!(members.len(), 2, "phone-only users have no email");
        assert_eq!(members[0].name.as_deref(), Some("Ada Lovelace"));
        assert_eq!(members[0].role, MemberRole::Admin);
        assert_eq!(members[1].role, MemberRole::Member);
        assert_eq!(
            find_member(&members, " ADA@example.com")
                .unwrap()
                .user_id
                .as_str(),
            "user_1"
        );
        assert!(find_member(&members, "eve@example.com").is_none());
    }
}
