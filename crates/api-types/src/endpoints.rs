//! Every `/api` route as a typed [`Endpoint`], plus the route table [`ALL`].
//!
//! `docs/plan/api.md` is the prose version of this table; a test keeps them in sync.
//!
//! The web worker mounts each route with `Route::path` and checks `Route::access` before the
//! handler runs. Reads go straight to D1. Mutations validate the body ([`crate::Validate`]),
//! then call the Restate handler named in `Route::restate` and answer with fresh data read
//! back from D1.

use crate::account::{
    AddToCartRequest, AddressList, BuildDetail, BuildList, Cart, CreateBuildRequest,
    PlaceOrderRequest, PlaceOrderResponse, SaveAddressRequest, SavedAddress, SetCartItemRequest,
    UpdateBuildRequest,
};
use crate::admin::{
    AdminCancelOrderRequest, AdminOrderDetail, AdminOrderListQuery, AdminOrderSummary,
    AdminOverview, AdminTicketDetail, AdminTicketListQuery, AdminTicketSummary, CustomerDetail,
    CustomerListQuery, CustomerSummary, DemoResetRequest, DemoResetResponse, EmailDetail,
    EmailListQuery, EmailSummary, InventoryItem, InventoryList, InventoryQuery, InvoiceListQuery,
    InvoiceReissued, OrderUpdated, PaymentListQuery, PaymentsLedger, SetStockRequest,
    SettingsResponse, TicketActionRequest, TicketTrace, TicketUpdated, UpdateSettingsRequest,
    VoidReissueRequest,
};
use crate::cases::{
    InitiateRefundRequest, InvestigationDetail, InvestigationListQuery, InvestigationStarted,
    InvestigationSummary, OpenInvestigationRequest, RefundDetail, RefundListQuery, RefundStarted,
    RefundSummary,
};
use crate::catalog::{
    AlternativesRequest, AlternativesResponse, CatalogResponse, CompatCheckRequest,
    CompatCheckResponse, ProductDetail, StockResponse,
};
use crate::common::{Deleted, Page};
use crate::demo::{
    AdvanceClockRequest, CreateScenarioRequest, CreateScenarioResponse, DemoPanel,
    RunDemoActionRequest, SetFaultsRequest,
};
use crate::muhaha::{
    ModelsResponse, MuhahaOrgList, ReseedResponse, ResetAllResponse, UpdateModelsRequest,
};
use crate::orders::{InvoiceDocument, InvoiceSummary, OrderDetail, OrderListQuery, OrderSummary};
use crate::route::{Access, Empty, Endpoint, Method, Route};
use crate::session::{Config, DevLoginRequest, DevLoginResponse, Me};
use crate::support::{
    SendSupportMessageRequest, SendSupportMessageResponse, TicketDetail, TicketListQuery,
    TicketSummary,
};
use crate::tasks::{DecideTaskRequest, HumanTaskDetail, HumanTaskSummary, TaskListQuery};

