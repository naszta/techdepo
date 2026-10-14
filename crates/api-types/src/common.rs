//! Shared DTO building blocks: pagination, references, addresses and evidence.

use serde::{Deserialize, Serialize};
use techdepo_domain::{
    InvoiceNumber, LostOutcome, Money, OrderId, OrderNumber, OrderStatus, ShipmentId, TicketId,
    TicketStatus, TrackingNumber, UserId,
};
use time::OffsetDateTime;

use crate::error::{FieldError, check_text};
use crate::ids::PaymentId;
use crate::vocab::{OrderEventKind, RefKind, TraceStatus};
use techdepo_domain::InvoiceId;

/// Default page size of list endpoints.
pub const DEFAULT_PAGE_SIZE: u32 = 25;
/// Largest page size a list endpoint accepts.
pub const MAX_PAGE_SIZE: u32 = 100;

/// The effective page size for a `limit` query parameter.
pub fn page_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE)
}

/// One page of a list, newest first unless the endpoint says otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    /// The items on this page.
    pub items: Vec<T>,
    /// Pass as `cursor` to get the next page; absent on the last page. Opaque to clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    /// A page with no next page.
    pub fn last(items: Vec<T>) -> Self {
        Self {
            items,
            next_cursor: None,
        }
    }
}

impl<T> Default for Page<T> {
    fn default() -> Self {
        Self::last(Vec::new())
    }
}

/// Response of a `DELETE`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deleted {
    /// The deleted record's ID.
    pub id: String,
}

/// A link to any record, for timelines, evidence and activity feeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityRef {
    /// What kind of record.
    pub kind: RefKind,
    /// Its ID (orders: the order ID, not the number).
    pub id: String,
    /// Display text, e.g. `TD-1042`, `T-7F2M9Q`, `TD-2026-000042`.
    pub label: String,
}

/// A link to an order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderRef {
    /// Order ID.
    pub id: OrderId,
    /// Order number (what URLs and people use).
    pub number: OrderNumber,
    /// Current status.
    pub status: OrderStatus,
}

/// A link to a ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketRef {
    /// Ticket ID (what URLs and people use).
    pub id: TicketId,
    /// Subject of the first message.
    pub subject: String,
    /// Current status.
    pub status: TicketStatus,
}

/// A link to a customer (an org member).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerRef {
    /// Clerk user ID.
    pub user_id: UserId,
    /// Email address.
    pub email: String,
    /// Display name, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A postal address (shipping or billing). Stored as JSON in D1 (`*_address_json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    /// Recipient's full name.
    pub name: String,
    /// Company, for business invoices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
    /// Street and number.
    pub line1: String,
    /// Apartment, floor, etc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line2: Option<String>,
    /// Postal code.
    pub postal_code: String,
    /// City.
    pub city: String,
    /// ISO 3166-1 alpha-2 country code, upper case (`HU`).
    pub country: String,
    /// Phone number for the carrier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    /// VAT number, for business invoices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vat_id: Option<String>,
}

impl Address {
    /// Pushes field errors for this address, with field paths under `prefix` (e.g.
    /// `shipping_address.city`).
    pub fn check(&self, prefix: &str, errors: &mut Vec<FieldError>) {
        let field = |name: &str| format!("{prefix}.{name}");
        check_text(errors, &field("name"), &self.name, 1, 100);
        check_text(errors, &field("line1"), &self.line1, 1, 200);
        check_text(errors, &field("postal_code"), &self.postal_code, 2, 16);
        check_text(errors, &field("city"), &self.city, 1, 100);
        let country_ok =
            self.country.len() == 2 && self.country.bytes().all(|b| b.is_ascii_uppercase());
        if !country_ok {
            errors.push(FieldError::new(
                field("country"),
                "Two-letter country code, e.g. HU",
            ));
        }
        for (name, value, max) in [
            ("company", &self.company, 100),
            ("line2", &self.line2, 200),
            ("phone", &self.phone, 32),
            ("vat_id", &self.vat_id, 32),
        ] {
            if let Some(value) = value {
                check_text(errors, &field(name), value, 0, max);
            }
        }
    }

    /// The address as display lines (name, company, street, city line, country).
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![self.name.clone()];
        lines.extend(self.company.clone());
        lines.push(self.line1.clone());
        lines.extend(self.line2.clone());
        lines.push(format!("{} {}", self.postal_code, self.city));
        lines.push(self.country.clone());
        lines
    }
}

/// A piece of evidence attached to a refund or a human task, so a person can decide without
/// digging. Built by deterministic code (detectors, workflows), never by an LLM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    /// Two captured charges with the same amount within the detector's window.
    DuplicateCharge {
        /// The order.
        order_number: OrderNumber,
        /// The first (legitimate) charge.
        first: PaymentId,
        /// The duplicate, which the refund reverses.
        duplicate: PaymentId,
        /// Amount of each charge.
        amount: Money,
        /// Seconds between the two charges.
        seconds_apart: i64,
    },
    /// An invoice was issued without a billing address.
    InvoiceMissingAddress {
        /// The invoice.
        invoice_id: InvoiceId,
        /// Its number.
        number: InvoiceNumber,
    },
    /// The order's state when the evidence was gathered.
    OrderState {
        /// The order.
        order_number: OrderNumber,
        /// Its status.
        status: OrderStatus,
        /// Workdays since shipment, for shipped orders.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workdays_since_ship: Option<u32>,
    },
    /// The carrier's answer to a trace.
    CarrierTrace {
        /// The shipment.
        shipment_id: ShipmentId,
        /// Tracking number.
        tracking_number: TrackingNumber,
        /// Trace state.
        status: TraceStatus,
        /// What the trace revealed, once concluded.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<LostOutcome>,
    },
    /// A customer message the decision is about.
    CustomerMessage {
        /// The ticket.
        ticket_id: TicketId,
        /// Message number within the ticket.
        seq: u32,
        /// Short excerpt.
        excerpt: String,
    },
    /// An order timeline event.
    OrderEvent {
        /// The order.
        order_number: OrderNumber,
        /// Event number within the order.
        seq: u32,
        /// Event kind.
        event: OrderEventKind,
        /// When it happened.
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
    },
    /// Free text from the workflow or a person.
    Note {
        /// The note.
        text: String,
    },
}
