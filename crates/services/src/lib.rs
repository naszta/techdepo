//! Every Restate service, virtual object and workflow of TechDepó, generic over the ports
//! (`techdepo_ports`), so it builds for wasm32 (production, on Workers) and natively (tests).
//!
//! | Component | Kind | Key | Role |
//! |---|---|---|---|
//! | [`Org`] | object | org ID | provisioning, settings, stock, demo reset, expiry (a delayed `expire`) |
//! | [`Customer`] | object | `<org>:<user>` | cart, builds, addresses, checkout |
//! | [`Order`] | object | order ID | the order state machine; sole writer of the order read model and timeline |
//! | [`Payments`] | service (private) | — | fake PSP: captures and refunds |
//! | [`Invoicing`] | service | — | invoices, void + reissue, credit notes |
//! | [`Carrier`] | service (private) | — | fake carrier: shipments, tracking, traces |
//! | [`Demo`] | service | — | scenario presets |
//! | [`Notifications`] | service (private) | — | renders every email (askama) and hands it to `Email` |
//! | [`Email`] | service (private) | — | logs and sends through the `Mailer` port; the email-outage fault |
//! | [`HumanTasks`] | service | — | the human task queue: create, decide (resolves awakeables), SLA nudges |
//! | [`RefundWorkflow`] | workflow | refund ID | approval (human task), refund, credit note, order update, customer email |
//! | [`SupportTicket`] | object | `<org>:<ticket>` | the conversation state machine: routing, buffering, waiters, reminders, replies, reply policy, ticket tasks |
//! | [`Router`] | service (private) | — | order extraction, one classifier pass, escalation thresholds, dispatch to the routes |
//! | [`CancelOrderRoute`], [`WhereIsMyOrderRoute`], [`BillingIssueRoute`], [`RefundRequestRoute`] | services (private) | — | the deterministic routes (`CancelOrder`, `WhereIsMyOrder`, `BillingIssue`, `RefundRequest`): decision tables over D1 facts, reporting to the ticket |
//! | [`ProductAdvisorRoute`], [`OrderAssistantRoute`] | services (private) | — | the agent routes (`ProductAdvisor`, `OrderAssistant`): an LLM agent over read-only tools, guardrails, the reply judge, traces ([`agents`]) |
//! | [`Investigation`] | workflow | investigation ID | carrier trace (polled), the person's offer, the customer's answer (Clef), refund or replacement |
//! | [`Muhaha`] | service | — | the operator's app-wide tools: reset every org, clear or reseed one, the model settings |
//!
//! The ingress handlers and their types are `techdepo_api_types::ingress`; internal handler
//! types live next to their component ([`order`], [`payments`], [`invoicing`], [`carrier`],
//! [`notifications`], [`email`], [`human_tasks`], [`refund`]).
//!
//! # Conventions
//!
//! - **Scoping:** a keyed handler checks that its key belongs to the input's `org_id` and that
//!   the record it touches has that `org_id`; a mismatch is a terminal `404` ([`errors`]).
//! - **Errors:** rejections are terminal errors whose code is the HTTP status and whose message
//!   is the `ApiError` JSON (`ingress::terminal_message`). Retryable port errors are returned as
//!   plain errors, so Restate retries them.
//! - **Time** comes from `restate_ext::ContextClockExt` (journaled), never from the system
//!   clock; each handler reads it once, as `SystemTime::now()`. Simulated time is the order's
//!   virtual clock on top of it.
//! - **Steps:** every `ctx.run` has a stable name (`load-cart`, `place-order`, …), so journals
//!   stay readable in the Restate UI and a code change that reorders steps fails loudly.
//! - **IDs** are minted from `ctx.rand_uuid()` (deterministic per invocation) or derived from
//!   the input (`ingress::derive_id`).
//! - **Email** goes `Notifications` → `Email` → `Mailer`, never directly to the port.
//! - **Models** come from the operator's model settings, read once per invocation in a
//!   journaled step ([`models`]), never from a fixed port: a replay, a retry or a running
//!   workflow keeps the models it started with.
//!
//! # Wiring
//!
//! `apps/services` builds the adapters and calls [`Services::bind_with`] with the policies from
//! its configuration ([`policy`]):
//!
//! ```ignore
//! let ports = Ports::new(Arc::new(D1Executor::new(env.d1("DB")?)), mailer);
//! let services = Services::new(ports, config);
//! let endpoint = services.bind_with(Endpoint::builder(), &policy::merged(overrides))?.build();
//! ```
//!
//! The support pipeline lives in [`support`]: intake ([`SupportTicket`]), the [`Router`], the
//! deterministic routes ([`support::routes`]) and the [`support::contract`] they and the agents
//! implement; [`investigation`] is the lost-parcel workflow. A route that is not bound yet is
//! escalated by the router ([`support::contract::route_target`]). The routes use the internal handlers
//! ([`order::Order`]'s `mark_refunded`, `record_event`, `request_return`,
//! [`carrier::Carrier`]'s `track` and `open_trace`, [`human_tasks::HumanTasks`]' `create` and
//! `cancel`).

