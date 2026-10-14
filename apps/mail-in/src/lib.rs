//! `techdepo-mail-in`: the support inbox (support.md §1).
//!
//! Cloudflare Email Routing delivers `support+<org>[.<ticket>]@EMAIL_DOMAIN` to this worker's
//! `email` handler. Plain code decides everything ([`intake`]): parse the MIME ([`parse`]), drop
//! automatic mail, our own domain, DMARC failures and oversized mail ([`filters`]), find the org
//! from the `+tag` ([`address`]), verify the sender is a member ([`directory`]), find or derive
//! the ticket, strip quoted history ([`quote`]), log to `email_log`, and hand the message to
//! `SupportTicket/<org>:<ticket>/inbound/send` with `Idempotency-Key: <Message-ID>`.
//!
//! | Variable | Use |
//! |---|---|
//! | `AUTH_MODE` | `dev`: senders are checked against D1 `org_members` (the dev login fills it), and `POST /inbound` accepts test mail; `clerk`: the Clerk Backend API (`CLERK_SECRET_KEY`, optional KV `MEMBERS_CACHE`). Anything else fails closed. |
//! | `EMAIL_DOMAIN`, `SUPPORT_LOCAL_PART` | the support address |
//! | `RESTATE_INGRESS_URL`, `RESTATE_API_KEY` | Restate ingress |
//! | `MAX_EMAIL_BYTES` (2 MiB), `MAX_INBOUND_PER_HOUR` (100 per org) | limits |
//! | `CLERK_API_URL` | Clerk Backend API base (`https://api.clerk.com/v1`) |
//!
//! Locally, `wrangler dev` triggers the email handler with
//! `curl -X POST 'localhost:8789/cdn-cgi/handler/email?from=a@example.com&to=support+<org>@techdepo.dev' --data-binary @mail.eml`;
//! `POST /inbound?from=…&to=…` (dev only) runs the same pipeline and answers with the outcome.

pub mod address;
pub mod directory;
pub mod filters;
pub mod intake;
pub mod parse;
pub mod quote;

#[cfg(target_arch = "wasm32")]
mod glue;
