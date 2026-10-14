//! Every SPA route. The web worker serves `index.html` for unknown paths (SPA fallback), so
//! deep links work.
//!
//! Three layouts: the shop (top navbar), the customer account and the admin area (sidebar).
//! Account and admin pages live in `pages::account` and `pages::admin`, one module per page,
//! so later milestones (orders, support, admin) replace a stub without touching the rest.

use dioxus::prelude::*;
use techdepo_api_types::Category;

use crate::layouts::{AccountLayout, AdminLayout, ShopLayout};
use crate::pages::account::{
    AccountBuilds, AccountHome, AccountOrder, AccountOrders, AccountProfile, AccountSupport,
    AccountTicket,
};
use crate::pages::admin::{
    AdminCustomer, AdminCustomers, AdminEmail, AdminEmails, AdminInventory, AdminInvestigation,
    AdminInvestigations, AdminInvoices, AdminMuhaha, AdminOrder, AdminOrders, AdminOverview,
    AdminPayments, AdminRefund, AdminRefunds, AdminSettings, AdminTask, AdminTasks, AdminTicket,
    AdminTickets,
};
use crate::pages::shop::{
    Builder, CartPage, CategoryPage, Checkout, Compare, DemoGuide, Home, NotFound, ProductPage,
    SignIn,
};
use crate::state::build::catalog;
use crate::ui::product::category_plural;

#[derive(Debug, Clone, PartialEq, Routable)]
#[rustfmt::skip]
pub enum Route {
    #[layout(ShopLayout)]
        #[route("/")]
        Home {},
        #[route("/parts/:category", CategoryPage)]
        Category { category: String },
        #[route("/products/:id", ProductPage)]
        Product { id: String },
        #[route("/compare?:ids")]
        Compare { ids: String },
        #[route("/builder")]
        Builder {},
        #[route("/cart", CartPage)]
        Cart {},
        #[route("/checkout")]
        Checkout {},
        // `clerk`: the Clerk widget's own hash route (`#/sso-callback` after Google and co.).
        // Part of the route so the router's canonical URL keeps it: without it, the router
        // drops the hash on load and Clerk shows "The External Account was not found" instead
        // of turning a new social account's sign-in into a sign-up.
        #[route("/sign-in?:redirect#:clerk")]
        SignIn { redirect: String, clerk: String },
        #[route("/demo")]
        DemoGuide {},
    #[end_layout]

    #[nest("/account")]
        #[layout(AccountLayout)]
            #[route("/")]
            AccountHome {},
            #[route("/orders")]
            AccountOrders {},
            #[route("/orders/:number")]
            AccountOrder { number: String },
            #[route("/builds")]
            AccountBuilds {},
            #[route("/support?:subject")]
            AccountSupport { subject: String },
            #[route("/support/:id")]
            AccountTicket { id: String },
            #[route("/profile")]
            AccountProfile {},
        #[end_layout]
    #[end_nest]

    #[nest("/admin")]
        #[layout(AdminLayout)]
            #[route("/")]
            AdminOverview {},
            #[route("/tasks")]
            AdminTasks {},
            #[route("/tasks/:id")]
            AdminTask { id: String },
            #[route("/orders")]
            AdminOrders {},
            #[route("/orders/:number")]
            AdminOrder { number: String },
            #[route("/tickets")]
            AdminTickets {},
            #[route("/tickets/:id?:tab")]
            AdminTicket { id: String, tab: String },
            #[route("/investigations")]
            AdminInvestigations {},
            #[route("/investigations/:id")]
            AdminInvestigation { id: String },
            #[route("/refunds")]
            AdminRefunds {},
            #[route("/refunds/:id")]
            AdminRefund { id: String },
            #[route("/invoices")]
            AdminInvoices {},
            #[route("/payments")]
            AdminPayments {},
            #[route("/inventory")]
            AdminInventory {},
            #[route("/customers")]
            AdminCustomers {},
            #[route("/customers/:user_id")]
            AdminCustomer { user_id: String },
            #[route("/emails")]
            AdminEmails {},
            #[route("/emails/:id")]
            AdminEmail { id: String },
            #[route("/settings")]
            AdminSettings {},
            #[route("/muhaha")]
            AdminMuhaha {},
        #[end_layout]
    #[end_nest]

