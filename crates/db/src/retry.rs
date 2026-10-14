//! Retries transient D1 failures, for the workers that have no Restate retries around their
//! queries (the web worker, mail-in).
//!
//! A momentary D1 hiccup (or, in local dev, `SQLITE_BUSY` / "Failed to get session commit token"
//! while another worker writes the shared `--persist-to` database) would otherwise surface as an
//! error page or a failed email delivery. Every call is retried a few times with a short backoff
//! when the error is retryable (`SqlError::is_retryable`). Batches are transactions, so a failed
//! one changed nothing and is safe to repeat; single writes through it must be idempotent (the
//! web worker's member-cache upsert, mail-in's rejection log keyed by message).

use std::time::Duration;

use techdepo_ports::{BatchResult, Param, Row, SqlError, SqlExecutor, Statement, async_trait};

/// Attempts per call.
const ATTEMPTS: u32 = 5;
/// Backoff before the n-th retry: `n × BACKOFF`.
const BACKOFF: Duration = Duration::from_millis(40);

/// A [`SqlExecutor`] that retries retryable errors.
pub struct Retrying<E> {
    inner: E,
}

impl<E> Retrying<E> {
    /// Wraps `inner`.
    pub fn new(inner: E) -> Self {
        Self { inner }
    }
}

macro_rules! retry {
    ($call:expr) => {{
        let mut attempt = 1;
        loop {
            match $call.await {
                Err(e) if e.is_retryable() && attempt < ATTEMPTS => {
                    sleep(BACKOFF * attempt).await;
                    attempt += 1;
                }
                result => break result,
            }
        }
    }};
}

#[async_trait]
impl<E: SqlExecutor> SqlExecutor for Retrying<E> {
    async fn execute(&self, sql: &str, params: &[Param]) -> Result<u64, SqlError> {
        retry!(self.inner.execute(sql, params))
    }

    async fn query_all(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, SqlError> {
        retry!(self.inner.query_all(sql, params))
    }

    async fn query_first(&self, sql: &str, params: &[Param]) -> Result<Option<Row>, SqlError> {
        retry!(self.inner.query_first(sql, params))
    }

    async fn batch(&self, statements: &[Statement]) -> Result<Vec<BatchResult>, SqlError> {
        retry!(self.inner.batch(statements))
    }
}

/// `setTimeout` as a future. Not `worker::Delay`: it unwraps `setTimeout`'s return value as a
/// number, which workerd's timer object is not (it panics). The JS future is `!Send`; sound to
/// wrap, a Worker isolate is single-threaded.
#[cfg(target_arch = "wasm32")]
pub async fn sleep(duration: Duration) {
    use worker::js_sys::{Function, Promise, Reflect, global};
    use worker::wasm_bindgen::{JsCast, JsValue};

    let millis = JsValue::from_f64(duration.as_secs_f64() * 1000.0);
    let promise = Promise::new(&mut |resolve, _reject| {
        let scope = global();
        if let Some(set_timeout) = Reflect::get(&scope, &JsValue::from_str("setTimeout"))
            .ok()
            .and_then(|f| f.dyn_into::<Function>().ok())
        {
            let _ = set_timeout.call2(&scope, &resolve, &millis);
        } else {
            let _ = resolve.call0(&JsValue::NULL);
        }
    });
    let _ =
        send_wrapper::SendWrapper::new(worker::wasm_bindgen_futures::JsFuture::from(promise)).await;
}

/// No-op natively (tests).
#[cfg(not(target_arch = "wasm32"))]
pub async fn sleep(_duration: Duration) {}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    struct Flaky {
        failures: u32,
        calls: AtomicU32,
    }

    #[async_trait]
    impl SqlExecutor for Flaky {
        async fn execute(&self, _: &str, _: &[Param]) -> Result<u64, SqlError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.failures {
                Err(SqlError::Unavailable("database is locked".into()))
            } else {
                Ok(1)
            }
        }
        async fn query_all(&self, _: &str, _: &[Param]) -> Result<Vec<Row>, SqlError> {
            Err(SqlError::Database("syntax".into()))
        }
        async fn query_first(&self, _: &str, _: &[Param]) -> Result<Option<Row>, SqlError> {
            Ok(None)
        }
        async fn batch(&self, _: &[Statement]) -> Result<Vec<BatchResult>, SqlError> {
            Ok(Vec::new())
        }
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        use std::pin::pin;
        use std::task::{Context, Poll, Waker};
        let mut f = pin!(f);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    #[test]
    fn retries_transient_errors_only() {
        let sql = Retrying::new(Flaky {
            failures: 2,
            calls: AtomicU32::new(0),
        });
        assert_eq!(block_on(sql.execute("x", &[])).unwrap(), 1);
        assert_eq!(sql.inner.calls.load(Ordering::SeqCst), 3);
        assert!(block_on(sql.query_all("x", &[])).is_err());

        let hopeless = Retrying::new(Flaky {
            failures: 100,
            calls: AtomicU32::new(0),
        });
        assert!(block_on(hopeless.execute("x", &[])).is_err());
        assert_eq!(hopeless.inner.calls.load(Ordering::SeqCst), ATTEMPTS);
    }
}
