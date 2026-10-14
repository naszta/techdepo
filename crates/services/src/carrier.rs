//! `Carrier` (service, ingress-private): the fake delivery company.
//!
//! It owns `shipments` and `carrier_events`. Time is the caller's: every handler takes the
//! order's simulated now. A lost parcel stays "in transit" for the customer; only a trace
//! ([`Carrier`]'s `open_trace`) reveals whether it resurfaces (arriving the next workday) or is gone.
//!
//! A trace takes the carrier a little while ([`TRACE_LATENCY`], real time): the first
//! `open_trace` opens it (`open`), and later calls report `open` until the answer is there, then
//! reveal it. The `Investigation` workflow polls it.
//!
//! The "carrier API down" fault is a window of real time the `Order` VO passes in
//! (`api_down_until`): until it ends, `create_shipment` fails with a retryable error and
//! Restate retries it, which is what the demo shows in the Restate UI.

use std::sync::Arc;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::vocab::{CarrierEventKind, ShipmentStatus, TraceStatus};
use techdepo_db::OrgScope;
use techdepo_db::shipments::{NewCarrierEvent, NewShipment, ShipmentRow};
use techdepo_domain::{
    LostOutcome, OrderId, OrgId, ShipmentId, TrackingNumber, WorkCalendar, carrier_eta,
};
use time::macros::time;
use time::{Date, Duration, OffsetDateTime};

use crate::Deps;
use crate::errors::{not_found, sql};
use crate::util::{self, mint, wall_clock};

/// Where scans happen.
const WAREHOUSE: &str = "TechDepó warehouse, Budapest";

/// A shipment as the `Order` VO keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipmentInfo {
    /// Shipment ID.
    pub id: ShipmentId,
    /// Carrier name.
    pub carrier: String,
    /// Tracking number.
    pub tracking_number: TrackingNumber,
    /// Ship date (simulated).
    #[serde(with = "techdepo_api_types::serde_fmt::date")]
    pub shipped_on: Date,
    /// ETA.
    #[serde(with = "techdepo_api_types::serde_fmt::date")]
    pub eta: Date,
}

impl ShipmentInfo {
    fn from_row(row: &ShipmentRow) -> Self {
        Self {
            id: row.id.clone(),
            carrier: row.carrier.clone(),
            tracking_number: row.tracking_number.clone(),
            shipped_on: row.shipped_on,
            eta: row.eta,
        }
    }
}

/// `Carrier/create_shipment` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateShipmentInput {
    /// Org.
    pub org_id: OrgId,
    /// Order.
    pub order_id: OrderId,
    /// The order's simulated now (the ship date).
    #[serde(with = "time::serde::rfc3339")]
    pub sim_now: OffsetDateTime,
    /// Hidden truth: the parcel will be lost, with this outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost: Option<LostOutcome>,
    /// The "API down" fault: fail (retryably) until this real instant.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub api_down_until: Option<OffsetDateTime>,
}

/// Input of `Carrier/track` and `Carrier/open_trace`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackInput {
    /// Org.
    pub org_id: OrgId,
    /// Order.
    pub order_id: OrderId,
    /// The order's simulated now.
    #[serde(with = "time::serde::rfc3339")]
    pub sim_now: OffsetDateTime,
}

/// `Carrier/track` output: what the carrier says (a lost parcel is "in transit").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackResult {
    /// Shipment.
    pub shipment: ShipmentInfo,
    /// Status.
    pub status: ShipmentStatus,
    /// Delivery date, once delivered.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "techdepo_api_types::serde_fmt::date::option"
    )]
    pub delivered_on: Option<Date>,
}

/// `Carrier/open_trace` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceResult {
    /// Shipment.
    pub shipment: ShipmentInfo,
    /// What the trace found.
    pub status: TraceStatus,
    /// The hidden outcome, now revealed (`None` when the parcel was never lost).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<LostOutcome>,
}

