//! Every email TechDepó sends, as pure functions from facts to [`EmailContent`].
//!
//! - **Order lifecycle** ([`OrderTemplate`]): confirmation with the invoice summary, payment
//!   received, shipped, delivered, cancelled, refund started / done / rejected, invoice
//!   reissued. `Notifications/order` loads the facts from D1 and picks the template.
//! - **Support** ([`SupportTemplate`]): the transactional replies of the support routes and
//!   workflows (support.md §4 and §6): ticket received, ask for the order number, the cancel
//!   and where-is-my-order answers, investigation opened, the offer, the reminder, closed, and
//!   free text (an agent's or a person's reply) in the same layout. `SupportTicket` (support
//!   pipeline) renders them through `Notifications/ticket`.
//! - **Admin** ([`task_overdue`]): the SLA nudge of a human task.
//!
//! Copy rules: customer emails never mention demo faults or internal truth (a lost parcel is
//! "in transit"), and every email links back to the page where the customer can see more.

use serde::{Deserialize, Serialize};
use techdepo_domain::{
    HumanTaskId, InvoiceNumber, Money, OrderNumber, RETURN_WINDOW_DAYS, TicketId, TrackingNumber,
    VatRate,
};
use time::Date;
use time::macros::format_description;

use super::content::{EmailContent, Fact, ItemLine};

/// Absolute links into the SPA (`APP_BASE_URL`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Links {
    base_url: String,
}

impl Links {
    /// Links under `base_url` (no trailing slash needed).
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
        }
    }

    /// The customer's order page.
    pub fn order(&self, number: OrderNumber) -> String {
        format!("{}/account/orders/{number}", self.base_url)
    }

    /// The customer's order list.
    pub fn orders(&self) -> String {
        format!("{}/account/orders", self.base_url)
    }

    /// The customer's ticket page.
    pub fn ticket(&self, ticket: &TicketId) -> String {
        format!("{}/account/support/{ticket}", self.base_url)
    }

    /// The admin's task page.
    pub fn task(&self, task: &HumanTaskId) -> String {
        format!("{}/admin/tasks/{task}", self.base_url)
    }
}

/// `Mon, 19 Oct 2026`.
pub fn fmt_date(date: Date) -> String {
    date.format(format_description!(
        "[weekday repr:short], [day padding:none] [month repr:short] [year]"
    ))
    .unwrap_or_else(|_| date.to_string())
}

/// The greeting, with the whole name: guessing a first name from free text gets family-name-first
/// names (Hungarian: `Kiss Anna`) wrong.
fn hello(name: Option<&str>) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => format!("Hi {name},"),
        None => "Hi,".to_owned(),
    }
}

// -- order lifecycle ------------------------------------------------------------------------------

/// What every order email knows about the order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderFacts {
    /// Number.
    pub number: OrderNumber,
    /// Customer's name.
    pub customer_name: Option<String>,
    /// Lines.
    pub items: Vec<ItemLine>,
    /// Items subtotal.
    pub items_total: Money,
    /// Shipping.
    pub shipping: Money,
    /// Total.
    pub total: Money,
    /// VAT rate.
    pub vat_rate: VatRate,
    /// Shipping address lines.
    pub ship_to: Vec<String>,
    /// Paid by the store (a replacement).
    pub store_paid: bool,
}

impl OrderFacts {
    fn totals(&self) -> Vec<Fact> {
        let b = self.total.vat_breakdown(self.vat_rate);
        vec![
            Fact::new("Items", self.items_total),
            Fact::new(
                "Shipping",
                if self.shipping.is_positive() {
                    self.shipping.to_string()
                } else {
                    "Free".to_owned()
                },
            ),
            Fact::new(format!("Incl. VAT {}", self.vat_rate), b.vat),
            Fact::new("Total", self.total),
        ]
    }
}

/// The invoice summary in the confirmation email.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceFacts {
    /// Number.
    pub number: InvoiceNumber,
    /// Issue date.
    pub issue_date: Date,
    /// Gross.
    pub gross: Money,
    /// VAT rate.
    pub vat_rate: VatRate,
}

impl InvoiceFacts {
    fn facts(&self) -> Vec<Fact> {
        let b = self.gross.vat_breakdown(self.vat_rate);
        vec![
            Fact::new("Invoice", self.number),
            Fact::new("Issued", fmt_date(self.issue_date)),
            Fact::new("Net", b.net),
            Fact::new(format!("VAT {}", self.vat_rate), b.vat),
            Fact::new("Total", b.gross),
        ]
    }
}

/// The parcel, as the customer sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipmentFacts {
    /// Carrier.
    pub carrier: String,
    /// Tracking number.
    pub tracking_number: TrackingNumber,
    /// Ship date (simulated).
    pub shipped_on: Date,
    /// Expected delivery.
    pub eta: Date,
}

