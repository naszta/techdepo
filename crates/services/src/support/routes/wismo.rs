//! `WhereIsMyOrder` (support.md §4): the shipping policy as code, on the order's simulated clock.
//!
//! | Order | Answer |
//! |---|---|
//! | awaiting payment | waiting for payment |
//! | paid, processing | being prepared |
//! | shipped, 0–3 workdays | the standard 1–3 workday estimate, with the ETA |
//! | shipped, 4 workdays | the carrier is overloaded, please give it a little longer |
//! | shipped, 5+ workdays | start `Investigation`, "we opened an investigation" (the ticket waits on it) |
//! | shipped, investigation open | "we are already on it" |
//! | delivered | the carrier reports delivery on X; asked again after that answer: escalate |
//! | cancelled, refunded, returned | nothing is on its way |
//!
//! Read-only, so it may infer the customer's only order (and says which one it looked up).

use std::sync::Arc;

use restate_sdk::prelude::*;
use techdepo_api_types::ingress::{StartInvestigationInput, derive_id};
use techdepo_api_types::vocab::Direction;
use techdepo_domain::{
    Actor, InvestigationId, OrderNumber, OrderStatus, ShippingPolicyOutcome, shipping_policy,
};

use super::super::contract::{ConversationMessage, Reply, RouteOutcome, RouteRequest, WorkflowRef};
use super::{OrderFacts, Routed, choose_order, escalate, load_order, reply, report};
use crate::Deps;
use crate::investigation::InvestigationClient;
use crate::notifications::templates::{ShipmentFacts, SupportTemplate, WismoAnswer};

/// What to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WismoPlan {
    /// Tell the customer.
    Tell(WismoAnswer),
    /// Open an investigation with the carrier.
    Investigate,
    /// An investigation is already open.
    AlreadyInvestigating(InvestigationId),
    /// A person has to look.
    Escalate(&'static str),
}

/// The decision table. `asked_again`: we already told the customer the parcel was delivered.
pub fn wismo_plan(facts: &OrderFacts, asked_again: bool) -> WismoPlan {
    let shipment = facts.parcel.as_ref().map(|p| ShipmentFacts {
        carrier: p.carrier.clone(),
        tracking_number: p.tracking_number.clone(),
        shipped_on: p.shipped_on,
        eta: p.eta,
    });
    match facts.status {
        OrderStatus::AwaitingPayment => WismoPlan::Tell(WismoAnswer::AwaitingPayment),
        OrderStatus::Paid | OrderStatus::Processing => WismoPlan::Tell(WismoAnswer::BeingPrepared),
        OrderStatus::Delivered if asked_again => WismoPlan::Escalate(
            "The carrier reports the order delivered, but the customer still asks where it is",
        ),
        OrderStatus::Delivered => WismoPlan::Tell(WismoAnswer::Delivered {
            on: facts
                .parcel
                .as_ref()
                .and_then(|p| p.delivered_on)
                .unwrap_or(facts.today),
        }),
        OrderStatus::Shipped => {
            if let Some(id) = &facts.open_investigation {
                return WismoPlan::AlreadyInvestigating(id.clone());
            }
            let Some(shipment) = shipment else {
                return WismoPlan::Escalate("The order is shipped but has no shipment record");
            };
            match shipping_policy(facts.workdays_since_ship()) {
                ShippingPolicyOutcome::WithinStandard => {
                    WismoPlan::Tell(WismoAnswer::WithinEstimate { shipment })
                }
                ShippingPolicyOutcome::AskToWait => {
                    WismoPlan::Tell(WismoAnswer::CarrierOverloaded { shipment })
                }
                ShippingPolicyOutcome::OpenInvestigation => WismoPlan::Investigate,
            }
        }
        OrderStatus::Cancelled
        | OrderStatus::Refunded
        | OrderStatus::ReturnRequested
        | OrderStatus::Returned => WismoPlan::Tell(WismoAnswer::Closed {
            state: facts.status.label().to_lowercase(),
        }),
    }
}

/// Whether we already told the customer, in this conversation, that `number` was delivered.
pub fn told_delivered(conversation: &[ConversationMessage], number: OrderNumber) -> bool {
    let needle = format!("order {number} was delivered on");
    conversation
        .iter()
        .any(|m| m.direction == Direction::Outbound && m.body.contains(&needle))
}

/// The `WhereIsMyOrder` route.
pub struct WhereIsMyOrderRoute {
    deps: Arc<Deps>,
}

impl WhereIsMyOrderRoute {
    /// The route over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }

    async fn decide(&self, ctx: &Context<'_>, req: &RouteRequest) -> Result<Routed, TerminalError> {
        let (order, inferred) = match choose_order(req, false) {
            Ok(chosen) => chosen,
            Err(outcome) => return Ok(Routed::without_order(*outcome)),
        };
        let facts = load_order(&self.deps, ctx, req, &order, None).await?;
        let n = facts.number;
        let outcome = match wismo_plan(&facts, told_delivered(&req.conversation, n)) {
            WismoPlan::Tell(answer) => reply(
                SupportTemplate::Wismo {
                    order_number: n,
                    answer,
                    inferred,
                },
                true,
            ),
            WismoPlan::AlreadyInvestigating(_) => {
                // The investigation reports to the ticket that opened it.
                reply(
                    SupportTemplate::InvestigationUnderWay { order_number: n },
                    true,
                )
            }
            WismoPlan::Escalate(reason) => escalate(reason),
            WismoPlan::Investigate => {
                let id = InvestigationId::new(derive_id(
                    "inq",
                    &[
                        req.org_id.as_str(),
                        &req.ticket_id.to_string(),
                        &req.generation.to_string(),
                        facts.order_id.as_str(),
                    ],
                ));
                let _ = ctx
                    .workflow_client::<InvestigationClient>(id.as_str())
                    .run(Json(StartInvestigationInput {
                        org_id: req.org_id.clone(),
                        order_id: facts.order_id.clone(),
                        ticket_id: Some(req.ticket_id.clone()),
                        opened_by: Actor::System,
                        opened_by_user: None,
                        note: Some(format!(
                            "{} workdays since shipment (ticket {})",
                            facts.workdays_since_ship(),
                            req.ticket_id
                        )),
                    }))
                    .send();
                RouteOutcome::WaitingOnWorkflow {
                    workflow: WorkflowRef::Investigation(id),
                    reply: Some(Reply::template(SupportTemplate::InvestigationOpened {
                        order_number: n,
                    })),
                }
            }
        };
        Ok(Routed::about(&order, outcome))
    }
}

