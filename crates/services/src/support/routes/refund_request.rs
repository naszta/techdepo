//! `RefundRequest` (support.md §4): the same table as `CancelOrder` ([`super::cancel::order_plan`]).
//! A paid, unshipped order is refunded (`RefundWorkflow`, needs approval); a delivered one within
//! the return window gets its return requested, so the order shows **Return requested** and the
//! refund starts when the parcel is back; everything else is explained.

use std::sync::Arc;

use restate_sdk::prelude::*;
use techdepo_api_types::vocab::RefundReason;

use super::super::contract::RouteRequest;
use super::cancel::{Ask, execute, order_plan};
use super::{Routed, choose_order, load_order, report};
use crate::Deps;

/// The `RefundRequest` route.
pub struct RefundRequestRoute {
    deps: Arc<Deps>,
}

impl RefundRequestRoute {
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
        let plan = order_plan(Ask::Refund, facts.status, delivered_on, facts.today);
        execute(ctx, req, &order, &facts, plan, Ask::Refund).await
    }
}

#[restate_sdk::service(name = "RefundRequest", ingress_private = true)]
impl RefundRequestRoute {
    /// Handles "I want a refund / to return it" and reports to the ticket.
    #[handler]
    async fn run(&self, ctx: Context<'_>, input: Json<RouteRequest>) -> Result<(), HandlerError> {
        let req = input.into_inner();
        let routed = self.decide(&ctx, &req).await;
        report(&ctx, &req, "refund request", routed);
        Ok(())
    }
}
