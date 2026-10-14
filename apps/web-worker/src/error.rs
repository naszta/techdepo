//! Every non-2xx response is an [`ApiError`] (`docs/plan/api.md`, "Errors").

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use techdepo_api_types::{ApiError, ErrorCode};
use techdepo_ports::SqlError;

/// Result of a route handler.
pub type ApiResult<T> = Result<T, ApiError>;

/// The response for `error`: its status and the error as JSON.
pub fn error_response(error: &ApiError) -> Response {
    let status = StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, axum::Json(error)).into_response()
}

/// A D1 failure. Details go to the log, not to the client.
pub fn db(error: SqlError) -> ApiError {
    log(&format!("D1: {error}"));
    if error.is_retryable() {
        ApiError::new(
            ErrorCode::Unavailable,
            "The database is temporarily unavailable. Try again in a moment.",
        )
    } else {
        ApiError::new(
            ErrorCode::Internal,
            "Something went wrong reading your data.",
        )
    }
}

/// `400` for a malformed path parameter, query or body.
pub fn bad_request(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::BadRequest, message)
}

/// `409 invalid_transition`.
pub fn invalid_transition(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::InvalidTransition, message)
}

/// Writes to the worker log (`wrangler tail`, the dev console).
pub fn log(message: &str) {
    #[cfg(target_arch = "wasm32")]
    worker::console_error!("{message}");
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("{message}");
}

/// `?` on repository calls.
pub trait DbResultExt<T> {
    /// Maps the D1 error to an [`ApiError`].
    fn db(self) -> ApiResult<T>;
}

impl<T> DbResultExt<T> for Result<T, SqlError> {
    fn db(self) -> ApiResult<T> {
        self.map_err(db)
    }
}

/// `?` on lookups that must find a record.
pub trait FoundExt<T> {
    /// `404 <what> not found` when absent.
    fn found(self, what: impl std::fmt::Display) -> ApiResult<T>;
}

impl<T> FoundExt<T> for Option<T> {
    fn found(self, what: impl std::fmt::Display) -> ApiResult<T> {
        self.ok_or_else(|| ApiError::not_found(what))
    }
}

impl<T> FoundExt<T> for Result<Option<T>, SqlError> {
    fn found(self, what: impl std::fmt::Display) -> ApiResult<T> {
        self.db()?.found(what)
    }
}