#[restate_sdk::service(name = "WhereIsMyOrder", ingress_private = true)]
impl WhereIsMyOrderRoute {
    /// Handles "where is my order?" and reports to the ticket.
    #[handler]
    async fn run(&self, ctx: Context<'_>, input: Json<RouteRequest>) -> Result<(), HandlerError> {
        let req = input.into_inner();
        let routed = self.decide(&ctx, &req).await;
        report(&ctx, &req, "where is my order", routed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use techdepo_domain::{Actor, Money, OrderId, TrackingNumber, WorkCalendar};
    use time::macros::date;
    use time::{Date, OffsetDateTime};

    use super::super::Parcel;
    use super::*;

    fn shipped(today: Date) -> OrderFacts {
        OrderFacts {
            order_id: OrderId::new("ord_1"),
            number: OrderNumber::new(1042),
            status: OrderStatus::Shipped,
            total: Money::from_euros(100),
            today,
            calendar: WorkCalendar::new(),
            parcel: Some(Parcel {
                carrier: "Kestrel Parcel".into(),
                tracking_number: TrackingNumber::new("KP0000000001"),
                // A Friday.
                shipped_on: date!(2026 - 10 - 16),
                eta: date!(2026 - 10 - 20),
                delivered_on: None,
            }),
            refund_pending: false,
            open_investigation: None,
            refund: None,
        }
    }

    #[test]
    fn shipped_orders_follow_the_shipping_policy() {
        let plan = |today| wismo_plan(&shipped(today), false);
        assert!(
            matches!(
                plan(date!(2026 - 10 - 19)),
                WismoPlan::Tell(WismoAnswer::WithinEstimate { .. })
            ),
            "Friday to Monday: 1 workday"
        );
        assert!(matches!(
            plan(date!(2026 - 10 - 21)),
            WismoPlan::Tell(WismoAnswer::WithinEstimate { .. })
        ));
        assert!(
            matches!(
                plan(date!(2026 - 10 - 22)),
                WismoPlan::Tell(WismoAnswer::CarrierOverloaded { .. })
            ),
            "4 workdays"
        );
        assert_eq!(
            plan(date!(2026 - 10 - 23)),
            WismoPlan::Investigate,
            "5 workdays"
        );
        let mut open = shipped(date!(2026 - 10 - 27));
        open.open_investigation = Some(InvestigationId::new("inq_1"));
        assert_eq!(
            wismo_plan(&open, false),
            WismoPlan::AlreadyInvestigating(InvestigationId::new("inq_1"))
        );
    }

    #[test]
    fn other_states() {
        let mut facts = shipped(date!(2026 - 10 - 19));
        facts.status = OrderStatus::AwaitingPayment;
        assert_eq!(
            wismo_plan(&facts, false),
            WismoPlan::Tell(WismoAnswer::AwaitingPayment)
        );
        facts.status = OrderStatus::Processing;
        assert_eq!(
            wismo_plan(&facts, false),
            WismoPlan::Tell(WismoAnswer::BeingPrepared)
        );
        facts.status = OrderStatus::Refunded;
        assert_eq!(
            wismo_plan(&facts, false),
            WismoPlan::Tell(WismoAnswer::Closed {
                state: "refunded".into()
            })
        );
        facts.status = OrderStatus::Delivered;
        if let Some(p) = facts.parcel.as_mut() {
            p.delivered_on = Some(date!(2026 - 10 - 19));
        }
        assert_eq!(
            wismo_plan(&facts, false),
            WismoPlan::Tell(WismoAnswer::Delivered {
                on: date!(2026 - 10 - 19)
            })
        );
        assert!(matches!(wismo_plan(&facts, true), WismoPlan::Escalate(_)));
    }

    #[test]
    fn insisting_after_the_delivery_answer_is_detected() {
        let msg = |direction, body: &str| ConversationMessage {
            seq: 1,
            direction,
            author: Actor::System,
            subject: "Re: x".into(),
            body: body.into(),
            at: OffsetDateTime::UNIX_EPOCH,
        };
        let n = OrderNumber::new(1042);
        let ours = msg(
            Direction::Outbound,
            "The carrier reports that order TD-1042 was delivered on Mon, 19 Oct 2026.",
        );
        assert!(told_delivered(std::slice::from_ref(&ours), n));
        assert!(!told_delivered(&[ours], OrderNumber::new(1043)));
        assert!(!told_delivered(
            &[msg(
                Direction::Inbound,
                "order TD-1042 was delivered on Monday?"
            )],
            n
        ));
    }
}
