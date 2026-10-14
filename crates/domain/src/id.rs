//! Identifier newtypes.
//!
//! Opaque IDs (`OrderId`, `RefundId`, …) are plain strings minted elsewhere (Clerk, Restate
//! random UUIDs, the catalog). Formatted IDs (`OrderNumber`, `TicketId`, `InvoiceNumber`,
//! `PublicOrgId`) have a human-facing shape that is validated on parse and on deserialize.

use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::hash::fnv1a64;

/// A formatted identifier could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind} `{value}`: expected {expected}")]
pub struct ParseIdError {
    /// Which identifier failed to parse.
    pub kind: &'static str,
    /// The rejected input.
    pub value: String,
    /// Description of the accepted format.
    pub expected: &'static str,
}

impl ParseIdError {
    fn new(kind: &'static str, value: &str, expected: &'static str) -> Self {
        Self {
            kind,
            value: value.to_owned(),
            expected,
        }
    }
}

macro_rules! opaque_id {
    ($( $(#[$meta:meta])* $name:ident; )+) => {$(
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wraps an existing identifier.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// The identifier as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Unwraps the identifier.
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl FromStr for $name {
            type Err = std::convert::Infallible;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(s.to_owned()))
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }
    )+};
}

opaque_id! {
    /// Clerk organization ID (`org_2Zx…`). One organization is one webshop instance.
    OrgId;
    /// Clerk user ID (`user_2Ab…`).
    UserId;
    /// Internal order ID; the key of the `Order` virtual object.
    OrderId;
    /// Internal invoice or credit note ID.
    InvoiceId;
    /// Catalog product ID: a lower-case slug such as `voltaris-v5-8600`.
    ProductId;
    /// Stock keeping unit such as `CPU-VOL-8600`.
    Sku;
    /// Refund ID; the key of `RefundWorkflow`.
    RefundId;
    /// Investigation ID; the key of the `Investigation` workflow.
    InvestigationId;
    /// Human task ID.
    HumanTaskId;
    /// Shipment ID (fake carrier).
    ShipmentId;
    /// Carrier tracking number.
    TrackingNumber;
}

impl TrackingNumber {
    /// Stable 64-bit hash, used to derive per-shipment simulation parameters.
    pub fn stable_hash(&self) -> u64 {
        fnv1a64(self.0.as_bytes())
    }
}

// ---------------------------------------------------------------------------------------------
// PublicOrgId
// ---------------------------------------------------------------------------------------------

/// Short public org ID used in email addresses: exactly 8 characters of `[a-z0-9]`.
///
/// Clerk org IDs are long and mixed-case, which mail systems mangle in the local part, so each
/// org gets one of these (`support+k3x9q2ab@techdepo.dev`).
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct PublicOrgId(String);

impl PublicOrgId {
    /// Length of a public org ID.
    pub const LEN: usize = 8;
    const ALPHABET: &'static [u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    const EXPECTED: &'static str = "8 characters of [a-z0-9]";

    /// Builds an ID from caller-supplied random bytes (the domain has no entropy source).
    pub fn from_entropy(bytes: [u8; Self::LEN]) -> Self {
        let id = bytes
            .iter()
            .map(|b| Self::ALPHABET[usize::from(*b) % Self::ALPHABET.len()] as char)
            .collect();
        Self(id)
    }

    /// Parses and validates an ID.
    pub fn parse(s: &str) -> Result<Self, ParseIdError> {
        let valid = s.len() == Self::LEN
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        if valid {
            Ok(Self(s.to_owned()))
        } else {
            Err(ParseIdError::new("public org ID", s, Self::EXPECTED))
        }
    }

