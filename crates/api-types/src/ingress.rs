//! The Restate ingress contract: the handlers that the web worker, mail-in and xtask call on
//! `techdepo-services`, with their keys, inputs and outputs.
//!
//! Only these handlers are reachable from ingress; every other service (`Router`, the agents,
//! `Notifications`, `Email`, `Payments`, `Carrier`) is `ingress_private` and its types live in
//! `crates/services`.
//!
//! # Calling convention
//!
//! - **Path:** `{RESTATE_INGRESS_URL}/{Service}/{handler}` for services, and
//!   `{RESTATE_INGRESS_URL}/{Object}/{key}/{handler}` for virtual objects and workflows
//!   ([`Handler::call_path`]). Append `/send` for a one-way call ([`Handler::send_path`]):
//!   ingress answers `202` with an invocation ID and does not wait.
//! - **Headers:** `Authorization: Bearer <RESTATE_API_KEY>` and `Idempotency-Key` (see
//!   [`idempotency_key`]). A handler whose input is `()` gets **no body and no content-type**.
//! - **Scoping:** inputs carry the caller's `org_id` (and `user_id`), taken from the verified
//!   session or the verified inbound email, never from the request body. Keyed handlers must
//!   also check that their key belongs to that org ([`keys`]).
//! - **Errors:** a handler rejects a request with a Restate *terminal* error whose code is the
//!   HTTP status and whose message is an [`ApiError`] as JSON ([`terminal_message`]). The web
//!   worker turns it back into an [`ApiError`] with [`api_error_from_terminal`] and returns it
//!   to the SPA as is. Anything else (retries exhausted, timeouts) becomes `503 unavailable`.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use techdepo_domain::{
    Actor, Assignee, Channel, FaultToggles, HumanTaskId, InvestigationId, InvoiceId, ModelSettings,
    Money, OrderAction, OrderId, OrderNumber, OrderStatus, OrgId, OrgSettings, ProductId,
    PublicOrgId, TicketId, TicketStatus, UserId, fnv1a64,
};
use time::OffsetDateTime;

use crate::admin::{DemoResetResponse, InvoiceReissued, TicketAction};
use crate::catalog::StockEntry;
use crate::common::{Address, Evidence};
use crate::demo::{ClockStep, DemoScenario};
use crate::error::{ApiError, ErrorCode};
use crate::ids::{AddressId, BuildId, EmailId, PaymentId};
use crate::muhaha::{ModelsResponse, ReseedResponse, ResetAllResponse};
use crate::session::OrgInfo;
use crate::tasks::HumanDecision;
use crate::vocab::{
    HumanTaskStatus, InvestigationOutcome, PaymentMethod, RefundReason, RefundStatus,
};

/// The kind of Restate component a handler belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceKind {
    /// Stateless service: no key.
    Service,
    /// Virtual object: keyed, calls serialized per key.
    VirtualObject,
    /// Workflow: keyed by workflow ID, `run` executes once per key.
    Workflow,
}

/// Name and kind of one ingress handler, for tables and docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HandlerInfo {
    /// Service, object or workflow name.
    pub service: &'static str,
    /// Handler name.
    pub handler: &'static str,
    /// Component kind.
    pub kind: ServiceKind,
}

impl HandlerInfo {
    /// `Service/handler`, the form used in [`crate::Route::restate`] and `docs/plan/api.md`.
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.service, self.handler)
    }
}

/// A typed ingress handler.
pub trait Handler {
    /// Name and kind.
    const INFO: HandlerInfo;
    /// JSON input (`()` means no body).
    type Input: Serialize + DeserializeOwned;
    /// JSON output (`()` means no body).
    type Output: Serialize + DeserializeOwned;

    /// Request-response path. `key` is required for objects and workflows and ignored for
    /// services; it is percent-encoded.
    fn call_path(key: Option<&str>) -> String {
        let info = Self::INFO;
        match (info.kind, key) {
            (ServiceKind::Service, _) | (_, None) => format!("/{}/{}", info.service, info.handler),
            (_, Some(key)) => {
                let mut encoded = String::new();
                encode_key(&mut encoded, key);
                format!("/{}/{encoded}/{}", info.service, info.handler)
            }
        }
    }

    /// One-way (`/send`) path.
    fn send_path(key: Option<&str>) -> String {
        format!("{}/send", Self::call_path(key))
    }
}

