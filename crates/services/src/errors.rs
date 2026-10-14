//! Turning domain and port failures into Restate errors.
//!
//! Rejections are **terminal** errors whose code is the HTTP status and whose message is the
//! [`ApiError`] as JSON (`ingress::terminal_message`); the web worker passes them through. A
//! retryable port error (D1 unreachable) stays a plain error, so Restate retries it.

use restate_ext::TerminalErrorExt;
use restate_sdk::prelude::*;
use techdepo_api_types::ingress::terminal_message;
use techdepo_api_types::{ApiError, ErrorCode};
use techdepo_domain::{OrgId, TransitionError};
use techdepo_ports::SqlError;

/// `error` as a terminal error.
pub fn terminal(error: &ApiError) -> TerminalError {
    TerminalError::new_with_code(error.status(), terminal_message(error))
}

/// `error` as a handler error.
pub fn reject(error: ApiError) -> HandlerError {
    terminal(&error).into()
}

/// `404` for a record missing from the org (or someone else's).
pub fn not_found(what: impl std::fmt::Display) -> HandlerError {
    reject(ApiError::not_found(what))
}

/// `409 invalid_transition` for an action the order state machine does not allow now, e.g.
/// "Cannot ship an order that is awaiting payment".
pub fn invalid_transition(error: TransitionError) -> HandlerError {
    reject(ApiError::new(
        ErrorCode::InvalidTransition,
        transition_message(error),
    ))
}

fn transition_message(error: TransitionError) -> String {
    format!(
        "Cannot {} an order that is {}",
        error.action.as_str().replace('_', " "),
        error.from.label().to_lowercase()
    )
}

/// `409 invalid_transition` with a message.
pub fn not_now(message: impl Into<String>) -> HandlerError {
    reject(ApiError::new(ErrorCode::InvalidTransition, message))
}

/// `422` for one invalid field.
pub fn invalid(field: &str, message: impl Into<String>) -> HandlerError {
    reject(ApiError::invalid_field(field, message))
}

/// A database error: retryable ones are retried by Restate, anything else is a bug (`500`).
pub fn sql(error: SqlError) -> HandlerError {
    if error.is_retryable() {
        HandlerError::from(error)
    } else {
        reject(ApiError::new(
            ErrorCode::Internal,
            format!("Database error: {error}"),
        ))
    }
}

/// A broken invariant (`500`, terminal).
pub fn internal(message: impl std::fmt::Display) -> HandlerError {
    reject(ApiError::new(ErrorCode::Internal, message.to_string()))
}

/// Any other error, as a terminal `500` that keeps its message (`restate_ext`'s
/// `TerminalErrorExt`, then the status code).
pub fn bug<T, E: std::fmt::Display + Send + Sync + 'static>(
    result: Result<T, E>,
) -> Result<T, HandlerError> {
    result
        .terminal()
        .map_err(|e| terminal(&ApiError::new(ErrorCode::Internal, e.message().to_owned())).into())
}

/// Checks that a `<org>:<rest>` object key belongs to `org`.
pub fn check_key_org<'k>(key: &'k str, org: &OrgId) -> Result<&'k str, HandlerError> {
    match techdepo_api_types::ingress::keys::split(key) {
        Some((key_org, rest)) if key_org == org.as_str() => Ok(rest),
        _ => Err(not_found("record")),
    }
}

#[cfg(test)]
mod tests {
    use techdepo_api_types::ingress::api_error_from_terminal;
    use techdepo_domain::{OrderAction, OrderStatus};

    use super::*;

    #[test]
    fn transition_errors_are_409_invalid_transition() {
        let err = OrderStatus::Shipped.apply(OrderAction::Cancel).unwrap_err();
        let api = ApiError::new(ErrorCode::InvalidTransition, transition_message(err));
        let t = terminal(&api);
        assert_eq!(t.code(), 409);
        let back = api_error_from_terminal(t.code(), t.message());
        assert_eq!(back.code, ErrorCode::InvalidTransition);
        assert_eq!(back.message, "Cannot cancel an order that is shipped");
        let err = OrderStatus::AwaitingPayment
            .apply(OrderAction::Ship)
            .unwrap_err();
        assert_eq!(
            transition_message(err),
            "Cannot ship an order that is awaiting payment"
        );
    }

    #[test]
    fn keys_are_checked_against_the_org() {
        let org = OrgId::new("org_1");
        assert_eq!(check_key_org("org_1:user_1", &org).ok(), Some("user_1"));
        assert!(check_key_org("org_2:user_1", &org).is_err());
        assert!(check_key_org("org_1", &org).is_err());
    }
}
