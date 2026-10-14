//! `AUTH_MODE=clerk`: the session JWT is verified by dioxus-clerk's `ClerkAuthLayer` (JWKS,
//! cached per isolate); the profile data the token does not carry (email, name, org name) comes
//! from the Clerk Backend API, and only when the member cache or the `orgs` row lacks it.

use serde::Deserialize;

/// The parts of a Clerk user the member cache needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Primary email address (lower case).
    pub email: String,
    /// Display name.
    pub name: Option<String>,
}

#[derive(Deserialize)]
struct UserJson {
    #[serde(default)]
    primary_email_address_id: Option<String>,
    #[serde(default)]
    email_addresses: Vec<EmailJson>,
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
    #[serde(default)]
    username: Option<String>,
}

#[derive(Deserialize)]
struct EmailJson {
    id: String,
    email_address: String,
}

#[derive(Deserialize)]
struct OrgJson {
    name: String,
}

/// Parses `GET /v1/users/{id}`.
pub fn parse_user(json: &str) -> Option<Profile> {
    let user: UserJson = serde_json::from_str(json).ok()?;
    let email = user
        .email_addresses
        .iter()
        .find(|e| Some(&e.id) == user.primary_email_address_id.as_ref())
        .or_else(|| user.email_addresses.first())?
        .email_address
        .to_lowercase();
    let full = [user.first_name, user.last_name]
        .into_iter()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let name = if full.is_empty() {
        user.username
    } else {
        Some(full)
    };
    Some(Profile { email, name })
}

/// Parses `GET /v1/organizations/{id}`.
pub fn parse_org_name(json: &str) -> Option<String> {
    serde_json::from_str::<OrgJson>(json).ok().map(|o| o.name)
}

/// The Clerk Backend API.
#[derive(Debug, Clone)]
pub struct BackendApi {
    secret_key: String,
}

const BASE_URL: &str = "https://api.clerk.com/v1";

impl BackendApi {
    /// A client authenticated with `CLERK_SECRET_KEY`.
    pub fn new(secret_key: String) -> Self {
        Self { secret_key }
    }

    /// The user's primary email and name.
    pub async fn user(&self, user_id: &str) -> Option<Profile> {
        let body = self.get(&format!("{BASE_URL}/users/{user_id}")).await?;
        parse_user(&body)
    }

    /// The organization's name.
    pub async fn org_name(&self, org_id: &str) -> Option<String> {
        let body = self
            .get(&format!("{BASE_URL}/organizations/{org_id}"))
            .await?;
        parse_org_name(&body)
    }

    #[cfg(target_arch = "wasm32")]
    async fn get(&self, url: &str) -> Option<String> {
        use send_wrapper::SendWrapper;
        use worker::{Fetch, Headers, Method, Request, RequestInit};

        let url = url.to_owned();
        let auth = format!("Bearer {}", self.secret_key);
        let result: worker::Result<(u16, String)> = SendWrapper::new(async move {
            let headers = Headers::new();
            headers.set("authorization", &auth)?;
            let mut init = RequestInit::new();
            init.with_method(Method::Get).with_headers(headers);
            let request = Request::new_with_init(&url, &init)?;
            let mut response = Fetch::Request(request).send().await?;
            Ok((
                response.status_code(),
                response.text().await.unwrap_or_default(),
            ))
        })
        .await;
        match result {
            Ok((200, body)) => Some(body),
            Ok((status, body)) => {
                crate::error::log(&format!("Clerk Backend API {status}: {body}"));
                None
            }
            Err(e) => {
                crate::error::log(&format!("Clerk Backend API: {e}"));
                None
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn get(&self, _url: &str) -> Option<String> {
        let _ = &self.secret_key;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_primary_email() {
        let json = r#"{"id":"user_1","primary_email_address_id":"idn_2",
            "email_addresses":[{"id":"idn_1","email_address":"old@x.io"},{"id":"idn_2","email_address":"Jane@Example.com"}],
            "first_name":"Jane","last_name":null,"username":"jd"}"#;
        assert_eq!(
            parse_user(json),
            Some(Profile {
                email: "jane@example.com".into(),
                name: Some("Jane".into())
            })
        );
        assert_eq!(
            parse_org_name(r#"{"id":"org_1","name":"Acme"}"#).as_deref(),
            Some("Acme")
        );
    }
}
