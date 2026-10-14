//! The worker's configuration, read from its environment (wrangler `[vars]`, secrets, and
//! locally `.dev.vars` written by `cargo run -p xtask -- dev-vars`). See `.env.example`.
//!
//! | Variable | Use |
//! |---|---|
//! | `AUTH_MODE` | `clerk` (default) or `dev` (dev login; never in production) |
//! | `CLERK_PUBLISHABLE_KEY`, `CLERK_SECRET_KEY` | Clerk keys (`AUTH_MODE=clerk`) |
//! | `CLERK_AUTHORIZED_PARTIES` | origins accepted in the session token's `azp`, comma separated |
//! | `RESTATE_INGRESS_URL`, `RESTATE_API_KEY` | where mutations go (key empty locally) |
//! | `RESTATE_ADMIN_URL` | Restate UI deep links, only when it is a local URL |
//! | `EMAIL_DOMAIN`, `SUPPORT_LOCAL_PART` | support addresses (`<local part>+<org>@<domain>`; local part default `support`) |
//! | `APP_BASE_URL`, `LLM_BACKEND` | shown in `/api/config` |
//! | `MAX_INBOUND_PER_HOUR` | support messages accepted per org and hour, mail and the web form together (default 100, as mail-in) |
//! | `MAX_ORDERS_PER_HOUR` | orders an org may create per hour through checkout and scenario presets (default 50) |

use techdepo_api_types::session::{AuthMode, LlmBackend, SupportMailbox};

/// `MAX_INBOUND_PER_HOUR` when unset (mail-in's default).
pub const DEFAULT_MAX_INBOUND_PER_HOUR: u32 = 100;

/// `MAX_ORDERS_PER_HOUR` when unset.
pub const DEFAULT_MAX_ORDERS_PER_HOUR: u32 = 50;

/// Everything the worker reads from its environment.
#[derive(Debug, Clone)]
pub struct Settings {
    /// How requests are authenticated.
    pub auth_mode: AuthMode,
    /// Clerk publishable key.
    pub clerk_publishable_key: Option<String>,
    /// Clerk secret key (JWKS and the Backend API).
    pub clerk_secret_key: Option<String>,
    /// Accepted `azp` origins.
    pub clerk_authorized_parties: Vec<String>,
    /// Restate ingress base URL.
    pub restate_ingress_url: String,
    /// Restate API key (Restate Cloud).
    pub restate_api_key: Option<String>,
    /// Restate UI, for local deep links.
    pub restate_ui_url: Option<String>,
    /// Support addresses.
    pub support: SupportMailbox,
    /// Public base URL.
    pub app_base_url: String,
    /// LLM backend the services use.
    pub llm_backend: LlmBackend,
    /// Support messages accepted per org and rolling hour (mail-in's limit too).
    pub max_inbound_per_hour: u32,
    /// Orders an org may create per rolling hour (checkout and scenario presets).
    pub max_orders_per_hour: u32,
}

