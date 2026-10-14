//! `/api/admin/*` (`org:admin` only; the access check is done by `endpoint::authed` from the
//! route table).

pub mod billing;
pub mod cases;
pub mod muhaha;
pub mod orders;
pub mod overview;
pub mod store;
pub mod tasks;
pub mod tickets;
