//! One badge per meaning, used everywhere (design.md, "Status vocabulary"). Icon and colour
//! always appear together, never colour alone.

use dioxus::prelude::*;
use techdepo_api_types::catalog::{Stock, StockLevel};
use techdepo_api_types::orders::InvoiceSummary;
use techdepo_api_types::vocab::{
    AgentRunStatus, EmailStatus, HumanTaskKind, HumanTaskStatus, InvestigationStatus,
    MessageStatus, PaymentKind, PaymentStatus, RefundStatus, ShipmentStatus,
};
use techdepo_api_types::{
    Actor, Assignee, InvoiceKind, InvoiceStatus, OrderFlags, OrderStatus, TicketStatus, Verdict,
};
use techdepo_compat::Severity;

use crate::components::badge::{Badge, BadgeAppearance, BadgeColor, BadgeSize};
use crate::state::build::Fit;
use crate::ui::icons::{Icon, IconName};

fn severity_style(severity: Option<Severity>) -> (BadgeColor, IconName, &'static str) {
    match severity {
        None => (BadgeColor::Success, IconName::Check, "Compatible"),
        Some(Severity::Note) => (BadgeColor::Info, IconName::Info, "Compatible, with notes"),
        Some(Severity::Warning) => (
            BadgeColor::Warning,
            IconName::TriangleAlert,
            "Works, with warnings",
        ),
        Some(Severity::Error) => (BadgeColor::Error, IconName::X, "Incompatible"),
    }
}

/// The overall compatibility verdict of a build.
#[component]
pub fn CompatBadge(verdict: Verdict, #[props(default)] small: bool) -> Element {
    let (color, icon) = match verdict {
        Verdict::Compatible => (BadgeColor::Success, IconName::Check),
        Verdict::Warnings => (BadgeColor::Warning, IconName::TriangleAlert),
        Verdict::Incompatible => (BadgeColor::Error, IconName::X),
    };
    let size = if small {
        BadgeSize::Sm
    } else {
        BadgeSize::Default
    };
    rsx! {
        Badge { color, size, appearance: BadgeAppearance::Soft, class: "gap-1 whitespace-nowrap",
            Icon { name: icon, class: "size-3.5" }
            "{verdict.label()}"
        }
    }
}

/// How a product fits the current build (product cards, product page).
#[component]
pub fn FitBadge(fit: Fit, #[props(default)] small: bool) -> Element {
    let (color, icon, label) = severity_style(fit.worst);
    let label = if fit.in_build { "In your build" } else { label };
    let size = if small {
        BadgeSize::Sm
    } else {
        BadgeSize::Default
    };
    rsx! {
        Badge { color, size, appearance: BadgeAppearance::Soft, class: "gap-1 whitespace-nowrap",
            Icon { name: icon, class: "size-3.5" }
            "{label}"
        }
    }
}

/// The icon for one issue's severity, with an accessible name.
#[component]
pub fn SeverityIcon(
    severity: Severity,
    #[props(default = "size-4".to_string())] class: String,
) -> Element {
    let (icon, color, label) = match severity {
        Severity::Error => (IconName::CircleX, "text-error", "Error"),
        Severity::Warning => (IconName::TriangleAlert, "text-warning", "Warning"),
        Severity::Note => (IconName::Info, "text-info", "Note"),
    };
    rsx! {
        Icon { name: icon, class: "{class} {color}", label: label.to_string() }
    }
}

/// An order's status.
#[component]
pub fn OrderStatusBadge(status: OrderStatus) -> Element {
    let (color, appearance) = match status {
        OrderStatus::AwaitingPayment => (BadgeColor::Default, BadgeAppearance::Ghost),
        OrderStatus::Paid | OrderStatus::Processing => (BadgeColor::Info, BadgeAppearance::Soft),
        OrderStatus::Shipped => (BadgeColor::Primary, BadgeAppearance::Soft),
        OrderStatus::Delivered => (BadgeColor::Success, BadgeAppearance::Soft),
        OrderStatus::ReturnRequested => (BadgeColor::Warning, BadgeAppearance::Soft),
        OrderStatus::Cancelled | OrderStatus::Refunded | OrderStatus::Returned => {
            (BadgeColor::Neutral, BadgeAppearance::Outline)
        }
    };
    rsx! {
        Badge { color, appearance, class: "whitespace-nowrap", "{status.label()}" }
    }
}

