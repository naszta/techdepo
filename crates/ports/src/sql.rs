//! SQL access: the [`SqlExecutor`] port over D1 (SQLite).
//!
//! The schema and its conventions are in `migrations/` (see `migrations/README.md`). In short:
//! instants are `INTEGER` epoch milliseconds, dates are `TEXT` `YYYY-MM-DD`, money is `INTEGER`
//! cents, booleans are `0`/`1`, and JSON documents are `TEXT`. The [`column`](mod@column) serde helpers map
//! those representations to Rust types when decoding rows.
//!
//! # Row contract
//!
//! A [`Row`] is a JSON object keyed by column name (or alias). Adapters must return:
//!
//! | SQLite value | JSON value |
//! |---|---|
//! | `NULL` | `null` |
//! | `INTEGER` | an integer number. D1 hands numbers to JS as doubles, so the D1 adapter turns every whole double into an `i64` before building the row. Otherwise serde rejects `1.0` for an `i64` field. |
//! | `REAL` | a floating-point number |
//! | `TEXT` | a string |
//! | `BLOB` | an array of byte values |
//!
//! Rows decode into typed structs with serde ([`SqlExecutorExt::query_as`]).

use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use techdepo_domain::{
    Actor, Assignee, Channel, HumanTaskId, Intent, InvestigationId, InvoiceId, InvoiceKind,
    InvoiceNumber, InvoiceStatus, Money, OrderId, OrderNumber, OrderStatus, OrgId, ProductId,
    PublicOrgId, RefundId, ShipmentId, Sku, TicketId, TicketStatus, TrackingNumber, UserId,
    VatRate,
};
use time::{Date, OffsetDateTime};

/// One result row: column name → value (see the module's row contract).
pub type Row = serde_json::Map<String, serde_json::Value>;

/// A bound parameter. Placeholders are SQLite's `?1`, `?2`, … (or bare `?`).
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    /// `NULL`.
    Null,
    /// `INTEGER`.
    Int(i64),
    /// `REAL`.
    Real(f64),
    /// `TEXT`.
    Text(String),
    /// `BLOB`.
    Blob(Vec<u8>),
}

impl Param {
    /// A `TEXT` parameter holding `value` as JSON (for `*_json` columns).
    pub fn json<T: serde::Serialize + ?Sized>(value: &T) -> Result<Self, serde_json::Error> {
        serde_json::to_string(value).map(Self::Text)
    }

    /// An `INTEGER` from a `u64`, or an error if it does not fit an `i64`.
    pub fn try_u64(value: u64) -> Result<Self, SqlError> {
        i64::try_from(value)
            .map(Self::Int)
            .map_err(|_| SqlError::Database(format!("integer {value} out of range")))
    }
}

macro_rules! param_from {
    ($($ty:ty => |$v:ident| $body:expr;)+) => {$(
        impl From<$ty> for Param {
            fn from($v: $ty) -> Self {
                $body
            }
        }
    )+};
}

param_from! {
    bool => |v| Self::Int(i64::from(v));
    i8 => |v| Self::Int(i64::from(v));
    i16 => |v| Self::Int(i64::from(v));
    i32 => |v| Self::Int(i64::from(v));
    i64 => |v| Self::Int(v);
    u8 => |v| Self::Int(i64::from(v));
    u16 => |v| Self::Int(i64::from(v));
    u32 => |v| Self::Int(i64::from(v));
    f32 => |v| Self::Real(f64::from(v));
    f64 => |v| Self::Real(v);
    String => |v| Self::Text(v);
    &str => |v| Self::Text(v.to_owned());
    &String => |v| Self::Text(v.clone());
    Cow<'_, str> => |v| Self::Text(v.into_owned());
    Vec<u8> => |v| Self::Blob(v);
    &[u8] => |v| Self::Blob(v.to_vec());
    // Money is cents, VAT rates are basis points.
    Money => |v| Self::Int(v.cents());
    VatRate => |v| Self::Int(i64::from(v.basis_points()));
    // Instants are epoch milliseconds, dates are `YYYY-MM-DD`.
    OffsetDateTime => |v| Self::Int(column::millis::to_millis(v));
    Date => |v| Self::Text(column::date_text::format(v));
    // Order numbers are stored as their numeric part.
    OrderNumber => |v| Self::Int(i64::from(v.value()));
    // Formatted IDs are stored in display form.
    PublicOrgId => |v| Self::Text(v.as_str().to_owned());
    &PublicOrgId => |v| Self::Text(v.as_str().to_owned());
    TicketId => |v| Self::Text(v.to_string());
    &TicketId => |v| Self::Text(v.to_string());
    InvoiceNumber => |v| Self::Text(v.to_string());
}

