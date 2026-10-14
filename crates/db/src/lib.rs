//! Persistence for TechDepó: typed repositories over the [`SqlExecutor`] port, plus the D1
//! backend.
//!
//! - **Repositories** are plain SQL over [`SqlExecutor`], so the same code runs on D1 (wasm) and,
//!   later, on rusqlite (native). They are shared by `crates/services` (writes, inside
//!   `ctx.run`) and the web worker (reads).
//! - **Scoping:** every org-owned table is reached through an [`OrgScope`]; every query filters
//!   on its `org_id`. Repositories that are scoped to a customer as well (cart, builds, addresses,
//!   orders, tickets) take the `UserId` explicitly and filter on it too.
//! - **Writes are statements.** Each repository exposes `*_statement(s)` builders returning
//!   [`Statement`]s, so callers can combine several changes into one atomic
//!   [`SqlExecutor::batch`], plus convenience methods that execute a single change. Writes are
//!   idempotent (deterministic keys, `ON CONFLICT`, upserts; see `migrations/README.md`), because
//!   a `ctx.run` may execute twice.
//! - **Views** ([`views`]) turn rows into the `techdepo_api_types` DTOs. They are pure, so the web
//!   worker and the services price a cart, list valid demo actions, etc. the same way.
//! - **D1** ([`D1Executor`], wasm only) implements [`SqlExecutor`] for a Workers D1 binding.
//!
//! ```ignore
//! let db = Db::new(Arc::new(D1Executor::new(env.d1("DB")?)));
//! let scope = OrgScope::verified(org_id);           // from the verified session
//! let order = db.orders(&scope).get(&order_id).await?;
//! ```

#[cfg(target_arch = "wasm32")]
mod d1;
mod page;
mod retry;
mod scope;
#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "sqlite")))]
mod sqlite;

pub mod addresses;
pub mod agent_runs;
pub mod app_settings;
pub mod builds;
pub mod carts;
pub mod email_log;
pub mod inventory;
pub mod investigations;
pub mod invoices;
pub mod orders;
pub mod orgs;
pub mod payments;
pub mod refunds;
pub mod shipments;
pub mod tasks;
pub mod tickets;
pub mod views;

use std::sync::Arc;

#[cfg(target_arch = "wasm32")]
pub use d1::D1Executor;
pub use page::{Cursor, PageRequest};
pub use retry::{Retrying, sleep};
pub use scope::OrgScope;
#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "sqlite")))]
pub use sqlite::{MIGRATIONS, SqliteExecutor};
pub use techdepo_ports::{
    BatchResult, Param, Row, SqlError, SqlExecutor, SqlExecutorExt, Statement,
};

/// A cheap, cloneable handle to the database: an `Arc<dyn SqlExecutor>` plus repository
/// constructors.
#[derive(Clone)]
pub struct Db {
    sql: Arc<dyn SqlExecutor>,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db").finish_non_exhaustive()
    }
}

impl Db {
    /// Wraps an executor.
    pub fn new(sql: Arc<dyn SqlExecutor>) -> Self {
        Self { sql }
    }

    /// The executor, for ad-hoc queries and batches.
    pub fn sql(&self) -> &dyn SqlExecutor {
        &*self.sql
    }

    /// The shared executor (for components that keep their own handle, such as the LLM usage
    /// ledger).
    pub fn executor(&self) -> Arc<dyn SqlExecutor> {
        self.sql.clone()
    }

    /// Runs `statements` as one transaction.
    pub async fn batch(&self, statements: &[Statement]) -> Result<Vec<BatchResult>, SqlError> {
        if statements.is_empty() {
            return Ok(Vec::new());
        }
        self.sql.batch(statements).await
    }

