//! The support agents (support.md §5–§7): `ProductAdvisor` and `OrderAssistant`, two routes
//! (`<Route>/run(RouteRequest)`, [`crate::support::contract`]) that run an LLM agent instead of
//! a decision table.
//!
//! | Route | Service | Tools | Typical question |
//! |---|---|---|---|
//! | [`ProductAdvisorRoute`] | `ProductAdvisor` | catalog search and details, the compat engine, alternatives, saved builds, buying guides | "Will the Kestrel B650 work with my Nimbus DDR4 kit?" |
//! | [`OrderAssistantRoute`] | `OrderAssistant` | the customer's orders, tracking, invoices, policies (read-only) | "Can I change the delivery address?" |
//!
//! **Agents decide, code acts.** An agent returns an outcome (reply, question, escalate,
//! reroute); its text then passes the [`guardrails`] (one retry with feedback) and the
//! [`judge`] ([`pipeline`]), and the route reports it to `SupportTicket/route_result`. The
//! ticket applies the org's AI reply policy: it sends the reply, holds it as a review draft, or
//! escalates when a guardrail failed. Agents never send email, never move money and have no
//! write tools; a request to *do* something is a `reroute` to the deterministic route.
//!
//! **Tenant scope by construction:** the tools read the org and the customer from their
//! [`tools::ToolCx`], filled from the request (which the router filled from the ticket), never
//! from the model's arguments.
//!
//! **Durability:** each model call is a journaled step (`llm-<n>`) behind the org's daily token
//! budget, each D1 read a journaled tool step (`tool-<n>-<name>`), the judge one step (`judge`),
//! the trace one batch (`record-trace`) ([`runtime::Journaled`]).

pub mod guardrails;
pub mod judge;
pub mod kb;
pub mod pipeline;
pub mod prompts;
pub mod runtime;
pub mod tools;

use std::sync::Arc;

use restate_sdk::prelude::*;
use techdepo_api_types::vocab::AgentKind;
use techdepo_db::OrgScope;
use techdepo_domain::OrgSettings;

use crate::Deps;
use crate::errors::sql;
use crate::support::contract::{self, RouteOutcome, RouteRequest, RouteResult};
use crate::util;

/// `ProductAdvisor` service name.
pub const PRODUCT_ADVISOR: &str = "ProductAdvisor";
/// `OrderAssistant` service name.
pub const ORDER_ASSISTANT: &str = "OrderAssistant";

/// Runs `agent` on the request and reports the outcome (with its trace) to the ticket.
async fn handle(
    deps: &Deps,
    ctx: &Context<'_>,
    agent: AgentKind,
    req: RouteRequest,
) -> Result<(), HandlerError> {
    let now = util::now(ctx).await?;
    let db = deps.db.clone();
    let scope = OrgScope::verified(req.org_id.clone());
    let settings: Result<Json<OrgSettings>, TerminalError> = ctx
        .run(|| async move { db.orgs(&scope).settings().await.map(Json).map_err(sql) })
        .name("load-settings")
        .await;
    let (outcome, agent_run_id) = match settings {
        Ok(settings) => {
            let rt = runtime::Journaled::new(ctx);
            let answered =
                pipeline::answer(&rt, deps, agent, &req, &settings.into_inner(), now).await;
            (answered.outcome, Some(answered.agent_run_id))
        }
        Err(e) => (
            RouteOutcome::Escalate {
                reason: format!("{} could not start: {}", agent.label(), e.message()),
                causes: Vec::new(),
            },
            None,
        ),
    };
    let result = RouteResult {
        org_id: req.org_id.clone(),
        generation: Some(req.generation),
        route: Some(req.route),
        order: req.order.clone(),
        agent_run_id,
        outcome,
    };
    let _ = ctx
        .request::<_, ()>(
            contract::route_result_target(&req.org_id, &req.ticket_id),
            Json(result),
        )
        .send();
    Ok(())
}

/// The `ProductAdvisor` route.
pub struct ProductAdvisorRoute {
    deps: Arc<Deps>,
}

impl ProductAdvisorRoute {
    /// The route over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }
}

#[restate_sdk::service(name = "ProductAdvisor", ingress_private = true)]
impl ProductAdvisorRoute {
    /// Answers a product or compatibility question with the agent and reports to the ticket.
    #[handler]
    async fn run(&self, ctx: Context<'_>, input: Json<RouteRequest>) -> Result<(), HandlerError> {
        handle(
            &self.deps,
            &ctx,
            AgentKind::ProductAdvisor,
            input.into_inner(),
        )
        .await
    }
}

/// The `OrderAssistant` route.
pub struct OrderAssistantRoute {
    deps: Arc<Deps>,
}

impl OrderAssistantRoute {
    /// The route over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }
}

#[restate_sdk::service(name = "OrderAssistant", ingress_private = true)]
impl OrderAssistantRoute {
    /// Answers a question about the customer's orders or the shop's policies (read-only) and
    /// reports to the ticket.
    #[handler]
    async fn run(&self, ctx: Context<'_>, input: Json<RouteRequest>) -> Result<(), HandlerError> {
        handle(
            &self.deps,
            &ctx,
            AgentKind::OrderAssistant,
            input.into_inner(),
        )
        .await
    }
}

// SQLite-backed: native only (the wasm test build skips it).
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