/// An order email and its specific facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "template", rename_all = "snake_case")]
pub enum OrderTemplate {
    /// After payment: thanks, the items and the invoice summary (the invoice is attached).
    OrderConfirmation {
        /// The invoice, if issued (replacement orders have none).
        invoice: Option<InvoiceFacts>,
    },
    /// A payment was captured (a retried payment after a decline).
    PaymentReceived {
        /// Amount.
        amount: Money,
    },
    /// Handed to the carrier.
    Shipped {
        /// The parcel.
        shipment: ShipmentFacts,
    },
    /// Delivered.
    Delivered {
        /// Delivery date (simulated).
        on: Date,
    },
    /// Cancelled before payment.
    Cancelled {
        /// Why, if given.
        reason: Option<String>,
    },
    /// A refund awaits approval.
    RefundStarted {
        /// Amount.
        amount: Money,
    },
    /// The money is on its way back.
    RefundDone {
        /// Amount.
        amount: Money,
        /// The whole order was refunded.
        full: bool,
        /// Credit note, if the order was invoiced.
        credit_note: Option<InvoiceNumber>,
    },
    /// A person rejected the refund.
    RefundRejected {
        /// Amount asked for.
        amount: Money,
        /// Why.
        reason: String,
    },
    /// The invoice was replaced by a corrected one (attached).
    InvoiceReissued {
        /// The voided invoice.
        voided: InvoiceNumber,
        /// Its credit note.
        credit_note: InvoiceNumber,
        /// The corrected invoice.
        reissued: InvoiceNumber,
        /// Why.
        reason: String,
    },
}

impl OrderTemplate {
    /// `email_log.template` and the `Message-ID` part.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::OrderConfirmation { .. } => "order_confirmation",
            Self::PaymentReceived { .. } => "payment_received",
            Self::Shipped { .. } => "order_shipped",
            Self::Delivered { .. } => "order_delivered",
            Self::Cancelled { .. } => "order_cancelled",
            Self::RefundStarted { .. } => "refund_started",
            Self::RefundDone { .. } => "refund_done",
            Self::RefundRejected { .. } => "refund_rejected",
            Self::InvoiceReissued { .. } => "invoice_reissued",
        }
    }
}

/// Composes an order email.
pub fn order_email(order: &OrderFacts, template: &OrderTemplate, links: &Links) -> EmailContent {
    let n = order.number;
    let hi = hello(order.customer_name.as_deref());
    let link = links.order(n);
    match template {
        OrderTemplate::OrderConfirmation { invoice } => {
            let mut c = EmailContent::new(
                template.name(),
                format!("Order {n} confirmed"),
                "Thanks for your order",
            )
            .preheader(format!("We received your payment for {n} and are preparing your parcel."))
            .text(hi)
            .text(if order.store_paid {
                format!(
                    "Your replacement order {n} is on us. We are preparing it now and will email you when it ships."
                )
            } else {
                format!(
                    "We received your payment of {} for order {n} and started preparing your parcel. We will email you when it ships.",
                    order.total
                )
            })
            .items(order.items.clone(), order.totals())
            .facts(vec![Fact::new("Ship to", order.ship_to.join(", "))]);
            if let Some(invoice) = invoice {
                c = c
                    .text("Your invoice is attached to this email and always available on the order page.")
                    .facts(invoice.facts());
            }
            c.cta("View your order", link)
        }
        OrderTemplate::PaymentReceived { amount } => EmailContent::new(
            template.name(),
            format!("Payment received for {n}"),
            "Payment received",
        )
        .text(hi)
        .text(format!("We received your payment of {amount} for order {n}. Thank you!"))
        .cta("View your order", link),
        OrderTemplate::Shipped { shipment } => EmailContent::new(
            template.name(),
            format!("Your order {n} has shipped"),
            "Your order is on its way",
        )
        .preheader(format!(
            "{} is bringing it, expected {}.",
            shipment.carrier,
            fmt_date(shipment.eta)
        ))
        .text(hi)
        .text(format!(
            "Order {n} left our warehouse. {} usually delivers within 1–3 workdays.",
            shipment.carrier
        ))
        .facts(vec![
            Fact::new("Carrier", &shipment.carrier),
            Fact::new("Tracking number", &shipment.tracking_number),
            Fact::new("Shipped", fmt_date(shipment.shipped_on)),
            Fact::new("Expected delivery", fmt_date(shipment.eta)),
        ])
        .cta("Track your order", link),
        OrderTemplate::Delivered { on } => EmailContent::new(
            template.name(),
            format!("Delivered: order {n}"),
            "Your order was delivered",
        )
        .text(hi)
        .text(format!(
            "The carrier reports that order {n} was delivered on {}. Enjoy your new parts!",
            fmt_date(*on)
        ))
        .text(format!(
            "Something not right? You can return the order within {RETURN_WINDOW_DAYS} days of delivery. Just reply to this email."
        ))
        .cta("View your order", link),
        OrderTemplate::Cancelled { reason } => {
            let mut c = EmailContent::new(
                template.name(),
                format!("Order {n} cancelled"),
                "Your order was cancelled",
            )
            .text(hi)
            .text(format!(
                "Order {n} was cancelled before payment, so nothing was charged."
            ));
            if let Some(reason) = reason.as_deref().filter(|r| !r.trim().is_empty()) {
                c = c.facts(vec![Fact::new("Reason", reason.trim())]);
            }
            c.cta("View your order", link)
        }
        OrderTemplate::RefundStarted { amount } => EmailContent::new(
            template.name(),
            format!("Refund requested for {n}"),
            "We are reviewing your refund",
        )
        .text(hi)
        .text(format!(
            "We registered a refund of {amount} for order {n}. A colleague confirms every refund before the money moves; we will email you as soon as it is done."
        ))
        .cta("View your order", link),
        OrderTemplate::RefundDone {
            amount,
            full,
            credit_note,
        } => {
            let mut facts = vec![Fact::new("Amount", amount)];
            if let Some(cn) = credit_note {
                facts.push(Fact::new("Credit note", cn));
            }
            EmailContent::new(
                template.name(),
                format!("Your refund for {n} is on its way"),
                "Your refund is on its way",
            )
            .text(hi)
            .text(if *full {
                format!("We refunded order {n} in full. The money goes back to the original payment method and usually arrives within 3–5 business days.")
            } else {
                format!("We refunded {amount} for order {n}. The money goes back to the original payment method and usually arrives within 3–5 business days.")
            })
            .facts(facts)
            .cta("View your order", link)
        }
        OrderTemplate::RefundRejected { amount, reason } => EmailContent::new(
            template.name(),
            format!("About your refund for {n}"),
            "We could not approve your refund",
        )
        .text(hi)
        .text(format!(
            "We looked at your refund request of {amount} for order {n} and could not approve it:"
        ))
        .quote(reason.trim())
        .text("If you think we got this wrong, just reply to this email and a person will look at it again.")
        .cta("View your order", link),
        OrderTemplate::InvoiceReissued {
            voided,
            credit_note,
            reissued,
            reason,
        } => EmailContent::new(
            template.name(),
            format!("Corrected invoice for order {n}"),
            "Here is your corrected invoice",
        )
        .text(hi)
        .text(format!(
            "We replaced invoice {voided} of order {n} with a corrected one. Nothing changes about your payment."
        ))
        .facts(vec![
            Fact::new("New invoice", reissued),
            Fact::new("Replaces", voided),
            Fact::new("Credit note", credit_note),
            Fact::new("Reason", reason.trim()),
        ])
        .text("The corrected invoice is attached.")
        .cta("View your order", link),
    }
}

