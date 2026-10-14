//! The `/api` router: every endpoint of `techdepo_api_types::endpoints::ALL`, mounted from its
//! typed `Endpoint` (method, path, access). `tests::every_route_is_mounted` keeps the two in
//! sync.

mod account;
mod admin;
mod catalog;
mod orders;
mod session;
mod support;

use axum::Router;
use techdepo_api_types::Endpoint;
use techdepo_api_types::endpoints::*;

use crate::app::App;
use crate::endpoint::{self, not_found_route};
use crate::error::error_response;

macro_rules! mount {
    ($($kind:ident $endpoint:ident => $handler:expr;)+) => {
        /// Endpoint names in mount order.
        pub const MOUNTED: &[&str] = &[$(stringify!($endpoint)),+];

        fn mount(router: Router<App>) -> Router<App> {
            router
                $(.route(<$endpoint as Endpoint>::ROUTE.path, mount!(@$kind $endpoint, $handler)))+
        }
    };
    (@public $endpoint:ident, $handler:expr) => { endpoint::public::<$endpoint, _, _>($handler) };
    (@authed $endpoint:ident, $handler:expr) => { endpoint::authed::<$endpoint, _, _>($handler) };
    (@raw $endpoint:ident, $handler:expr) => { $handler };
}

mount! {
    // Session
    public GetConfig => session::config;
    public DevLogin => session::dev_login;
    authed GetMe => session::me;

    // Catalog and compatibility
    raw GetCatalog => catalog::catalog_route();
    authed GetStock => catalog::stock;
    authed GetProduct => catalog::product;
    authed CheckCompat => catalog::check_compat;
    authed FindAlternatives => catalog::alternatives;

    // Cart, builds, addresses, checkout
    authed GetCart => account::cart;
    authed AddToCart => account::add_to_cart;
    authed SetCartItem => account::set_cart_item;
    authed RemoveCartItem => account::remove_cart_item;
    authed ClearCart => account::clear_cart;
    authed ListBuilds => account::list_builds;
    authed CreateBuild => account::create_build;
    authed GetBuild => account::get_build;
    authed UpdateBuild => account::update_build;
    authed DeleteBuild => account::delete_build;
    authed AddBuildToCart => account::add_build_to_cart;
    authed ListAddresses => account::list_addresses;
    authed CreateAddress => account::create_address;
    authed UpdateAddress => account::update_address;
    authed DeleteAddress => account::delete_address;
    authed PlaceOrder => account::checkout;

    // Orders and demo controls
    authed ListOrders => orders::list;
    authed GetOrder => orders::detail;
    authed GetOrderInvoice => orders::invoice;
    authed GetDemoPanel => orders::demo_panel;
    authed RunDemoAction => orders::run_demo_action;
    authed SetDemoFaults => orders::set_demo_faults;
    authed AdvanceDemoClock => orders::advance_clock;
    authed CreateScenario => orders::create_scenario;

    // Support (customer)
    authed ListTickets => support::list;
    authed GetTicket => support::detail;
    authed SendSupportMessage => support::send_message;

    // Admin
    authed GetAdminOverview => admin::overview::overview;
    authed ListTasks => admin::tasks::list;
    authed GetTask => admin::tasks::detail;
    authed DecideTask => admin::tasks::decide;
    authed ListAdminOrders => admin::orders::list;
    authed GetAdminOrder => admin::orders::detail;
    authed CancelOrder => admin::orders::cancel;
    authed InitiateRefund => admin::orders::refund;
    authed OpenInvestigation => admin::orders::investigate;
    authed ListAdminTickets => admin::tickets::list;
    authed GetAdminTicket => admin::tickets::detail;
    authed GetTicketTrace => admin::tickets::trace;
    authed ActOnTicket => admin::tickets::act;
    authed ListInvestigations => admin::cases::list_investigations;
    authed GetInvestigation => admin::cases::investigation;
    authed ListRefunds => admin::cases::list_refunds;
    authed GetRefund => admin::cases::refund;
    authed ListInvoices => admin::billing::list_invoices;
    authed GetInvoice => admin::billing::invoice;
    authed VoidReissueInvoice => admin::billing::void_reissue;
    authed GetPayments => admin::billing::payments;
    authed GetInventory => admin::store::inventory;
    authed SetStock => admin::store::set_stock;
    authed ListCustomers => admin::store::customers;
    authed GetCustomer => admin::store::customer;
    authed ListEmails => admin::store::emails;
    authed GetEmail => admin::store::email;
    authed GetSettings => admin::store::settings;
    authed UpdateSettings => admin::store::update_settings;
    authed ResetDemo => admin::store::reset_demo;
    authed ListMuhahaOrgs => admin::muhaha::orgs;
    authed ResetAllOrgs => admin::muhaha::reset_all;
    authed ClearOrg => admin::muhaha::clear_org;
    authed ReseedOrg => admin::muhaha::reseed_org;
    authed GetModels => admin::muhaha::models;
    authed UpdateModels => admin::muhaha::update_models;
}

/// The `/api` router with its state.
pub fn router(app: App) -> Router {
    mount(Router::new())
        .fallback(|| async { error_response(&not_found_route()) })
        .method_not_allowed_fallback(|| async { error_response(&not_found_route()) })
        .with_state(app)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_route_is_mounted() {
        let mounted: BTreeSet<_> = MOUNTED.iter().copied().collect();
        let table: BTreeSet<_> = ALL.iter().map(|r| r.name).collect();
        assert_eq!(mounted.len(), MOUNTED.len(), "a route is mounted twice");
        assert_eq!(mounted, table);
    }
}
