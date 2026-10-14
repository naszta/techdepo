//! [`SqlExecutor`] over an in-process SQLite database (native only): the same SQL and
//! migrations as D1, for tests now and the e2e backend later.

use std::sync::Mutex;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, ToSql, params_from_iter};
use techdepo_ports::{BatchResult, Param, Row, SqlError, SqlExecutor, Statement, async_trait};

/// The migrations, in order (`migrations/*.sql`).
pub const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_init.sql",
        include_str!("../../../migrations/0001_init.sql"),
    ),
    (
        "0002_one_invoice_replacement.sql",
        include_str!("../../../migrations/0002_one_invoice_replacement.sql"),
    ),
    (
        "0003_org_expiry.sql",
        include_str!("../../../migrations/0003_org_expiry.sql"),
    ),
    (
        "0004_org_features.sql",
        include_str!("../../../migrations/0004_org_features.sql"),
    ),
    (
        "0005_app_settings.sql",
        include_str!("../../../migrations/0005_app_settings.sql"),
    ),
];

/// SQLite as a [`SqlExecutor`]. Calls are synchronous under a mutex.
pub struct SqliteExecutor {
    conn: Mutex<Connection>,
}

impl SqliteExecutor {
    /// A fresh in-memory database with the migrations applied and foreign keys on (as D1).
    pub fn in_memory() -> Result<Self, SqlError> {
        Self::migrated(Connection::open_in_memory().map_err(map_error)?)
    }

    /// A database file, created and migrated if new (an existing file is used as is).
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, SqlError> {
        let path = path.as_ref();
        let fresh = !path.exists();
        let conn = Connection::open(path).map_err(map_error)?;
        if fresh {
            return Self::migrated(conn);
        }
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(map_error)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn migrated(conn: Connection) -> Result<Self, SqlError> {
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(map_error)?;
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).map_err(map_error)?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn with<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, SqlError>,
    ) -> Result<T, SqlError> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| SqlError::Unavailable("sqlite mutex poisoned".into()))?;
        f(&mut conn)
    }
}

#[async_trait]
impl SqlExecutor for SqliteExecutor {
    async fn execute(&self, sql: &str, params: &[Param]) -> Result<u64, SqlError> {
        self.with(|conn| run(conn, sql, params).map(|r| r.rows_affected))
    }

    async fn query_all(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, SqlError> {
        self.with(|conn| run(conn, sql, params).map(|r| r.rows))
    }

    async fn query_first(&self, sql: &str, params: &[Param]) -> Result<Option<Row>, SqlError> {
        Ok(self.query_all(sql, params).await?.into_iter().next())
    }

    async fn batch(&self, statements: &[Statement]) -> Result<Vec<BatchResult>, SqlError> {
        self.with(|conn| {
            let tx = conn.transaction().map_err(map_error)?;
            let mut out = Vec::with_capacity(statements.len());
            for s in statements {
                out.push(run(&tx, &s.sql, &s.params)?);
            }
            tx.commit().map_err(map_error)?;
            Ok(out)
        })
    }
}

fn run(conn: &Connection, sql: &str, params: &[Param]) -> Result<BatchResult, SqlError> {
    let mut stmt = conn.prepare(sql).map_err(map_error)?;
    let names: Vec<String> = stmt.column_names().into_iter().map(str::to_owned).collect();
    let bound: Vec<Bind<'_>> = params.iter().map(Bind).collect();
    let mut rows = Vec::new();
    {
        let mut cursor = stmt.query(params_from_iter(bound)).map_err(map_error)?;
        while let Some(row) = cursor.next().map_err(map_error)? {
            let mut map = Row::new();
            for (i, name) in names.iter().enumerate() {
                let value = row.get_ref(i).map_err(map_error)?;
                map.insert(name.clone(), to_json(value));
            }
            rows.push(map);
        }
    }
    let verb = sql
        .trim_start()
        .get(..6)
        .unwrap_or_default()
        .to_ascii_uppercase();
    let writes = matches!(verb.as_str(), "INSERT" | "UPDATE" | "DELETE");
    let rows_affected = if writes { conn.changes() } else { 0 };
    Ok(BatchResult {
        rows_affected,
        rows,
    })
}

/// A `Param` as a rusqlite parameter.
struct Bind<'a>(&'a Param);

impl ToSql for Bind<'_> {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, Value};
        Ok(match self.0 {
            Param::Null => ToSqlOutput::Owned(Value::Null),
            Param::Int(v) => ToSqlOutput::Owned(Value::Integer(*v)),
            Param::Real(v) => ToSqlOutput::Owned(Value::Real(*v)),
            Param::Text(v) => ToSqlOutput::Borrowed(ValueRef::Text(v.as_bytes())),
            Param::Blob(v) => ToSqlOutput::Borrowed(ValueRef::Blob(v)),
        })
    }
}

fn to_json(value: ValueRef<'_>) -> serde_json::Value {
    match value {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(i) => i.into(),
        ValueRef::Real(f) => f.into(),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned().into(),
        ValueRef::Blob(b) => b.iter().copied().map(serde_json::Value::from).collect(),
    }
}

fn map_error(error: rusqlite::Error) -> SqlError {
    SqlError::from_sqlite_message(error.to_string())
}
