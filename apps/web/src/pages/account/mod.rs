//! The customer's account (commerce.md §6): orders with their demo controls, support tickets
//! with the "Send as email" form, saved builds, and the address book.

mod builds;
pub(crate) mod demo;
mod home;
pub(crate) mod order;
mod orders;
mod profile;
mod support;

pub use builds::AccountBuilds;
pub use home::AccountHome;
pub use order::AccountOrder;
pub use orders::AccountOrders;
pub use profile::AccountProfile;
pub use support::{AccountSupport, AccountTicket};