/// `Carrier/move_ship_date` input (demo: "ship date to last Friday").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveShipDateInput {
    /// Org.
    pub org_id: OrgId,
    /// Order.
    pub order_id: OrderId,
    /// New ship date.
    #[serde(with = "techdepo_api_types::serde_fmt::date")]
    pub shipped_on: Date,
}

// -- pure rules ---------------------------------------------------------------------------------

/// How long the carrier takes to answer a trace (real time, demo-sized).
pub const TRACE_LATENCY: Duration = Duration::seconds(15);

/// Whether a trace opened at `opened_at` (real time) has its answer at `now`.
pub fn trace_answered(opened_at: OffsetDateTime, now: OffsetDateTime) -> bool {
    now - opened_at >= TRACE_LATENCY
}

/// A tracking number from random bytes: `KP` and 10 digits.
pub fn tracking_number(bytes: [u8; 16]) -> TrackingNumber {
    let mut n = [0_u8; 8];
    n.copy_from_slice(&bytes[..8]);
    TrackingNumber::new(format!("KP{:010}", u64::from_le_bytes(n) % 10_000_000_000))
}

/// The ETA of a parcel shipped on `shipped_on` (1–3 workdays on the org's `calendar`, fixed per
/// tracking number).
pub fn eta(calendar: &WorkCalendar, shipped_on: Date, tracking: &TrackingNumber) -> Date {
    carrier_eta(calendar, shipped_on, tracking)
}

/// Whether the carrier delivers now: in transit, not missing, and the simulated date reached
/// the ETA.
pub fn delivery_due(missing: bool, status: ShipmentStatus, eta: Date, today: Date) -> bool {
    status == ShipmentStatus::InTransit && !missing && today >= eta
}

/// When a due parcel was delivered on the simulated timeline: on its ETA, at 14:00, unless
/// that is still ahead of simulated now.
pub fn delivered_at(eta: Date, sim_now: OffsetDateTime) -> OffsetDateTime {
    let at = eta.with_time(time!(14:00)).assume_offset(sim_now.offset());
    at.min(sim_now)
}

/// What a trace finds: a parcel that was never lost (or resurfaces) is found, arriving the next
/// workday at the latest (carriers do not deliver at weekends or on the org's holidays); a gone
/// one yields no information.
pub fn trace(
    calendar: &WorkCalendar,
    lost: Option<LostOutcome>,
    eta: Date,
    today: Date,
) -> (TraceStatus, Option<Date>) {
    let tomorrow = calendar.add_workdays(today, 1);
    match lost {
        Some(LostOutcome::Gone) => (TraceStatus::NoInformation, None),
        Some(LostOutcome::Resurfaces) => (TraceStatus::Resurfaced, Some(tomorrow)),
        None => (TraceStatus::Resurfaced, Some(eta.min(tomorrow).max(today))),
    }
}

fn event(
    kind: CarrierEventKind,
    internal: bool,
    description: impl Into<String>,
    at: OffsetDateTime,
    sim_at: OffsetDateTime,
) -> NewCarrierEvent {
    NewCarrierEvent {
        kind,
        internal,
        description: description.into(),
        location: matches!(
            kind,
            CarrierEventKind::LabelCreated | CarrierEventKind::PickedUp
        )
        .then(|| WAREHOUSE.to_owned()),
        at,
        sim_at,
    }
}

// -- the service --------------------------------------------------------------------------------

/// The `Carrier` service.
pub struct Carrier {
    deps: Arc<Deps>,
}

impl Carrier {
    /// The service over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }
}