    #[layout(ShopLayout)]
        #[route("/:..segments")]
        NotFound { segments: Vec<String> },
}

impl Route {
    /// The browser tab title: the page, then the shop.
    pub fn title(&self) -> String {
        let page = match self {
            Self::Home {} => return "TechDepó".to_owned(),
            Self::Category { category } => category.parse::<Category>().map_or_else(
                |_| "Not found".to_owned(),
                |c| category_plural(c).to_owned(),
            ),
            Self::Product { id } => catalog()
                .get(id.as_str())
                .map_or_else(|| "Part not found".to_owned(), |p| p.name.clone()),
            Self::Compare { .. } => "Compare parts".to_owned(),
            Self::Builder {} => "PC builder".to_owned(),
            Self::Cart {} => "Cart".to_owned(),
            Self::Checkout {} => "Checkout".to_owned(),
            Self::SignIn { .. } => "Sign in".to_owned(),
            Self::DemoGuide {} => "How this demo works".to_owned(),
            Self::AccountHome {} => "My account".to_owned(),
            Self::AccountOrders {} => "My orders".to_owned(),
            Self::AccountOrder { number } => format!("Order {number}"),
            Self::AccountBuilds {} => "Saved builds".to_owned(),
            Self::AccountSupport { .. } => "Support".to_owned(),
            Self::AccountTicket { id } => format!("Ticket {id}"),
            Self::AccountProfile {} => "Profile".to_owned(),
            Self::AdminOverview {} => "Admin".to_owned(),
            Self::AdminTasks {} => "Human tasks · Admin".to_owned(),
            Self::AdminTask { .. } => "Human task · Admin".to_owned(),
            Self::AdminOrders {} => "Orders · Admin".to_owned(),
            Self::AdminOrder { number } => format!("Order {number} · Admin"),
            Self::AdminTickets {} => "Tickets · Admin".to_owned(),
            Self::AdminTicket { id, .. } => format!("Ticket {id} · Admin"),
            Self::AdminInvestigations {} => "Investigations · Admin".to_owned(),
            Self::AdminInvestigation { .. } => "Investigation · Admin".to_owned(),
            Self::AdminRefunds {} => "Refunds · Admin".to_owned(),
            Self::AdminRefund { .. } => "Refund · Admin".to_owned(),
            Self::AdminInvoices {} => "Invoices · Admin".to_owned(),
            Self::AdminPayments {} => "Payments · Admin".to_owned(),
            Self::AdminInventory {} => "Inventory · Admin".to_owned(),
            Self::AdminCustomers {} => "Customers · Admin".to_owned(),
            Self::AdminCustomer { .. } => "Customer · Admin".to_owned(),
            Self::AdminEmails {} => "Email log · Admin".to_owned(),
            Self::AdminEmail { .. } => "Email · Admin".to_owned(),
            Self::AdminSettings {} => "Settings · Admin".to_owned(),
            Self::AdminMuhaha {} => "Muhaha · Admin".to_owned(),
            Self::NotFound { .. } => "Not found".to_owned(),
        };
        format!("{page} · TechDepó")
    }

    /// The support page with an empty "Send as email" form.
    pub fn support() -> Self {
        Self::AccountSupport {
            subject: String::new(),
        }
    }

    /// An admin ticket page on its conversation tab.
    pub fn admin_ticket(id: impl ToString) -> Self {
        Self::AdminTicket {
            id: id.to_string(),
            tab: String::new(),
        }
    }

    /// An admin ticket page on its agent trace tab.
    pub fn admin_ticket_trace(id: impl ToString) -> Self {
        Self::AdminTicket {
            id: id.to_string(),
            tab: "trace".to_owned(),
        }
    }

    /// The sign-in page, returning to `redirect` afterwards.
    pub fn sign_in_to(redirect: impl Into<String>) -> Self {
        Self::SignIn {
            redirect: redirect.into(),
            clerk: String::new(),
        }
    }
}
