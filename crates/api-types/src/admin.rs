//! The admin dashboard (`/admin`, `org:admin` only): overview, orders, tickets, invoices,
//! payments ledger, inventory, customers, email log, settings and demo reset.
//!
//! Human tasks are in [`crate::tasks`]; investigations and refunds in [`crate::cases`].

use serde::{Deserialize, Serialize};
use techdepo_catalog::{Category, Tier};
use techdepo_domain::{
    Assignee, Channel, FaultToggles, Intent, InvoiceKind, InvoiceStatus, LostOutcome, Money,
    OrderNumber, OrderStatus, OrgSettings, ProductId, Sku, TicketId, TicketStatus, UserId,
};
use time::OffsetDateTime;

use crate::cases::{InvestigationSummary, RefundSummary};
use crate::catalog::Stock;
use crate::common::{Address, CustomerRef, EntityRef, OrderRef, TicketRef};
use crate::demo::DemoPanel;
use crate::error::{ApiError, Validate, check_text};
use crate::ids::{EmailId, PaymentId};
use crate::orders::{InvoiceRef, OrderDetail, OrderSummary, PaymentEntry};
use crate::session::OrgInfo;
use crate::support::{
    AgentRunDetail, AgentRunSummary, Classification, MAX_MESSAGE_CHARS, TicketMessage,
    TicketSummary,
};
use crate::tasks::{HumanTaskSummary, MAX_NOTE_CHARS, check_hand_back};
use crate::vocab::{
    Awaiting, Direction, EmailStatus, OrgRole, PaymentKind, PaymentStatus, RejectReason,
    TicketRoute, TraceStatus,
};

// ---------------------------------------------------------------------------------------------
// Overview
// ---------------------------------------------------------------------------------------------

/// Orders in one status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusCount {
    /// Status.
    pub status: OrderStatus,
    /// Number of orders.
    pub count: u32,
}

/// One entry of the recent activity feed (the latest order events, ticket messages and task
/// decisions across the org).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityItem {
    /// What it is about.
    pub entity: EntityRef,
    /// One line.
    pub summary: String,
    /// Who did it.
    pub actor: techdepo_domain::Actor,
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// `GET /api/admin/overview`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminOverview {
    /// Orders by status (every status, zeros included).
    pub orders_by_status: Vec<StatusCount>,
    /// Tickets not resolved.
    pub open_tickets: u32,
    /// Pending human tasks.
    pub pending_tasks: u32,
    /// Pending tasks past their SLA.
    pub overdue_tasks: u32,
    /// Simulated revenue: succeeded charges minus succeeded refunds.
    pub revenue: Money,
    /// Sum of completed refunds.
    pub refunded: Money,
    /// Refunds awaiting approval.
    pub refunds_pending: u32,
    /// Open investigations.
    pub open_investigations: u32,
    /// The oldest pending tasks, for the inbox preview.
    pub tasks: Vec<HumanTaskSummary>,
    /// Recent activity, newest first.
    pub activity: Vec<ActivityItem>,
}

// ---------------------------------------------------------------------------------------------
// Orders
// ---------------------------------------------------------------------------------------------

/// `GET /api/admin/orders` query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminOrderListQuery {
    /// Status filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<OrderStatus>,
    /// Customer filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<UserId>,
    /// Only orders with `refund_pending` or `investigation_open`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flagged: Option<bool>,
    /// Include cancelled orders (hidden by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_cancelled: Option<bool>,
    /// Search: order number (`TD-1042` or `1042`) or customer email.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Page cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// An order in the admin list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminOrderSummary {
    /// The order.
    pub order: OrderSummary,
    /// Its customer.
    pub customer: CustomerRef,
    /// Open tickets about it.
    pub open_tickets: u32,
}

/// The shipment's hidden truth (admin and demo controls only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipmentTruth {
    /// Lost in transit, with the outcome a trace will reveal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost_outcome: Option<LostOutcome>,
    /// Carrier trace state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_status: Option<TraceStatus>,
}

/// `GET /api/admin/orders/{number}`: the customer's order page plus everything internal.
/// `order.timeline` and `order.shipment.events` include internal entries here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminOrderDetail {
    /// The order page data, unfiltered.
    pub order: OrderDetail,
    /// Customer.
    pub customer: CustomerRef,
    /// Hidden shipment truth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shipment_truth: Option<ShipmentTruth>,
    /// Armed faults (`demo_faults`).
    pub faults: FaultToggles,
    /// Investigations.
    pub investigations: Vec<InvestigationSummary>,
    /// Refunds.
    pub refunds: Vec<RefundSummary>,
    /// Human tasks about this order.
    pub tasks: Vec<HumanTaskSummary>,
    /// The demo panel (admins may drive any order).
    pub demo: DemoPanel,
    /// Admin actions that apply now.
    pub actions: AdminOrderActions,
}

