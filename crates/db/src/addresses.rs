//! The customer address book (`customer_addresses`).

use serde::Deserialize;
use techdepo_api_types::{Address, AddressId};
use techdepo_domain::UserId;
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutorExt, params};
use time::OffsetDateTime;

use crate::{SqlError, Statement, json_param, repository, stmt};

/// A `customer_addresses` row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AddressRow {
    /// Address ID.
    pub id: AddressId,
    /// Owner.
    pub user_id: UserId,
    /// Label.
    pub label: String,
    /// The address.
    #[serde(rename = "address_json", with = "column::json_text")]
    pub address: Address,
    /// Default shipping address.
    #[serde(with = "column::bool_int")]
    pub is_default_shipping: bool,
    /// Default billing address.
    #[serde(with = "column::bool_int")]
    pub is_default_billing: bool,
    /// Created.
    #[serde(with = "column::millis")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "column::millis")]
    pub updated_at: OffsetDateTime,
}

/// What `save_statements` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveAddress<'a> {
    /// Address ID.
    pub id: &'a AddressId,
    /// Label.
    pub label: &'a str,
    /// The address.
    pub address: &'a Address,
    /// Make it the default shipping address.
    pub default_shipping: bool,
    /// Make it the default billing address.
    pub default_billing: bool,
}

repository! {
    /// Address book.
    Addresses
}

impl<'a> Addresses<'a> {
    /// The user's addresses: defaults first, then by label.
    pub async fn list(&self, user: &UserId) -> Result<Vec<AddressRow>, SqlError> {
        self.sql
            .query_as(
                "SELECT * FROM customer_addresses WHERE org_id = ?1 AND user_id = ?2 \
                 ORDER BY is_default_shipping DESC, is_default_billing DESC, label, id",
                &params![self.org(), user],
            )
            .await
    }

    /// One of the user's addresses.
    pub async fn get(&self, user: &UserId, id: &AddressId) -> Result<Option<AddressRow>, SqlError> {
        self.sql
            .query_first_as(
                "SELECT * FROM customer_addresses WHERE org_id = ?1 AND id = ?2 AND user_id = ?3",
                &params![self.org(), id.as_str(), user],
            )
            .await
    }

    /// The user's default shipping address, else their most recently changed one.
    pub async fn preferred_shipping(&self, user: &UserId) -> Result<Option<AddressRow>, SqlError> {
        self.sql
            .query_first_as(
                "SELECT * FROM customer_addresses WHERE org_id = ?1 AND user_id = ?2 \
                 ORDER BY is_default_shipping DESC, updated_at DESC LIMIT 1",
                &params![self.org(), user],
            )
            .await
    }

    /// Whether an address with this ID exists in the org at all.
    pub async fn exists(&self, id: &AddressId) -> Result<bool, SqlError> {
        Ok(self
            .sql
            .query_first(
                "SELECT 1 AS one FROM customer_addresses WHERE org_id = ?1 AND id = ?2",
                &params![self.org(), id.as_str()],
            )
            .await?
            .is_some())
    }

    /// Creates or replaces one of the user's addresses; a new default clears the flag on the
    /// user's other addresses. Never takes over another user's row.
    pub fn save_statements(
        &self,
        user: &UserId,
        save: &SaveAddress<'_>,
        at: OffsetDateTime,
    ) -> Result<Vec<Statement>, SqlError> {
        let mut out = Vec::new();
        if save.default_shipping {
            out.push(stmt(
                "UPDATE customer_addresses SET is_default_shipping = 0, updated_at = ?3 \
                 WHERE org_id = ?1 AND user_id = ?2 AND is_default_shipping = 1",
                params![self.org(), user, at],
            ));
        }
        if save.default_billing {
            out.push(stmt(
                "UPDATE customer_addresses SET is_default_billing = 0, updated_at = ?3 \
                 WHERE org_id = ?1 AND user_id = ?2 AND is_default_billing = 1",
                params![self.org(), user, at],
            ));
        }
        out.push(stmt(
            "INSERT INTO customer_addresses (org_id, id, user_id, label, address_json, is_default_shipping, is_default_billing, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8) ON CONFLICT (org_id, id) DO UPDATE SET \
             label = excluded.label, address_json = excluded.address_json, \
             is_default_shipping = excluded.is_default_shipping, is_default_billing = excluded.is_default_billing, \
             updated_at = excluded.updated_at WHERE customer_addresses.user_id = excluded.user_id",
            vec![
                self.org().into(),
                save.id.as_str().into(),
                user.into(),
                save.label.into(),
                json_param(save.address)?,
                save.default_shipping.into(),
                save.default_billing.into(),
                at.into(),
            ],
        ));
        Ok(out)
    }

    /// Deletes one of the user's addresses; returns whether it existed.
    pub async fn delete(&self, user: &UserId, id: &AddressId) -> Result<bool, SqlError> {
        let n = self
            .sql
            .execute(
                "DELETE FROM customer_addresses WHERE org_id = ?1 AND id = ?2 AND user_id = ?3",
                &params![self.org(), id.as_str(), user],
            )
            .await?;
        Ok(n > 0)
    }
}
