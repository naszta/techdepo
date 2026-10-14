//! Enums whose wire names are stored in D1 (and sent over the API), apart from the ones that
//! live in `techdepo-domain` (order status, intent, ticket status, actor, assignee, channel,
//! invoice kind and status, lost outcome) and `techdepo-catalog` (category).
//!
//! The migrations' `CHECK (col IN (…))` lists mirror these. The test
//! `migration_checks_match_enums` fails when they drift, so adding a variant means adding a
//! migration that rebuilds the table (see `migrations/README.md`).

use techdepo_domain::labeled_enum;

labeled_enum! {
    /// A member's role in the org (Clerk role keys). `org_members.role`.
    pub enum OrgRole {
        /// May use `/admin` and the admin APIs.
        Admin => ("org:admin", "Admin"),
        /// A customer.
        Member => ("org:member", "Member"),
    }
}

impl OrgRole {
    /// Parses a Clerk role key. Unknown custom roles are treated as plain members.
    pub fn from_clerk(role: &str) -> Self {
        role.parse().unwrap_or(Self::Member)
    }

    /// Whether this role may use the admin APIs.
    pub const fn is_admin(self) -> bool {
        matches!(self, Self::Admin)
    }
}

labeled_enum! {
    /// Where an `org_members` row came from.
    pub enum MemberSource {
        /// Synced from Clerk session claims or the Backend API.
        Clerk => ("clerk", "Clerk"),
        /// Created by the dev login (`AUTH_MODE=dev`).
        Dev => ("dev", "Dev login"),
    }
}

labeled_enum! {
    /// How an order is paid. `orders.payment_method`; `payments.method` takes only the
    /// [`PaymentMethod::CUSTOMER`] ones.
    pub enum PaymentMethod {
        /// Card (captured immediately when "Simulate payment" runs).
        Card => ("card", "Card"),
        /// Bank transfer.
        BankTransfer => ("bank_transfer", "Bank transfer"),
        /// Paid by the store: replacement orders (`replacement_for` set, all prices 0). Never
        /// chosen at checkout and never on a payment.
        Store => ("store", "Paid by the store"),
    }
}

impl PaymentMethod {
    /// The methods a customer can choose at checkout, and the only ones a payment carries.
    pub const CUSTOMER: &[Self] = &[Self::Card, Self::BankTransfer];
}

labeled_enum! {
    /// Direction of a payment ledger entry. Amounts are always positive.
    pub enum PaymentKind {
        /// Money taken from the customer.
        Charge => ("charge", "Charge"),
        /// Money returned to the customer.
        Refund => ("refund", "Refund"),
    }
}

labeled_enum! {
    /// Outcome of a payment ledger entry.
    pub enum PaymentStatus {
        /// Captured (charge) or paid out (refund).
        Succeeded => ("succeeded", "Succeeded"),
        /// Declined or failed; `failure_reason` says why.
        Failed => ("failed", "Failed"),
    }
}

