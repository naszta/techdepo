//! `Order` (virtual object, keyed by order ID): the order state machine.
//!
//! All mutations of an order go through this object, so they are serialized per order. Its
//! Restate state ([`OrderState`]) is the source of truth for the status, the timeline's next
//! `seq`, the armed faults and the virtual clock; every change is mirrored into the D1 read
//! model and appended to `order_events` in one batch inside a named `ctx.run`
//! (`record-<event kind>`). Nobody else writes `order_events`: other components call
//! `record_event`.
//!
//! The transitions are `techdepo_domain::OrderStatus::apply`; an action that is not allowed
//! now is a terminal `409 invalid_transition`. Automatic steps follow the manual ones: payment
//! starts processing and issues the invoice; advancing the clock past the carrier's ETA
//! delivers (unless the parcel is lost). A fault fires on the step it is armed for and is then
//! disarmed.

use std::sync::Arc;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::demo::ClockStep;
use techdepo_api_types::ingress::{
    AdvanceClockInput, CancelOrderInput, ClockAdvanced, DemoActionInput, PlacedOrder,
    SetFaultsInput, StartRefundInput, derive_id,
};
use techdepo_api_types::orders::InvoiceRef;
use techdepo_api_types::vocab::{OrderEventKind, PaymentMethod, RefKind, RefundReason};
use techdepo_db::OrgScope;
use techdepo_db::inventory::is_oversell;
use techdepo_db::orders::{NewOrder, NewOrderEvent, placed_number};
use techdepo_domain::{
    Actor, CarrierFault, ClockAdvance, EmailFault, FaultToggles, InvoiceFault, LostOutcome,
    MAX_EMAIL_OUTAGE_MINUTES, Money, OrderAction, OrderId, OrderNumber, OrderStatus, OrgId,
    PaymentFault, RefundId, ShipmentFault, UserId, VirtualClock, WorkCalendar, previous_weekday,
    within_return_window,
};
use time::{Date, Duration, OffsetDateTime, Weekday};

use crate::Deps;
use crate::carrier::{
    CarrierClient, CreateShipmentInput, MoveShipDateInput, ShipmentInfo, TrackInput, TrackResult,
};
use crate::errors::{internal, invalid, invalid_transition, not_found, not_now, reject, sql};
use crate::invoicing::{InvoicingClient, IssueInvoiceInput, IssuedInvoice};
use crate::notifications::{NotificationsClient, OrderNotice, OrderNoticeInput};
use crate::payments::{CaptureInput, CaptureOutcome, PaymentsClient};
use crate::refund::RefundWorkflowClient;
use crate::util;

/// Restate state key of [`OrderState`].
const STATE: &str = "order";

/// How long the "carrier API down" fault keeps the carrier failing (real time).
pub const CARRIER_OUTAGE: Duration = Duration::seconds(20);

/// The order as its object knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderState {
    /// Org.
    pub org_id: OrgId,
    /// Customer.
    pub user_id: UserId,
    /// Order number.
    pub number: OrderNumber,
    /// Status.
    pub status: OrderStatus,
    /// Payment method.
    pub payment_method: PaymentMethod,
    /// Total.
    pub total: Money,
    /// `seq` of the next timeline entry.
    pub next_seq: u32,
    /// Faults armed for the next steps.
    pub faults: FaultToggles,
    /// Simulated clock.
    pub clock: VirtualClock,
    /// Money refunded so far.
    pub refunded: Money,
    /// The current invoice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invoice: Option<InvoiceRef>,
    /// The shipment, once shipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shipment: Option<ShipmentInfo>,
    /// Delivery date (simulated).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "techdepo_api_types::serde_fmt::date::option"
    )]
    pub delivered_on: Option<Date>,
    /// End of the simulated email provider outage (real time), once the fault fired.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub email_outage_until: Option<OffsetDateTime>,
}

/// `Order/place` input (internal: `Customer/checkout`, `Demo/create_scenario`, and later the
/// replacement orders of investigations).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceOrderInput {
    /// Org.
    pub org_id: OrgId,
    /// The order (its ID must be the object key).
    pub order: NewOrder,
    /// Faults to arm (the org's defaults, or a scenario's).
    pub faults: FaultToggles,
    /// Who placed it.
    pub actor: Actor,
    /// The person, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_user_id: Option<UserId>,
}

/// `Order/request_return` input (internal: support routes; the demo control is a demo action).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestReturnInput {
    /// Org.
    pub org_id: OrgId,
    /// Who asked.
    pub actor: Actor,
    /// The person, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<UserId>,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `Order/mark_refunded` input (internal: `RefundWorkflow`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkRefundedInput {
    /// Org.
    pub org_id: OrgId,
    /// The refund.
    pub refund_id: RefundId,
    /// Amount returned.
    pub amount: Money,
    /// Whether this refunds the order in full (moves it to `refunded`); a partial refund
    /// (a duplicate charge, goodwill) only records the event.
    pub full: bool,
    /// Who approved it.
    pub actor: Actor,
    /// The person, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<UserId>,
}

