//! Orders: the read model the `Order` VO writes (`orders`, `order_items`, `order_events`), the
//! order number sequence, and the demo simulation state (`demo_faults`, `order_clock`).

use serde::{Deserialize, Serialize};
use techdepo_api_types::vocab::{OrderEventKind, PaymentMethod, RefKind};
use techdepo_api_types::{Address, Category};
use techdepo_domain::{
    Actor, FaultToggles, Money, OrderId, OrderNumber, OrderStatus, ProductId, Sku, UserId, VatRate,
    VirtualClock,
};
use techdepo_ports::sql::column;
use techdepo_ports::{SqlExecutorExt, params};
use time::OffsetDateTime;

use crate::inventory::Inventory;
use crate::{Cursor, PageRequest, Param, SqlError, Statement, json_param, repository, stmt};

/// serde for order numbers stored as their numeric part (`INTEGER 1042` ↔ `TD-1042`).
pub mod number_int {
    use serde::{Deserialize, Deserializer};
    use techdepo_domain::OrderNumber;

    /// Deserializes `1042` as `TD-1042`.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<OrderNumber, D::Error> {
        u32::deserialize(d).map(OrderNumber::new)
    }

    /// The same for nullable columns.
    pub mod option {
        use serde::{Deserialize, Deserializer};
        use techdepo_domain::OrderNumber;

        /// Deserializes `null` as `None`.
        pub fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<OrderNumber>, D::Error> {
            Ok(Option::<u32>::deserialize(d)?.map(OrderNumber::new))
        }
    }
}

/// An `orders` row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OrderRow {
    /// Order ID.
    pub id: OrderId,
    /// Order number.
    #[serde(with = "number_int")]
    pub number: OrderNumber,
    /// Customer.
    pub user_id: UserId,
    /// Customer email (snapshot).
    pub customer_email: String,
    /// Customer name (snapshot).
    pub customer_name: Option<String>,
    /// Status.
    pub status: OrderStatus,
    /// Overlay: refund pending.
    #[serde(with = "column::bool_int")]
    pub refund_pending: bool,
    /// Overlay: investigation open.
    #[serde(with = "column::bool_int")]
    pub investigation_open: bool,
    /// The lost order this one replaces.
    pub replacement_for: Option<OrderId>,
    /// Sum of the lines.
    #[serde(rename = "items_total_cents")]
    pub items_total: Money,
    /// Shipping fee.
    #[serde(rename = "shipping_cents")]
    pub shipping: Money,
    /// Total.
    #[serde(rename = "total_cents")]
    pub total: Money,
    /// VAT rate.
    #[serde(rename = "vat_rate_bp")]
    pub vat_rate: VatRate,
    /// Payment method.
    pub payment_method: PaymentMethod,
    /// Shipping address.
    #[serde(rename = "shipping_address_json", with = "column::json_text")]
    pub shipping_address: Address,
    /// Billing address (`None`: same as shipping).
    #[serde(
        rename = "billing_address_json",
        default,
        with = "column::json_text::option"
    )]
    pub billing_address: Option<Address>,
    /// Compatibility errors were overridden at checkout.
    #[serde(with = "column::bool_int")]
    pub compat_override: bool,
    /// Paid.
    #[serde(default, with = "column::millis::option")]
    pub paid_at: Option<OffsetDateTime>,
    /// Shipped.
    #[serde(default, with = "column::millis::option")]
    pub shipped_at: Option<OffsetDateTime>,
    /// Delivered.
    #[serde(default, with = "column::millis::option")]
    pub delivered_at: Option<OffsetDateTime>,
    /// Cancelled.
    #[serde(default, with = "column::millis::option")]
    pub cancelled_at: Option<OffsetDateTime>,
    /// Refunded.
    #[serde(default, with = "column::millis::option")]
    pub refunded_at: Option<OffsetDateTime>,
    /// Returned.
    #[serde(default, with = "column::millis::option")]
    pub returned_at: Option<OffsetDateTime>,
    /// Placed.
    #[serde(with = "column::millis")]
    pub created_at: OffsetDateTime,
    /// Last change.
    #[serde(with = "column::millis")]
    pub updated_at: OffsetDateTime,
}

