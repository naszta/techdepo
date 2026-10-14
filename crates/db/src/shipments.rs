//! The fake carrier's records (`shipments`, `carrier_events`). One shipment per order.
//!
//! `lost_outcome` and `trace_status` are hidden truth; events with `internal = 1` are hidden from
//! the customer. Event inserts are idempotent: the scan events written with the shipment have
//! fixed `seq`s, and every later event kind happens at most once per shipment, so it is inserted
//! "once per kind" with the next `seq`.

use serde::{Deserialize, Serialize};
use techdepo_api_types::vocab::{CarrierEventKind, ShipmentStatus, TraceStatus};
use techdepo_domain::{LostOutcome, OrderId, ShipmentId, TrackingNumber};
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutorExt, params};
use time::{Date, OffsetDateTime};

use crate::{SqlError, Statement, repository, stmt};

/// A `shipments` row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ShipmentRow {
    /// Shipment ID.
    pub id: ShipmentId,
    /// Order.
    pub order_id: OrderId,
    /// Carrier name.
    pub carrier: String,
    /// Tracking number.
    pub tracking_number: TrackingNumber,
    /// What the customer sees.
    pub status: ShipmentStatus,
    /// Hidden: lost, with the outcome a trace reveals.
    pub lost_outcome: Option<LostOutcome>,
    /// Hidden: trace state.
    pub trace_status: Option<TraceStatus>,
    /// Ship date (simulated).
    #[serde(with = "column::date_text")]
    pub shipped_on: Date,
    /// Ship instant (simulated).
    #[serde(with = "column::millis")]
    pub shipped_sim_at: OffsetDateTime,
    /// ETA.
    #[serde(with = "column::date_text")]
    pub eta: Date,
    /// Delivery date (simulated).
    #[serde(default, with = "column::date_text::option")]
    pub delivered_on: Option<Date>,
    /// Delivery instant (simulated).
    #[serde(default, with = "column::millis::option")]
    pub delivered_sim_at: Option<OffsetDateTime>,
    /// Created.
    #[serde(with = "column::millis")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "column::millis")]
    pub updated_at: OffsetDateTime,
}

impl ShipmentRow {
    /// Whether the parcel is lost and no trace has found it.
    pub fn is_missing(&self) -> bool {
        self.lost_outcome.is_some() && self.trace_status != Some(TraceStatus::Resurfaced)
    }
}

/// A `carrier_events` row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CarrierEventRow {
    /// Event number.
    pub seq: u32,
    /// Kind.
    pub kind: CarrierEventKind,
    /// Hidden from the customer.
    #[serde(with = "column::bool_int")]
    pub internal: bool,
    /// Description.
    pub description: String,
    /// Location.
    pub location: Option<String>,
    /// Real time.
    #[serde(with = "column::millis")]
    pub at: OffsetDateTime,
    /// Simulated time.
    #[serde(with = "column::millis")]
    pub sim_at: OffsetDateTime,
}

/// A carrier event to append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewCarrierEvent {
    /// Kind.
    pub kind: CarrierEventKind,
    /// Hidden from the customer.
    pub internal: bool,
    /// Description.
    pub description: String,
    /// Location.
    pub location: Option<String>,
    /// Real time.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// Simulated time.
    #[serde(with = "time::serde::rfc3339")]
    pub sim_at: OffsetDateTime,
}

/// A shipment to create.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewShipment {
    /// Shipment ID.
    pub id: ShipmentId,
    /// Order.
    pub order_id: OrderId,
    /// Carrier name.
    pub carrier: String,
    /// Tracking number.
    pub tracking_number: TrackingNumber,
    /// Hidden: lost outcome.
    pub lost_outcome: Option<LostOutcome>,
    /// Ship date (simulated).
    pub shipped_on: Date,
    /// Ship instant (simulated).
    #[serde(with = "time::serde::rfc3339")]
    pub shipped_sim_at: OffsetDateTime,
    /// ETA.
    pub eta: Date,
    /// Real time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// The first scan events (`seq` 1, 2, …).
    pub events: Vec<NewCarrierEvent>,
}

repository! {
    /// Shipments and carrier events.
    Shipments
}

impl<'a> Shipments<'a> {
    /// The order's shipment.
    pub async fn for_order(&self, order: &OrderId) -> Result<Option<ShipmentRow>, SqlError> {
        self.sql
            .query_first_as(
                "SELECT * FROM shipments WHERE org_id = ?1 AND order_id = ?2",
                &params![self.org(), order],
            )
            .await
    }

    /// A shipment by tracking number.
    pub async fn by_tracking(
        &self,
        tracking: &TrackingNumber,
    ) -> Result<Option<ShipmentRow>, SqlError> {
        self.sql
            .query_first_as(
                "SELECT * FROM shipments WHERE org_id = ?1 AND tracking_number = ?2",
                &params![self.org(), tracking],
            )
            .await
    }

