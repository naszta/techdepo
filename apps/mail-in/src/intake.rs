//! The intake pipeline (support.md §1), plain code over the ports so it runs natively in tests:
//!
//! 1. the recipient must be a support address of a known org (`support+<public>[.<ticket>]@`);
//! 2. size, MIME, automatic mail, our own domain, DMARC (fail closed: our MTA's
//!    `Authentication-Results` must say `dmarc=pass`), the org's hourly inbound limit;
//! 3. the sender must be a member of that org ([`MemberDirectory`]);
//! 4. the ticket: the address's ticket part, else `In-Reply-To` / `References` looked up in
//!    `email_log` and `ticket_messages`, else `TicketId::derive(public org, Message-ID)`
//!    (deterministic, so a redelivery cannot fork a ticket). A ticket of another member is never
//!    continued: the message opens its own ticket instead;
//! 5. quoted history and signatures are stripped from the text the ticket gets; `email_log`
//!    keeps the full text.
//!
//! Every message is logged to `email_log`: accepted, or rejected with its reason (in the org's
//! log when the org is known, unscoped otherwise). Rejected mail gets no reply (no backscatter).
//! A failure that may pass (D1, the directory) is [`Temporary`]: the sending server retries.
//! The caller then hands an accepted message to `SupportTicket/inbound` with
//! `Idempotency-Key: <Message-ID>`; a redelivered message reuses its log entry and ticket.

use techdepo_api_types::EmailId;
use techdepo_api_types::ingress::{InboundMessage, derive_id, keys};
use techdepo_api_types::support::MAX_MESSAGE_CHARS;
use techdepo_api_types::vocab::{Direction, EmailStatus, RejectReason};
use techdepo_db::email_log::{NewEmail, insert_unscoped_rejection_statement};
use techdepo_db::orgs::{OrgRow, find_by_public_id};
use techdepo_db::{Db, OrgScope, SqlError};
use techdepo_domain::{Channel, TicketId, UserId, fnv1a64};
use techdepo_ports::MemberDirectory;
use time::OffsetDateTime;

use crate::address::{self, SupportAddress, bare};
use crate::filters;
use crate::parse::{self, ParsedEmail};
use crate::quote::strip_quoted;

/// Intake settings (from the worker's environment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntakeConfig {
    /// `EMAIL_DOMAIN`.
    pub domain: String,
    /// `SUPPORT_LOCAL_PART`.
    pub support_local_part: String,
    /// `MAX_EMAIL_BYTES`: larger mail is dropped.
    pub max_bytes: usize,
    /// `MAX_INBOUND_PER_HOUR`: accepted emails per org and hour.
    pub max_per_org_per_hour: u32,
    /// How the sender is authenticated (`INBOUND_SENDER_AUTH`, `INBOUND_AUTHSERV_ID`).
    pub sender_auth: filters::SenderAuth,
}

impl Default for IntakeConfig {
    fn default() -> Self {
        Self {
            domain: "techdepo.dev".to_owned(),
            support_local_part: "support".to_owned(),
            max_bytes: 2 * 1024 * 1024,
            max_per_org_per_hour: 100,
            sender_auth: filters::SenderAuth::default(),
        }
    }
}

/// One delivery: the SMTP envelope and the raw message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    /// `MAIL FROM`.
    pub from: String,
    /// `RCPT TO` (the support address).
    pub to: String,
    /// The raw RFC 822 message (empty when oversized: it is not read).
    pub raw: Vec<u8>,
    /// Its size.
    pub raw_size: usize,
    /// When it arrived.
    pub received_at: OffsetDateTime,
}

/// What intake decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Hand it to `SupportTicket/<key>/inbound`.
    Accepted {
        /// The object key, `<org>:<ticket>`.
        key: String,
        /// The message.
        message: Box<InboundMessage>,
    },
    /// Dropped (and logged).
    Rejected {
        /// Why.
        reason: RejectReason,
        /// Details for the worker log.
        detail: String,
    },
}

/// A failure that may pass: the sender should retry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("temporary failure: {0}")]
pub struct Temporary(pub String);

impl From<SqlError> for Temporary {
    fn from(e: SqlError) -> Self {
        Self(format!("database: {e}"))
    }
}

/// Longest text body logged.
const MAX_LOGGED_CHARS: usize = 100_000;

fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((i, _)) => text[..i].to_owned(),
        None => text.to_owned(),
    }
}

/// The `Message-ID`, or a stand-in derived from the delivery (stable across redeliveries).
fn message_id(parsed: Option<&ParsedEmail>, env: &Envelope, domain: &str) -> String {
    parsed
        .and_then(|p| p.message_id.clone())
        .unwrap_or_else(|| {
            let mut bytes = env.raw.clone();
            bytes.extend_from_slice(
                format!("\n{}\n{}\n{}", env.from, env.to, env.raw_size).as_bytes(),
            );
            format!("mailin.{:016x}@{domain}", fnv1a64(&bytes))
        })
}