#[restate_sdk::service(name = "Carrier", ingress_private = true)]
impl Carrier {
    /// Hands the parcel to the carrier: tracking number, ETA, first scans. Idempotent per order.
    #[handler]
    async fn create_shipment(
        &self,
        mut ctx: Context<'_>,
        input: Json<CreateShipmentInput>,
    ) -> Result<Json<ShipmentInfo>, HandlerError> {
        let input = input.into_inner();
        let at = util::now(&ctx).await?;
        let id = ShipmentId::new(mint("shp", ctx.rand_uuid()));
        let tracking = tracking_number(ctx.rand_uuid().into_bytes());
        let shipped_on = input.sim_now.date();
        let mut new = NewShipment {
            id,
            order_id: input.order_id.clone(),
            carrier: self.deps.config.carrier_name.clone(),
            // Set from the org's calendar in `create-shipment`.
            eta: shipped_on,
            tracking_number: tracking,
            lost_outcome: input.lost,
            shipped_on,
            shipped_sim_at: input.sim_now,
            created_at: at,
            events: vec![
                event(
                    CarrierEventKind::LabelCreated,
                    false,
                    "Shipping label created",
                    at,
                    input.sim_now,
                ),
                event(
                    CarrierEventKind::PickedUp,
                    false,
                    "Parcel picked up",
                    at,
                    input.sim_now + Duration::seconds(1),
                ),
            ],
        };
        let db = self.deps.db.clone();
        let scope = OrgScope::verified(input.org_id.clone());
        let down_until = input.api_down_until;
        let info: Json<ShipmentInfo> = ctx
            .run(|| async move {
                if let Some(until) = down_until
                    && wall_clock() < until
                {
                    // Retryable on purpose: Restate retries until the simulated outage ends.
                    return Err(HandlerError::from(format!(
                        "carrier API unavailable (simulated outage until {until})"
                    )));
                }
                let shipments = db.shipments(&scope);
                if let Some(existing) = shipments.for_order(&new.order_id).await.map_err(sql)? {
                    return Ok(Json(ShipmentInfo::from_row(&existing)));
                }
                if db
                    .orders(&scope)
                    .get(&new.order_id)
                    .await
                    .map_err(sql)?
                    .is_none()
                {
                    return Err(not_found("order"));
                }
                let calendar = db.orgs(&scope).settings().await.map_err(sql)?.calendar();
                new.eta = eta(&calendar, new.shipped_on, &new.tracking_number);
                db.batch(&shipments.create_statements(&new))
                    .await
                    .map_err(sql)?;
                Ok(Json(ShipmentInfo {
                    id: new.id.clone(),
                    carrier: new.carrier.clone(),
                    tracking_number: new.tracking_number.clone(),
                    shipped_on: new.shipped_on,
                    eta: new.eta,
                }))
            })
            .name("create-shipment")
            .await?;
        Ok(info)
    }

