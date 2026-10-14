//! Small helpers shared by the components: time and ID minting.

use std::time::{SystemTime, UNIX_EPOCH};

use restate_ext::ContextClockExt;
use restate_sdk::prelude::*;
use time::OffsetDateTime;

/// A journaled `SystemTime` as an `OffsetDateTime` (UTC, millisecond precision).
pub(crate) fn to_offset(t: SystemTime) -> OffsetDateTime {
    let millis = t
        .duration_since(UNIX_EPOCH)
        .map(|d| i128::try_from(d.as_millis()).unwrap_or(i128::MAX))
        .unwrap_or(0);
    OffsetDateTime::from_unix_timestamp_nanos(millis * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// The handler's (journaled) now.
pub(crate) async fn now<C: ContextClockExt>(ctx: &C) -> Result<OffsetDateTime, HandlerError> {
    Ok(to_offset(ctx.now().await?))
}

/// The live wall clock. **Only inside `ctx.run`** (its result is then journaled), for checks that
/// must see time pass across retries, such as a simulated outage window.
pub(crate) fn wall_clock() -> OffsetDateTime {
    let millis = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| i128::try_from(d.as_millis()).unwrap_or(i128::MAX))
        .unwrap_or(0);
    OffsetDateTime::from_unix_timestamp_nanos(millis * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// `<prefix>_<32 hex>` from a deterministic UUID.
pub(crate) fn mint(prefix: &str, uuid: restate_sdk::uuid::Uuid) -> String {
    format!("{prefix}_{}", uuid.simple())
}