/// The intake pipeline over a database and a member directory.
pub struct Intake<'a> {
    db: &'a Db,
    directory: &'a dyn MemberDirectory,
    config: &'a IntakeConfig,
}

impl<'a> Intake<'a> {
    /// The pipeline.
    pub fn new(db: &'a Db, directory: &'a dyn MemberDirectory, config: &'a IntakeConfig) -> Self {
        Self {
            db,
            directory,
            config,
        }
    }

    /// Runs one delivery through the checks, logs it, and says what to do with it.
    pub async fn process(&self, env: &Envelope) -> Result<Outcome, Temporary> {
        let cfg = self.config;
        let oversized = env.raw_size > cfg.max_bytes;
        let parsed = if oversized {
            None
        } else {
            parse::parse(&env.raw)
        };
        let message_id = message_id(parsed.as_ref(), env, &cfg.domain);
        let from = parsed
            .as_ref()
            .and_then(|p| p.from.clone())
            .unwrap_or_else(|| bare(&env.from));
        let reject = Rejection {
            env,
            parsed: parsed.as_ref(),
            message_id: &message_id,
            from: &from,
        };

        let Some(recipient) = address::parse(&env.to, &cfg.support_local_part, &cfg.domain) else {
            return reject
                .log(
                    self.db,
                    None,
                    RejectReason::UnknownOrg,
                    "not a support address",
                )
                .await;
        };
        let Some(org) = find_by_public_id(self.db.sql(), &recipient.public_org_id).await? else {
            return reject
                .log(
                    self.db,
                    None,
                    RejectReason::UnknownOrg,
                    "no org has this tag",
                )
                .await;
        };
        let scope = OrgScope::verified(org.id.clone());
        if oversized {
            let detail = format!("{} bytes", env.raw_size);
            return reject
                .log(self.db, Some(&scope), RejectReason::Oversized, &detail)
                .await;
        }
        let Some(parsed) = parsed.as_ref() else {
            return reject
                .log(
                    self.db,
                    Some(&scope),
                    RejectReason::Malformed,
                    "unparseable MIME",
                )
                .await;
        };
        let from_domain = address::domain_of(&from);
        if let Some(reason) = filters::screen(
            parsed,
            from_domain.as_deref(),
            &cfg.domain,
            &cfg.sender_auth,
        ) {
            return reject.log(self.db, Some(&scope), reason, "filtered").await;
        }

        let email_log = self.db.email_log(&scope);
        let existing = email_log.inbound_by_message_id(&message_id).await?;
        if existing.is_none() {
            let since = env.received_at - time::Duration::hours(1);
            if email_log.inbound_accepted_since(since).await? >= cfg.max_per_org_per_hour {
                return reject
                    .log(
                        self.db,
                        Some(&scope),
                        RejectReason::RateLimited,
                        "hourly limit",
                    )
                    .await;
            }
        }

        let member = match self.directory.is_member(&org.id, &from).await {
            Ok(Some(member)) => member,
            Ok(None) => {
                return reject
                    .log(self.db, Some(&scope), RejectReason::NotMember, &from)
                    .await;
            }
            Err(e) => return Err(Temporary(e.to_string())),
        };

        let ticket_id = match existing.as_ref().and_then(|e| e.ticket_id.clone()) {
            // A redelivery: the same ticket as the first time.
            Some(ticket) => ticket,
            None => {
                self.resolve_ticket(
                    &scope,
                    &org,
                    &recipient,
                    parsed,
                    &member.user_id,
                    &message_id,
                )
                .await?
            }
        };
        let id = derive_id("eml", &[org.id.as_str(), "inbound", &message_id]);
        if existing.is_none() {
            let entry = NewEmail {
                id: id.clone(),
                direction: Some(Direction::Inbound),
                channel: Some(Channel::Email),
                status: Some(EmailStatus::Accepted),
                message_id: Some(message_id.clone()),
                in_reply_to: parsed.in_reply_to.last().cloned(),
                references: parsed.references.clone(),
                from_address: from.clone(),
                from_name: parsed.from_name.clone(),
                to_address: bare(&env.to),
                reply_to: parsed.reply_to.clone(),
                subject: parsed.subject.clone(),
                text_body: Some(truncate(&parsed.text, MAX_LOGGED_CHARS)),
                html_body: parsed
                    .html
                    .as_deref()
                    .map(|h| truncate(h, MAX_LOGGED_CHARS)),
                raw_size: u32::try_from(env.raw_size).ok(),
                auth_results: parsed.authentication_results.clone(),
                user_id: Some(member.user_id.clone()),
                ticket_id: Some(ticket_id.clone()),
                created_at_ms: millis(env.received_at),
                ..NewEmail::default()
            };
            self.db
                .batch(&[email_log.insert_statement(&entry)?])
                .await?;
        }
        let id = existing.map_or_else(|| EmailId::new(id), |e| e.id);

        let body = truncate(&strip_quoted(&parsed.text), MAX_MESSAGE_CHARS);
        let message = InboundMessage {
            org_id: org.id.clone(),
            public_org_id: org.public_id.clone(),
            ticket_id: ticket_id.clone(),
            user_id: member.user_id,
            customer_email: member.email,
            // The directory's name first: the `From` display name is the sender's to choose.
            customer_name: member.name.or_else(|| parsed.from_name.clone()),
            channel: Channel::Email,
            message_id,
            in_reply_to: parsed.in_reply_to.last().cloned(),
            references: parsed.references.clone(),
            subject: if parsed.subject.is_empty() {
                "(no subject)".to_owned()
            } else {
                truncate(&parsed.subject, 200)
            },
            body_text: if body.is_empty() {
                "(no text)".to_owned()
            } else {
                body
            },
            received_at: env.received_at,
            email_log_id: Some(id),
        };
        Ok(Outcome::Accepted {
            key: keys::ticket(&org.id, &ticket_id),
            message: Box::new(message),
        })
    }