/// Which admin actions the order page offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminOrderActions {
    /// `awaiting_payment`: cancel directly.
    pub can_cancel: bool,
    /// Paid and something left to refund, with no refund pending.
    pub can_refund: bool,
    /// Shipped, not delivered, no open investigation.
    pub can_investigate: bool,
    /// Amount not yet refunded.
    pub refundable: Money,
}

/// `POST /api/admin/orders/{number}/cancel`: cancel an unpaid order. Paid orders are refunded
/// instead (`409 invalid_transition`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminCancelOrderRequest {
    /// Why, for the timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Validate for AdminCancelOrderRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        if let Some(reason) = &self.reason {
            check_text(&mut errors, "reason", reason, 0, MAX_NOTE_CHARS);
        }
        ApiError::check(errors)
    }
}

/// Response of admin order mutations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderUpdated {
    /// The order with its new status.
    pub order: OrderRef,
}

// ---------------------------------------------------------------------------------------------
// Invoices
// ---------------------------------------------------------------------------------------------

/// `GET /api/admin/invoices` query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceListQuery {
    /// Issued or voided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<InvoiceStatus>,
    /// Invoice or credit note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<InvoiceKind>,
    /// Order filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<OrderNumber>,
    /// Page cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// `POST /api/admin/invoices/{id}/void-reissue`: void an issued invoice with a credit note and
/// issue a corrected one. No money moves, so no approval is needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoidReissueRequest {
    /// Why (printed on the credit note).
    pub reason: String,
    /// Corrected billing address; absent uses the order's billing (or shipping) address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing_address: Option<Address>,
}

impl Validate for VoidReissueRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        check_text(&mut errors, "reason", &self.reason, 1, 500);
        if let Some(address) = &self.billing_address {
            address.check("billing_address", &mut errors);
        }
        ApiError::check(errors)
    }
}

/// Response of `POST /api/admin/invoices/{id}/void-reissue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceReissued {
    /// The voided invoice.
    pub voided: InvoiceRef,
    /// The credit note cancelling it.
    pub credit_note: InvoiceRef,
    /// The corrected invoice.
    pub reissued: InvoiceRef,
}

// ---------------------------------------------------------------------------------------------
// Tickets
// ---------------------------------------------------------------------------------------------

/// `GET /api/admin/tickets` query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminTicketListQuery {
    /// Status filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TicketStatus>,
    /// Only open (or only resolved) tickets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
    /// Intent filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<Intent>,
    /// Assignee filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Assignee>,
    /// Customer filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<UserId>,
    /// Page cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A ticket in the admin list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminTicketSummary {
    /// Customer-visible header.
    pub ticket: TicketSummary,
    /// Customer.
    pub customer: CustomerRef,
    /// AI or a person.
    pub assignee: Assignee,
    /// Classified intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<Intent>,
    /// Its probability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_confidence: Option<f64>,
    /// The route taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<TicketRoute>,
    /// Pending human tasks on it.
    pub pending_tasks: u32,
}

/// `GET /api/admin/tickets/{id}`: thread plus the "why" side panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminTicketDetail {
    /// Header.
    pub ticket: AdminTicketSummary,
    /// Every message, drafts and discarded drafts included, oldest first.
    pub messages: Vec<TicketMessage>,
    /// The latest classification with per-intent probabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Classification>,
    /// What the ticket waits for from the customer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting: Option<Awaiting>,
    /// Why it was escalated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation_reason: Option<String>,
    /// Confidence of the latest AI reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_confidence: Option<f64>,
    /// Human tasks about it.
    pub tasks: Vec<HumanTaskSummary>,
    /// Investigations it started.
    pub investigations: Vec<InvestigationSummary>,
    /// Refunds it started.
    pub refunds: Vec<RefundSummary>,
    /// Traces, oldest first (details via the trace endpoint).
    pub runs: Vec<AgentRunSummary>,
    /// Address that continues this ticket by email.
    pub reply_address: String,
}

/// `GET /api/admin/tickets/{id}/trace`: every LLM and tool call, with inputs and outputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TicketTrace {
    /// The ticket.
    pub ticket_id: TicketId,
    /// Traces, oldest first.
    pub runs: Vec<AgentRunDetail>,
}

