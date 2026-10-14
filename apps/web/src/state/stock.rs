//! Per-org stock (`/api/stock`), loaded after sign-in. Signed out, the shop shows no stock.

use std::collections::HashMap;

use dioxus::prelude::*;
use techdepo_api_types::catalog::Stock;
use techdepo_api_types::endpoints::GetStock;
use techdepo_api_types::{ApiError, ProductId};

use crate::api::Api;
use crate::session::Session;

/// Stock by product, provided at the root.
#[derive(Clone, Copy)]
pub struct StockStore {
    /// Empty when signed out or not loaded yet.
    pub stock: Signal<HashMap<ProductId, Stock>>,
    session: Session,
}

/// The stock store from context.
pub fn use_stock() -> StockStore {
    use_context::<StockStore>()
}

impl StockStore {
    /// An empty store for `session`.
    pub fn new(session: Session) -> Self {
        Self {
            stock: Signal::new(HashMap::new()),
            session,
        }
    }

    /// The stock of one product, if known.
    pub fn get(&self, id: &ProductId) -> Option<Stock> {
        self.stock.read().get(id).copied()
    }

    /// Reloads all stock.
    pub async fn refresh(mut self) -> Result<(), ApiError> {
        let response = Api::new(self.session).get::<GetStock>(&[]).await?;
        self.stock.set(
            response
                .items
                .into_iter()
                .map(|e| (e.product_id, e.stock))
                .collect(),
        );
        Ok(())
    }

    /// Forgets the stock (sign-out).
    pub fn clear_local(mut self) {
        self.stock.set(HashMap::new());
    }
}
