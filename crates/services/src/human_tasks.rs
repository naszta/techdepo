//! `HumanTasks` (service): the one queue for every decision a person has to make
//! (support.md §8).
//!
//! - `create` (internal) writes a `human_tasks` row and schedules the SLA nudge (a delayed
//!   one-way call to `nudge`). Only the kinds a workflow waits on carry an awakeable
//!   (`HumanTaskKind::has_waiter`: refund approval, investigation decision); the caller creates
//!   the awakeable and passes its ID, which never leaves the server.
//! - `decide` (ingress, admin inbox) validates the decision against the task's allowed set,
//!   records who decided, then resolves the waiting workflow's awakeable with a
//!   [`TaskResolution`] or, for a ticket-owned task, hands the decision to
//!   `SupportTicket/task_decided` ([`crate::support`]). It is the only path that resolves a task's
//!   awakeable, so the browser never sees awakeable IDs. A second decision is
//!   `409 already_decided`.
//! - `nudge` (internal, delayed) emails the org's admins once when a task is still pending at its
//!   SLA; the workflow keeps waiting.
//! - `refresh` (internal) marks a pending ticket task as having a new customer message.
//! - `cancel` (internal) withdraws a pending task (a person acted on the ticket instead).

use std::sync::Arc;
use std::time::Duration;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::ingress::{DecideTaskInput, TaskDecided};
use techdepo_api_types::tasks::{DecideTaskRequest, HumanDecision, TaskContext};
use techdepo_api_types::vocab::{HumanTaskKind, HumanTaskStatus};
use techdepo_api_types::{ApiError, ErrorCode, Evidence};
use techdepo_db::OrgScope;
use techdepo_db::tasks::NewTask;
use techdepo_domain::{
    HumanTaskId, InvestigationId, MAX_SETTINGS_DURATION_SECS, OrderId, OrgId, RefundId, TicketId,
    UserId,
};
use time::OffsetDateTime;

use crate::Deps;
use crate::errors::{internal, not_found, reject, sql};
use crate::notifications::{NotificationsClient, TaskNudgeInput};
use crate::util::{self, mint};

/// `HumanTasks/create` input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTaskInput {
    /// Org.
    pub org_id: OrgId,
    /// What to decide (its kind is the task's kind).
    pub context: TaskContext,
    /// Inbox title, e.g. `Approve €229.90 refund for TD-1042`.
    pub title: String,
    /// One-line summary.
    pub summary: String,
    /// Evidence gathered by code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
    /// The waiting workflow's awakeable (exactly for `has_waiter` kinds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awakeable_id: Option<String>,
    /// Ticket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_id: Option<TicketId>,
    /// Order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_id: Option<OrderId>,
    /// Refund.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refund_id: Option<RefundId>,
    /// Investigation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub investigation_id: Option<InvestigationId>,
}

/// What a waiting workflow's awakeable resolves to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskResolution {
    /// The task.
    pub task_id: HumanTaskId,
    /// The decision.
    pub decision: HumanDecision,
    /// Note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Who decided.
    pub decided_by: UserId,
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub decided_at: OffsetDateTime,
}

/// `HumanTasks/nudge` and `HumanTasks/cancel` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRefInput {
    /// Org.
    pub org_id: OrgId,
    /// Task.
    pub task_id: HumanTaskId,
}

/// `HumanTasks/refresh` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshTaskInput {
    /// Org.
    pub org_id: OrgId,
    /// Task.
    pub task_id: HumanTaskId,
    /// The new one-line summary.
    pub summary: String,
}

/// The task row facts `decide` needs (journaled; carries the awakeable, so never returned).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Loaded {
    kind: HumanTaskKind,
    ticket_id: Option<TicketId>,
    awakeable_id: Option<String>,
}

/// `409 already_decided`.
fn already_decided() -> HandlerError {
    reject(ApiError::new(
        ErrorCode::AlreadyDecided,
        "This task was already decided",
    ))
}

/// Checks a decision against the task (kind, allowed decisions, text lengths) and its state.
pub fn check_decision(
    kind: HumanTaskKind,
    status: HumanTaskStatus,
    decision: &HumanDecision,
    note: Option<&str>,
) -> Result<(), ApiError> {
    match status {
        HumanTaskStatus::Pending => {}
        HumanTaskStatus::Decided => {
            return Err(ApiError::new(
                ErrorCode::AlreadyDecided,
                "This task was already decided",
            ));
        }
        HumanTaskStatus::Cancelled => {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "This task is no longer needed",
            ));
        }
    }
    DecideTaskRequest {
        decision: decision.clone(),
        note: note.map(str::to_owned),
    }
    .validate_for(kind)
}