/// A person acting on a ticket from the admin ticket page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TicketAction {
    /// Reply manually. Takes the ticket over unless it already is with a person.
    Reply {
        /// Body.
        body: String,
        /// Resolve the ticket after sending.
        #[serde(default)]
        close: bool,
    },
    /// Take over from the AI (`assignee = human`).
    TakeOver,
    /// Hand back to the AI: the messages nobody answered yet are routed again (under `intent`
    /// when given); with none, automatic handling resumes with the next message.
    HandBack {
        /// What the customer wants (never `other`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intent: Option<Intent>,
    },
    /// Resolve without replying.
    Close,
}

/// `POST /api/admin/tickets/{id}/actions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketActionRequest {
    /// The action.
    pub action: TicketAction,
}

impl Validate for TicketActionRequest {
    fn validate(&self) -> Result<(), ApiError> {
        let mut errors = Vec::new();
        match &self.action {
            TicketAction::Reply { body, .. } => {
                check_text(&mut errors, "action.body", body, 1, MAX_MESSAGE_CHARS);
            }
            TicketAction::HandBack { intent } => {
                check_hand_back(&mut errors, "action.intent", *intent);
            }
            TicketAction::TakeOver | TicketAction::Close => {}
        }
        ApiError::check(errors)
    }
}

/// Response of `POST /api/admin/tickets/{id}/actions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketUpdated {
    /// The ticket with its new status.
    pub ticket: TicketRef,
    /// Its assignee.
    pub assignee: Assignee,
}

// ---------------------------------------------------------------------------------------------
// Payments ledger
// ---------------------------------------------------------------------------------------------

/// Two captured charges of the same amount on one order within this many seconds are flagged
/// as a suspected duplicate (the `BillingIssue` detector uses the same rule).
pub const DUPLICATE_CHARGE_WINDOW_SECS: i64 = 15 * 60;

/// `GET /api/admin/payments` query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentListQuery {
    /// Charge or refund.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<PaymentKind>,
    /// Succeeded or failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PaymentStatus>,
    /// Order filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<OrderNumber>,
    /// Only entries in a suspected duplicate pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspicious: Option<bool>,
    /// Page cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// One ledger row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// The entry.
    pub payment: PaymentEntry,
    /// Customer.
    pub customer: CustomerRef,
    /// Suspected duplicate of this earlier charge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<PaymentId>,
    /// A refund already reverses this charge.
    pub refunded: bool,
}

/// A suspected double charge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateCharge {
    /// Order number.
    pub order_number: OrderNumber,
    /// The earlier charge.
    pub first: PaymentId,
    /// The later charge (the one to refund).
    pub duplicate: PaymentId,
    /// Amount of each.
    pub amount: Money,
    /// Seconds between them.
    pub seconds_apart: i64,
    /// A refund already reverses the duplicate.
    pub resolved: bool,
}

/// `GET /api/admin/payments`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentsLedger {
    /// Ledger rows, newest first.
    pub entries: crate::Page<LedgerEntry>,
    /// Every suspected duplicate in the org (not paginated), newest first.
    pub duplicates: Vec<DuplicateCharge>,
}

/// The deterministic duplicate-charge detector: succeeded charges on the same order with the
/// same amount within [`DUPLICATE_CHARGE_WINDOW_SECS`]. Each later charge is paired with the
/// earliest unpaired earlier one. `entries` may be in any order and may span orders.
pub fn find_duplicate_charges(entries: &[PaymentEntry]) -> Vec<DuplicateCharge> {
    let mut charges: Vec<&PaymentEntry> = entries
        .iter()
        .filter(|p| p.kind == PaymentKind::Charge && p.status == PaymentStatus::Succeeded)
        .collect();
    charges.sort_by(|a, b| {
        (&a.order_id, a.created_at, &a.id).cmp(&(&b.order_id, b.created_at, &b.id))
    });
    let refunded: Vec<&PaymentId> = entries
        .iter()
        .filter(|p| p.kind == PaymentKind::Refund && p.status == PaymentStatus::Succeeded)
        .filter_map(|p| p.refund_of.as_ref())
        .collect();

    let mut paired = vec![false; charges.len()];
    let mut found = Vec::new();
    for later in 0..charges.len() {
        for earlier in 0..later {
            let (a, b) = (charges[earlier], charges[later]);
            let seconds_apart = (b.created_at - a.created_at).whole_seconds();
            if paired[earlier]
                || a.order_id != b.order_id
                || a.amount != b.amount
                || seconds_apart > DUPLICATE_CHARGE_WINDOW_SECS
            {
                continue;
            }
            paired[earlier] = true;
            paired[later] = true;
            found.push(DuplicateCharge {
                order_number: b.order_number,
                first: a.id.clone(),
                duplicate: b.id.clone(),
                amount: b.amount,
                seconds_apart,
                resolved: refunded.contains(&&b.id),
            });
            break;
        }
    }
    found
}