labeled_enum! {
    /// What an `order_events` row records.
    pub enum OrderEventKind {
        /// The order was placed.
        Placed => ("placed", "Order placed"),
        /// A charge was captured.
        PaymentCaptured => ("payment_captured", "Payment captured"),
        /// A charge was declined.
        PaymentDeclined => ("payment_declined", "Payment declined"),
        /// Processing started.
        ProcessingStarted => ("processing_started", "Processing started"),
        /// An email to the customer was sent.
        EmailSent => ("email_sent", "Email sent"),
        /// An invoice was issued.
        InvoiceIssued => ("invoice_issued", "Invoice issued"),
        /// An invoice was voided.
        InvoiceVoided => ("invoice_voided", "Invoice voided"),
        /// A credit note was issued.
        CreditNoteIssued => ("credit_note_issued", "Credit note issued"),
        /// Handed to the carrier.
        Shipped => ("shipped", "Shipped"),
        /// A carrier scan or status change.
        CarrierUpdate => ("carrier_update", "Carrier update"),
        /// Delivered.
        Delivered => ("delivered", "Delivered"),
        /// Cancelled.
        Cancelled => ("cancelled", "Cancelled"),
        /// The customer asked to return the order.
        ReturnRequested => ("return_requested", "Return requested"),
        /// The parcel came back.
        Returned => ("returned", "Returned"),
        /// A refund was requested.
        RefundRequested => ("refund_requested", "Refund requested"),
        /// A person approved a refund.
        RefundApproved => ("refund_approved", "Refund approved"),
        /// A person rejected a refund.
        RefundRejected => ("refund_rejected", "Refund rejected"),
        /// Money was returned (full or partial; `status_after` says whether the order is now
        /// `refunded`).
        Refunded => ("refunded", "Refunded"),
        /// A carrier investigation was opened.
        InvestigationOpened => ("investigation_opened", "Investigation opened"),
        /// A carrier investigation was closed.
        InvestigationClosed => ("investigation_closed", "Investigation closed"),
        /// A replacement order was created for this one (or this one is a replacement).
        ReplacementCreated => ("replacement_created", "Replacement created"),
        /// A support ticket was linked to this order.
        TicketLinked => ("ticket_linked", "Ticket linked"),
        /// Demo: the fault toggles changed.
        FaultsChanged => ("faults_changed", "Demo faults changed"),
        /// Demo: an armed fault fired.
        FaultTriggered => ("fault_triggered", "Demo fault triggered"),
        /// Demo: the order's simulated clock moved forward.
        ClockAdvanced => ("clock_advanced", "Simulated clock advanced"),
        /// Demo: the ship date was moved back ("ship date to last Friday").
        ShipDateChanged => ("ship_date_changed", "Ship date changed"),
        /// A free-text note from staff or the system.
        Note => ("note", "Note"),
    }
}

impl OrderEventKind {
    /// Whether the customer's order timeline shows this event. Fault events would leak the
    /// hidden truth ("the parcel is lost"), and notes are internal; the demo drawer and the
    /// admin timeline show everything.
    pub const fn is_customer_visible(self) -> bool {
        !matches!(
            self,
            Self::FaultTriggered | Self::FaultsChanged | Self::Note
        )
    }

    /// Whether this is a demo-control event rather than something a real shop would record.
    pub const fn is_demo(self) -> bool {
        matches!(
            self,
            Self::FaultsChanged
                | Self::FaultTriggered
                | Self::ClockAdvanced
                | Self::ShipDateChanged
        )
    }
}

labeled_enum! {
    /// The kind of record an [`crate::EntityRef`] (or `order_events.ref_kind`) points at.
    pub enum RefKind {
        /// An order.
        Order => ("order", "Order"),
        /// A payment ledger entry.
        Payment => ("payment", "Payment"),
        /// An invoice or credit note.
        Invoice => ("invoice", "Invoice"),
        /// A shipment.
        Shipment => ("shipment", "Shipment"),
        /// A support ticket.
        Ticket => ("ticket", "Ticket"),
        /// A refund.
        Refund => ("refund", "Refund"),
        /// A carrier investigation.
        Investigation => ("investigation", "Investigation"),
        /// A human task.
        HumanTask => ("human_task", "Task"),
        /// An `email_log` entry.
        Email => ("email", "Email"),
        /// An agent or classifier trace.
        AgentRun => ("agent_run", "AI trace"),
    }
}

labeled_enum! {
    /// What the customer sees about a shipment. A lost parcel stays `in_transit`.
    pub enum ShipmentStatus {
        /// With the carrier.
        InTransit => ("in_transit", "In transit"),
        /// Delivered.
        Delivered => ("delivered", "Delivered"),
    }
}

labeled_enum! {
    /// State of a carrier trace (hidden truth, admin and demo controls only).
    pub enum TraceStatus {
        /// Requested; the carrier is looking.
        Open => ("open", "Trace open"),
        /// Found: delivery tomorrow.
        Resurfaced => ("resurfaced", "Found"),
        /// The carrier has no information.
        NoInformation => ("no_information", "No information"),
    }
}