macro_rules! endpoints {
    (@restate) => { None };
    (@restate $handler:literal) => { Some($handler) };
    ($(
        $(#[$meta:meta])*
        $name:ident: $method:ident $path:literal, $access:ident $(, calls $handler:literal)?;
            $query:ty, $body:ty => $response:ty;
    )+) => {
        $(
            $(#[$meta])*
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub struct $name;

            impl Endpoint for $name {
                const ROUTE: Route = Route {
                    name: stringify!($name),
                    method: Method::$method,
                    path: $path,
                    access: Access::$access,
                    restate: endpoints!(@restate $($handler)?),
                };
                type Query = $query;
                type Body = $body;
                type Response = $response;
            }
        )+

        /// Every route, in documentation order.
        pub const ALL: &[Route] = &[$(<$name as Endpoint>::ROUTE),+];
    };
}

endpoints! {
    // --- Session ------------------------------------------------------------------------------

    /// Runtime config for the SPA (auth mode, Clerk key, support domain, catalog version).
    GetConfig: Get "/api/config", Public;
        Empty, Empty => Config;
    /// Dev login (`AUTH_MODE=dev`): returns a dev token; provisions the org on first use.
    DevLogin: Post "/api/dev/login", DevOnly, calls "Org/provision";
        Empty, DevLoginRequest => DevLoginResponse;
    /// The signed-in user, org and role. Provisions an unknown org (`Org/provision`, first call
    /// only) and refreshes the `org_members` cache.
    GetMe: Get "/api/me", Member, calls "Org/provision";
        Empty, Empty => Me;

    // --- Catalog and compatibility -------------------------------------------------------------

    /// The global catalog (no long-form content). `ETag` = catalog version.
    GetCatalog: Get "/api/catalog", Public;
        Empty, Empty => CatalogResponse;
    /// Per-org stock of every product.
    GetStock: Get "/api/stock", Member;
        Empty, Empty => StockResponse;
    /// Product page: product, rich content, stock, "works well with".
    GetProduct: Get "/api/products/{id}", Member;
        Empty, Empty => ProductDetail;
    /// Compatibility report for a set of parts (pure; no state).
    CheckCompat: Post "/api/compat/check", Member;
        Empty, CompatCheckRequest => CompatCheckResponse;
    /// Compatible alternatives for one slot within a price band (pure; no state).
    FindAlternatives: Post "/api/compat/alternatives", Member;
        Empty, AlternativesRequest => AlternativesResponse;

    // --- Cart ----------------------------------------------------------------------------------

    /// The caller's cart with stock and compatibility.
    GetCart: Get "/api/cart", Member;
        Empty, Empty => Cart;
    /// Add units of a product.
    AddToCart: Post "/api/cart/items", Member, calls "Customer/add_to_cart";
        Empty, AddToCartRequest => Cart;
    /// Set a line's quantity (`0` removes it).
    SetCartItem: Put "/api/cart/items/{product_id}", Member, calls "Customer/set_cart_item";
        Empty, SetCartItemRequest => Cart;
    /// Remove a line.
    RemoveCartItem: Delete "/api/cart/items/{product_id}", Member, calls "Customer/set_cart_item";
        Empty, Empty => Cart;
    /// Empty the cart.
    ClearCart: Delete "/api/cart", Member, calls "Customer/clear_cart";
        Empty, Empty => Cart;

    // --- Builds --------------------------------------------------------------------------------

    /// The caller's saved builds.
    ListBuilds: Get "/api/builds", Member;
        Empty, Empty => BuildList;
    /// Save a build.
    CreateBuild: Post "/api/builds", Member, calls "Customer/save_build";
        Empty, CreateBuildRequest => BuildDetail;
    /// One saved build with its live report.
    GetBuild: Get "/api/builds/{id}", Member;
        Empty, Empty => BuildDetail;
    /// Rename a build or replace its parts.
    UpdateBuild: Put "/api/builds/{id}", Member, calls "Customer/save_build";
        Empty, UpdateBuildRequest => BuildDetail;
    /// Delete a build.
    DeleteBuild: Delete "/api/builds/{id}", Member, calls "Customer/delete_build";
        Empty, Empty => Deleted;
    /// Add every part of a build to the cart.
    AddBuildToCart: Post "/api/builds/{id}/add-to-cart", Member, calls "Customer/add_build_to_cart";
        Empty, Empty => Cart;

    // --- Addresses -----------------------------------------------------------------------------

    /// The caller's address book.
    ListAddresses: Get "/api/addresses", Member;
        Empty, Empty => AddressList;
    /// Save a new address.
    CreateAddress: Post "/api/addresses", Member, calls "Customer/save_address";
        Empty, SaveAddressRequest => SavedAddress;
    /// Replace an address.
    UpdateAddress: Put "/api/addresses/{id}", Member, calls "Customer/save_address";
        Empty, SaveAddressRequest => SavedAddress;
    /// Delete an address.
    DeleteAddress: Delete "/api/addresses/{id}", Member, calls "Customer/delete_address";
        Empty, Empty => Deleted;

    // --- Checkout and orders -------------------------------------------------------------------

    /// Place an order from the cart.
    PlaceOrder: Post "/api/checkout", Member, calls "Customer/checkout";
        Empty, PlaceOrderRequest => PlaceOrderResponse;
    /// The caller's orders, newest first (cancelled hidden unless asked).
    ListOrders: Get "/api/orders", Member;
        OrderListQuery, Empty => Page<OrderSummary>;
    /// Order page: items, totals, timeline, card statement, invoices, tracking, tickets.
    GetOrder: Get "/api/orders/{number}", Member;
        Empty, Empty => OrderDetail;
    /// One invoice or credit note of the order, by invoice number.
    GetOrderInvoice: Get "/api/orders/{number}/invoices/{invoice}", Member;
        Empty, Empty => InvoiceDocument;

    // --- Demo controls -------------------------------------------------------------------------

    /// The demo drawer: next actions, faults, simulated clock, hidden truth.
    GetDemoPanel: Get "/api/orders/{number}/demo", Member;
        Empty, Empty => DemoPanel;
    /// Run one lifecycle step.
    RunDemoAction: Post "/api/orders/{number}/demo/actions", Member, calls "Order/demo_action";
        Empty, RunDemoActionRequest => DemoPanel;
    /// Arm faults for the next steps.
    SetDemoFaults: Put "/api/orders/{number}/demo/faults", Member, calls "Order/set_faults";
        Empty, SetFaultsRequest => DemoPanel;
    /// Move the simulated clock or the ship date.
    AdvanceDemoClock: Post "/api/orders/{number}/demo/clock", Member, calls "Order/advance_clock";
        Empty, AdvanceClockRequest => DemoPanel;
    /// Create an order in a preset state.
    CreateScenario: Post "/api/demo/scenarios", Member, calls "Demo/create_scenario";
        Empty, CreateScenarioRequest => CreateScenarioResponse;

    // --- Support -------------------------------------------------------------------------------

    /// The caller's tickets, most recently updated first.
    ListTickets: Get "/api/tickets", Member;
        TicketListQuery, Empty => Page<TicketSummary>;
    /// A ticket's conversation (customer view).
    GetTicket: Get "/api/tickets/{id}", Member;
        Empty, Empty => TicketDetail;
    /// The "Send as email" form.
    SendSupportMessage: Post "/api/support/messages", Member, calls "SupportTicket/inbound";
        Empty, SendSupportMessageRequest => SendSupportMessageResponse;

    // --- Admin: overview and human tasks -------------------------------------------------------

    /// KPIs, pending tasks and recent activity.
    GetAdminOverview: Get "/api/admin/overview", Admin;
        Empty, Empty => AdminOverview;
    /// The human tasks inbox (pending first).
    ListTasks: Get "/api/admin/tasks", Admin;
        TaskListQuery, Empty => Page<HumanTaskSummary>;
    /// One task with context, evidence and allowed decisions.
    GetTask: Get "/api/admin/tasks/{id}", Admin;
        Empty, Empty => HumanTaskDetail;
    /// Decide a task (resolves the waiting workflow).
    DecideTask: Post "/api/admin/tasks/{id}/decide", Admin, calls "HumanTasks/decide";
        Empty, DecideTaskRequest => HumanTaskDetail;

    // --- Admin: orders -------------------------------------------------------------------------

    /// All orders, filterable.
    ListAdminOrders: Get "/api/admin/orders", Admin;
        AdminOrderListQuery, Empty => Page<AdminOrderSummary>;
    /// Order page with everything internal.
    GetAdminOrder: Get "/api/admin/orders/{number}", Admin;
        Empty, Empty => AdminOrderDetail;
    /// Cancel an unpaid order.
    CancelOrder: Post "/api/admin/orders/{number}/cancel", Admin, calls "Order/cancel";
        Empty, AdminCancelOrderRequest => OrderUpdated;
    /// Initiate a (pre-approved) refund.
    InitiateRefund: Post "/api/admin/orders/{number}/refunds", Admin, calls "RefundWorkflow/run";
        Empty, InitiateRefundRequest => RefundStarted;
    /// Open a carrier investigation without a ticket.
    OpenInvestigation: Post "/api/admin/orders/{number}/investigations", Admin, calls "Investigation/run";
        Empty, OpenInvestigationRequest => InvestigationStarted;

    // --- Admin: tickets ------------------------------------------------------------------------

    /// All tickets, filterable.
    ListAdminTickets: Get "/api/admin/tickets", Admin;
        AdminTicketListQuery, Empty => Page<AdminTicketSummary>;
    /// Thread, classification, route, confidence, tasks.
    GetAdminTicket: Get "/api/admin/tickets/{id}", Admin;
        Empty, Empty => AdminTicketDetail;
    /// Agent trace: every LLM and tool call.
    GetTicketTrace: Get "/api/admin/tickets/{id}/trace", Admin;
        Empty, Empty => TicketTrace;
    /// Reply, take over, hand back, close.
    ActOnTicket: Post "/api/admin/tickets/{id}/actions", Admin, calls "SupportTicket/human";
        Empty, TicketActionRequest => TicketUpdated;

    // --- Admin: investigations, refunds, invoices, payments -----------------------------------

    /// Investigations.
    ListInvestigations: Get "/api/admin/investigations", Admin;
        InvestigationListQuery, Empty => Page<InvestigationSummary>;
    /// One investigation with carrier responses.
    GetInvestigation: Get "/api/admin/investigations/{id}", Admin;
        Empty, Empty => InvestigationDetail;
    /// Refunds.
    ListRefunds: Get "/api/admin/refunds", Admin;
        RefundListQuery, Empty => Page<RefundSummary>;
    /// One refund with evidence and ledger entries.
    GetRefund: Get "/api/admin/refunds/{id}", Admin;
        Empty, Empty => RefundDetail;
    /// Invoices and credit notes.
    ListInvoices: Get "/api/admin/invoices", Admin;
        InvoiceListQuery, Empty => Page<InvoiceSummary>;
    /// One invoice document.
    GetInvoice: Get "/api/admin/invoices/{id}", Admin;
        Empty, Empty => InvoiceDocument;
    /// Void with a credit note and issue a corrected invoice.
    VoidReissueInvoice: Post "/api/admin/invoices/{id}/void-reissue", Admin, calls "Invoicing/void_and_reissue";
        Empty, VoidReissueRequest => InvoiceReissued;
    /// The payments ledger with suspected duplicate charges.
    GetPayments: Get "/api/admin/payments", Admin;
        PaymentListQuery, Empty => PaymentsLedger;

    // --- Admin: inventory, customers, email, settings ------------------------------------------

    /// Per-org stock of every product.
    GetInventory: Get "/api/admin/inventory", Admin;
        InventoryQuery, Empty => InventoryList;
    /// Set one product's stock.
    SetStock: Put "/api/admin/inventory/{product_id}", Admin, calls "Org/set_stock";
        Empty, SetStockRequest => InventoryItem;
    /// Org members with their activity.
    ListCustomers: Get "/api/admin/customers", Admin;
        CustomerListQuery, Empty => Page<CustomerSummary>;
    /// One member's orders and tickets.
    GetCustomer: Get "/api/admin/customers/{user_id}", Admin;
        Empty, Empty => CustomerDetail;
    /// Email log, inbound (including rejected) and outbound.
    ListEmails: Get "/api/admin/emails", Admin;
        EmailListQuery, Empty => Page<EmailSummary>;
    /// One logged email.
    GetEmail: Get "/api/admin/emails/{id}", Admin;
        Empty, Empty => EmailDetail;
    /// Org settings.
    GetSettings: Get "/api/admin/settings", Admin;
        Empty, Empty => SettingsResponse;
    /// Replace org settings.
    UpdateSettings: Put "/api/admin/settings", Admin, calls "Org/update_settings";
        Empty, UpdateSettingsRequest => SettingsResponse;
    /// Wipe the org's demo data and restock.
    ResetDemo: Post "/api/admin/demo/reset", Admin, calls "Org/reset_demo";
        Empty, DemoResetRequest => DemoResetResponse;

    // --- Operator: Muhaha ----------------------------------------------------------------------

    /// Every org, newest first.
    ListMuhahaOrgs: Get "/api/admin/muhaha/orgs", Operator;
        Empty, Empty => MuhahaOrgList;
    /// Reset every org (wipe its demo data and restock), one after another.
    ResetAllOrgs: Post "/api/admin/muhaha/reset", Operator, calls "Muhaha/reset_all";
        Empty, DemoResetRequest => ResetAllResponse;
    /// Wipe one org's demo data (its stock stays).
    ClearOrg: Post "/api/admin/muhaha/orgs/{org_id}/clear", Operator, calls "Muhaha/clear_org";
        Empty, DemoResetRequest => DemoResetResponse;
    /// Restore every catalog product of one org to its default stock.
    ReseedOrg: Post "/api/admin/muhaha/orgs/{org_id}/reseed", Operator, calls "Muhaha/reseed_org";
        Empty, Empty => ReseedResponse;
    /// The model settings, their defaults and the providers this deployment can reach (only
    /// the services worker knows its secrets, hence a Restate call for a read).
    GetModels: Get "/api/admin/muhaha/models", Operator, calls "Muhaha/models";
        Empty, Empty => ModelsResponse;
    /// Replace the model settings, or go back to the defaults.
    UpdateModels: Put "/api/admin/muhaha/models", Operator, calls "Muhaha/update_models";
        Empty, UpdateModelsRequest => ModelsResponse;
}

/// The route matching `method` and a concrete `path` (e.g. `/api/orders/TD-1042`).
pub fn find(method: Method, path: &str) -> Option<&'static Route> {
    let segments: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    ALL.iter().find(|route| {
        route.method == method && {
            let pattern: Vec<&str> = route.path.split('/').collect();
            pattern.len() == segments.len()
                && pattern
                    .iter()
                    .zip(&segments)
                    .all(|(p, s)| (p.starts_with('{') && !s.is_empty()) || p == s)
        }
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::ingress;

    /// Pure POST endpoints: computations without state, so no Restate handler.
    const PURE: &[&str] = &["CheckCompat", "FindAlternatives"];

    #[test]
    fn table_is_consistent() {
        let mut seen = HashSet::new();
        for route in ALL {
            assert!(
                seen.insert((route.method, route.path)),
                "duplicate {}",
                route.name
            );
            assert!(route.path.starts_with("/api/"), "{}", route.name);
            assert_eq!(
                route.path.starts_with("/api/admin/"),
                matches!(route.access, Access::Admin | Access::Operator),
                "{}: admin paths and admin access go together",
                route.name
            );
            assert_eq!(
                route.path.starts_with("/api/admin/muhaha/"),
                route.access == Access::Operator,
                "{}: Muhaha paths and operator access go together",
                route.name
            );
            match (route.method, route.restate) {
                (Method::Get, Some(_)) => assert!(
                    ["GetMe", "GetModels"].contains(&route.name),
                    "{} reads through Restate",
                    route.name
                ),
                (Method::Get, None) => {}
                (_, None) => assert!(
                    PURE.contains(&route.name),
                    "{} mutates without Restate",
                    route.name
                ),
                (_, Some(handler)) => {
                    assert!(
                        ingress::find(handler).is_some(),
                        "{}: unknown handler {handler}",
                        route.name
                    );
                }
            }
        }
    }

    #[test]
    fn every_ingress_handler_has_a_caller_or_is_internal() {
        // Handlers only called by other services or mail-in, not by a route.
        const NOT_ROUTED: &[&str] = &[];
        let routed: HashSet<_> = ALL.iter().filter_map(|r| r.restate).collect();
        for handler in ingress::ALL {
            let name = handler.qualified();
            assert!(
                routed.contains(name.as_str()) || NOT_ROUTED.contains(&name.as_str()),
                "{name} is not called by any route"
            );
        }
    }

    #[test]
    fn api_doc_lists_every_route() {
        let doc = include_str!("../../../docs/plan/api.md");
        for route in ALL {
            let needle = format!("`{} {}`", route.method.as_str(), route.path);
            assert!(
                doc.contains(&needle),
                "docs/plan/api.md is missing {needle}"
            );
            if let Some(handler) = route.restate {
                assert!(
                    doc.contains(&format!("`{handler}`")),
                    "docs/plan/api.md is missing `{handler}`"
                );
            }
        }
    }

    #[test]
    fn finds_routes_by_concrete_path() {
        assert_eq!(
            find(Method::Get, "/api/orders/TD-1042").unwrap().name,
            "GetOrder"
        );
        assert_eq!(find(Method::Get, "/api/orders").unwrap().name, "ListOrders");
        assert_eq!(
            find(Method::Post, "/api/orders/TD-1042/demo/actions")
                .unwrap()
                .name,
            "RunDemoAction"
        );
        assert!(find(Method::Delete, "/api/orders/TD-1042").is_none());
        assert_eq!(GetOrder::ROUTE.url(&["TD-1042"]), "/api/orders/TD-1042");
    }
}