    /// The operator's app-wide settings (not tenant data: no scope).
    pub fn app_settings(&self) -> app_settings::AppSettings<'_> {
        app_settings::AppSettings::new(self.sql())
    }

    /// Orgs, settings and the member cache.
    pub fn orgs<'a>(&'a self, scope: &'a OrgScope) -> orgs::Orgs<'a> {
        orgs::Orgs::new(self.sql(), scope)
    }

    /// Per-org stock.
    pub fn inventory<'a>(&'a self, scope: &'a OrgScope) -> inventory::Inventory<'a> {
        inventory::Inventory::new(self.sql(), scope)
    }

    /// Carts.
    pub fn carts<'a>(&'a self, scope: &'a OrgScope) -> carts::Carts<'a> {
        carts::Carts::new(self.sql(), scope)
    }

    /// Saved builds.
    pub fn builds<'a>(&'a self, scope: &'a OrgScope) -> builds::Builds<'a> {
        builds::Builds::new(self.sql(), scope)
    }

    /// Address book.
    pub fn addresses<'a>(&'a self, scope: &'a OrgScope) -> addresses::Addresses<'a> {
        addresses::Addresses::new(self.sql(), scope)
    }

    /// Orders, items, events, demo faults and the order clock.
    pub fn orders<'a>(&'a self, scope: &'a OrgScope) -> orders::Orders<'a> {
        orders::Orders::new(self.sql(), scope)
    }

    /// The payments ledger.
    pub fn payments<'a>(&'a self, scope: &'a OrgScope) -> payments::Payments<'a> {
        payments::Payments::new(self.sql(), scope)
    }

    /// Invoices and credit notes.
    pub fn invoices<'a>(&'a self, scope: &'a OrgScope) -> invoices::Invoices<'a> {
        invoices::Invoices::new(self.sql(), scope)
    }

    /// Shipments and carrier events.
    pub fn shipments<'a>(&'a self, scope: &'a OrgScope) -> shipments::Shipments<'a> {
        shipments::Shipments::new(self.sql(), scope)
    }

    /// Carrier investigations (read side).
    pub fn investigations<'a>(&'a self, scope: &'a OrgScope) -> investigations::Investigations<'a> {
        investigations::Investigations::new(self.sql(), scope)
    }

    /// Human tasks.
    pub fn tasks<'a>(&'a self, scope: &'a OrgScope) -> tasks::Tasks<'a> {
        tasks::Tasks::new(self.sql(), scope)
    }

    /// Refunds.
    pub fn refunds<'a>(&'a self, scope: &'a OrgScope) -> refunds::Refunds<'a> {
        refunds::Refunds::new(self.sql(), scope)
    }

    /// The email log.
    pub fn email_log<'a>(&'a self, scope: &'a OrgScope) -> email_log::EmailLog<'a> {
        email_log::EmailLog::new(self.sql(), scope)
    }

    /// Agent and classifier traces (read side; the support pipeline writes them).
    pub fn agent_runs<'a>(&'a self, scope: &'a OrgScope) -> agent_runs::AgentRuns<'a> {
        agent_runs::AgentRuns::new(self.sql(), scope)
    }

    /// Tickets (read side; the `SupportTicket` VO writes them).
    pub fn tickets<'a>(&'a self, scope: &'a OrgScope) -> tickets::Tickets<'a> {
        tickets::Tickets::new(self.sql(), scope)
    }
}

/// Rows a write changed, from a batch result (0 when absent).
pub fn rows_affected(results: &[BatchResult], index: usize) -> u64 {
    results.get(index).map_or(0, |r| r.rows_affected)
}

/// Declares a repository struct borrowing an executor and an [`OrgScope`].
macro_rules! repository {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy)]
        pub struct $name<'a> {
            sql: &'a dyn $crate::SqlExecutor,
            scope: &'a $crate::OrgScope,
        }

        impl<'a> $name<'a> {
            /// The repository for `scope`.
            pub fn new(sql: &'a dyn $crate::SqlExecutor, scope: &'a $crate::OrgScope) -> Self {
                Self { sql, scope }
            }

            /// The org every query filters on.
            pub fn org(&self) -> &'a techdepo_domain::OrgId {
                self.scope.org_id()
            }
        }
    };
}
pub(crate) use repository;

/// Statement shorthand.
pub(crate) fn stmt(sql: &'static str, params: Vec<Param>) -> Statement {
    Statement::new(sql, params)
}

/// `Param::json`, mapping the (impossible for our types) error to a decode error.
pub(crate) fn json_param<T: serde::Serialize + ?Sized>(value: &T) -> Result<Param, SqlError> {
    Param::json(value).map_err(|e| SqlError::Decode(e.to_string()))
}