/// A support ticket's status, collapsed to what a customer cares about.
#[component]
pub fn TicketStatusBadge(status: TicketStatus) -> Element {
    let (color, label) = match status {
        TicketStatus::New | TicketStatus::Routing | TicketStatus::WithRoute => {
            (BadgeColor::Info, "Open")
        }
        TicketStatus::AwaitingCustomer => (BadgeColor::Warning, "Awaiting customer"),
        TicketStatus::WithHuman => (BadgeColor::Primary, "With a person"),
        TicketStatus::Resolved => (BadgeColor::Success, "Resolved"),
    };
    rsx! {
        Badge { color, appearance: BadgeAppearance::Soft, class: "whitespace-nowrap", "{label}" }
    }
}

/// Who did something: AI, a person, the system or the customer.
#[component]
pub fn ActorTag(actor: Actor) -> Element {
    let (icon, label) = match actor {
        Actor::Ai => (IconName::Sparkles, "AI"),
        Actor::Human => (IconName::UserRound, "Human"),
        Actor::System => (IconName::Cog, "System"),
        Actor::Customer => (IconName::User, "Customer"),
    };
    rsx! {
        Badge { size: BadgeSize::Sm, appearance: BadgeAppearance::Outline, class: "gap-1",
            Icon { name: icon, class: "size-3" }
            "{label}"
        }
    }
}

/// Stock level in the caller's org.
#[component]
pub fn StockBadge(stock: Stock) -> Element {
    let (color, icon, label) = match stock.level {
        StockLevel::InStock => (BadgeColor::Success, IconName::Check, "In stock".to_string()),
        StockLevel::Low => (
            BadgeColor::Warning,
            IconName::CircleAlert,
            format!("Only {} left", stock.stock),
        ),
        StockLevel::OutOfStock => (BadgeColor::Error, IconName::X, "Out of stock".to_string()),
    };
    rsx! {
        Badge { color, appearance: BadgeAppearance::Soft, class: "gap-1 whitespace-nowrap",
            Icon { name: icon, class: "size-3.5" }
            "{label}"
        }
    }
}

/// Overlay flags next to an order's status: outline badges (`refund pending`,
/// `investigation open`, `replacement`).
#[component]
pub fn OrderFlagBadges(flags: OrderFlags) -> Element {
    rsx! {
        if flags.refund_pending {
            Badge { appearance: BadgeAppearance::Outline, size: BadgeSize::Sm, class: "whitespace-nowrap", "Refund pending" }
        }
        if flags.investigation_open {
            Badge { appearance: BadgeAppearance::Outline, size: BadgeSize::Sm, class: "whitespace-nowrap", "Investigation open" }
        }
        if flags.replacement_for.is_some() {
            Badge { appearance: BadgeAppearance::Outline, size: BadgeSize::Sm, class: "whitespace-nowrap", "Replacement" }
        }
    }
}

/// An invoice's kind and state: issued invoice, voided invoice, credit note.
#[component]
pub fn InvoiceBadge(invoice: InvoiceSummary) -> Element {
    let (color, appearance, label) = match (invoice.kind, invoice.status) {
        (InvoiceKind::CreditNote, _) => (BadgeColor::Info, BadgeAppearance::Soft, "Credit note"),
        (_, InvoiceStatus::Voided) => (BadgeColor::Neutral, BadgeAppearance::Outline, "Voided"),
        (_, InvoiceStatus::Issued) => (BadgeColor::Success, BadgeAppearance::Soft, "Issued"),
    };
    rsx! {
        Badge { color, appearance, size: BadgeSize::Sm, class: "whitespace-nowrap", "{label}" }
    }
}