/// The `HumanTasks` service.
pub struct HumanTasks {
    deps: Arc<Deps>,
}

impl HumanTasks {
    /// The service over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }
}

#[restate_sdk::service(name = "HumanTasks")]
impl HumanTasks {
    /// Creates a task and schedules its SLA nudge. Returns the task ID.
    #[handler(ingress_private = true)]
    async fn create(
        &self,
        mut ctx: Context<'_>,
        input: Json<CreateTaskInput>,
    ) -> Result<Json<HumanTaskId>, HandlerError> {
        let input = input.into_inner();
        let kind = input.context.kind();
        if kind.has_waiter() != input.awakeable_id.is_some() {
            return Err(internal(format!(
                "a {} task {} an awakeable",
                kind.as_str(),
                if kind.has_waiter() {
                    "needs"
                } else {
                    "must not have"
                }
            )));
        }
        let at = util::now(&ctx).await?;
        let id = HumanTaskId::new(mint("tsk", ctx.rand_uuid()));
        let db = self.deps.db.clone();
        let task_id = id.clone();
        let org_id = input.org_id.clone();
        let sla_secs: u64 = ctx
            .run(|| async move {
                let scope = OrgScope::verified(input.org_id.clone());
                let sla = db
                    .orgs(&scope)
                    .settings()
                    .await
                    .map_err(sql)?
                    .human_sla_secs
                    .min(MAX_SETTINGS_DURATION_SECS);
                let task = NewTask {
                    id: task_id,
                    kind,
                    title: input.title,
                    summary: input.summary,
                    context: input.context,
                    evidence: input.evidence,
                    awakeable_id: input.awakeable_id,
                    // Clamped above (settings stored before the cap); never overflow the date
                    // range either way.
                    sla_due_at: at
                        .checked_add(time::Duration::seconds(i64::try_from(sla).unwrap_or(0)))
                        .unwrap_or(at),
                    ticket_id: input.ticket_id,
                    order_id: input.order_id,
                    refund_id: input.refund_id,
                    investigation_id: input.investigation_id,
                    created_at: at,
                };
                let tasks = db.tasks(&scope);
                db.batch(&[tasks.insert_statement(&task).map_err(sql)?])
                    .await
                    .map_err(sql)?;
                Ok(sla)
            })
            .name("create-task")
            .await?;
        let _ = ctx
            .service_client::<HumanTasksClient>()
            .nudge(Json(TaskRefInput {
                org_id,
                task_id: id.clone(),
            }))
            .send_after(Duration::from_secs(sla_secs.max(1)));
        Ok(Json(id))
    }