/// `Order/record_event` input (internal: anyone who must put something on the timeline).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordEventInput {
    /// Org.
    pub org_id: OrgId,
    /// Kind (never a status change: those go through the order's own handlers).
    pub kind: OrderEventKind,
    /// Who.
    pub actor: Actor,
    /// The person, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_user_id: Option<UserId>,
    /// One line.
    pub summary: String,
    /// Related record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<(RefKind, String)>,
    /// Details.
    #[serde(default)]
    pub data: serde_json::Value,
    /// Set the `refund_pending` flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refund_pending: Option<bool>,
    /// Set the `investigation_open` flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub investigation_open: Option<bool>,
    /// The caller's invocation (`ctx.invocation_id()`): the event's Restate UI deep link then
    /// opens the workflow or service that did it, not this `record_event` call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_id: Option<String>,
}

/// One change to commit: a timeline entry, maybe a new status and flags, plus extra statements
/// applied in the same batch.
struct Change {
    kind: OrderEventKind,
    actor: Actor,
    actor_user_id: Option<UserId>,
    summary: String,
    reference: Option<(RefKind, String)>,
    data: serde_json::Value,
    status: Option<OrderStatus>,
    refund_pending: Option<bool>,
    investigation_open: Option<bool>,
    /// The invocation to link instead of the object's own (`record_event` callers).
    invocation_id: Option<String>,
    extra: Vec<techdepo_db::Statement>,
}

impl Change {
    fn new(
        kind: OrderEventKind,
        actor: Actor,
        actor_user_id: Option<UserId>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            actor,
            actor_user_id,
            summary: summary.into(),
            reference: None,
            data: serde_json::Value::Null,
            status: None,
            refund_pending: None,
            investigation_open: None,
            invocation_id: None,
            extra: Vec::new(),
        }
    }

    fn status(mut self, status: OrderStatus) -> Self {
        self.status = Some(status);
        self
    }

    fn reference(mut self, kind: RefKind, id: impl Into<String>) -> Self {
        self.reference = Some((kind, id.into()));
        self
    }

    fn data(mut self, data: serde_json::Value) -> Self {
        self.data = data;
        self
    }

    fn extra(mut self, statement: techdepo_db::Statement) -> Self {
        self.extra.push(statement);
        self
    }
}

/// The label of a fired fault, for the timeline.
pub fn fault_label(faults: &FaultToggles, action: OrderAction) -> Option<String> {
    techdepo_db::views::armed_fault(action, faults)
}

/// An armed fault that fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fired {
    /// The payment fault (charge twice, declined).
    Payment,
    /// The invoice fault (missing billing address).
    Invoice,
    /// The shipping faults (lost parcel, carrier API down).
    Shipment,
    /// The email provider outage (with the confirmation email).
    Email,
}

/// The faults after one fired: each fires once, then is disarmed.
pub fn disarm(faults: &FaultToggles, fired: Fired) -> FaultToggles {
    let mut next = *faults;
    match fired {
        Fired::Payment => next.payment = PaymentFault::None,
        Fired::Invoice => next.invoice = InvoiceFault::None,
        Fired::Shipment => {
            next.shipment = ShipmentFault::None;
            next.carrier = CarrierFault::None;
        }
        Fired::Email => next.email = EmailFault::None,
    }
    next
}

/// The new ship date for "ship date to last Friday", or why not.
pub fn last_friday(shipped_on: Date, sim_today: Date) -> Result<Date, &'static str> {
    let friday = previous_weekday(sim_today, Weekday::Friday);
    if friday >= shipped_on {
        Err("The parcel was already shipped on or before last Friday")
    } else {
        Ok(friday)
    }
}

/// The `Order` virtual object.
pub struct Order {
    deps: Arc<Deps>,
}

impl Order {
    /// The object over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }

    async fn load(ctx: &ObjectContext<'_>, org: &OrgId) -> Result<OrderState, HandlerError> {
        match ctx.get::<Json<OrderState>>(STATE).await? {
            Some(state) if state.0.org_id == *org => Ok(state.into_inner()),
            _ => Err(not_found("order")),
        }
    }

    /// Writes `change` to D1 in one batch, then advances the object's state.
    async fn commit(
        &self,
        ctx: &ObjectContext<'_>,
        state: &mut OrderState,
        at: OffsetDateTime,
        change: Change,
    ) -> Result<(), HandlerError> {
        let order_id = OrderId::new(ctx.key());
        let event = NewOrderEvent {
            seq: state.next_seq,
            kind: change.kind,
            actor: change.actor,
            actor_user_id: change.actor_user_id,
            status_after: change.status,
            summary: change.summary,
            reference: change.reference,
            data: change.data,
            invocation_id: Some(
                change
                    .invocation_id
                    .unwrap_or_else(|| ctx.invocation_id().to_owned()),
            ),
            at,
            sim_at: state.clock.now(at),
        };
        let db = self.deps.db.clone();
        let scope = OrgScope::verified(state.org_id.clone());
        let (status, refund_pending, investigation_open, extra) = (
            change.status,
            change.refund_pending,
            change.investigation_open,
            change.extra,
        );
        ctx.run(|| async move {
            let orders = db.orders(&scope);
            let mut statements = extra;
            statements.push(match status {
                Some(status) => orders.status_statement(&order_id, status, at),
                None => orders.touch_statement(&order_id, at),
            });
            if refund_pending.is_some() || investigation_open.is_some() {
                statements.push(orders.flags_statement(
                    &order_id,
                    refund_pending,
                    investigation_open,
                    at,
                ));
            }
            statements.push(orders.event_statement(&order_id, &event).map_err(sql)?);
            db.batch(&statements).await.map_err(sql)?;
            Ok(())
        })
        .name(format!("record-{}", change.kind.as_str()))
        .await?;
        state.next_seq += 1;
        if let Some(status) = status {
            state.status = status;
        }
        ctx.set(STATE, Json(state.clone()));
        Ok(())
    }

