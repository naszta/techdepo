//! Route descriptors and the typed [`Endpoint`] trait.

use std::fmt::Write as _;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// HTTP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    /// `GET`: no body.
    Get,
    /// `POST`: JSON body.
    Post,
    /// `PUT`: JSON body.
    Put,
    /// `DELETE`: no body.
    Delete,
}

impl Method {
    /// Upper-case method name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }

    /// Whether requests carry a JSON body.
    pub const fn has_body(self) -> bool {
        matches!(self, Self::Post | Self::Put)
    }
}

/// Who may call a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Access {
    /// Anyone, without a token.
    Public,
    /// Anyone, without a token, but only when `AUTH_MODE=dev`; `404` otherwise.
    DevOnly,
    /// A signed-in member of the active org (any role). Data is scoped to the org, and
    /// customer routes are further scoped to the caller's own records.
    Member,
    /// A signed-in `org:admin` of the active org; `403` for other members.
    Admin,
    /// The operator: an `org:admin` of an org with `OrgFeature::Muhaha`. Acts across every
    /// org and on app-wide settings. `404` for everyone else, so the routes stay unknown.
    Operator,
}

impl Access {
    /// Whether the route needs a signed-in caller.
    pub const fn needs_token(self) -> bool {
        matches!(self, Self::Member | Self::Admin | Self::Operator)
    }
}

/// One route: method, path pattern and access.
///
/// Path parameters use axum 0.8 syntax (`/api/orders/{number}`), so `path` can be passed to
/// `axum::Router::route` as is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Route {
    /// Endpoint type name, e.g. `GetOrder`.
    pub name: &'static str,
    /// HTTP method.
    pub method: Method,
    /// Path pattern.
    pub path: &'static str,
    /// Who may call it.
    pub access: Access,
    /// The Restate handler a mutation calls (`Service/handler`, see [`crate::ingress`]), or
    /// `None` for reads, which go straight to D1.
    pub restate: Option<&'static str>,
}

impl Route {
    /// The names of the path parameters, in order.
    pub fn params(&self) -> impl Iterator<Item = &'static str> {
        self.path
            .split('/')
            .filter_map(|seg| seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
    }

    /// The concrete path with `params` substituted in order and percent-encoded, e.g.
    /// `GetOrder::ROUTE.url(&["TD-1042"])` is `/api/orders/TD-1042`.
    ///
    /// # Panics
    ///
    /// If the number of `params` does not match the pattern (a programming error).
    pub fn url(&self, params: &[&str]) -> String {
        let mut out = String::with_capacity(self.path.len() + 16);
        let mut values = params.iter();
        for (i, seg) in self.path.split('/').enumerate() {
            if i > 0 {
                out.push('/');
            }
            if seg.starts_with('{') && seg.ends_with('}') {
                let value = values
                    .next()
                    .unwrap_or_else(|| panic!("missing path parameter {seg} for {}", self.name));
                percent_encode_segment(&mut out, value);
            } else {
                out.push_str(seg);
            }
        }
        assert!(
            values.next().is_none(),
            "too many path parameters for {}",
            self.name
        );
        out
    }
}

fn percent_encode_segment(out: &mut String, value: &str) {
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            // Writing to a String cannot fail.
            let _ = write!(out, "%{b:02X}");
        }
    }
}

/// A typed endpoint: its route plus the query, body and response types.
///
/// A typed client can be written once against this trait:
///
/// ```ignore
/// async fn call<E: Endpoint>(params: &[&str], query: &E::Query, body: &E::Body)
///     -> Result<E::Response, ApiError>;
/// ```
pub trait Endpoint {
    /// Method, path and access.
    const ROUTE: Route;
    /// Query string parameters ([`Empty`] when there are none).
    type Query: Serialize + DeserializeOwned;
    /// JSON body ([`Empty`] for `GET`/`DELETE`, which send no body).
    type Body: Serialize + DeserializeOwned;
    /// JSON body of a `2xx` response.
    type Response: Serialize + DeserializeOwned;
}

/// No query parameters, or an empty JSON object body (`{}`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Route = Route {
        name: "GetThing",
        method: Method::Get,
        path: "/api/things/{id}/parts/{part}",
        access: Access::Member,
        restate: None,
    };

    #[test]
    fn url_substitutes_and_encodes() {
        assert_eq!(R.params().collect::<Vec<_>>(), ["id", "part"]);
        assert_eq!(
            R.url(&["TD-1042", "a b/c"]),
            "/api/things/TD-1042/parts/a%20b%2Fc"
        );
    }

    #[test]
    #[should_panic(expected = "missing path parameter")]
    fn url_panics_on_missing_param() {
        R.url(&["x"]);
    }

    #[test]
    fn empty_is_an_empty_object() {
        assert_eq!(serde_json::to_string(&Empty {}).unwrap(), "{}");
        let _: Empty = serde_json::from_str("{}").unwrap();
    }
}
