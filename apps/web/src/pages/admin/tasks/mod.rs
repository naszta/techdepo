//! The human tasks inbox (commerce.md §7, support.md §8): the centre of the admin side. One
//! queue for refund approvals, investigation decisions, reply reviews, escalations and declined
//! offers, as a split view: the list on the left, the selected task on the right.
//!
//! `/admin/tasks` shows the list alone; `/admin/tasks/<id>` selects a task. On small screens
//! the selected task replaces the list.

mod decision;
mod detail;

use dioxus::prelude::*;
use techdepo_api_types::endpoints::ListTasks;
use techdepo_api_types::tasks::{HumanTaskSummary, TaskListQuery};
use techdepo_api_types::vocab::{HumanTaskKind, HumanTaskStatus};

use super::common::{FilterBar, SelectFilter, options, parsed, use_paged};
use crate::api::use_api;
use crate::pages::PageHeader;
use crate::routes::Route;
use crate::ui::badges::task_kind_icon;
use crate::ui::format::{ago, relative};
use crate::ui::{EmptyState, Icon, IconName, Mono, TaskKindBadge, TaskStatusBadge};

use detail::TaskDetail;

#[component]
pub fn AdminTasks() -> Element {
    rsx! { TasksInbox { selected: None } }
}

#[component]
pub fn AdminTask(id: String) -> Element {
    rsx! { TasksInbox { selected: Some(id) } }
}

const STATUSES: [(HumanTaskStatus, &str); 3] = [
    (HumanTaskStatus::Pending, "Pending"),
    (HumanTaskStatus::Decided, "Decided"),
    (HumanTaskStatus::Cancelled, "Cancelled"),
];

#[component]
fn TasksInbox(selected: Option<String>) -> Element {
    let api = use_api();
    let mut status = use_signal(|| HumanTaskStatus::Pending);
    let kind = use_signal(String::new);
    let mut tasks = use_paged(move |cursor| async move {
        let query = TaskListQuery {
            status: Some(status()),
            kind: parsed::<HumanTaskKind>(&kind()),
            cursor,
            limit: Some(50),
        };
        api.query::<ListTasks>(&[], &query).await
    });
    let has_selection = selected.is_some();

    rsx! {
        PageHeader {
            title: "Human tasks",
            subtitle: "Decisions only a person makes. Workflows wait here; nothing moves until you decide.",
        }
        FilterBar {
            div { class: "join", role: "group", aria_label: "Task status",
                for (value , label) in STATUSES {
                    button {
                        key: "{label}",
                        r#type: "button",
                        class: "btn btn-sm join-item",
                        class: if status() == value { "btn-active" },
                        aria_pressed: if status() == value { "true" } else { "false" },
                        onclick: move |_| status.set(value),
                        "{label}"
                    }
                }
            }
            SelectFilter {
                label: "Kind",
                value: kind,
                options: options(HumanTaskKind::ALL, HumanTaskKind::as_str, HumanTaskKind::label),
            }
            button { class: "btn btn-ghost btn-sm self-end", onclick: move |_| tasks.restart(),
                Icon { name: IconName::RefreshCw, class: "size-3.5" }
                "Refresh"
            }
        }
        div { class: "grid items-start gap-4 lg:grid-cols-[minmax(0,22rem)_minmax(0,1fr)]",
            div {
                class: "flex min-w-0 flex-col gap-2 lg:sticky lg:top-20 lg:max-h-[calc(100vh-7rem)] lg:overflow-y-auto lg:pr-1",
                class: if has_selection { "hidden lg:flex" },
                {tasks.render(|items| rsx! {
                    if items.is_empty() {
                        EmptyState {
                            message: match (status(), kind().is_empty()) {
                                (HumanTaskStatus::Pending, true) => "Nothing waiting for a decision.",
                                (HumanTaskStatus::Pending, false) => "No tasks of this kind waiting for a decision.",
                                (HumanTaskStatus::Decided, true) => "No decided tasks yet.",
                                (HumanTaskStatus::Decided, false) => "No decided tasks of this kind yet.",
                                (HumanTaskStatus::Cancelled, true) => "No cancelled tasks.",
                                (HumanTaskStatus::Cancelled, false) => "No cancelled tasks of this kind.",
                            }
                            .to_owned(),
                            icon: IconName::ListChecks,
                        }
                    }
                    ul { class: "flex flex-col gap-2",
                        for task in items {
                            li { key: "{task.id}",
                                TaskRow { selected: selected.as_deref() == Some(task.id.as_str()), task }
                            }
                        }
                    }
                })}
            }
            match selected {
                Some(id) => rsx! {
                    div { class: "min-w-0",
                        Link { to: Route::AdminTasks {}, class: "mb-3 inline-flex items-center gap-1 text-sm text-base-content/70 hover:text-base-content lg:hidden",
                            Icon { name: IconName::ArrowLeft, class: "size-3.5" }
                            "All tasks"
                        }
                        TaskDetail { key: "{id}", id: id.clone(), on_changed: move |_| tasks.restart() }
                    }
                },
                None => rsx! {
                    div { class: "hidden min-h-64 flex-col items-center justify-center gap-2 rounded-box border border-dashed border-base-300 p-8 text-center text-base-content/60 lg:flex",
                        Icon { name: IconName::ListChecks, class: "size-6" }
                        p { "Select a task to see its context, evidence and the decisions it offers." }
                    }
                },
            }
        }
    }
}

/// One task in a list: kind, title, summary, customer and SLA. Links to the task.
#[component]
pub fn TaskRow(task: HumanTaskSummary, #[props(default)] selected: bool) -> Element {
    let pending = task.status == HumanTaskStatus::Pending;
    rsx! {
        Link {
            to: Route::AdminTask { id: task.id.to_string() },
            class: if selected {
                "flex gap-3 rounded-box border border-primary bg-primary/5 p-3"
            } else {
                "flex gap-3 rounded-box border border-base-300 bg-base-100 p-3 hover:bg-base-200"
            },
            aria_current: if selected { "page" },
            div { class: "grid size-8 shrink-0 place-items-center rounded-full border border-base-300 bg-base-100 text-base-content/70",
                Icon { name: task_kind_icon(task.kind), class: "size-4" }
            }
            div { class: "flex min-w-0 flex-1 flex-col gap-1",
                div { class: "flex items-start justify-between gap-2",
                    span { class: "font-medium leading-snug", "{task.title}" }
                    if !pending || task.overdue {
                        TaskStatusBadge { status: task.status, overdue: task.overdue }
                    }
                }
                span { class: "line-clamp-2 text-xs text-base-content/70", "{task.summary}" }
                div { class: "flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-base-content/60",
                    TaskKindBadge { kind: task.kind }
                    if let Some(order) = task.order.clone() {
                        Mono { "{order.number}" }
                    }
                    if let Some(ticket) = task.ticket.clone() {
                        Mono { "{ticket.id}" }
                    }
                    if let Some(customer) = task.customer.clone() {
                        span { class: "truncate", "{customer.email}" }
                    }
                }
                span { class: "text-xs text-base-content/50",
                    "Opened {ago(task.created_at)}"
                    if pending && task.overdue { " · SLA passed {relative(task.sla_due_at)}" }
                    else if pending { " · SLA {relative(task.sla_due_at)}" }
                }
            }
        }
    }
}
