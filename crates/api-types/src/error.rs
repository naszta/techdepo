//! The error body of every non-2xx `/api` response.

use std::fmt;

use serde::{Deserialize, Serialize};
use techdepo_domain::labeled_enum;

labeled_enum! {
    /// Machine-readable error kind. Each maps to one HTTP status ([`ErrorCode::status`]).
    pub enum ErrorCode {
        /// 400: malformed JSON, query or path parameter.
        BadRequest => ("bad_request", "Bad request"),
        /// 401: no or invalid credentials.
        Unauthenticated => ("unauthenticated", "Sign in required"),
        /// 403: signed in, but not allowed (e.g. not `org:admin`).
        Forbidden => ("forbidden", "Not allowed"),
        /// 404: unknown route, or a record that does not exist in this org (or for this user).
        NotFound => ("not_found", "Not found"),
        /// 409: the record changed state (generic).
        Conflict => ("conflict", "Conflict"),
        /// 409: the order state machine does not allow the action now.
        InvalidTransition => ("invalid_transition", "Not possible right now"),
        /// 409: not enough stock to place the order.
        OutOfStock => ("out_of_stock", "Out of stock"),
        /// 409: the cart changed since the checkout page loaded (`expected_total` mismatch).
        CartChanged => ("cart_changed", "Cart changed"),
        /// 409: the cart has compatibility errors and the order did not override them.
        IncompatibleBuild => ("incompatible_build", "Incompatible parts"),
        /// 409: the human task was already decided.
        AlreadyDecided => ("already_decided", "Already decided"),
        /// 422: the request is well-formed but invalid; see [`ApiError::fields`].
        Validation => ("validation", "Invalid input"),
        /// 429: rate limit or LLM budget exhausted.
        RateLimited => ("rate_limited", "Too many requests"),
        /// 503: Restate or D1 unreachable; retry later.
        Unavailable => ("unavailable", "Temporarily unavailable"),
        /// 500: a bug.
        Internal => ("internal", "Something went wrong"),
    }
}

impl ErrorCode {
    /// The HTTP status for this code.
    pub const fn status(self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::Unauthenticated => 401,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::Conflict
            | Self::InvalidTransition
            | Self::OutOfStock
            | Self::CartChanged
            | Self::IncompatibleBuild
            | Self::AlreadyDecided => 409,
            Self::Validation => 422,
            Self::RateLimited => 429,
            Self::Unavailable => 503,
            Self::Internal => 500,
        }
    }

    /// The generic code for an HTTP status (for statuses without a more specific code).
    pub const fn from_status(status: u16) -> Self {
        match status {
            400 => Self::BadRequest,
            401 => Self::Unauthenticated,
            403 => Self::Forbidden,
            404 => Self::NotFound,
            409 => Self::Conflict,
            422 => Self::Validation,
            429 => Self::RateLimited,
            502..=504 => Self::Unavailable,
            _ => Self::Internal,
        }
    }

    /// Whether the client may retry the same request later.
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::RateLimited | Self::Unavailable)
    }
}

/// One invalid field of a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    /// Field path in the request body, e.g. `shipping_address.postal_code`.
    pub field: String,
    /// What is wrong, ready to show under the field.
    pub message: String,
}

impl FieldError {
    /// A field error.
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

/// An error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    /// What kind of error.
    pub code: ErrorCode,
    /// Human-readable message, safe to show to the user.
    pub message: String,
    /// Invalid fields (for [`ErrorCode::Validation`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldError>,
}

impl ApiError {
    /// An error with a message.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            fields: Vec::new(),
        }
    }

    /// `404` for a missing record, e.g. `ApiError::not_found("order TD-1042")`.
    pub fn not_found(what: impl fmt::Display) -> Self {
        Self::new(ErrorCode::NotFound, format!("{what} not found"))
    }

    /// `422` with field errors.
    pub fn validation(fields: Vec<FieldError>) -> Self {
        let message = match fields.as_slice() {
            [one] => one.message.clone(),
            _ => "Some fields are invalid".to_owned(),
        };
        Self {
            code: ErrorCode::Validation,
            message,
            fields,
        }
    }

    /// `422` for one field.
    pub fn invalid_field(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::validation(vec![FieldError::new(field, message)])
    }

    /// `Ok(())` if `fields` is empty, otherwise a validation error.
    pub fn check(fields: Vec<FieldError>) -> Result<(), Self> {
        if fields.is_empty() {
            Ok(())
        } else {
            Err(Self::validation(fields))
        }
    }

    /// The HTTP status of this error.
    pub const fn status(&self) -> u16 {
        self.code.status()
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ApiError {}

/// Request validation shared by the SPA (before submitting) and the web worker (before calling
/// Restate). Returns a [`ErrorCode::Validation`] error listing every invalid field.
pub trait Validate {
    /// Checks the request.
    fn validate(&self) -> Result<(), ApiError>;
}

/// Pushes a field error if `value` (trimmed) is empty or longer than `max` characters.
pub(crate) fn check_text(
    errors: &mut Vec<FieldError>,
    field: &str,
    value: &str,
    min: usize,
    max: usize,
) {
    let len = value.trim().chars().count();
    if len < min {
        let message = if len == 0 {
            "Required".to_owned()
        } else {
            format!("At least {min} characters")
        };
        errors.push(FieldError::new(field, message));
    } else if value.chars().count() > max {
        errors.push(FieldError::new(field, format!("At most {max} characters")));
    }
}

/// Pushes a field error if `value` is not a client-generated idempotency key: 8–64 characters
/// of `[A-Za-z0-9_-]` (a UUID is fine).
pub(crate) fn check_client_key(errors: &mut Vec<FieldError>, field: &str, value: &str) {
    let valid = (8..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !valid {
        errors.push(FieldError::new(
            field,
            "Must be 8–64 characters of A–Z, a–z, 0–9, - or _",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shape() {
        let err = ApiError::invalid_field("quantity", "At most 99");
        assert_eq!(err.status(), 422);
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            serde_json::json!({
                "code": "validation",
                "message": "At most 99",
                "fields": [{ "field": "quantity", "message": "At most 99" }],
            })
        );
        let plain = ApiError::not_found("order TD-1042");
        assert_eq!(
            serde_json::to_value(&plain).unwrap(),
            serde_json::json!({ "code": "not_found", "message": "order TD-1042 not found" })
        );
        for code in ErrorCode::ALL {
            let generic = ErrorCode::from_status(code.status());
            assert_eq!(generic.status(), code.status(), "{code:?}");
        }
    }
}