    /// The ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PublicOrgId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for PublicOrgId {
    type Err = ParseIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for PublicOrgId {
    type Error = ParseIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<PublicOrgId> for String {
    fn from(value: PublicOrgId) -> Self {
        value.0
    }
}

// ---------------------------------------------------------------------------------------------
// OrderNumber
// ---------------------------------------------------------------------------------------------

/// Customer-facing order number, displayed as `TD-1042`. Sequential per org.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct OrderNumber(u32);

impl OrderNumber {
    const PREFIX: &'static str = "TD-";
    const EXPECTED: &'static str = "TD-<number>, e.g. TD-1042";
    const MAX_DIGITS: usize = 9;

    /// Wraps a sequence value.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// The numeric part.
    pub const fn value(self) -> u32 {
        self.0
    }

    /// Finds every order number mentioned in free text (case-insensitive, `TD-\d+`).
    ///
    /// Invoice numbers (`TD-2026-000042`) are not mistaken for order numbers. Duplicates are
    /// removed; the order of first mention is kept.
    pub fn find_all(text: &str) -> Vec<Self> {
        let bytes = text.as_bytes();
        let mut found = Vec::new();
        let mut i = 0;
        while i + Self::PREFIX.len() < bytes.len() {
            let starts_word = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            if starts_word && bytes[i..i + Self::PREFIX.len()].eq_ignore_ascii_case(b"TD-") {
                let digits_start = i + Self::PREFIX.len();
                let digits_len = bytes[digits_start..]
                    .iter()
                    .take_while(|b| b.is_ascii_digit())
                    .count();
                let end = digits_start + digits_len;
                let next = bytes.get(end).copied();
                let after = bytes.get(end + 1).copied();
                let is_invoice = next == Some(b'-') && after.is_some_and(|b| b.is_ascii_digit());
                let ends_word = !next.is_some_and(|b| b.is_ascii_alphanumeric());
                if digits_len > 0
                    && ends_word
                    && !is_invoice
                    && let Ok(number) = Self::parse_digits(&text[digits_start..end])
                    && !found.contains(&number)
                {
                    found.push(number);
                }
                i = end.max(i + 1);
            } else {
                i += 1;
            }
        }
        found
    }

    /// Parses `TD-1042` (case-insensitive).
    pub fn parse(s: &str) -> Result<Self, ParseIdError> {
        let err = || ParseIdError::new("order number", s, Self::EXPECTED);
        let prefix = s.get(..Self::PREFIX.len()).ok_or_else(err)?;
        if !prefix.eq_ignore_ascii_case(Self::PREFIX) {
            return Err(err());
        }
        Self::parse_digits(&s[Self::PREFIX.len()..]).map_err(|()| err())
    }

    fn parse_digits(digits: &str) -> Result<Self, ()> {
        if digits.is_empty()
            || digits.len() > Self::MAX_DIGITS
            || !digits.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(());
        }
        digits.parse().map(Self).map_err(|_| ())
    }
}

impl fmt::Display for OrderNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", Self::PREFIX, self.0)
    }
}

