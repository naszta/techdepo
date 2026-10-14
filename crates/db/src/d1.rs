//! [`SqlExecutor`] over a Workers D1 binding (wasm only).
//!
//! D1's JS futures and handles are `!Send`; the port's futures must be `Send` (services await
//! them inside `ctx.run`). Every call therefore runs inside one `SendWrapper`ed future, which is
//! sound because a Worker isolate is single-threaded (see the `techdepo_ports` crate docs).
//!
//! Conversions follow the port's row contract: parameters bind as JS values (integers as
//! numbers, which is exact up to 2^53: epoch milliseconds and cents are far below that), and
//! whole numbers in result rows come back as `i64`, so serde decodes them into integer fields.

use send_wrapper::SendWrapper;
use techdepo_ports::{BatchResult, Param, Row, SqlError, SqlExecutor, Statement, async_trait};
use worker::js_sys::Uint8Array;
use worker::wasm_bindgen::JsValue;
use worker::{D1Database, D1PreparedStatement, D1Result};

/// D1 as a [`SqlExecutor`].
pub struct D1Executor {
    db: SendWrapper<D1Database>,
}

impl D1Executor {
    /// Wraps a D1 binding (`env.d1("DB")?`).
    pub fn new(db: D1Database) -> Self {
        Self {
            db: SendWrapper::new(db),
        }
    }

    fn prepare(&self, sql: &str, params: &[Param]) -> Result<D1PreparedStatement, SqlError> {
        let values: Vec<JsValue> = params.iter().map(to_js).collect();
        self.db.prepare(sql).bind(&values).map_err(map_error)
    }
}

#[async_trait]
impl SqlExecutor for D1Executor {
    async fn execute(&self, sql: &str, params: &[Param]) -> Result<u64, SqlError> {
        SendWrapper::new(async move {
            let result = self.prepare(sql, params)?.run().await.map_err(map_error)?;
            check(&result)?;
            Ok(changes(&result))
        })
        .await
    }

    async fn query_all(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, SqlError> {
        SendWrapper::new(async move {
            let result = self.prepare(sql, params)?.all().await.map_err(map_error)?;
            check(&result)?;
            rows(&result)
        })
        .await
    }

    async fn query_first(&self, sql: &str, params: &[Param]) -> Result<Option<Row>, SqlError> {
        Ok(self.query_all(sql, params).await?.into_iter().next())
    }

    async fn batch(&self, statements: &[Statement]) -> Result<Vec<BatchResult>, SqlError> {
        SendWrapper::new(async move {
            let prepared = statements
                .iter()
                .map(|s| self.prepare(&s.sql, &s.params))
                .collect::<Result<Vec<_>, _>>()?;
            let results = self.db.batch(prepared).await.map_err(map_error)?;
            results
                .iter()
                .map(|r| {
                    check(r)?;
                    Ok(BatchResult {
                        rows_affected: changes(r),
                        rows: rows(r)?,
                    })
                })
                .collect()
        })
        .await
    }
}

fn to_js(param: &Param) -> JsValue {
    match param {
        Param::Null => JsValue::NULL,
        // Exact for |v| <= 2^53, which covers epoch milliseconds, cents and counters.
        #[allow(clippy::cast_precision_loss)]
        Param::Int(v) => JsValue::from_f64(*v as f64),
        Param::Real(v) => JsValue::from_f64(*v),
        Param::Text(v) => JsValue::from_str(v),
        Param::Blob(v) => Uint8Array::from(v.as_slice()).into(),
    }
}

fn check(result: &D1Result) -> Result<(), SqlError> {
    match result.error() {
        Some(message) if !result.success() => Err(classify(message)),
        _ => Ok(()),
    }
}

fn changes(result: &D1Result) -> u64 {
    result
        .meta()
        .ok()
        .flatten()
        .and_then(|m| m.changes)
        .map_or(0, |n| u64::try_from(n).unwrap_or(u64::MAX))
}

fn rows(result: &D1Result) -> Result<Vec<Row>, SqlError> {
    let values: Vec<serde_json::Value> = result.results().map_err(map_error)?;
    values
        .into_iter()
        .map(|value| match normalize(value) {
            serde_json::Value::Object(row) => Ok(row),
            other => Err(SqlError::Decode(format!(
                "expected a row object, got {other}"
            ))),
        })
        .collect()
}

/// D1 hands numbers to JS as doubles: turn whole ones back into integers.
fn normalize(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Number(n) if n.is_f64() => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 =>
            {
                #[allow(clippy::cast_possible_truncation)]
                Value::from(f as i64)
            }
            _ => Value::Number(n),
        },
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, normalize(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        other => other,
    }
}

fn map_error(error: worker::Error) -> SqlError {
    let message = match &error {
        worker::Error::D1(e) => e.cause(),
        other => other.to_string(),
    };
    classify(message)
}

/// D1 reports SQLite errors with SQLite's wording, plus its own transient failures. Locally,
/// two `wrangler dev` processes sharing one `--persist-to` database can also see
/// `SQLITE_BUSY` ("database is locked", "Failed to get session commit token: internal error"),
/// which is transient too.
fn classify(message: String) -> SqlError {
    const TRANSIENT: [&str; 8] = [
        "network connection lost",
        "timed out",
        "timeout",
        "overloaded",
        "storage caused object to be reset",
        "internal error",
        "database is locked",
        "sqlite_busy",
    ];
    let lower = message.to_lowercase();
    if TRANSIENT.iter().any(|t| lower.contains(t)) {
        SqlError::Unavailable(message)
    } else {
        SqlError::from_sqlite_message(message)
    }
}
