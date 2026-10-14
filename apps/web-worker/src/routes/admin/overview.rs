//! The admin dashboard's overview.

use techdepo_api_types::admin::{ActivityItem, AdminOverview, StatusCount};
use techdepo_api_types::endpoints::GetAdminOverview;
use techdepo_api_types::vocab::{HumanTaskStatus, RefKind};
use techdepo_api_types::{EntityRef, OrderStatus};
use techdepo_db::PageRequest;

use crate::app::{App, now};
use crate::assemble;
use crate::auth::Caller;
use crate::endpoint::Input;
use crate::error::{ApiResult, DbResultExt};

/// Pending tasks shown in the inbox preview.
const PREVIEW_TASKS: u32 = 5;
/// Entries in the activity feed.
const ACTIVITY: u32 = 20;

pub async fn overview(
    app: App,
    caller: Caller,
    _: Input<GetAdminOverview>,
) -> ApiResult<AdminOverview> {
    let scope = caller.scope();
    let db = &app.db;
    let now = now();
    let counts = db.orders(&scope).count_by_status().await.db()?;
    let orders_by_status = OrderStatus::ALL
        .iter()
        .map(|&status| StatusCount {
            status,
            count: counts
                .iter()
                .find(|c| c.status == status)
                .map_or(0, |c| c.count),
        })
        .collect();
    let (pending_tasks, overdue_tasks) = db.tasks(&scope).pending_counts(now).await.db()?;
    let (refunded, refunds_pending) = db.refunds(&scope).totals().await.db()?;
    let preview = PageRequest::from_query(None, Some(PREVIEW_TASKS));
    let (pending, _) = db
        .tasks(&scope)
        .list(HumanTaskStatus::Pending, None, &preview)
        .await
        .db()?;

    // Recent activity: the latest order events and task decisions, newest first.
    let mut activity: Vec<ActivityItem> = db
        .orders(&scope)
        .recent_events(ACTIVITY)
        .await
        .db()?
        .into_iter()
        .map(|(event, number)| ActivityItem {
            entity: EntityRef {
                kind: RefKind::Order,
                id: event.order_id.as_str().to_owned(),
                label: number.to_string(),
            },
            summary: event.summary,
            actor: event.actor,
            at: event.at,
        })
        .collect();
    let recent = PageRequest::from_query(None, Some(ACTIVITY));
    let (decided, _) = db
        .tasks(&scope)
        .list(HumanTaskStatus::Decided, None, &recent)
        .await
        .db()?;
    activity.extend(decided.into_iter().filter_map(|t| {
        let at = t.decided_at?;
        let decision = t.decision.map_or("decided", |d| d.label());
        Some(ActivityItem {
            entity: EntityRef {
                kind: RefKind::HumanTask,
                id: t.id.as_str().to_owned(),
                label: t.title.clone(),
            },
            summary: format!("{}: {decision}", t.title),
            actor: techdepo_api_types::Actor::Human,
            at,
        })
    }));
    activity.sort_by_key(|a| std::cmp::Reverse(a.at));
    activity.truncate(ACTIVITY as usize);

    Ok(AdminOverview {
        orders_by_status,
        open_tickets: db.tickets(&scope).open_count(None, None).await.db()?,
        pending_tasks,
        overdue_tasks,
        revenue: db.payments(&scope).revenue().await.db()?,
        refunded,
        refunds_pending,
        open_investigations: db.investigations(&scope).open_count().await.db()?,
        tasks: assemble::task_summaries(&app, &scope, &pending, now).await?,
        activity,
    })
}