    /// Tracks the order's parcel at the order's simulated now, delivering it when due.
    #[handler]
    async fn track(
        &self,
        ctx: Context<'_>,
        input: Json<TrackInput>,
    ) -> Result<Json<TrackResult>, HandlerError> {
        let input = input.into_inner();
        let at = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        let scope = OrgScope::verified(input.org_id.clone());
        let result: Json<TrackResult> = ctx
            .run(|| async move {
                let shipments = db.shipments(&scope);
                let row = shipments
                    .for_order(&input.order_id)
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| not_found("shipment"))?;
                let today = input.sim_now.date();
                if !delivery_due(row.is_missing(), row.status, row.eta, today) {
                    return Ok(Json(TrackResult {
                        shipment: ShipmentInfo::from_row(&row),
                        status: row.status,
                        delivered_on: row.delivered_on,
                    }));
                }
                let sim_at = delivered_at(row.eta, input.sim_now);
                let out_for_delivery = sim_at.replace_time(time!(8:00)).min(sim_at);
                db.batch(&[
                    shipments.event_once_statement(
                        &row.id,
                        &event(
                            CarrierEventKind::OutForDelivery,
                            false,
                            "Out for delivery",
                            at,
                            out_for_delivery,
                        ),
                    ),
                    shipments.event_once_statement(
                        &row.id,
                        &event(CarrierEventKind::Delivered, false, "Delivered", at, sim_at),
                    ),
                    shipments.deliver_statement(&row.id, sim_at.date(), sim_at, at),
                ])
                .await
                .map_err(sql)?;
                Ok(Json(TrackResult {
                    shipment: ShipmentInfo::from_row(&row),
                    status: ShipmentStatus::Delivered,
                    delivered_on: Some(sim_at.date()),
                }))
            })
            .name("track")
            .await?;
        Ok(result)
    }

    /// Opens a trace on the order's parcel (`open`), or reports on the open one: still `open`
    /// until [`TRACE_LATENCY`] has passed, then what happened to the parcel. Idempotent: a
    /// concluded trace returns its result again.
    #[handler]
    async fn open_trace(
        &self,
        ctx: Context<'_>,
        input: Json<TrackInput>,
    ) -> Result<Json<TraceResult>, HandlerError> {
        let input = input.into_inner();
        let at = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        let scope = OrgScope::verified(input.org_id.clone());
        let result: Json<TraceResult> = ctx
            .run(|| async move {
                let shipments = db.shipments(&scope);
                let row = shipments
                    .for_order(&input.order_id)
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| not_found("shipment"))?;
                let pending = Json(TraceResult {
                    shipment: ShipmentInfo::from_row(&row),
                    status: TraceStatus::Open,
                    outcome: None,
                });
                match row.trace_status {
                    Some(TraceStatus::Open) => {
                        let opened_at = shipments
                            .events(&row.id, true)
                            .await
                            .map_err(sql)?
                            .into_iter()
                            .find(|e| e.kind == CarrierEventKind::TraceOpened)
                            .map_or(at, |e| e.at);
                        if !trace_answered(opened_at, wall_clock()) {
                            return Ok(pending);
                        }
                    }
                    Some(status) => {
                        return Ok(Json(TraceResult {
                            shipment: ShipmentInfo::from_row(&row),
                            status,
                            outcome: row.lost_outcome,
                        }));
                    }
                    None => {
                        db.batch(&[
                            shipments.event_once_statement(
                                &row.id,
                                &event(
                                    CarrierEventKind::TraceOpened,
                                    true,
                                    "Trace opened",
                                    at,
                                    input.sim_now,
                                ),
                            ),
                            shipments.trace_statement(&row.id, TraceStatus::Open, None, at),
                        ])
                        .await
                        .map_err(sql)?;
                        return Ok(pending);
                    }
                }
                let calendar = db.orgs(&scope).settings().await.map_err(sql)?.calendar();
                let (status, new_eta) =
                    trace(&calendar, row.lost_outcome, row.eta, input.sim_now.date());
                let found = status == TraceStatus::Resurfaced;
                let mut statements = vec![
                    shipments.event_once_statement(
                        &row.id,
                        &if found {
                            event(
                                CarrierEventKind::TraceFound,
                                true,
                                "Trace: parcel located in the network",
                                at,
                                input.sim_now,
                            )
                        } else {
                            event(
                                CarrierEventKind::TraceNoInformation,
                                true,
                                "Trace: no information on the parcel",
                                at,
                                input.sim_now,
                            )
                        },
                    ),
                    shipments.trace_statement(&row.id, status, new_eta, at),
                ];
                if let Some(eta) = new_eta.filter(|_| row.lost_outcome.is_some()) {
                    statements.push(shipments.event_once_statement(
                        &row.id,
                        &event(
                            CarrierEventKind::Delayed,
                            false,
                            format!("Delayed, now expected on {eta}"),
                            at,
                            input.sim_now,
                        ),
                    ));
                }
                db.batch(&statements).await.map_err(sql)?;
                let mut shipment = ShipmentInfo::from_row(&row);
                if let Some(eta) = new_eta {
                    shipment.eta = eta;
                }
                Ok(Json(TraceResult {
                    shipment,
                    status,
                    outcome: row.lost_outcome,
                }))
            })
            .name("open-trace")
            .await?;
        Ok(result)
    }

    /// Moves the ship date (and the scans and ETA with it). Demo control.
    #[handler]
    async fn move_ship_date(
        &self,
        ctx: Context<'_>,
        input: Json<MoveShipDateInput>,
    ) -> Result<Json<ShipmentInfo>, HandlerError> {
        let input = input.into_inner();
        let at = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        let scope = OrgScope::verified(input.org_id.clone());
        let info: Json<ShipmentInfo> = ctx
            .run(|| async move {
                let shipments = db.shipments(&scope);
                let row = shipments
                    .for_order(&input.order_id)
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| not_found("shipment"))?;
                if row.shipped_on == input.shipped_on {
                    return Ok(Json(ShipmentInfo::from_row(&row)));
                }
                let shift_ms = (input.shipped_on - row.shipped_on).whole_milliseconds();
                let shift_ms = i64::try_from(shift_ms).unwrap_or(0);
                let calendar = db.orgs(&scope).settings().await.map_err(sql)?.calendar();
                let new_eta = eta(&calendar, input.shipped_on, &row.tracking_number);
                db.batch(&shipments.move_ship_date_statements(
                    &row.id,
                    input.shipped_on,
                    shift_ms,
                    new_eta,
                    at,
                ))
                .await
                .map_err(sql)?;
                let mut info = ShipmentInfo::from_row(&row);
                info.shipped_on = input.shipped_on;
                info.eta = new_eta;
                Ok(Json(info))
            })
            .name("move-ship-date")
            .await?;
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use time::macros::{date, datetime};

    use super::*;

    #[test]
    fn tracking_numbers_and_eta() {
        let t = tracking_number([7; 16]);
        assert!(t.as_str().starts_with("KP") && t.as_str().len() == 12);
        assert_eq!(t, tracking_number([7; 16]));
        let friday = date!(2026 - 10 - 16);
        let e = eta(&WorkCalendar::new(), friday, &t);
        let days = WorkCalendar::new().workdays_between(friday, e);
        assert!((1..=3).contains(&days));
        // A holiday on Monday pushes the ETA one workday further.
        let holiday = WorkCalendar::with_holidays([date!(2026 - 10 - 19)]);
        assert_eq!(
            eta(&holiday, friday, &t),
            WorkCalendar::new().add_workdays(e, 1)
        );
    }

    #[test]
    fn delivery_rules() {
        let eta = date!(2026 - 10 - 20);
        assert!(!delivery_due(
            false,
            ShipmentStatus::InTransit,
            eta,
            date!(2026 - 10 - 19)
        ));
        assert!(delivery_due(false, ShipmentStatus::InTransit, eta, eta));
        assert!(
            !delivery_due(true, ShipmentStatus::InTransit, eta, date!(2026 - 10 - 30)),
            "lost parcels never arrive"
        );
        assert!(!delivery_due(false, ShipmentStatus::Delivered, eta, eta));
        let now = datetime!(2026-10-22 09:00 UTC);
        assert_eq!(delivered_at(eta, now), datetime!(2026-10-20 14:00 UTC));
        assert_eq!(delivered_at(date!(2026 - 10 - 22), now), now);
    }

    #[test]
    fn traces_reveal_the_outcome() {
        let today = date!(2026 - 10 - 23);
        let eta = date!(2026 - 10 - 20);
        let cal = WorkCalendar::new();
        assert_eq!(
            trace(&cal, Some(LostOutcome::Gone), eta, today),
            (TraceStatus::NoInformation, None)
        );
        // Found on a Friday: it arrives on Monday, not on Saturday.
        assert_eq!(
            trace(&cal, Some(LostOutcome::Resurfaces), eta, today),
            (TraceStatus::Resurfaced, Some(date!(2026 - 10 - 26)))
        );
        // ... or on Tuesday when Monday is a holiday.
        assert_eq!(
            trace(
                &WorkCalendar::with_holidays([date!(2026 - 10 - 26)]),
                Some(LostOutcome::Resurfaces),
                eta,
                today
            ),
            (TraceStatus::Resurfaced, Some(date!(2026 - 10 - 27)))
        );
        assert_eq!(trace(&cal, None, eta, today).0, TraceStatus::Resurfaced);
        let opened = datetime!(2026-10-23 10:00:00 UTC);
        assert!(!trace_answered(opened, opened + Duration::seconds(14)));
        assert!(trace_answered(opened, opened + TRACE_LATENCY));
    }
}
