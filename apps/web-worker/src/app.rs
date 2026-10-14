//! Per-request application state shared by every route.

use std::sync::Arc;

use techdepo_api_types::session::SupportMailbox;
use techdepo_catalog::Catalog;
use techdepo_db::Db;
use time::OffsetDateTime;

use crate::clerk::BackendApi;
use crate::ingress::Ingress;
use crate::settings::Settings;
use crate::throttle::Throttle;

/// What every handler needs: configuration, D1, the Restate ingress and Clerk.
#[derive(Clone)]
pub struct App {
    /// Configuration.
    pub settings: Arc<Settings>,
    /// D1 (reads, and the member cache).
    pub db: Db,
    /// Restate ingress (every mutation).
    pub ingress: Ingress,
    /// Clerk Backend API (`AUTH_MODE=clerk`).
    pub clerk: Option<BackendApi>,
    /// The per-user burst limit on mutations.
    pub throttle: Throttle,
}

impl App {
    /// Builds the state from settings, a database and the mutation limiter.
    pub fn new(settings: Settings, db: Db, throttle: Throttle) -> Self {
        let ingress = Ingress::new(
            settings.restate_ingress_url.clone(),
            settings.restate_api_key.clone(),
        );
        let clerk = settings.clerk_secret_key.clone().map(BackendApi::new);
        Self {
            settings: Arc::new(settings),
            db,
            ingress,
            clerk,
            throttle,
        }
    }

    /// The compiled-in catalog.
    pub fn catalog(&self) -> &'static Catalog {
        Catalog::global()
    }

    /// The support email domain.
    pub fn domain(&self) -> &str {
        &self.settings.support.domain
    }

    /// The support addresses.
    pub fn support(&self) -> &SupportMailbox {
        &self.settings.support
    }
}

/// Real now (the worker's clock; frozen between I/O on Workers, which is fine for reads).
pub fn now() -> OffsetDateTime {
    #[cfg(target_arch = "wasm32")]
    {
        let millis = worker::js_sys::Date::now();
        #[allow(clippy::cast_possible_truncation)]
        let nanos = (millis as i128) * 1_000_000;
        OffsetDateTime::from_unix_timestamp_nanos(nanos).unwrap_or(OffsetDateTime::UNIX_EPOCH)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        OffsetDateTime::now_utc()
    }
}