impl OrderRow {
    /// The billing address in effect (billing, else shipping).
    pub fn effective_billing_address(&self) -> &Address {
        self.billing_address
            .as_ref()
            .unwrap_or(&self.shipping_address)
    }
}

/// An `order_items` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderItemRow {
    /// Line number (1-based).
    pub line_no: u32,
    /// Product.
    pub product_id: ProductId,
    /// SKU as sold.
    pub sku: Sku,
    /// Name as sold.
    pub name: String,
    /// Category as sold.
    pub category: Category,
    /// Unit price as sold.
    #[serde(rename = "unit_price_cents")]
    pub unit_price: Money,
    /// Units.
    pub quantity: u32,
    /// `unit_price × quantity`.
    #[serde(rename = "line_total_cents")]
    pub line_total: Money,
}

/// An `order_events` row.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct OrderEventRow {
    /// The order.
    pub order_id: OrderId,
    /// Event number within the order.
    pub seq: u32,
    /// Kind.
    pub kind: OrderEventKind,
    /// Who.
    pub actor: Actor,
    /// The person, if any.
    pub actor_user_id: Option<UserId>,
    /// Status after the event, if it changed.
    pub status_after: Option<OrderStatus>,
    /// One line.
    pub summary: String,
    /// Related record kind.
    pub ref_kind: Option<RefKind>,
    /// Related record ID.
    pub ref_id: Option<String>,
    /// Details.
    #[serde(rename = "data_json", with = "column::json_text")]
    pub data: serde_json::Value,
    /// Restate invocation.
    pub invocation_id: Option<String>,
    /// Real time.
    #[serde(with = "column::millis")]
    pub at: OffsetDateTime,
    /// The order's simulated time.
    #[serde(with = "column::millis")]
    pub sim_at: OffsetDateTime,
}

/// A `demo_faults` row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DemoFaultsRow {
    /// Armed toggles.
    #[serde(rename = "faults_json", with = "column::json_text")]
    pub faults: FaultToggles,
    /// Email sends fail until this instant.
    #[serde(default, with = "column::millis::option")]
    pub email_outage_until: Option<OffsetDateTime>,
    /// Last change.
    #[serde(with = "column::millis")]
    pub updated_at: OffsetDateTime,
}

/// One line of a new order (the product snapshot as sold).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewOrderItem {
    /// Product.
    pub product_id: ProductId,
    /// SKU.
    pub sku: Sku,
    /// Name.
    pub name: String,
    /// Category.
    pub category: Category,
    /// Unit price.
    pub unit_price: Money,
    /// Units.
    pub quantity: u32,
}

impl NewOrderItem {
    /// `unit_price × quantity`.
    pub fn line_total(&self) -> Money {
        self.unit_price * self.quantity
    }
}

/// Everything needed to write a new order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewOrder {
    /// Order ID.
    pub id: OrderId,
    /// Customer.
    pub user_id: UserId,
    /// Customer email.
    pub customer_email: String,
    /// Customer name.
    pub customer_name: Option<String>,
    /// Lines.
    pub items: Vec<NewOrderItem>,
    /// Shipping fee.
    pub shipping: Money,
    /// VAT rate.
    pub vat_rate: VatRate,
    /// Payment method.
    pub payment_method: PaymentMethod,
    /// Shipping address.
    pub shipping_address: Address,
    /// Billing address (`None`: same as shipping).
    pub billing_address: Option<Address>,
    /// Compatibility errors overridden.
    pub compat_override: bool,
    /// The lost order this replaces (payment method must be `store`).
    pub replacement_for: Option<OrderId>,
}

impl NewOrder {
    /// Sum of the lines.
    pub fn items_total(&self) -> Money {
        self.items.iter().map(NewOrderItem::line_total).sum()
    }

    /// Lines plus shipping.
    pub fn total(&self) -> Money {
        self.items_total() + self.shipping
    }
}