    /// Which ticket the message continues, or the new ticket's ID.
    async fn resolve_ticket(
        &self,
        scope: &OrgScope,
        org: &OrgRow,
        recipient: &SupportAddress,
        parsed: &ParsedEmail,
        user: &UserId,
        message_id: &str,
    ) -> Result<TicketId, Temporary> {
        let tickets = self.db.tickets(scope);
        // Another member's ticket is never continued.
        let usable = |ticket: TicketId| async move {
            match tickets.get(&ticket).await? {
                Some(row) if row.user_id != *user => Ok::<_, SqlError>(None),
                _ => Ok(Some(ticket)),
            }
        };
        if let Some(ticket) = recipient.ticket_id.clone()
            && let Some(ticket) = usable(ticket).await?
        {
            return Ok(ticket);
        }
        let mut thread: Vec<String> = parsed.references.clone();
        for id in &parsed.in_reply_to {
            if !thread.contains(id) {
                thread.push(id.clone());
            }
        }
        if !thread.is_empty() {
            let logged = self
                .db
                .email_log(scope)
                .ticket_for_message_ids(&thread)
                .await?;
            let found = match logged {
                Some(ticket) => Some(ticket),
                None => tickets.find_by_message_ids(&thread).await?,
            };
            if let Some(ticket) = found
                && let Some(ticket) = usable(ticket).await?
            {
                return Ok(ticket);
            }
        }
        Ok(TicketId::derive(&org.public_id, message_id))
    }
}

fn millis(at: OffsetDateTime) -> i64 {
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or(0)
}

/// Logs a rejected message.
struct Rejection<'e> {
    env: &'e Envelope,
    parsed: Option<&'e ParsedEmail>,
    message_id: &'e str,
    from: &'e str,
}

impl Rejection<'_> {
    async fn log(
        &self,
        db: &Db,
        scope: Option<&OrgScope>,
        reason: RejectReason,
        detail: &str,
    ) -> Result<Outcome, Temporary> {
        let p = self.parsed;
        let org = scope.map_or("-", |s| s.org_id().as_str());
        let entry = NewEmail {
            id: derive_id("eml", &[org, "rejected", self.message_id]),
            direction: Some(Direction::Inbound),
            channel: Some(Channel::Email),
            status: Some(EmailStatus::Rejected),
            reject_reason: Some(reason),
            message_id: Some(self.message_id.to_owned()),
            in_reply_to: p.and_then(|p| p.in_reply_to.last().cloned()),
            references: p.map(|p| p.references.clone()).unwrap_or_default(),
            from_address: self.from.to_owned(),
            from_name: p.and_then(|p| p.from_name.clone()),
            to_address: bare(&self.env.to),
            reply_to: p.and_then(|p| p.reply_to.clone()),
            subject: p.map(|p| truncate(&p.subject, 500)).unwrap_or_default(),
            text_body: p.map(|p| truncate(&p.text, MAX_LOGGED_CHARS)),
            raw_size: u32::try_from(self.env.raw_size).ok(),
            auth_results: p.and_then(|p| p.authentication_results.clone()),
            created_at_ms: millis(self.env.received_at),
            ..NewEmail::default()
        };
        let statement = match scope {
            Some(scope) => db.email_log(scope).insert_statement(&entry)?,
            None => insert_unscoped_rejection_statement(&entry)?,
        };
        db.batch(&[statement]).await?;
        Ok(Outcome::Rejected {
            reason,
            detail: detail.to_owned(),
        })
    }
}