labeled_enum! {
    /// A carrier scan event.
    pub enum CarrierEventKind {
        /// Label printed.
        LabelCreated => ("label_created", "Label created"),
        /// Collected from the shop.
        PickedUp => ("picked_up", "Picked up"),
        /// Moving through the network.
        InTransit => ("in_transit", "In transit"),
        /// On the van.
        OutForDelivery => ("out_for_delivery", "Out for delivery"),
        /// Delivered.
        Delivered => ("delivered", "Delivered"),
        /// Delayed.
        Delayed => ("delayed", "Delayed"),
        /// A trace was opened (internal).
        TraceOpened => ("trace_opened", "Trace opened"),
        /// The trace found the parcel (internal).
        TraceFound => ("trace_found", "Trace: found"),
        /// The trace found nothing (internal).
        TraceNoInformation => ("trace_no_information", "Trace: no information"),
    }
}

labeled_enum! {
    /// Where the router sent a ticket. `tickets.route`.
    pub enum TicketRoute {
        /// `CancelOrder` workflow.
        CancelOrder => ("cancel_order", "Cancel order (workflow)"),
        /// `WhereIsMyOrder` workflow and shipping policy.
        WhereIsMyOrder => ("where_is_my_order", "Where is my order (workflow)"),
        /// `BillingIssue` workflow and detectors.
        BillingIssue => ("billing_issue", "Billing issue (workflow)"),
        /// `RefundRequest` workflow.
        RefundRequest => ("refund_request", "Refund request (workflow)"),
        /// `ProductAdvisor` agent.
        ProductAdvisor => ("product_advisor", "Product advisor (agent)"),
        /// `OrderAssistant` agent.
        OrderAssistant => ("order_assistant", "Order assistant (agent)"),
        /// A person.
        Human => ("human", "Human"),
    }
}

impl TicketRoute {
    /// The route for an intent (support.md §3). `other` goes to a person.
    pub const fn for_intent(intent: techdepo_domain::Intent) -> Self {
        use techdepo_domain::Intent;
        match intent {
            Intent::CancelOrder => Self::CancelOrder,
            Intent::OrderStatus => Self::WhereIsMyOrder,
            Intent::BillingProblem => Self::BillingIssue,
            Intent::RefundOrReturn => Self::RefundRequest,
            Intent::ProductAdvice => Self::ProductAdvisor,
            Intent::OrderQuestion => Self::OrderAssistant,
            Intent::Other => Self::Human,
        }
    }

    /// Whether the route is an LLM agent (as opposed to a code workflow or a person).
    pub const fn is_agent(self) -> bool {
        matches!(self, Self::ProductAdvisor | Self::OrderAssistant)
    }
}

labeled_enum! {
    /// What a ticket waiting on the customer expects next. `tickets.awaiting`.
    pub enum Awaiting {
        /// An order number (the route needs one to act).
        OrderNumber => ("order_number", "Order number"),
        /// A reply to a refund/reship offer.
        OfferReply => ("offer_reply", "Reply to offer"),
        /// A clarification of an unclear reply or question.
        Clarification => ("clarification", "Clarification"),
    }
}

labeled_enum! {
    /// Direction of a ticket message or email.
    pub enum Direction {
        /// From the customer.
        Inbound => ("inbound", "Inbound"),
        /// To the customer.
        Outbound => ("outbound", "Outbound"),
    }
}

labeled_enum! {
    /// State of a ticket message. Customers only see `received` and `sent`.
    pub enum MessageStatus {
        /// An inbound message.
        Received => ("received", "Received"),
        /// An AI draft awaiting review.
        Draft => ("draft", "Draft"),
        /// Sent to the customer.
        Sent => ("sent", "Sent"),
        /// A draft a person discarded.
        Discarded => ("discarded", "Discarded"),
    }
}

impl MessageStatus {
    /// Whether the customer sees messages in this state.
    pub const fn is_customer_visible(self) -> bool {
        matches!(self, Self::Received | Self::Sent)
    }
}