    /// Applies `action`'s transition, or rejects it with `409 invalid_transition`.
    fn transition(state: &OrderState, action: OrderAction) -> Result<OrderStatus, HandlerError> {
        state.status.apply(action).map_err(invalid_transition)
    }

    /// Records a fired fault and disarms it, in one batch.
    async fn fire_fault(
        &self,
        ctx: &ObjectContext<'_>,
        state: &mut OrderState,
        at: OffsetDateTime,
        fired: Fired,
        label: String,
        by: Option<UserId>,
    ) -> Result<(), HandlerError> {
        let faults = disarm(&state.faults, fired);
        let scope = OrgScope::verified(state.org_id.clone());
        let order_id = OrderId::new(ctx.key());
        let statement = self
            .deps
            .db
            .orders(&scope)
            .set_faults_statement(&order_id, &faults, state.email_outage_until, at)
            .map_err(sql)?;
        let change = Change::new(
            OrderEventKind::FaultTriggered,
            Actor::System,
            by,
            format!("Demo fault: {label}"),
        )
        .data(serde_json::json!({ "fault": label }))
        .extra(statement);
        self.commit(ctx, state, at, change).await?;
        state.faults = faults;
        ctx.set(STATE, Json(state.clone()));
        Ok(())
    }

    /// Tells the customer (one-way: `Notifications` records its timeline entry through this
    /// object, so it must not be awaited here).
    fn notify(ctx: &ObjectContext<'_>, state: &OrderState, notice: OrderNotice) {
        let _ = ctx
            .service_client::<NotificationsClient>()
            .order(Json(OrderNoticeInput {
                org_id: state.org_id.clone(),
                order_id: OrderId::new(ctx.key()),
                notice,
            }))
            .send();
    }

    /// Starts the refund of a returned order (commerce.md: the parcel is back, `RefundWorkflow`
    /// starts and still needs approval). One-way; the refund ID derives from the order, so a
    /// retry of this handler finds the same workflow instead of starting a second refund.
    fn start_return_refund(ctx: &ObjectContext<'_>, state: &OrderState, by: UserId) {
        let order_id = OrderId::new(ctx.key());
        let refund_id = RefundId::new(derive_id(
            "ref",
            &[state.org_id.as_str(), order_id.as_str(), "return"],
        ));
        let _ = ctx
            .workflow_client::<RefundWorkflowClient>(refund_id.as_str())
            .run(Json(StartRefundInput {
                org_id: state.org_id.clone(),
                order_id,
                amount: None,
                reason: RefundReason::Return,
                reason_text: Some("Parcel returned to the warehouse".to_owned()),
                evidence: Vec::new(),
                pre_approved: false,
                requested_by: Actor::Customer,
                requested_by_user: Some(by),
                charge_payment_id: None,
                ticket_id: None,
                investigation_id: None,
            }))
            .send();
    }

    /// The confirmation email after payment. An armed email-outage fault fires here: the
    /// provider "fails" for its minutes from now (`demo_faults.email_outage_until`), and the
    /// `Email` service retries until it is over.
    async fn confirm(
        &self,
        ctx: &ObjectContext<'_>,
        state: &mut OrderState,
        at: OffsetDateTime,
    ) -> Result<(), HandlerError> {
        if let EmailFault::Outage { minutes } = state.faults.email {
            // Org defaults stored before they were validated may be out of range.
            let minutes = minutes.clamp(1, MAX_EMAIL_OUTAGE_MINUTES);
            let until = at + Duration::minutes(i64::from(minutes));
            state.email_outage_until = Some(until);
            self.fire_fault(
                ctx,
                state,
                at,
                Fired::Email,
                format!("Email provider outage for {minutes} min"),
                None,
            )
            .await?;
        }
        Self::notify(ctx, state, OrderNotice::Confirmation);
        Ok(())
    }

    // -- steps --------------------------------------------------------------------------------

