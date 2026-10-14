//! Mounting typed endpoints: every route is an `techdepo_api_types::Endpoint`, and its method,
//! path, access, query, body and response types come from there.
//!
//! [`public`] and [`authed`] turn an async function into an axum `MethodRouter` that
//! - decodes the path parameters, the query (`E::Query`) and the JSON body (`E::Body`), with
//!   `400 bad_request` for anything malformed;
//! - enforces `E::ROUTE.access`: `DevOnly` is `404` outside `AUTH_MODE=dev`, `Member` needs a
//!   verified identity (`401`; `403` without an active org), `Admin` also `org:admin` (`403`),
//!   `Operator` also an org with `OrgFeature::Muhaha` (`404` otherwise, like an unknown route);
//! - counts a mutation against the user's burst limit (`429`, [`crate::throttle`]) before the
//!   caller is resolved;
//! - answers `200` with the response as JSON, or the [`ApiError`] with its status.

use std::future::Future;

use axum::body::Bytes;
use axum::extract::{Query, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, delete, get, post, put};
use dioxus_clerk::server::VerificationOutcome;
use serde::de::DeserializeOwned;
use techdepo_api_types::session::AuthMode;
use techdepo_api_types::vocab::{MemberSource, OrgRole};
use techdepo_api_types::{Access, ApiError, Endpoint, ErrorCode, Method, OrgFeature};

use crate::app::{App, now};
use crate::auth::{self, Caller, DevClaims, Identity};
use crate::error::{ApiResult, bad_request, error_response};

/// Largest JSON body accepted.
const MAX_BODY_BYTES: usize = 256 * 1024;

/// A decoded request of endpoint `E`.
#[derive(Debug)]
pub struct Input<E: Endpoint> {
    /// Path parameters, in pattern order, percent-decoded.
    pub params: Vec<String>,
    /// Query parameters.
    pub query: E::Query,
    /// JSON body.
    pub body: E::Body,
    /// Request headers.
    pub headers: HeaderMap,
}

impl<E: Endpoint> Input<E> {
    /// The `i`-th path parameter (empty if absent, which routing rules out).
    pub fn param(&self, i: usize) -> &str {
        self.params.get(i).map_or("", String::as_str)
    }
}

/// A route anyone may call (`Public`, `DevOnly`).
pub fn public<E, F, Fut>(f: F) -> MethodRouter<App>
where
    E: Endpoint + Send + 'static,
    E::Query: Send,
    E::Body: Send,
    F: Fn(App, Input<E>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = ApiResult<E::Response>> + Send + 'static,
{
    debug_assert!(matches!(E::ROUTE.access, Access::Public | Access::DevOnly));
    let handler = move |State(app): State<App>, req: Request| {
        let f = f.clone();
        async move {
            let result = async {
                if E::ROUTE.access == Access::DevOnly && app.settings.auth_mode != AuthMode::Dev {
                    return Err(not_found_route());
                }
                let (_, input) = decode::<E>(req).await?;
                f(app, input).await
            }
            .await;
            respond(result)
        }
    };
    method::<E, _, _>(handler)
}

/// A route for signed-in members (`Member`), admins (`Admin`) or the operator (`Operator`).
pub fn authed<E, F, Fut>(f: F) -> MethodRouter<App>
where
    E: Endpoint + Send + 'static,
    E::Query: Send,
    E::Body: Send,
    F: Fn(App, Caller, Input<E>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = ApiResult<E::Response>> + Send + 'static,
{
    debug_assert!(E::ROUTE.access.needs_token());
    let handler = move |State(app): State<App>, req: Request| {
        let f = f.clone();
        async move {
            let result = async {
                let (parts, input) = decode::<E>(req).await?;
                let identity = identity(&app, &parts)?;
                check_access(E::ROUTE.access, identity.role.is_admin(), None)?;
                app.throttle
                    .check(
                        &E::ROUTE,
                        &format!("{}:{}", identity.org_id, identity.user_id),
                    )
                    .await?;
                let caller = auth::resolve(&app, identity, now()).await?;
                check_access(
                    E::ROUTE.access,
                    caller.role.is_admin(),
                    Some(&caller.org.features),
                )?;
                f(app, caller, input).await
            }
            .await;
            respond(result)
        }
    };
    method::<E, _, _>(handler)
}

fn method<E, H, T>(handler: H) -> MethodRouter<App>
where
    E: Endpoint,
    H: axum::handler::Handler<T, App>,
    T: 'static,
{
    match E::ROUTE.method {
        Method::Get => get(handler),
        Method::Post => post(handler),
        Method::Put => put(handler),
        Method::Delete => delete(handler),
    }
}

fn respond<T: serde::Serialize>(result: ApiResult<T>) -> Response {
    match result {
        Ok(body) => axum::Json(body).into_response(),
        Err(error) => error_response(&error),
    }
}

/// `404` for an unknown route (or a dev-only route outside dev mode).
pub fn not_found_route() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "No such API route")
}

/// The role check of an authenticated route, run before the caller is resolved (`features`
/// unknown: `None`, so nobody is provisioned for a route they may not call) and again after,
/// with the org's features. An operator route is `404` for anyone else, like an unknown route.
fn check_access(access: Access, admin: bool, features: Option<&[OrgFeature]>) -> ApiResult<()> {
    match access {
        Access::Operator
            if !admin || features.is_some_and(|f| !f.contains(&OrgFeature::Muhaha)) =>
        {
            Err(not_found_route())
        }
        Access::Admin if !admin => Err(ApiError::new(
            ErrorCode::Forbidden,
            "Only shop admins can do that.",
        )),
        _ => Ok(()),
    }
}

