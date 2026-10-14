//! App-level components on top of the registry components (`crate::components`).
//!
//! Meaning is never coloured inline: statuses, compatibility, actors and stock go through
//! these components so the status vocabulary of `docs/plan/design.md` stays consistent.

pub mod badges;
pub mod demo;
pub mod explain;
pub mod feedback;
pub mod format;
pub mod icons;
pub mod markdown;
pub mod money;
pub mod product;
pub mod restate;
pub mod theme;

pub use badges::{
    ActorTag, AgentRunStatusBadge, AssigneeTag, CompatBadge, EmailStatusBadge, FitBadge,
    InvestigationStatusBadge, InvoiceBadge, MessageStatusBadge, OrderFlagBadges, OrderStatusBadge,
    PaymentBadge, RefundStatusBadge, RetryBadge, SeverityIcon, ShipmentStatusBadge, StockBadge,
    Tag, TaskKindBadge, TaskStatusBadge, TicketStatusBadge,
};
pub use demo::{DEMO_BUTTON, DemoDrawer, DemoFrame, DemoSection};
pub use feedback::{DemoBanner, EmptyState, ErrorAlert, PageSkeleton};
pub use icons::{Icon, IconName};
pub use money::{MoneyText, Mono, ShortId};
pub use restate::RestateLink;
pub use theme::ThemeToggle;