pub(crate) fn encode_key(out: &mut String, key: &str) {
    use std::fmt::Write as _;
    for b in key.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b':') {
            out.push(char::from(b));
        } else {
            // Writing to a String cannot fail.
            let _ = write!(out, "%{b:02X}");
        }
    }
}

macro_rules! handlers {
    ($(
        $(#[$meta:meta])*
        $name:ident = $kind:ident $service:literal / $handler:literal ($input:ty) -> $output:ty;
    )+) => {
        $(
            $(#[$meta])*
            pub struct $name;

            impl Handler for $name {
                const INFO: HandlerInfo = HandlerInfo {
                    service: $service,
                    handler: $handler,
                    kind: ServiceKind::$kind,
                };
                type Input = $input;
                type Output = $output;
            }
        )+

        /// Every ingress handler.
        pub const ALL: &[HandlerInfo] = &[$(<$name as Handler>::INFO),+];
    };
}

handlers! {
    /// Lazily provision the org on its first authenticated request: public ID, default
    /// settings, inventory from catalog defaults. Idempotent: returns the existing org.
    /// Key: [`keys::org`].
    OrgProvision = VirtualObject "Org" / "provision" (ProvisionOrgInput) -> OrgInfo;
    /// Replace the org settings. Key: [`keys::org`].
    OrgUpdateSettings = VirtualObject "Org" / "update_settings" (UpdateSettingsInput) -> OrgSettings;
    /// Set one product's stock. Key: [`keys::org`].
    OrgSetStock = VirtualObject "Org" / "set_stock" (SetStockInput) -> StockEntry;
    /// Wipe operational data and restore default stock. Key: [`keys::org`].
    OrgResetDemo = VirtualObject "Org" / "reset_demo" (ResetDemoInput) -> DemoResetResponse;

    /// Add units of a product to the cart. Key: [`keys::customer`].
    CustomerAddToCart = VirtualObject "Customer" / "add_to_cart" (CartItemInput) -> ();
    /// Set a cart line's quantity (`0` removes it). Key: [`keys::customer`].
    CustomerSetCartItem = VirtualObject "Customer" / "set_cart_item" (CartItemInput) -> ();
    /// Empty the cart. Key: [`keys::customer`].
    CustomerClearCart = VirtualObject "Customer" / "clear_cart" (CustomerInput) -> ();
    /// Create or update a saved build. Key: [`keys::customer`].
    CustomerSaveBuild = VirtualObject "Customer" / "save_build" (SaveBuildInput) -> ();
    /// Delete a saved build. Key: [`keys::customer`].
    CustomerDeleteBuild = VirtualObject "Customer" / "delete_build" (BuildRefInput) -> ();
    /// Add every part of a saved build to the cart. Key: [`keys::customer`].
    CustomerAddBuildToCart = VirtualObject "Customer" / "add_build_to_cart" (BuildRefInput) -> ();
    /// Create or replace a saved address. Key: [`keys::customer`].
    CustomerSaveAddress = VirtualObject "Customer" / "save_address" (SaveAddressInput) -> ();
    /// Delete a saved address. Key: [`keys::customer`].
    CustomerDeleteAddress = VirtualObject "Customer" / "delete_address" (AddressRefInput) -> ();
    /// Snapshot the cart, check stock, total and compatibility, call `Order/place` (which
    /// reserves stock and writes the order in `awaiting_payment`), then empty the cart.
    /// Key: [`keys::customer`].
    CustomerCheckout = VirtualObject "Customer" / "checkout" (CheckoutInput) -> PlacedOrder;

    /// Run one demo step (pay, ship, deliver, …) with the armed faults. Key: [`keys::order`].
    OrderDemoAction = VirtualObject "Order" / "demo_action" (DemoActionInput) -> OrderStatus;
    /// Replace the order's armed faults. Key: [`keys::order`].
    OrderSetFaults = VirtualObject "Order" / "set_faults" (SetFaultsInput) -> ();
    /// Move the order's simulated clock (and deliver if the ETA passed), or move the ship date
    /// to last Friday. Key: [`keys::order`].
    OrderAdvanceClock = VirtualObject "Order" / "advance_clock" (AdvanceClockInput) -> ClockAdvanced;
    /// Cancel an unpaid order. Key: [`keys::order`].
    OrderCancel = VirtualObject "Order" / "cancel" (CancelOrderInput) -> OrderStatus;

    /// A customer message from mail-in (`channel = email`) or the web form (`channel = web`).
    /// Called one-way (`/send`) with `Idempotency-Key: <Message-ID>`. Key: [`keys::ticket`].
    SupportTicketInbound = VirtualObject "SupportTicket" / "inbound" (InboundMessage) -> ();
    /// A person acting on the ticket: reply, take over, hand back, close. Key: [`keys::ticket`].
    SupportTicketHuman = VirtualObject "SupportTicket" / "human" (HumanTicketInput) -> TicketState;

    /// Record a decision, then resolve the waiting workflow's awakeable or, for a ticket-owned
    /// task, hand the decision to `SupportTicket/task_decided`.
    HumanTasksDecide = Service "HumanTasks" / "decide" (DecideTaskInput) -> TaskDecided;

    /// Void an invoice with a credit note and issue a corrected one.
    InvoicingVoidAndReissue = Service "Invoicing" / "void_and_reissue" (VoidReissueInput) -> InvoiceReissued;

    /// Execute a refund (after approval unless pre-approved). Started one-way from the admin
    /// UI. Key: [`keys::refund`].
    RefundWorkflowRun = Workflow "RefundWorkflow" / "run" (StartRefundInput) -> RefundFinal;

    /// Trace a parcel and resolve it with the customer. Started one-way from the admin UI
    /// (or by `WhereIsMyOrder`). Key: [`keys::investigation`].
    InvestigationRun = Workflow "Investigation" / "run" (StartInvestigationInput) -> InvestigationFinal;

    /// Create an order in a preset state for the caller.
    DemoCreateScenario = Service "Demo" / "create_scenario" (CreateScenarioInput) -> PlacedOrder;

    /// The operator (Muhaha): reset every org with `Org/reset_demo`, one after another.
    MuhahaResetAll = Service "Muhaha" / "reset_all" (OperatorInput) -> ResetAllResponse;
    /// The operator: wipe one org's demo data (`Org/clear_data`); its stock stays.
    MuhahaClearOrg = Service "Muhaha" / "clear_org" (OrgTargetInput) -> DemoResetResponse;
    /// The operator: one org's stock back to the catalog defaults (`Org/reseed_stock`).
    MuhahaReseedOrg = Service "Muhaha" / "reseed_org" (OrgTargetInput) -> ReseedResponse;
    /// The model settings in effect, the defaults and the providers with credentials.
    MuhahaModels = Service "Muhaha" / "models" (OperatorInput) -> ModelsResponse;
    /// Replace (or forget) the model settings.
    MuhahaUpdateModels = Service "Muhaha" / "update_models" (UpdateModelsInput) -> ModelsResponse;
}

/// Looks up a handler by `Service/handler`.
pub fn find(qualified: &str) -> Option<&'static HandlerInfo> {
    let (service, handler) = qualified.split_once('/')?;
    ALL.iter()
        .find(|h| h.service == service && h.handler == handler)
}

/// Virtual object and workflow keys.
pub mod keys {
    use techdepo_domain::{InvestigationId, OrderId, OrgId, RefundId, TicketId, UserId};

    /// `Org`: the org ID.
    pub fn org(org: &OrgId) -> String {
        org.as_str().to_owned()
    }

    /// `Customer`: `<org_id>:<user_id>` (Clerk IDs contain no `:`).
    pub fn customer(org: &OrgId, user: &UserId) -> String {
        format!("{}:{}", org.as_str(), user.as_str())
    }

    /// `Order`: the order ID (globally unique).
    pub fn order(order: &OrderId) -> String {
        order.as_str().to_owned()
    }

    /// `SupportTicket`: `<org_id>:<ticket_id>`. Ticket codes are short, so the org keeps two
    /// orgs' tickets apart.
    pub fn ticket(org: &OrgId, ticket: &TicketId) -> String {
        format!("{}:{ticket}", org.as_str())
    }

    /// `RefundWorkflow`: the refund ID.
    pub fn refund(refund: &RefundId) -> String {
        refund.as_str().to_owned()
    }

    /// `Investigation`: the investigation ID.
    pub fn investigation(investigation: &InvestigationId) -> String {
        investigation.as_str().to_owned()
    }

    /// Splits a `<org_id>:<rest>` key.
    pub fn split(key: &str) -> Option<(&str, &str)> {
        key.split_once(':')
    }
}

/// The `Idempotency-Key` for a mutation the SPA identified with a client key:
/// `<org>:<user>:<route>:<client key>`. Mutations without a client key (cart, settings, demo
/// controls) send none: they are naturally idempotent or meant to repeat.
pub fn idempotency_key(org: &OrgId, user: &UserId, route: &str, client_key: &str) -> String {
    format!("{}:{}:{route}:{client_key}", org.as_str(), user.as_str())
}

/// A deterministic ID `<prefix>_<16 hex>` from its parts, for records whose ID must be known
/// before the handler runs (workflow keys, builds and addresses created from the SPA):
/// `derive_id("ref", &[org, user, client_key])`.
pub fn derive_id(prefix: &str, parts: &[&str]) -> String {
    let joined = parts.join("\u{1f}");
    format!("{prefix}_{:016x}", fnv1a64(joined.as_bytes()))
}

/// The terminal error message for `error`: the [`ApiError`] as JSON. Use the error's status as
/// the terminal error code.
pub fn terminal_message(error: &ApiError) -> String {
    serde_json::to_string(error).unwrap_or_else(|_| error.message.clone())
}

/// Turns a terminal error back into an [`ApiError`]; falls back to the code's generic error
/// with `message` as text.
pub fn api_error_from_terminal(code: u16, message: &str) -> ApiError {
    serde_json::from_str(message)
        .unwrap_or_else(|_| ApiError::new(ErrorCode::from_status(code), message))
}

// ---------------------------------------------------------------------------------------------
// Inputs and outputs
// ---------------------------------------------------------------------------------------------

/// `Org/provision` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionOrgInput {
    /// Org name from Clerk (or the dev login).
    pub name: String,
}