macro_rules! param_from_id {
    ($($ty:ty),+ $(,)?) => {$(
        impl From<$ty> for Param {
            fn from(v: $ty) -> Self {
                Self::Text(v.into_inner())
            }
        }

        impl From<&$ty> for Param {
            fn from(v: &$ty) -> Self {
                Self::Text(v.as_str().to_owned())
            }
        }
    )+};
}

param_from_id!(
    OrgId,
    UserId,
    OrderId,
    InvoiceId,
    ProductId,
    Sku,
    RefundId,
    InvestigationId,
    HumanTaskId,
    ShipmentId,
    TrackingNumber,
);

macro_rules! param_from_wire {
    ($($ty:ty),+ $(,)?) => {$(
        impl From<$ty> for Param {
            fn from(v: $ty) -> Self {
                Self::Text(v.as_str().to_owned())
            }
        }
    )+};
}

// Domain enums are stored by wire name. For `techdepo_api_types::vocab` enums, bind
// `kind.as_str()`.
param_from_wire!(
    OrderStatus,
    Intent,
    TicketStatus,
    Actor,
    Assignee,
    Channel,
    InvoiceKind,
    InvoiceStatus,
);

impl<T: Into<Param>> From<Option<T>> for Param {
    fn from(v: Option<T>) -> Self {
        v.map_or(Self::Null, Into::into)
    }
}

/// Builds a `Vec<Param>`: `params![org_id, status, limit]`.
#[macro_export]
macro_rules! params {
    () => { ::std::vec::Vec::<$crate::sql::Param>::new() };
    ($($value:expr),+ $(,)?) => { ::std::vec![$($crate::sql::Param::from($value)),+] };
}

/// A statement with its parameters, for [`SqlExecutor::batch`].
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    /// SQL text with `?N` placeholders.
    pub sql: Cow<'static, str>,
    /// Bound parameters.
    pub params: Vec<Param>,
}

impl Statement {
    /// A statement with parameters.
    pub fn new(sql: impl Into<Cow<'static, str>>, params: Vec<Param>) -> Self {
        Self {
            sql: sql.into(),
            params,
        }
    }
}

/// The result of one statement in a [`SqlExecutor::batch`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BatchResult {
    /// Rows changed by `INSERT`/`UPDATE`/`DELETE`.
    pub rows_affected: u64,
    /// Rows returned (`SELECT`, or `RETURNING`).
    pub rows: Vec<Row>,
}

/// Which constraint a write violated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintKind {
    /// `UNIQUE` or `PRIMARY KEY`.
    Unique,
    /// `FOREIGN KEY`.
    ForeignKey,
    /// `CHECK` (for example oversold stock).
    Check,
    /// `NOT NULL`.
    NotNull,
}

/// A database operation failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SqlError {
    /// A constraint rejected the write. Not retryable.
    #[error("{kind:?} constraint failed: {message}")]
    Constraint {
        /// The violated constraint.
        kind: ConstraintKind,
        /// The database's message, e.g. `CHECK constraint failed: stock >= 0`.
        message: String,
    },
    /// The database could not be reached or timed out. Retryable.
    #[error("database unavailable: {0}")]
    Unavailable(String),
    /// Any other database error (syntax, type mismatch, …). Not retryable.
    #[error("database error: {0}")]
    Database(String),
    /// A row did not match the requested type. Not retryable.
    #[error("cannot decode row: {0}")]
    Decode(String),
}

impl SqlError {
    /// Classifies an SQLite error message (D1 and rusqlite use SQLite's wording), falling back
    /// to [`SqlError::Database`].
    pub fn from_sqlite_message(message: impl Into<String>) -> Self {
        let message = message.into();
        let kind = [
            ("UNIQUE constraint failed", ConstraintKind::Unique),
            ("PRIMARY KEY constraint failed", ConstraintKind::Unique),
            ("FOREIGN KEY constraint failed", ConstraintKind::ForeignKey),
            ("CHECK constraint failed", ConstraintKind::Check),
            ("NOT NULL constraint failed", ConstraintKind::NotNull),
        ]
        .into_iter()
        .find_map(|(needle, kind)| message.contains(needle).then_some(kind));
        match kind {
            Some(kind) => Self::Constraint { kind, message },
            None => Self::Database(message),
        }
    }

    /// Whether retrying the same operation may succeed.
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }

    /// Whether this is a violation of `kind`.
    pub fn is_constraint(&self, kind: ConstraintKind) -> bool {
        matches!(self, Self::Constraint { kind: k, .. } if *k == kind)
    }
}

