//! The email log (commerce.md §7): every outbound email as sent and every inbound one,
//! **including rejected mail** with the reason (auto-replies, failed DMARC, unknown senders).
//! One email shows its headers and bodies; HTML renders in a frame that runs no scripts.

use dioxus::prelude::*;
use techdepo_api_types::TicketId;
use techdepo_api_types::admin::{EmailListQuery, EmailSummary};
use techdepo_api_types::endpoints::{GetEmail, ListEmails};
use techdepo_api_types::vocab::{Direction, EmailStatus};

use super::common::{
    BackLink, Fact, Facts, FilterBar, Muted, OrderLink, SearchFilter, SelectFilter, TableWrap,
    non_blank, options, parsed, use_paged,
};
use crate::api::use_api;
use crate::pages::{PageHeader, Panel};
use crate::routes::Route;
use crate::ui::format::{ago, datetime};
use crate::ui::{
    EmailStatusBadge, EmptyState, ErrorAlert, Icon, IconName, Mono, PageSkeleton, Tag,
};

#[component]
pub fn AdminEmails() -> Element {
    let api = use_api();
    let direction = use_signal(String::new);
    let status = use_signal(String::new);
    let ticket = use_signal(String::new);
    let emails = use_paged(move |cursor| async move {
        let query = EmailListQuery {
            direction: parsed::<Direction>(&direction()),
            status: parsed::<EmailStatus>(&status()),
            ticket: non_blank(&ticket()).and_then(|t| t.to_uppercase().parse::<TicketId>().ok()),
            cursor,
            limit: Some(50),
        };
        api.query::<ListEmails>(&[], &query).await
    });

    rsx! {
        PageHeader { title: "Email log", subtitle: "Everything we sent and received, including mail we dropped and why." }
        FilterBar {
            SelectFilter { label: "Direction", value: direction, options: options(Direction::ALL, Direction::as_str, Direction::label) }
            SelectFilter { label: "Status", value: status, options: options(EmailStatus::ALL, EmailStatus::as_str, EmailStatus::label) }
            SearchFilter { label: "Ticket", placeholder: "T-7F2M9Q", value: ticket }
        }
        {emails.render(|items| rsx! {
            if items.is_empty() {
                EmptyState { message: "No email matches.", icon: IconName::Mail }
            } else {
                TableWrap {
                    table { class: "table table-sm",
                        thead {
                            tr {
                                th {}
                                th { "Subject" }
                                th { "From → To" }
                                th { "Status" }
                                th { "About" }
                                th { "When" }
                            }
                        }
                        tbody {
                            for email in items {
                                EmailRow { key: "{email.id}", email }
                            }
                        }
                    }
                }
            }
        })}
    }
}

#[component]
fn DirectionIcon(direction: Direction) -> Element {
    let (icon, label) = match direction {
        Direction::Inbound => (IconName::ArrowDownLeft, "Inbound"),
        Direction::Outbound => (IconName::ArrowUpRight, "Outbound"),
    };
    rsx! {
        span { class: "inline-flex items-center gap-1 text-xs text-base-content/70",
            Icon { name: icon, class: "size-3.5" }
            "{label}"
        }
    }
}

#[component]
fn EmailRow(email: EmailSummary) -> Element {
    rsx! {
        tr { class: "hover:bg-base-200",
            td { DirectionIcon { direction: email.direction } }
            td { class: "max-w-72",
                Link { to: Route::AdminEmail { id: email.id.to_string() }, class: "flex flex-col",
                    span { class: "link-hover truncate font-medium", if email.subject.is_empty() { "(no subject)" } else { "{email.subject}" } }
                    span { class: "text-xs text-base-content/60",
                        "{email.channel.label()}"
                        if let Some(template) = email.template.clone() { " · template " Mono { "{template}" } }
                    }
                }
            }
            td { class: "max-w-64 text-xs",
                div { class: "flex flex-col",
                    span { class: "truncate", title: "{email.from}", "{email.from}" }
                    span { class: "truncate text-base-content/60", title: "{email.to}", "→ {email.to}" }
                }
            }
            td {
                div { class: "flex flex-col items-start gap-0.5",
                    EmailStatusBadge { status: email.status }
                    if let Some(reason) = email.reject_reason {
                        span { class: "text-xs text-base-content/70", "{reason.label()}" }
                    }
                    if email.attempts > 1 {
                        span { class: "text-xs text-base-content/60", "{email.attempts} attempts" }
                    }
                }
            }
            td { class: "text-xs",
                div { class: "flex flex-col",
                    if let Some(t) = email.ticket_id.clone() {
                        Link { to: Route::admin_ticket(&t), class: "link link-hover", Mono { "{t}" } }
                    }
                    if let Some(n) = email.order_number { OrderLink { number: n } }
                }
            }
            td { class: "whitespace-nowrap text-base-content/70", title: "{datetime(email.created_at)}", "{ago(email.created_at)}" }
        }
    }
}