// ---------------------------------------------------------------------------------------------
// Inventory
// ---------------------------------------------------------------------------------------------

/// `GET /api/admin/inventory` query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryQuery {
    /// Category filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
    /// Only low or out-of-stock products.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low_only: Option<bool>,
}

/// One product's stock row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryItem {
    /// Product.
    pub product_id: ProductId,
    /// SKU.
    pub sku: Sku,
    /// Name.
    pub name: String,
    /// Category.
    pub category: Category,
    /// Tier.
    pub tier: Tier,
    /// Price (global, read-only).
    pub price: Money,
    /// Stock in this org.
    pub stock: Stock,
    /// Catalog default (what demo reset restores).
    pub default_stock: u32,
    /// Units in unpaid or unshipped orders.
    pub reserved: u32,
    /// Last change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// `GET /api/admin/inventory`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryList {
    /// Rows in catalog order.
    pub items: Vec<InventoryItem>,
}

/// Largest stock an admin can set.
pub const MAX_STOCK: u32 = 9_999;

/// `PUT /api/admin/inventory/{product_id}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetStockRequest {
    /// New stock.
    pub stock: u32,
}

impl Validate for SetStockRequest {
    fn validate(&self) -> Result<(), ApiError> {
        if self.stock > MAX_STOCK {
            Err(ApiError::invalid_field(
                "stock",
                format!("At most {MAX_STOCK}"),
            ))
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Customers
// ---------------------------------------------------------------------------------------------

/// `GET /api/admin/customers` query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerListQuery {
    /// Search by email or name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Page cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// An org member with their activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerSummary {
    /// Identity.
    pub customer: CustomerRef,
    /// Role.
    pub role: OrgRole,
    /// Orders placed.
    pub order_count: u32,
    /// Open tickets.
    pub open_tickets: u32,
    /// Succeeded charges minus refunds.
    pub total_spent: Money,
    /// Last authenticated request.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub last_seen_at: Option<OffsetDateTime>,
}

/// `GET /api/admin/customers/{user_id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerDetail {
    /// Header.
    pub customer: CustomerSummary,
    /// Their orders, newest first.
    pub orders: Vec<OrderSummary>,
    /// Their tickets, newest first.
    pub tickets: Vec<TicketSummary>,
}

// ---------------------------------------------------------------------------------------------
// Email log
// ---------------------------------------------------------------------------------------------

/// `GET /api/admin/emails` query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailListQuery {
    /// Inbound or outbound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    /// Status filter (`rejected` shows dropped inbound mail).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<EmailStatus>,
    /// Ticket filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<TicketId>,
    /// Page cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// An email in the log list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailSummary {
    /// Log entry ID.
    pub id: EmailId,
    /// Inbound or outbound.
    pub direction: Direction,
    /// Email or web form.
    pub channel: Channel,
    /// Outcome.
    pub status: EmailStatus,
    /// Why inbound mail was dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject_reason: Option<RejectReason>,
    /// `From` address.
    pub from: String,
    /// `To` address.
    pub to: String,
    /// Subject.
    pub subject: String,
    /// Outbound template name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Related ticket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_id: Option<TicketId>,
    /// Related order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_number: Option<OrderNumber>,
    /// Send attempts (outbound; more than 1 shows provider-outage retries).
    pub attempts: u32,
    /// Logged.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Accepted by the provider.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub sent_at: Option<OffsetDateTime>,
}

/// `GET /api/admin/emails/{id}`: the copy as sent or received.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailDetail {
    /// Header.
    pub email: EmailSummary,
    /// `Message-ID` (without angle brackets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// `In-Reply-To`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    /// `References`.
    pub references: Vec<String>,
    /// `Reply-To`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// Display name of the sender.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_name: Option<String>,
    /// Plain-text body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_body: Option<String>,
    /// HTML body (render in a sandboxed iframe).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_body: Option<String>,
    /// Inbound: `Authentication-Results` summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_results: Option<String>,
    /// Inbound: raw size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_size: Option<u32>,
    /// Outbound: provider message ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Last error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Settings and demo reset
// ---------------------------------------------------------------------------------------------

