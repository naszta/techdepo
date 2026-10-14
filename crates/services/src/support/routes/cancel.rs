//! `CancelOrder` (support.md §4): the order's state decides everything, no LLM.
//!
//! | Order state | Action | Reply |
//! |---|---|---|
//! | awaiting payment | `Order/cancel` | cancelled (resolved) |
//! | paid, processing | `RefundWorkflow` (needs approval) | refund requested; the ticket waits on the refund |
//! | shipped | none | already shipped, return once it arrives |
//! | delivered, within 14 days | `Order/request_return` | return requested |
//! | delivered, later | none | the return window has closed |
//! | return requested / returned | none | your return is under way |
//! | cancelled / refunded | none | already taken care of |
//!
//! The table is [`order_plan`], shared with `RefundRequest`.

use std::sync::Arc;

use restate_sdk::prelude::*;
use techdepo_api_types::Evidence;
use techdepo_api_types::ingress::{CancelOrderInput, StartRefundInput};
use techdepo_api_types::vocab::RefundReason;
use techdepo_domain::{Actor, Cancellation, OrderStatus, within_return_window};
use time::Date;

use super::super::contract::{OrderBrief, Reply, RouteOutcome, RouteRequest, WorkflowRef};
use super::{OrderFacts, Routed, choose_order, load_order, refund_blocked, reply, report};
use crate::Deps;
use crate::notifications::templates::{CancelOutcome, SupportTemplate};
use crate::order::{OrderClient, RequestReturnInput};
use crate::refund::RefundWorkflowClient;

/// What the customer asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// "Cancel my order."
    Cancel,
    /// "I want my money back."
    Refund,
}

/// What to do about the order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderPlan {
    /// Cancel it (`Order/cancel`).
    Cancel,
    /// Refund it (`RefundWorkflow`, needs approval).
    Refund,
    /// Request the return (`Order/request_return`).
    RequestReturn,
    /// Nothing was charged (a refund request for an unpaid order).
    NothingToRefund,
    /// Nothing to do; tell the customer why.
    Tell(CancelOutcome),
}

/// The decision table (support.md §4 `CancelOrder`, reused by `RefundRequest`): a function of the
/// order's status and, for a delivered order, the return window on its simulated calendar.
pub fn order_plan(
    ask: Ask,
    status: OrderStatus,
    delivered_on: Option<Date>,
    today: Date,
) -> OrderPlan {
    match status.cancellation() {
        Cancellation::CancelNow => match ask {
            Ask::Cancel => OrderPlan::Cancel,
            Ask::Refund => OrderPlan::NothingToRefund,
        },
        Cancellation::RefundRequired => OrderPlan::Refund,
        Cancellation::InTransit => OrderPlan::Tell(CancelOutcome::AlreadyShipped),
        Cancellation::ReturnRequired => match delivered_on {
            Some(on) if !within_return_window(on, today) => {
                OrderPlan::Tell(CancelOutcome::ReturnWindowClosed { delivered_on: on })
            }
            _ => OrderPlan::RequestReturn,
        },
        Cancellation::ReturnInProgress => OrderPlan::Tell(CancelOutcome::ReturnUnderWay),
        Cancellation::AlreadyClosed => OrderPlan::Tell(CancelOutcome::AlreadyClosed),
    }
}

/// Carries out `plan` for the customer's `order` (both cancel and refund routes).
pub(super) async fn execute(
    ctx: &Context<'_>,
    req: &RouteRequest,
    order: &OrderBrief,
    facts: &OrderFacts,
    plan: OrderPlan,
    ask: Ask,
) -> Result<Routed, TerminalError> {
    let n = facts.number;
    let cancel = |outcome| {
        reply(
            SupportTemplate::Cancel {
                order_number: n,
                outcome,
            },
            true,
        )
    };
    let outcome = match plan {
        OrderPlan::Cancel => {
            ctx.object_client::<OrderClient>(facts.order_id.as_str())
                .cancel(Json(CancelOrderInput {
                    org_id: req.org_id.clone(),
                    actor: Actor::System,
                    by: None,
                    reason: Some(format!(
                        "Customer asked by email (ticket {})",
                        req.ticket_id
                    )),
                }))
                .call()
                .await?;
            cancel(CancelOutcome::Cancelled)
        }
        OrderPlan::RequestReturn => {
            ctx.object_client::<OrderClient>(facts.order_id.as_str())
                .request_return(Json(RequestReturnInput {
                    org_id: req.org_id.clone(),
                    actor: Actor::System,
                    by: None,
                    reason: Some(format!(
                        "Customer asked by email (ticket {})",
                        req.ticket_id
                    )),
                }))
                .call()
                .await?;
            cancel(CancelOutcome::ReturnRequested)
        }
        OrderPlan::NothingToRefund => {
            reply(SupportTemplate::NothingToRefund { order_number: n }, true)
        }
        OrderPlan::Tell(outcome) => cancel(outcome),
        OrderPlan::Refund => match &facts.refund {
            Some(Ok(plan)) if !facts.refund_pending => {
                let refund_id = super::refund_id(req, &facts.order_id);
                let _ = ctx
                    .workflow_client::<RefundWorkflowClient>(refund_id.as_str())
                    .run(Json(StartRefundInput {
                        org_id: req.org_id.clone(),
                        order_id: facts.order_id.clone(),
                        amount: None,
                        reason: RefundReason::Cancellation,
                        reason_text: Some(match ask {
                            Ask::Cancel => "Customer asked to cancel the paid order".to_owned(),
                            Ask::Refund => "Customer asked for a refund before shipment".to_owned(),
                        }),
                        evidence: vec![Evidence::OrderState {
                            order_number: n,
                            status: facts.status,
                            workdays_since_ship: None,
                        }],
                        pre_approved: false,
                        requested_by: Actor::Customer,
                        requested_by_user: Some(req.user_id.clone()),
                        charge_payment_id: None,
                        ticket_id: Some(req.ticket_id.clone()),
                        investigation_id: None,
                    }))
                    .send();
                let template = match ask {
                    Ask::Cancel => SupportTemplate::Cancel {
                        order_number: n,
                        outcome: CancelOutcome::RefundRequested {
                            amount: plan.amount,
                        },
                    },
                    Ask::Refund => SupportTemplate::RefundRequested {
                        order_number: n,
                        amount: plan.amount,
                    },
                };
                RouteOutcome::WaitingOnWorkflow {
                    workflow: WorkflowRef::Refund(refund_id),
                    reply: Some(Reply::template(template)),
                }
            }
            Some(Err(error)) => reply(refund_blocked(facts, error), true),
            _ => reply(SupportTemplate::RefundUnderWay { order_number: n }, true),
        },
    };
    Ok(Routed::about(order, outcome))
}

