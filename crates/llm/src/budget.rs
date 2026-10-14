//! Token accounting and the daily budgets (`llm_usage`, support.md §9).
//!
//! Every model and classifier call adds its tokens to `llm_usage` (per org, UTC day and model,
//! an upsert-increment: a retried step may count twice, which budgets tolerate). Before an
//! agent's model call, [`OrgBudget`] (the agent loop's [`Budget`]) checks the org's daily
//! budget and the global one. Classifier calls are recorded but not gated: triage must always
//! run, and a spent budget then shows up as agents escalating.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use techdepo_agent::{Budget, BudgetCheck, StepError};
use techdepo_domain::OrgId;
use techdepo_ports::{SqlError, SqlExecutor, SqlExecutorExt, Statement, Usage, params};
use time::Date;

/// Daily token limits; `0` means unlimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetLimits {
    /// Tokens per org per UTC day (`LLM_DAILY_TOKEN_BUDGET_PER_ORG`).
    pub per_org_daily_tokens: u64,
    /// Tokens over all orgs per UTC day (`LLM_DAILY_TOKEN_BUDGET_GLOBAL`).
    pub global_daily_tokens: u64,
}

impl Default for BudgetLimits {
    fn default() -> Self {
        Self {
            per_org_daily_tokens: 200_000,
            global_daily_tokens: 5_000_000,
        }
    }
}

/// Tokens used on one day.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct DailyUsage {
    /// By the org.
    pub org_tokens: u64,
    /// By everyone.
    pub global_tokens: u64,
}

impl BudgetLimits {
    /// Whether `usage` leaves room for another call.
    pub fn check(&self, usage: DailyUsage) -> BudgetCheck {
        if self.per_org_daily_tokens > 0 && usage.org_tokens >= self.per_org_daily_tokens {
            return BudgetCheck::Exhausted {
                reason: format!(
                    "the organization's daily AI budget ({} tokens) is used up",
                    self.per_org_daily_tokens
                ),
            };
        }
        if self.global_daily_tokens > 0 && usage.global_tokens >= self.global_daily_tokens {
            return BudgetCheck::Exhausted {
                reason: "the demo's global daily AI budget is used up".to_owned(),
            };
        }
        BudgetCheck::Allowed
    }
}

/// `llm_usage`: reads the day's totals, records calls.
#[derive(Clone)]
pub struct UsageLedger {
    sql: Arc<dyn SqlExecutor>,
    limits: BudgetLimits,
}

impl std::fmt::Debug for UsageLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsageLedger")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl UsageLedger {
    /// A ledger over `sql` enforcing `limits`.
    pub fn new(sql: Arc<dyn SqlExecutor>, limits: BudgetLimits) -> Self {
        Self { sql, limits }
    }

    /// The limits.
    pub fn limits(&self) -> BudgetLimits {
        self.limits
    }

    /// Tokens used on `day` by `org` and by everyone.
    pub async fn used(&self, org: &OrgId, day: Date) -> Result<DailyUsage, SqlError> {
        let usage = self
            .sql
            .query_first_as::<DailyUsage>(
                "SELECT \
                   COALESCE(SUM(CASE WHEN org_id = ?1 THEN input_tokens + output_tokens ELSE 0 END), 0) AS org_tokens, \
                   COALESCE(SUM(input_tokens + output_tokens), 0) AS global_tokens \
                 FROM llm_usage WHERE day = ?2",
                &params![org, day],
            )
            .await?;
        Ok(usage.unwrap_or_default())
    }

    /// Whether `org` may make another call on `day`.
    pub async fn check(&self, org: &OrgId, day: Date) -> Result<BudgetCheck, SqlError> {
        Ok(self.limits.check(self.used(org, day).await?))
    }

    /// Adds one call's `usage` (for a `Db::batch`).
    pub fn record_statement(org: &OrgId, day: Date, model: &str, usage: Usage) -> Statement {
        Statement::new(
            "INSERT INTO llm_usage (org_id, day, model, calls, input_tokens, output_tokens) \
             VALUES (?1, ?2, ?3, 1, ?4, ?5) \
             ON CONFLICT (org_id, day, model) DO UPDATE SET \
               calls = calls + 1, \
               input_tokens = input_tokens + excluded.input_tokens, \
               output_tokens = output_tokens + excluded.output_tokens",
            params![org, day, model, usage.input_tokens, usage.output_tokens],
        )
    }

    /// Adds one call's `usage` now.
    pub async fn record(
        &self,
        org: &OrgId,
        day: Date,
        model: &str,
        usage: Usage,
    ) -> Result<(), SqlError> {
        let statement = Self::record_statement(org, day, model, usage);
        self.sql.execute(&statement.sql, &statement.params).await?;
        Ok(())
    }

    /// The agent loop's budget for `org` on `day` (the UTC date of the handler's journaled
    /// `ctx.now()`).
    pub fn for_org(&self, org: OrgId, day: Date) -> OrgBudget {
        OrgBudget {
            ledger: self.clone(),
            org,
            day,
        }
    }
}

