//! The deterministic support routes (support.md §4): Restate services invoked by name
//! (`<Route>/run(RouteRequest)`, [`super::contract`]) that decide from the facts in D1 with no
//! LLM, act through the commerce components, and report exactly one [`RouteOutcome`] back to
//! `SupportTicket/route_result`.
//!
//! | Route | Service | Acts | Order |
//! |---|---|---|---|
//! | [`cancel::CancelOrderRoute`] | `CancelOrder` | `Order/cancel`, `RefundWorkflow`, `Order/request_return` | explicit |
//! | [`refund_request::RefundRequestRoute`] | `RefundRequest` | `RefundWorkflow`, `Order/request_return` | explicit |
//! | [`billing::BillingIssueRoute`] | `BillingIssue` | `RefundWorkflow` (duplicate charge), `Invoicing/void_and_reissue` | explicit |
//! | [`wismo::WhereIsMyOrderRoute`] | `WhereIsMyOrder` | `Investigation` | may infer the only one |
//!
//! Each route is a pure decision table over the facts (unit-tested here) plus its effects.
//! **Missing information** ([`choose_order`]): a route that acts needs an order the customer named
//! in the new message; a read-only one may use the ticket's order or infer the customer's only
//! order (and says so). Otherwise it asks (`AskCustomer{expects: order_number}`, with a link to
//! the order list); order numbers that are not the customer's are named in the question and never
//! looked up. A second failed answer escalates.
//!
//! A route never leaves the ticket hanging: a terminal failure (a bug, a rejected transition) is
//! reported as an escalation; retryable failures are retried by Restate inside the steps.

pub mod billing;
pub mod cancel;
pub mod refund_request;
pub mod wismo;

use restate_ext::ContextClockExt;
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::ingress::{api_error_from_terminal, derive_id};
use techdepo_api_types::vocab::{Awaiting, RefundReason};
use techdepo_api_types::{ApiError, ErrorCode};
use techdepo_db::OrgScope;
use techdepo_db::refunds::{RefundPlan, plan_refund};
use techdepo_domain::{
    InvestigationId, Money, OrderId, OrderNumber, OrderStatus, TrackingNumber, WorkCalendar,
};
use time::{Date, OffsetDateTime};

pub use billing::BillingIssueRoute;
pub use cancel::CancelOrderRoute;
pub use refund_request::RefundRequestRoute;
pub use wismo::WhereIsMyOrderRoute;

use super::contract::{self, OrderBrief, Reply, RouteOutcome, RouteRequest, RouteResult};
use crate::Deps;
use crate::errors::{not_found, sql};
use crate::notifications::templates::SupportTemplate;
use crate::util;

/// `CancelOrder` service name.
pub const CANCEL_ORDER: &str = "CancelOrder";
/// `WhereIsMyOrder` service name.
pub const WHERE_IS_MY_ORDER: &str = "WhereIsMyOrder";
/// `BillingIssue` service name.
pub const BILLING_ISSUE: &str = "BillingIssue";
/// `RefundRequest` service name.
pub const REFUND_REQUEST: &str = "RefundRequest";

/// What a route decided: the outcome and the order it worked on (linked to the ticket).
#[derive(Debug, Clone, PartialEq)]
pub struct Routed {
    /// The order.
    pub order: Option<OrderBrief>,
    /// The outcome.
    pub outcome: RouteOutcome,
}

impl Routed {
    /// An outcome about `order`.
    pub fn about(order: &OrderBrief, outcome: RouteOutcome) -> Self {
        Self {
            order: Some(order.clone()),
            outcome,
        }
    }

    /// An outcome about no order.
    pub fn without_order(outcome: RouteOutcome) -> Self {
        Self {
            order: None,
            outcome,
        }
    }
}

/// A template reply; `resolve` closes the ticket once it is sent.
pub fn reply(template: SupportTemplate, resolve: bool) -> RouteOutcome {
    RouteOutcome::Reply {
        reply: Reply::template(template),
        resolve,
    }
}