// -- support --------------------------------------------------------------------------------------

/// What happened to a cancellation request (support.md §4 `CancelOrder`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CancelOutcome {
    /// Unpaid: cancelled.
    Cancelled,
    /// Paid, not shipped: a refund awaits approval.
    RefundRequested {
        /// Amount.
        amount: Money,
    },
    /// Shipped: can be returned once it arrives.
    AlreadyShipped,
    /// Delivered within the window: the return is requested.
    ReturnRequested,
    /// Delivered, window over.
    ReturnWindowClosed {
        /// Delivery date.
        delivered_on: Date,
    },
    /// A return is under way.
    ReturnUnderWay,
    /// Already cancelled or refunded.
    AlreadyClosed,
}

/// The where-is-my-order answer (support.md §4 `WhereIsMyOrder`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WismoAnswer {
    /// Not paid yet.
    AwaitingPayment,
    /// Paid, not shipped.
    BeingPrepared,
    /// Delivered.
    Delivered {
        /// Delivery date.
        on: Date,
    },
    /// 0–3 workdays since shipment: the standard estimate.
    WithinEstimate {
        /// The parcel.
        shipment: ShipmentFacts,
    },
    /// 4 workdays: the carrier is overloaded.
    CarrierOverloaded {
        /// The parcel.
        shipment: ShipmentFacts,
    },
    /// Cancelled, refunded or returned: nothing is on its way.
    Closed {
        /// The order's state, as the customer sees it (`cancelled`, `refunded`, …).
        state: String,
    },
}

/// A corrected invoice (the `BillingIssue` route voided and reissued it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectedInvoice {
    /// The voided invoice.
    pub voided: InvoiceNumber,
    /// The corrected one.
    pub reissued: InvoiceNumber,
}

/// What the customer is offered after a failed trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfferKind {
    /// A refund.
    Refund,
    /// A replacement.
    Reship,
    /// Their choice.
    Both,
}

