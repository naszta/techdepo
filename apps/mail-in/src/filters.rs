//! The cheap checks that drop mail before anything else happens (support.md §1 step 2, §9 loop
//! protection). Dropped mail gets no reply (no backscatter); it is logged with its reason.

use techdepo_api_types::vocab::RejectReason;

use crate::parse::ParsedEmail;

/// Whether the message is automatic: `Auto-Submitted` other than `no` (RFC 3834),
/// `Precedence: bulk/list/junk/auto_reply`, mailing-list headers, or `X-Autoreply`.
pub fn is_automatic(p: &ParsedEmail) -> bool {
    let auto_submitted = p
        .auto_submitted
        .as_deref()
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .is_some_and(|v| !v.is_empty() && v != "no");
    let precedence = p.precedence.as_deref().is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "bulk" | "list" | "junk" | "auto_reply"
        )
    });
    auto_submitted || precedence || p.list || p.autoreply_header
}

/// How the sender's identity is established before the `From` address is trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SenderAuth {
    /// Production: the topmost `Authentication-Results` must come from our receiving MTA
    /// (`authserv_id`, Cloudflare Email Routing's `mx.cloudflare.net`) and say `dmarc=pass` for
    /// the `From` domain. Anything else fails closed: a missing header, another authserv-id (a
    /// header the sender wrote), `dmarc=none` / `temperror` / `permerror`, or a domain without
    /// a DMARC policy.
    Dmarc {
        /// The receiving MTA's authserv-id.
        authserv_id: String,
    },
    /// Local development only (`AUTH_MODE=dev`): wrangler's email trigger adds no
    /// `Authentication-Results`, so the `From` address is taken as is.
    TrustFrom,
}

impl Default for SenderAuth {
    fn default() -> Self {
        Self::Dmarc {
            authserv_id: "mx.cloudflare.net".to_owned(),
        }
    }
}

/// Whether our receiving MTA's `Authentication-Results` (the topmost) says DMARC passed for
/// `from_domain` (RFC 8601: `authserv-id [version]; method=result ptype.property=value ...`).
pub fn dmarc_passed(p: &ParsedEmail, from_domain: Option<&str>, authserv_id: &str) -> bool {
    let Some(header) = p.authentication_results.as_deref() else {
        return false;
    };
    let header = header.to_ascii_lowercase();
    let mut parts = header.split(';');
    let id = parts
        .next()
        .and_then(|first| first.split_whitespace().next())
        .unwrap_or("");
    if id != authserv_id.to_ascii_lowercase() {
        return false;
    }
    let Some(dmarc) = parts.map(str::trim).find(|r| r.starts_with("dmarc=")) else {
        return false;
    };
    let mut tokens = dmarc.split_whitespace();
    if tokens.next() != Some("dmarc=pass") {
        return false;
    }
    // When the result names the domain it evaluated, it must be the `From` domain.
    match tokens.find_map(|t| t.strip_prefix("header.from=")) {
        Some(domain) => from_domain.is_some_and(|d| d.eq_ignore_ascii_case(domain)),
        None => true,
    }
}

/// Whether `from_domain` is our own domain or one of its subdomains (mail loops).
pub fn is_own_domain(from_domain: &str, own: &str) -> bool {
    let (from, own) = (from_domain.to_ascii_lowercase(), own.to_ascii_lowercase());
    from == own || from.ends_with(&format!(".{own}"))
}

/// The first reason to drop the message, if any.
pub fn screen(
    p: &ParsedEmail,
    from_domain: Option<&str>,
    own_domain: &str,
    auth: &SenderAuth,
) -> Option<RejectReason> {
    if is_automatic(p) {
        return Some(RejectReason::AutoReply);
    }
    if from_domain.is_some_and(|d| is_own_domain(d, own_domain)) {
        return Some(RejectReason::OwnDomain);
    }
    if let SenderAuth::Dmarc { authserv_id } = auth
        && !dmarc_passed(p, from_domain, authserv_id)
    {
        return Some(RejectReason::DmarcFail);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASS: &str = "mx.cloudflare.net; spf=pass smtp.mailfrom=example.com; \
        dkim=pass header.d=example.com; dmarc=pass header.from=example.com";

    fn email() -> ParsedEmail {
        ParsedEmail {
            from: Some("ada@example.com".into()),
            subject: "Hi".into(),
            authentication_results: Some(PASS.into()),
            ..ParsedEmail::default()
        }
    }

    fn screen(p: &ParsedEmail, from_domain: Option<&str>, own: &str) -> Option<RejectReason> {
        super::screen(p, from_domain, own, &SenderAuth::default())
    }

    #[test]
    fn automatic_mail() {
        assert_eq!(screen(&email(), Some("example.com"), "techdepo.dev"), None);
        for (auto_submitted, precedence) in [
            (Some("auto-replied"), None),
            (Some("auto-generated; owner-email=x"), None),
            (None, Some("bulk")),
            (None, Some(" List ")),
        ] {
            let p = ParsedEmail {
                auto_submitted: auto_submitted.map(str::to_owned),
                precedence: precedence.map(str::to_owned),
                ..email()
            };
            assert_eq!(
                screen(&p, Some("example.com"), "techdepo.dev"),
                Some(RejectReason::AutoReply)
            );
        }
        let human = ParsedEmail {
            auto_submitted: Some("no".into()),
            ..email()
        };
        assert!(!is_automatic(&human));
        let list = ParsedEmail {
            list: true,
            ..email()
        };
        assert!(is_automatic(&list));
    }

    #[test]
    fn own_domain_and_dmarc() {
        assert_eq!(
            screen(&email(), Some("techdepo.dev"), "techdepo.dev"),
            Some(RejectReason::OwnDomain)
        );
        assert_eq!(
            screen(&email(), Some("send.TechDepo.dev"), "techdepo.dev"),
            Some(RejectReason::OwnDomain)
        );
        assert!(!is_own_domain("nottechdepo.dev", "techdepo.dev"));
        let with = |results: Option<&str>| ParsedEmail {
            authentication_results: results.map(str::to_owned),
            ..email()
        };
        let verdict =
            |results: Option<&str>| screen(&with(results), Some("example.com"), "techdepo.dev");
        assert_eq!(verdict(Some(PASS)), None);
        assert_eq!(verdict(Some("MX.Cloudflare.net 1; dmarc=pass")), None);
        // Fail closed: anything but our MTA's pass for the From domain.
        for results in [
            None,
            Some("mx.cloudflare.net; spf=pass; dmarc=fail (p=none)"),
            Some("mx.cloudflare.net; dmarc=none header.from=example.com"),
            Some("mx.cloudflare.net; dmarc=temperror"),
            Some("mx.cloudflare.net; dmarc=permerror"),
            Some("mx.cloudflare.net; spf=pass; dkim=pass"),
            Some("attacker.example; dmarc=pass header.from=example.com"),
            Some("mx.cloudflare.net; dmarc=pass header.from=evil.example"),
        ] {
            assert_eq!(
                verdict(results),
                Some(RejectReason::DmarcFail),
                "{results:?}"
            );
        }
        // Local development trusts the From address.
        assert_eq!(
            super::screen(
                &with(None),
                Some("example.com"),
                "techdepo.dev",
                &SenderAuth::TrustFrom
            ),
            None
        );
    }
}
