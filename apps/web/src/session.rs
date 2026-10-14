//! Who is using the SPA: runtime config, the sign-in state and the bearer token.
//!
//! One SPA build runs everywhere because the auth mode comes from `GET /api/config` at
//! startup:
//!
//! - `AUTH_MODE=clerk`: the app is wrapped in `ClerkProvider` (publishable key from the
//!   config) and [`ClerkBridge`] mirrors Clerk's auth state into the [`Session`]. Tokens come
//!   from `use_auth().get_token()` on every request.
//! - `AUTH_MODE=dev`: the dev login (`POST /api/dev/login`) returns an unsigned token, kept in
//!   `localStorage` so a reload stays signed in.
//!
//! Either way the app then calls `GET /api/me` (which provisions the org on first use) and the
//! rest of the UI only looks at [`AuthState`].

use dioxus::prelude::*;
use dioxus_clerk::prelude::{AuthStatus, UseAuth, use_auth};
use techdepo_api_types::endpoints::{GetConfig, GetMe};
use techdepo_api_types::session::{AuthMode, Config, DevLoginResponse, LlmBackend, Me};
use techdepo_api_types::{ApiError, ErrorCode, OrgFeature};

use crate::api::Api;
use crate::browser;

const DEV_TOKEN_KEY: &str = "techdepo.dev_token";

/// Where the sign-in flow stands.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthState {
    /// Config, Clerk or `/api/me` still loading.
    Loading,
    /// Nobody is signed in.
    SignedOut,
    /// Signed in, with the user, org and role from `/api/me`.
    SignedIn(Me),
    /// Signed in, but `/api/me` failed (e.g. `403` without an active Clerk org).
    Failed(ApiError),
}

/// How requests are authenticated.
#[derive(Clone, Default)]
enum Credentials {
    #[default]
    None,
    Dev(String),
    Clerk(UseAuth),
}

/// The session, provided at the app root. Copy it freely.
#[derive(Clone, Copy)]
pub struct Session {
    /// `GET /api/config`; `None` until the first answer (or the offline fallback).
    pub config: Signal<Option<Config>>,
    /// The API could not be reached at startup; the shop still works from the compiled-in
    /// catalog.
    pub offline: Signal<bool>,
    /// The sign-in state.
    pub auth: Signal<AuthState>,
    credentials: Signal<Credentials>,
}

/// The session from context.
pub fn use_session() -> Session {
    use_context::<Session>()
}

impl Session {
    /// A fresh session; call [`Session::bootstrap`] once.
    pub fn new() -> Self {
        Self {
            config: Signal::new(None),
            offline: Signal::new(false),
            auth: Signal::new(AuthState::Loading),
            credentials: Signal::new(Credentials::None),
        }
    }

    /// Loads the config and, in dev mode, restores a saved dev token.
    pub async fn bootstrap(mut self) {
        let api = Api::new(self);
        let config = match api.get::<GetConfig>(&[]).await {
            Ok(config) => config,
            Err(error) => {
                dioxus::logger::tracing::warn!("GET /api/config failed: {error}");
                self.offline.set(true);
                offline_config()
            }
        };
        if config.catalog_version != techdepo_catalog::CATALOG_VERSION {
            dioxus::logger::tracing::warn!(
                "catalog version differs: API {} vs SPA {}",
                config.catalog_version,
                techdepo_catalog::CATALOG_VERSION
            );
        }
        let mode = config.auth_mode;
        self.config.set(Some(config));

        if mode == AuthMode::Dev {
            match browser::load_string(DEV_TOKEN_KEY) {
                Some(token) if !*self.offline.peek() => {
                    self.credentials.set(Credentials::Dev(token));
                    self.refresh_me().await;
                }
                _ => self.auth.set(AuthState::SignedOut),
            }
        }
        // Clerk mode: `ClerkBridge` takes over once the provider has loaded.
    }

    /// The auth mode, once the config is known.
    pub fn auth_mode(&self) -> Option<AuthMode> {
        self.config.read().as_ref().map(|c| c.auth_mode)
    }

    /// The signed-in user, org and role.
    pub fn me(&self) -> Option<Me> {
        match &*self.auth.read() {
            AuthState::SignedIn(me) => Some(me.clone()),
            _ => None,
        }
    }

