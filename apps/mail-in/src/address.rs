//! The support address: `support+<public org>[.<ticket>]@<EMAIL_DOMAIN>` (support.md §1).

use techdepo_domain::{PublicOrgId, TicketId};

/// What the recipient address says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportAddress {
    /// The org's public ID (the `+tag`).
    pub public_org_id: PublicOrgId,
    /// The ticket, when the address is a ticket's `Reply-To`. An unparseable ticket part is
    /// ignored (the threading headers or a new ticket take over).
    pub ticket_id: Option<TicketId>,
}

/// The bare, lower-cased address of `raw` (`"Name" <a@b>`, `<a@b>` or `a@b`).
pub fn bare(raw: &str) -> String {
    let raw = raw.trim();
    let inner = match (raw.rfind('<'), raw.rfind('>')) {
        (Some(start), Some(end)) if start < end => &raw[start + 1..end],
        _ => raw,
    };
    inner.trim().to_ascii_lowercase()
}

/// The domain of an address (after the last `@`), lower-cased.
pub fn domain_of(address: &str) -> Option<String> {
    bare(address)
        .rsplit_once('@')
        .map(|(_, d)| d.to_owned())
        .filter(|d| !d.is_empty())
}

/// Parses a recipient as a support address of `domain` with local part `local_part`. `None` if
/// it is not one, or its org tag is malformed.
pub fn parse(recipient: &str, local_part: &str, domain: &str) -> Option<SupportAddress> {
    let address = bare(recipient);
    let (local, host) = address.rsplit_once('@')?;
    if !host.eq_ignore_ascii_case(domain) {
        return None;
    }
    let tag = local
        .strip_prefix(&local_part.to_ascii_lowercase())?
        .strip_prefix('+')?;
    let (org, ticket) = match tag.split_once('.') {
        Some((org, ticket)) => (org, Some(ticket)),
        None => (tag, None),
    };
    Some(SupportAddress {
        public_org_id: PublicOrgId::parse(org).ok()?,
        ticket_id: ticket.and_then(|t| TicketId::parse(t).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_addresses() {
        let a = parse("support+k3x9q2ab@techdepo.dev", "support", "techdepo.dev").unwrap();
        assert_eq!(a.public_org_id.as_str(), "k3x9q2ab");
        assert_eq!(a.ticket_id, None);

        let t = parse(
            "TechDepó Support <Support+K3X9Q2AB.7f2m9q@TechDepo.dev>",
            "support",
            "techdepo.dev",
        )
        .unwrap();
        assert_eq!(t.ticket_id, Some(TicketId::parse("T-7F2M9Q").unwrap()));

        let junk_ticket = parse(
            "support+k3x9q2ab.!!@techdepo.dev",
            "support",
            "techdepo.dev",
        );
        assert_eq!(junk_ticket.unwrap().ticket_id, None);

        for wrong in [
            "support@techdepo.dev",
            "support+short@techdepo.dev",
            "support+k3x9q2ab@example.com",
            "sales+k3x9q2ab@techdepo.dev",
            "supportx+k3x9q2ab@techdepo.dev",
            "nonsense",
        ] {
            assert_eq!(parse(wrong, "support", "techdepo.dev"), None, "{wrong}");
        }
    }

    #[test]
    fn bare_addresses_and_domains() {
        assert_eq!(bare("\"Ada L\" <Ada@Example.COM>"), "ada@example.com");
        assert_eq!(bare(" ada@example.com "), "ada@example.com");
        assert_eq!(
            domain_of("Ada <ada@mail.Example.com>").as_deref(),
            Some("mail.example.com")
        );
        assert_eq!(domain_of("nobody"), None);
    }
}
