//! Server-side carts (`carts`, `cart_items`), one per user.

use serde::Deserialize;
use techdepo_domain::{ProductId, UserId};
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutorExt, params};
use time::OffsetDateTime;

use crate::{SqlError, Statement, repository, stmt};

/// A `cart_items` row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CartItemRow {
    /// Product.
    pub product_id: ProductId,
    /// Units (1–99).
    pub quantity: u32,
    /// First added.
    #[serde(with = "column::millis")]
    pub added_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "column::millis")]
    pub updated_at: OffsetDateTime,
}

/// A user's cart: its lines in the order they were added, and when it last changed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CartRows {
    /// Lines.
    pub items: Vec<CartItemRow>,
    /// Last change of the cart (absent for a cart never written).
    pub updated_at: Option<OffsetDateTime>,
}

repository! {
    /// Carts.
    Carts
}

impl<'a> Carts<'a> {
    /// The user's cart.
    pub async fn get(&self, user: &UserId) -> Result<CartRows, SqlError> {
        let items = self
            .sql
            .query_as(
                "SELECT product_id, quantity, added_at, updated_at FROM cart_items \
                 WHERE org_id = ?1 AND user_id = ?2 ORDER BY added_at, product_id",
                &params![self.org(), user],
            )
            .await?;
        let updated_at: Option<i64> = self
            .sql
            .query_scalar(
                "SELECT updated_at FROM carts WHERE org_id = ?1 AND user_id = ?2",
                &params![self.org(), user],
            )
            .await?;
        let updated_at = updated_at
            .map(column::millis::from_millis)
            .transpose()
            .map_err(|e| SqlError::Decode(e.to_string()))?;
        Ok(CartRows { items, updated_at })
    }

    /// Sets a line's quantity (`0` removes it), creating the cart if needed. Idempotent.
    pub fn set_quantity_statements(
        &self,
        user: &UserId,
        product: &ProductId,
        quantity: u32,
        at: OffsetDateTime,
    ) -> Vec<Statement> {
        let touch = self.touch_statement(user, at);
        let line = if quantity == 0 {
            stmt(
                "DELETE FROM cart_items WHERE org_id = ?1 AND user_id = ?2 AND product_id = ?3",
                params![self.org(), user, product],
            )
        } else {
            stmt(
                "INSERT INTO cart_items (org_id, user_id, product_id, quantity, added_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5) ON CONFLICT (org_id, user_id, product_id) \
                 DO UPDATE SET quantity = excluded.quantity, updated_at = excluded.updated_at",
                params![self.org(), user, product, quantity, at],
            )
        };
        vec![touch, line]
    }

    /// Empties the cart (keeps the `carts` row, so `updated_at` moves).
    pub fn clear_statements(&self, user: &UserId, at: OffsetDateTime) -> Vec<Statement> {
        vec![
            self.touch_statement(user, at),
            stmt(
                "DELETE FROM cart_items WHERE org_id = ?1 AND user_id = ?2",
                params![self.org(), user],
            ),
        ]
    }

    fn touch_statement(&self, user: &UserId, at: OffsetDateTime) -> Statement {
        stmt(
            "INSERT INTO carts (org_id, user_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?3) \
             ON CONFLICT (org_id, user_id) DO UPDATE SET updated_at = excluded.updated_at",
            params![self.org(), user, at],
        )
    }
}