/// A parcel's status as the customer sees it (a lost parcel is "in transit").
#[component]
pub fn ShipmentStatusBadge(status: ShipmentStatus) -> Element {
    let (color, icon) = match status {
        ShipmentStatus::InTransit => (BadgeColor::Primary, IconName::Truck),
        ShipmentStatus::Delivered => (BadgeColor::Success, IconName::Check),
    };
    rsx! {
        Badge { color, appearance: BadgeAppearance::Soft, class: "gap-1 whitespace-nowrap",
            Icon { name: icon, class: "size-3.5" }
            "{status.label()}"
        }
    }
}

/// One card-statement line's state: captured, declined or refunded.
#[component]
pub fn PaymentBadge(kind: PaymentKind, status: PaymentStatus) -> Element {
    let (color, label) = match (kind, status) {
        (_, PaymentStatus::Failed) => (BadgeColor::Error, "Declined"),
        (PaymentKind::Charge, PaymentStatus::Succeeded) => (BadgeColor::Neutral, "Captured"),
        (PaymentKind::Refund, PaymentStatus::Succeeded) => (BadgeColor::Success, "Refunded"),
    };
    rsx! {
        Badge { color, appearance: BadgeAppearance::Soft, size: BadgeSize::Sm, class: "whitespace-nowrap", "{label}" }
    }
}

/// What a human task asks for. Outline, with the kind's icon.
#[component]
pub fn TaskKindBadge(kind: HumanTaskKind) -> Element {
    rsx! {
        Badge { appearance: BadgeAppearance::Outline, size: BadgeSize::Sm, class: "gap-1 whitespace-nowrap",
            Icon { name: task_kind_icon(kind), class: "size-3" }
            "{kind.label()}"
        }
    }
}

/// The icon of a human task kind (task lists, overview).
pub const fn task_kind_icon(kind: HumanTaskKind) -> IconName {
    match kind {
        HumanTaskKind::ApproveRefund => IconName::Scale,
        HumanTaskKind::InvestigationDecision => IconName::Search,
        HumanTaskKind::ReviewReply => IconName::Sparkles,
        HumanTaskKind::Escalation => IconName::Hand,
        HumanTaskKind::OffersDeclined => IconName::Ban,
    }
}

/// A human task's state; a pending task past its SLA says so.
#[component]
pub fn TaskStatusBadge(status: HumanTaskStatus, #[props(default)] overdue: bool) -> Element {
    let (color, icon, label) = match (status, overdue) {
        (HumanTaskStatus::Pending, true) => (BadgeColor::Warning, IconName::Hourglass, "Overdue"),
        (HumanTaskStatus::Pending, false) => (BadgeColor::Info, IconName::Clock, "Pending"),
        (HumanTaskStatus::Decided, _) => (BadgeColor::Success, IconName::Check, "Decided"),
        (HumanTaskStatus::Cancelled, _) => (BadgeColor::Neutral, IconName::Ban, "Cancelled"),
    };
    rsx! {
        Badge { color, appearance: BadgeAppearance::Soft, size: BadgeSize::Sm, class: "gap-1 whitespace-nowrap",
            Icon { name: icon, class: "size-3" }
            "{label}"
        }
    }
}

/// A refund's state.
#[component]
pub fn RefundStatusBadge(status: RefundStatus) -> Element {
    let (color, appearance) = match status {
        RefundStatus::Requested => (BadgeColor::Warning, BadgeAppearance::Soft),
        RefundStatus::Approved => (BadgeColor::Info, BadgeAppearance::Soft),
        RefundStatus::Rejected => (BadgeColor::Neutral, BadgeAppearance::Outline),
        RefundStatus::Completed => (BadgeColor::Success, BadgeAppearance::Soft),
    };
    rsx! {
        Badge { color, appearance, size: BadgeSize::Sm, class: "whitespace-nowrap", "{status.label()}" }
    }
}

