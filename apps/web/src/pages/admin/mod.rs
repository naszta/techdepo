//! Admin pages (`org:admin` only), one module per page. Admin is denser than the shop (14 px,
//! compact tables) but uses the same theme; `common` holds the shared list, filter and link
//! building blocks.
//!
//! The support pipeline (tickets, traces, reply reviews, escalations) is built separately; these
//! pages read its records through the admin API and render empty states until it runs.

mod cases;
pub(crate) mod common;
mod customers;
mod emails;
mod inventory;
mod invoices;
mod muhaha;
mod order;
mod orders;
mod overview;
mod payments;
mod settings;
mod tasks;
mod tickets;

pub use cases::{AdminInvestigation, AdminInvestigations, AdminRefund, AdminRefunds};
pub use common::entity_route;
pub use customers::{AdminCustomer, AdminCustomers};
pub use emails::{AdminEmail, AdminEmails};
pub use inventory::AdminInventory;
pub use invoices::AdminInvoices;
pub use muhaha::AdminMuhaha;
pub use order::AdminOrder;
pub use orders::AdminOrders;
pub use overview::AdminOverview;
pub use payments::AdminPayments;
pub use settings::AdminSettings;
pub use tasks::{AdminTask, AdminTasks};
pub use tickets::{AdminTicket, AdminTickets};