    /// Pay: capture (with the payment fault), then start processing and issue the invoice.
    async fn pay(
        &self,
        ctx: &ObjectContext<'_>,
        state: &mut OrderState,
        at: OffsetDateTime,
        by: Option<UserId>,
    ) -> Result<(), HandlerError> {
        let paid = Self::transition(state, OrderAction::Pay)?;
        let order_id = OrderId::new(ctx.key());
        let payment_fault = state.faults.payment;
        let invoice_fault = state.faults.invoice;
        let outcome: Json<CaptureOutcome> = ctx
            .service_client::<PaymentsClient>()
            .capture(Json(CaptureInput {
                org_id: state.org_id.clone(),
                order_id: order_id.clone(),
                amount: state.total,
                method: state.payment_method,
                fault: payment_fault,
                sim_at: state.clock.now(at),
            }))
            .call()
            .await?;
        let outcome = outcome.into_inner();
        if payment_fault != PaymentFault::None {
            let label = fault_label(&state.faults, OrderAction::Pay).unwrap_or_default();
            self.fire_fault(ctx, state, at, Fired::Payment, label, by.clone())
                .await?;
        }
        if let Some(declined) = outcome.declined {
            let change = Change::new(
                OrderEventKind::PaymentDeclined,
                Actor::Customer,
                by,
                format!("Payment of {} declined ({})", state.total, declined.reason),
            )
            .reference(RefKind::Payment, declined.id.as_str());
            return self.commit(ctx, state, at, change).await;
        }
        for (i, charge) in outcome.charges.iter().enumerate() {
            let mut change = Change::new(
                OrderEventKind::PaymentCaptured,
                Actor::Customer,
                by.clone(),
                if i == 0 {
                    format!("Payment of {} captured", charge.amount)
                } else {
                    format!("Payment of {} captured again", charge.amount)
                },
            )
            .reference(RefKind::Payment, charge.id.as_str())
            .data(
                serde_json::json!({ "provider_ref": charge.provider_ref, "amount": charge.amount }),
            );
            if i == 0 {
                change = change.status(paid);
            }
            self.commit(ctx, state, at, change).await?;
        }
        self.after_payment(ctx, state, at, invoice_fault).await
    }

    /// The automatic steps after payment: processing, then the invoice (EU prepaid practice).
    async fn after_payment(
        &self,
        ctx: &ObjectContext<'_>,
        state: &mut OrderState,
        at: OffsetDateTime,
        invoice_fault: InvoiceFault,
    ) -> Result<(), HandlerError> {
        let processing = Self::transition(state, OrderAction::StartProcessing)?;
        self.commit(
            ctx,
            state,
            at,
            Change::new(
                OrderEventKind::ProcessingStarted,
                Actor::System,
                None,
                "Processing started",
            )
            .status(processing),
        )
        .await?;
        // Replacement orders are paid by the store and not invoiced.
        if state.payment_method == PaymentMethod::Store {
            return self.confirm(ctx, state, at).await;
        }
        let missing = invoice_fault == InvoiceFault::MissingBillingAddress;
        let issued: Json<IssuedInvoice> = ctx
            .service_client::<InvoicingClient>()
            .issue(Json(IssueInvoiceInput {
                org_id: state.org_id.clone(),
                order_id: OrderId::new(ctx.key()),
                missing_billing_address: missing,
                issue_date: state.clock.now(at).date(),
            }))
            .call()
            .await?;
        let issued = issued.into_inner();
        if missing {
            self.fire_fault(
                ctx,
                state,
                at,
                Fired::Invoice,
                "Invoice without billing address".to_owned(),
                None,
            )
            .await?;
        }
        let change = Change::new(
            OrderEventKind::InvoiceIssued,
            Actor::System,
            None,
            format!("Invoice {} issued", issued.invoice.number),
        )
        .reference(RefKind::Invoice, issued.invoice.id.as_str())
        .data(serde_json::json!({ "number": issued.invoice.number, "gross": issued.gross }));
        self.commit(ctx, state, at, change).await?;
        state.invoice = Some(issued.invoice);
        ctx.set(STATE, Json(state.clone()));
        self.confirm(ctx, state, at).await
    }

    /// Ship: hand the parcel to the carrier (with the lost and API-down faults).
    async fn ship(
        &self,
        ctx: &ObjectContext<'_>,
        state: &mut OrderState,
        at: OffsetDateTime,
        by: Option<UserId>,
    ) -> Result<(), HandlerError> {
        let shipped = Self::transition(state, OrderAction::Ship)?;
        let lost = match state.faults.shipment {
            ShipmentFault::Lost { outcome } => Some(outcome),
            ShipmentFault::None => None,
        };
        let api_down_until =
            (state.faults.carrier == CarrierFault::ApiDown).then(|| at + CARRIER_OUTAGE);
        if let Some(label) = fault_label(&state.faults, OrderAction::Ship) {
            self.fire_fault(ctx, state, at, Fired::Shipment, label, by.clone())
                .await?;
        }
        let info: Json<ShipmentInfo> = ctx
            .service_client::<CarrierClient>()
            .create_shipment(Json(CreateShipmentInput {
                org_id: state.org_id.clone(),
                order_id: OrderId::new(ctx.key()),
                sim_now: state.clock.now(at),
                lost,
                api_down_until,
            }))
            .call()
            .await?;
        let info = info.into_inner();
        let change = Change::new(
            OrderEventKind::Shipped,
            Actor::Human,
            by,
            format!("Shipped with {}, tracking {}, expected {}", info.carrier, info.tracking_number, info.eta),
        )
        .status(shipped)
        .reference(RefKind::Shipment, info.id.as_str())
        .data(serde_json::json!({ "tracking_number": info.tracking_number, "eta": info.eta.to_string() }));
        self.commit(ctx, state, at, change).await?;
        state.shipment = Some(info);
        ctx.set(STATE, Json(state.clone()));
        Self::notify(ctx, state, OrderNotice::Shipped);
        Ok(())
    }

