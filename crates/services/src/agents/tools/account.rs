//! The customer's own records: orders, tracking, invoices and saved builds. Every read is scoped
//! to the org and the customer of the [`ToolCx`] and journaled.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use techdepo_agent::{Runtime, Tool, ToolError};
use techdepo_catalog::Catalog;
use techdepo_db::orders::{OrderFilter, OrderItemRow, OrderRow};
use techdepo_db::{PageRequest, SqlError};
use techdepo_domain::{OrderNumber, ProductId};

use super::ToolCx;
use super::catalog::{check, row};

/// Orders `list_my_orders` returns.
const MAX_ORDERS: usize = 10;
/// Carrier events `get_tracking` returns (the latest).
const MAX_EVENTS: usize = 6;

/// No parameters.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct NoArgs {}

/// Arguments of the per-order tools.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OrderArgs {
    /// The order number, e.g. `TD-1042`.
    pub order_number: String,
}

impl OrderArgs {
    fn number(&self) -> Result<OrderNumber, ToolError> {
        let text = self.order_number.trim();
        text.parse::<OrderNumber>()
            .ok()
            .or_else(|| OrderNumber::find_all(text).first().copied())
            .or_else(|| text.parse::<u32>().ok().map(OrderNumber::new))
            .ok_or_else(|| {
                ToolError::InvalidArguments(format!(
                    "`{text}` is not an order number (they look like TD-1042)"
                ))
            })
    }
}

fn not_yours(number: OrderNumber) -> ToolError {
    ToolError::NotFound(format!(
        "{number} is not one of this customer's orders (it may not exist, or belong to someone \
         else; never discuss other customers' orders)"
    ))
}

fn date(at: Option<time::OffsetDateTime>) -> Value {
    at.map_or(Value::Null, |t| json!(t.date().to_string()))
}

fn order_json(o: &OrderRow, items: &[OrderItemRow]) -> Value {
    let ship_to = &o.shipping_address;
    json!({
        "number": o.number.to_string(),
        "status": o.status,
        "status_label": o.status.label(),
        "placed_on": o.created_at.date().to_string(),
        "paid_on": date(o.paid_at),
        "shipped_on": date(o.shipped_at),
        "delivered_on": date(o.delivered_at),
        "cancelled_on": date(o.cancelled_at),
        "refunded_on": date(o.refunded_at),
        "items": items.iter().map(|i| json!({
            "name": i.name,
            "sku": i.sku,
            "quantity": i.quantity,
            "unit_price": i.unit_price.to_string(),
            "line_total": i.line_total.to_string(),
        })).collect::<Vec<_>>(),
        "items_total": o.items_total.to_string(),
        "shipping": o.shipping.to_string(),
        "total": o.total.to_string(),
        "payment_method": o.payment_method.label(),
        "ship_to": format!("{}, {} {}, {}", ship_to.name, ship_to.postal_code, ship_to.city, ship_to.country),
        "separate_billing_address": o.billing_address.is_some(),
        "refund_in_progress": o.refund_pending,
        "carrier_investigation_open": o.investigation_open,
        "is_replacement": o.replacement_for.is_some(),
    })
}

/// The customer's order `number` as the model sees it, or `None` if it is not theirs.
async fn load_order<R: Runtime>(
    cx: &ToolCx<'_, R>,
    tool: &str,
    number: OrderNumber,
) -> Result<Option<Value>, ToolError> {
    cx.read(tool, move |db, scope, customer| async move {
        let orders = db.orders(&scope);
        let Some(order) = orders.get_for_user(&customer, number).await? else {
            return Ok(None);
        };
        let items = orders.items(&order.id).await?;
        Ok(Some(order_json(&order, &items)))
    })
    .await
}

/// `list_my_orders`.
pub struct ListMyOrders;

impl<R: Runtime> Tool<ToolCx<'_, R>> for ListMyOrders {
    const NAME: &'static str = "list_my_orders";
    const DESCRIPTION: &'static str = "The customer's most recent orders (number, status, date, \
        total, first item). Use it when they do not say which order they mean.";
    type Args = NoArgs;

    async fn call(&self, cx: &ToolCx<'_, R>, _args: NoArgs) -> Result<Value, ToolError> {
        let orders: Vec<Value> = cx
            .read("list_my_orders", |db, scope, customer| async move {
                let filter = OrderFilter {
                    user_id: Some(customer),
                    include_cancelled: true,
                    ..OrderFilter::default()
                };
                let orders = db.orders(&scope);
                let (rows, _) = orders.list(&filter, &PageRequest::first()).await?;
                let mut out = Vec::new();
                for o in rows.into_iter().take(MAX_ORDERS) {
                    let items = orders.items(&o.id).await?;
                    let (headline, count) = techdepo_db::views::headline(&items);
                    out.push(json!({
                        "number": o.number.to_string(),
                        "status": o.status,
                        "status_label": o.status.label(),
                        "placed_on": o.created_at.date().to_string(),
                        "total": o.total.to_string(),
                        "items": headline,
                        "item_count": count,
                    }));
                }
                Ok::<_, SqlError>(out)
            })
            .await?;
        Ok(json!({ "orders": orders }))
    }
}

/// `get_order`.
pub struct GetOrder;