/// A support email and its facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "template", rename_all = "snake_case")]
pub enum SupportTemplate {
    /// Acknowledges a new ticket.
    TicketReceived,
    /// The route needs an order number.
    AskOrderNumber,
    /// The cancel route's answer.
    Cancel {
        /// Order.
        order_number: OrderNumber,
        /// What happened.
        outcome: CancelOutcome,
    },
    /// The where-is-my-order answer.
    Wismo {
        /// Order.
        order_number: OrderNumber,
        /// The answer.
        answer: WismoAnswer,
        /// The customer named no order: this is their only one (the reply says so).
        #[serde(default)]
        inferred: bool,
    },
    /// The order numbers the customer named are not on their account: which order is it?
    OrderNotFound {
        /// The numbers (never looked up beyond "not yours").
        numbers: Vec<OrderNumber>,
    },
    /// The refund route's answer for a paid, unshipped order: a refund awaits approval.
    RefundRequested {
        /// Order.
        order_number: OrderNumber,
        /// Amount.
        amount: Money,
    },
    /// A refund of the order is already being handled.
    RefundUnderWay {
        /// Order.
        order_number: OrderNumber,
    },
    /// A refund request for an unpaid order: nothing was charged.
    NothingToRefund {
        /// Order.
        order_number: OrderNumber,
    },
    /// The billing route's answer: the duplicate charge's refund awaits approval and/or the
    /// invoice was corrected.
    BillingFixed {
        /// Order.
        order_number: OrderNumber,
        /// The duplicate charge being refunded.
        duplicate: Option<Money>,
        /// The corrected invoice.
        invoice: Option<CorrectedInvoice>,
    },
    /// An investigation with the carrier is already open.
    InvestigationUnderWay {
        /// Order.
        order_number: OrderNumber,
    },
    /// An investigation was opened with the carrier.
    InvestigationOpened {
        /// Order.
        order_number: OrderNumber,
    },
    /// The carrier found the parcel.
    ParcelFound {
        /// Order.
        order_number: OrderNumber,
        /// New delivery date.
        eta: Date,
    },
    /// The offer after a failed trace.
    Offer {
        /// Order.
        order_number: OrderNumber,
        /// What is offered.
        kind: OfferKind,
        /// The refund amount (refund and both).
        refund_amount: Option<Money>,
    },
    /// The answer to the offer was unclear: ask again.
    OfferUnclear {
        /// Order.
        order_number: OrderNumber,
        /// What is offered.
        kind: OfferKind,
        /// The refund amount (refund and both).
        refund_amount: Option<Money>,
    },
    /// The customer accepted the replacement: it is on its way.
    ReplacementPlaced {
        /// The lost order.
        order_number: OrderNumber,
        /// The replacement order.
        replacement: OrderNumber,
    },
    /// A refund the ticket asked for completed (`RefundWorkflow`, support.md §4).
    RefundDone {
        /// Order.
        order_number: OrderNumber,
        /// Amount.
        amount: Money,
        /// The whole order was refunded.
        full: bool,
        /// Credit note, if one was issued.
        credit_note: Option<InvoiceNumber>,
    },
    /// A refund the ticket asked for was rejected (or could not be paid out).
    RefundRejected {
        /// Order.
        order_number: OrderNumber,
        /// Amount.
        amount: Money,
        /// Why (shown to the customer).
        reason: String,
    },
    /// No answer yet: a gentle reminder of what we asked.
    Reminder {
        /// What we are waiting for, as one sentence.
        waiting_for: String,
    },
    /// The ticket is closed.
    Closed,
    /// The ticket went to a person: a colleague will reply (support.md §8). Answers nothing.
    Escalated,
    /// Free text (an agent's reply after the guardrails, or a person's reply) in the layout.
    Reply {
        /// Body; blank lines separate paragraphs.
        body: String,
    },
}

impl SupportTemplate {
    /// `email_log.template`.
    pub fn name(&self) -> String {
        match self {
            Self::TicketReceived => "ticket_received".to_owned(),
            Self::AskOrderNumber => "ask_order_number".to_owned(),
            Self::Cancel { outcome, .. } => format!(
                "cancel_{}",
                match outcome {
                    CancelOutcome::Cancelled => "cancelled",
                    CancelOutcome::RefundRequested { .. } => "refund_requested",
                    CancelOutcome::AlreadyShipped => "already_shipped",
                    CancelOutcome::ReturnRequested => "return_requested",
                    CancelOutcome::ReturnWindowClosed { .. } => "window_closed",
                    CancelOutcome::ReturnUnderWay => "return_under_way",
                    CancelOutcome::AlreadyClosed => "already_closed",
                }
            ),
            Self::Wismo { answer, .. } => format!(
                "wismo_{}",
                match answer {
                    WismoAnswer::AwaitingPayment => "awaiting_payment",
                    WismoAnswer::BeingPrepared => "being_prepared",
                    WismoAnswer::Delivered { .. } => "delivered",
                    WismoAnswer::WithinEstimate { .. } => "within_estimate",
                    WismoAnswer::CarrierOverloaded { .. } => "carrier_overloaded",
                    WismoAnswer::Closed { .. } => "closed",
                }
            ),
            Self::OrderNotFound { .. } => "order_not_found".to_owned(),
            Self::RefundRequested { .. } => "refund_requested".to_owned(),
            Self::RefundUnderWay { .. } => "refund_under_way".to_owned(),
            Self::NothingToRefund { .. } => "nothing_to_refund".to_owned(),
            Self::BillingFixed { .. } => "billing_fixed".to_owned(),
            Self::InvestigationUnderWay { .. } => "investigation_under_way".to_owned(),
            Self::OfferUnclear { .. } => "offer_unclear".to_owned(),
            Self::ReplacementPlaced { .. } => "replacement_placed".to_owned(),
            Self::InvestigationOpened { .. } => "investigation_opened".to_owned(),
            Self::ParcelFound { .. } => "parcel_found".to_owned(),
            Self::Offer { .. } => "offer".to_owned(),
            Self::RefundDone { .. } => "refund_done".to_owned(),
            Self::RefundRejected { .. } => "refund_rejected".to_owned(),
            Self::Reminder { .. } => "reminder".to_owned(),
            Self::Closed => "ticket_closed".to_owned(),
            Self::Escalated => "ticket_escalated".to_owned(),
            Self::Reply { .. } => "reply".to_owned(),
        }
    }
}

/// The ticket a support email belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketFacts {
    /// Ticket.
    pub ticket_id: TicketId,
    /// The conversation's subject (the customer's first subject, without `Re:`).
    pub subject: String,
    /// Customer's name.
    pub customer_name: Option<String>,
}

