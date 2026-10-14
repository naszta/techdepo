//! The human tasks inbox. `HumanTasks/decide` is the only path that resolves a task's
//! awakeable; the browser never sees awakeable IDs.

use techdepo_api_types::endpoints::{DecideTask, GetTask, ListTasks};
use techdepo_api_types::ingress::{DecideTaskInput, HumanTasksDecide};
use techdepo_api_types::tasks::{HumanTaskDetail, HumanTaskSummary};
use techdepo_api_types::vocab::HumanTaskStatus;
use techdepo_api_types::{HumanTaskId, Page};
use techdepo_db::PageRequest;
use techdepo_db::tasks::TaskRow;

use crate::app::{App, now};
use crate::assemble;
use crate::auth::Caller;
use crate::endpoint::Input;
use crate::error::{ApiResult, DbResultExt, FoundExt};

async fn task(app: &App, caller: &Caller, id: &str) -> ApiResult<TaskRow> {
    app.db
        .tasks(&caller.scope())
        .get(&HumanTaskId::new(id))
        .await
        .found(format!("Task {id}"))
}

pub async fn list(
    app: App,
    caller: Caller,
    input: Input<ListTasks>,
) -> ApiResult<Page<HumanTaskSummary>> {
    let q = input.query;
    let scope = caller.scope();
    let page = PageRequest::from_query(q.cursor.as_deref(), q.limit);
    let (rows, next_cursor) = app
        .db
        .tasks(&scope)
        .list(q.status.unwrap_or(HumanTaskStatus::Pending), q.kind, &page)
        .await
        .db()?;
    Ok(Page {
        items: assemble::task_summaries(&app, &scope, &rows, now()).await?,
        next_cursor,
    })
}

pub async fn detail(app: App, caller: Caller, input: Input<GetTask>) -> ApiResult<HumanTaskDetail> {
    let row = task(&app, &caller, input.param(0)).await?;
    assemble::task_detail(&app, &caller.scope(), &row, now()).await
}

/// Validates the decision against the task's kind, then `HumanTasks/decide` records it and
/// resolves the waiting workflow (or forwards it to the ticket).
pub async fn decide(
    app: App,
    caller: Caller,
    input: Input<DecideTask>,
) -> ApiResult<HumanTaskDetail> {
    let row = task(&app, &caller, input.param(0)).await?;
    let req = input.body;
    req.validate_for(row.kind)?;
    let decide = DecideTaskInput {
        org_id: caller.org_id().clone(),
        task_id: row.id.clone(),
        by: caller.user_id.clone(),
        decision: req.decision,
        note: req.note,
    };
    app.ingress
        .call::<HumanTasksDecide>(None, &decide, None)
        .await?;
    let row = task(&app, &caller, row.id.as_str()).await?;
    assemble::task_detail(&app, &caller.scope(), &row, now()).await
}
