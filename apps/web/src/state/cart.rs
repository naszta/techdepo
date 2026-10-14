//! The caller's cart (`/api/cart`). Every mutation goes through the API (a Restate handler on
//! the other side) and replaces the cart with the fresh one it returns.

use dioxus::prelude::*;
use techdepo_api_types::account::{AddToCartRequest, Cart, SetCartItemRequest};
use techdepo_api_types::endpoints::{
    AddBuildToCart, AddToCart, ClearCart, GetCart, RemoveCartItem, SetCartItem,
};
use techdepo_api_types::{ApiError, BuildId, Empty, ProductId};

use crate::api::Api;
use crate::session::Session;

/// The cart, provided at the root.
#[derive(Clone, Copy)]
pub struct CartStore {
    /// The last cart the API returned; `None` when signed out or not loaded yet.
    pub cart: Signal<Option<Cart>>,
    session: Session,
}

/// The cart from context.
pub fn use_cart() -> CartStore {
    use_context::<CartStore>()
}

impl CartStore {
    /// An empty store for `session`.
    pub fn new(session: Session) -> Self {
        Self {
            cart: Signal::new(None),
            session,
        }
    }

    fn api(&self) -> Api {
        Api::new(self.session)
    }

    /// Units in the cart, for the header badge.
    pub fn count(&self) -> u32 {
        self.cart.read().as_ref().map_or(0, Cart::item_count)
    }

    /// Forgets the cart (sign-out).
    pub fn clear_local(mut self) {
        self.cart.set(None);
    }

    fn apply(mut self, result: Result<Cart, ApiError>) -> Result<Cart, ApiError> {
        if let Ok(cart) = &result {
            self.cart.set(Some(cart.clone()));
        }
        result
    }

    /// Reloads the cart.
    pub async fn refresh(self) -> Result<Cart, ApiError> {
        let result = self.api().get::<GetCart>(&[]).await;
        self.apply(result)
    }

    /// Adds `quantity` units of a product.
    pub async fn add(self, product_id: ProductId, quantity: u32) -> Result<Cart, ApiError> {
        let body = AddToCartRequest {
            product_id,
            quantity,
        };
        let result = self.api().send::<AddToCart>(&[], &body).await;
        self.apply(result)
    }

    /// Sets a line's quantity (`0` removes it).
    pub async fn set_quantity(
        self,
        product_id: ProductId,
        quantity: u32,
    ) -> Result<Cart, ApiError> {
        let body = SetCartItemRequest { quantity };
        let result = self
            .api()
            .send::<SetCartItem>(&[product_id.as_str()], &body)
            .await;
        self.apply(result)
    }

    /// Removes a line.
    pub async fn remove(self, product_id: ProductId) -> Result<Cart, ApiError> {
        let result = self
            .api()
            .send::<RemoveCartItem>(&[product_id.as_str()], &Empty {})
            .await;
        self.apply(result)
    }

    /// Empties the cart.
    pub async fn clear(self) -> Result<Cart, ApiError> {
        let result = self.api().send::<ClearCart>(&[], &Empty {}).await;
        self.apply(result)
    }

    /// Adds every part of a saved build.
    pub async fn add_build(self, build: BuildId) -> Result<Cart, ApiError> {
        let result = self
            .api()
            .send::<AddBuildToCart>(&[build.as_str()], &Empty {})
            .await;
        self.apply(result)
    }
}