/// `GET /api/admin/settings` and the response of `PUT`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsResponse {
    /// The org.
    pub org: OrgInfo,
    /// Current settings.
    pub settings: OrgSettings,
    /// Defaults, for "reset to default" buttons.
    pub defaults: OrgSettings,
}

/// `PUT /api/admin/settings`: replace the settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateSettingsRequest {
    /// The full settings (missing fields take their defaults).
    pub settings: OrgSettings,
}

impl Validate for UpdateSettingsRequest {
    fn validate(&self) -> Result<(), ApiError> {
        self.settings
            .validate()
            .map_err(|e| ApiError::invalid_field("settings", e.to_string()))
    }
}

/// The word the reset dialog asks the admin to type.
pub const RESET_CONFIRMATION: &str = "RESET";

/// `POST /api/admin/demo/reset`: wipe the org's orders, tickets, invoices, tasks and traces
/// and restore default stock. Settings, members, carts, builds and addresses survive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoResetRequest {
    /// Must be [`RESET_CONFIRMATION`].
    pub confirm: String,
}

impl Validate for DemoResetRequest {
    fn validate(&self) -> Result<(), ApiError> {
        if self.confirm == RESET_CONFIRMATION {
            Ok(())
        } else {
            Err(ApiError::invalid_field(
                "confirm",
                format!("Type {RESET_CONFIRMATION} to confirm"),
            ))
        }
    }
}

/// Response of `POST /api/admin/demo/reset`: rows deleted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoResetResponse {
    /// Orders deleted.
    pub orders: u32,
    /// Tickets deleted.
    pub tickets: u32,
    /// Human tasks deleted.
    pub tasks: u32,
    /// Emails deleted from the log.
    pub emails: u32,
    /// Products whose stock was restored.
    pub products_restocked: u32,
}

/// Helper for list filters that accept "TD-1042" or "1042".
pub fn parse_order_query(q: &str) -> Option<OrderNumber> {
    let q = q.trim();
    OrderNumber::parse(q)
        .ok()
        .or_else(|| q.parse::<u32>().ok().map(OrderNumber::new))
}

#[cfg(test)]
mod tests {
    use techdepo_domain::OrderId;
    use time::macros::datetime;

    use super::*;
    use crate::vocab::PaymentMethod;

    fn charge(id: &str, order: &str, cents: i64, at: OffsetDateTime) -> PaymentEntry {
        PaymentEntry {
            id: PaymentId::new(id),
            order_id: OrderId::new(order),
            order_number: OrderNumber::new(1042),
            kind: PaymentKind::Charge,
            status: PaymentStatus::Succeeded,
            amount: Money::from_cents(cents),
            method: PaymentMethod::Card,
            provider_ref: format!("ch_{id}"),
            statement_descriptor: "TECHDEPO*TD-1042".into(),
            refund_of: None,
            failure_reason: None,
            created_at: at,
            sim_at: at,
        }
    }

    #[test]
    fn detects_duplicate_charges() {
        let t = datetime!(2026-10-13 12:00 UTC);
        let mut entries = vec![
            charge("pay_b", "ord_1", 22_990, t + time::Duration::seconds(2)),
            charge("pay_a", "ord_1", 22_990, t),
            // Same amount, other order: not a duplicate.
            charge("pay_c", "ord_2", 22_990, t),
            // Same order, outside the window.
            charge("pay_d", "ord_1", 22_990, t + time::Duration::hours(2)),
        ];
        let found = find_duplicate_charges(&entries);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].first.as_str(), "pay_a");
        assert_eq!(found[0].duplicate.as_str(), "pay_b");
        assert_eq!(found[0].seconds_apart, 2);
        assert!(!found[0].resolved);

        let mut refund = charge("pay_r", "ord_1", 22_990, t + time::Duration::minutes(5));
        refund.kind = PaymentKind::Refund;
        refund.refund_of = Some(PaymentId::new("pay_b"));
        entries.push(refund);
        assert!(find_duplicate_charges(&entries)[0].resolved);
    }

    #[test]
    fn order_query_parsing() {
        assert_eq!(parse_order_query("TD-1042"), Some(OrderNumber::new(1042)));
        assert_eq!(parse_order_query(" 1042 "), Some(OrderNumber::new(1042)));
        assert_eq!(parse_order_query("jane@example.com"), None);
    }

    #[test]
    fn reset_needs_confirmation() {
        assert!(
            DemoResetRequest {
                confirm: "reset".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            DemoResetRequest {
                confirm: RESET_CONFIRMATION.into()
            }
            .validate()
            .is_ok()
        );
    }
}