/// An investigation's state.
#[component]
pub fn InvestigationStatusBadge(status: InvestigationStatus) -> Element {
    let (color, appearance) = match status {
        InvestigationStatus::Open | InvestigationStatus::Tracing => {
            (BadgeColor::Info, BadgeAppearance::Soft)
        }
        InvestigationStatus::AwaitingDecision | InvestigationStatus::Escalated => {
            (BadgeColor::Warning, BadgeAppearance::Soft)
        }
        InvestigationStatus::AwaitingCustomer => (BadgeColor::Primary, BadgeAppearance::Soft),
        InvestigationStatus::Resolved => (BadgeColor::Success, BadgeAppearance::Soft),
    };
    rsx! {
        Badge { color, appearance, size: BadgeSize::Sm, class: "whitespace-nowrap", "{status.label()}" }
    }
}

/// An email log entry's outcome (rejected inbound mail, failed or retried sends).
#[component]
pub fn EmailStatusBadge(status: EmailStatus) -> Element {
    let (color, icon) = match status {
        EmailStatus::Accepted | EmailStatus::Sent => (BadgeColor::Success, IconName::Check),
        EmailStatus::Queued => (BadgeColor::Info, IconName::Clock),
        EmailStatus::Rejected => (BadgeColor::Warning, IconName::MailX),
        EmailStatus::Failed => (BadgeColor::Error, IconName::CircleX),
    };
    rsx! {
        Badge { color, appearance: BadgeAppearance::Soft, size: BadgeSize::Sm, class: "gap-1 whitespace-nowrap",
            Icon { name: icon, class: "size-3" }
            "{status.label()}"
        }
    }
}

/// Who handles a ticket: the AI or a person.
#[component]
pub fn AssigneeTag(assignee: Assignee) -> Element {
    let actor = match assignee {
        Assignee::Ai => Actor::Ai,
        Assignee::Human => Actor::Human,
    };
    rsx! { ActorTag { actor } }
}

/// A message that never reached the customer: an AI draft or a discarded one (admin threads).
#[component]
pub fn MessageStatusBadge(status: MessageStatus) -> Element {
    let (color, appearance) = match status {
        MessageStatus::Draft => (BadgeColor::Warning, BadgeAppearance::Soft),
        MessageStatus::Discarded => (BadgeColor::Neutral, BadgeAppearance::Outline),
        MessageStatus::Received | MessageStatus::Sent => {
            (BadgeColor::Default, BadgeAppearance::Ghost)
        }
    };
    rsx! {
        Badge { color, appearance, size: BadgeSize::Sm, class: "whitespace-nowrap", "{status.label()}" }
    }
}

/// An agent run's state (ticket trace).
#[component]
pub fn AgentRunStatusBadge(status: AgentRunStatus) -> Element {
    let (color, icon) = match status {
        AgentRunStatus::Running => (BadgeColor::Info, IconName::Clock),
        AgentRunStatus::Completed => (BadgeColor::Success, IconName::Check),
        AgentRunStatus::Failed => (BadgeColor::Error, IconName::CircleX),
    };
    rsx! {
        Badge { color, appearance: BadgeAppearance::Soft, size: BadgeSize::Sm, class: "gap-1 whitespace-nowrap",
            Icon { name: icon, class: "size-3" }
            "{status.label()}"
        }
    }
}

/// A run that is not its first attempt (Restate retried it).
#[component]
pub fn RetryBadge(attempt: u32) -> Element {
    rsx! {
        Badge { color: BadgeColor::Warning, appearance: BadgeAppearance::Soft, size: BadgeSize::Sm, class: "gap-1 whitespace-nowrap",
            Icon { name: IconName::RefreshCw, class: "size-3" }
            "Retry {attempt}"
        }
    }
}

/// A neutral label without a status meaning (escalation causes, reject reasons, counts, an
/// agent's outcome): outlined, never coloured.
#[component]
pub fn Tag(#[props(default)] icon: Option<IconName>, children: Element) -> Element {
    rsx! {
        Badge { appearance: BadgeAppearance::Outline, size: BadgeSize::Sm, class: "gap-1 whitespace-nowrap",
            if let Some(icon) = icon {
                Icon { name: icon, class: "size-3" }
            }
            {children}
        }
    }
}