/// `Org/update_settings` input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateSettingsInput {
    /// The admin.
    pub by: UserId,
    /// New settings (already validated).
    pub settings: OrgSettings,
}

/// `Org/set_stock` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetStockInput {
    /// The admin.
    pub by: UserId,
    /// Product.
    pub product_id: ProductId,
    /// New stock.
    pub stock: u32,
}

/// `Org/reset_demo` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetDemoInput {
    /// The admin.
    pub by: UserId,
}

/// Input of the operator's `Muhaha` handlers that act on no particular org. The web worker
/// calls them only for an operator (`Access::Operator`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorInput {
    /// The operator.
    pub by: UserId,
}

/// `Muhaha/clear_org` and `Muhaha/reseed_org` input: any org, not the caller's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgTargetInput {
    /// The operator.
    pub by: UserId,
    /// The org acted on.
    pub org_id: OrgId,
}

/// `Muhaha/update_models` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateModelsInput {
    /// The operator.
    pub by: UserId,
    /// The new settings (validated again by the handler); `None` forgets them.
    pub settings: Option<ModelSettings>,
}

/// Identifies the customer for `Customer` handlers (must match the key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerInput {
    /// Org.
    pub org_id: OrgId,
    /// Customer.
    pub user_id: UserId,
}

/// `Customer/add_to_cart` and `Customer/set_cart_item` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CartItemInput {
    /// Customer.
    #[serde(flatten)]
    pub customer: CustomerInput,
    /// Product (must exist in the catalog).
    pub product_id: ProductId,
    /// Units to add, or the new quantity.
    pub quantity: u32,
}