impl<R: Runtime> Tool<ToolCx<'_, R>> for GetOrder {
    const NAME: &'static str = "get_order";
    const DESCRIPTION: &'static str = "One of the customer's orders: status, dates, items, \
        totals, payment method, where it ships.";
    type Args = OrderArgs;

    async fn call(&self, cx: &ToolCx<'_, R>, args: OrderArgs) -> Result<Value, ToolError> {
        let number = args.number()?;
        load_order(cx, "get_order", number)
            .await?
            .ok_or_else(|| not_yours(number))
    }
}

/// `get_tracking`.
pub struct GetTracking;

impl<R: Runtime> Tool<ToolCx<'_, R>> for GetTracking {
    const NAME: &'static str = "get_tracking";
    const DESCRIPTION: &'static str = "The carrier's tracking of one of the customer's orders: \
        carrier, tracking number, status, ship date, expected delivery, latest events.";
    type Args = OrderArgs;

    async fn call(&self, cx: &ToolCx<'_, R>, args: OrderArgs) -> Result<Value, ToolError> {
        let number = args.number()?;
        cx.read("get_tracking", move |db, scope, customer| async move {
            let Some(order) = db.orders(&scope).get_for_user(&customer, number).await? else {
                return Ok(None);
            };
            let shipments = db.shipments(&scope);
            let Some(s) = shipments.for_order(&order.id).await? else {
                return Ok(Some(json!({
                    "order_number": number.to_string(),
                    "order_status": order.status.label(),
                    "status": "not_shipped",
                    "message": "The order has not been handed to the carrier yet.",
                })));
            };
            let events = shipments.events(&s.id, false).await?;
            let skip = events.len().saturating_sub(MAX_EVENTS);
            Ok(Some(json!({
                "order_number": number.to_string(),
                "order_status": order.status.label(),
                "carrier": s.carrier,
                "tracking_number": s.tracking_number,
                "status": s.status,
                "shipped_on": s.shipped_on.to_string(),
                "eta": s.eta.to_string(),
                "delivered_on": s.delivered_on.map(|d| d.to_string()),
                "events": events.iter().skip(skip).map(|e| json!({
                    "what": e.description,
                    "where": e.location,
                    "on": e.at.date().to_string(),
                })).collect::<Vec<_>>(),
            })))
        })
        .await?
        .ok_or_else(|| not_yours(number))
    }
}

/// `get_invoices`.
pub struct GetInvoices;

impl<R: Runtime> Tool<ToolCx<'_, R>> for GetInvoices {
    const NAME: &'static str = "get_invoices";
    const DESCRIPTION: &'static str = "The invoices and credit notes of one of the customer's \
        orders: number, kind, status (issued or voided), date, amounts with VAT, billing name \
        and whether a billing address is on it.";
    type Args = OrderArgs;

    async fn call(&self, cx: &ToolCx<'_, R>, args: OrderArgs) -> Result<Value, ToolError> {
        let number = args.number()?;
        cx.read("get_invoices", move |db, scope, customer| async move {
            let Some(order) = db.orders(&scope).get_for_user(&customer, number).await? else {
                return Ok(None);
            };
            let invoices = db.invoices(&scope).for_order(&order.id).await?;
            let rows: Vec<Value> = invoices
                .iter()
                .map(|i| {
                    json!({
                        "number": i.number.to_string(),
                        "kind": i.kind.label(),
                        "status": i.status.label(),
                        "issued_on": i.issue_date.to_string(),
                        "total": i.gross.to_string(),
                        "net": i.net.to_string(),
                        "vat": i.vat.to_string(),
                        "billing_name": i.billing_name,
                        "has_billing_address": i.billing_address.is_some(),
                        "replaces": i.supersedes_number.as_ref().map(ToString::to_string),
                        "replaced_by": i.superseded_by_number.as_ref().map(ToString::to_string),
                        "credit_for": i.credit_for_number.as_ref().map(ToString::to_string),
                    })
                })
                .collect();
            Ok(Some(json!({
                "order_number": number.to_string(),
                "invoices": rows,
                "note": "The customer can download each invoice from the order page in their account.",
            })))
        })
        .await?
        .ok_or_else(|| not_yours(number))
    }
}

/// `get_my_builds`.
pub struct GetMyBuilds;

impl<R: Runtime> Tool<ToolCx<'_, R>> for GetMyBuilds {
    const NAME: &'static str = "get_my_builds";
    const DESCRIPTION: &'static str = "The customer's saved PC builds from the shop's builder: \
        name, parts (with IDs) and the compatibility engine's verdict.";
    type Args = NoArgs;

    async fn call(&self, cx: &ToolCx<'_, R>, _args: NoArgs) -> Result<Value, ToolError> {
        let builds: Vec<(String, Vec<ProductId>)> = cx
            .read("get_my_builds", |db, scope, customer| async move {
                let rows = db.builds(&scope).list(&customer).await?;
                Ok(rows.into_iter().map(|b| (b.name, b.product_ids)).collect())
            })
            .await?;
        let catalog = Catalog::global();
        let rows: Vec<Value> = builds
            .iter()
            .map(|(name, ids)| {
                let parts: Vec<_> = ids.iter().filter_map(|id| catalog.get(id)).collect();
                let report = (parts.len() >= 2).then(|| check(&parts).ok()).flatten();
                json!({
                    "name": name,
                    "parts": parts.iter().map(|p| row(p)).collect::<Vec<_>>(),
                    "verdict": report.as_ref().map(|r| r.verdict),
                    "issues": report.as_ref().map(|r| r.issues.iter().map(|i| i.message.clone()).collect::<Vec<_>>()),
                })
            })
            .collect();
        Ok(json!({ "builds": rows }))
    }
}