/// Hand the ticket to a person.
pub fn escalate(reason: impl Into<String>) -> RouteOutcome {
    RouteOutcome::Escalate {
        reason: reason.into(),
        causes: Vec::new(),
    }
}

/// Which order a route works on, or what to do without one (support.md §3 "Missing
/// information"). `acts`: the route changes something, so the customer must have named the order
/// in this message. Returns the order and whether it was inferred as the customer's only order
/// (the reply then says which order it looked up; the ticket's own order needs no such note).
pub fn choose_order(
    req: &RouteRequest,
    acts: bool,
) -> Result<(OrderBrief, bool), Box<RouteOutcome>> {
    if let Some(order) = &req.order
        && (req.order_explicit || !acts)
    {
        return Ok((order.clone(), false));
    }
    if !acts
        && req.foreign_order_numbers.is_empty()
        && let [only] = req.customer_orders.as_slice()
    {
        return Ok((only.clone(), true));
    }
    Err(Box::new(ask_for_order(req)))
}

/// Ask for the order number, naming the numbers that are not the customer's; escalate when the
/// answer to our own question still did not identify one.
fn ask_for_order(req: &RouteRequest) -> RouteOutcome {
    if req.resumed == Some(Awaiting::OrderNumber) {
        return escalate("The customer's answer still did not identify one of their orders");
    }
    let question = if req.foreign_order_numbers.is_empty() {
        SupportTemplate::AskOrderNumber
    } else {
        SupportTemplate::OrderNotFound {
            numbers: req.foreign_order_numbers.clone(),
        }
    };
    RouteOutcome::AskCustomer {
        question: Reply::template(question),
        expects: Awaiting::OrderNumber,
        awakeable_id: None,
    }
}

/// The ID of a refund a route starts: derived from the routing generation, so a retried route
/// finds the same workflow instead of starting a second refund.
pub(crate) fn refund_id(req: &RouteRequest, order: &OrderId) -> techdepo_domain::RefundId {
    techdepo_domain::RefundId::new(derive_id(
        "ref",
        &[
            req.org_id.as_str(),
            &req.ticket_id.to_string(),
            &req.generation.to_string(),
            order.as_str(),
        ],
    ))
}

/// Reports `routed` to the ticket (one-way), or an escalation when the route failed for good.
pub fn report(
    ctx: &Context<'_>,
    req: &RouteRequest,
    label: &str,
    routed: Result<Routed, TerminalError>,
) {
    let routed = routed.unwrap_or_else(|e| {
        let error = api_error_from_terminal(e.code(), e.message());
        Routed::without_order(escalate(format!(
            "The {label} route could not finish: {}",
            error.message
        )))
    });
    let result = RouteResult {
        org_id: req.org_id.clone(),
        generation: Some(req.generation),
        route: Some(req.route),
        order: routed.order,
        agent_run_id: None,
        outcome: routed.outcome,
    };
    let _ = ctx
        .request::<_, ()>(
            contract::route_result_target(&req.org_id, &req.ticket_id),
            Json(result),
        )
        .send();
}

/// The handler's (journaled) now.
pub(crate) async fn now(ctx: &Context<'_>) -> Result<OffsetDateTime, TerminalError> {
    Ok(util::to_offset(ctx.now().await?))
}

/// The parcel, as the carrier records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parcel {
    /// Carrier.
    pub carrier: String,
    /// Tracking number.
    pub tracking_number: TrackingNumber,
    /// Ship date (simulated).
    pub shipped_on: Date,
    /// ETA (moved by a trace that found it).
    pub eta: Date,
    /// Delivery date (simulated).
    pub delivered_on: Option<Date>,
}