impl Settings {
    /// Reads the settings through `get` (a variable or secret, `None` when unset or empty).
    /// Errors name the variable at fault.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let auth_mode = match get("AUTH_MODE").as_deref() {
            None | Some("clerk") => AuthMode::Clerk,
            Some("dev") => AuthMode::Dev,
            Some(other) => {
                return Err(format!("AUTH_MODE must be `clerk` or `dev`, not `{other}`"));
            }
        };
        let llm_backend = match get("LLM_BACKEND").as_deref() {
            None | Some("mock") => LlmBackend::Mock,
            Some("workers-ai") => LlmBackend::WorkersAi,
            Some(other) => {
                return Err(format!(
                    "LLM_BACKEND must be `mock` or `workers-ai`, not `{other}`"
                ));
            }
        };
        let clerk_secret_key = get("CLERK_SECRET_KEY");
        if auth_mode == AuthMode::Clerk && clerk_secret_key.is_none() {
            return Err("CLERK_SECRET_KEY is required with AUTH_MODE=clerk".to_owned());
        }
        let restate_ui_url = get("RESTATE_ADMIN_URL").filter(|url| is_local(url));
        let max_inbound_per_hour =
            limit(&get, "MAX_INBOUND_PER_HOUR", DEFAULT_MAX_INBOUND_PER_HOUR)?;
        let max_orders_per_hour = limit(&get, "MAX_ORDERS_PER_HOUR", DEFAULT_MAX_ORDERS_PER_HOUR)?;
        Ok(Self {
            auth_mode,
            clerk_publishable_key: get("CLERK_PUBLISHABLE_KEY"),
            clerk_secret_key,
            clerk_authorized_parties: get("CLERK_AUTHORIZED_PARTIES")
                .map(|v| {
                    v.split(',')
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            restate_ingress_url: get("RESTATE_INGRESS_URL")
                .unwrap_or_else(|| "http://localhost:8080".to_owned())
                .trim_end_matches('/')
                .to_owned(),
            restate_api_key: get("RESTATE_API_KEY"),
            restate_ui_url,
            support: SupportMailbox {
                local_part: get("SUPPORT_LOCAL_PART").unwrap_or_else(|| "support".to_owned()),
                domain: get("EMAIL_DOMAIN").unwrap_or_else(|| "techdepo.dev".to_owned()),
            },
            app_base_url: get("APP_BASE_URL").unwrap_or_else(|| "http://localhost:8787".to_owned()),
            llm_backend,
            max_inbound_per_hour,
            max_orders_per_hour,
        })
    }

    /// Reads the settings from the worker's environment.
    #[cfg(target_arch = "wasm32")]
    pub fn from_env(env: &worker::Env) -> Result<Self, String> {
        Self::from_lookup(|name| {
            env.var(name)
                .map(|v| v.to_string())
                .or_else(|_| env.secret(name).map(|s| s.to_string()))
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        })
    }
}

/// A numeric limit `name`, or `default` when unset.
fn limit(get: &impl Fn(&str) -> Option<String>, name: &str, default: u32) -> Result<u32, String> {
    get(name).map_or(Ok(default), |v| {
        v.parse().map_err(|_| format!("{name} must be a number"))
    })
}

fn is_local(url: &str) -> bool {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    rest.starts_with("localhost") || rest.starts_with("127.0.0.1") || rest.starts_with("[::1]")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn settings(vars: &[(&str, &str)]) -> Result<Settings, String> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Settings::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn clerk_needs_a_secret() {
        assert!(settings(&[]).is_err());
        let dev = settings(&[
            ("AUTH_MODE", "dev"),
            ("RESTATE_ADMIN_URL", "http://localhost:9070"),
            ("RESTATE_INGRESS_URL", "http://localhost:8080/"),
        ])
        .unwrap();
        assert_eq!(dev.auth_mode, AuthMode::Dev);
        assert_eq!(dev.restate_ingress_url, "http://localhost:8080");
        assert_eq!(dev.restate_ui_url.as_deref(), Some("http://localhost:9070"));
        assert_eq!(dev.max_inbound_per_hour, DEFAULT_MAX_INBOUND_PER_HOUR);
        assert_eq!(dev.max_orders_per_hour, DEFAULT_MAX_ORDERS_PER_HOUR);
        assert!(settings(&[("AUTH_MODE", "dev"), ("MAX_INBOUND_PER_HOUR", "x")]).is_err());
        assert!(settings(&[("AUTH_MODE", "dev"), ("MAX_ORDERS_PER_HOUR", "x")]).is_err());
        let cloud = settings(&[
            ("AUTH_MODE", "dev"),
            ("RESTATE_ADMIN_URL", "https://x.env.eu.restate.cloud:9070"),
        ])
        .unwrap();
        assert_eq!(cloud.restate_ui_url, None);
    }
}