    /// Cancel an unpaid order and put its stock back.
    async fn do_cancel(
        &self,
        ctx: &ObjectContext<'_>,
        state: &mut OrderState,
        at: OffsetDateTime,
        actor: Actor,
        by: Option<UserId>,
        reason: Option<String>,
    ) -> Result<(), HandlerError> {
        let cancelled = Self::transition(state, OrderAction::Cancel)?;
        let scope = OrgScope::verified(state.org_id.clone());
        let restock = self
            .deps
            .db
            .orders(&scope)
            .restock_statement(&OrderId::new(ctx.key()), at);
        let summary = match &reason {
            Some(r) if !r.trim().is_empty() => format!("Order cancelled: {}", r.trim()),
            _ => "Order cancelled".to_owned(),
        };
        let change = Change::new(OrderEventKind::Cancelled, actor, by, summary)
            .status(cancelled)
            .extra(restock)
            .data(serde_json::json!({ "reason": reason }));
        self.commit(ctx, state, at, change).await?;
        Self::notify(ctx, state, OrderNotice::Cancelled { reason });
        Ok(())
    }

    /// Request a return of a delivered order within the return window.
    async fn do_request_return(
        &self,
        ctx: &ObjectContext<'_>,
        state: &mut OrderState,
        at: OffsetDateTime,
        actor: Actor,
        by: Option<UserId>,
        reason: Option<String>,
    ) -> Result<(), HandlerError> {
        let next = Self::transition(state, OrderAction::RequestReturn)?;
        let today = state.clock.now(at).date();
        if let Some(delivered) = state.delivered_on
            && !within_return_window(delivered, today)
        {
            return Err(not_now(format!(
                "The {}-day return window ended {}",
                techdepo_domain::RETURN_WINDOW_DAYS,
                delivered + Duration::days(techdepo_domain::RETURN_WINDOW_DAYS)
            )));
        }
        let change = Change::new(
            OrderEventKind::ReturnRequested,
            actor,
            by,
            "Return requested",
        )
        .status(next)
        .data(serde_json::json!({ "reason": reason }));
        self.commit(ctx, state, at, change).await
    }

    /// Asks the carrier whether the parcel arrived by now, and delivers the order if so.
    async fn deliver_if_due(
        &self,
        ctx: &ObjectContext<'_>,
        state: &mut OrderState,
        at: OffsetDateTime,
    ) -> Result<(), HandlerError> {
        if state.status != OrderStatus::Shipped {
            return Ok(());
        }
        let tracked: Json<TrackResult> = ctx
            .service_client::<CarrierClient>()
            .track(Json(TrackInput {
                org_id: state.org_id.clone(),
                order_id: OrderId::new(ctx.key()),
                sim_now: state.clock.now(at),
            }))
            .call()
            .await?;
        let tracked = tracked.into_inner();
        state.shipment = Some(tracked.shipment.clone());
        let Some(on) = tracked.delivered_on else {
            ctx.set(STATE, Json(state.clone()));
            return Ok(());
        };
        let delivered = Self::transition(state, OrderAction::Deliver)?;
        let change = Change::new(
            OrderEventKind::Delivered,
            Actor::System,
            None,
            format!("Delivered on {on}"),
        )
        .status(delivered)
        .reference(RefKind::Shipment, tracked.shipment.id.as_str());
        self.commit(ctx, state, at, change).await?;
        state.delivered_on = Some(on);
        ctx.set(STATE, Json(state.clone()));
        Self::notify(ctx, state, OrderNotice::Delivered);
        Ok(())
    }
}