    /// The shipment's events, oldest first. `include_internal = false` for the customer.
    pub async fn events(
        &self,
        shipment: &ShipmentId,
        include_internal: bool,
    ) -> Result<Vec<CarrierEventRow>, SqlError> {
        self.sql
            .query_as(
                "SELECT seq, kind, internal, description, location, at, sim_at FROM carrier_events \
                 WHERE org_id = ?1 AND shipment_id = ?2 AND (?3 = 1 OR internal = 0) ORDER BY seq",
                &params![self.org(), shipment, include_internal],
            )
            .await
    }

    /// Creates the shipment with its first events (idempotent).
    pub fn create_statements(&self, s: &NewShipment) -> Vec<Statement> {
        let mut out = vec![stmt(
            "INSERT INTO shipments (org_id, id, order_id, carrier, tracking_number, status, lost_outcome, \
             shipped_on, shipped_sim_at, eta, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'in_transit', ?6, ?7, ?8, ?9, ?10, ?10) ON CONFLICT (org_id, id) DO NOTHING",
            params![
                self.org(),
                &s.id,
                &s.order_id,
                s.carrier.as_str(),
                &s.tracking_number,
                s.lost_outcome.map(lost_str),
                s.shipped_on,
                s.shipped_sim_at,
                s.eta,
                s.created_at
            ],
        )];
        for (i, e) in s.events.iter().enumerate() {
            out.push(stmt(
                "INSERT INTO carrier_events (org_id, shipment_id, seq, kind, internal, description, location, at, sim_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT (org_id, shipment_id, seq) DO NOTHING",
                params![
                    self.org(),
                    &s.id,
                    u32::try_from(i + 1).unwrap_or(u32::MAX),
                    e.kind.as_str(),
                    e.internal,
                    e.description.as_str(),
                    e.location.clone(),
                    e.at,
                    e.sim_at
                ],
            ));
        }
        out
    }

    /// Appends an event of `kind` unless the shipment already has one.
    pub fn event_once_statement(&self, shipment: &ShipmentId, e: &NewCarrierEvent) -> Statement {
        stmt(
            "INSERT INTO carrier_events (org_id, shipment_id, seq, kind, internal, description, location, at, sim_at) \
             SELECT ?1, ?2, (SELECT coalesce(max(seq), 0) + 1 FROM carrier_events WHERE org_id = ?1 AND shipment_id = ?2), \
             ?3, ?4, ?5, ?6, ?7, ?8 \
             WHERE NOT EXISTS (SELECT 1 FROM carrier_events WHERE org_id = ?1 AND shipment_id = ?2 AND kind = ?3)",
            params![
                self.org(),
                shipment,
                e.kind.as_str(),
                e.internal,
                e.description.as_str(),
                e.location.clone(),
                e.at,
                e.sim_at
            ],
        )
    }

    /// Marks the parcel delivered.
    pub fn deliver_statement(
        &self,
        shipment: &ShipmentId,
        on: Date,
        sim_at: OffsetDateTime,
        at: OffsetDateTime,
    ) -> Statement {
        stmt(
            "UPDATE shipments SET status = 'delivered', delivered_on = ?3, delivered_sim_at = ?4, updated_at = ?5 \
             WHERE org_id = ?1 AND id = ?2 AND status = 'in_transit'",
            params![self.org(), shipment, on, sim_at, at],
        )
    }

    /// Records the trace state (and, when the parcel resurfaced, its new ETA).
    pub fn trace_statement(
        &self,
        shipment: &ShipmentId,
        trace: TraceStatus,
        eta: Option<Date>,
        at: OffsetDateTime,
    ) -> Statement {
        stmt(
            "UPDATE shipments SET trace_status = ?3, eta = coalesce(?4, eta), updated_at = ?5 WHERE org_id = ?1 AND id = ?2",
            params![self.org(), shipment, trace.as_str(), eta, at],
        )
    }

    /// Moves the ship date back (demo: "ship date to last Friday"): the ship date and instant,
    /// the ETA, and every event's simulated time shift by the same amount.
    pub fn move_ship_date_statements(
        &self,
        shipment: &ShipmentId,
        shipped_on: Date,
        shift_ms: i64,
        eta: Date,
        at: OffsetDateTime,
    ) -> Vec<Statement> {
        vec![
            stmt(
                "UPDATE shipments SET shipped_on = ?3, shipped_sim_at = shipped_sim_at + ?4, eta = ?5, updated_at = ?6 \
                 WHERE org_id = ?1 AND id = ?2",
                params![self.org(), shipment, shipped_on, shift_ms, eta, at],
            ),
            stmt(
                "UPDATE carrier_events SET sim_at = sim_at + ?3 WHERE org_id = ?1 AND shipment_id = ?2",
                params![self.org(), shipment, shift_ms],
            ),
        ]
    }
}

fn lost_str(outcome: LostOutcome) -> &'static str {
    match outcome {
        LostOutcome::Resurfaces => "resurfaces",
        LostOutcome::Gone => "gone",
    }
}
