//! The order's event timeline, newest first (daisyUI `timeline`), each event tagged with who did
//! it (AI, a person, the system or the customer).
//!
//! The admin order page reuses it with `admin`: references link to admin pages, internal and
//! demo events are marked, and an event links the invocation that recorded it in the Restate UI
//! when the API exposes one (local development): the `Order` handler, or the workflow or service
//! that called `Order/record_event`.

use dioxus::prelude::*;
use techdepo_api_types::orders::OrderEvent;
use techdepo_api_types::restate_ui::RestateUiLink;
use techdepo_api_types::vocab::OrderEventKind as K;

use crate::components::timeline::{
    Timeline, TimelineCompact, TimelineConnector, TimelineDirection, TimelineEnd, TimelineItem,
    TimelineMiddle,
};
use crate::pages::Panel;
use crate::pages::admin::entity_route;
use crate::routes::Route;
use crate::ui::format::datetime;
use crate::ui::restate::use_restate_ui;
use crate::ui::{ActorTag, Icon, IconName, Mono, RestateLink};
use techdepo_api_types::vocab::RefKind;

#[component]
pub fn OrderTimeline(events: Vec<OrderEvent>, #[props(default)] admin: bool) -> Element {
    let count = events.len();
    let restate_ui = use_restate_ui().filter(|_| admin);
    rsx! {
        Panel { title: "Timeline", subtitle: "Times are on the order's simulated clock (UTC).",
            if events.is_empty() {
                p { class: "text-sm text-base-content/70", "Nothing has happened yet." }
            } else {
                Timeline {
                    direction: TimelineDirection::Vertical,
                    compact: TimelineCompact::Compact,
                    class: "-ml-1",
                    for (i , event) in events.into_iter().rev().enumerate() {
                        TimelineItem { key: "{event.seq}",
                            if i > 0 {
                                TimelineConnector { class: "bg-base-300" }
                            }
                            TimelineMiddle {
                                div { class: "grid size-7 place-items-center rounded-full border border-base-300 bg-base-100 text-base-content/70",
                                    Icon { name: icon(event.kind), class: "size-3.5" }
                                }
                            }
                            TimelineEnd { class: "mb-4 ml-2 flex min-w-0 flex-col gap-1",
                                div { class: "flex flex-wrap items-center gap-2",
                                    span { class: "text-sm font-medium", "{event.summary}" }
                                    ActorTag { actor: event.actor }
                                    if admin && event.kind.is_demo() {
                                        span { class: "badge badge-sm border-dashed border-amber-600 bg-amber-50 text-amber-900 dark:bg-amber-950 dark:text-amber-100", "Demo" }
                                    } else if admin && !event.kind.is_customer_visible() {
                                        span { class: "badge badge-ghost badge-sm", "Internal" }
                                    }
                                }
                                span {
                                    class: "text-xs text-base-content/60",
                                    title: "Real time: {datetime(event.at)}",
                                    "{datetime(event.sim_at)}"
                                    if let Some(entity) = event.entity.clone() {
                                        " · "
                                        if let (true, Some(to)) = (admin, entity_route(&entity)) {
                                            Link { to, class: "link link-hover", Mono { "{entity.label}" } }
                                        } else if entity.kind == RefKind::Ticket && !admin {
                                            Link { to: Route::AccountTicket { id: entity.id.clone() }, class: "link link-hover",
                                                Mono { "{entity.label}" }
                                            }
                                        } else {
                                            Mono { "{entity.label}" }
                                        }
                                    }
                                    if let (Some(_), Some(invocation)) = (restate_ui.as_ref(), event.invocation_id.clone()) {
                                        " · "
                                        RestateLink { link: RestateUiLink::invocation(invocation) }
                                    }
                                }
                            }
                            if i + 1 < count {
                                TimelineConnector { class: "bg-base-300" }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn icon(kind: K) -> IconName {
    match kind {
        K::Placed | K::ReplacementCreated => IconName::Package,
        K::PaymentCaptured | K::PaymentDeclined => IconName::CreditCard,
        K::ProcessingStarted => IconName::Boxes,
        K::EmailSent => IconName::Mail,
        K::InvoiceIssued | K::InvoiceVoided | K::CreditNoteIssued => IconName::Receipt,
        K::Shipped | K::CarrierUpdate | K::ShipDateChanged => IconName::Truck,
        K::Delivered => IconName::CircleCheck,
        K::Cancelled => IconName::CircleX,
        K::ReturnRequested | K::Returned => IconName::ArrowLeft,
        K::RefundRequested | K::RefundApproved | K::RefundRejected | K::Refunded => IconName::Scale,
        K::InvestigationOpened | K::InvestigationClosed => IconName::Search,
        K::TicketLinked => IconName::LifeBuoy,
        K::FaultsChanged | K::FaultTriggered => IconName::Flask,
        K::ClockAdvanced => IconName::Clock,
        K::Note => IconName::Info,
    }
}