#[restate_sdk::object(name = "Order")]
impl Order {
    /// Writes the order (`awaiting_payment`): allocates its number, takes the stock, arms the
    /// faults and starts the clock. Idempotent: an existing order is returned. A replacement
    /// order (paid by the store) moves on to processing at once.
    #[handler(ingress_private = true)]
    async fn place(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<PlaceOrderInput>,
    ) -> Result<Json<PlacedOrder>, HandlerError> {
        let input = input.into_inner();
        if let Some(existing) = ctx.get::<Json<OrderState>>(STATE).await? {
            let existing = existing.into_inner();
            if existing.org_id != input.org_id {
                return Err(not_found("order"));
            }
            return Ok(Json(PlacedOrder {
                order_id: OrderId::new(ctx.key()),
                number: existing.number,
                status: existing.status,
            }));
        }
        if input.order.id.as_str() != ctx.key() {
            return Err(internal(
                "Order/place called with a key that is not the order ID",
            ));
        }
        if input.order.items.is_empty() {
            return Err(invalid("items", "An order needs at least one item"));
        }
        let at = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        let scope = OrgScope::verified(input.org_id.clone());
        let order = input.order.clone();
        let faults = input.faults;
        let event = NewOrderEvent {
            seq: 1,
            kind: OrderEventKind::Placed,
            actor: input.actor,
            actor_user_id: input.actor_user_id.clone(),
            status_after: Some(OrderStatus::AwaitingPayment),
            summary: format!(
                "Order placed: {} item(s), {}",
                order.items.iter().map(|i| i.quantity).sum::<u32>(),
                order.total()
            ),
            reference: None,
            data: serde_json::json!({ "total": order.total(), "compat_override": order.compat_override }),
            invocation_id: Some(ctx.invocation_id().to_owned()),
            at,
            sim_at: at,
        };
        let number: Json<OrderNumber> = ctx
            .run(|| async move {
                let orders = db.orders(&scope);
                if let Some(existing) = orders.get(&order.id).await.map_err(sql)? {
                    return Ok(Json(existing.number));
                }
                let statements = orders
                    .place_statements(&order, &faults, &event, at)
                    .map_err(sql)?;
                match db.batch(&statements).await {
                    Ok(results) => placed_number(&results)
                        .map(Json)
                        .ok_or_else(|| internal("order number not returned")),
                    Err(e) if is_oversell(&e) => Err(reject(techdepo_api_types::ApiError::new(
                        techdepo_api_types::ErrorCode::OutOfStock,
                        "Some items just sold out; review your cart",
                    ))),
                    Err(e) if e.is_constraint(techdepo_ports::ConstraintKind::ForeignKey) => {
                        Err(not_found("org"))
                    }
                    Err(e) => Err(sql(e)),
                }
            })
            .name("place-order")
            .await?;
        let mut state = OrderState {
            org_id: input.org_id.clone(),
            user_id: input.order.user_id.clone(),
            number: number.into_inner(),
            status: OrderStatus::AwaitingPayment,
            payment_method: input.order.payment_method,
            total: input.order.total(),
            next_seq: 2,
            faults: input.faults,
            clock: VirtualClock::default(),
            refunded: Money::ZERO,
            invoice: None,
            shipment: None,
            delivered_on: None,
            email_outage_until: None,
        };
        ctx.set(STATE, Json(state.clone()));
        if state.payment_method == PaymentMethod::Store {
            let paid = Self::transition(&state, OrderAction::Pay)?;
            let change = Change::new(
                OrderEventKind::PaymentCaptured,
                Actor::System,
                None,
                "Paid by the store (replacement)",
            )
            .status(paid);
            self.commit(&ctx, &mut state, at, change).await?;
            self.after_payment(&ctx, &mut state, at, InvoiceFault::None)
                .await?;
        }
        Ok(Json(PlacedOrder {
            order_id: OrderId::new(ctx.key()),
            number: state.number,
            status: state.status,
        }))
    }

    /// Runs one demo step (pay, ship, cancel, request return, parcel back) with the armed
    /// faults.
    #[handler]
    async fn demo_action(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<DemoActionInput>,
    ) -> Result<Json<OrderStatus>, HandlerError> {
        let input = input.into_inner();
        let mut state = Self::load(&ctx, &input.org_id).await?;
        if !input.action.is_demo_control() {
            return Err(invalid("action", "Not a demo control"));
        }
        Self::transition(&state, input.action)?;
        let at = util::now(&ctx).await?;
        let by = Some(input.by.clone());
        match input.action {
            OrderAction::Pay => self.pay(&ctx, &mut state, at, by).await?,
            OrderAction::Ship => self.ship(&ctx, &mut state, at, by).await?,
            OrderAction::Cancel => {
                self.do_cancel(&ctx, &mut state, at, Actor::Customer, by, None)
                    .await?
            }
            OrderAction::RequestReturn => {
                self.do_request_return(&ctx, &mut state, at, Actor::Customer, by, None)
                    .await?
            }
            OrderAction::MarkReturned => {
                let returned = Self::transition(&state, OrderAction::MarkReturned)?;
                let change = Change::new(
                    OrderEventKind::Returned,
                    Actor::Customer,
                    by,
                    "Parcel returned to the warehouse",
                )
                .status(returned);
                self.commit(&ctx, &mut state, at, change).await?;
                Self::start_return_refund(&ctx, &state, input.by.clone());
            }
            OrderAction::StartProcessing | OrderAction::Deliver | OrderAction::MarkRefunded => {
                return Err(invalid("action", "Not a demo control"));
            }
        }
        Ok(Json(state.status))
    }

    /// Replaces the armed faults.
    #[handler]
    async fn set_faults(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<SetFaultsInput>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        let mut state = Self::load(&ctx, &input.org_id).await?;
        if let Err(e) = input.faults.validate() {
            return Err(invalid(&format!("faults.{}", e.field()), e.to_string()));
        }
        if state.faults == input.faults {
            return Ok(());
        }
        let at = util::now(&ctx).await?;
        let scope = OrgScope::verified(state.org_id.clone());
        let statement = self
            .deps
            .db
            .orders(&scope)
            .set_faults_statement(
                &OrderId::new(ctx.key()),
                &input.faults,
                state.email_outage_until,
                at,
            )
            .map_err(sql)?;
        let change = Change::new(
            OrderEventKind::FaultsChanged,
            Actor::Human,
            Some(input.by.clone()),
            "Demo faults changed",
        )
        .data(serde_json::json!({ "faults": input.faults }))
        .extra(statement);
        self.commit(&ctx, &mut state, at, change).await?;
        state.faults = input.faults;
        ctx.set(STATE, Json(state));
        Ok(())
    }

