//! Restate ingress client: every mutation goes through a Restate handler
//! (`techdepo_api_types::ingress`, `docs/plan/api.md` "Mutations and idempotency").
//!
//! - Request-response calls wait for the handler's result; workflows and the support inbox are
//!   started one-way (`/send`).
//! - `Authorization: Bearer <RESTATE_API_KEY>` when a key is configured (Restate Cloud).
//! - `Idempotency-Key` for creates the SPA identified with a client key
//!   ([`ingress::idempotency_key`]).
//! - A handler's terminal error carries the `ApiError` as its message and is passed through
//!   unchanged. Errors from ingress itself (`"source": "ingress"`: an unknown service or
//!   handler, a bad key) and anything else unexpected become `503 unavailable`.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use techdepo_api_types::ingress::{self, Handler, ServiceKind};
use techdepo_api_types::{ApiError, ErrorCode};

use crate::error::{ApiResult, log};

/// The Restate ingress.
#[derive(Debug, Clone)]
pub struct Ingress {
    base_url: String,
    api_key: Option<String>,
}

impl Ingress {
    /// A client for `base_url` (`RESTATE_INGRESS_URL`).
    pub fn new(base_url: String, api_key: Option<String>) -> Self {
        Self { base_url, api_key }
    }

    /// Calls `H` and waits for its output.
    pub async fn call<H: Handler>(
        &self,
        key: Option<&str>,
        input: &H::Input,
        idempotency_key: Option<&str>,
    ) -> ApiResult<H::Output> {
        let path = H::call_path(key);
        let body = self
            .post(&path, input, idempotency_for::<H>(idempotency_key))
            .await?;
        decode_output::<H::Output>(&path, &body)
    }

    /// Starts `H` one-way (`/send`): returns once Restate has accepted the invocation.
    pub async fn send<H: Handler>(
        &self,
        key: Option<&str>,
        input: &H::Input,
        idempotency_key: Option<&str>,
    ) -> ApiResult<()> {
        self.post(
            &H::send_path(key),
            input,
            idempotency_for::<H>(idempotency_key),
        )
        .await
        .map(drop)
    }

    async fn post<I: serde::Serialize>(
        &self,
        path: &str,
        input: &I,
        idempotency_key: Option<&str>,
    ) -> ApiResult<String> {
        let body = serde_json::to_value(input).map_err(|e| {
            ApiError::new(
                ErrorCode::Internal,
                format!("Cannot encode the request: {e}"),
            )
        })?;
        // A handler whose input is `()` takes no body and no content-type.
        let body = (!body.is_null()).then(|| body.to_string());
        let url = format!("{}{path}", self.base_url);
        let (status, text) = transport::post(
            &url,
            self.api_key.as_deref(),
            idempotency_key,
            body.as_deref(),
        )
        .await
        .map_err(|e| {
            log(&format!("Restate ingress {path}: {e}"));
            ApiError::new(
                ErrorCode::Unavailable,
                "The order system is temporarily unavailable. Try again in a moment.",
            )
        })?;
        interpret(path, status, &text)
    }
}

/// The `Idempotency-Key` to send to `H`: none for workflows, which Restate rejects with one
/// (a workflow run is already idempotent per workflow key, which the caller derives from the
/// same client key).
fn idempotency_for<H: Handler>(key: Option<&str>) -> Option<&str> {
    if H::INFO.kind == ServiceKind::Workflow {
        None
    } else {
        key
    }
}

/// An error body from ingress: `{"code": 409, "message": "...", "source"?: "ingress"}`.
#[derive(Deserialize)]
struct IngressError {
    #[serde(default)]
    code: Option<u16>,
    #[serde(default)]
    message: String,
    #[serde(default)]
    source: Option<String>,
}