/// A timeline entry to append. `seq` is assigned by the `Order` VO, the only writer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewOrderEvent {
    /// Event number within the order.
    pub seq: u32,
    /// Kind.
    pub kind: OrderEventKind,
    /// Who.
    pub actor: Actor,
    /// The person, if any.
    pub actor_user_id: Option<UserId>,
    /// Status after the event, if it changed.
    pub status_after: Option<OrderStatus>,
    /// One line.
    pub summary: String,
    /// Related record.
    pub reference: Option<(RefKind, String)>,
    /// Details.
    pub data: serde_json::Value,
    /// Restate invocation.
    pub invocation_id: Option<String>,
    /// Real time.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The order's simulated time.
    #[serde(with = "time::serde::rfc3339")]
    pub sim_at: OffsetDateTime,
}

/// Filters of an order list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OrderFilter {
    /// Only this customer's orders.
    pub user_id: Option<UserId>,
    /// Status.
    pub status: Option<OrderStatus>,
    /// Include cancelled orders.
    pub include_cancelled: bool,
    /// Only orders with `refund_pending` or `investigation_open`.
    pub flagged: bool,
    /// Order number.
    pub number: Option<OrderNumber>,
    /// Customer email substring (case-insensitive).
    pub email_like: Option<String>,
}

/// Counts per status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct StatusCountRow {
    /// Status.
    pub status: OrderStatus,
    /// Orders.
    pub count: u32,
}

/// Columns stamped with the time an order reached a status.
const fn milestone_column(status: OrderStatus) -> Option<&'static str> {
    match status {
        OrderStatus::Paid => Some("paid_at"),
        OrderStatus::Shipped => Some("shipped_at"),
        OrderStatus::Delivered => Some("delivered_at"),
        OrderStatus::Cancelled => Some("cancelled_at"),
        OrderStatus::Refunded => Some("refunded_at"),
        OrderStatus::Returned => Some("returned_at"),
        OrderStatus::AwaitingPayment | OrderStatus::Processing | OrderStatus::ReturnRequested => {
            None
        }
    }
}

repository! {
    /// Orders and their timeline, faults and clock.
    Orders
}

impl<'a> Orders<'a> {
    // -- reads --------------------------------------------------------------------------------

    /// An order by ID.
    pub async fn get(&self, id: &OrderId) -> Result<Option<OrderRow>, SqlError> {
        self.sql
            .query_first_as(
                "SELECT * FROM orders WHERE org_id = ?1 AND id = ?2",
                &params![self.org(), id],
            )
            .await
    }

    /// An order by number.
    pub async fn get_by_number(&self, number: OrderNumber) -> Result<Option<OrderRow>, SqlError> {
        self.sql
            .query_first_as(
                "SELECT * FROM orders WHERE org_id = ?1 AND number = ?2",
                &params![self.org(), number],
            )
            .await
    }

    /// An order of this customer's by number (`None` if someone else's).
    pub async fn get_for_user(
        &self,
        user: &UserId,
        number: OrderNumber,
    ) -> Result<Option<OrderRow>, SqlError> {
        self.sql
            .query_first_as(
                "SELECT * FROM orders WHERE org_id = ?1 AND number = ?2 AND user_id = ?3",
                &params![self.org(), number, user],
            )
            .await
    }

    /// The order's lines.
    pub async fn items(&self, id: &OrderId) -> Result<Vec<OrderItemRow>, SqlError> {
        self.sql
            .query_as(
                "SELECT line_no, product_id, sku, name, category, unit_price_cents, quantity, line_total_cents \
                 FROM order_items WHERE org_id = ?1 AND order_id = ?2 ORDER BY line_no",
                &params![self.org(), id],
            )
            .await
    }