labeled_enum! {
    /// State of a carrier investigation (mirrors the `Investigation` workflow).
    pub enum InvestigationStatus {
        /// Created.
        Open => ("open", "Open"),
        /// Waiting for the carrier's trace.
        Tracing => ("tracing", "Tracing"),
        /// Waiting for a person to choose the offer.
        AwaitingDecision => ("awaiting_decision", "Awaiting decision"),
        /// Waiting for the customer to answer the offer.
        AwaitingCustomer => ("awaiting_customer", "Awaiting customer"),
        /// Closed with an outcome.
        Resolved => ("resolved", "Resolved"),
        /// Closed by handing it to a person.
        Escalated => ("escalated", "Escalated"),
    }
}

impl InvestigationStatus {
    /// Whether the investigation is still running.
    pub const fn is_open(self) -> bool {
        !matches!(self, Self::Resolved | Self::Escalated)
    }
}

labeled_enum! {
    /// What a person decided to offer after a "no information" trace.
    pub enum InvestigationDecision {
        /// Offer a refund.
        OfferRefund => ("offer_refund", "Offer refund"),
        /// Offer a replacement.
        OfferReship => ("offer_reship", "Offer replacement"),
        /// Let the customer choose.
        OfferBoth => ("offer_both", "Offer both"),
        /// Write a custom reply instead.
        CustomReply => ("custom_reply", "Custom reply"),
    }
}

labeled_enum! {
    /// How an investigation ended.
    pub enum InvestigationOutcome {
        /// The carrier found the parcel.
        Resurfaced => ("resurfaced", "Parcel found"),
        /// The customer accepted a refund.
        Refunded => ("refunded", "Refunded"),
        /// The customer accepted a replacement.
        Reshipped => ("reshipped", "Replacement shipped"),
        /// The customer declined every offer.
        Declined => ("declined", "Offers declined"),
        /// Handed to a person (unclear replies, no reply).
        Escalated => ("escalated", "Escalated"),
        /// A person answered with a custom reply.
        CustomReply => ("custom_reply", "Custom reply"),
    }
}

labeled_enum! {
    /// Why a refund was requested.
    pub enum RefundReason {
        /// Cancelling a paid, unshipped order.
        Cancellation => ("cancellation", "Cancellation"),
        /// Reversing a duplicate charge.
        DuplicateCharge => ("duplicate_charge", "Duplicate charge"),
        /// The parcel was returned.
        Return => ("return", "Return"),
        /// The parcel was lost.
        LostParcel => ("lost_parcel", "Lost parcel"),
        /// Goodwill gesture.
        Goodwill => ("goodwill", "Goodwill"),
        /// Anything else (`reason_text` says what).
        Other => ("other", "Other"),
    }
}

impl RefundReason {
    /// Whether refunding an invoiced order for this reason issues a credit note. A duplicate
    /// charge was never invoiced (the invoice covers the order once), so crediting the invoice
    /// would wrongly reverse part of a valid sale.
    pub const fn credits_invoice(self) -> bool {
        !matches!(self, Self::DuplicateCharge)
    }
}

labeled_enum! {
    /// State of a refund (mirrors `RefundWorkflow`).
    pub enum RefundStatus {
        /// Waiting for approval.
        Requested => ("requested", "Requested"),
        /// Approved; the money is being returned.
        Approved => ("approved", "Approved"),
        /// Rejected by a person.
        Rejected => ("rejected", "Rejected"),
        /// Money returned.
        Completed => ("completed", "Completed"),
    }
}

labeled_enum! {
    /// The kind of decision a human task asks for.
    pub enum HumanTaskKind {
        /// Approve or reject a refund.
        ApproveRefund => ("approve_refund", "Refund approval"),
        /// Choose what to offer after a "no information" trace.
        InvestigationDecision => ("investigation_decision", "Investigation decision"),
        /// Review an AI-authored reply before it is sent.
        ReviewReply => ("review_reply", "Reply review"),
        /// A ticket escalated to a person.
        Escalation => ("escalation", "Escalation"),
        /// The customer declined every offer.
        OffersDeclined => ("offers_declined", "Offers declined"),
    }
}

