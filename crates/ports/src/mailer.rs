//! Outbound email transport: the [`Mailer`] port.
//!
//! Only our `Email` Restate service (in `crates/services`) calls this, inside `ctx.run`, so
//! sends are durable and retried. Everything else (`Notifications`, `SupportTicket`) calls that
//! service. The service is also where `email_log` rows are written and where the demo's
//! email-outage fault is applied, before the transport is reached.
//!
//! Transports: Resend's HTTP API (`EMAIL_TRANSPORT=resend`) and Mailpit's HTTP API
//! (`POST {MAILPIT_URL}/api/v1/send`, `EMAIL_TRANSPORT=mailpit`).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// A name and an address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mailbox {
    /// Address, e.g. `support+k3x9q2ab@techdepo.dev`.
    pub email: String,
    /// Display name, e.g. `TechDepó Support`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Mailbox {
    /// A bare address.
    pub fn new(email: impl Into<String>) -> Self {
        Self {
            email: email.into(),
            name: None,
        }
    }

    /// An address with a display name.
    pub fn named(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            email: email.into(),
            name: Some(name.into()),
        }
    }
}

impl fmt::Display for Mailbox {
    /// `Name <address>` (the name quoted if it contains specials), or the bare address.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(name) if name.contains([',', ';', '<', '>', '"', '(', ')', '@', ':']) => {
                write!(f, "\"{}\" <{}>", name.replace('"', "\\\""), self.email)
            }
            Some(name) => write!(f, "{name} <{}>", self.email),
            None => f.write_str(&self.email),
        }
    }
}

/// An extra header, e.g. `In-Reply-To`, `References` or `Auto-Submitted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// Header name.
    pub name: String,
    /// Header value, already formatted (e.g. `<a@x> <b@x>` for `References`).
    pub value: String,
}

impl Header {
    /// A header.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// A text attachment, such as the invoice HTML on the confirmation email. Binary attachments
/// are out of scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// File name, e.g. `TD-2026-000042.html`.
    pub filename: String,
    /// MIME type, e.g. `text/html; charset=utf-8`.
    pub content_type: String,
    /// UTF-8 content.
    pub content: String,
}

/// One email, fully rendered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundEmail {
    /// `Message-ID` without angle brackets, e.g. `T-7F2M9Q.3@techdepo.dev`. Chosen by the caller
    /// and used for threading and as the send's idempotency key.
    pub message_id: String,
    /// Sender.
    pub from: Mailbox,
    /// The one recipient (every email here goes to one customer).
    pub to: Mailbox,
    /// `Reply-To`, e.g. `support+<org>.<ticket>@techdepo.dev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Mailbox>,
    /// Subject.
    pub subject: String,
    /// Plain-text body (always present).
    pub text: String,
    /// HTML body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    /// Extra headers. Transports set `Message-ID`, `From`, `To`, `Reply-To` and `Subject`
    /// themselves; those names must not appear here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<Header>,
    /// Text attachments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

/// A send failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MailError {
    /// Network failure, timeout, 5xx or throttling. Retryable (this is how the demo's
    /// provider-outage fault shows up in the Restate UI).
    #[error("mail transport unavailable: {0}")]
    Unavailable(String),
    /// The provider refused this email (invalid address, unverified domain, …). Not retryable.
    #[error("email rejected: {0}")]
    Rejected(String),
    /// Misconfiguration (missing API key). Not retryable.
    #[error("mail transport misconfigured: {0}")]
    Config(String),
}

impl MailError {
    /// Whether retrying the same send may succeed.
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

/// Sends email.
#[async_trait]
pub trait Mailer: Send + Sync {
    /// Sends `email` and returns the provider's message ID (Resend's `id`, Mailpit's `ID`).
    async fn send(&self, email: &OutboundEmail) -> Result<String, MailError>;
}

#[async_trait]
impl<T: Mailer + ?Sized> Mailer for Arc<T> {
    async fn send(&self, email: &OutboundEmail) -> Result<String, MailError> {
        (**self).send(email).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailbox_display() {
        assert_eq!(
            Mailbox::named("TechDepó Support", "support+k3x9q2ab@techdepo.dev").to_string(),
            "TechDepó Support <support+k3x9q2ab@techdepo.dev>"
        );
        assert_eq!(
            Mailbox::named("Doe, Jane", "jane@example.com").to_string(),
            "\"Doe, Jane\" <jane@example.com>"
        );
        assert_eq!(Mailbox::new("a@b.c").to_string(), "a@b.c");
    }
}