/// `Customer/save_build` input. Creating needs `name`; absent fields keep their value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveBuildInput {
    /// Customer.
    #[serde(flatten)]
    pub customer: CustomerInput,
    /// Build ID (for creates: `derive_id("bld", [org, user, client key])`).
    pub build_id: BuildId,
    /// Name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_ids: Option<Vec<ProductId>>,
}

/// `Customer/delete_build` and `Customer/add_build_to_cart` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildRefInput {
    /// Customer.
    #[serde(flatten)]
    pub customer: CustomerInput,
    /// Build.
    pub build_id: BuildId,
}

/// `Customer/save_address` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveAddressInput {
    /// Customer.
    #[serde(flatten)]
    pub customer: CustomerInput,
    /// Address ID (for creates: `derive_id("adr", [org, user, client key])`).
    pub address_id: AddressId,
    /// Label.
    pub label: String,
    /// The address.
    pub address: Address,
    /// Default shipping.
    pub default_shipping: bool,
    /// Default billing.
    pub default_billing: bool,
}

/// `Customer/delete_address` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressRefInput {
    /// Customer.
    #[serde(flatten)]
    pub customer: CustomerInput,
    /// Address.
    pub address_id: AddressId,
}

/// `Customer/checkout` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutInput {
    /// Customer.
    #[serde(flatten)]
    pub customer: CustomerInput,
    /// Customer's email, snapshotted on the order (from the session).
    pub customer_email: String,
    /// Customer's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_name: Option<String>,
    /// Shipping address.
    pub shipping_address: Address,
    /// Billing address; absent means "same as shipping".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing_address: Option<Address>,
    /// Payment method.
    pub payment_method: PaymentMethod,
    /// Total the customer saw; a mismatch fails with `cart_changed`.
    pub expected_total: Money,
    /// Accept compatibility errors.
    pub override_compat: bool,
    /// Also save the shipping address.
    pub save_address: bool,
}

