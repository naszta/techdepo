//! `Email` (service, ingress-private): the one place that talks to the mail transport.
//!
//! Our own service rather than `restate-email` (which cannot be bound on Workers, see
//! CLAUDE.md). `Email/send` takes a fully rendered [`OutboundEmail`] and:
//!
//! 1. **logs it** as `queued` in `email_log` (step `log-email`); a `Message-ID` that was already
//!    sent returns the earlier result, so a re-sent notice never reaches the customer twice;
//! 2. **sends it** through the [`Mailer`] port (step `send-email`). The demo's
//!    **email provider outage** applies here: while the order's `demo_faults.email_outage_until`
//!    lies in the future (real time), the step fails with a *retryable* error, so the
//!    invocation visibly retries in the Restate UI until the outage ends and then delivers.
//!    Each failed attempt is counted on the log entry (`attempts`, `error`). A transport error
//!    that is retryable is retried the same way; a rejection is terminal and logged `failed`;
//! 3. **marks it** `sent` with the provider's ID (step `mark-sent`).
//!
//! The service's retry policy (`policy::defaults`) keeps retrying for well over the longest
//! outage (60 minutes) with a capped interval, so delivery follows the outage's end closely.

use std::sync::Arc;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::EmailId;
use techdepo_api_types::ingress::derive_id;
use techdepo_api_types::vocab::{Direction, EmailStatus};
use techdepo_db::OrgScope;
use techdepo_db::email_log::NewEmail;
use techdepo_domain::{Channel, OrderId, OrgId, TicketId, UserId};
use techdepo_ports::{MailError, OutboundEmail};
use time::OffsetDateTime;

use crate::Deps;
use crate::errors::{internal, reject, sql};
use crate::util::{self, wall_clock};

/// `Email/send` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendEmailInput {
    /// Org (the log entry's scope).
    pub org_id: OrgId,
    /// The email. Its `message_id` is the idempotency key.
    pub email: OutboundEmail,
    /// Template name for the log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Recipient member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<UserId>,
    /// Ticket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_id: Option<TicketId>,
    /// Order (its email-outage fault applies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_id: Option<OrderId>,
}

/// `Email/send` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentEmail {
    /// The `email_log` entry.
    pub email_id: EmailId,
    /// The provider's message ID.
    pub provider_id: String,
}

/// The log entry of a `Message-ID` in an org (deterministic, so retries and re-sends find it).
pub fn email_log_id(org: &OrgId, message_id: &str) -> EmailId {
    EmailId::new(derive_id("eml", &[org.as_str(), message_id]))
}

/// Whether a simulated provider outage is still on at `now`.
pub fn outage_active(until: Option<OffsetDateTime>, now: OffsetDateTime) -> bool {
    until.is_some_and(|until| now < until)
}

/// The error of an attempt during a simulated outage.
pub fn outage_error(until: OffsetDateTime) -> MailError {
    MailError::Unavailable(format!(
        "503 from the email provider (demo outage until {:02}:{:02}:{:02} UTC)",
        until.hour(),
        until.minute(),
        until.second()
    ))
}

/// The `Email` service.
pub struct Email {
    deps: Arc<Deps>,
}

impl Email {
    /// The service over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }
}

