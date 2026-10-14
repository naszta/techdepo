//! `/api/config`, `/api/dev/login`, `/api/me`.

use techdepo_api_types::Validate;
use techdepo_api_types::endpoints::{DevLogin, GetConfig, GetMe};
use techdepo_api_types::session::{AuthMode, Config, DevLoginResponse, Me};
use techdepo_catalog::CATALOG_VERSION;

use crate::app::{App, now};
use crate::auth::{self, Caller, DevClaims};
use crate::endpoint::Input;
use crate::error::ApiResult;

pub async fn config(app: App, _: Input<GetConfig>) -> ApiResult<Config> {
    let s = &app.settings;
    Ok(Config {
        auth_mode: s.auth_mode,
        clerk_publishable_key: (s.auth_mode == AuthMode::Clerk)
            .then(|| s.clerk_publishable_key.clone())
            .flatten(),
        support_domain: s.support.domain.clone(),
        catalog_version: CATALOG_VERSION.to_owned(),
        llm_backend: s.llm_backend,
        restate_ui_url: s.restate_ui_url.clone(),
    })
}

/// Signs in as any email (`AUTH_MODE=dev` only; the route is `404` otherwise). Provisions the
/// dev org and the member on first use.
pub async fn dev_login(app: App, input: Input<DevLogin>) -> ApiResult<DevLoginResponse> {
    let req = input.body;
    req.validate()?;
    let claims = DevClaims::new(&req.email, req.name, req.org_name.as_deref(), req.role);
    let token = claims.token();
    let caller = auth::resolve(&app, claims.into(), now()).await?;
    Ok(DevLoginResponse {
        token,
        me: caller.me(app.support()),
    })
}

/// The signed-in user, org and role. Resolving the caller already provisioned an unknown org
/// (`Org/provision`) and refreshed the member cache.
pub async fn me(app: App, caller: Caller, _: Input<GetMe>) -> ApiResult<Me> {
    Ok(caller.me(app.support()))
}