/// Output of `Customer/checkout` and `Demo/create_scenario`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacedOrder {
    /// Order ID.
    pub order_id: OrderId,
    /// Order number.
    pub number: OrderNumber,
    /// Status after placement (and the scenario's steps).
    pub status: OrderStatus,
}

/// `Order/demo_action` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoActionInput {
    /// Org (must own the order).
    pub org_id: OrgId,
    /// Who pressed the button.
    pub by: UserId,
    /// The step.
    pub action: OrderAction,
}

/// `Order/set_faults` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetFaultsInput {
    /// Org.
    pub org_id: OrgId,
    /// Who changed them.
    pub by: UserId,
    /// New toggles.
    pub faults: FaultToggles,
}

/// `Order/advance_clock` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvanceClockInput {
    /// Org.
    pub org_id: OrgId,
    /// Who pressed the button.
    pub by: UserId,
    /// The step.
    pub step: ClockStep,
}

/// Output of `Order/advance_clock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockAdvanced {
    /// New offset in seconds.
    pub offset_secs: i64,
    /// Order status afterwards (it may have been delivered).
    pub status: OrderStatus,
}

/// `Order/cancel` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelOrderInput {
    /// Org.
    pub org_id: OrgId,
    /// Who cancels: `human` (admin), `customer`, or `system` (the `CancelOrder` route).
    pub actor: Actor,
    /// The person, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<UserId>,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `SupportTicket/inbound` input: one verified customer message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundMessage {
    /// Org.
    pub org_id: OrgId,
    /// Its public ID (for reply addresses).
    pub public_org_id: PublicOrgId,
    /// Ticket: from the `+tag`, else the threading headers, else
    /// `TicketId::derive(org, message_id)` for a new ticket.
    pub ticket_id: TicketId,
    /// The verified sender.
    pub user_id: UserId,
    /// Sender's address.
    pub customer_email: String,
    /// Sender's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_name: Option<String>,
    /// Email or web form.
    pub channel: Channel,
    /// `Message-ID` without angle brackets (web form: `web.<client key>@<domain>`).
    pub message_id: String,
    /// `In-Reply-To`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    /// `References`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<String>,
    /// Subject.
    pub subject: String,
    /// Body with quoted history and signature stripped.
    pub body_text: String,
    /// When it was received.
    #[serde(with = "time::serde::rfc3339")]
    pub received_at: OffsetDateTime,
    /// The `email_log` row mail-in wrote (absent for the web form; the ticket logs it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_log_id: Option<EmailId>,
}

/// `SupportTicket/human` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanTicketInput {
    /// Org.
    pub org_id: OrgId,
    /// The admin.
    pub by: UserId,
    /// What they did.
    pub action: TicketAction,
}

/// Output of `SupportTicket/human`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketState {
    /// Status afterwards.
    pub status: TicketStatus,
    /// Assignee afterwards.
    pub assignee: Assignee,
}

/// `HumanTasks/decide` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecideTaskInput {
    /// Org.
    pub org_id: OrgId,
    /// Task.
    pub task_id: HumanTaskId,
    /// The admin.
    pub by: UserId,
    /// The decision (validated against the task's allowed set again by the handler).
    pub decision: HumanDecision,
    /// Note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Output of `HumanTasks/decide`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDecided {
    /// Task.
    pub task_id: HumanTaskId,
    /// Status afterwards (`decided`).
    pub status: HumanTaskStatus,
}