    /// Whether someone is signed in (and `/api/me` answered).
    pub fn is_signed_in(&self) -> bool {
        matches!(*self.auth.read(), AuthState::SignedIn(_))
    }

    /// Whether the signed-in user is an `org:admin`.
    pub fn is_admin(&self) -> bool {
        self.me().is_some_and(|me| me.is_admin())
    }

    /// Whether the signed-in user's org has `feature`.
    pub fn has_feature(&self, feature: OrgFeature) -> bool {
        self.me().is_some_and(|me| me.org.has(feature))
    }

    /// Calls `/api/me` with the current credentials and updates [`Session::auth`].
    pub async fn refresh_me(mut self) {
        self.auth.set(AuthState::Loading);
        match Api::new(self).get::<GetMe>(&[]).await {
            Ok(me) => self.auth.set(AuthState::SignedIn(me)),
            Err(error) if error.code == ErrorCode::Unauthenticated => {
                self.auth.set(AuthState::SignedOut)
            }
            Err(error) => self.auth.set(AuthState::Failed(error)),
        }
    }

    /// Completes a dev login.
    pub fn sign_in_dev(mut self, response: DevLoginResponse) {
        browser::save_string(DEV_TOKEN_KEY, &response.token);
        self.credentials.set(Credentials::Dev(response.token));
        self.auth.set(AuthState::SignedIn(response.me));
    }

    /// Signs out (dev: forgets the token; Clerk: ends the Clerk session).
    pub fn sign_out(mut self) {
        let credentials = self.credentials.peek().clone();
        match credentials {
            Credentials::Clerk(auth) => auth.sign_out(),
            Credentials::Dev(_) | Credentials::None => {
                browser::remove(DEV_TOKEN_KEY);
                self.credentials.set(Credentials::None);
                self.auth.set(AuthState::SignedOut);
            }
        }
    }

    /// The `Authorization: Bearer` token for the next request.
    pub async fn bearer(&self) -> Option<String> {
        let credentials = self.credentials.peek().clone();
        match credentials {
            Credentials::None => None,
            Credentials::Dev(token) => Some(token),
            Credentials::Clerk(auth) => auth.get_token().await.ok().flatten(),
        }
    }

    /// A request came back `401`: the dev token is stale (e.g. after a mode switch).
    pub fn on_unauthenticated(mut self) {
        let is_dev = matches!(*self.credentials.peek(), Credentials::Dev(_));
        if is_dev {
            browser::remove(DEV_TOKEN_KEY);
            self.credentials.set(Credentials::None);
            self.auth.set(AuthState::SignedOut);
        }
    }
}

/// What the SPA assumes when `/api/config` is unreachable: browse-only, dev auth.
fn offline_config() -> Config {
    Config {
        auth_mode: AuthMode::Dev,
        clerk_publishable_key: None,
        support_domain: "techdepo.dev".to_owned(),
        catalog_version: techdepo_catalog::CATALOG_VERSION.to_owned(),
        llm_backend: LlmBackend::Mock,
        restate_ui_url: None,
    }
}

/// The Clerk identity the session follows; changes trigger a new `/api/me`.
#[derive(Debug, Clone, PartialEq)]
enum ClerkIdentity {
    Loading,
    SignedOut,
    SignedIn {
        user: Option<String>,
        org: Option<String>,
    },
}

/// Mirrors Clerk's auth state into the [`Session`]. Render once, inside `ClerkProvider`.
#[component]
pub fn ClerkBridge() -> Element {
    let auth = use_auth();
    let mut session = use_session();
    let identity = use_memo(move || match auth.status() {
        AuthStatus::SignedOut => ClerkIdentity::SignedOut,
        AuthStatus::SignedIn => ClerkIdentity::SignedIn {
            user: auth.user_id(),
            org: auth.org_id(),
        },
        _ => ClerkIdentity::Loading,
    });

    use_effect(move || match identity() {
        ClerkIdentity::Loading => session.auth.set(AuthState::Loading),
        ClerkIdentity::SignedOut => {
            session.credentials.set(Credentials::None);
            session.auth.set(AuthState::SignedOut);
        }
        ClerkIdentity::SignedIn { .. } => {
            session.credentials.set(Credentials::Clerk(auth));
            spawn(session.refresh_me());
        }
    });

    rsx! {}
}