#[component]
pub fn AdminEmail(id: String) -> Element {
    let api = use_api();
    let mut detail = use_resource(use_reactive!(|(id,)| async move {
        api.get::<GetEmail>(&[id.as_str()]).await
    }));
    let d = match &*detail.read() {
        None => return rsx! { PageSkeleton { rows: 3 } },
        Some(Err(error)) => {
            return rsx! {
                BackLink { to: Route::AdminEmails {}, label: "Email log" }
                ErrorAlert { error: error.clone(), on_retry: move |_| detail.restart() }
            };
        }
        Some(Ok(d)) => d.clone(),
    };
    let e = d.email.clone();
    let from = match d.from_name.clone() {
        Some(name) => format!("{name} <{}>", e.from),
        None => e.from.clone(),
    };

    rsx! {
        BackLink { to: Route::AdminEmails {}, label: "Email log" }
        div { class: "mb-5 flex flex-col gap-1",
            div { class: "flex flex-wrap items-center gap-2",
                DirectionIcon { direction: e.direction }
                EmailStatusBadge { status: e.status }
                if let Some(reason) = e.reject_reason {
                    Tag { "{reason.label()}" }
                }
            }
            h1 { class: "text-xl font-semibold tracking-tight", if e.subject.is_empty() { "(no subject)" } else { "{e.subject}" } }
        }
        div { class: "grid items-start gap-5 *:min-w-0 xl:grid-cols-[minmax(0,1fr)_24rem]",
            div { class: "flex flex-col gap-5",
                if let Some(error) = d.error.clone() {
                    div { role: "alert", class: "alert alert-error alert-soft",
                        Icon { name: IconName::CircleX, class: "size-4" }
                        div { class: "flex flex-col",
                            span { class: "font-medium", "Last error" }
                            span { class: "break-words", "{error}" }
                        }
                    }
                }
                Panel { title: "Text",
                    match d.text_body.clone() {
                        Some(text) => rsx! { pre { class: "max-h-[32rem] overflow-auto whitespace-pre-wrap break-words font-mono text-xs leading-relaxed", "{text}" } },
                        None => rsx! { Muted { "No plain-text body." } },
                    }
                }
                if let Some(html) = d.html_body.clone() {
                    Panel { title: "HTML", subtitle: "Rendered without scripts.",
                        iframe {
                            title: "HTML body",
                            class: "h-[32rem] w-full rounded-field border border-base-300 bg-white",
                            "sandbox": "",
                            "srcdoc": "{html}",
                        }
                    }
                }
            }
            Panel { title: "Headers",
                Facts {
                    Fact { label: "From", span { class: "break-all", "{from}" } }
                    Fact { label: "To", span { class: "break-all", "{e.to}" } }
                    if let Some(reply_to) = d.reply_to.clone() {
                        Fact { label: "Reply-To", span { class: "break-all", "{reply_to}" } }
                    }
                    if let Some(message_id) = d.message_id.clone() {
                        Fact { label: "Message-ID", Mono { class: "break-all text-xs", "{message_id}" } }
                    }
                    if let Some(in_reply_to) = d.in_reply_to.clone() {
                        Fact { label: "In-Reply-To", Mono { class: "break-all text-xs", "{in_reply_to}" } }
                    }
                    if !d.references.is_empty() {
                        Fact { label: "References",
                            for (i , r) in d.references.iter().enumerate() {
                                Mono { key: "{i}", class: "block break-all text-xs", "{r}" }
                            }
                        }
                    }
                    Fact { label: "Channel", "{e.channel.label()}" }
                    if let Some(template) = e.template.clone() {
                        Fact { label: "Template", Mono { "{template}" } }
                    }
                    if let Some(t) = e.ticket_id.clone() {
                        Fact { label: "Ticket", Link { to: Route::admin_ticket(&t), class: "link link-hover", Mono { "{t}" } } }
                    }
                    if let Some(n) = e.order_number {
                        Fact { label: "Order", OrderLink { number: n } }
                    }
                    Fact { label: "Logged", "{datetime(e.created_at)}" }
                    if let Some(sent) = e.sent_at {
                        Fact { label: "Sent", "{datetime(sent)}" }
                    }
                    if e.direction == Direction::Outbound {
                        Fact { label: "Attempts", "{e.attempts}" }
                    }
                    if let Some(provider) = d.provider_id.clone() {
                        Fact { label: "Provider ID", Mono { class: "break-all text-xs", "{provider}" } }
                    }
                    if let Some(auth) = d.auth_results.clone() {
                        Fact { label: "Authentication", span { class: "break-words text-xs", "{auth}" } }
                    }
                    if let Some(size) = d.raw_size {
                        Fact { label: "Size", "{size} bytes" }
                    }
                }
            }
        }
    }
}
