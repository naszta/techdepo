//! Typed client for `/api/*`, written once against [`Endpoint`].
//!
//! The route table, DTOs and errors all come from `techdepo-api-types`, so a call reads
//! `api.get::<GetCart>(&[])` and returns `Result<Cart, ApiError>`. Every failure (network,
//! non-2xx, undecodable body) is an [`ApiError`] whose `message` is safe to show.
//!
//! Authenticated routes carry `Authorization: Bearer <token>`: the Clerk session JWT in
//! `AUTH_MODE=clerk`, or the dev token in `AUTH_MODE=dev` (see [`crate::session`]).

use dioxus::prelude::*;
use gloo_net::http::{Request, RequestBuilder, Response};
use serde::Serialize;
use techdepo_api_types::{ApiError, Empty, Endpoint, ErrorCode, Method};

use crate::session::Session;

/// The API client. Cheap to copy; get one with [`use_api`].
#[derive(Clone, Copy)]
pub struct Api {
    session: Session,
}

/// The API client for this component.
pub fn use_api() -> Api {
    Api {
        session: use_context::<Session>(),
    }
}

impl Api {
    /// A client for `session` (for code outside components, e.g. stores).
    pub fn new(session: Session) -> Self {
        Self { session }
    }

    /// `GET`/`DELETE`-style call with no query and no body.
    pub async fn get<E>(self, params: &[&str]) -> Result<E::Response, ApiError>
    where
        E: Endpoint<Query = Empty, Body = Empty>,
    {
        self.call::<E>(params, &Empty {}, &Empty {}).await
    }

    /// A list or other read with query parameters.
    pub async fn query<E>(self, params: &[&str], query: &E::Query) -> Result<E::Response, ApiError>
    where
        E: Endpoint<Body = Empty>,
    {
        self.call::<E>(params, query, &Empty {}).await
    }

    /// A mutation with a JSON body (`POST`/`PUT`) and no query parameters.
    pub async fn send<E>(self, params: &[&str], body: &E::Body) -> Result<E::Response, ApiError>
    where
        E: Endpoint<Query = Empty>,
    {
        self.call::<E>(params, &Empty {}, body).await
    }

    /// The general form: path parameters, query and body.
    pub async fn call<E: Endpoint>(
        self,
        params: &[&str],
        query: &E::Query,
        body: &E::Body,
    ) -> Result<E::Response, ApiError> {
        let route = E::ROUTE;
        let url = route.url(params);
        let pairs = query_pairs(query);

        let mut builder = match route.method {
            Method::Get => Request::get(&url),
            Method::Post => Request::post(&url),
            Method::Put => Request::put(&url),
            Method::Delete => Request::delete(&url),
        }
        .header("accept", "application/json")
        // Through gloo's own parameters: it would append them (even none) to a `?` in the URL
        // as a trailing `&`.
        .query(pairs.iter().map(|(key, value)| (key.as_str(), value)));

        if route.access.needs_token() {
            let Some(token) = self.session.bearer().await else {
                return Err(ApiError::new(
                    ErrorCode::Unauthenticated,
                    "Sign in to continue",
                ));
            };
            builder = builder.header("authorization", &format!("Bearer {token}"));
        }

        let response = send(builder, route.method.has_body().then_some(body)).await?;
        let status = response.status();
        if (200..300).contains(&status) {
            return response.json::<E::Response>().await.map_err(|e| {
                ApiError::new(
                    ErrorCode::Internal,
                    format!("Unexpected response from {}: {e}", route.name),
                )
            });
        }

        let error = match response.json::<ApiError>().await {
            Ok(error) => error,
            Err(_) => ApiError::new(
                ErrorCode::from_status(status),
                ErrorCode::from_status(status).label(),
            ),
        };
        if error.code == ErrorCode::Unauthenticated {
            self.session.on_unauthenticated();
        }
        Err(error)
    }
}

async fn send<B: Serialize>(
    builder: RequestBuilder,
    body: Option<&B>,
) -> Result<Response, ApiError> {
    let request = match body {
        Some(body) => builder.json(body),
        None => builder.build(),
    }
    .map_err(|e| {
        ApiError::new(
            ErrorCode::BadRequest,
            format!("Could not encode request: {e}"),
        )
    })?;
    request.send().await.map_err(|_| {
        ApiError::new(
            ErrorCode::Unavailable,
            "Can't reach the TechDepó API. Check that the web worker is running.",
        )
    })
}

/// `(name, value)` pairs from a query struct, skipping absent (`null`) fields. Sequences repeat
/// the key.
fn query_pairs<Q: Serialize>(query: &Q) -> Vec<(String, String)> {
    let Ok(serde_json::Value::Object(map)) = serde_json::to_value(query) else {
        return Vec::new();
    };
    let mut pairs = Vec::new();
    for (key, value) in map {
        let values = match value {
            serde_json::Value::Array(items) => items,
            other => vec![other],
        };
        for value in values {
            let text = match value {
                serde_json::Value::Null => continue,
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            pairs.push((key.clone(), text));
        }
    }
    pairs
}