/// The facts a route decides on, read from D1 in one journaled step (`load-order`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderFacts {
    /// Order.
    pub order_id: OrderId,
    /// Number.
    pub number: OrderNumber,
    /// Status.
    pub status: OrderStatus,
    /// Total.
    pub total: Money,
    /// Today on the order's simulated calendar.
    pub today: Date,
    /// The org's workday calendar (absent in journals written before holidays were
    /// configurable: no holidays).
    #[serde(default)]
    pub calendar: WorkCalendar,
    /// The parcel, once shipped.
    pub parcel: Option<Parcel>,
    /// A refund awaits approval or execution.
    pub refund_pending: bool,
    /// The running investigation, if any.
    pub open_investigation: Option<InvestigationId>,
    /// What a refund for the asked reason would pay back (when one was asked for).
    pub refund: Option<Result<RefundPlan, ApiError>>,
}

impl OrderFacts {
    /// Full workdays since shipment (0 before).
    pub fn workdays_since_ship(&self) -> u32 {
        self.parcel.as_ref().map_or(0, |p| {
            self.calendar.workdays_between(p.shipped_on, self.today)
        })
    }
}

/// Loads the facts about `order` (which must be `req`'s customer's). `refund`: plan a refund for
/// this reason (and charge) as `RefundWorkflow` would.
pub(crate) async fn load_order(
    deps: &Deps,
    ctx: &Context<'_>,
    req: &RouteRequest,
    order: &OrderBrief,
    refund: Option<(RefundReason, Option<techdepo_api_types::PaymentId>)>,
) -> Result<OrderFacts, TerminalError> {
    let at = now(ctx).await?;
    let db = deps.db.clone();
    let (org, user, id) = (
        req.org_id.clone(),
        req.user_id.clone(),
        order.order_id.clone(),
    );
    let facts: Json<OrderFacts> = ctx
        .run(|| async move {
            let scope = OrgScope::verified(org);
            let orders = db.orders(&scope);
            let row = orders
                .get(&id)
                .await
                .map_err(sql)?
                .filter(|o| o.user_id == user)
                .ok_or_else(|| not_found("order"))?;
            let today = orders.clock(&id).await.map_err(sql)?.now(at).date();
            let calendar = db.orgs(&scope).settings().await.map_err(sql)?.calendar();
            let parcel = db
                .shipments(&scope)
                .for_order(&id)
                .await
                .map_err(sql)?
                .map(|s| Parcel {
                    carrier: s.carrier,
                    tracking_number: s.tracking_number,
                    shipped_on: s.shipped_on,
                    eta: s.eta,
                    delivered_on: s.delivered_on,
                });
            let open_investigation = if row.investigation_open {
                db.investigations(&scope)
                    .for_order(&id)
                    .await
                    .map_err(sql)?
                    .into_iter()
                    .find(|v| v.status.is_open())
                    .map(|v| v.id)
            } else {
                None
            };
            let refund = match refund {
                Some((reason, charge)) => {
                    let payments = db.payments(&scope).for_order(&id).await.map_err(sql)?;
                    let refunds = db.refunds(&scope).for_order(&id).await.map_err(sql)?;
                    Some(plan_refund(
                        &payments,
                        &refunds,
                        reason,
                        charge.as_ref(),
                        None,
                    ))
                }
                None => None,
            };
            Ok(Json(OrderFacts {
                order_id: row.id,
                number: row.number,
                status: row.status,
                total: row.total,
                today,
                calendar,
                parcel,
                refund_pending: row.refund_pending,
                open_investigation,
                refund,
            }))
        })
        .name("load-order")
        .await?;
    Ok(facts.into_inner())
}

/// Why a planned refund cannot start, as the customer's answer: one is already under way, or
/// there is nothing left to refund.
pub fn refund_blocked(facts: &OrderFacts, error: &ApiError) -> SupportTemplate {
    if facts.refund_pending
        || (error.code == ErrorCode::InvalidTransition && error.message.contains("in progress"))
    {
        SupportTemplate::RefundUnderWay {
            order_number: facts.number,
        }
    } else {
        SupportTemplate::Cancel {
            order_number: facts.number,
            outcome: crate::notifications::templates::CancelOutcome::AlreadyClosed,
        }
    }
}

#[cfg(test)]
mod tests {
    use techdepo_api_types::vocab::TicketRoute;
    use techdepo_domain::{Intent, OrgId, PublicOrgId, TicketId, UserId};