async fn decode<E: Endpoint>(req: Request) -> ApiResult<(Parts, Input<E>)> {
    let (parts, body) = req.into_parts();
    let params = path_params(E::ROUTE.path, parts.uri.path())?;
    let query = decode_query::<E::Query>(&parts.uri)?;
    let body = if E::ROUTE.method.has_body() {
        let bytes = axum::body::to_bytes(body, MAX_BODY_BYTES)
            .await
            .map_err(|_| bad_request("The request body is too large or unreadable"))?;
        decode_body::<E::Body>(&bytes)?
    } else {
        decode_body::<E::Body>(&Bytes::new())?
    };
    let headers = parts.headers.clone();
    Ok((
        parts,
        Input {
            params,
            query,
            body,
            headers,
        },
    ))
}

fn decode_query<Q: DeserializeOwned>(uri: &Uri) -> ApiResult<Q> {
    if uri.query().is_none_or(str::is_empty) {
        return serde_json::from_str("{}")
            .map_err(|e| bad_request(format!("Missing query parameters: {e}")));
    }
    Query::<Q>::try_from_uri(uri)
        .map(|Query(q)| q)
        .map_err(|e| bad_request(format!("Invalid query: {}", e.body_text())))
}

fn decode_body<B: DeserializeOwned>(bytes: &[u8]) -> ApiResult<B> {
    let bytes: &[u8] = if bytes.iter().all(u8::is_ascii_whitespace) {
        b"{}"
    } else {
        bytes
    };
    serde_json::from_slice(bytes).map_err(|e| bad_request(format!("Invalid JSON body: {e}")))
}

/// The values of `pattern`'s `{param}` segments in `path`, percent-decoded.
pub fn path_params(pattern: &str, path: &str) -> ApiResult<Vec<String>> {
    let mut out = Vec::new();
    for (p, s) in pattern.split('/').zip(path.split('/')) {
        if p.starts_with('{') && p.ends_with('}') {
            let value = percent_decode(s).ok_or_else(|| bad_request("Malformed path"))?;
            out.push(value);
        }
    }
    Ok(out)
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The verified identity of a request.
fn identity(app: &App, parts: &Parts) -> ApiResult<Identity> {
    let unauthenticated = || ApiError::new(ErrorCode::Unauthenticated, "Sign in to continue");
    match app.settings.auth_mode {
        AuthMode::Dev => bearer(&parts.headers)
            .and_then(DevClaims::from_token)
            .map(Identity::from)
            .ok_or_else(unauthenticated),
        AuthMode::Clerk => match parts.extensions.get::<VerificationOutcome>() {
            Some(VerificationOutcome::Valid(auth)) => {
                let org_id = auth.org_id.clone().ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::Forbidden,
                        "Choose a shop (organization) to continue.",
                    )
                })?;
                Ok(Identity {
                    org_id: org_id.into(),
                    user_id: auth.user_id.clone().into(),
                    role: OrgRole::from_clerk(auth.org_role.as_deref().unwrap_or_default()),
                    email: None,
                    name: None,
                    org_name: None,
                    source: MemberSource::Clerk,
                })
            }
            Some(VerificationOutcome::Unavailable) => Err(ApiError::new(
                ErrorCode::Unavailable,
                "Sign-in could not be verified right now. Try again in a moment.",
            )),
            _ => Err(unauthenticated()),
        },
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_params_are_decoded() {
        assert_eq!(
            path_params(
                "/api/orders/{number}/invoices/{invoice}",
                "/api/orders/TD-1042/invoices/TD-2026-000001"
            )
            .unwrap(),
            ["TD-1042", "TD-2026-000001"]
        );
        assert_eq!(
            path_params("/api/products/{id}", "/api/products/a%20b").unwrap(),
            ["a b"]
        );
        assert!(path_params("/api/products/{id}", "/api/products/%zz").is_err());
    }

    #[test]
    fn operator_routes_are_unknown_to_everyone_else() {
        let muhaha = [OrgFeature::Muhaha];
        let code = |access, admin, features| {
            check_access(access, admin, features)
                .err()
                .map(|e: ApiError| e.code)
        };
        // Members, before and after resolving.
        assert_eq!(
            code(Access::Operator, false, None),
            Some(ErrorCode::NotFound)
        );
        assert_eq!(
            code(Access::Operator, false, Some(&muhaha[..])),
            Some(ErrorCode::NotFound)
        );
        // Admins of an org without the feature.
        assert_eq!(code(Access::Operator, true, None), None);
        assert_eq!(
            code(Access::Operator, true, Some(&[])),
            Some(ErrorCode::NotFound)
        );
        assert_eq!(code(Access::Operator, true, Some(&muhaha[..])), None);
        // The other levels are unchanged.
        assert_eq!(code(Access::Admin, false, None), Some(ErrorCode::Forbidden));
        assert_eq!(code(Access::Admin, true, Some(&[])), None);
        assert_eq!(code(Access::Member, false, Some(&[])), None);
    }

    #[test]
    fn empty_bodies_are_empty_objects() {
        let _: techdepo_api_types::Empty = decode_body(b"").unwrap();
        let err = decode_body::<techdepo_api_types::session::DevLoginRequest>(b"{").unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
    }

    #[test]
    fn queries_decode() {
        use techdepo_api_types::orders::OrderListQuery;
        let uri: Uri = "/api/orders?include_cancelled=true&limit=5&status=shipped"
            .parse()
            .unwrap();
        let q: OrderListQuery = decode_query(&uri).unwrap();
        assert_eq!(q.include_cancelled, Some(true));
        assert_eq!(q.limit, Some(5));
        let none: OrderListQuery = decode_query(&"/api/orders".parse().unwrap()).unwrap();
        assert_eq!(none, OrderListQuery::default());
        assert!(decode_query::<OrderListQuery>(&"/api/orders?limit=x".parse().unwrap()).is_err());
    }
}