/// `Re: <subject> [#T-7F2M9Q]` (support.md §6: the third fallback for finding the ticket).
pub fn ticket_subject(subject: &str, ticket: &TicketId) -> String {
    let mut s = subject.trim();
    while let Some(rest) = s
        .get(..3)
        .filter(|p| p.eq_ignore_ascii_case("re:"))
        .map(|_| s[3..].trim_start())
    {
        s = rest;
    }
    let tag = format!("[#{ticket}]");
    let s = s.replace(&tag, "");
    let s = s.trim();
    let s = if s.is_empty() { "Your question" } else { s };
    format!("Re: {s} {tag}")
}

/// `TD-1, TD-2 and TD-3`.
fn join_numbers(numbers: &[OrderNumber]) -> String {
    let all: Vec<String> = numbers.iter().map(ToString::to_string).collect();
    match all.split_last() {
        None => "that order".to_owned(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// Composes a support email.
pub fn support_email(
    ticket: &TicketFacts,
    template: &SupportTemplate,
    links: &Links,
) -> EmailContent {
    let subject = ticket_subject(&ticket.subject, &ticket.ticket_id);
    let hi = hello(ticket.customer_name.as_deref());
    let t = &ticket.ticket_id;
    let c = |heading: &str| {
        EmailContent::new(template.name(), subject.clone(), heading).text(hi.clone())
    };
    let content = match template {
        SupportTemplate::TicketReceived => c("We got your message")
            .text(format!("Thanks for writing to us. Your ticket number is {t}; we are on it and will reply shortly.")),
        SupportTemplate::AskOrderNumber => c("Which order is this about?")
            .text("To help you we need the order number. You can find it in your confirmation email or on your orders page; it looks like TD-1042.")
            .text("Just reply to this email with the number.")
            .text(format!("All your orders: {}", links.orders())),
        SupportTemplate::OrderNotFound { numbers } => c("Which order is this about?")
            .text(format!(
                "We could not find {} among the orders on your account, so we can't act on {}.",
                join_numbers(numbers),
                if numbers.len() == 1 { "it" } else { "them" }
            ))
            .text("Please reply with the number of your own order; it looks like TD-1042.")
            .text(format!("All your orders: {}", links.orders())),
        SupportTemplate::Cancel { order_number: n, outcome } => match outcome {
            CancelOutcome::Cancelled => c("Your order is cancelled")
                .text(format!("We cancelled order {n}. It was not paid yet, so nothing was charged.")),
            CancelOutcome::RefundRequested { amount } => c("Refund requested")
                .text(format!("Order {n} is already paid, so cancelling it means refunding {amount}. We requested the refund; a colleague confirms it and we will email you when the money is on its way.")),
            CancelOutcome::AlreadyShipped => c("Your order is already on its way")
                .text(format!("Order {n} has already shipped, so we can't cancel it anymore. Once it arrives you can return it within {RETURN_WINDOW_DAYS} days and we refund you.")),
            CancelOutcome::ReturnRequested => c("Return requested")
                .text(format!("We registered a return for order {n}. Please send the parcel back to us; the refund starts as soon as it arrives."))
                .note("Return address: TechDepó Demo Kft., Fő utca 1, 1011 Budapest, HU"),
            CancelOutcome::ReturnWindowClosed { delivered_on } => c("The return window has closed")
                .text(format!("Order {n} was delivered on {}, more than {RETURN_WINDOW_DAYS} days ago, so it can no longer be returned.", fmt_date(*delivered_on)))
                .text("If a part is faulty, the manufacturer's warranty still covers it: reply with what is wrong and we will help you with the claim."),
            CancelOutcome::ReturnUnderWay => c("Your return is under way")
                .text(format!("The return of order {n} is already under way. We refund you as soon as the parcel is back with us.")),
            CancelOutcome::AlreadyClosed => c("Already taken care of")
                .text(format!("Order {n} is already cancelled or refunded, so there is nothing left to do.")),
        },
        SupportTemplate::Wismo { order_number: n, answer, inferred } => {
            let c = |heading: &str| {
                let base = c(heading);
                if *inferred {
                    base.text(format!("You didn't mention an order number, so we looked up {n}, the only order on your account."))
                } else {
                    base
                }
            };
            match answer {
            WismoAnswer::AwaitingPayment => c("Your order is waiting for payment")
                .text(format!("Order {n} is not paid yet. As soon as the payment arrives we start preparing it.")),
            WismoAnswer::BeingPrepared => c("Your order is being prepared")
                .text(format!("Order {n} is paid and we are preparing your parcel. We will email you the tracking number as soon as it ships.")),
            WismoAnswer::Delivered { on } => c("Your order was delivered")
                .text(format!("The carrier reports that order {n} was delivered on {}. If you can't find it, reply to this email and we will look into it.", fmt_date(*on))),
            WismoAnswer::WithinEstimate { shipment } => c("Your order is on its way")
                .text(format!("Order {n} shipped on {} and is within the carrier's standard 1–3 workday delivery estimate.", fmt_date(shipment.shipped_on)))
                .facts(vec![
                    Fact::new("Carrier", &shipment.carrier),
                    Fact::new("Tracking number", &shipment.tracking_number),
                    Fact::new("Expected delivery", fmt_date(shipment.eta)),
                ]),
            WismoAnswer::CarrierOverloaded { shipment } => c("Your parcel is a little late")
                .text(format!("Order {n} shipped on {}. {} is overloaded at the moment and parcels take a little longer than usual. Please give it one or two more workdays; if it still hasn't arrived, reply and we open an investigation with the carrier.", fmt_date(shipment.shipped_on), shipment.carrier))
                .facts(vec![Fact::new("Tracking number", &shipment.tracking_number)]),
            WismoAnswer::Closed { state } => c("Nothing is on its way")
                .text(format!("Order {n} is {state}, so there is no parcel on its way. If you think that is wrong, just reply and a colleague will look at it.")),
            }
        }
        SupportTemplate::RefundRequested { order_number: n, amount } => c("Refund requested")
            .text(format!("Order {n} has not shipped yet, so we can refund it: we requested a refund of {amount}. A colleague confirms every refund before the money moves; we will email you as soon as it is on its way.")),
        SupportTemplate::RefundUnderWay { order_number: n } => c("Your refund is already being handled")
            .text(format!("A refund for order {n} is already under way. We will email you as soon as it is done.")),
        SupportTemplate::NothingToRefund { order_number: n } => c("Nothing was charged")
            .text(format!("Order {n} is not paid yet, so nothing was charged and there is nothing to refund."))
            .text(format!("If you don't want the order any more, reply \"cancel {n}\" and we cancel it.")),
        SupportTemplate::BillingFixed { order_number: n, duplicate, invoice } => {
            // A duplicate charge still waits for a person's approval: nothing is "sorted out" yet.
            let mut c = c(match (duplicate, invoice) {
                (Some(_), None) => "We found the duplicate charge",
                (None, Some(_)) => "We corrected your invoice",
                _ => "We are sorting out your billing",
            });
            if let Some(amount) = duplicate {
                c = c.text(format!("You are right: the payment of {amount} for order {n} was captured twice. We requested a refund of the duplicate charge; a colleague confirms it and we will email you when the money is on its way."));
            }
            if let Some(inv) = invoice {
                c = c
                    .text(format!("The invoice of order {n} was missing your billing address. We voided it and issued a corrected one, which you receive in a separate email and always find on the order page."))
                    .facts(vec![
                        Fact::new("New invoice", inv.reissued),
                        Fact::new("Replaces", inv.voided),
                    ]);
            }
            c
        }
        SupportTemplate::InvestigationUnderWay { order_number: n } => c("We are already on it")
            .text(format!("We already opened an investigation with the carrier about order {n} and will get back to you as soon as we hear from them.")),
        SupportTemplate::InvestigationOpened { order_number: n } => c("We opened an investigation")
            .text(format!("Order {n} is taking longer than it should. We opened an investigation with the carrier and will get back to you as soon as we hear from them, usually within a day.")),
        SupportTemplate::ParcelFound { order_number: n, eta } => c("Good news: your parcel was found")
            .text(format!("The carrier found the parcel of order {n}. It is back on track and should arrive on {}.", fmt_date(*eta))),
        SupportTemplate::Offer { order_number: n, kind, refund_amount } => {
            let refund = refund_amount.map_or_else(|| "a full refund".to_owned(), |a| format!("a refund of {a}"));
            let base = c("We are sorry: your parcel is lost")
                .text(format!("Unfortunately the carrier could not locate the parcel of order {n}. We are very sorry about that."));
            match kind {
                OfferKind::Refund => base.text(format!("We would like to offer you {refund}. Reply \"refund\" to accept.")),
                OfferKind::Reship => base.text("We would like to send you a replacement at our cost. Reply \"replacement\" to accept."),
                OfferKind::Both => base.text(format!("You can choose: {refund}, or a replacement at our cost. Just reply \"refund\" or \"replacement\".")),
            }
        }
        SupportTemplate::OfferUnclear { order_number: n, kind, refund_amount } => {
            let refund = refund_amount.map_or_else(|| "a full refund".to_owned(), |a| format!("a refund of {a}"));
            let base = c("Sorry, we weren't sure what you meant")
                .text(format!("Thanks for your reply about order {n}. We want to get this right, so let us ask once more."));
            match kind {
                OfferKind::Refund => base.text(format!("We can offer you {refund}. Reply \"refund\" to accept it, or tell us what you would prefer.")),
                OfferKind::Reship => base.text("We can send you a replacement at our cost. Reply \"replacement\" to accept it, or tell us what you would prefer."),
                OfferKind::Both => base.text(format!("Would you like {refund} or a replacement at our cost? Just reply \"refund\" or \"replacement\".")),
            }
        }
        SupportTemplate::ReplacementPlaced { order_number: n, replacement } => c("Your replacement is on its way")
            .text(format!("Thanks for letting us know. We placed replacement order {replacement} for the lost parcel of order {n}, at our cost. We will email you the tracking number as soon as it ships.")),
        SupportTemplate::RefundDone { order_number: n, amount, full, credit_note } => {
            let mut facts = vec![Fact::new("Amount", amount)];
            if let Some(cn) = credit_note {
                facts.push(Fact::new("Credit note", cn));
            }
            c("Your refund is on its way")
                .text(if *full {
                    format!("We refunded order {n} in full. The money goes back to the original payment method and usually arrives within 3–5 business days.")
                } else {
                    format!("We refunded {amount} for order {n}. The money goes back to the original payment method and usually arrives within 3–5 business days.")
                })
                .facts(facts)
        }
        SupportTemplate::RefundRejected { order_number: n, amount, reason } => c("We could not approve your refund")
            .text(format!("We looked at your refund request of {amount} for order {n} and could not approve it:"))
            .quote(reason.trim())
            .text("If you think we got this wrong, just reply to this email and a person will look at it again."),
        SupportTemplate::Reminder { waiting_for } => c("Just checking in")
            .text(format!("We are still waiting for your answer: {}", waiting_for.trim()))
            .text("Reply to this email whenever you are ready. If we don't hear from you, we will close the ticket in a few days."),
        SupportTemplate::Closed => c("Your ticket is closed")
            .text(format!("We closed ticket {t}. If you need anything else, just reply to this email and it reopens.")),
        SupportTemplate::Escalated => c("A colleague will look at this")
            .text("We passed your message to a colleague, who will get back to you in this conversation.")
            .text("There is nothing you need to do in the meantime. If you want to add something, just reply to this email."),
        SupportTemplate::Reply { body } => EmailContent::new(template.name(), subject.clone(), "Our reply")
            .body(body),
    };
    content.cta("View the conversation", links.ticket(t))
}

// -- admin ----------------------------------------------------------------------------------------

/// The SLA nudge of a pending human task, to the org's admins.
pub fn task_overdue(
    task: &HumanTaskId,
    kind_label: &str,
    title: &str,
    summary: &str,
    waiting_since: &str,
    links: &Links,
) -> EmailContent {
    EmailContent::new(
        "task_overdue",
        format!("Waiting for you: {title}"),
        "A decision is waiting for you",
    )
    .text(format!(
        "This {} has passed its SLA. The workflow keeps waiting until someone decides.",
        kind_label.to_lowercase()
    ))
    .facts(vec![
        Fact::new("Task", title),
        Fact::new("Summary", summary),
        Fact::new("Waiting since", waiting_since),
    ])
    .cta("Open the task", links.task(task))
}

#[cfg(test)]
mod tests {
    use time::macros::date;

    use super::*;

    fn order() -> OrderFacts {
        OrderFacts {
            number: OrderNumber::new(1042),
            customer_name: Some("Ada Lovelace".into()),
            items: vec![ItemLine {
                name: "Voltaris V5 8600".into(),
                sku: "CPU-VOL-8600".into(),
                quantity: 1,
                amount: "€254.00".into(),
            }],
            items_total: Money::from_euros(254),
            shipping: Money::ZERO,
            total: Money::from_euros(254),
            vat_rate: VatRate::DEFAULT,
            ship_to: vec!["Ada Lovelace".into(), "Fő utca 1".into()],
            store_paid: false,
        }
    }

    #[test]
    fn confirmation_has_items_and_invoice() {
        let links = Links::new("http://localhost:8787/");
        let c = order_email(
            &order(),
            &OrderTemplate::OrderConfirmation {
                invoice: Some(InvoiceFacts {
                    number: InvoiceNumber::new(2026, 42),
                    issue_date: date!(2026 - 10 - 16),
                    gross: Money::from_euros(254),
                    vat_rate: VatRate::DEFAULT,
                }),
            },
            &links,
        );
        assert_eq!(c.subject, "Order TD-1042 confirmed");
        assert_eq!(c.template, "order_confirmation");
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("Hi Ada Lovelace,"));
        assert!(json.contains("TD-2026-000042"));
        assert!(json.contains("€54.00"), "VAT of €254 at 27%");
        assert!(json.contains("Fri, 16 Oct 2026"));
        assert_eq!(
            c.cta.unwrap().url,
            "http://localhost:8787/account/orders/TD-1042"
        );
    }

    #[test]
    fn every_template_renders() {
        let links = Links::new("http://x");
        let shipment = ShipmentFacts {
            carrier: "Kestrel Parcel".into(),
            tracking_number: TrackingNumber::new("KP0000000001"),
            shipped_on: date!(2026 - 10 - 16),
            eta: date!(2026 - 10 - 19),
        };
        let frame = super::super::content::Frame {
            support_address: "support+k3x9q2ab@techdepo.dev".into(),
            footer: "f".into(),
        };
        let n = OrderNumber::new(1042);
        let orders = [
            OrderTemplate::OrderConfirmation { invoice: None },
            OrderTemplate::PaymentReceived {
                amount: Money::from_euros(1),
            },
            OrderTemplate::Shipped {
                shipment: shipment.clone(),
            },
            OrderTemplate::Delivered {
                on: date!(2026 - 10 - 19),
            },
            OrderTemplate::Cancelled {
                reason: Some("changed my mind".into()),
            },
            OrderTemplate::RefundStarted {
                amount: Money::from_euros(1),
            },
            OrderTemplate::RefundDone {
                amount: Money::from_euros(1),
                full: false,
                credit_note: None,
            },
            OrderTemplate::RefundRejected {
                amount: Money::from_euros(1),
                reason: "no".into(),
            },
            OrderTemplate::InvoiceReissued {
                voided: InvoiceNumber::new(2026, 1),
                credit_note: InvoiceNumber::new(2026, 2),
                reissued: InvoiceNumber::new(2026, 3),
                reason: "address".into(),
            },
        ];
        for t in &orders {
            let out =
                super::super::content::render(&order_email(&order(), t, &links), &frame).unwrap();
            assert!(out.text.contains("TD-1042"), "{}", t.name());
        }
        let ticket = TicketFacts {
            ticket_id: TicketId::parse("T-7F2M9Q").unwrap(),
            subject: "Re: RE: where is my order? [#T-7F2M9Q]".into(),
            customer_name: None,
        };
        let supports = [
            SupportTemplate::TicketReceived,
            SupportTemplate::AskOrderNumber,
            SupportTemplate::Cancel {
                order_number: n,
                outcome: CancelOutcome::Cancelled,
            },
            SupportTemplate::Cancel {
                order_number: n,
                outcome: CancelOutcome::RefundRequested {
                    amount: Money::from_euros(9),
                },
            },
            SupportTemplate::Cancel {
                order_number: n,
                outcome: CancelOutcome::AlreadyShipped,
            },
            SupportTemplate::Cancel {
                order_number: n,
                outcome: CancelOutcome::ReturnRequested,
            },
            SupportTemplate::Cancel {
                order_number: n,
                outcome: CancelOutcome::ReturnWindowClosed {
                    delivered_on: date!(2026 - 09 - 01),
                },
            },
            SupportTemplate::Cancel {
                order_number: n,
                outcome: CancelOutcome::ReturnUnderWay,
            },
            SupportTemplate::Cancel {
                order_number: n,
                outcome: CancelOutcome::AlreadyClosed,
            },
            SupportTemplate::Wismo {
                order_number: n,
                answer: WismoAnswer::AwaitingPayment,
                inferred: false,
            },
            SupportTemplate::Wismo {
                order_number: n,
                answer: WismoAnswer::BeingPrepared,
                inferred: false,
            },
            SupportTemplate::Wismo {
                order_number: n,
                answer: WismoAnswer::Delivered {
                    on: date!(2026 - 10 - 19),
                },
                inferred: true,
            },
            SupportTemplate::Wismo {
                order_number: n,
                answer: WismoAnswer::WithinEstimate {
                    shipment: shipment.clone(),
                },
                inferred: false,
            },
            SupportTemplate::Wismo {
                order_number: n,
                answer: WismoAnswer::CarrierOverloaded { shipment },
                inferred: false,
            },
            SupportTemplate::InvestigationOpened { order_number: n },
            SupportTemplate::ParcelFound {
                order_number: n,
                eta: date!(2026 - 10 - 20),
            },
            SupportTemplate::Offer {
                order_number: n,
                kind: OfferKind::Both,
                refund_amount: Some(Money::from_euros(9)),
            },
            SupportTemplate::RefundDone {
                order_number: n,
                amount: Money::from_euros(9),
                full: false,
                credit_note: Some(InvoiceNumber::new(2026, 2)),
            },
            SupportTemplate::RefundRejected {
                order_number: n,
                amount: Money::from_euros(9),
                reason: "Outside the return window".into(),
            },
            SupportTemplate::Reminder {
                waiting_for: "your order number".into(),
            },
            SupportTemplate::Closed,
            SupportTemplate::Escalated,
            SupportTemplate::Reply {
                body: "Hello.\n\nBye.".into(),
            },
        ];
        for t in &supports {
            let c = support_email(&ticket, t, &links);
            assert_eq!(
                c.subject,
                "Re: where is my order? [#T-7F2M9Q]",
                "{}",
                t.name()
            );
            super::super::content::render(&c, &frame).unwrap();
        }
    }

    #[test]
    fn support_copy() {
        let links = Links::new("http://x");
        let ticket = TicketFacts {
            ticket_id: TicketId::parse("T-7F2M9Q").unwrap(),
            subject: "Help".into(),
            customer_name: None,
        };
        let text = |t: &SupportTemplate| {
            serde_json::to_string(&support_email(&ticket, t, &links)).unwrap()
        };
        let ask = text(&SupportTemplate::AskOrderNumber);
        assert!(ask.contains("http://x/account/orders"), "{ask}");
        let foreign = text(&SupportTemplate::OrderNotFound {
            numbers: vec![
                OrderNumber::new(1),
                OrderNumber::new(2),
                OrderNumber::new(3),
            ],
        });
        assert!(foreign.contains("TD-1, TD-2 and TD-3"), "{foreign}");
        let inferred = text(&SupportTemplate::Wismo {
            order_number: OrderNumber::new(1042),
            answer: WismoAnswer::BeingPrepared,
            inferred: true,
        });
        assert!(inferred.contains("we looked up TD-1042"), "{inferred}");
    }

    #[test]
    fn subjects_are_threaded_once() {
        let t = TicketId::parse("T-7F2M9Q").unwrap();
        assert_eq!(
            ticket_subject("Order help", &t),
            "Re: Order help [#T-7F2M9Q]"
        );
        assert_eq!(ticket_subject("  ", &t), "Re: Your question [#T-7F2M9Q]");
    }
}