impl HumanTaskKind {
    /// Whether a workflow waits on the task's awakeable: `RefundWorkflow` for a refund approval,
    /// `Investigation` for its offer. Only these rows have `human_tasks.awakeable_id`.
    ///
    /// The other kinds belong to a ticket (`SupportTicket` creates them and never waits), so
    /// `HumanTasks/decide` hands their decision to `SupportTicket/task_decided` instead.
    pub const fn has_waiter(self) -> bool {
        matches!(self, Self::ApproveRefund | Self::InvestigationDecision)
    }

    /// The decisions a task of this kind offers.
    pub const fn allowed_decisions(self) -> &'static [DecisionKind] {
        use DecisionKind as D;
        match self {
            Self::ApproveRefund => &[D::Approve, D::Reject],
            Self::InvestigationDecision => {
                &[D::OfferRefund, D::OfferReship, D::OfferBoth, D::CustomReply]
            }
            Self::ReviewReply => &[D::Send, D::EditAndSend, D::Discard],
            Self::Escalation => &[D::Reply, D::HandBack, D::Close],
            Self::OffersDeclined => &[D::Reply, D::Close],
        }
    }
}

labeled_enum! {
    /// State of a human task.
    pub enum HumanTaskStatus {
        /// Waiting for a person.
        Pending => ("pending", "Pending"),
        /// Decided.
        Decided => ("decided", "Decided"),
        /// No longer needed (e.g. the ticket was closed or the demo reset).
        Cancelled => ("cancelled", "Cancelled"),
    }
}

labeled_enum! {
    /// A decision on a human task. [`crate::tasks::HumanDecision`] carries the details.
    pub enum DecisionKind {
        /// Approve the refund.
        Approve => ("approve", "Approve"),
        /// Reject the refund (with a reason).
        Reject => ("reject", "Reject"),
        /// Offer a refund.
        OfferRefund => ("offer_refund", "Offer refund"),
        /// Offer a replacement.
        OfferReship => ("offer_reship", "Offer replacement"),
        /// Offer both.
        OfferBoth => ("offer_both", "Offer both"),
        /// Send a custom reply instead of an offer.
        CustomReply => ("custom_reply", "Custom reply"),
        /// Send the AI draft as is.
        Send => ("send", "Send"),
        /// Send an edited version of the draft.
        EditAndSend => ("edit_and_send", "Edit and send"),
        /// Discard the draft and take over the ticket.
        Discard => ("discard", "Discard and take over"),
        /// Reply to the customer (the ticket stays with a person).
        Reply => ("reply", "Reply"),
        /// Hand the ticket back to the AI.
        HandBack => ("hand_back", "Hand back to AI"),
        /// Close the ticket.
        Close => ("close", "Close"),
    }
}

labeled_enum! {
    /// Outcome of an `email_log` entry.
    pub enum EmailStatus {
        /// Inbound and passed every check.
        Accepted => ("accepted", "Accepted"),
        /// Inbound and dropped; `reject_reason` says why.
        Rejected => ("rejected", "Rejected"),
        /// Outbound, not yet confirmed by the provider (retrying).
        Queued => ("queued", "Queued"),
        /// Outbound and accepted by the provider.
        Sent => ("sent", "Sent"),
        /// Outbound and permanently refused.
        Failed => ("failed", "Failed"),
    }
}

labeled_enum! {
    /// Why mail-in dropped an inbound email (support.md §1). No reply is sent (no backscatter).
    pub enum RejectReason {
        /// `Auto-Submitted` or `Precedence: bulk/list`.
        AutoReply => ("auto_reply", "Auto-reply"),
        /// Sent from our own domain (loop protection).
        OwnDomain => ("own_domain", "From our own domain"),
        /// DMARC failed.
        DmarcFail => ("dmarc_fail", "DMARC failed"),
        /// Too large.
        Oversized => ("oversized", "Too large"),
        /// Unparseable MIME.
        Malformed => ("malformed", "Malformed"),
        /// The `+tag` names no org.
        UnknownOrg => ("unknown_org", "Unknown shop"),
        /// The sender is not a member of the org.
        NotMember => ("not_member", "Sender is not a member"),
        /// The org's inbound rate limit was hit.
        RateLimited => ("rate_limited", "Rate limited"),
        /// Already received (same Message-ID).
        Duplicate => ("duplicate", "Duplicate"),
    }
}

