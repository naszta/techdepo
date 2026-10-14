//! Pure domain types and rules for TechDepó.
//!
//! Everything here is deterministic and free of I/O: no clocks, no randomness, no storage.
//! Time and entropy are passed in by the caller, so the same code runs in the SPA, the
//! Workers and native tests.

#[macro_use]
mod labeled_enum;

pub mod calendar;
pub mod clock;
pub mod fault;
pub mod feature;
mod hash;
pub mod id;
pub mod invoice;
pub mod models;
pub mod money;
pub mod order;
pub mod settings;
pub mod shipping;
pub mod support;

pub use calendar::{WorkCalendar, next_monday_9am, previous_weekday};
pub use clock::{ClockAdvance, VirtualClock};
pub use fault::{
    CarrierFault, EmailFault, FaultError, FaultToggles, InvoiceFault, LostOutcome,
    MAX_EMAIL_OUTAGE_MINUTES, PaymentFault, ShipmentFault,
};
pub use feature::{OrgFeature, known_features};
pub use hash::fnv1a64;
pub use id::{
    HumanTaskId, InvestigationId, InvoiceId, InvoiceNumber, OrderId, OrderNumber, OrgId,
    ParseIdError, ProductId, PublicOrgId, RefundId, ShipmentId, Sku, TicketId, TrackingNumber,
    UserId,
};
pub use invoice::{InvoiceKind, InvoiceStatus};
pub use labeled_enum::ParseEnumError;
pub use models::{
    MAX_MODEL_ID_CHARS, ModelAddressError, ModelChoice, ModelKind, ModelProvider, ModelSettings,
    ModelSettingsError, ModelSlot,
};
pub use money::{Money, VatBreakdown, VatRate};
pub use order::{
    Cancellation, OrderAction, OrderFlags, OrderStatus, RETURN_WINDOW_DAYS, TransitionError,
    demo_actions, valid_actions, within_return_window,
};
pub use settings::{
    MAX_HOLIDAYS, MAX_SETTINGS_DURATION_SECS, OrgSettings, ReplyDisposition, ReplyPolicy,
    SettingsError,
};
pub use shipping::{
    SHIPPING_COUNTRIES, STANDARD_TRANSIT_WORKDAYS, ShippingPolicyOutcome, carrier_eta,
    shipping_policy, ships_to, transit_workdays,
};
pub use support::{Actor, Assignee, Channel, Intent, TicketStatus};
