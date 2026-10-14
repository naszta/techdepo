//! The seeded shop the agent scenarios run in: an in-memory SQLite database with the
//! migrations, one org with the catalog in stock, the customer Ada with one order, another
//! customer's order, and the ticket the traces belong to.

use std::sync::Arc;

use serde_json::json;
use techdepo_api_types::Address;
use techdepo_api_types::vocab::{OrderEventKind, PaymentMethod};
use techdepo_catalog::Catalog;
use techdepo_db::orders::{NewOrder, NewOrderEvent, NewOrderItem, placed_number};
use techdepo_db::{Db, OrgScope, SqliteExecutor, Statement};
use techdepo_domain::{
    Actor, FaultToggles, Money, OrderId, OrderStatus, OrgId, OrgSettings, PublicOrgId, TicketId,
    UserId, VatRate,
};
use techdepo_llm::LlmPorts;
use techdepo_ports::testing::RecordingMailer;
use techdepo_services::ports::FixedModels;
use techdepo_services::support::contract::OrderBrief;
use techdepo_services::{Config, Deps, Ports, Services};
use time::OffsetDateTime;

/// The org.
pub const ORG: &str = "org_eval";
/// Its public ID.
pub const PUBLIC_ORG: &str = "eval2k9q";
/// The customer.
pub const CUSTOMER: &str = "user_ada";
/// The ticket every scenario answers.
pub const TICKET: &str = "T-EVA7Q2";

/// A seeded shop.
pub struct Shop {
    /// The services' dependencies over the shop's database and the models under evaluation.
    pub deps: Arc<Deps>,
    /// Ada's order.
    pub mine: OrderBrief,
    /// Another customer's order.
    pub theirs: OrderBrief,
}

impl Shop {
    /// A fresh shop answering with `models`.
    pub async fn seed(models: &LlmPorts, now: OffsetDateTime) -> Result<Self, String> {
        let sql = Arc::new(SqliteExecutor::in_memory().map_err(|e| e.to_string())?);
        let mut ports = Ports::new(sql, Arc::new(RecordingMailer::new()));
        // The eval's models answer every task: no model settings are stored.
        ports.models = Arc::new(FixedModels::new(
            models.llm.clone(),
            models.classifier.clone(),
        ));
        let deps = Services::new(ports, Config::default()).deps().clone();
        let db = &deps.db;
        let scope = OrgScope::verified(OrgId::new(ORG));
        let public = PublicOrgId::parse(PUBLIC_ORG).map_err(|e| e.to_string())?;
        let mut statements = vec![
            db.orgs(&scope)
                .insert_statement(
                    &public,
                    "Eval shop",
                    &OrgSettings::default(),
                    techdepo_catalog::CATALOG_VERSION,
                    now,
                    None,
                )
                .map_err(|e| e.to_string())?,
        ];
        statements.extend(db.inventory(&scope).seed_statements(Catalog::global(), now));
        statements.push(Statement::new(
            "INSERT INTO tickets (org_id, id, user_id, customer_email, subject, channel, status, \
             created_at, updated_at) VALUES (?1, ?2, ?3, 'ada@example.com', 'Question', 'email', \
             'routing', ?4, ?4)",
            vec![ORG.into(), TICKET.into(), CUSTOMER.into(), now.into()],
        ));
        db.batch(&statements).await.map_err(|e| e.to_string())?;
        let theirs = place(
            db,
            &scope,
            Order {
                id: "ord_bob",
                user: "user_bob",
                email: "bob@example.com",
                name: "Bob Builder",
                product: "nordwatt-prime-850",
            },
            now,
        )
        .await?;
        let mine = place(
            db,
            &scope,
            Order {
                id: "ord_ada",
                user: CUSTOMER,
                email: "ada@example.com",
                name: "Ada Lovelace",
                product: "kestrel-b650-aero",
            },
            now,
        )
        .await?;
        Ok(Self { deps, mine, theirs })
    }

    /// The ticket.
    pub fn ticket() -> TicketId {
        TicketId::parse(TICKET).unwrap_or_else(|_| unreachable!("a valid ticket ID"))
    }
}

struct Order<'a> {
    id: &'a str,
    user: &'a str,
    email: &'a str,
    name: &'a str,
    product: &'a str,
}

async fn place(
    db: &Db,
    scope: &OrgScope,
    o: Order<'_>,
    now: OffsetDateTime,
) -> Result<OrderBrief, String> {
    let product = Catalog::global()
        .get(o.product)
        .ok_or_else(|| format!("no product {}", o.product))?;
    let user = UserId::new(o.user);
    let order = NewOrder {
        id: OrderId::new(o.id),
        user_id: user.clone(),
        customer_email: o.email.into(),
        customer_name: Some(o.name.into()),
        items: vec![NewOrderItem {
            product_id: product.id.clone(),
            sku: product.sku.clone(),
            name: product.name.clone(),
            category: product.category,
            unit_price: product.price,
            quantity: 1,
        }],
        shipping: Money::ZERO,
        vat_rate: VatRate::DEFAULT,
        payment_method: PaymentMethod::Card,
        shipping_address: Address {
            name: o.name.into(),
            line1: "Fő utca 1".into(),
            postal_code: "1011".into(),
            city: "Budapest".into(),
            country: "HU".into(),
            ..Address::default()
        },
        billing_address: None,
        compat_override: false,
        replacement_for: None,
    };
    let event = NewOrderEvent {
        seq: 1,
        kind: OrderEventKind::Placed,
        actor: Actor::Customer,
        actor_user_id: Some(user),
        status_after: Some(OrderStatus::AwaitingPayment),
        summary: "Order placed".into(),
        reference: None,
        data: json!({}),
        invocation_id: None,
        at: now,
        sim_at: now,
    };
    let statements = db
        .orders(scope)
        .place_statements(&order, &FaultToggles::default(), &event, now)
        .map_err(|e| e.to_string())?;
    let result = db.batch(&statements).await.map_err(|e| e.to_string())?;
    let number = placed_number(&result).ok_or("the order got no number")?;
    Ok(OrderBrief {
        order_id: OrderId::new(o.id),
        number,
        status: OrderStatus::AwaitingPayment,
    })
}
