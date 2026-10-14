//! `techdepo-services`: every Restate service of TechDepó behind one Workers fetch handler.
//!
//! Restate calls this worker in request-response mode (Workers buffer the request body). Each
//! request builds the endpoint from the environment ([`settings`]): the D1 binding `DB`, the
//! mail transport ([`mailer`]), the model providers (`techdepo_llm::ModelRegistry`: the `AI`
//! binding and the providers whose secrets are set; `LLM_BACKEND` and `AI_*` give the defaults,
//! whose providers must be among them), the request identity keys and the service policies (`restate-config`, merged over the built-in
//! `techdepo_services::policy`), then hands the request to `Endpoint::handle_with_options`
//! and returns the collected response body.

mod mailer;
mod settings;

use std::sync::Arc;

use http_body_util::BodyExt;
use restate_config::ConfigureEndpointExt;
use restate_sdk::prelude::*;
use techdepo_db::D1Executor;
use techdepo_ports::Models;
use techdepo_services::{Ports, Services, policy};
use worker::{Context, Env, HttpRequest, Result, event};

use crate::settings::Settings;

/// The endpoint for this environment.
fn endpoint(env: &Env) -> std::result::Result<Endpoint, String> {
    let settings = Settings::from_env(env)?;
    let db = env.d1("DB").map_err(|e| format!("D1 binding `DB`: {e}"))?;
    let mut ports = Ports::new(Arc::new(D1Executor::new(db)), settings.mailer);
    let models = settings.llm.registry(env, |name| settings::get(env, name));
    let available = models.available();
    let mut defaults = models.defaults().providers().into_iter();
    if let Some(missing) = defaults.find(|p| !available.contains(p)) {
        return Err(match missing.secrets_hint() {
            Some(secrets) => format!(
                "the default models (LLM_BACKEND, AI_*) are on {}: set {secrets}",
                missing.label()
            ),
            None => format!(
                "the default models (LLM_BACKEND, AI_*) are on {}: it needs the `AI` binding",
                missing.label()
            ),
        });
    }
    ports.models = Arc::new(models);
    let services = Services::new(ports, settings.config);
    let builder = Endpoint::builder()
        .configure(&settings.restate.endpoint)
        .map_err(|e| format!("RESTATE_IDENTITY_KEYS / RESTATE_CONFIG: {e}"))?;
    let builder = services
        .bind_with(builder, &policy::merged(settings.restate.services))
        .map_err(|e| format!("RESTATE_CONFIG: {e}"))?;
    Ok(builder.build())
}

fn body(bytes: Vec<u8>) -> Result<worker::Body> {
    worker::Body::from_stream(futures::stream::once(async move {
        Ok::<_, std::convert::Infallible>(bytes)
    }))
}

#[event(fetch)]
async fn fetch(req: HttpRequest, env: Env, _ctx: Context) -> Result<http::Response<worker::Body>> {
    let endpoint = match endpoint(&env) {
        Ok(endpoint) => endpoint,
        Err(message) => {
            worker::console_error!("techdepo-services misconfigured: {message}");
            let mut resp = http::Response::new(body(message.into_bytes())?);
            *resp.status_mut() = http::StatusCode::INTERNAL_SERVER_ERROR;
            return Ok(resp);
        }
    };
    let resp = endpoint.handle_with_options(
        req,
        HandleOptions {
            protocol_mode: ProtocolMode::RequestResponse,
        },
    );
    let (parts, resp_body) = resp.into_parts();
    let bytes = resp_body
        .collect()
        .await
        .map_err(|e| worker::Error::RustError(e.to_string()))?
        .to_bytes();
    Ok(http::Response::from_parts(parts, body(bytes.to_vec())?))
}
