//! Restate service policies (`restate-config`): the defaults in code, overridable per service
//! from the endpoint's JSON configuration (`RESTATE_CONFIG` in `apps/services`).
//!
//! | Service | Policy | Why |
//! |---|---|---|
//! | `Email` | retry every ≤ 15 s, ~2 hours of attempts, then pause | the demo's provider outage lasts up to 60 minutes and must end in a delivery, visibly retried |
//! | `Payments`, `RefundWorkflow` | capped retries, then **pause** | a stuck money step waits for a person instead of being killed |
//! | every service | journal, idempotency and workflow retention of 7 days ([`with_retention`]) | see below |
//!
//! **Retention.** The server defaults keep a completed invocation's journal for a day at most,
//! so the admin pages' links into the Restate UI (`api_types::restate_ui`) would show nothing
//! for last week's order. Every service keeps journals for [`JOURNAL_RETENTION`]. Idempotent calls
//! (`Idempotency-Key`: the web worker's client keys, mail-in's `Message-ID`) and workflow runs
//! keep their result as long ([`IDEMPOTENCY_RETENTION`], [`WORKFLOW_RETENTION`]), because that
//! retention also caps their journal's; it also outlasts a sending MTA's retry window (up to
//! 5 days) for a redelivered email, and keeps a refund or investigation ID from running twice.
//! A policy (or a `RESTATE_CONFIG` override) that sets its own retention keeps it.
//!
//! The agent routes keep the default timeouts: on Workers the endpoint runs in request-response
//! mode, where Restate ignores inactivity timeouts (RT0021); the default 10-minute abort timeout
//! is plenty for a few model calls.
//!
//! Which services are `ingress_private` is declared on the services themselves (the macro
//! attributes), so it holds even without configuration; a policy may still override it.

use std::collections::BTreeMap;
use std::time::Duration;

use restate_config::{RetryPolicyOnMaxAttempts, ServiceOptionsConfig};

/// Service name → policy.
pub type Policies = BTreeMap<String, ServiceOptionsConfig>;

/// How long Restate keeps a completed invocation's journal.
pub const JOURNAL_RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);
/// How long an `Idempotency-Key` deduplicates (and keeps its result and journal).
pub const IDEMPOTENCY_RETENTION: Duration = JOURNAL_RETENTION;
/// How long a finished workflow run keeps its result, state and journal (its ID stays taken).
pub const WORKFLOW_RETENTION: Duration = JOURNAL_RETENTION;

fn retrying(initial: Duration, max_interval: Duration, max_attempts: u64) -> ServiceOptionsConfig {
    ServiceOptionsConfig {
        retry_policy_initial_interval: Some(initial),
        retry_policy_exponentiation_factor: Some(2.0),
        retry_policy_max_interval: Some(max_interval),
        retry_policy_max_attempts: Some(max_attempts),
        retry_policy_on_max_attempts: Some(RetryPolicyOnMaxAttempts::Pause),
        ..ServiceOptionsConfig::default()
    }
}

/// The built-in policies.
pub fn defaults() -> Policies {
    let money = || retrying(Duration::from_secs(1), Duration::from_secs(60), 30);
    let mut policies = Policies::new();
    policies.insert(
        "Email".to_owned(),
        retrying(Duration::from_secs(1), Duration::from_secs(15), 500),
    );
    policies.insert("Payments".to_owned(), money());
    policies.insert("RefundWorkflow".to_owned(), money());
    policies
}

/// `policy` with the retention defaults filled in where it sets none. `workflow_handler` is the
/// entry handler (`run`) of a workflow, `None` for services and objects.
pub fn with_retention(
    mut policy: ServiceOptionsConfig,
    workflow_handler: Option<&str>,
) -> ServiceOptionsConfig {
    policy.journal_retention.get_or_insert(JOURNAL_RETENTION);
    policy
        .idempotency_retention
        .get_or_insert(IDEMPOTENCY_RETENTION);
    if let Some(handler) = workflow_handler {
        policy
            .handlers
            .entry(handler.to_owned())
            .or_default()
            .workflow_retention
            .get_or_insert(WORKFLOW_RETENTION);
    }
    policy
}

/// The defaults with `overrides` on top (an override replaces that service's default).
pub fn merged(overrides: Policies) -> Policies {
    let mut policies = defaults();
    policies.extend(overrides);
    policies
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_fills_only_what_a_policy_leaves_unset() {
        let email = with_retention(defaults()["Email"].clone(), None);
        assert_eq!(email.journal_retention, Some(JOURNAL_RETENTION));
        assert_eq!(email.idempotency_retention, Some(IDEMPOTENCY_RETENTION));
        assert_eq!(email.retry_policy_max_attempts, Some(500));
        assert!(email.handlers.is_empty());

        let short = Duration::from_secs(3600);
        let override_ = ServiceOptionsConfig {
            journal_retention: Some(short),
            ..ServiceOptionsConfig::default()
        };
        let policy = with_retention(override_, Some("run"));
        assert_eq!(policy.journal_retention, Some(short));
        assert_eq!(policy.idempotency_retention, Some(IDEMPOTENCY_RETENTION));
        assert_eq!(
            policy.handlers["run"].workflow_retention,
            Some(WORKFLOW_RETENTION)
        );
    }
}