labeled_enum! {
    /// Which LLM-backed component produced a trace. `agent_runs.agent`.
    pub enum AgentKind {
        /// Ticket classification.
        Router => ("router", "Router (classifier)"),
        /// The `ProductAdvisor` agent.
        ProductAdvisor => ("product_advisor", "Product advisor"),
        /// The `OrderAssistant` agent.
        OrderAssistant => ("order_assistant", "Order assistant"),
        /// The reply-confidence judge.
        Judge => ("judge", "Reply judge"),
        /// Interpreting a customer's reply to an offer.
        ReplyInterpreter => ("reply_interpreter", "Reply interpreter"),
    }
}

labeled_enum! {
    /// State of a trace.
    pub enum AgentRunStatus {
        /// In progress.
        Running => ("running", "Running"),
        /// Finished with an outcome.
        Completed => ("completed", "Completed"),
        /// Failed (LLM or tool error after retries, budget exhausted).
        Failed => ("failed", "Failed"),
    }
}

labeled_enum! {
    /// The kind of outcome an agent returned. [`crate::support::AgentOutcome`] carries the
    /// details.
    pub enum AgentOutcomeKind {
        /// A reply for the customer.
        Reply => ("reply", "Reply"),
        /// A question for the customer.
        AskCustomer => ("ask_customer", "Ask customer"),
        /// Hand to a person.
        Escalate => ("escalate", "Escalate"),
        /// Send to a different route.
        Reroute => ("reroute", "Reroute"),
    }
}

