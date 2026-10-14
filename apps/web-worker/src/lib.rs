//! `techdepo-web`: the `/api/*` routes of `docs/plan/api.md` plus the SPA as Static Assets.
//!
//! - **Routing:** every route of `techdepo_api_types::endpoints::ALL` is mounted from its typed
//!   `Endpoint` ([`routes`], [`endpoint`]); a test checks none is missing.
//! - **Reads** go straight to D1 through `techdepo_db` repositories and views, scoped to the
//!   caller's verified org (and user, for customer routes).
//! - **Mutations** validate the body, call the Restate handler through ingress ([`ingress`]),
//!   then read fresh data back from D1 for the response. Pure computations (catalog,
//!   compatibility) run in-process.
//! - **Auth** ([`auth`]): Clerk session JWTs (`AUTH_MODE=clerk`) or unsigned dev tokens
//!   (`AUTH_MODE=dev`); the org is provisioned on first sight and the member cache refreshed.
//! - **Rate limits** ([`throttle`]): a per-user burst limit on every mutation and a per-org
//!   hourly order cap (`429 rate_limited`).
//! - **Static Assets:** wrangler serves the files in `public/` (the SPA build) without running
//!   the worker. `/api/*` always runs the worker (`run_worker_first`); any other path that
//!   matched no file reaches it too, and [`spa::fallback`] decides: `index.html` for an
//!   extension-less page path, `404` for a missing file.

pub mod app;
mod assemble;
pub mod auth;
pub mod clerk;
pub mod endpoint;
pub mod error;
pub mod ingress;
pub mod routes;
pub mod settings;
pub mod spa;
pub mod throttle;

#[cfg(target_arch = "wasm32")]
mod worker_entry {
    use std::cell::RefCell;
    use std::sync::Arc;

    use dioxus_clerk::server::{ClerkAuthLayer, ClerkAuthLayerConfig};
    use techdepo_api_types::session::AuthMode;
    use techdepo_api_types::{ApiError, ErrorCode};
    use techdepo_db::{D1Executor, Db, Retrying};
    use tower_service::Service;
    use worker::{Context, Env, HttpRequest, Result, event};

    use crate::app::App;
    use crate::error::{error_response, log};
    use crate::routes;
    use crate::settings::Settings;
    use crate::spa::{self, Fallback};
    use crate::throttle::Throttle;

    thread_local! {
        /// One verifier per isolate, so its JWKS cache outlives a request.
        static CLERK: RefCell<Option<ClerkAuthLayer>> = const { RefCell::new(None) };
    }

    fn clerk_layer(settings: &Settings) -> std::result::Result<ClerkAuthLayer, String> {
        if let Some(layer) = CLERK.with(|c| c.borrow().clone()) {
            return Ok(layer);
        }
        let secret = settings.clerk_secret_key.clone().unwrap_or_default();
        let mut config = ClerkAuthLayerConfig::new(secret);
        if !settings.clerk_authorized_parties.is_empty() {
            config = config.with_authorized_parties(settings.clerk_authorized_parties.clone());
        }
        let layer = ClerkAuthLayer::from_config(config).map_err(|e| e.to_string())?;
        CLERK.with(|c| *c.borrow_mut() = Some(layer.clone()));
        Ok(layer)
    }

    /// A non-`/api` request that matched no static asset (see [`spa`]).
    async fn not_an_asset(
        req: HttpRequest,
        env: &Env,
    ) -> Result<axum::http::Response<axum::body::Body>> {
        let uri = req.uri();
        if spa::fallback(req.method(), uri.path()) == Fallback::NotFound {
            let mut response = axum::http::Response::new(axum::body::Body::from("Not found"));
            *response.status_mut() = axum::http::StatusCode::NOT_FOUND;
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            return Ok(response);
        }
        // The shell: `/` is `index.html` to the assets binding.
        let index = format!(
            "{}://{}/",
            uri.scheme_str().unwrap_or("https"),
            uri.authority().map_or("localhost", |a| a.as_str()),
        );
        let response = env.assets("ASSETS")?.fetch(index, None).await?;
        let (parts, body) = response.into_parts();
        Ok(axum::http::Response::from_parts(
            parts,
            axum::body::Body::new(body),
        ))
    }

    fn misconfigured(message: &str) -> axum::http::Response<axum::body::Body> {
        log(&format!("techdepo-web misconfigured: {message}"));
        error_response(&ApiError::new(
            ErrorCode::Internal,
            "The API is misconfigured. Check the worker's logs.",
        ))
    }

    #[event(fetch)]
    async fn fetch(
        req: HttpRequest,
        env: Env,
        _ctx: Context,
    ) -> Result<axum::http::Response<axum::body::Body>> {
        let path = req.uri().path();
        if path != "/api" && !path.starts_with("/api/") {
            return not_an_asset(req, &env).await;
        }
        let settings = match Settings::from_env(&env) {
            Ok(settings) => settings,
            Err(message) => return Ok(misconfigured(&message)),
        };
        let db = match env.d1("DB") {
            Ok(db) => Db::new(Arc::new(Retrying::new(D1Executor::new(db)))),
            Err(e) => return Ok(misconfigured(&format!("D1 binding `DB`: {e}"))),
        };
        let clerk = match settings.auth_mode {
            AuthMode::Clerk => match clerk_layer(&settings) {
                Ok(layer) => Some(layer),
                Err(message) => return Ok(misconfigured(&message)),
            },
            AuthMode::Dev => None,
        };
        let mut router = routes::router(App::new(settings, db, Throttle::from_env(&env)));
        let response = match clerk {
            Some(layer) => router.layer(layer).call(req).await?,
            None => router.call(req).await?,
        };
        Ok(response)
    }
}
