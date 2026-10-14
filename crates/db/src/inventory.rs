//! Per-org stock (`inventory`). Products themselves are compiled in (`techdepo_catalog`).

use std::collections::HashMap;

use serde::Deserialize;
use techdepo_catalog::Catalog;
use techdepo_domain::ProductId;
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutorExt, params};
use time::OffsetDateTime;

use crate::{SqlError, Statement, repository, stmt};

/// Most units of one product an org may stock (`PUT /api/admin/inventory/{id}`).
pub const MAX_STOCK: u32 = 9999;

/// An `inventory` row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StockRow {
    /// Product.
    pub product_id: ProductId,
    /// Units available.
    pub stock: u32,
    /// Last change.
    #[serde(with = "column::millis")]
    pub updated_at: OffsetDateTime,
}

repository! {
    /// Per-org stock.
    Inventory
}

impl<'a> Inventory<'a> {
    /// Every stocked product.
    pub async fn list(&self) -> Result<Vec<StockRow>, SqlError> {
        self.sql
            .query_as(
                "SELECT product_id, stock, updated_at FROM inventory WHERE org_id = ?1 ORDER BY product_id",
                &params![self.org()],
            )
            .await
    }

    /// Stock by product (absent products have no row: treat as 0).
    pub async fn stock_map(&self) -> Result<HashMap<ProductId, u32>, SqlError> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .map(|r| (r.product_id, r.stock))
            .collect())
    }

    /// One product's stock row.
    pub async fn get(&self, product: &ProductId) -> Result<Option<StockRow>, SqlError> {
        self.sql
            .query_first_as(
                "SELECT product_id, stock, updated_at FROM inventory WHERE org_id = ?1 AND product_id = ?2",
                &params![self.org(), product],
            )
            .await
    }

    /// Sets one product's stock (upsert).
    pub fn set_statement(&self, product: &ProductId, stock: u32, at: OffsetDateTime) -> Statement {
        stmt(
            "INSERT INTO inventory (org_id, product_id, stock, updated_at) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (org_id, product_id) DO UPDATE SET stock = excluded.stock, updated_at = excluded.updated_at",
            params![self.org(), product, stock, at],
        )
    }

    /// Seeds every catalog product at its default stock, leaving existing rows alone (`INSERT OR
    /// IGNORE`: re-running it after a catalog change only adds the new products).
    pub fn seed_statements(&self, catalog: &Catalog, at: OffsetDateTime) -> Vec<Statement> {
        catalog
            .all()
            .iter()
            .map(|p| {
                stmt(
                    "INSERT OR IGNORE INTO inventory (org_id, product_id, stock, updated_at) VALUES (?1, ?2, ?3, ?4)",
                    params![self.org(), &p.id, p.default_stock, at],
                )
            })
            .collect()
    }

    /// Restores every catalog product to its default stock (demo reset).
    pub fn restore_defaults_statements(
        &self,
        catalog: &Catalog,
        at: OffsetDateTime,
    ) -> Vec<Statement> {
        catalog
            .all()
            .iter()
            .map(|p| self.set_statement(&p.id, p.default_stock, at))
            .collect()
    }

    /// Takes `quantity` units. The `stock >= 0` CHECK aborts the whole batch on oversell
    /// ([`is_oversell`]).
    pub fn take_statement(
        &self,
        product: &ProductId,
        quantity: u32,
        at: OffsetDateTime,
    ) -> Statement {
        stmt(
            "UPDATE inventory SET stock = stock - ?3, updated_at = ?4 WHERE org_id = ?1 AND product_id = ?2",
            params![self.org(), product, quantity, at],
        )
    }

    /// Returns `quantity` units (cancelled unpaid order).
    pub fn put_back_statement(
        &self,
        product: &ProductId,
        quantity: u32,
        at: OffsetDateTime,
    ) -> Statement {
        stmt(
            "UPDATE inventory SET stock = min(stock + ?3, 9999), updated_at = ?4 WHERE org_id = ?1 AND product_id = ?2",
            params![self.org(), product, quantity, at],
        )
    }
}

/// Whether a failed write was the stock CHECK (an oversell).
pub fn is_oversell(error: &SqlError) -> bool {
    matches!(error, SqlError::Constraint { kind: techdepo_ports::ConstraintKind::Check, message } if message.contains("stock"))
}
