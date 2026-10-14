//! The [`Mailer`] port over HTTP, through the Workers Fetch API.
//!
//! - [`Mailpit`] (`EMAIL_TRANSPORT=mailpit`, local): `POST {MAILPIT_URL}/api/v1/send`.
//! - [`Resend`] (`EMAIL_TRANSPORT=resend`): `POST https://api.resend.com/emails`, with the
//!   email's `Message-ID` as `Idempotency-Key`, so a retried send is not delivered twice.
//!
//! The payloads are plain functions ([`mailpit_payload`], [`resend_payload`]); only [`post`]
//! touches JS, inside a `SendWrapper` (sound: a Worker isolate is single-threaded), because the
//! port's futures must be `Send`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use send_wrapper::SendWrapper;
use serde_json::{Map, Value, json};
use techdepo_ports::{MailError, Mailbox, Mailer, OutboundEmail, async_trait};
use worker::wasm_bindgen::JsValue;
use worker::{Fetch, Headers, Method, Request, RequestInit};

/// Mailpit's HTTP send API.
pub struct Mailpit {
    url: String,
}

impl Mailpit {
    /// Mailpit at `base_url` (e.g. `http://localhost:8025`).
    pub fn new(base_url: &str) -> Self {
        Self {
            url: format!("{}/api/v1/send", base_url.trim_end_matches('/')),
        }
    }
}

/// Resend's HTTP API.
pub struct Resend {
    api_key: String,
}

impl Resend {
    /// Resend with an API key.
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

fn mailpit_box(m: &Mailbox) -> Value {
    json!({ "Email": m.email, "Name": m.name.clone().unwrap_or_default() })
}

/// The Mailpit `/api/v1/send` body.
pub fn mailpit_payload(email: &OutboundEmail) -> Value {
    let mut headers = Map::new();
    headers.insert(
        "Message-ID".to_owned(),
        Value::String(format!("<{}>", email.message_id)),
    );
    for h in &email.headers {
        headers.insert(h.name.clone(), Value::String(h.value.clone()));
    }
    let mut body = json!({
        "From": mailpit_box(&email.from),
        "To": [mailpit_box(&email.to)],
        "Subject": email.subject,
        "Text": email.text,
        "Headers": headers,
    });
    if let Some(html) = &email.html {
        body["HTML"] = Value::String(html.clone());
    }
    if let Some(reply_to) = &email.reply_to {
        body["ReplyTo"] = json!([mailpit_box(reply_to)]);
    }
    if !email.attachments.is_empty() {
        body["Attachments"] = email
            .attachments
            .iter()
            .map(|a| {
                json!({
                    "Filename": a.filename,
                    "ContentType": a.content_type,
                    "Content": STANDARD.encode(a.content.as_bytes()),
                })
            })
            .collect();
    }
    body
}

/// The Resend `/emails` body.
pub fn resend_payload(email: &OutboundEmail) -> Value {
    let mut headers = Map::new();
    headers.insert(
        "Message-ID".to_owned(),
        Value::String(format!("<{}>", email.message_id)),
    );
    for h in &email.headers {
        headers.insert(h.name.clone(), Value::String(h.value.clone()));
    }
    let mut body = json!({
        "from": email.from.to_string(),
        "to": [email.to.to_string()],
        "subject": email.subject,
        "text": email.text,
        "headers": headers,
    });
    if let Some(html) = &email.html {
        body["html"] = Value::String(html.clone());
    }
    if let Some(reply_to) = &email.reply_to {
        body["reply_to"] = Value::String(reply_to.to_string());
    }
    if !email.attachments.is_empty() {
        body["attachments"] = email
            .attachments
            .iter()
            .map(|a| {
                json!({
                    "filename": a.filename,
                    "content_type": a.content_type,
                    "content": STANDARD.encode(a.content.as_bytes()),
                })
            })
            .collect();
    }
    body
}

/// Maps an HTTP status to the port's error kinds.
pub fn status_error(provider: &str, status: u16, body: &str) -> MailError {
    let detail = format!(
        "{provider} answered {status}: {}",
        body.chars().take(300).collect::<String>()
    );
    match status {
        401 | 403 => MailError::Config(detail),
        408 | 409 | 429 => MailError::Unavailable(detail),
        s if s >= 500 => MailError::Unavailable(detail),
        _ => MailError::Rejected(detail),
    }
}

/// POSTs JSON and returns the status and body. Network failures are retryable.
async fn post(
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> Result<(u16, String), MailError> {
    let unavailable = |e: worker::Error| MailError::Unavailable(e.to_string());
    let h = Headers::new();
    h.set("content-type", "application/json")
        .map_err(unavailable)?;
    for (name, value) in headers {
        h.set(name, value).map_err(unavailable)?;
    }
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(h)
        .with_body(Some(JsValue::from_str(&body.to_string())));
    let request = Request::new_with_init(url, &init).map_err(unavailable)?;
    let mut response = Fetch::Request(request).send().await.map_err(unavailable)?;
    let status = response.status_code();
    let text = response.text().await.unwrap_or_default();
    Ok((status, text))
}

/// The provider's message ID from a JSON response (`ID` for Mailpit, `id` for Resend).
fn provider_id(text: &str, field: &str) -> Result<String, MailError> {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get(field).and_then(Value::as_str).map(str::to_owned))
        .ok_or_else(|| MailError::Unavailable(format!("unexpected response: {text}")))
}

#[async_trait]
impl Mailer for Mailpit {
    async fn send(&self, email: &OutboundEmail) -> Result<String, MailError> {
        let body = mailpit_payload(email);
        let url = self.url.clone();
        SendWrapper::new(async move {
            let (status, text) = post(&url, &[], &body).await?;
            if (200..300).contains(&status) {
                provider_id(&text, "ID")
            } else {
                Err(status_error("Mailpit", status, &text))
            }
        })
        .await
    }
}

#[async_trait]
impl Mailer for Resend {
    async fn send(&self, email: &OutboundEmail) -> Result<String, MailError> {
        if self.api_key.is_empty() {
            return Err(MailError::Config("RESEND_API_KEY is not set".to_owned()));
        }
        let body = resend_payload(email);
        let auth = format!("Bearer {}", self.api_key);
        let key = email.message_id.clone();
        SendWrapper::new(async move {
            let headers = [
                ("authorization", auth.as_str()),
                ("idempotency-key", key.as_str()),
            ];
            let (status, text) = post("https://api.resend.com/emails", &headers, &body).await?;
            if (200..300).contains(&status) {
                provider_id(&text, "id")
            } else {
                Err(status_error("Resend", status, &text))
            }
        })
        .await
    }
}