/// The `CancelOrder` route.
pub struct CancelOrderRoute {
    deps: Arc<Deps>,
}

impl CancelOrderRoute {
    /// The route over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }

    async fn decide(&self, ctx: &Context<'_>, req: &RouteRequest) -> Result<Routed, TerminalError> {
        let (order, _) = match choose_order(req, true) {
            Ok(chosen) => chosen,
            Err(outcome) => return Ok(Routed::without_order(*outcome)),
        };
        let refund = Some((RefundReason::Cancellation, None));
        let facts = load_order(&self.deps, ctx, req, &order, refund).await?;
        let delivered_on = facts.parcel.as_ref().and_then(|p| p.delivered_on);
        let plan = order_plan(Ask::Cancel, facts.status, delivered_on, facts.today);
        execute(ctx, req, &order, &facts, plan, Ask::Cancel).await
    }
}

#[restate_sdk::service(name = "CancelOrder", ingress_private = true)]
impl CancelOrderRoute {
    /// Handles "cancel my order" and reports to the ticket.
    #[handler]
    async fn run(&self, ctx: Context<'_>, input: Json<RouteRequest>) -> Result<(), HandlerError> {
        let req = input.into_inner();
        let routed = self.decide(&ctx, &req).await;
        report(&ctx, &req, "cancel order", routed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use time::macros::date;

    use super::*;

    #[test]
    fn the_order_state_decides_a_cancellation() {
        let today = date!(2026 - 10 - 20);
        let plan = |status| order_plan(Ask::Cancel, status, None, today);
        assert_eq!(plan(OrderStatus::AwaitingPayment), OrderPlan::Cancel);
        assert_eq!(plan(OrderStatus::Paid), OrderPlan::Refund);
        assert_eq!(plan(OrderStatus::Processing), OrderPlan::Refund);
        assert_eq!(
            plan(OrderStatus::Shipped),
            OrderPlan::Tell(CancelOutcome::AlreadyShipped)
        );
        assert_eq!(
            plan(OrderStatus::ReturnRequested),
            OrderPlan::Tell(CancelOutcome::ReturnUnderWay)
        );
        assert_eq!(
            plan(OrderStatus::Returned),
            OrderPlan::Tell(CancelOutcome::ReturnUnderWay)
        );
        assert_eq!(
            plan(OrderStatus::Cancelled),
            OrderPlan::Tell(CancelOutcome::AlreadyClosed)
        );
        assert_eq!(
            plan(OrderStatus::Refunded),
            OrderPlan::Tell(CancelOutcome::AlreadyClosed)
        );
    }

    #[test]
    fn delivered_orders_are_returned_within_the_window() {
        let delivered = date!(2026 - 10 - 01);
        let within = order_plan(
            Ask::Cancel,
            OrderStatus::Delivered,
            Some(delivered),
            date!(2026 - 10 - 15),
        );
        assert_eq!(within, OrderPlan::RequestReturn);
        let late = order_plan(
            Ask::Refund,
            OrderStatus::Delivered,
            Some(delivered),
            date!(2026 - 10 - 16),
        );
        assert_eq!(
            late,
            OrderPlan::Tell(CancelOutcome::ReturnWindowClosed {
                delivered_on: delivered
            })
        );
    }

    #[test]
    fn a_refund_of_an_unpaid_order_refunds_nothing() {
        let today = date!(2026 - 10 - 20);
        assert_eq!(
            order_plan(Ask::Refund, OrderStatus::AwaitingPayment, None, today),
            OrderPlan::NothingToRefund
        );
        assert_eq!(
            order_plan(Ask::Refund, OrderStatus::Paid, None, today),
            OrderPlan::Refund
        );
    }
}