    /// The first line of each order and the unit count, for list headlines.
    pub async fn headlines(
        &self,
        ids: &[OrderId],
    ) -> Result<Vec<(OrderId, String, u32)>, SqlError> {
        #[derive(Deserialize)]
        struct H {
            order_id: OrderId,
            name: String,
            lines: u32,
            units: u32,
        }
        let mut out = Vec::with_capacity(ids.len());
        // One query per order keeps the bound parameters under D1's limit of 100.
        for id in ids {
            let h: Option<H> = self
                .sql
                .query_first_as(
                    "SELECT order_id, (SELECT name FROM order_items i2 WHERE i2.org_id = ?1 AND i2.order_id = ?2 ORDER BY line_no LIMIT 1) AS name, \
                     count(*) AS lines, sum(quantity) AS units FROM order_items WHERE org_id = ?1 AND order_id = ?2 GROUP BY order_id",
                    &params![self.org(), id],
                )
                .await?;
            if let Some(h) = h {
                let headline = if h.lines > 1 {
                    format!("{} and {} more", h.name, h.lines - 1)
                } else {
                    h.name
                };
                out.push((h.order_id, headline, h.units));
            }
        }
        Ok(out)
    }

    /// Orders matching `filter`, newest first.
    pub async fn list(
        &self,
        filter: &OrderFilter,
        page: &PageRequest,
    ) -> Result<(Vec<OrderRow>, Option<String>), SqlError> {
        let (key, id) = page.desc_bounds();
        let rows: Vec<OrderRow> = self
            .sql
            .query_as(
                "SELECT * FROM orders WHERE org_id = ?1 \
                 AND (?2 IS NULL OR user_id = ?2) \
                 AND (?3 IS NULL OR status = ?3) \
                 AND (?4 = 1 OR ?3 = 'cancelled' OR status <> 'cancelled') \
                 AND (?5 = 0 OR refund_pending = 1 OR investigation_open = 1) \
                 AND (?6 IS NULL OR number = ?6) \
                 AND (?7 IS NULL OR lower(customer_email) LIKE ?7) \
                 AND (created_at < ?8 OR (created_at = ?8 AND id < ?9)) \
                 ORDER BY created_at DESC, id DESC LIMIT ?10",
                &params![
                    self.org(),
                    filter.user_id.as_ref(),
                    filter.status,
                    filter.include_cancelled,
                    filter.flagged,
                    filter.number,
                    filter
                        .email_like
                        .as_ref()
                        .map(|e| format!("%{}%", e.to_lowercase())),
                    key,
                    id,
                    page.fetch()
                ],
            )
            .await?;
        Ok(page.finish(rows, |r| {
            Cursor::new(column::millis::to_millis(r.created_at), r.id.as_str())
        }))
    }

    /// Orders per status (statuses without orders are absent).
    pub async fn count_by_status(&self) -> Result<Vec<StatusCountRow>, SqlError> {
        self.sql
            .query_as(
                "SELECT status, count(*) AS count FROM orders WHERE org_id = ?1 GROUP BY status",
                &params![self.org()],
            )
            .await
    }

    /// Orders created since `since` (any status, replacements included): the web API's per-org
    /// hourly order cap.
    pub async fn created_since(&self, since: OffsetDateTime) -> Result<u32, SqlError> {
        Ok(self
            .sql
            .query_scalar(
                "SELECT count(*) AS n FROM orders WHERE org_id = ?1 AND created_at >= ?2",
                &params![self.org(), since],
            )
            .await?
            .unwrap_or(0))
    }

