//! The `/api/*` contract between the SPA (`apps/web`) and the web worker (`apps/web-worker`),
//! plus the Restate ingress contract the web worker, mail-in and xtask use to call
//! `crates/services`.
//!
//! - [`endpoints`]: every route as a typed [`Endpoint`] (method, path, access, query, body and
//!   response types), plus [`endpoints::ALL`]. The prose version, with the Restate handler
//!   each mutation calls, is `docs/plan/api.md`.
//! - [`ApiError`]: the body of every non-2xx response.
//! - The DTO modules, grouped by screen: [`session`], [`catalog`], [`account`], [`orders`],
//!   [`demo`], [`support`], [`tasks`], [`cases`], [`admin`] and [`muhaha`] (the operator's).
//! - [`vocab`]: enums whose wire names are stored in D1. The migrations' CHECK constraints mirror
//!   them, and a test keeps the two in sync.
//! - [`ingress`]: Restate service, handler and key names, and the handler input and output
//!   types. [`restate_ui`]: deep links from admin pages into the Restate UI.
//!
//! The crate is pure data: it builds for `wasm32-unknown-unknown` (the SPA) and natively.
//!
//! # Wire conventions
//!
//! | What | JSON |
//! |---|---|
//! | Field names | `snake_case` |
//! | Instants | RFC 3339 strings in UTC, e.g. `"2026-10-19T09:00:00Z"` |
//! | Calendar dates | `"YYYY-MM-DD"` |
//! | Money | integer cents EUR, VAT-inclusive ([`Money`]) |
//! | Enums | wire names (`"awaiting_payment"`); tagged unions use a `"kind"` field |
//! | IDs | strings. Order numbers are `"TD-1042"`, ticket IDs `"T-7F2M9Q"`, and invoice numbers `"TD-2026-000042"`. |
//! | Absent optionals | omitted or `null`; both are accepted |
//!
//! # Requests
//!
//! - Every route except the [`Access::Public`] ones needs `Authorization: Bearer <token>`. That
//!   is the Clerk session JWT, or the dev token from [`endpoints::DevLogin`] in
//!   `AUTH_MODE=dev`.
//! - `GET` and `DELETE` send no body. `POST` and `PUT` send a JSON body
//!   (`content-type: application/json`), which may be `{}` (see [`Empty`]).
//! - List endpoints take `cursor` and `limit` query parameters and return a [`Page`].
//! - Every response is JSON: `2xx` carries the endpoint's response type, and anything else
//!   carries an [`ApiError`].

pub mod account;
pub mod admin;
pub mod cases;
pub mod catalog;
mod common;
pub mod demo;
pub mod endpoints;
mod error;
mod ids;
pub mod ingress;
pub mod muhaha;
pub mod orders;
pub mod restate_ui;
mod route;
pub mod session;
pub mod support;
pub mod tasks;
pub mod vocab;

pub use common::{
    Address, CustomerRef, DEFAULT_PAGE_SIZE, Deleted, EntityRef, Evidence, MAX_PAGE_SIZE, OrderRef,
    Page, TicketRef, page_limit,
};
pub use error::{ApiError, ErrorCode, FieldError, Validate};
pub use ids::{AddressId, AgentRunId, BuildId, EmailId, PaymentId};
pub use route::{Access, Empty, Endpoint, Method, Route};

// The domain and catalog types used in DTOs, re-exported so the SPA needs only this crate for
// the wire types.
pub use techdepo_catalog::{Category, Product};
pub use techdepo_compat::{Fix, Report, Slot, Verdict};
pub use techdepo_domain::{
    Actor, Assignee, Channel, FaultToggles, HumanTaskId, Intent, InvestigationId, InvoiceId,
    InvoiceKind, InvoiceNumber, InvoiceStatus, LostOutcome, ModelChoice, ModelKind, ModelProvider,
    ModelSettings, ModelSlot, Money, OrderAction, OrderFlags, OrderId, OrderNumber, OrderStatus,
    OrgFeature, OrgId, OrgSettings, ProductId, PublicOrgId, RefundId, ShipmentId, Sku, TicketId,
    TicketStatus, TrackingNumber, UserId, VatBreakdown, VatRate,
};

/// serde formats for `time` types (the workspace does not enable `time`'s human-readable serde).
pub mod serde_fmt {
    pub use time::serde::rfc3339;

    time::serde::format_description!(pub date, Date, "[year]-[month]-[day]");
}
