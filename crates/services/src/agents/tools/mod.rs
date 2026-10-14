//! The agents' tools (support.md §5, §7). All read-only.
//!
//! **Tenant scoping is by construction:** every tool gets its org and customer from the
//! [`ToolCx`], which the route fills from the ticket, never from the model's arguments. An order
//! number that is not the customer's is "not found", exactly like one that does not exist.
//!
//! **Durability:** tools that read D1 run inside a journaled step of the agent's
//! [`Runtime`] (`tool-<n>-<name>`), so a retried or replayed invocation sees the same data the
//! model saw. Pure tools (the compiled-in catalog, the compat engine, the knowledge base) run
//! directly: they are deterministic.
//!
//! | Tool | Agent | Reads |
//! |---|---|---|
//! | `search_products`, `get_product`, `check_compatibility`, `find_alternatives` | ProductAdvisor | catalog, compat engine |
//! | `get_my_builds` | ProductAdvisor | D1 `builds` |
//! | `list_guides`, `read_guide` | ProductAdvisor | knowledge base |
//! | `list_my_orders`, `get_order`, `get_tracking`, `get_invoices` | OrderAssistant | D1 orders, shipments, invoices |
//! | `list_policies`, `read_policy` | OrderAssistant | knowledge base |

pub mod account;
pub mod catalog;
pub mod knowledge;

use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};

use serde::Serialize;
use serde::de::DeserializeOwned;
use techdepo_agent::{Runtime, StepError, ToolError, ToolSet};
use techdepo_db::{Db, OrgScope, SqlError};
use techdepo_domain::{TicketId, UserId};

/// What every tool runs with: the journal, the database and the tenant scope, all filled by
/// code from the ticket.
pub struct ToolCx<'a, R> {
    rt: &'a R,
    db: Db,
    scope: OrgScope,
    customer: UserId,
    ticket: TicketId,
    reads: AtomicU32,
}

impl<'a, R: Runtime> ToolCx<'a, R> {
    /// Tools for `customer` of `scope`'s org, on `ticket`.
    pub fn new(rt: &'a R, db: Db, scope: OrgScope, customer: UserId, ticket: TicketId) -> Self {
        Self {
            rt,
            db,
            scope,
            customer,
            ticket,
            reads: AtomicU32::new(0),
        }
    }

    /// The customer the tools act for.
    pub fn customer(&self) -> &UserId {
        &self.customer
    }

    /// The ticket.
    pub fn ticket(&self) -> &TicketId {
        &self.ticket
    }

    /// Runs `read` (D1, scoped to the org and the customer) as the journaled step
    /// `tool-<n>-<tool>`. A step that fails for good is fatal for the run (the agent escalates).
    pub async fn read<T, F, Fut>(&self, tool: &str, read: F) -> Result<T, ToolError>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
        F: FnOnce(Db, OrgScope, UserId) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, SqlError>> + Send + 'static,
    {
        let n = self.reads.fetch_add(1, Ordering::Relaxed);
        let (db, scope, customer) = (self.db.clone(), self.scope.clone(), self.customer.clone());
        self.rt
            .step::<T>(
                &format!("tool-{n}-{tool}"),
                Box::new(move || {
                    Box::pin(async move {
                        read(db, scope, customer).await.map_err(|e| StepError {
                            retryable: e.is_retryable(),
                            message: e.to_string(),
                        })
                    })
                }),
            )
            .await
            .map_err(|e| ToolError::Fatal(format!("could not read the shop's data: {}", e.message)))
    }
}

/// `ProductAdvisor`'s tools.
pub fn product_advisor<'a, R: Runtime>() -> ToolSet<ToolCx<'a, R>> {
    ToolSet::new()
        .with(catalog::SearchProducts)
        .with(catalog::GetProduct)
        .with(catalog::CheckCompatibility)
        .with(catalog::FindAlternatives)
        .with(account::GetMyBuilds)
        .with(knowledge::ListGuides)
        .with(knowledge::ReadGuide)
}

/// `OrderAssistant`'s tools (read-only; actions are rerouted).
pub fn order_assistant<'a, R: Runtime>() -> ToolSet<ToolCx<'a, R>> {
    ToolSet::new()
        .with(account::ListMyOrders)
        .with(account::GetOrder)
        .with(account::GetTracking)
        .with(account::GetInvoices)
        .with(knowledge::ListPolicies)
        .with(knowledge::ReadPolicy)
}