    /// Units of each product in orders that still hold stock for shipping (paid or not yet
    /// shipped), for the admin inventory's "reserved" column.
    pub async fn reserved_units(&self) -> Result<Vec<(ProductId, u32)>, SqlError> {
        #[derive(Deserialize)]
        struct R {
            product_id: ProductId,
            units: u32,
        }
        let rows: Vec<R> = self
            .sql
            .query_as(
                "SELECT i.product_id, sum(i.quantity) AS units FROM order_items i \
                 JOIN orders o ON o.org_id = i.org_id AND o.id = i.order_id \
                 WHERE i.org_id = ?1 AND o.status IN ('awaiting_payment', 'paid', 'processing') \
                 GROUP BY i.product_id",
                &params![self.org()],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.product_id, r.units)).collect())
    }

    /// A customer's order count and net spend (succeeded charges minus refunds), for the
    /// admin customer list.
    pub async fn customer_stats(&self, user: &UserId) -> Result<(u32, Money), SqlError> {
        #[derive(Deserialize)]
        struct S {
            orders: u32,
            spent: i64,
        }
        let s: Option<S> = self
            .sql
            .query_first_as(
                "SELECT (SELECT count(*) FROM orders WHERE org_id = ?1 AND user_id = ?2) AS orders, \
                 (SELECT coalesce(sum(CASE p.kind WHEN 'charge' THEN p.amount_cents ELSE -p.amount_cents END), 0) \
                 FROM payments p JOIN orders o ON o.org_id = p.org_id AND o.id = p.order_id \
                 WHERE p.org_id = ?1 AND o.user_id = ?2 AND p.status = 'succeeded') AS spent",
                &params![self.org(), user],
            )
            .await?;
        Ok(s.map_or((0, Money::ZERO), |s| (s.orders, Money::from_cents(s.spent))))
    }

    /// The order's timeline, oldest first.
    pub async fn events(&self, id: &OrderId) -> Result<Vec<OrderEventRow>, SqlError> {
        self.sql
            .query_as(
                "SELECT * FROM order_events WHERE org_id = ?1 AND order_id = ?2 ORDER BY seq",
                &params![self.org(), id],
            )
            .await
    }

    /// The latest events across the org, newest first (admin activity feed).
    pub async fn recent_events(
        &self,
        limit: u32,
    ) -> Result<Vec<(OrderEventRow, OrderNumber)>, SqlError> {
        #[derive(Deserialize)]
        struct R {
            #[serde(flatten)]
            event: OrderEventRow,
            #[serde(with = "number_int")]
            number: OrderNumber,
        }
        let rows: Vec<R> = self
            .sql
            .query_as(
                "SELECT e.*, o.number FROM order_events e JOIN orders o ON o.org_id = e.org_id AND o.id = e.order_id \
                 WHERE e.org_id = ?1 ORDER BY e.at DESC, e.seq DESC LIMIT ?2",
                &params![self.org(), limit],
            )
            .await?;
        Ok(rows.into_iter().map(|r| (r.event, r.number)).collect())
    }

    /// The armed faults.
    pub async fn faults(&self, id: &OrderId) -> Result<Option<DemoFaultsRow>, SqlError> {
        self.sql
            .query_first_as(
                "SELECT faults_json, email_outage_until, updated_at FROM demo_faults WHERE org_id = ?1 AND order_id = ?2",
                &params![self.org(), id],
            )
            .await
    }

    /// The order's simulated clock (zero offset when absent).
    pub async fn clock(&self, id: &OrderId) -> Result<VirtualClock, SqlError> {
        let offset: Option<i64> = self
            .sql
            .query_scalar(
                "SELECT offset_secs FROM order_clock WHERE org_id = ?1 AND order_id = ?2",
                &params![self.org(), id],
            )
            .await?;
        Ok(VirtualClock::from_offset_secs(offset.unwrap_or(0)))
    }

    // -- writes -------------------------------------------------------------------------------

    /// Writes a new order in one batch: allocates its number, inserts the order and its lines,
    /// takes the stock (an oversell aborts the batch: [`crate::inventory::is_oversell`]),
    /// appends `placed` (`event.seq` must be 1), arms `faults` and starts the clock at offset 0.
    ///
    /// The first result's rows hold the allocated number ([`placed_number`]). Not idempotent on
    /// its own: check [`Orders::get`] first, inside the same `ctx.run`.
    pub fn place_statements(
        &self,
        order: &NewOrder,
        faults: &FaultToggles,
        event: &NewOrderEvent,
        at: OffsetDateTime,
    ) -> Result<Vec<Statement>, SqlError> {
        let org = self.org();
        let mut out = vec![
            stmt(
                "INSERT INTO order_sequences (org_id, last_number) VALUES (?1, 1001) \
                 ON CONFLICT (org_id) DO UPDATE SET last_number = last_number + 1 RETURNING last_number",
                params![org],
            ),
            stmt(
                "INSERT INTO orders (org_id, id, number, user_id, customer_email, customer_name, status, \
                 replacement_for, items_total_cents, shipping_cents, total_cents, vat_rate_bp, payment_method, \
                 shipping_address_json, billing_address_json, compat_override, created_at, updated_at) \
                 VALUES (?1, ?2, (SELECT last_number FROM order_sequences WHERE org_id = ?1), ?3, ?4, ?5, 'awaiting_payment', \
                 ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
                vec![
                    org.into(),
                    (&order.id).into(),
                    (&order.user_id).into(),
                    order.customer_email.as_str().into(),
                    order.customer_name.clone().into(),
                    order.replacement_for.as_ref().into(),
                    order.items_total().into(),
                    order.shipping.into(),
                    order.total().into(),
                    order.vat_rate.into(),
                    order.payment_method.as_str().into(),
                    json_param(&order.shipping_address)?,
                    order
                        .billing_address
                        .as_ref()
                        .map(json_param)
                        .transpose()?
                        .unwrap_or(Param::Null),
                    order.compat_override.into(),
                    at.into(),
                ],
            ),
        ];
        let inventory = Inventory::new(self.sql, self.scope);
        for (i, item) in order.items.iter().enumerate() {
            let line_no = u32::try_from(i + 1).unwrap_or(u32::MAX);
            out.push(stmt(
                "INSERT INTO order_items (org_id, order_id, line_no, product_id, sku, name, category, \
                 unit_price_cents, quantity, line_total_cents) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    org,
                    &order.id,
                    line_no,
                    &item.product_id,
                    &item.sku,
                    item.name.as_str(),
                    item.category.as_str(),
                    item.unit_price,
                    item.quantity,
                    item.line_total()
                ],
            ));
            out.push(inventory.take_statement(&item.product_id, item.quantity, at));
        }
        out.push(self.event_statement(&order.id, event)?);
        out.push(self.set_faults_statement(&order.id, faults, None, at)?);
        out.push(self.set_clock_statement(&order.id, VirtualClock::default(), at));
        Ok(out)
    }

    /// Moves the order to `status`, stamping its milestone column the first time.
    pub fn status_statement(
        &self,
        id: &OrderId,
        status: OrderStatus,
        at: OffsetDateTime,
    ) -> Statement {
        let sql: &'static str = match milestone_column(status) {
            Some("paid_at") => {
                "UPDATE orders SET status = ?3, updated_at = ?4, paid_at = coalesce(paid_at, ?4) WHERE org_id = ?1 AND id = ?2"
            }
            Some("shipped_at") => {
                "UPDATE orders SET status = ?3, updated_at = ?4, shipped_at = coalesce(shipped_at, ?4) WHERE org_id = ?1 AND id = ?2"
            }
            Some("delivered_at") => {
                "UPDATE orders SET status = ?3, updated_at = ?4, delivered_at = coalesce(delivered_at, ?4) WHERE org_id = ?1 AND id = ?2"
            }
            Some("cancelled_at") => {
                "UPDATE orders SET status = ?3, updated_at = ?4, cancelled_at = coalesce(cancelled_at, ?4) WHERE org_id = ?1 AND id = ?2"
            }
            Some("refunded_at") => {
                "UPDATE orders SET status = ?3, updated_at = ?4, refunded_at = coalesce(refunded_at, ?4) WHERE org_id = ?1 AND id = ?2"
            }
            Some("returned_at") => {
                "UPDATE orders SET status = ?3, updated_at = ?4, returned_at = coalesce(returned_at, ?4) WHERE org_id = ?1 AND id = ?2"
            }
            _ => "UPDATE orders SET status = ?3, updated_at = ?4 WHERE org_id = ?1 AND id = ?2",
        };
        stmt(sql, params![self.org(), id, status, at])
    }

    /// Sets overlay flags (`None` leaves a flag unchanged).
    pub fn flags_statement(
        &self,
        id: &OrderId,
        refund_pending: Option<bool>,
        investigation_open: Option<bool>,
        at: OffsetDateTime,
    ) -> Statement {
        stmt(
            "UPDATE orders SET refund_pending = coalesce(?3, refund_pending), \
             investigation_open = coalesce(?4, investigation_open), updated_at = ?5 WHERE org_id = ?1 AND id = ?2",
            params![self.org(), id, refund_pending, investigation_open, at],
        )
    }

    /// Bumps `updated_at` (an event without a status change).
    pub fn touch_statement(&self, id: &OrderId, at: OffsetDateTime) -> Statement {
        stmt(
            "UPDATE orders SET updated_at = ?3 WHERE org_id = ?1 AND id = ?2",
            params![self.org(), id, at],
        )
    }

    /// Appends a timeline entry (idempotent on `seq`).
    pub fn event_statement(
        &self,
        id: &OrderId,
        event: &NewOrderEvent,
    ) -> Result<Statement, SqlError> {
        let (ref_kind, ref_id) = match &event.reference {
            Some((kind, rid)) => (Some(kind.as_str()), Some(rid.clone())),
            None => (None, None),
        };
        Ok(stmt(
            "INSERT INTO order_events (org_id, order_id, seq, kind, actor, actor_user_id, status_after, summary, \
             ref_kind, ref_id, data_json, invocation_id, at, sim_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
             ON CONFLICT (org_id, order_id, seq) DO NOTHING",
            vec![
                self.org().into(),
                id.into(),
                event.seq.into(),
                event.kind.as_str().into(),
                event.actor.into(),
                event.actor_user_id.as_ref().into(),
                event.status_after.into(),
                event.summary.as_str().into(),
                ref_kind.into(),
                ref_id.into(),
                json_param(&event.data)?,
                event.invocation_id.clone().into(),
                event.at.into(),
                event.sim_at.into(),
            ],
        ))
    }

    /// Replaces the armed faults.
    pub fn set_faults_statement(
        &self,
        id: &OrderId,
        faults: &FaultToggles,
        email_outage_until: Option<OffsetDateTime>,
        at: OffsetDateTime,
    ) -> Result<Statement, SqlError> {
        Ok(stmt(
            "INSERT INTO demo_faults (org_id, order_id, faults_json, email_outage_until, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (org_id, order_id) DO UPDATE SET \
             faults_json = excluded.faults_json, email_outage_until = excluded.email_outage_until, \
             updated_at = excluded.updated_at",
            vec![
                self.org().into(),
                id.into(),
                json_param(faults)?,
                email_outage_until.into(),
                at.into(),
            ],
        ))
    }

    /// Sets the clock offset (it never decreases).
    pub fn set_clock_statement(
        &self,
        id: &OrderId,
        clock: VirtualClock,
        at: OffsetDateTime,
    ) -> Statement {
        stmt(
            "INSERT INTO order_clock (org_id, order_id, offset_secs, updated_at) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (org_id, order_id) DO UPDATE SET offset_secs = max(order_clock.offset_secs, excluded.offset_secs), \
             updated_at = excluded.updated_at",
            params![self.org(), id, clock.offset_secs().max(0), at],
        )
    }

    /// Returns the order's units to stock (cancelling an unpaid order). Runs once: it is a
    /// no-op when the order is already cancelled, so put it **before** the status change in the
    /// same batch.
    pub fn restock_statement(&self, id: &OrderId, at: OffsetDateTime) -> Statement {
        stmt(
            "UPDATE inventory SET stock = min(stock + (SELECT sum(i.quantity) FROM order_items i \
             WHERE i.org_id = ?1 AND i.order_id = ?2 AND i.product_id = inventory.product_id), 9999), updated_at = ?3 \
             WHERE org_id = ?1 AND product_id IN (SELECT product_id FROM order_items WHERE org_id = ?1 AND order_id = ?2) \
             AND EXISTS (SELECT 1 FROM orders WHERE org_id = ?1 AND id = ?2 AND status <> 'cancelled')",
            params![self.org(), id, at],
        )
    }
}

/// The order number a [`Orders::place_statements`] batch allocated.
pub fn placed_number(results: &[crate::BatchResult]) -> Option<OrderNumber> {
    let value = results
        .first()?
        .rows
        .first()?
        .get("last_number")?
        .as_i64()?;
    u32::try_from(value).ok().map(OrderNumber::new)
}