    /// Moves the simulated clock (delivering when it passes the ETA of a parcel that is not
    /// lost), or moves the ship date to last Friday.
    #[handler]
    async fn advance_clock(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<AdvanceClockInput>,
    ) -> Result<Json<ClockAdvanced>, HandlerError> {
        let input = input.into_inner();
        let mut state = Self::load(&ctx, &input.org_id).await?;
        if state.status.is_terminal() {
            return Err(not_now(format!(
                "The order is {}",
                state.status.as_str().replace('_', " ")
            )));
        }
        let at = util::now(&ctx).await?;
        let by = Some(input.by.clone());
        match input.step.advance() {
            Some(advance) => {
                let scope = OrgScope::verified(state.org_id.clone());
                let calendar = if advance == ClockAdvance::Workday {
                    let db = self.deps.db.clone();
                    let scope = scope.clone();
                    let calendar: Json<WorkCalendar> = ctx
                        .run(|| async move {
                            let settings = db.orgs(&scope).settings().await.map_err(sql)?;
                            Ok(Json(settings.calendar()))
                        })
                        .name("load-calendar")
                        .await?;
                    calendar.into_inner()
                } else {
                    WorkCalendar::new()
                };
                let simulated_now = state.clock.apply(at, advance, &calendar);
                let statement = self.deps.db.orders(&scope).set_clock_statement(
                    &OrderId::new(ctx.key()),
                    state.clock,
                    at,
                );
                let change = Change::new(
                    OrderEventKind::ClockAdvanced,
                    Actor::Human,
                    by,
                    format!("Simulated clock: {} (now {})", input.step.label(), simulated_now.date()),
                )
                .data(serde_json::json!({ "step": input.step, "offset_secs": state.clock.offset_secs() }))
                .extra(statement);
                self.commit(&ctx, &mut state, at, change).await?;
                self.deliver_if_due(&ctx, &mut state, at).await?;
            }
            None => {
                let shipment = match (&state.shipment, state.status) {
                    (Some(s), OrderStatus::Shipped) => s.clone(),
                    _ => return Err(not_now("Only a shipped order that is not delivered yet")),
                };
                debug_assert_eq!(input.step, ClockStep::ShipLastFriday);
                let friday = last_friday(shipment.shipped_on, state.clock.now(at).date())
                    .map_err(not_now)?;
                let moved: Json<ShipmentInfo> = ctx
                    .service_client::<CarrierClient>()
                    .move_ship_date(Json(MoveShipDateInput {
                        org_id: state.org_id.clone(),
                        order_id: OrderId::new(ctx.key()),
                        shipped_on: friday,
                    }))
                    .call()
                    .await?;
                let moved = moved.into_inner();
                let change = Change::new(
                    OrderEventKind::ShipDateChanged,
                    Actor::Human,
                    by,
                    format!("Ship date moved to {} (expected {})", moved.shipped_on, moved.eta),
                )
                .reference(RefKind::Shipment, moved.id.as_str())
                .data(serde_json::json!({ "from": shipment.shipped_on.to_string(), "to": moved.shipped_on.to_string(), "eta": moved.eta.to_string() }));
                self.commit(&ctx, &mut state, at, change).await?;
                state.shipment = Some(moved);
                ctx.set(STATE, Json(state.clone()));
            }
        }
        Ok(Json(ClockAdvanced {
            offset_secs: state.clock.offset_secs(),
            status: state.status,
        }))
    }

    /// Cancels an unpaid order (paid orders are refunded instead: `409 invalid_transition`).
    #[handler]
    async fn cancel(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<CancelOrderInput>,
    ) -> Result<Json<OrderStatus>, HandlerError> {
        let input = input.into_inner();
        let mut state = Self::load(&ctx, &input.org_id).await?;
        let at = util::now(&ctx).await?;
        self.do_cancel(&ctx, &mut state, at, input.actor, input.by, input.reason)
            .await?;
        Ok(Json(state.status))
    }

    /// Requests a return (support routes).
    #[handler(ingress_private = true)]
    async fn request_return(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<RequestReturnInput>,
    ) -> Result<Json<OrderStatus>, HandlerError> {
        let input = input.into_inner();
        let mut state = Self::load(&ctx, &input.org_id).await?;
        let at = util::now(&ctx).await?;
        self.do_request_return(&ctx, &mut state, at, input.actor, input.by, input.reason)
            .await?;
        Ok(Json(state.status))
    }

