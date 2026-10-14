//! The status stepper at the top of the order page (daisyUI `steps`).
//!
//! The happy path is Placed → Paid → Processing → Shipped → Delivered. An order that left it
//! (cancelled, refunded, returned) shows how far it got, then where it ended.

use dioxus::prelude::*;
use techdepo_api_types::OrderStatus;
use techdepo_api_types::orders::OrderDetail;
use techdepo_api_types::vocab::OrderEventKind;
use time::Date;

use crate::components::steps::{Step, StepColor, Steps};
use crate::ui::format::day;

#[derive(Clone, PartialEq)]
struct StepView {
    label: String,
    when: Option<String>,
    color: StepColor,
    done: bool,
}

#[component]
pub fn OrderProgress(detail: OrderDetail) -> Element {
    let steps = steps(&detail);
    rsx! {
        // Narrow content (a phone, or a laptop with the demo drawer open) squeezes all steps
        // into the width (smaller text that may wrap) rather than scrolling the current step out
        // of sight. Sized by `main`'s width (container queries), not the viewport's.
        div { class: "overflow-x-auto rounded-box border border-base-300 bg-base-100 px-1 py-4 @xl:px-2",
            // daisyUI gives every step `min-width: 4rem`: six steps (an order that left the happy
            // path) would not fit a phone.
            Steps { class: "w-full @max-xl:[&>.step]:min-w-0 @xl:min-w-[36rem]", aria_label: "Order progress",
                for step in steps {
                    Step {
                        key: "{step.label}",
                        color: step.color,
                        "data-content": if step.done { "✓" },
                        div { class: "flex flex-col items-center px-0.5 text-center leading-tight",
                            span { class: "text-[0.7rem] font-medium @xl:text-sm", "{step.label}" }
                            span { class: "text-[0.65rem] text-base-content/60 @xl:text-xs", {step.when.unwrap_or_default()} }
                        }
                    }
                }
            }
        }
    }
}

const PATH: [(&str, OrderEventKind); 5] = [
    ("Placed", OrderEventKind::Placed),
    ("Paid", OrderEventKind::PaymentCaptured),
    ("Processing", OrderEventKind::ProcessingStarted),
    ("Shipped", OrderEventKind::Shipped),
    ("Delivered", OrderEventKind::Delivered),
];

fn steps(detail: &OrderDetail) -> Vec<StepView> {
    let status = detail.order.status;
    let happened = |kind: OrderEventKind| detail.timeline.iter().find(|e| e.kind == kind);
    let milestones = &detail.milestones;

    // How far along the happy path the order got (index into PATH).
    let reached = match status {
        OrderStatus::AwaitingPayment => 0,
        OrderStatus::Paid => 1,
        OrderStatus::Processing => 2,
        OrderStatus::Shipped => 3,
        OrderStatus::Delivered => 4,
        OrderStatus::Cancelled
        | OrderStatus::Refunded
        | OrderStatus::ReturnRequested
        | OrderStatus::Returned => {
            if milestones.delivered_at.is_some() {
                4
            } else if milestones.shipped_at.is_some() {
                3
            } else if happened(OrderEventKind::ProcessingStarted).is_some() {
                2
            } else if milestones.paid_at.is_some() {
                1
            } else {
                0
            }
        }
    };
    let off_path = !matches!(
        status,
        OrderStatus::AwaitingPayment
            | OrderStatus::Paid
            | OrderStatus::Processing
            | OrderStatus::Shipped
            | OrderStatus::Delivered
    );

    let shipment = detail.shipment.as_ref();
    let mut steps: Vec<StepView> = PATH
        .iter()
        .enumerate()
        .filter(|(i, _)| !off_path || *i <= reached)
        .map(|(i, (label, kind))| {
            // The shipment's ship date wins over the Shipped event's time: "set ship date to
            // last Friday" moves the shipment after the event was recorded.
            let shipped_on = matches!(kind, OrderEventKind::Shipped)
                .then(|| shipment.map(|s| day(s.shipped_on)))
                .flatten();
            let when = shipped_on
                .or_else(|| happened(*kind).map(|e| day(e.sim_at.date())))
                .or_else(|| match kind {
                    OrderEventKind::Placed => Some(day(detail.order.created_at.date())),
                    OrderEventKind::Shipped => shipment.map(|s| day(s.shipped_on)),
                    OrderEventKind::Delivered => shipment.and_then(|s| {
                        s.delivered_on
                            .map(day)
                            .or_else(|| (i > reached).then(|| expected(s.eta)))
                    }),
                    _ => None,
                });
            StepView {
                label: (*label).to_owned(),
                when,
                color: if i <= reached {
                    StepColor::Primary
                } else {
                    StepColor::Default
                },
                done: i < reached || (i == reached && (i == 4 || off_path)),
            }
        })
        .collect();

    if off_path {
        // On the order's simulated clock, like the other steps: the event that moved the
        // order into its current status. The milestones are real instants (a fallback for
        // timelines that do not carry the event).
        let when = detail
            .timeline
            .iter()
            .rev()
            .find(|e| e.status_after == Some(status))
            .map(|e| e.sim_at)
            .or_else(|| {
                [
                    milestones.cancelled_at,
                    milestones.refunded_at,
                    milestones.returned_at,
                ]
                .into_iter()
                .flatten()
                .max()
            })
            .map(|at| day(at.date()));
        steps.push(StepView {
            label: status.label().to_owned(),
            when,
            color: if status == OrderStatus::ReturnRequested {
                StepColor::Warning
            } else {
                StepColor::Neutral
            },
            done: false,
        });
    }
    steps
}

fn expected(eta: Date) -> String {
    format!("Expected by {}", day(eta))
}