    /// Records a person's decision and wakes whoever waits on it.
    #[handler]
    async fn decide(
        &self,
        ctx: Context<'_>,
        input: Json<DecideTaskInput>,
    ) -> Result<Json<TaskDecided>, HandlerError> {
        let input = input.into_inner();
        let at = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        let req = input.clone();
        let loaded: Json<Loaded> = ctx
            .run(|| async move {
                let scope = OrgScope::verified(req.org_id.clone());
                let tasks = db.tasks(&scope);
                let task = tasks
                    .get(&req.task_id)
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| not_found("task"))?;
                // A retry of this step after the update committed finds our own decision (same
                // person, same decision, this invocation's journaled time): carry on. A second
                // request, even an identical one, is `409 already_decided`, so a double click
                // never resolves or forwards twice.
                let ours = task.status == HumanTaskStatus::Decided
                    && task.decided_by.as_ref() == Some(&req.by)
                    && task.decision_detail.as_ref() == Some(&req.decision)
                    && task.decided_at.map(millis) == Some(millis(at));
                if !ours {
                    check_decision(task.kind, task.status, &req.decision, req.note.as_deref())
                        .map_err(reject)?;
                    let statement = tasks
                        .decide_statement(
                            &task.id,
                            &req.decision,
                            req.decision.kind(),
                            req.note.as_deref(),
                            &req.by,
                            at,
                        )
                        .map_err(sql)?;
                    let results = db.batch(&[statement]).await.map_err(sql)?;
                    if techdepo_db::rows_affected(&results, 0) == 0 {
                        return Err(already_decided());
                    }
                }
                let awakeable_id = tasks.awakeable_id(&task.id).await.map_err(sql)?;
                Ok(Json(Loaded {
                    kind: task.kind,
                    ticket_id: task.ticket_id,
                    awakeable_id,
                }))
            })
            .name("record-decision")
            .await?;
        let loaded = loaded.into_inner();
        if loaded.kind.has_waiter() {
            let awakeable = loaded
                .awakeable_id
                .ok_or_else(|| internal("task without its awakeable"))?;
            ctx.resolve_awakeable(
                &awakeable,
                Json(TaskResolution {
                    task_id: input.task_id.clone(),
                    decision: input.decision.clone(),
                    note: input.note.clone(),
                    decided_by: input.by.clone(),
                    decided_at: at,
                }),
            );
        } else if let Some(ticket) = &loaded.ticket_id {
            crate::support::task_decided(
                &ctx,
                &input.org_id,
                ticket,
                crate::support::TaskDecidedInput {
                    org_id: input.org_id.clone(),
                    task_id: input.task_id.clone(),
                    kind: loaded.kind,
                    decision: input.decision.clone(),
                    note: input.note.clone(),
                    by: input.by.clone(),
                },
            );
        }
        Ok(Json(TaskDecided {
            task_id: input.task_id,
            status: HumanTaskStatus::Decided,
        }))
    }

    /// The SLA passed: if the task is still pending, nudge the admins (once).
    #[handler(ingress_private = true)]
    async fn nudge(
        &self,
        ctx: Context<'_>,
        input: Json<TaskRefInput>,
    ) -> Result<bool, HandlerError> {
        let input = input.into_inner();
        let at = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        let req = input.clone();
        let nudged: bool = ctx
            .run(|| async move {
                let scope = OrgScope::verified(req.org_id.clone());
                let tasks = db.tasks(&scope);
                let results = db
                    .batch(&[tasks.nudge_statement(&req.task_id, at)])
                    .await
                    .map_err(sql)?;
                Ok(techdepo_db::rows_affected(&results, 0) > 0)
            })
            .name("mark-nudged")
            .await?;
        if nudged {
            let _ = ctx
                .service_client::<NotificationsClient>()
                .task_nudge(Json(TaskNudgeInput {
                    org_id: input.org_id,
                    task_id: input.task_id,
                }))
                .send();
        }
        Ok(nudged)
    }

    /// The customer wrote again on a ticket a pending task is about: new summary, flagged as a
    /// new customer message. Returns whether it was pending.
    #[handler(ingress_private = true)]
    async fn refresh(
        &self,
        ctx: Context<'_>,
        input: Json<RefreshTaskInput>,
    ) -> Result<bool, HandlerError> {
        let input = input.into_inner();
        let at = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        ctx.run(|| async move {
            let scope = OrgScope::verified(input.org_id.clone());
            let statement = db
                .tasks(&scope)
                .refresh_statement(&input.task_id, &input.summary, at);
            let results = db.batch(&[statement]).await.map_err(sql)?;
            Ok(techdepo_db::rows_affected(&results, 0) > 0)
        })
        .name("refresh-task")
        .await
        .map_err(Into::into)
    }

    /// Withdraws a pending task. Returns whether it was pending.
    #[handler(ingress_private = true)]
    async fn cancel(
        &self,
        ctx: Context<'_>,
        input: Json<TaskRefInput>,
    ) -> Result<bool, HandlerError> {
        let input = input.into_inner();
        let at = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        ctx.run(|| async move {
            let scope = OrgScope::verified(input.org_id.clone());
            let results = db
                .batch(&[db.tasks(&scope).cancel_statement(&input.task_id, at)])
                .await
                .map_err(sql)?;
            Ok(techdepo_db::rows_affected(&results, 0) > 0)
        })
        .name("cancel-task")
        .await
        .map_err(Into::into)
    }
}

/// Epoch milliseconds, the precision `human_tasks.decided_at` is stored at.
fn millis(at: OffsetDateTime) -> i128 {
    at.unix_timestamp_nanos().div_euclid(1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions_are_checked() {
        let kind = HumanTaskKind::ApproveRefund;
        assert!(
            check_decision(
                kind,
                HumanTaskStatus::Pending,
                &HumanDecision::Approve,
                None
            )
            .is_ok()
        );
        let wrong =
            check_decision(kind, HumanTaskStatus::Pending, &HumanDecision::Send, None).unwrap_err();
        assert_eq!(wrong.code, ErrorCode::Validation);
        let twice = check_decision(
            kind,
            HumanTaskStatus::Decided,
            &HumanDecision::Approve,
            None,
        )
        .unwrap_err();
        assert_eq!(twice.code, ErrorCode::AlreadyDecided);
        assert_eq!(twice.status(), 409);
        let empty = check_decision(
            kind,
            HumanTaskStatus::Pending,
            &HumanDecision::Reject { reason: " ".into() },
            None,
        );
        assert!(empty.is_err(), "a rejection needs a reason");
    }
}