/// Turns an ingress response into its body or an [`ApiError`].
pub fn interpret(path: &str, status: u16, body: &str) -> ApiResult<String> {
    if (200..300).contains(&status) {
        return Ok(body.to_owned());
    }
    let parsed: Option<IngressError> = serde_json::from_str(body).ok();
    let from_ingress = parsed
        .as_ref()
        .is_some_and(|e| e.source.as_deref() == Some("ingress"));
    match parsed {
        // A terminal error of the handler: its message is the ApiError JSON.
        Some(e) if !from_ingress && serde_json::from_str::<ApiError>(&e.message).is_ok() => Err(
            ingress::api_error_from_terminal(e.code.unwrap_or(status), &e.message),
        ),
        Some(e) if !from_ingress && (400..500).contains(&status) => Err(
            ingress::api_error_from_terminal(e.code.unwrap_or(status), &e.message),
        ),
        _ => {
            log(&format!("Restate ingress {path} answered {status}: {body}"));
            let message = if from_ingress && status == 404 {
                "This feature is not available yet. Try again later."
            } else {
                "The order system is temporarily unavailable. Try again in a moment."
            };
            Err(ApiError::new(ErrorCode::Unavailable, message))
        }
    }
}

fn decode_output<T: DeserializeOwned>(path: &str, body: &str) -> ApiResult<T> {
    let body = if body.trim().is_empty() { "null" } else { body };
    serde_json::from_str(body).map_err(|e| {
        log(&format!(
            "Restate ingress {path}: unexpected output {body}: {e}"
        ));
        ApiError::new(
            ErrorCode::Internal,
            "Unexpected answer from the order system.",
        )
    })
}

#[cfg(target_arch = "wasm32")]
mod transport {
    use send_wrapper::SendWrapper;
    use worker::wasm_bindgen::JsValue;
    use worker::{Fetch, Headers, Method, Request, RequestInit};

    /// `POST url`; returns the status and body. The Fetch future is `!Send`; it is wrapped in
    /// `SendWrapper` (sound: a Worker isolate is single-threaded).
    pub async fn post(
        url: &str,
        api_key: Option<&str>,
        idempotency_key: Option<&str>,
        body: Option<&str>,
    ) -> worker::Result<(u16, String)> {
        let url = url.to_owned();
        let api_key = api_key.map(str::to_owned);
        let idempotency_key = idempotency_key.map(str::to_owned);
        let body = body.map(str::to_owned);
        SendWrapper::new(async move {
            let headers = Headers::new();
            if body.is_some() {
                headers.set("content-type", "application/json")?;
            }
            if let Some(key) = &api_key {
                headers.set("authorization", &format!("Bearer {key}"))?;
            }
            if let Some(key) = &idempotency_key {
                headers.set("idempotency-key", key)?;
            }
            let mut init = RequestInit::new();
            init.with_method(Method::Post)
                .with_headers(headers)
                .with_body(body.as_deref().map(JsValue::from_str));
            let request = Request::new_with_init(&url, &init)?;
            let mut response = Fetch::Request(request).send().await?;
            let status = response.status_code();
            let text = response.text().await.unwrap_or_default();
            Ok((status, text))
        })
        .await
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod transport {
    /// Native builds (unit tests) have no Fetch.
    pub async fn post(
        _url: &str,
        _api_key: Option<&str>,
        _idempotency_key: Option<&str>,
        _body: Option<&str>,
    ) -> Result<(u16, String), String> {
        Err("no Fetch API outside Workers".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_errors_pass_through() {
        let err = ApiError::new(ErrorCode::OutOfStock, "Only 2 left");
        let body = serde_json::json!({
            "code": 409,
            "message": ingress::terminal_message(&err),
        })
        .to_string();
        assert_eq!(interpret("/Customer/k/checkout", 409, &body), Err(err));
    }

    #[test]
    fn unknown_services_are_unavailable() {
        let body =
            r#"{"code":404,"message":"service 'SupportTicket' not found","source":"ingress"}"#;
        let err = interpret("/SupportTicket/k/inbound/send", 404, body).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert_eq!(
            interpret("/x", 500, "boom").unwrap_err().code,
            ErrorCode::Unavailable
        );
        assert_eq!(interpret("/x", 200, "{}").unwrap(), "{}");
    }

    #[test]
    fn unit_outputs_decode_from_empty_bodies() {
        decode_output::<()>("/x", "").unwrap();
        assert_eq!(decode_output::<u32>("/x", "7").unwrap(), 7);
    }
}