labeled_enum! {
    /// One step of a trace.
    pub enum AgentStepKind {
        /// A chat completion.
        LlmCall => ("llm_call", "LLM call"),
        /// A tool call.
        ToolCall => ("tool_call", "Tool call"),
        /// A classifier pass (Clef).
        Classification => ("classification", "Classification"),
        /// A guardrail check.
        Guardrail => ("guardrail", "Guardrail"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use techdepo_catalog::Category;
    use techdepo_domain::{
        Actor, Assignee, Channel, Intent, InvoiceKind, InvoiceStatus, LostOutcome, OrderStatus,
        TicketStatus,
    };

    use super::*;

    const MIGRATIONS: &[&str] = &[include_str!("../../../migrations/0001_init.sql")];

    /// The quoted values of `CHECK (<column> IN (…))` on `table.column`, from the last
    /// migration that defines the table.
    fn check_values(table: &str, column: &str) -> BTreeSet<String> {
        let header = format!("CREATE TABLE {table} (");
        let body = MIGRATIONS
            .iter()
            .rev()
            .find_map(|sql| {
                let start = sql.find(&header)? + header.len();
                let end = start + sql[start..].find(") STRICT;")?;
                Some(&sql[start..end])
            })
            .unwrap_or_else(|| panic!("table {table} not found"));
        let line = body
            .lines()
            .map(str::trim_start)
            .find(|l| {
                l.strip_prefix(column)
                    .is_some_and(|rest| rest.starts_with(char::is_whitespace))
            })
            .unwrap_or_else(|| panic!("column {table}.{column} not found"));
        let needle = format!("{column} IN (");
        let start = line
            .find(&needle)
            .unwrap_or_else(|| panic!("{table}.{column} has no IN check"))
            + needle.len();
        let end = start + line[start..].find(')').expect("closing parenthesis");
        line[start..end]
            .split(',')
            .map(|v| v.trim().trim_matches('\'').to_owned())
            .collect()
    }

    fn wire<T: Copy>(all: &[T], as_str: fn(T) -> &'static str) -> BTreeSet<String> {
        all.iter().map(|v| as_str(*v).to_owned()).collect()
    }

    macro_rules! assert_checks {
        ($($table:literal . $column:literal => $ty:ty;)+) => {$(
            assert_eq!(
                check_values($table, $column),
                wire(<$ty>::ALL, <$ty>::as_str),
                "{}.{} vs {}",
                $table,
                $column,
                stringify!($ty),
            );
        )+};
    }

    #[test]
    fn migration_checks_match_enums() {
        assert_checks! {
            "org_members"."role" => OrgRole;
            "org_members"."source" => MemberSource;
            "orders"."status" => OrderStatus;
            "orders"."payment_method" => PaymentMethod;
            "order_items"."category" => Category;
            "order_events"."kind" => OrderEventKind;
            "order_events"."actor" => Actor;
            "order_events"."status_after" => OrderStatus;
            "order_events"."ref_kind" => RefKind;
            "payments"."kind" => PaymentKind;
            "payments"."status" => PaymentStatus;
            "invoices"."kind" => InvoiceKind;
            "invoices"."status" => InvoiceStatus;
            "shipments"."status" => ShipmentStatus;
            "shipments"."trace_status" => TraceStatus;
            "carrier_events"."kind" => CarrierEventKind;
            "tickets"."channel" => Channel;
            "tickets"."status" => TicketStatus;
            "tickets"."assignee" => Assignee;
            "tickets"."intent" => Intent;
            "tickets"."route" => TicketRoute;
            "tickets"."awaiting" => Awaiting;
            "ticket_messages"."direction" => Direction;
            "ticket_messages"."author" => Actor;
            "ticket_messages"."channel" => Channel;
            "ticket_messages"."status" => MessageStatus;
            "investigations"."status" => InvestigationStatus;
            "investigations"."decision" => InvestigationDecision;
            "investigations"."outcome" => InvestigationOutcome;
            "investigations"."opened_by" => Actor;
            "refunds"."reason" => RefundReason;
            "refunds"."status" => RefundStatus;
            "human_tasks"."kind" => HumanTaskKind;
            "human_tasks"."status" => HumanTaskStatus;
            "human_tasks"."decision" => DecisionKind;
            "email_log"."direction" => Direction;
            "email_log"."channel" => Channel;
            "email_log"."status" => EmailStatus;
            "email_log"."reject_reason" => RejectReason;
            "agent_runs"."agent" => AgentKind;
            "agent_runs"."status" => AgentRunStatus;
            "agent_runs"."outcome" => AgentOutcomeKind;
            "agent_steps"."kind" => AgentStepKind;
        }
        assert_eq!(
            check_values("payments", "method"),
            wire(PaymentMethod::CUSTOMER, PaymentMethod::as_str),
        );
        // LostOutcome is a plain serde enum in the domain crate, not a labeled enum.
        let lost: BTreeSet<String> = [LostOutcome::Resurfaces, LostOutcome::Gone]
            .iter()
            .map(|o| {
                serde_json::to_value(o)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(check_values("shipments", "lost_outcome"), lost);
        assert_eq!(check_values("investigations", "carrier_result"), lost);
    }

    #[test]
    fn awakeable_only_for_kinds_with_a_waiter() {
        let waiting: Vec<String> = HumanTaskKind::ALL
            .iter()
            .filter(|k| k.has_waiter())
            .map(|k| format!("'{}'", k.as_str()))
            .collect();
        let check = format!(
            "CHECK ((awakeable_id IS NOT NULL) = (kind IN ({})))",
            waiting.join(", ")
        );
        assert!(MIGRATIONS.iter().any(|m| m.contains(&check)), "{check}");
    }

    #[test]
    fn every_task_kind_offers_decisions() {
        for kind in HumanTaskKind::ALL {
            assert!(!kind.allowed_decisions().is_empty(), "{kind:?}");
        }
        for intent in Intent::ALL {
            let route = TicketRoute::for_intent(*intent);
            assert_eq!(route == TicketRoute::Human, *intent == Intent::Other);
        }
        assert_eq!(OrgRole::from_clerk("org:admin"), OrgRole::Admin);
        assert_eq!(OrgRole::from_clerk("org:billing"), OrgRole::Member);
    }
}
