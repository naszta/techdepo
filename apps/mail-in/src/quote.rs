//! Stripping quoted history and signatures from a reply, so the ticket (and the classifier)
//! sees only what the customer wrote this time. The full text stays in `email_log`.

/// Lines that start the quoted part of a reply.
fn is_separator(line: &str) -> bool {
    let l = line.trim();
    let lower = l.to_lowercase();
    l == "--"
        || line.trim_end() == "-- "
        || line.trim_end() == "--"
        || lower.starts_with("-----original message-----")
        || lower.starts_with("----- original message -----")
        || lower.starts_with("-------- original message --------")
        || lower.starts_with("______________________________")
        || lower.starts_with("sent from my ")
        || lower.starts_with("get outlook for ")
        || lower.ends_with(" wrote:")
        || lower.ends_with(" írta:")
        || lower.ends_with(" schrieb:")
        || lower.ends_with(" a écrit :")
        || lower.ends_with(" escribió:")
}

/// `From: …` followed by `Sent:` / `Date:`: Outlook's quoted header block.
fn is_outlook_header(line: &str, next: Option<&str>) -> bool {
    let lower = line.trim().to_lowercase();
    (lower.starts_with("from:") || lower.starts_with("feladó:") || lower.starts_with("von:"))
        && next.is_some_and(|n| {
            let n = n.trim().to_lowercase();
            n.starts_with("sent:")
                || n.starts_with("date:")
                || n.starts_with("elküldve:")
                || n.starts_with("gesendet:")
        })
}

/// A Gmail attribution broken over two lines: `On Mon, … Ada <` / `ada@x> wrote:`.
fn is_split_attribution(line: &str, next: Option<&str>) -> bool {
    let lower = line.trim().to_lowercase();
    (lower.starts_with("on ") || lower.starts_with("am "))
        && next.is_some_and(|n| {
            let n = n.trim().to_lowercase();
            n.ends_with("wrote:") || n.ends_with("schrieb:")
        })
}

/// The new part of a reply: everything before the quoted history or signature, with `>`
/// quotes removed. Falls back to the whole text when nothing would be left.
pub fn strip_quoted(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.lines().collect();
    let mut kept = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let next = lines.get(i + 1).copied();
        if is_separator(line) || is_outlook_header(line, next) || is_split_attribution(line, next) {
            break;
        }
        if line.trim_start().starts_with('>') {
            continue;
        }
        kept.push(line.trim_end());
    }
    let stripped = kept.join("\n").trim().to_owned();
    if stripped.is_empty() {
        text.trim().to_owned()
    } else {
        stripped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gmail_reply() {
        let body = "It is TD-1042, thanks!\n\nOn Mon, 12 Oct 2026 at 10:00, TechDepó Support <support+k3x9q2ab.7f2m9q@techdepo.dev> wrote:\n> Which order is this about?\n> Just reply with the number.";
        assert_eq!(strip_quoted(body), "It is TD-1042, thanks!");
        let split = "Yes please.\n\nOn Mon, 12 Oct 2026 at 10:00, TechDepó Support <\nsupport@techdepo.dev> wrote:\n> Hi";
        assert_eq!(strip_quoted(split), "Yes please.");
    }

    #[test]
    fn outlook_reply_and_signatures() {
        let outlook = "Refund please.\r\n\r\nFrom: TechDepó Support <support@techdepo.dev>\r\nSent: Monday\r\nSubject: Re: …\r\n\r\nOld text";
        assert_eq!(strip_quoted(outlook), "Refund please.");
        let sig = "Where is my parcel?\n\n-- \nAda Lovelace\nAnalytical Engines Ltd";
        assert_eq!(strip_quoted(sig), "Where is my parcel?");
        let phone = "ok, cancel it\n\nSent from my iPhone";
        assert_eq!(strip_quoted(phone), "ok, cancel it");
        let hungarian = "Köszönöm!\n\n2026. okt. 12., h 10:00 TechDepó Support írta:\n> Szia";
        assert_eq!(strip_quoted(hungarian), "Köszönöm!");
    }

    #[test]
    fn inline_quotes_are_dropped_and_nothing_left_keeps_the_text() {
        let inline = "> Which order?\nTD-1042\n> And the reason?\nChanged my mind";
        assert_eq!(strip_quoted(inline), "TD-1042\nChanged my mind");
        assert_eq!(strip_quoted("> only a quote"), "> only a quote");
        assert_eq!(strip_quoted("  plain  "), "plain");
    }
}