/// Executes SQL against D1 (SQLite).
///
/// Every query must filter on `org_id` (the `db` crate's `OrgScope` enforces this); the port
/// itself is scope-agnostic. Writes inside `ctx.run` must be idempotent (see
/// `migrations/README.md`).
#[async_trait]
pub trait SqlExecutor: Send + Sync {
    /// Runs a write and returns the number of rows changed.
    async fn execute(&self, sql: &str, params: &[Param]) -> Result<u64, SqlError>;

    /// Runs a query and returns every row.
    async fn query_all(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, SqlError>;

    /// Runs a query and returns the first row, if any.
    async fn query_first(&self, sql: &str, params: &[Param]) -> Result<Option<Row>, SqlError>;

    /// Runs `statements` in order as **one transaction**: either all apply or none does.
    /// Returns one result per statement.
    async fn batch(&self, statements: &[Statement]) -> Result<Vec<BatchResult>, SqlError>;
}

#[async_trait]
impl<T: SqlExecutor + ?Sized> SqlExecutor for Arc<T> {
    async fn execute(&self, sql: &str, params: &[Param]) -> Result<u64, SqlError> {
        (**self).execute(sql, params).await
    }

    async fn query_all(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, SqlError> {
        (**self).query_all(sql, params).await
    }

    async fn query_first(&self, sql: &str, params: &[Param]) -> Result<Option<Row>, SqlError> {
        (**self).query_first(sql, params).await
    }

    async fn batch(&self, statements: &[Statement]) -> Result<Vec<BatchResult>, SqlError> {
        (**self).batch(statements).await
    }
}

/// Typed queries on top of any [`SqlExecutor`]: rows decode with serde.
#[async_trait]
pub trait SqlExecutorExt: SqlExecutor {
    /// Runs a query and decodes every row as `T`.
    async fn query_as<T: DeserializeOwned + Send>(
        &self,
        sql: &str,
        params: &[Param],
    ) -> Result<Vec<T>, SqlError> {
        self.query_all(sql, params)
            .await?
            .into_iter()
            .map(decode_row)
            .collect()
    }

    /// Runs a query and decodes the first row as `T`.
    async fn query_first_as<T: DeserializeOwned + Send>(
        &self,
        sql: &str,
        params: &[Param],
    ) -> Result<Option<T>, SqlError> {
        self.query_first(sql, params)
            .await?
            .map(decode_row)
            .transpose()
    }

    /// Runs a query and decodes the first column of the first row (`SELECT count(*) …`).
    async fn query_scalar<T: DeserializeOwned + Send>(
        &self,
        sql: &str,
        params: &[Param],
    ) -> Result<Option<T>, SqlError> {
        let Some(row) = self.query_first(sql, params).await? else {
            return Ok(None);
        };
        let value = row.into_iter().next().map(|(_, v)| v);
        value
            .map(|v| serde_json::from_value(v).map_err(|e| SqlError::Decode(e.to_string())))
            .transpose()
    }
}

impl<E: SqlExecutor + ?Sized> SqlExecutorExt for E {}

/// Decodes one row as `T`.
pub fn decode_row<T: DeserializeOwned>(row: Row) -> Result<T, SqlError> {
    serde_json::from_value(serde_json::Value::Object(row))
        .map_err(|e| SqlError::Decode(e.to_string()))
}

/// serde helpers for the schema's column representations, for use with `#[serde(with = …)]`.
///
/// ```
/// use serde::Deserialize;
/// use techdepo_ports::sql::column;
///
/// #[derive(Deserialize)]
/// struct OrderRow {
///     #[serde(with = "column::bool_int")]
///     refund_pending: bool,
///     #[serde(with = "column::millis")]
///     created_at: time::OffsetDateTime,
///     #[serde(default, with = "column::millis::option")]
///     paid_at: Option<time::OffsetDateTime>,
///     #[serde(with = "column::json_text")]
///     shipping_address_json: serde_json::Value,
/// }
/// ```
pub mod column {
    /// `INTEGER` `0`/`1` ↔ `bool`.
    pub mod bool_int {
        use serde::{Deserialize, Deserializer, Serializer, de};

        /// Serializes as `0`/`1`.
        pub fn serialize<S: Serializer>(value: &bool, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_i64(i64::from(*value))
        }

        /// Accepts `0`/`1` (and JSON booleans).
        pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
            #[derive(Deserialize)]
            #[serde(untagged)]
            enum Raw {
                Bool(bool),
                Int(i64),
            }
            match Raw::deserialize(deserializer)? {
                Raw::Bool(b) => Ok(b),
                Raw::Int(0) => Ok(false),
                Raw::Int(1) => Ok(true),
                Raw::Int(n) => Err(de::Error::custom(format!("expected 0 or 1, got {n}"))),
            }
        }
    }