/// `Invoicing/void_and_reissue` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoidReissueInput {
    /// Org.
    pub org_id: OrgId,
    /// The invoice to void.
    pub invoice_id: InvoiceId,
    /// Why.
    pub reason: String,
    /// Corrected billing address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing_address: Option<Address>,
    /// `human` (admin) or `system` (the `BillingIssue` route).
    pub actor: Actor,
    /// The admin, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<UserId>,
}

/// `RefundWorkflow/run` input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartRefundInput {
    /// Org.
    pub org_id: OrgId,
    /// Order.
    pub order_id: OrderId,
    /// Amount; absent refunds everything not yet refunded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<Money>,
    /// Reason.
    pub reason: RefundReason,
    /// Free-text reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_text: Option<String>,
    /// Evidence for the approver.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
    /// Skip the approval task (a person made the offer or initiated the refund).
    pub pre_approved: bool,
    /// Who asked.
    pub requested_by: Actor,
    /// The person, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by_user: Option<UserId>,
    /// The charge to reverse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge_payment_id: Option<PaymentId>,
    /// Ticket to report back to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_id: Option<TicketId>,
    /// Investigation that offered it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub investigation_id: Option<InvestigationId>,
}

/// Output of `RefundWorkflow/run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundFinal {
    /// `completed` or `rejected`.
    pub status: RefundStatus,
}

/// `Investigation/run` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartInvestigationInput {
    /// Org.
    pub org_id: OrgId,
    /// Order.
    pub order_id: OrderId,
    /// Ticket; absent when an admin opens it from the order page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_id: Option<TicketId>,
    /// `human` (admin) or `system` (`WhereIsMyOrder`).
    pub opened_by: Actor,
    /// The admin, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened_by_user: Option<UserId>,
    /// Note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Output of `Investigation/run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvestigationFinal {
    /// How it ended.
    pub outcome: InvestigationOutcome,
}

/// `Demo/create_scenario` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateScenarioInput {
    /// Customer.
    #[serde(flatten)]
    pub customer: CustomerInput,
    /// Customer's email.
    pub customer_email: String,
    /// Customer's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_name: Option<String>,
    /// The preset.
    pub scenario: DemoScenario,
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn paths() {
        assert_eq!(HumanTasksDecide::call_path(None), "/HumanTasks/decide");
        let key = keys::customer(&OrgId::new("org_2Zx"), &UserId::new("user_9"));
        assert_eq!(key, "org_2Zx:user_9");
        assert_eq!(
            CustomerAddToCart::call_path(Some(&key)),
            "/Customer/org_2Zx:user_9/add_to_cart"
        );
        assert_eq!(
            SupportTicketInbound::send_path(Some("org_1:T-7F2M9Q")),
            "/SupportTicket/org_1:T-7F2M9Q/inbound/send"
        );
        assert_eq!(
            CustomerAddToCart::call_path(Some("a b/c")),
            "/Customer/a%20b%2Fc/add_to_cart"
        );
    }

    #[test]
    fn handler_names_are_unique() {
        let names: HashSet<_> = ALL.iter().map(HandlerInfo::qualified).collect();
        assert_eq!(names.len(), ALL.len());
        assert!(find("Order/demo_action").is_some());
        assert!(find("Order/nope").is_none());
    }

    #[test]
    fn terminal_errors_round_trip() {
        let err = ApiError::new(ErrorCode::OutOfStock, "Only 2 left of Voltaris V5");
        let back = api_error_from_terminal(409, &terminal_message(&err));
        assert_eq!(back, err);
        let plain = api_error_from_terminal(409, "cannot ship an unpaid order");
        assert_eq!(plain.code, ErrorCode::Conflict);
    }

    #[test]
    fn derived_ids_are_stable() {
        let a = derive_id("ref", &["org_1", "user_1", "k"]);
        assert_eq!(a, derive_id("ref", &["org_1", "user_1", "k"]));
        assert_ne!(a, derive_id("ref", &["org_1", "user_1k", ""]));
        assert!(a.starts_with("ref_") && a.len() == 20);
    }

    #[test]
    fn flattened_customer_inputs() {
        let input = CartItemInput {
            customer: CustomerInput {
                org_id: OrgId::new("o"),
                user_id: UserId::new("u"),
            },
            product_id: ProductId::new("p"),
            quantity: 2,
        };
        assert_eq!(
            serde_json::to_value(&input).unwrap(),
            serde_json::json!({ "org_id": "o", "user_id": "u", "product_id": "p", "quantity": 2 })
        );
    }
}
