//! App-wide client state, provided at the root: the cart and stock (server state, refreshed
//! when the session changes) and the PC builder's current build (a local draft).

pub mod build;
pub mod cart;
pub mod stock;

pub use build::{BuildStore, use_build};
pub use cart::{CartStore, use_cart};
pub use stock::{StockStore, use_stock};
