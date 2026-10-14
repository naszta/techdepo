//! Rate limits on the web API's mutations (public demo cost and abuse, roadmap M7).
//!
//! - **Burst limit:** every mutating route (a non-`GET` route that calls Restate) counts
//!   against a per-user limit before the caller is resolved, through the Workers Rate Limiting
//!   binding `MUTATION_LIMITER` (`[[ratelimits]]` in `wrangler.toml`: 60 per 60 s, per
//!   Cloudflare location). Without the binding (native tests) nothing is limited; a failing
//!   binding lets the request through, so the limiter never takes the shop down.
//! - **Order cap:** checkout and scenario presets each create an order whose workflow runs for
//!   days, so an org may create at most `MAX_ORDERS_PER_HOUR` orders per rolling hour
//!   ([`order_quota`], counted in D1 like the support message limit).
//!
//! Both answer `429 rate_limited`.

use techdepo_api_types::{ApiError, ErrorCode, Method, Route};

use crate::app::{App, now};
use crate::auth::Caller;
use crate::error::{ApiResult, DbResultExt};

/// The binding's name in `wrangler.toml`.
pub const BINDING: &str = "MUTATION_LIMITER";

/// The per-user burst limiter (cheap to clone).
#[derive(Clone, Default)]
pub struct Throttle {
    #[cfg(target_arch = "wasm32")]
    limiter: Option<std::sync::Arc<worker::RateLimiter>>,
}

impl Throttle {
    /// No limit (native tests, or no binding).
    pub fn unlimited() -> Self {
        Self::default()
    }

    /// The `MUTATION_LIMITER` binding, or no limit when the worker has none.
    #[cfg(target_arch = "wasm32")]
    pub fn from_env(env: &worker::Env) -> Self {
        Self {
            limiter: env.rate_limiter(BINDING).ok().map(std::sync::Arc::new),
        }
    }

    /// Counts a call of `route` by `key` (`<org>:<user>`); `429` once the user is over the
    /// limit. Reads are never limited.
    pub async fn check(&self, route: &Route, key: &str) -> ApiResult<()> {
        if !is_mutation(route) {
            return Ok(());
        }
        if self.allows(key).await {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorCode::RateLimited,
                "Too many changes in a short time. Wait a minute and try again.",
            ))
        }
    }

    #[cfg(target_arch = "wasm32")]
    async fn allows(&self, key: &str) -> bool {
        let Some(limiter) = &self.limiter else {
            return true;
        };
        match limiter.limit(key.to_owned()).await {
            Ok(outcome) => outcome.success,
            Err(e) => {
                crate::error::log(&format!("rate limiter `{BINDING}` failed: {e}"));
                true
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_async)]
    async fn allows(&self, _key: &str) -> bool {
        true
    }
}

/// Whether `route` changes state: it calls Restate and is not a `GET` (`GET /api/me`
/// provisions an unknown org, which is not the caller's doing to throttle).
pub fn is_mutation(route: &Route) -> bool {
    route.restate.is_some() && route.method != Method::Get
}

/// The org's hourly order cap (`MAX_ORDERS_PER_HOUR`), checked before checkout and scenario
/// presets. Orders of any kind created in the last hour count.
pub async fn order_quota(app: &App, caller: &Caller) -> ApiResult<()> {
    let created = app
        .db
        .orders(&caller.scope())
        .created_since(now() - time::Duration::hours(1))
        .await
        .db()?;
    if created >= app.settings.max_orders_per_hour {
        return Err(ApiError::new(
            ErrorCode::RateLimited,
            "Your organization created too many orders in the last hour. Please try again \
             later.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use techdepo_api_types::Endpoint;
    use techdepo_api_types::endpoints::{
        AddToCart, CreateScenario, GetMe, GetOrder, PlaceOrder, RunDemoAction,
    };

    use super::*;

    #[test]
    fn only_mutations_are_limited() {
        for route in [
            &PlaceOrder::ROUTE,
            &CreateScenario::ROUTE,
            &RunDemoAction::ROUTE,
            &AddToCart::ROUTE,
        ] {
            assert!(is_mutation(route), "{}", route.name);
        }
        assert!(!is_mutation(&GetMe::ROUTE));
        assert!(!is_mutation(&GetOrder::ROUTE));
    }
}