pub mod agents;
pub mod carrier;
pub mod checkout;
pub mod customer;
pub mod demo;
pub mod email;
pub mod errors;
pub mod human_tasks;
pub mod investigation;
pub mod invoicing;
pub mod models;
pub mod muhaha;
pub mod notifications;
pub mod order;
pub mod org;
pub mod payments;
pub mod policy;
pub mod ports;
pub mod refund;
pub mod support;
mod util;

use std::sync::Arc;

use restate_config::ConfigError;
use restate_sdk::discovery::HandlerType;
use restate_sdk::endpoint::Builder;
use restate_sdk::service::{Discoverable, IntoServiceDefinition};
use techdepo_db::Db;
use techdepo_ports::{Mailer, Models};

pub use agents::{OrderAssistantRoute, ProductAdvisorRoute};
pub use carrier::Carrier;
pub use customer::Customer;
pub use demo::Demo;
pub use email::Email;
pub use human_tasks::HumanTasks;
pub use investigation::Investigation;
pub use invoicing::Invoicing;
pub use muhaha::Muhaha;
pub use notifications::Notifications;
pub use order::Order;
pub use org::Org;
pub use payments::Payments;
pub use ports::Ports;
pub use refund::RefundWorkflow;
pub use support::router::Router;
pub use support::routes::{
    BillingIssueRoute, CancelOrderRoute, RefundRequestRoute, WhereIsMyOrderRoute,
};
pub use support::ticket::SupportTicket;

/// Runtime configuration of the services.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Domain of the support address (`support+<org>@<domain>`), `EMAIL_DOMAIN`.
    pub support_domain: String,
    /// Local part of the support address, `SUPPORT_LOCAL_PART`.
    pub support_local_part: String,
    /// Display name of the sender, `EMAIL_FROM_NAME`.
    pub from_name: String,
    /// Base URL of the shop for links in emails, `APP_BASE_URL`.
    pub app_base_url: String,
    /// The fake carrier's name on shipments.
    pub carrier_name: String,
    /// Daily LLM token budgets the agents enforce (`LLM_DAILY_TOKEN_BUDGET_*`).
    pub llm_budget: techdepo_llm::BudgetLimits,
    /// How long after provisioning an org is deleted (`ORG_EXPIRY_DAYS`, [`org`]); `None`: never.
    pub org_expiry: Option<std::time::Duration>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            support_domain: "techdepo.dev".to_owned(),
            support_local_part: "support".to_owned(),
            from_name: "TechDepó Support".to_owned(),
            app_base_url: "http://localhost:8787".to_owned(),
            carrier_name: "Kestrel Parcel".to_owned(),
            llm_budget: techdepo_llm::BudgetLimits::default(),
            org_expiry: None,
        }
    }
}

impl Config {
    /// The support address parts.
    pub fn support_mailbox(&self) -> techdepo_api_types::session::SupportMailbox {
        techdepo_api_types::session::SupportMailbox {
            local_part: self.support_local_part.clone(),
            domain: self.support_domain.clone(),
        }
    }
}

/// What every component depends on: ports and configuration. Shared behind an `Arc`.
pub struct Deps {
    /// D1 (or SQLite) through the repositories.
    pub db: Db,
    /// Mail transport (the `Email` service only).
    pub mailer: Arc<dyn Mailer>,
    /// The models, picked per invocation from the model settings ([`models`]).
    pub models: Arc<dyn Models>,
    /// Configuration.
    pub config: Config,
}

/// The registry: builds every component from the ports and binds them to an endpoint.
#[derive(Clone)]
pub struct Services {
    deps: Arc<Deps>,
}

