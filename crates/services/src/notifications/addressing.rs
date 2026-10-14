//! Addresses and threading headers (support.md §6).
//!
//! | Header | Order and admin emails | Ticket emails |
//! |---|---|---|
//! | `From` | `TechDepó Support <support+<org>@<domain>>` | same |
//! | `Reply-To` | the support address (a reply opens a ticket) | `support+<org>.<ticket>@<domain>` |
//! | `Message-ID` | `order.<order id>.<template>[.<ref>]@<domain>` | `<ticket>.<n>@<domain>` |
//! | `In-Reply-To` / `References` | — | the customer's message IDs |
//! | `Auto-Submitted` | `auto-generated` | `auto-replied` |
//!
//! Message IDs are deterministic, so they double as the send's idempotency key.

use techdepo_domain::{HumanTaskId, OrderId, PublicOrgId, TicketId, UserId};
use techdepo_ports::{Header, Mailbox};

use crate::Config;

/// `support+<org>@<domain>`.
pub fn support_address(config: &Config, org: &PublicOrgId) -> String {
    config.support_mailbox().address(org)
}

/// `support+<org>.<ticket>@<domain>` (the ticket code lower-cased: mail systems may lower-case
/// it anyway, and parsing is case-insensitive).
pub fn ticket_address(config: &Config, org: &PublicOrgId, ticket: &TicketId) -> String {
    config.support_mailbox().ticket_reply(org, ticket)
}

/// The sender of every email.
pub fn from(config: &Config, org: &PublicOrgId) -> Mailbox {
    Mailbox::named(&config.from_name, support_address(config, org))
}

/// `Reply-To` of a ticket's emails.
pub fn ticket_reply_to(config: &Config, org: &PublicOrgId, ticket: &TicketId) -> Mailbox {
    Mailbox::named(&config.from_name, ticket_address(config, org, ticket))
}

/// `Message-ID` of an order email, e.g. `order.ord_1.order_confirmation@techdepo.dev`;
/// `reference` tells several emails of one template apart (a refund ID).
pub fn order_message_id(
    config: &Config,
    order: &OrderId,
    template: &str,
    reference: Option<&str>,
) -> String {
    match reference {
        Some(r) => format!("order.{order}.{template}.{r}@{}", config.support_domain),
        None => format!("order.{order}.{template}@{}", config.support_domain),
    }
}

/// `Message-ID` of the `n`-th message of a ticket, e.g. `T-7F2M9Q.3@techdepo.dev`.
pub fn ticket_message_id(config: &Config, ticket: &TicketId, n: u32) -> String {
    format!("{ticket}.{n}@{}", config.support_domain)
}

/// `Message-ID` of a task's SLA nudge to one admin.
pub fn nudge_message_id(config: &Config, task: &HumanTaskId, admin: &UserId) -> String {
    format!("task.{task}.nudge.{admin}@{}", config.support_domain)
}

/// `<id>` as a header value.
fn angle(id: &str) -> String {
    format!("<{}>", id.trim_matches(['<', '>']))
}

/// `In-Reply-To` and `References` for a reply. `references` is the thread so far (oldest first);
/// `in_reply_to` is appended to it when missing.
pub fn threading_headers(in_reply_to: Option<&str>, references: &[String]) -> Vec<Header> {
    let mut headers = Vec::new();
    let mut refs: Vec<String> = references.iter().map(|r| angle(r)).collect();
    if let Some(parent) = in_reply_to {
        let parent = angle(parent);
        if !refs.contains(&parent) {
            refs.push(parent.clone());
        }
        headers.push(Header::new("In-Reply-To", parent));
    }
    if !refs.is_empty() {
        headers.push(Header::new("References", refs.join(" ")));
    }
    headers
}

/// `Auto-Submitted: auto-generated` (notifications nobody asked for, RFC 3834).
pub fn auto_generated() -> Header {
    Header::new("Auto-Submitted", "auto-generated")
}

/// `Auto-Submitted: auto-replied` (answers to a customer's message).
pub fn auto_replied() -> Header {
    Header::new("Auto-Submitted", "auto-replied")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_ids() {
        let config = Config::default();
        let org = PublicOrgId::parse("k3x9q2ab").unwrap();
        let ticket = TicketId::parse("T-7F2M9Q").unwrap();
        assert_eq!(
            from(&config, &org).to_string(),
            "TechDepó Support <support+k3x9q2ab@techdepo.dev>"
        );
        assert_eq!(
            ticket_reply_to(&config, &org, &ticket).email,
            "support+k3x9q2ab.7f2m9q@techdepo.dev"
        );
        assert_eq!(
            ticket_message_id(&config, &ticket, 3),
            "T-7F2M9Q.3@techdepo.dev"
        );
        assert_eq!(
            order_message_id(
                &config,
                &OrderId::new("ord_1"),
                "refund_done",
                Some("ref_1")
            ),
            "order.ord_1.refund_done.ref_1@techdepo.dev"
        );
    }

    #[test]
    fn threading() {
        let h = threading_headers(Some("b@x"), &["<a@x>".to_owned()]);
        assert_eq!(h[0], Header::new("In-Reply-To", "<b@x>"));
        assert_eq!(h[1], Header::new("References", "<a@x> <b@x>"));
        assert!(threading_headers(None, &[]).is_empty());
    }
}