    use super::*;

    fn brief(n: u32) -> OrderBrief {
        OrderBrief {
            order_id: OrderId::new(format!("ord_{n}")),
            number: OrderNumber::new(n),
            status: OrderStatus::Shipped,
        }
    }

    pub(super) fn request() -> RouteRequest {
        RouteRequest {
            org_id: OrgId::new("org_1"),
            public_org_id: PublicOrgId::parse("k3x9q2ab").unwrap(),
            ticket_id: TicketId::parse("T-7F2M9Q").unwrap(),
            generation: 1,
            user_id: UserId::new("user_1"),
            customer_email: "a@example.com".into(),
            customer_name: None,
            route: TicketRoute::CancelOrder,
            intent: Intent::CancelOrder,
            intent_p: 0.9,
            subject: "Cancel".into(),
            message: "please cancel".into(),
            conversation: Vec::new(),
            order: None,
            order_explicit: false,
            customer_orders: vec![brief(1001)],
            foreign_order_numbers: Vec::new(),
            resumed: None,
        }
    }

    fn asks(outcome: &RouteOutcome) -> Option<SupportTemplate> {
        match outcome {
            RouteOutcome::AskCustomer {
                question: Reply::Template { template },
                expects: Awaiting::OrderNumber,
                awakeable_id: None,
            } => Some(template.clone()),
            _ => None,
        }
    }

    #[test]
    fn acting_routes_need_an_order_named_in_the_message() {
        let mut req = request();
        // One candidate: a read-only route infers it, an acting one asks.
        assert_eq!(choose_order(&req, false), Ok((brief(1001), true)));
        assert_eq!(
            asks(&choose_order(&req, true).unwrap_err()),
            Some(SupportTemplate::AskOrderNumber)
        );
        // The ticket's order from earlier: fine to read (no "only order" note), not to act on.
        req.order = Some(brief(1001));
        assert_eq!(choose_order(&req, false), Ok((brief(1001), false)));
        assert!(choose_order(&req, true).is_err());
        req.order_explicit = true;
        assert_eq!(choose_order(&req, true), Ok((brief(1001), false)));
    }

    #[test]
    fn several_candidates_or_foreign_numbers_ask() {
        let mut req = request();
        req.customer_orders.push(brief(1002));
        assert!(choose_order(&req, false).is_err(), "two candidates");
        let mut req = request();
        req.foreign_order_numbers = vec![OrderNumber::new(1)];
        assert_eq!(
            asks(&choose_order(&req, false).unwrap_err()),
            Some(SupportTemplate::OrderNotFound {
                numbers: vec![OrderNumber::new(1)]
            }),
            "someone else's order is never inferred around"
        );
        req.resumed = Some(Awaiting::OrderNumber);
        assert!(matches!(
            *choose_order(&req, true).unwrap_err(),
            RouteOutcome::Escalate { .. }
        ));
    }

    #[test]
    fn workdays_since_ship_follow_the_calendar() {
        let facts = OrderFacts {
            order_id: OrderId::new("ord_1"),
            number: OrderNumber::new(1001),
            status: OrderStatus::Shipped,
            total: Money::from_euros(100),
            today: time::macros::date!(2026 - 10 - 19),
            calendar: WorkCalendar::new(),
            parcel: Some(Parcel {
                carrier: "Kestrel Parcel".into(),
                tracking_number: TrackingNumber::new("KP0000000001"),
                shipped_on: time::macros::date!(2026 - 10 - 16),
                eta: time::macros::date!(2026 - 10 - 20),
                delivered_on: None,
            }),
            refund_pending: false,
            open_investigation: None,
            refund: None,
        };
        assert_eq!(facts.workdays_since_ship(), 1, "Friday to Monday");
        let holiday = OrderFacts {
            calendar: WorkCalendar::with_holidays([time::macros::date!(2026 - 10 - 19)]),
            ..facts
        };
        assert_eq!(holiday.workdays_since_ship(), 0, "Monday is a holiday");
    }
}