    /// `INTEGER` epoch milliseconds (UTC) ↔ `OffsetDateTime`.
    pub mod millis {
        use serde::{Deserialize, Deserializer, Serializer, de};
        use time::OffsetDateTime;

        /// Epoch milliseconds of `value`, saturating outside the `i64` range.
        pub fn to_millis(value: OffsetDateTime) -> i64 {
            i64::try_from(value.unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX)
        }

        /// The instant `millis` after the epoch, in UTC.
        pub fn from_millis(millis: i64) -> Result<OffsetDateTime, time::error::ComponentRange> {
            OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000)
        }

        /// Serializes as epoch milliseconds.
        pub fn serialize<S: Serializer>(
            value: &OffsetDateTime,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            serializer.serialize_i64(to_millis(*value))
        }

        /// Deserializes from epoch milliseconds.
        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<OffsetDateTime, D::Error> {
            from_millis(i64::deserialize(deserializer)?).map_err(de::Error::custom)
        }

        /// The same for `Option<OffsetDateTime>` (`NULL` ↔ `None`).
        pub mod option {
            use serde::{Deserialize, Deserializer, Serializer, de};
            use time::OffsetDateTime;

            /// Serializes `None` as `null`.
            pub fn serialize<S: Serializer>(
                value: &Option<OffsetDateTime>,
                serializer: S,
            ) -> Result<S::Ok, S::Error> {
                match value {
                    Some(v) => serializer.serialize_some(&super::to_millis(*v)),
                    None => serializer.serialize_none(),
                }
            }

