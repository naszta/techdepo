//! The deployment's providers inside a Worker ([`LlmConfig::registry`]): Workers AI through the
//! `AI` binding, and the HTTP providers whose API key is a secret of the Worker. Their requests
//! go out through the Workers Fetch API ([`WorkerFetch`]), not reqwest.

use std::time::Duration;

use allama_http::{
    Headers, HttpRequest, HttpResponse, HttpTransport, Method, SendFuture, TransportError,
    TransportErrorKind,
};
use futures::future::{Either, select};
use send_wrapper::SendWrapper;
use techdepo_domain::ModelProvider;
use worker::js_sys::{Function, Promise, Reflect, Uint8Array, global};
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::{AbortController, Env, Fetch, Request, RequestInit, RequestRedirect};

use crate::config::LlmConfig;
use crate::registry::ModelRegistry;

impl LlmConfig {
    /// Every provider `env` gives access to, with [`LlmConfig::defaults`] as the defaults:
    /// Workers AI when the `AI` binding exists, and each HTTP provider whose secrets are all set
    /// ([`ModelProvider::secrets`]). `secret` reads a secret or variable (empty means unset).
    pub fn registry(&self, env: &Env, secret: impl Fn(&str) -> Option<String>) -> ModelRegistry {
        let mut registry = ModelRegistry::new(self.defaults());
        if let Ok(ai) = allama_cloudflare_worker::Provider::from_env(env, "AI") {
            let ai = std::sync::Arc::new(ai);
            registry = registry
                .with_chat(ModelProvider::WorkersAi, ai.clone())
                .with_decisions(ModelProvider::WorkersAi, ai);
        }
        for &provider in ModelProvider::ALL {
            let Some(values) = provider
                .secrets()
                .iter()
                .map(|&name| secret(name))
                .collect::<Option<Vec<_>>>()
                .filter(|values| !values.is_empty())
            else {
                continue;
            };
            match with_http_provider(registry.clone(), provider, &values) {
                Ok(next) => registry = next,
                Err(e) => worker::console_error!("{}: {e}", provider.as_str()),
            }
        }
        registry
    }
}

/// `registry` plus `provider` with the values of its [`ModelProvider::secrets`], in order,
/// sending through [`WorkerFetch`]. The providers retry nothing themselves: each call runs in a
/// journaled step, which Restate retries.
fn with_http_provider(
    registry: ModelRegistry,
    provider: ModelProvider,
    secrets: &[String],
) -> allama::Result<ModelRegistry> {
    use allama_openai_compat::providers::{Azure, OpenRouter};

    let key = secrets.first().map(String::as_str).unwrap_or_default();
    Ok(match (provider, secrets) {
        (ModelProvider::Mock | ModelProvider::WorkersAi, _) => registry,
        (ModelProvider::Anthropic, _) => registry.with_chat(
            provider,
            allama_anthropic::Provider::builder()
                .api_key(key)
                .http_transport(WorkerFetch)
                .build()?,
        ),
        (ModelProvider::OpenAi, _) => registry.with_chat(
            provider,
            allama_openai::ResponsesProvider::builder()
                .api_key(key)
                .http_transport(WorkerFetch)
                .build()?,
        ),
        (ModelProvider::Gemini, _) => registry.with_chat(
            provider,
            allama_gemini::Provider::builder()
                .api_key(key)
                .http_transport(WorkerFetch)
                .build()?,
        ),
        (ModelProvider::Azure, [key, resource]) => registry.with_chat(
            provider,
            Azure::builder(resource, key)?
                .http_transport(WorkerFetch)
                .build()?,
        ),
        (ModelProvider::AzureResponses, [key, resource]) => registry.with_chat(
            provider,
            Azure::responses_builder(resource, key)?
                .http_transport(WorkerFetch)
                .build()?,
        ),
        (ModelProvider::Azure | ModelProvider::AzureResponses, _) => registry,
        (ModelProvider::OpenRouter, _) => registry.with_chat(
            provider,
            OpenRouter::builder(key)?
                .http_transport(WorkerFetch)
                .build()?,
        ),
    })
}

/// allama's HTTP transport over the Workers Fetch API. Redirects are not followed (allama
/// reports them), the request's timeout aborts the fetch, and the body is read whole: the
/// adapters make unary calls only.
#[derive(Debug, Clone, Copy)]
pub struct WorkerFetch;

impl HttpTransport for WorkerFetch {
    fn send(&self, request: HttpRequest) -> SendFuture<'_> {
        // Sound: a Worker isolate is single-threaded (see `techdepo_ports`).
        Box::pin(SendWrapper::new(fetch(request)))
    }
}

async fn fetch(request: HttpRequest) -> Result<HttpResponse, TransportError> {
    let invalid =
        |e: worker::Error| TransportError::new(TransportErrorKind::InvalidRequest, e.to_string());
    let headers = worker::Headers::new();
    for (name, value) in request.headers.iter() {
        headers.append(name, value).map_err(invalid)?;
    }
    let mut init = RequestInit::new();
    init.with_method(match request.method {
        Method::Get => worker::Method::Get,
        _ => worker::Method::Post,
    })
    .with_headers(headers)
    .with_redirect(RequestRedirect::Manual);
    if let Some(body) = &request.body {
        init.with_body(Some(Uint8Array::from(body.as_ref()).into()));
    }
    let fetch = Fetch::Request(Request::new_with_init(&request.url, &init).map_err(invalid)?);

    let controller = AbortController::default();
    let signal = controller.signal();
    let send = std::pin::pin!(fetch.send_with_signal(&signal));
    let sent = match request.timeout {
        None => send.await,
        Some(timeout) => match select(send, std::pin::pin!(sleep(timeout))).await {
            Either::Left((sent, _)) => sent,
            Either::Right(((), _)) => {
                controller.abort();
                return Err(TransportError::new(
                    TransportErrorKind::Timeout,
                    format!("no response within {timeout:?}"),
                )
                .authored());
            }
        },
    };
    let mut response =
        sent.map_err(|e| TransportError::new(TransportErrorKind::Connect, e.to_string()))?;
    let status = response.status_code();
    let headers: Headers = response.headers().entries().collect();
    let body = response
        .bytes()
        .await
        .map_err(|e| TransportError::new(TransportErrorKind::Body, e.to_string()))?;
    Ok(HttpResponse::from_bytes(status, headers, body))
}

/// A `setTimeout` promise: `worker::Delay` panics in workerd (as `techdepo_db::sleep`).
async fn sleep(duration: Duration) {
    let millis = JsValue::from_f64(duration.as_secs_f64() * 1000.0);
    let promise = Promise::new(&mut |resolve, _reject| {
        let scope = global();
        if let Some(set_timeout) = Reflect::get(&scope, &JsValue::from_str("setTimeout"))
            .ok()
            .and_then(|f| f.dyn_into::<Function>().ok())
        {
            let _ = set_timeout.call2(&scope, &resolve, &millis);
        } else {
            let _ = resolve.call0(&JsValue::NULL);
        }
    });
    let _ = worker::wasm_bindgen_futures::JsFuture::from(promise).await;
}
