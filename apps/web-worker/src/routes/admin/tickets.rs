//! Admin tickets: list, thread with the "why" panel, traces, and human actions
//! (`SupportTicket/human`; `503` when the services are not registered with Restate).

use techdepo_api_types::admin::{
    AdminTicketDetail, AdminTicketSummary, TicketTrace, TicketUpdated,
};
use techdepo_api_types::endpoints::{
    ActOnTicket, GetAdminTicket, GetTicketTrace, ListAdminTickets,
};
use techdepo_api_types::ingress::{HumanTicketInput, SupportTicketHuman, keys};
use techdepo_api_types::vocab::HumanTaskStatus;
use techdepo_api_types::{Page, TicketRef, Validate};
use techdepo_db::tickets::{TicketFilter, TicketRow};
use techdepo_db::{OrgScope, PageRequest, views};

use crate::app::{App, now};
use crate::assemble;
use crate::auth::Caller;
use crate::endpoint::Input;
use crate::error::{ApiResult, DbResultExt, FoundExt};
use crate::routes::support::ticket_id;

async fn ticket(app: &App, caller: &Caller, param: &str) -> ApiResult<TicketRow> {
    let id = ticket_id(param)?;
    app.db
        .tickets(&caller.scope())
        .get(&id)
        .await
        .found(format!("Ticket {id}"))
}

async fn summary(app: &App, scope: &OrgScope, row: &TicketRow) -> ApiResult<AdminTicketSummary> {
    let tasks = app.db.tasks(scope).for_ticket(&row.id).await.db()?;
    Ok(AdminTicketSummary {
        ticket: assemble::ticket_summary(app, scope, row).await?,
        customer: assemble::ticket_customer(app, scope, row).await?,
        assignee: row.assignee,
        intent: row.intent,
        intent_confidence: row.intent_confidence,
        route: row.route,
        pending_tasks: u32::try_from(
            tasks
                .iter()
                .filter(|t| t.status == HumanTaskStatus::Pending)
                .count(),
        )
        .unwrap_or(u32::MAX),
    })
}

pub async fn list(
    app: App,
    caller: Caller,
    input: Input<ListAdminTickets>,
) -> ApiResult<Page<AdminTicketSummary>> {
    let q = input.query;
    let scope = caller.scope();
    let filter = TicketFilter {
        user_id: q.user_id,
        status: q.status,
        open: q.open,
        intent: q.intent,
        assignee: q.assignee,
    };
    let page = PageRequest::from_query(q.cursor.as_deref(), q.limit);
    let (rows, next_cursor) = app.db.tickets(&scope).list(&filter, &page).await.db()?;
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(summary(&app, &scope, row).await?);
    }
    Ok(Page { items, next_cursor })
}

pub async fn detail(
    app: App,
    caller: Caller,
    input: Input<GetAdminTicket>,
) -> ApiResult<AdminTicketDetail> {
    let row = ticket(&app, &caller, input.param(0)).await?;
    let scope = caller.scope();
    let db = &app.db;
    let messages = db.tickets(&scope).messages(&row.id, false).await.db()?;
    let tasks = db.tasks(&scope).for_ticket(&row.id).await.db()?;
    let investigations = db.investigations(&scope).for_ticket(&row.id).await.db()?;
    let refunds = db.refunds(&scope).for_ticket(&row.id).await.db()?;
    let runs = db.agent_runs(&scope).for_ticket(&row.id).await.db()?;
    Ok(AdminTicketDetail {
        ticket: summary(&app, &scope, &row).await?,
        messages: messages.iter().map(views::ticket_message).collect(),
        classification: row.classification.clone(),
        awaiting: row.awaiting,
        escalation_reason: row.escalation_reason.clone(),
        reply_confidence: row.reply_confidence,
        tasks: assemble::task_summaries(&app, &scope, &tasks, now()).await?,
        investigations: assemble::investigation_summaries(&app, &scope, &investigations).await?,
        refunds: refunds.iter().map(assemble::refund_summary).collect(),
        runs: runs.iter().map(|r| r.summary()).collect(),
        reply_address: app.support().ticket_reply(&caller.org.public_id, &row.id),
    })
}

pub async fn trace(
    app: App,
    caller: Caller,
    input: Input<GetTicketTrace>,
) -> ApiResult<TicketTrace> {
    let row = ticket(&app, &caller, input.param(0)).await?;
    let scope = caller.scope();
    let agent_runs = app.db.agent_runs(&scope);
    let runs = agent_runs.for_ticket(&row.id).await.db()?;
    let mut out = Vec::with_capacity(runs.len());
    for run in &runs {
        let steps = agent_runs.steps(&run.id).await.db()?;
        out.push(run.detail(&steps));
    }
    Ok(TicketTrace {
        ticket_id: row.id,
        runs: out,
    })
}

pub async fn act(app: App, caller: Caller, input: Input<ActOnTicket>) -> ApiResult<TicketUpdated> {
    input.body.validate()?;
    let row = ticket(&app, &caller, input.param(0)).await?;
    let human = HumanTicketInput {
        org_id: caller.org_id().clone(),
        by: caller.user_id.clone(),
        action: input.body.action,
    };
    let state = app
        .ingress
        .call::<SupportTicketHuman>(Some(&keys::ticket(caller.org_id(), &row.id)), &human, None)
        .await?;
    Ok(TicketUpdated {
        ticket: TicketRef {
            id: row.id,
            subject: row.subject,
            status: state.status,
        },
        assignee: state.assignee,
    })
}