            /// Deserializes `null` as `None`.
            pub fn deserialize<'de, D: Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Option<OffsetDateTime>, D::Error> {
                Option::<i64>::deserialize(deserializer)?
                    .map(|ms| super::from_millis(ms).map_err(de::Error::custom))
                    .transpose()
            }
        }
    }

    /// `TEXT` `YYYY-MM-DD` ↔ `time::Date`.
    pub mod date_text {
        use serde::{Deserialize, Deserializer, Serializer, de};
        use time::Date;
        use time::macros::format_description;

        const FORMAT: &[time::format_description::BorrowedFormatItem<'static>] =
            format_description!("[year]-[month]-[day]");

        /// Formats as `YYYY-MM-DD`.
        pub fn format(value: Date) -> String {
            // Formatting a date with a static, date-only description cannot fail.
            value.format(FORMAT).unwrap_or_default()
        }

        /// Parses `YYYY-MM-DD`.
        pub fn parse(s: &str) -> Result<Date, time::error::Parse> {
            Date::parse(s, FORMAT)
        }

        /// Serializes as `YYYY-MM-DD`.
        pub fn serialize<S: Serializer>(value: &Date, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(&format(*value))
        }

        /// Deserializes from `YYYY-MM-DD`.
        pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Date, D::Error> {
            let s = String::deserialize(deserializer)?;
            parse(&s).map_err(de::Error::custom)
        }

        /// The same for `Option<Date>` (`NULL` ↔ `None`).
        pub mod option {
            use serde::{Deserialize, Deserializer, Serializer, de};
            use time::Date;

            /// Serializes `None` as `null`.
            pub fn serialize<S: Serializer>(
                value: &Option<Date>,
                serializer: S,
            ) -> Result<S::Ok, S::Error> {
                match value {
                    Some(v) => serializer.serialize_some(&super::format(*v)),
                    None => serializer.serialize_none(),
                }
            }

            /// Deserializes `null` as `None`.
            pub fn deserialize<'de, D: Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Option<Date>, D::Error> {
                Option::<String>::deserialize(deserializer)?
                    .map(|s| super::parse(&s).map_err(de::Error::custom))
                    .transpose()
            }
        }
    }

    /// `TEXT` holding JSON ↔ any serde type (`*_json` columns).
    pub mod json_text {
        use serde::de::DeserializeOwned;
        use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser};

        /// Serializes `value` to a JSON string.
        pub fn serialize<T: Serialize, S: Serializer>(
            value: &T,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            let text = serde_json::to_string(value).map_err(ser::Error::custom)?;
            serializer.serialize_str(&text)
        }

        /// Parses the column's JSON text.
        pub fn deserialize<'de, T: DeserializeOwned, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<T, D::Error> {
            let text = String::deserialize(deserializer)?;
            serde_json::from_str(&text).map_err(de::Error::custom)
        }

        /// The same for nullable columns (`NULL` ↔ `None`).
        pub mod option {
            use serde::de::DeserializeOwned;
            use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser};

            /// Serializes `None` as `null`.
            pub fn serialize<T: Serialize, S: Serializer>(
                value: &Option<T>,
                serializer: S,
            ) -> Result<S::Ok, S::Error> {
                match value {
                    Some(v) => {
                        let text = serde_json::to_string(v).map_err(ser::Error::custom)?;
                        serializer.serialize_some(&text)
                    }
                    None => serializer.serialize_none(),
                }
            }

            /// Deserializes `null` as `None`.
            pub fn deserialize<'de, T: DeserializeOwned, D: Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Option<T>, D::Error> {
                Option::<String>::deserialize(deserializer)?
                    .map(|text| serde_json::from_str(&text).map_err(de::Error::custom))
                    .transpose()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;
    use time::macros::{date, datetime};

    use super::*;

    #[test]
    fn params_macro_and_conversions() {
        let org = OrgId::new("org_1");
        let params = params![
            &org,
            OrderStatus::Paid,
            Money::from_cents(1234),
            true,
            None::<i64>
        ];
        assert_eq!(
            params,
            vec![
                Param::Text("org_1".into()),
                Param::Text("paid".into()),
                Param::Int(1234),
                Param::Int(1),
                Param::Null,
            ]
        );
        assert_eq!(Param::from(OrderNumber::new(1042)), Param::Int(1042));
        assert_eq!(
            Param::from(datetime!(2026-10-13 12:00 UTC)),
            Param::Int(1_791_892_800_000)
        );
        assert_eq!(
            Param::from(date!(2026 - 10 - 09)),
            Param::Text("2026-10-09".into())
        );
        assert_eq!(
            Param::from(TicketId::parse("t-7f2m9q").unwrap()),
            Param::Text("T-7F2M9Q".into())
        );
        assert!(Param::try_u64(u64::MAX).is_err());
    }

    #[test]
    fn classifies_sqlite_errors() {
        let e = SqlError::from_sqlite_message("D1_ERROR: CHECK constraint failed: stock >= 0");
        assert!(e.is_constraint(ConstraintKind::Check));
        assert!(!e.is_retryable());
        let e =
            SqlError::from_sqlite_message("UNIQUE constraint failed: orders.org_id, orders.number");
        assert!(e.is_constraint(ConstraintKind::Unique));
        assert!(matches!(
            SqlError::from_sqlite_message("near \"SELEC\": syntax error"),
            SqlError::Database(_)
        ));
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct OrderRow {
        id: String,
        #[serde(with = "column::bool_int")]
        refund_pending: bool,
        #[serde(with = "column::millis")]
        created_at: OffsetDateTime,
        #[serde(default, with = "column::millis::option")]
        paid_at: Option<OffsetDateTime>,
        #[serde(with = "column::date_text")]
        eta: Date,
        #[serde(with = "column::json_text")]
        faults_json: techdepo_domain::FaultToggles,
        #[serde(default, with = "column::json_text::option")]
        billing_address_json: Option<serde_json::Value>,
        total_cents: Money,
        status: OrderStatus,
    }

    #[test]
    fn decodes_rows_with_column_helpers() {
        let row = json!({
            "id": "ord_1",
            "refund_pending": 1,
            "created_at": 1_791_892_800_000_i64,
            "paid_at": null,
            "eta": "2026-10-16",
            "faults_json": "{\"payment\":{\"kind\":\"charge_twice\"}}",
            "billing_address_json": null,
            "total_cents": 143_590,
            "status": "paid",
        });
        let serde_json::Value::Object(row) = row else {
            unreachable!()
        };
        let decoded: OrderRow = decode_row(row).unwrap();
        assert!(decoded.refund_pending);
        assert_eq!(decoded.created_at, datetime!(2026-10-13 12:00 UTC));
        assert_eq!(decoded.paid_at, None);
        assert_eq!(decoded.eta, date!(2026 - 10 - 16));
        assert_eq!(
            decoded.faults_json.payment,
            techdepo_domain::PaymentFault::ChargeTwice
        );
        assert_eq!(decoded.billing_address_json, None);
        assert_eq!(decoded.total_cents, Money::from_cents(143_590));
        assert_eq!(decoded.status, OrderStatus::Paid);
    }

    #[test]
    fn decode_errors_are_typed() {
        let mut row = Row::new();
        row.insert("id".into(), json!(1.5));
        let err = decode_row::<OrderRow>(row).unwrap_err();
        assert!(matches!(err, SqlError::Decode(_)));
    }
}
