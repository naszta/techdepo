//! The Workers entry points: the `email` handler and a small `fetch` handler for local tests.

use std::sync::Arc;

use techdepo_api_types::ingress::{Handler, SupportTicketInbound};
use techdepo_db::{D1Executor, Db, Retrying};
use techdepo_ports::MemberDirectory;
use time::OffsetDateTime;
use wasm_bindgen::prelude::wasm_bindgen;
use worker::js_sys::{Promise, futures::future_to_promise};
use worker::wasm_bindgen::{JsError, JsValue};
use worker::{
    Context, Env, Fetch, ForwardableEmailMessage, Headers, Method, Request, RequestInit, Response,
    Result, console_error, console_log, event,
};

use crate::directory::{ClerkDirectory, D1Directory};
use crate::filters::SenderAuth;
use crate::intake::{Envelope, Intake, IntakeConfig, Outcome};

/// The worker's configuration.
struct Settings {
    intake: IntakeConfig,
    ingress_url: String,
    api_key: Option<String>,
    clerk: Option<(String, String)>,
}

/// A variable or secret, if set and not empty.
fn get(env: &Env, name: &str) -> Option<String> {
    env.var(name)
        .map(|v| v.to_string())
        .or_else(|_| env.secret(name).map(|s| s.to_string()))
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn number<T: std::str::FromStr>(
    env: &Env,
    name: &str,
    default: T,
) -> std::result::Result<T, String> {
    match get(env, name) {
        Some(v) => v.parse().map_err(|_| format!("{name} must be a number")),
        None => Ok(default),
    }
}

impl Settings {
    fn from_env(env: &Env) -> std::result::Result<Self, String> {
        let (dev, clerk) = match get(env, "AUTH_MODE").as_deref() {
            Some("dev") => (true, None),
            Some("clerk") => {
                let key = get(env, "CLERK_SECRET_KEY")
                    .ok_or("CLERK_SECRET_KEY must be set with AUTH_MODE=clerk")?;
                let api = get(env, "CLERK_API_URL")
                    .unwrap_or_else(|| "https://api.clerk.com/v1".to_owned());
                (false, Some((api, key)))
            }
            _ => return Err("AUTH_MODE must be `dev` or `clerk`".to_owned()),
        };
        let sender_auth = match get(env, "INBOUND_SENDER_AUTH").as_deref() {
            None | Some("dmarc") => match get(env, "INBOUND_AUTHSERV_ID") {
                Some(authserv_id) => SenderAuth::Dmarc { authserv_id },
                None => IntakeConfig::default().sender_auth,
            },
            // wrangler's local email trigger adds no Authentication-Results.
            Some("trust-from") if dev => SenderAuth::TrustFrom,
            Some("trust-from") => {
                return Err("INBOUND_SENDER_AUTH=trust-from needs AUTH_MODE=dev".to_owned());
            }
            Some(_) => return Err("INBOUND_SENDER_AUTH must be `dmarc` or `trust-from`".to_owned()),
        };
        let defaults = IntakeConfig::default();
        let intake = IntakeConfig {
            domain: get(env, "EMAIL_DOMAIN").unwrap_or(defaults.domain),
            support_local_part: get(env, "SUPPORT_LOCAL_PART")
                .unwrap_or(defaults.support_local_part),
            max_bytes: number(env, "MAX_EMAIL_BYTES", defaults.max_bytes)?,
            max_per_org_per_hour: number(
                env,
                "MAX_INBOUND_PER_HOUR",
                defaults.max_per_org_per_hour,
            )?,
            sender_auth,
        };
        let ingress_url = match get(env, "RESTATE_INGRESS_URL") {
            Some(url) => url,
            None if dev => "http://localhost:8080".to_owned(),
            None => return Err("RESTATE_INGRESS_URL must be set".to_owned()),
        };
        Ok(Self {
            intake,
            ingress_url: ingress_url.trim_end_matches('/').to_owned(),
            api_key: get(env, "RESTATE_API_KEY"),
            clerk,
        })
    }
}

fn now() -> OffsetDateTime {
    let millis = worker::Date::now().as_millis();
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// `POST {ingress}/SupportTicket/<key>/inbound/send` with `Idempotency-Key: <Message-ID>`.
async fn send(
    settings: &Settings,
    key: &str,
    body: String,
    message_id: &str,
) -> std::result::Result<(), String> {
    let url = format!(
        "{}{}",
        settings.ingress_url,
        SupportTicketInbound::send_path(Some(key))
    );
    let headers = Headers::new();
    headers
        .set("content-type", "application/json")
        .map_err(|e| e.to_string())?;
    headers
        .set("idempotency-key", message_id)
        .map_err(|e| e.to_string())?;
    if let Some(api_key) = &settings.api_key {
        headers
            .set("authorization", &format!("Bearer {api_key}"))
            .map_err(|e| e.to_string())?;
    }
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(JsValue::from_str(&body)));
    let request = Request::new_with_init(&url, &init).map_err(|e| e.to_string())?;
    let mut response = Fetch::Request(request)
        .send()
        .await
        .map_err(|e| format!("Restate ingress unreachable: {e}"))?;
    let status = response.status_code();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(format!("Restate ingress answered {status}: {text}"))
    }
}

