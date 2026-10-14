//! MIME parsing (`mail-parser`, lenient with real-world mail) into the fields intake needs.

use mail_parser::{HeaderValue, MessageParser};

use crate::address::bare;

/// The parts of an inbound email that intake looks at.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedEmail {
    /// `Message-ID` without angle brackets.
    pub message_id: Option<String>,
    /// `In-Reply-To` IDs.
    pub in_reply_to: Vec<String>,
    /// `References` IDs, oldest first.
    pub references: Vec<String>,
    /// `From` address, lower-cased.
    pub from: Option<String>,
    /// `From` display name.
    pub from_name: Option<String>,
    /// `Reply-To` address.
    pub reply_to: Option<String>,
    /// Subject (decoded).
    pub subject: String,
    /// The text body (HTML-only mail is converted to text).
    pub text: String,
    /// The HTML body, if any.
    pub html: Option<String>,
    /// `Auto-Submitted`.
    pub auto_submitted: Option<String>,
    /// `Precedence`.
    pub precedence: Option<String>,
    /// A `List-Id` / `List-Unsubscribe` header is present (mailing list traffic).
    pub list: bool,
    /// `X-Autoreply` / `X-Autorespond` is present.
    pub autoreply_header: bool,
    /// The topmost `Authentication-Results` (added by our receiving MTA).
    pub authentication_results: Option<String>,
}

fn ids(value: &HeaderValue<'_>) -> Vec<String> {
    let raw: Vec<String> = match value {
        HeaderValue::Text(t) => vec![t.to_string()],
        HeaderValue::TextList(list) => list.iter().map(ToString::to_string).collect(),
        _ => Vec::new(),
    };
    raw.iter()
        .flat_map(|v| v.split_whitespace())
        .map(|id| id.trim_matches(['<', '>']).trim().to_owned())
        .filter(|id| !id.is_empty())
        .collect()
}

/// The first (topmost) occurrence of a header, raw, unfolded and trimmed. The topmost
/// `Authentication-Results` is the one our receiving MTA added; lower ones came with the mail.
fn raw_header(message: &mail_parser::Message<'_>, name: &str) -> Option<String> {
    let header = message
        .root_part()
        .headers
        .iter()
        .find(|h| h.name.as_str().eq_ignore_ascii_case(name))?;
    let raw = message
        .raw_message
        .get(header.offset_start as usize..header.offset_end as usize)?;
    let value = String::from_utf8_lossy(raw)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!value.is_empty()).then_some(value)
}

/// Parses raw RFC 822 bytes. `None` if they are not a message at all.
pub fn parse(raw: &[u8]) -> Option<ParsedEmail> {
    let message = MessageParser::default().parse(raw)?;
    // Something that parses but has no headers at all is not an email.
    if message.root_part().headers.is_empty() {
        return None;
    }
    let from = message.from().and_then(|a| a.first());
    let text = message
        .body_text(0)
        .map(|t| t.replace("\r\n", "\n"))
        .unwrap_or_default();
    Some(ParsedEmail {
        message_id: message
            .message_id()
            .map(|id| id.trim_matches(['<', '>']).trim().to_owned())
            .filter(|id| !id.is_empty()),
        in_reply_to: ids(message.in_reply_to()),
        references: ids(message.references()),
        from: from.and_then(|a| a.address()).map(bare),
        from_name: from
            .and_then(|a| a.name())
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty()),
        reply_to: message
            .reply_to()
            .and_then(|a| a.first())
            .and_then(|a| a.address())
            .map(bare),
        subject: message.subject().unwrap_or_default().trim().to_owned(),
        text,
        html: message.body_html(0).map(|h| h.into_owned()),
        auto_submitted: raw_header(&message, "Auto-Submitted"),
        precedence: raw_header(&message, "Precedence"),
        list: raw_header(&message, "List-Id").is_some()
            || raw_header(&message, "List-Unsubscribe").is_some(),
        autoreply_header: raw_header(&message, "X-Autoreply").is_some()
            || raw_header(&message, "X-Autorespond").is_some(),
        authentication_results: raw_header(&message, "Authentication-Results"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMAIL: &str = "Return-Path: <ada@example.com>\r\n\
Authentication-Results: mx.cloudflare.net; dkim=pass header.d=example.com; dmarc=pass\r\n\
Authentication-Results: spoofed; dmarc=fail\r\n\
From: =?utf-8?q?Ada_L=C3=B3velace?= <Ada@Example.com>\r\n\
To: support+k3x9q2ab@techdepo.dev\r\n\
Subject: Re: Cancel my order [#T-7F2M9Q]\r\n\
Message-ID: <CAF1234@mail.example.com>\r\n\
In-Reply-To: <T-7F2M9Q.2@techdepo.dev>\r\n\
References: <first@mail.example.com>\r\n <T-7F2M9Q.2@techdepo.dev>\r\n\
Content-Type: multipart/alternative; boundary=\"b1\"\r\n\
\r\n\
--b1\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
It is TD-1042.\r\n\
\r\n\
On Mon, 12 Oct 2026, TechDepó Support wrote:\r\n\
> Which order?\r\n\
--b1\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<p>It is TD-1042.</p>\r\n\
--b1--\r\n";

    #[test]
    fn parses_the_fields_intake_needs() {
        let p = parse(EMAIL.as_bytes()).unwrap();
        assert_eq!(p.message_id.as_deref(), Some("CAF1234@mail.example.com"));
        assert_eq!(p.in_reply_to, vec!["T-7F2M9Q.2@techdepo.dev"]);
        assert_eq!(
            p.references,
            vec!["first@mail.example.com", "T-7F2M9Q.2@techdepo.dev"]
        );
        assert_eq!(p.from.as_deref(), Some("ada@example.com"));
        assert_eq!(p.from_name.as_deref(), Some("Ada Lóvelace"));
        assert_eq!(p.subject, "Re: Cancel my order [#T-7F2M9Q]");
        assert!(p.text.starts_with("It is TD-1042."));
        assert!(p.html.unwrap().contains("<p>"));
        assert!(
            p.authentication_results
                .unwrap()
                .contains("mx.cloudflare.net"),
            "the topmost header wins"
        );
        assert!(!p.list && !p.autoreply_header && p.auto_submitted.is_none());
    }

    #[test]
    fn html_only_mail_gets_a_text_body() {
        let raw = "From: a@example.com\r\nSubject: Hi\r\nContent-Type: text/html\r\n\r\n<p>Where is <b>TD-7</b>?</p>\r\n";
        let p = parse(raw.as_bytes()).unwrap();
        assert!(p.text.contains("Where is"), "{}", p.text);
        assert!(p.message_id.is_none());
    }

    #[test]
    fn garbage_is_not_an_email() {
        assert!(parse(b"").is_none());
    }
}