impl FromStr for OrderNumber {
    type Err = ParseIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for OrderNumber {
    type Error = ParseIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<OrderNumber> for String {
    fn from(value: OrderNumber) -> Self {
        value.to_string()
    }
}

// ---------------------------------------------------------------------------------------------
// TicketId
// ---------------------------------------------------------------------------------------------

/// Support ticket ID, displayed as `T-7F2M9Q`: Crockford base32, upper-case.
///
/// The ticket ID appears in reply-to addresses (`support+<org>.<ticket>@…`), where mail systems
/// may lower-case it, so parsing is case-insensitive and accepts the code with or without the
/// `T-` prefix.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct TicketId(String);

impl TicketId {
    /// Length of codes produced by [`TicketId::derive`].
    pub const DERIVED_LEN: usize = 6;
    const MIN_LEN: usize = 4;
    const MAX_LEN: usize = 12;
    const PREFIX: &'static str = "T-";
    const ALPHABET: &'static [u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    const EXPECTED: &'static str = "T-<4 to 12 Crockford base32 characters>, e.g. T-7F2M9Q";

    /// Derives the ticket ID for a new conversation from the first message's `Message-ID`.
    ///
    /// Deterministic, so retried intake cannot fork a ticket. The org is part of the input so
    /// the same email sent to two shops yields two tickets.
    pub fn derive(org: &PublicOrgId, message_id: &str) -> Self {
        let mut input = Vec::with_capacity(org.as_str().len() + 1 + message_id.len());
        input.extend_from_slice(org.as_str().as_bytes());
        input.push(b'\n');
        input.extend_from_slice(message_id.trim().as_bytes());
        let mut hash = fnv1a64(&input);
        let mut code = String::with_capacity(Self::DERIVED_LEN);
        for _ in 0..Self::DERIVED_LEN {
            code.push(Self::ALPHABET[(hash & 0x1f) as usize] as char);
            hash >>= 5;
        }
        Self(code)
    }

    /// Parses `T-7F2M9Q`, `t-7f2m9q` or `7F2M9Q`.
    ///
    /// Crockford's ambiguous letters are normalized: `I`/`L` read as `1`, `O` as `0`.
    pub fn parse(s: &str) -> Result<Self, ParseIdError> {
        let err = || ParseIdError::new("ticket ID", s, Self::EXPECTED);
        let code = match s.get(..Self::PREFIX.len()) {
            Some(prefix) if prefix.eq_ignore_ascii_case(Self::PREFIX) => &s[Self::PREFIX.len()..],
            _ => s,
        };
        if !(Self::MIN_LEN..=Self::MAX_LEN).contains(&code.len()) {
            return Err(err());
        }
        code.bytes()
            .map(|b| match b.to_ascii_uppercase() {
                b'I' | b'L' => Some('1'),
                b'O' => Some('0'),
                c if Self::ALPHABET.contains(&c) => Some(c as char),
                _ => None,
            })
            .collect::<Option<String>>()
            .map(Self)
            .ok_or_else(err)
    }

    /// The code without the `T-` prefix, e.g. `7F2M9Q`.
    pub fn code(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TicketId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", Self::PREFIX, self.0)
    }
}

impl FromStr for TicketId {
    type Err = ParseIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for TicketId {
    type Error = ParseIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<TicketId> for String {
    fn from(value: TicketId) -> Self {
        value.to_string()
    }
}

// ---------------------------------------------------------------------------------------------
// InvoiceNumber
// ---------------------------------------------------------------------------------------------

/// Invoice number, sequential per org and year, displayed as `TD-2026-000042`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct InvoiceNumber {
    year: u16,
    seq: u32,
}

impl InvoiceNumber {
    const EXPECTED: &'static str = "TD-<year>-<6+ digit sequence>, e.g. TD-2026-000042";

    /// Builds an invoice number from the year and the per-org sequence value.
    pub const fn new(year: u16, seq: u32) -> Self {
        Self { year, seq }
    }

    /// Calendar year of issue.
    pub const fn year(self) -> u16 {
        self.year
    }

    /// Sequence value within the org and year.
    pub const fn seq(self) -> u32 {
        self.seq
    }

    /// Parses `TD-2026-000042`.
    pub fn parse(s: &str) -> Result<Self, ParseIdError> {
        let err = || ParseIdError::new("invoice number", s, Self::EXPECTED);
        let mut parts = s.split('-');
        let (Some(prefix), Some(year), Some(seq), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(err());
        };
        let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
        if prefix != "TD" || year.len() != 4 || !digits(year) || seq.len() < 6 || !digits(seq) {
            return Err(err());
        }
        Ok(Self {
            year: year.parse().map_err(|_| err())?,
            seq: seq.parse().map_err(|_| err())?,
        })
    }
}

impl fmt::Display for InvoiceNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TD-{:04}-{:06}", self.year, self.seq)
    }
}

impl FromStr for InvoiceNumber {
    type Err = ParseIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for InvoiceNumber {
    type Error = ParseIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<InvoiceNumber> for String {
    fn from(value: InvoiceNumber) -> Self {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_org_id_validates() {
        assert!(PublicOrgId::parse("k3x9q2ab").is_ok());
        assert!(PublicOrgId::parse("K3X9Q2AB").is_err());
        assert!(PublicOrgId::parse("k3x9q2a").is_err());
        assert!(PublicOrgId::parse("k3x9q2ab1").is_err());
        assert!(PublicOrgId::parse("k3x9-2ab").is_err());
        let id = PublicOrgId::from_entropy([0, 1, 25, 26, 35, 36, 255, 128]);
        assert!(PublicOrgId::parse(id.as_str()).is_ok());
    }

    #[test]
    fn order_number_round_trips() {
        let n: OrderNumber = "TD-1042".parse().unwrap();
        assert_eq!(n.value(), 1042);
        assert_eq!(n.to_string(), "TD-1042");
        assert_eq!("td-1042".parse::<OrderNumber>().unwrap(), n);
        assert!("TD-".parse::<OrderNumber>().is_err());
        assert!("TD-12a".parse::<OrderNumber>().is_err());
        assert!("XX-12".parse::<OrderNumber>().is_err());
        assert_eq!(serde_json::to_string(&n).unwrap(), "\"TD-1042\"");
    }

    #[test]
    fn order_numbers_are_found_in_text() {
        let text = "Hi, about TD-1042 and td-7. Invoice TD-2026-000042, not XTD-5, also TD-1042.";
        assert_eq!(
            OrderNumber::find_all(text),
            vec![OrderNumber::new(1042), OrderNumber::new(7)]
        );
        assert_eq!(OrderNumber::find_all("(TD-9)"), vec![OrderNumber::new(9)]);
        assert!(OrderNumber::find_all("TD-12abc TD-").is_empty());
    }

    #[test]
    fn ticket_id_is_deterministic_and_parses_back() {
        let org = PublicOrgId::parse("k3x9q2ab").unwrap();
        let a = TicketId::derive(&org, "<abc@example.com>");
        let b = TicketId::derive(&org, "<abc@example.com>");
        assert_eq!(a, b);
        assert_eq!(a.code().len(), TicketId::DERIVED_LEN);
        let other_org = PublicOrgId::parse("zzzzzzzz").unwrap();
        assert_ne!(a, TicketId::derive(&other_org, "<abc@example.com>"));

        let shown = a.to_string();
        assert!(shown.starts_with("T-"));
        assert_eq!(shown.parse::<TicketId>().unwrap(), a);
        assert_eq!(shown.to_lowercase().parse::<TicketId>().unwrap(), a);
        assert_eq!(a.code().parse::<TicketId>().unwrap(), a);
    }

    #[test]
    fn ticket_id_parsing_normalizes_and_rejects() {
        assert_eq!(TicketId::parse("t-7f2m").unwrap().to_string(), "T-7F2M");
        assert_eq!(TicketId::parse("T-ol1i").unwrap().code(), "0111");
        assert!(TicketId::parse("T-7F2").is_err());
        assert!(TicketId::parse("T-7F2U").is_err());
        assert!(TicketId::parse("T-7F2M!").is_err());
    }

    #[test]
    fn invoice_number_round_trips() {
        let n = InvoiceNumber::new(2026, 42);
        assert_eq!(n.to_string(), "TD-2026-000042");
        assert_eq!("TD-2026-000042".parse::<InvoiceNumber>().unwrap(), n);
        assert_eq!(
            "TD-2026-1234567".parse::<InvoiceNumber>().unwrap().seq(),
            1_234_567
        );
        assert!("TD-2026-42".parse::<InvoiceNumber>().is_err());
        assert!("TD-26-000042".parse::<InvoiceNumber>().is_err());
        assert!("XX-2026-000042".parse::<InvoiceNumber>().is_err());
    }
}