#[restate_sdk::service(name = "Email", ingress_private = true)]
impl Email {
    /// Logs, sends (retrying through outages) and marks one email.
    #[handler]
    async fn send(
        &self,
        ctx: Context<'_>,
        input: Json<SendEmailInput>,
    ) -> Result<Json<SentEmail>, HandlerError> {
        let input = input.into_inner();
        let at = util::now(&ctx).await?;
        let id = email_log_id(&input.org_id, &input.email.message_id);
        let scope = OrgScope::verified(input.org_id.clone());

        let db = self.deps.db.clone();
        let entry = NewEmail {
            id: id.to_string(),
            direction: Some(Direction::Outbound),
            channel: Some(Channel::Email),
            status: Some(EmailStatus::Queued),
            message_id: Some(input.email.message_id.clone()),
            in_reply_to: input
                .email
                .headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case("In-Reply-To"))
                .map(|h| h.value.trim_matches(['<', '>']).to_owned()),
            references: input
                .email
                .headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case("References"))
                .map(|h| {
                    h.value
                        .split_whitespace()
                        .map(|r| r.trim_matches(['<', '>']).to_owned())
                        .collect()
                })
                .unwrap_or_default(),
            from_address: input.email.from.email.clone(),
            from_name: input.email.from.name.clone(),
            to_address: input.email.to.email.clone(),
            reply_to: input.email.reply_to.as_ref().map(|r| r.email.clone()),
            subject: input.email.subject.clone(),
            text_body: Some(input.email.text.clone()),
            html_body: input.email.html.clone(),
            template: input.template.clone(),
            user_id: input.user_id.clone(),
            ticket_id: input.ticket_id.clone(),
            order_id: input.order_id.clone(),
            created_at_ms: at.unix_timestamp_nanos().div_euclid(1_000_000) as i64,
            ..NewEmail::default()
        };
        let (log_scope, log_id) = (scope.clone(), id.clone());
        let already: Json<Option<SentEmail>> = ctx
            .run(|| async move {
                let log = db.email_log(&log_scope);
                db.batch(&[log.insert_statement(&entry).map_err(sql)?])
                    .await
                    .map_err(sql)?;
                let row = log
                    .get(&log_id)
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| internal("email log entry missing after insert"))?;
                Ok(Json(match (row.status, row.provider_id) {
                    (EmailStatus::Sent, Some(provider_id)) => Some(SentEmail {
                        email_id: log_id,
                        provider_id,
                    }),
                    _ => None,
                }))
            })
            .name("log-email")
            .await?;
        if let Some(sent) = already.into_inner() {
            return Ok(Json(sent));
        }

        let db = self.deps.db.clone();
        let mailer = self.deps.mailer.clone();
        let (send_scope, send_id, order_id, email) = (
            scope.clone(),
            id.clone(),
            input.order_id.clone(),
            input.email.clone(),
        );
        let provider_id: String = ctx
            .run(|| async move {
                let log = db.email_log(&send_scope);
                let outage_until = match &order_id {
                    Some(order) => db
                        .orders(&send_scope)
                        .faults(order)
                        .await
                        .map_err(sql)?
                        .and_then(|f| f.email_outage_until),
                    None => None,
                };
                let result = if outage_active(outage_until, wall_clock()) {
                    Err(outage_error(
                        outage_until.unwrap_or(OffsetDateTime::UNIX_EPOCH),
                    ))
                } else {
                    mailer.send(&email).await
                };
                match result {
                    Ok(provider_id) => Ok(provider_id),
                    Err(e) if e.is_retryable() => {
                        // Best effort: the attempt counter is for people, the retry is Restate's.
                        let _ = db
                            .batch(&[log.attempt_failed_statement(&send_id, &e.to_string())])
                            .await;
                        Err(HandlerError::from(e))
                    }
                    Err(e) => {
                        db.batch(&[log.failed_statement(&send_id, &e.to_string())])
                            .await
                            .map_err(sql)?;
                        Err(reject(techdepo_api_types::ApiError::new(
                            techdepo_api_types::ErrorCode::Internal,
                            format!("Email {} not sent: {e}", email.message_id),
                        )))
                    }
                }
            })
            .name("send-email")
            .await?;

        let db = self.deps.db.clone();
        let (mark_id, mark_provider) = (id.clone(), provider_id.clone());
        let sent_at = util::now(&ctx).await?;
        ctx.run(|| async move {
            db.batch(&[db
                .email_log(&scope)
                .sent_statement(&mark_id, &mark_provider, sent_at)])
                .await
                .map_err(sql)?;
            Ok(())
        })
        .name("mark-sent")
        .await?;
        Ok(Json(SentEmail {
            email_id: id,
            provider_id,
        }))
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    #[test]
    fn outage_window() {
        let until = datetime!(2026-10-16 14:05 UTC);
        assert!(outage_active(
            Some(until),
            datetime!(2026-10-16 14:04:59 UTC)
        ));
        assert!(!outage_active(Some(until), until));
        assert!(!outage_active(None, until));
        let e = outage_error(until);
        assert!(e.is_retryable());
        assert!(e.to_string().contains("14:05:00"));
    }

    #[test]
    fn log_ids_are_per_org_and_message() {
        let a = email_log_id(
            &OrgId::new("org_1"),
            "order.ord_1.order_confirmation@techdepo.dev",
        );
        assert_eq!(
            a,
            email_log_id(
                &OrgId::new("org_1"),
                "order.ord_1.order_confirmation@techdepo.dev"
            )
        );
        assert_ne!(
            a,
            email_log_id(
                &OrgId::new("org_2"),
                "order.ord_1.order_confirmation@techdepo.dev"
            )
        );
        assert!(a.as_str().starts_with("eml_"));
    }
}
