//! The worker's configuration, read from its environment (wrangler `[vars]`, secrets, and
//! locally `.dev.vars` written by `cargo run -p xtask -- dev-vars`). See `.env.example`.
//!
//! | Variable | Use |
//! |---|---|
//! | `EMAIL_DOMAIN`, `SUPPORT_LOCAL_PART`, `EMAIL_FROM_NAME`, `APP_BASE_URL` | addresses and links in emails |
//! | `AUTH_MODE` | `dev` relaxes the two checks below for local development (never in production); anything else, or unset, fails closed |
//! | `EMAIL_TRANSPORT` | `mailpit` (local) or `resend`; required unless `AUTH_MODE=dev` (where it defaults to `mailpit`) |
//! | `MAILPIT_URL` / `RESEND_API_KEY` | the transport's endpoint / key (`RESEND_API_KEY` required with `resend`) |
//! | `RESTATE_IDENTITY_KEYS` | request identity keys (`publickeyv1_…`, comma separated); required unless `AUTH_MODE=dev` (or given in `RESTATE_CONFIG`) |
//! | `LLM_BACKEND`, `AI_*`, `LLM_DAILY_TOKEN_BUDGET_*` | the default models (`techdepo_llm::LlmConfig`): `mock` (default) or `workers-ai`, the `AI_*` model addresses (`anthropic:claude-sonnet-5`; a bare ID is Workers AI, which needs the `AI` binding). The operator's model settings (Muhaha page) override them per task |
//! | `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `OPENROUTER_API_KEY`, `AZURE_OPENAI_API_KEY` + `AZURE_OPENAI_RESOURCE` | secrets, all optional: each one (Azure: the pair) makes its provider available to the model settings |
//! | `ORG_EXPIRY_DAYS` | days after provisioning that a demo org and all its data are deleted (`techdepo_services::org`); unset or `0`: never |
//! | `RESTATE_CONFIG` | optional `restate-config` JSON: `{"endpoint": {"identity_keys": …}, "services": {"Email": {…}}}`, merged over the built-in policies |
//!
//! Without identity keys the endpoint accepts unsigned requests, and `ingress_private` only hides
//! handlers from Restate's ingress, not from the deployment URL: anyone reaching the worker could
//! call `Payments/refund` or `Order/place` directly. So a worker that is not explicitly in dev
//! mode refuses to start without them.

use std::sync::Arc;

use restate_config::Config as RestateConfig;
use techdepo_ports::Mailer;
use techdepo_services::Config;
use techdepo_services::policy::Policies;
use worker::Env;

use crate::mailer::{Mailpit, Resend};

/// Everything the endpoint needs from the environment.
pub struct Settings {
    /// Service configuration.
    pub config: Config,
    /// The mail transport.
    pub mailer: Arc<dyn Mailer>,
    /// Endpoint settings and per-service policy overrides.
    pub restate: RestateConfig<Policies>,
    /// The models.
    pub llm: techdepo_llm::LlmConfig,
}

/// A variable or secret, if set and not empty.
pub fn get(env: &Env, name: &str) -> Option<String> {
    env.var(name)
        .map(|v| v.to_string())
        .or_else(|_| env.secret(name).map(|s| s.to_string()))
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

impl Settings {
    /// Reads the settings. Errors name the variable at fault.
    pub fn from_env(env: &Env) -> Result<Self, String> {
        let defaults = Config::default();
        let config = Config {
            support_domain: get(env, "EMAIL_DOMAIN").unwrap_or(defaults.support_domain),
            support_local_part: get(env, "SUPPORT_LOCAL_PART")
                .unwrap_or(defaults.support_local_part),
            from_name: get(env, "EMAIL_FROM_NAME").unwrap_or(defaults.from_name),
            app_base_url: get(env, "APP_BASE_URL").unwrap_or(defaults.app_base_url),
            carrier_name: defaults.carrier_name,
            llm_budget: defaults.llm_budget,
            org_expiry: techdepo_services::org::parse_expiry_days(
                &get(env, "ORG_EXPIRY_DAYS").unwrap_or_default(),
            )?,
        };
        let dev = get(env, "AUTH_MODE").as_deref() == Some("dev");
        let transport = match get(env, "EMAIL_TRANSPORT") {
            Some(transport) => transport,
            None if dev => "mailpit".to_owned(),
            None => return Err("EMAIL_TRANSPORT must be set (`resend`)".to_owned()),
        };
        let mailer: Arc<dyn Mailer> = match transport.as_str() {
            "mailpit" => Arc::new(Mailpit::new(
                &get(env, "MAILPIT_URL").unwrap_or_else(|| "http://localhost:8025".to_owned()),
            )),
            "resend" => Arc::new(Resend::new(get(env, "RESEND_API_KEY").ok_or_else(
                || "RESEND_API_KEY must be set with EMAIL_TRANSPORT=resend".to_owned(),
            )?)),
            other => {
                return Err(format!(
                    "EMAIL_TRANSPORT must be `mailpit` or `resend`, not `{other}`"
                ));
            }
        };
        let mut restate: RestateConfig<Policies> = match get(env, "RESTATE_CONFIG") {
            Some(json) => {
                serde_json::from_str(&json).map_err(|e| format!("RESTATE_CONFIG: {e}"))?
            }
            None => RestateConfig::default(),
        };
        if let Some(keys) = get(env, "RESTATE_IDENTITY_KEYS") {
            restate.endpoint.identity_keys.extend(
                keys.split([',', ' ', '\n'])
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .map(str::to_owned),
            );
        }
        if restate.endpoint.identity_keys.is_empty() && !dev {
            return Err(
                "RESTATE_IDENTITY_KEYS must be set: without it the endpoint accepts unsigned \
                 requests (set AUTH_MODE=dev for local development only)"
                    .to_owned(),
            );
        }
        let llm = techdepo_llm::LlmConfig::from_lookup(|name| get(env, name))
            .map_err(|e| e.to_string())?;
        let config = Config {
            llm_budget: llm.budget,
            ..config
        };
        Ok(Self {
            config,
            mailer,
            restate,
            llm,
        })
    }
}