impl Services {
    /// The services over `ports`.
    pub fn new(ports: Ports, config: Config) -> Self {
        Self {
            deps: Arc::new(Deps {
                db: Db::new(ports.sql),
                mailer: ports.mailer,
                models: ports.models,
                config,
            }),
        }
    }

    /// The shared dependencies.
    pub fn deps(&self) -> &Arc<Deps> {
        &self.deps
    }

    /// Binds every component to `builder` with the built-in policies ([`policy::defaults`],
    /// plus [`policy::with_retention`] for every component).
    pub fn bind(&self, builder: Builder) -> Builder {
        self.bind_policies(builder, &policy::defaults())
    }

    /// Binds every component to `builder`, each with its policy from `policies` (keyed by
    /// service name, see [`policy`]) and the retention defaults filled in
    /// ([`policy::with_retention`]). Fails if a policy names a handler the service lacks.
    pub fn bind_with(
        &self,
        builder: Builder,
        policies: &policy::Policies,
    ) -> Result<Builder, ConfigError> {
        fn check<S: Discoverable>(policies: &policy::Policies) -> Result<(), ConfigError> {
            match policies.get(S::discover().name.as_str()) {
                Some(policy) => policy.validate_handlers::<S>(),
                None => Ok(()),
            }
        }
        check::<Org>(policies)?;
        check::<Customer>(policies)?;
        check::<Order>(policies)?;
        check::<Payments>(policies)?;
        check::<Invoicing>(policies)?;
        check::<Carrier>(policies)?;
        check::<Demo>(policies)?;
        check::<Notifications>(policies)?;
        check::<Email>(policies)?;
        check::<HumanTasks>(policies)?;
        check::<RefundWorkflow>(policies)?;
        check::<SupportTicket>(policies)?;
        check::<Router>(policies)?;
        check::<CancelOrderRoute>(policies)?;
        check::<WhereIsMyOrderRoute>(policies)?;
        check::<BillingIssueRoute>(policies)?;
        check::<RefundRequestRoute>(policies)?;
        check::<ProductAdvisorRoute>(policies)?;
        check::<OrderAssistantRoute>(policies)?;
        check::<Investigation>(policies)?;
        check::<Muhaha>(policies)?;
        Ok(self.bind_policies(builder, policies))
    }

    fn bind_policies(&self, builder: Builder, policies: &policy::Policies) -> Builder {
        fn with<S: Discoverable + IntoServiceDefinition>(
            builder: Builder,
            policies: &policy::Policies,
            service: S,
        ) -> Builder {
            let discovery = S::discover();
            let workflow_handler = discovery
                .handlers
                .iter()
                .find(|handler| handler.ty == Some(HandlerType::Workflow))
                .map(|handler| handler.name.to_string());
            let policy = policies
                .get(discovery.name.as_str())
                .cloned()
                .unwrap_or_default();
            let policy = policy::with_retention(policy, workflow_handler.as_deref());
            builder.bind(service.into_service_definition().options(policy.into()))
        }
        let deps = &self.deps;
        let b = with(builder, policies, Org::new(deps.clone()));
        let b = with(b, policies, Customer::new(deps.clone()));
        let b = with(b, policies, Order::new(deps.clone()));
        let b = with(b, policies, Payments::new(deps.clone()));
        let b = with(b, policies, Invoicing::new(deps.clone()));
        let b = with(b, policies, Carrier::new(deps.clone()));
        let b = with(b, policies, Demo::new(deps.clone()));
        let b = with(b, policies, Notifications::new(deps.clone()));
        let b = with(b, policies, Email::new(deps.clone()));
        let b = with(b, policies, HumanTasks::new(deps.clone()));
        let b = with(b, policies, RefundWorkflow::new(deps.clone()));
        let b = with(b, policies, SupportTicket::new(deps.clone()));
        let b = with(b, policies, Router::new(deps.clone()));
        let b = with(b, policies, CancelOrderRoute::new(deps.clone()));
        let b = with(b, policies, WhereIsMyOrderRoute::new(deps.clone()));
        let b = with(b, policies, BillingIssueRoute::new(deps.clone()));
        let b = with(b, policies, RefundRequestRoute::new(deps.clone()));
        let b = with(b, policies, ProductAdvisorRoute::new(deps.clone()));
        let b = with(b, policies, OrderAssistantRoute::new(deps.clone()));
        let b = with(b, policies, Investigation::new(deps.clone()));
        with(b, policies, Muhaha::new(deps.clone()))
    }
}