/// Runs one delivery: intake, then the hand-off to the ticket. `Err` is a temporary failure.
async fn handle(env: &Env, envelope: Envelope) -> std::result::Result<Outcome, String> {
    let settings = Settings::from_env(env)?;
    // Retried: a transient D1 error (locally: another worker writing the shared database) would
    // otherwise fail the delivery. Every write of the intake is idempotent (derived IDs).
    let db = Db::new(Arc::new(Retrying::new(D1Executor::new(
        env.d1("DB").map_err(|e| format!("D1 binding `DB`: {e}"))?,
    ))));
    let directory: Box<dyn MemberDirectory> = match &settings.clerk {
        Some((api, key)) => Box::new(ClerkDirectory::new(
            api.clone(),
            key.clone(),
            env.kv("MEMBERS_CACHE").ok(),
        )),
        None => Box::new(D1Directory::new(db.clone())),
    };
    let outcome = Intake::new(&db, &*directory, &settings.intake)
        .process(&envelope)
        .await
        .map_err(|e| e.to_string())?;
    if let Outcome::Accepted { key, message } = &outcome {
        let body = serde_json::to_string(message).map_err(|e| e.to_string())?;
        send(&settings, key, body, &message.message_id).await?;
    }
    Ok(outcome)
}

fn describe(outcome: &Outcome) -> serde_json::Value {
    match outcome {
        Outcome::Accepted { key, message } => serde_json::json!({
            "outcome": "accepted",
            "key": key,
            "ticket_id": message.ticket_id,
            "message_id": message.message_id,
            "body_text": message.body_text,
        }),
        Outcome::Rejected { reason, detail } => serde_json::json!({
            "outcome": "rejected",
            "reason": reason.as_str(),
            "detail": detail,
        }),
    }
}

/// The Workers `email` entry point, written out instead of `#[event(email)]`: that glue panics
/// when the handler returns `Err`, and a panic aborts the wasm instance, so every later delivery
/// (and request) to the isolate fails with `unreachable` until it is recycled. Here a temporary
/// failure rejects this delivery's promise and nothing else.
#[wasm_bindgen]
pub fn email(
    message: ForwardableEmailMessage,
    env: Env,
    _ctx: worker::worker_sys::Context,
) -> Promise {
    future_to_promise(std::panic::AssertUnwindSafe(async move {
        match receive(message, env).await {
            Ok(()) => Ok(JsValue::UNDEFINED),
            Err(e) => Err(JsError::new(&e.to_string()).into()),
        }
    }))
}

/// One delivery from Email Routing. `Err` is a temporary failure.
async fn receive(message: ForwardableEmailMessage, env: Env) -> Result<()> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let size = message.raw_size() as usize;
    let max = Settings::from_env(&env).map_or(usize::MAX, |s| s.intake.max_bytes);
    // Oversized mail is not read: it is dropped by size alone.
    let raw = if size > max {
        Vec::new()
    } else {
        message.raw_bytes().await?
    };
    let envelope = Envelope {
        from: message.from(),
        to: message.to(),
        raw,
        raw_size: size,
        received_at: now(),
    };
    match handle(&env, envelope).await {
        Ok(outcome) => {
            console_log!("mail-in: {}", describe(&outcome));
            Ok(())
        }
        Err(e) => {
            // A temporary failure: fail the delivery so the sending server retries.
            console_error!("mail-in: temporary failure: {e}");
            Err(worker::Error::RustError(e))
        }
    }
}

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let dev = get(&env, "AUTH_MODE").as_deref() == Some("dev");
    match (req.method(), url.path()) {
        (Method::Get, "/") => Response::ok(
            "techdepo-mail-in: receives support email (Cloudflare Email Routing). \
             Locally: POST /cdn-cgi/handler/email?from=…&to=… with a raw .eml body.",
        ),
        // Local testing only: the same pipeline, answering with the outcome.
        (Method::Post, "/inbound") if dev => {
            let query: std::collections::HashMap<String, String> =
                url.query_pairs().into_owned().collect();
            let (Some(from), Some(to)) = (query.get("from"), query.get("to")) else {
                return Response::error("`from` and `to` query parameters are required", 400);
            };
            let raw = req.bytes().await?;
            let envelope = Envelope {
                from: from.clone(),
                to: to.clone(),
                raw_size: raw.len(),
                raw,
                received_at: now(),
            };
            match handle(&env, envelope).await {
                Ok(outcome) => Response::from_json(&describe(&outcome)),
                Err(e) => Response::error(e, 503),
            }
        }
        _ => Response::error("Not found", 404),
    }
}