/// One org's budget on one day: the agent loop's [`Budget`].
#[derive(Debug, Clone)]
pub struct OrgBudget {
    ledger: UsageLedger,
    org: OrgId,
    day: Date,
}

fn step_error(error: SqlError) -> StepError {
    StepError {
        retryable: error.is_retryable(),
        message: error.to_string(),
    }
}

#[async_trait]
impl Budget for OrgBudget {
    async fn check(&self, _model: &str) -> Result<BudgetCheck, StepError> {
        self.ledger
            .check(&self.org, self.day)
            .await
            .map_err(step_error)
    }

    async fn record(&self, model: &str, usage: Usage) -> Result<(), StepError> {
        self.ledger
            .record(&self.org, self.day, model, usage)
            .await
            .map_err(step_error)
    }
}

// SQLite-backed: native only (the wasm test build skips it).
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use futures::executor::block_on;
    use techdepo_db::SqliteExecutor;
    use time::macros::date;

    use super::*;

    fn ledger(limits: BudgetLimits) -> (UsageLedger, Arc<dyn SqlExecutor>) {
        let sql: Arc<dyn SqlExecutor> = Arc::new(SqliteExecutor::in_memory().unwrap());
        for (id, public) in [("org_a", "aaaaaaaa"), ("org_b", "bbbbbbbb")] {
            block_on(sql.execute(
                "INSERT INTO orgs (id, public_id, name, created_at, updated_at) VALUES (?1, ?2, ?1, 0, 0)",
                &params![id, public],
            ))
            .unwrap();
        }
        (UsageLedger::new(sql.clone(), limits), sql)
    }

    fn usage(input_tokens: u32, output_tokens: u32) -> Usage {
        Usage {
            input_tokens,
            output_tokens,
        }
    }

    #[test]
    fn records_and_enforces_daily_budgets() {
        let limits = BudgetLimits {
            per_org_daily_tokens: 1_000,
            global_daily_tokens: 1_500,
        };
        let (ledger, sql) = ledger(limits);
        let (a, b) = (OrgId::new("org_a"), OrgId::new("org_b"));
        let today = date!(2026 - 10 - 14);

        let budget = ledger.for_org(a.clone(), today);
        assert_eq!(block_on(budget.check("m")), Ok(BudgetCheck::Allowed));
        block_on(budget.record("m", usage(600, 100))).unwrap();
        block_on(budget.record("m", usage(250, 50))).unwrap();
        block_on(ledger.record(&b, today, "clef", usage(400, 0))).unwrap();
        // Yesterday does not count.
        block_on(ledger.record(&b, date!(2026 - 10 - 13), "m", usage(9_000, 0))).unwrap();

        assert_eq!(
            block_on(ledger.used(&a, today)).unwrap(),
            DailyUsage {
                org_tokens: 1_000,
                global_tokens: 1_400
            }
        );
        let calls: Option<u32> = block_on(sql.query_scalar(
            "SELECT calls FROM llm_usage WHERE org_id = 'org_a' AND model = 'm'",
            &[],
        ))
        .unwrap();
        assert_eq!(calls, Some(2));

        assert!(matches!(
            block_on(budget.check("m")),
            Ok(BudgetCheck::Exhausted { reason }) if reason.contains("organization")
        ));
        assert_eq!(
            block_on(ledger.check(&b, today)).unwrap(),
            BudgetCheck::Allowed
        );
        block_on(ledger.record(&b, today, "clef", usage(100, 0))).unwrap();
        assert!(matches!(
            block_on(ledger.check(&b, today)).unwrap(),
            BudgetCheck::Exhausted { reason } if reason.contains("global")
        ));
    }

    #[test]
    fn zero_means_unlimited() {
        let limits = BudgetLimits {
            per_org_daily_tokens: 0,
            global_daily_tokens: 0,
        };
        assert_eq!(
            limits.check(DailyUsage {
                org_tokens: u64::MAX,
                global_tokens: u64::MAX
            }),
            BudgetCheck::Allowed
        );
    }
}