    /// Records a completed refund (`RefundWorkflow`): a full one moves the order to
    /// `refunded`; either clears `refund_pending`.
    #[handler(ingress_private = true)]
    async fn mark_refunded(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<MarkRefundedInput>,
    ) -> Result<Json<OrderStatus>, HandlerError> {
        let input = input.into_inner();
        let mut state = Self::load(&ctx, &input.org_id).await?;
        let at = util::now(&ctx).await?;
        let mut change = Change::new(
            OrderEventKind::Refunded,
            input.actor,
            input.by,
            if input.full {
                format!("Refunded in full: {}", input.amount)
            } else {
                format!("Refunded {}", input.amount)
            },
        )
        .reference(RefKind::Refund, input.refund_id.as_str())
        .data(serde_json::json!({ "amount": input.amount, "full": input.full }));
        change.refund_pending = Some(false);
        // The money already moved: a full refund of an order whose status cannot become
        // `refunded` (a delivered order) is still recorded, as a timeline entry only.
        if input.full
            && let Ok(refunded) = state.status.apply(OrderAction::MarkRefunded)
        {
            change = change.status(refunded);
        }
        self.commit(&ctx, &mut state, at, change).await?;
        state.refunded += input.amount;
        ctx.set(STATE, Json(state.clone()));
        Ok(Json(state.status))
    }

    /// Appends a timeline entry for another component (and sets overlay flags). Never changes
    /// the status.
    #[handler(ingress_private = true)]
    async fn record_event(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<RecordEventInput>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        let mut state = Self::load(&ctx, &input.org_id).await?;
        let at = util::now(&ctx).await?;
        let mut change = Change::new(input.kind, input.actor, input.actor_user_id, input.summary)
            .data(input.data);
        change.reference = input.reference;
        change.refund_pending = input.refund_pending;
        change.investigation_open = input.investigation_open;
        change.invocation_id = input.invocation_id;
        self.commit(&ctx, &mut state, at, change).await
    }

    /// The object's state (`None` for an unknown order). For workflows that need the status,
    /// clock or shipment without reading D1.
    #[handler(ingress_private = true)]
    async fn get(
        &self,
        ctx: SharedObjectContext<'_>,
    ) -> Result<Json<Option<OrderState>>, HandlerError> {
        Ok(Json(
            ctx.get::<Json<OrderState>>(STATE)
                .await?
                .map(Json::into_inner),
        ))
    }
}

/// Lost-parcel outcome armed on the order, if any.
pub fn armed_loss(faults: &FaultToggles) -> Option<LostOutcome> {
    match faults.shipment {
        ShipmentFault::Lost { outcome } => Some(outcome),
        ShipmentFault::None => None,
    }
}

#[cfg(test)]
mod tests {
    use time::macros::date;

    use super::*;

    #[test]
    fn faults_fire_once() {
        let armed = FaultToggles {
            payment: PaymentFault::ChargeTwice,
            invoice: InvoiceFault::MissingBillingAddress,
            shipment: ShipmentFault::Lost {
                outcome: LostOutcome::Gone,
            },
            carrier: CarrierFault::ApiDown,
            email: EmailFault::Outage { minutes: 5 },
        };
        let after_pay = disarm(&armed, Fired::Payment);
        assert_eq!(after_pay.payment, PaymentFault::None);
        assert_eq!(
            after_pay.invoice, armed.invoice,
            "fires after a successful payment"
        );
        assert_eq!(after_pay.shipment, armed.shipment);
        let after_invoice = disarm(&after_pay, Fired::Invoice);
        assert_eq!(after_invoice.invoice, InvoiceFault::None);
        let after_ship = disarm(&after_invoice, Fired::Shipment);
        assert_eq!(after_ship.shipment, ShipmentFault::None);
        assert_eq!(after_ship.carrier, CarrierFault::None);
        assert_eq!(
            after_ship.email, armed.email,
            "the email fault fires with the confirmation email"
        );
        assert_eq!(
            fault_label(&armed, OrderAction::Pay).as_deref(),
            Some("Charge twice")
        );
        assert_eq!(armed_loss(&armed), Some(LostOutcome::Gone));
    }

    #[test]
    fn ship_date_moves_back_to_last_friday() {
        let tuesday = date!(2026 - 10 - 20);
        assert_eq!(last_friday(tuesday, tuesday), Ok(date!(2026 - 10 - 16)));
        assert!(last_friday(date!(2026 - 10 - 16), tuesday).is_err());
        let friday = date!(2026 - 10 - 23);
        assert_eq!(
            last_friday(friday, friday),
            Ok(date!(2026 - 10 - 16)),
            "a week back on a Friday"
        );
    }

    #[test]
    fn state_round_trips() {
        let state = OrderState {
            org_id: OrgId::new("org_1"),
            user_id: UserId::new("user_1"),
            number: OrderNumber::new(1042),
            status: OrderStatus::Shipped,
            payment_method: PaymentMethod::Card,
            total: Money::from_euros(100),
            next_seq: 7,
            faults: FaultToggles::default(),
            clock: VirtualClock::from_offset_secs(86_400),
            refunded: Money::ZERO,
            invoice: None,
            shipment: Some(ShipmentInfo {
                id: techdepo_domain::ShipmentId::new("shp_1"),
                carrier: "Kestrel Parcel".into(),
                tracking_number: techdepo_domain::TrackingNumber::new("KP0000000001"),
                shipped_on: date!(2026 - 10 - 16),
                eta: date!(2026 - 10 - 19),
            }),
            delivered_on: None,
            email_outage_until: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"shipped_on\":\"2026-10-16\""));
        assert_eq!(serde_json::from_str::<OrderState>(&json).unwrap(), state);
    }
}
