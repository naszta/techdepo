//! The repositories' SQL against real SQLite with the real migration (D1 is SQLite).

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use futures::executor::block_on;
use techdepo_api_types::Address;
use techdepo_api_types::session::SupportMailbox;
use techdepo_api_types::vocab::{
    CarrierEventKind, OrderEventKind, PaymentKind, PaymentMethod, PaymentStatus, TraceStatus,
};
use techdepo_api_types::{BuildId, PaymentId};
use techdepo_catalog::Catalog;
use techdepo_db::invoices::{NUMBER_PLACEHOLDER, NewInvoice, NewInvoiceLine, issued_number};
use techdepo_db::orders::{NewOrder, NewOrderEvent, NewOrderItem, OrderFilter, placed_number};
use techdepo_db::payments::NewPayment;
use techdepo_db::shipments::{NewCarrierEvent, NewShipment};
use techdepo_db::{Db, OrgScope, PageRequest, SqliteExecutor, Statement, inventory, views};
use techdepo_domain::{
    Actor, FaultToggles, InvoiceId, InvoiceKind, InvoiceNumber, InvoiceStatus, LostOutcome, Money,
    OrderId, OrderNumber, OrderStatus, OrgFeature, OrgId, OrgSettings, PublicOrgId, ShipmentId,
    TrackingNumber, UserId, VatRate, VirtualClock,
};
use time::OffsetDateTime;
use time::macros::{date, datetime};

const NOW: OffsetDateTime = datetime!(2026-10-16 14:00 UTC);

fn db() -> Db {
    Db::new(Arc::new(
        SqliteExecutor::in_memory().expect("migrations apply"),
    ))
}

fn support_mailbox() -> SupportMailbox {
    SupportMailbox {
        local_part: "support".into(),
        domain: "techdepo.dev".into(),
    }
}

fn provision(db: &Db, scope: &OrgScope) {
    let orgs = db.orgs(scope);
    let mut statements = vec![
        orgs.insert_statement(
            &PublicOrgId::parse("k3x9q2ab").unwrap(),
            "Demo shop",
            &OrgSettings::default(),
            techdepo_catalog::CATALOG_VERSION,
            NOW,
            None,
        )
        .unwrap(),
    ];
    statements.extend(db.inventory(scope).seed_statements(Catalog::global(), NOW));
    block_on(db.batch(&statements)).unwrap();
}

/// Expires `scope`'s org ([`techdepo_db::orgs::Orgs::delete_statements`]) and checks that no
/// table keeps a row of it, while another org is untouched.
fn expire_and_check(db: &Db, scope: &OrgScope) {
    use techdepo_ports::SqlExecutorExt;
    #[derive(serde::Deserialize)]
    struct Name {
        name: String,
    }
    #[derive(serde::Deserialize)]
    struct Count {
        n: u32,
    }

    let other = OrgScope::verified(OrgId::new("org_keep"));
    let keep = db.orgs(&other);
    block_on(
        db.batch(&[keep
            .insert_statement(
                &PublicOrgId::parse("keep0000").unwrap(),
                "Kept",
                &OrgSettings::default(),
                "v",
                NOW,
                None,
            )
            .unwrap()]),
    )
    .unwrap();
    block_on(db.batch(&db.inventory(&other).seed_statements(Catalog::global(), NOW))).unwrap();

    let orgs = db.orgs(scope);
    let at = NOW + time::Duration::days(30);
    block_on(db.batch(&[orgs.set_expiry_statement(Some(at), NOW)])).unwrap();
    assert_eq!(block_on(orgs.get()).unwrap().unwrap().expires_at, Some(at));
    let upsert = techdepo_db::orgs::MemberUpsert {
        user_id: UserId::new("user_1"),
        email: "ada@example.com".into(),
        name: None,
        role: techdepo_api_types::vocab::OrgRole::Admin,
        source: techdepo_api_types::vocab::MemberSource::Dev,
    };
    block_on(orgs.upsert_member(&upsert, NOW)).unwrap();

    block_on(db.batch(&orgs.delete_statements())).unwrap();
    assert!(block_on(orgs.get()).unwrap().is_none());
    let tables: Vec<Name> = block_on(db.sql().query_as(
        "SELECT m.name FROM sqlite_master m WHERE m.type = 'table' \
         AND EXISTS (SELECT 1 FROM pragma_table_info(m.name) WHERE name = 'org_id')",
        &[],
    ))
    .unwrap();
    assert!(tables.len() > 20, "{} tables", tables.len());
    for table in tables {
        let left: Count = block_on(db.sql().query_first_as(
            &format!("SELECT count(*) AS n FROM {} WHERE org_id = ?1", table.name),
            &[scope.org_id().into()],
        ))
        .unwrap()
        .unwrap();
        assert_eq!(left.n, 0, "{} keeps rows of the expired org", table.name);
    }
    assert!(block_on(keep.get()).unwrap().is_some());
    assert_eq!(
        block_on(db.inventory(&other).list()).unwrap().len(),
        Catalog::global().len()
    );
}

fn address() -> Address {
    Address {
        name: "Ada Lovelace".into(),
        line1: "Fő utca 1".into(),
        postal_code: "1011".into(),
        city: "Budapest".into(),
        country: "HU".into(),
        ..Address::default()
    }
}

fn place(
    db: &Db,
    scope: &OrgScope,
    id: &str,
    quantity: u32,
) -> Result<OrderNumber, techdepo_db::SqlError> {
    let product = &Catalog::global().all()[0];
    let order = NewOrder {
        id: OrderId::new(id),
        user_id: UserId::new("user_1"),
        customer_email: "ada@example.com".into(),
        customer_name: Some("Ada".into()),
        items: vec![NewOrderItem {
            product_id: product.id.clone(),
            sku: product.sku.clone(),
            name: product.name.clone(),
            category: product.category,
            unit_price: product.price,
            quantity,
        }],
        shipping: Money::ZERO,
        vat_rate: VatRate::DEFAULT,
        payment_method: PaymentMethod::Card,
        shipping_address: address(),
        billing_address: None,
        compat_override: false,
        replacement_for: None,
    };
    let event = NewOrderEvent {
        seq: 1,
        kind: OrderEventKind::Placed,
        actor: Actor::Customer,
        actor_user_id: Some(UserId::new("user_1")),
        status_after: Some(OrderStatus::AwaitingPayment),
        summary: "Order placed".into(),
        reference: None,
        data: serde_json::json!({}),
        invocation_id: None,
        at: NOW,
        sim_at: NOW,
    };
    let statements =
        db.orders(scope)
            .place_statements(&order, &FaultToggles::default(), &event, NOW)?;
    let results = block_on(db.batch(&statements))?;
    Ok(placed_number(&results).expect("number returned"))
}

#[test]
fn org_features_skip_unknown_names() {
    let db = db();
    let scope = OrgScope::verified(OrgId::new("org_1"));
    provision(&db, &scope);
    // Switched on by hand (no handler writes features).
    let enable = Statement::new(
        "UPDATE orgs SET features_json = '[\"nope\", \"muhaha\"]' WHERE id = ?1",
        vec![scope.org_id().into()],
    );
    block_on(db.batch(&[enable])).unwrap();
    let org = block_on(db.orgs(&scope).get()).unwrap().unwrap();
    assert_eq!(org.features, [OrgFeature::Muhaha]);
    assert_eq!(
        views::org_info(&org, &support_mailbox()).features,
        [OrgFeature::Muhaha]
    );
}

#[test]
fn app_settings_hold_the_model_settings() {
    use techdepo_domain::{ModelChoice, ModelProvider, ModelSettings};

    let db = db();
    let settings = db.app_settings();
    assert_eq!(block_on(settings.models()).unwrap(), None);
    let defaults = ModelSettings::mock();
    assert_eq!(
        block_on(settings.models_or(defaults.clone())).unwrap(),
        defaults
    );

    let mut chosen = ModelSettings::uniform(
        ModelChoice::new(ModelProvider::Anthropic, "claude-sonnet-5"),
        ModelChoice::new(ModelProvider::WorkersAi, "@cf/cloudflare/clef"),
        true,
    );
    let by = UserId::new("user_op");
    for _ in 0..2 {
        let save = settings.set_models_statement(&chosen, &by, NOW).unwrap();
        block_on(db.batch(&[save])).unwrap();
        chosen.judge = ModelChoice::new(ModelProvider::OpenAi, "gpt-5-mini");
    }
    let stored = block_on(settings.models()).unwrap().unwrap();
    assert_eq!(stored.updated_by, Some(by));
    assert_eq!(stored.updated_at, NOW);
    assert_eq!(stored.settings.as_ref(), Some(&chosen), "an upsert");

    // A value that no longer parses counts as no settings.
    block_on(db.batch(&[Statement::new(
        "UPDATE app_settings SET value_json = '{\"judge\": 1}'",
        vec![],
    )]))
    .unwrap();
    assert_eq!(block_on(settings.models()).unwrap().unwrap().settings, None);
    assert_eq!(
        block_on(settings.models_or(defaults.clone())).unwrap(),
        defaults
    );

    block_on(db.batch(&[settings.clear_models_statement()])).unwrap();
    assert_eq!(block_on(settings.models()).unwrap(), None);
}

#[test]
fn the_operator_lists_every_org() {
    let db = db();
    assert!(
        block_on(techdepo_db::orgs::list_all(db.sql()))
            .unwrap()
            .is_empty()
    );
    let first = OrgScope::verified(OrgId::new("org_1"));
    provision(&db, &first);
    let second = OrgScope::verified(OrgId::new("org_2"));
    let later = NOW + time::Duration::hours(1);
    block_on(
        db.batch(&[db
            .orgs(&second)
            .insert_statement(
                &PublicOrgId::parse("zz9q2abc").unwrap(),
                "Second",
                &OrgSettings::default(),
                "old",
                later,
                None,
            )
            .unwrap()]),
    )
    .unwrap();
    place(&db, &first, "ord_1", 1).unwrap();

    let all = block_on(techdepo_db::orgs::list_all(db.sql())).unwrap();
    let names: Vec<&str> = all.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["Second", "Demo shop"], "newest first");
    assert_eq!((all[1].orders, all[1].tickets, all[1].members), (1, 0, 0));
    assert_eq!(all[0].catalog_version.as_deref(), Some("old"));
    assert_eq!(
        block_on(techdepo_db::orgs::all_ids(db.sql())).unwrap(),
        [OrgId::new("org_1"), OrgId::new("org_2")],
        "oldest first"
    );
}

#[test]
fn org_provisioning_and_settings() {
    let db = db();
    let scope = OrgScope::verified(OrgId::new("org_1"));
    provision(&db, &scope);
    provision(&db, &scope); // idempotent
    let org = block_on(db.orgs(&scope).get()).unwrap().unwrap();
    assert_eq!(org.public_id.as_str(), "k3x9q2ab");
    assert_eq!(org.settings, OrgSettings::default());
    assert!(org.features.is_empty());
    let stock = block_on(db.inventory(&scope).list()).unwrap();
    assert_eq!(stock.len(), Catalog::global().len());

    let settings = OrgSettings {
        vat_rate_bp: 2000,
        ..OrgSettings::default()
    };
    block_on(db.orgs(&scope).update_settings(&settings, NOW)).unwrap();
    assert_eq!(
        block_on(db.orgs(&scope).settings()).unwrap().vat_rate_bp,
        2000
    );

    let found = block_on(techdepo_db::orgs::find_by_public_id(
        db.sql(),
        &org.public_id,
    ))
    .unwrap();
    assert_eq!(found.unwrap().id, org.id);

    // Another org cannot take the same public ID.
    let other = OrgScope::verified(OrgId::new("org_2"));
    let clash = db
        .orgs(&other)
        .insert_statement(&org.public_id, "x", &settings, "v", NOW, None)
        .unwrap();
    let err = block_on(db.batch(&[clash])).unwrap_err();
    assert!(
        err.is_constraint(techdepo_ports::ConstraintKind::Unique),
        "{err}"
    );
}

#[test]
fn cart_builds_addresses() {
    let db = db();
    let scope = OrgScope::verified(OrgId::new("org_1"));
    provision(&db, &scope);
    let user = UserId::new("user_1");
    let product = &Catalog::global().all()[0];
    let carts = db.carts(&scope);
    block_on(db.batch(&carts.set_quantity_statements(&user, &product.id, 2, NOW))).unwrap();
    block_on(db.batch(&carts.set_quantity_statements(&user, &product.id, 3, NOW))).unwrap();
    let cart = block_on(carts.get(&user)).unwrap();
    assert_eq!(cart.items.len(), 1);
    assert_eq!(cart.items[0].quantity, 3);
    assert_eq!(cart.updated_at, Some(NOW));
    let stock = block_on(db.inventory(&scope).stock_map()).unwrap();
    let view = views::cart(&cart, &stock, Catalog::global());
    assert_eq!(view.total, product.price * 3_u32);
    block_on(db.batch(&carts.clear_statements(&user, NOW))).unwrap();
    assert!(block_on(carts.get(&user)).unwrap().items.is_empty());

    let builds = db.builds(&scope);
    let id = BuildId::new("bld_1");
    let save = builds
        .save_statement(&user, &id, "Mine", std::slice::from_ref(&product.id), NOW)
        .unwrap();
    block_on(db.batch(&[save])).unwrap();
    // Someone else cannot overwrite it.
    let steal = builds
        .save_statement(&UserId::new("user_2"), &id, "Theirs", &[], NOW)
        .unwrap();
    block_on(db.batch(&[steal])).unwrap();
    let row = block_on(builds.get(&user, &id)).unwrap().unwrap();
    assert_eq!(row.name, "Mine");
    assert!(
        block_on(builds.get(&UserId::new("user_2"), &id))
            .unwrap()
            .is_none()
    );
    assert!(block_on(builds.exists(&id)).unwrap());
    assert!(block_on(builds.delete(&user, &id)).unwrap());

    let addresses = db.addresses(&scope);
    let a1 = techdepo_api_types::AddressId::new("adr_1");
    let a2 = techdepo_api_types::AddressId::new("adr_2");
    for id in [&a1, &a2] {
        let save = techdepo_db::addresses::SaveAddress {
            id,
            label: "Home",
            address: &address(),
            default_shipping: true,
            default_billing: false,
        };
        block_on(db.batch(&addresses.save_statements(&user, &save, NOW).unwrap())).unwrap();
    }
    let list = block_on(addresses.list(&user)).unwrap();
    assert_eq!(list.iter().filter(|a| a.is_default_shipping).count(), 1);
    assert_eq!(
        block_on(addresses.preferred_shipping(&user))
            .unwrap()
            .unwrap()
            .id,
        a2
    );
}

#[test]
fn orders_numbers_events_and_stock() {
    let db = db();
    let scope = OrgScope::verified(OrgId::new("org_1"));
    provision(&db, &scope);
    assert_eq!(
        place(&db, &scope, "ord_1", 1).unwrap(),
        OrderNumber::new(1001)
    );
    assert_eq!(
        place(&db, &scope, "ord_2", 1).unwrap(),
        OrderNumber::new(1002)
    );

    // Oversell aborts the whole batch: no order, no number used.
    let err = place(&db, &scope, "ord_3", 9999).unwrap_err();
    assert!(inventory::is_oversell(&err), "{err}");
    assert!(
        block_on(db.orders(&scope).get(&OrderId::new("ord_3")))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        place(&db, &scope, "ord_4", 1).unwrap(),
        OrderNumber::new(1003)
    );

    let orders = db.orders(&scope);
    let id = OrderId::new("ord_1");
    let row = block_on(orders.get(&id)).unwrap().unwrap();
    assert_eq!(row.status, OrderStatus::AwaitingPayment);
    assert_eq!(row.number, OrderNumber::new(1001));
    assert_eq!(block_on(orders.items(&id)).unwrap().len(), 1);
    assert_eq!(
        block_on(orders.clock(&id)).unwrap(),
        VirtualClock::default()
    );
    assert!(block_on(orders.faults(&id)).unwrap().is_some());

    let paid = NewOrderEvent {
        seq: 2,
        kind: OrderEventKind::PaymentCaptured,
        actor: Actor::Customer,
        actor_user_id: None,
        status_after: Some(OrderStatus::Paid),
        summary: "Paid".into(),
        reference: Some((techdepo_api_types::vocab::RefKind::Payment, "pay_1".into())),
        data: serde_json::json!({ "amount": 100 }),
        invocation_id: Some("inv_1".into()),
        at: NOW,
        sim_at: NOW,
    };
    let statements = vec![
        orders.status_statement(&id, OrderStatus::Paid, NOW),
        orders.event_statement(&id, &paid).unwrap(),
        orders.event_statement(&id, &paid).unwrap(), // idempotent on seq
        orders.flags_statement(&id, Some(true), None, NOW),
        orders.set_clock_statement(&id, VirtualClock::from_offset_secs(3600), NOW),
        orders.set_clock_statement(&id, VirtualClock::from_offset_secs(60), NOW), // never back
    ];
    block_on(db.batch(&statements)).unwrap();
    let row = block_on(orders.get(&id)).unwrap().unwrap();
    assert_eq!(row.status, OrderStatus::Paid);
    assert_eq!(row.paid_at, Some(NOW));
    assert!(row.refund_pending);
    assert_eq!(block_on(orders.clock(&id)).unwrap().offset_secs(), 3600);
    let events = block_on(orders.events(&id)).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].data["amount"], 100);
    assert_eq!(block_on(orders.recent_events(10)).unwrap().len(), 4);

    let (page, next) = block_on(orders.list(
        &OrderFilter::default(),
        &PageRequest::from_query(None, Some(2)),
    ))
    .unwrap();
    assert_eq!(page.len(), 2);
    let (rest, _) = block_on(orders.list(
        &OrderFilter::default(),
        &PageRequest::from_query(next.as_deref(), Some(2)),
    ))
    .unwrap();
    assert_eq!(rest.len(), 1);
    let filtered = OrderFilter {
        flagged: true,
        ..OrderFilter::default()
    };
    assert_eq!(
        block_on(orders.list(&filtered, &PageRequest::first()))
            .unwrap()
            .0
            .len(),
        1
    );
    let by_number = OrderFilter {
        number: Some(OrderNumber::new(1002)),
        ..OrderFilter::default()
    };
    assert_eq!(
        block_on(orders.list(&by_number, &PageRequest::first()))
            .unwrap()
            .0[0]
            .id
            .as_str(),
        "ord_2"
    );
    let headlines = block_on(orders.headlines(std::slice::from_ref(&id))).unwrap();
    assert_eq!(headlines[0].2, 1);
    assert_eq!(block_on(orders.count_by_status()).unwrap().len(), 2);
    assert_eq!(block_on(orders.created_since(NOW)).unwrap(), 3);
    assert_eq!(
        block_on(orders.created_since(NOW + time::Duration::seconds(1))).unwrap(),
        0
    );
    assert!(!block_on(orders.reserved_units()).unwrap().is_empty());
}

#[test]
fn payments_invoices_shipments() {
    let db = db();
    let scope = OrgScope::verified(OrgId::new("org_1"));
    provision(&db, &scope);
    place(&db, &scope, "ord_1", 1).unwrap();
    let order = OrderId::new("ord_1");

    // Payments.
    let payments = db.payments(&scope);
    let charge = |id: &str, provider: &str| NewPayment {
        id: PaymentId::new(id),
        order_id: order.clone(),
        kind: PaymentKind::Charge,
        status: PaymentStatus::Succeeded,
        amount: Money::from_euros(100),
        method: PaymentMethod::Card,
        provider_ref: provider.into(),
        refund_of: None,
        refund_id: None,
        failure_reason: None,
        created_at: NOW,
        sim_at: NOW,
    };
    let refund = NewPayment {
        kind: PaymentKind::Refund,
        refund_of: Some(PaymentId::new("pay_2")),
        provider_ref: "re_1".into(),
        ..charge("pay_3", "")
    };
    block_on(db.batch(&[
        payments.insert_statement(&charge("pay_1", "ch_1")),
        payments.insert_statement(&charge("pay_2", "ch_2")),
        payments.insert_statement(&charge("pay_2", "ch_2")),
        payments.insert_statement(&refund),
    ]))
    .unwrap();
    assert_eq!(block_on(payments.for_order(&order)).unwrap().len(), 3);
    assert_eq!(
        block_on(payments.net_captured(&order)).unwrap(),
        Money::from_euros(100)
    );
    assert_eq!(
        block_on(payments.revenue()).unwrap(),
        Money::from_euros(100)
    );
    let (ledger, _) = block_on(payments.list(&Default::default(), &PageRequest::first())).unwrap();
    assert_eq!(ledger.len(), 3);

    // A refund is written only while the charge has enough left (pay_2 has €0 left, pay_1 €100).
    let refund_of = |id: &str, of: &str, euros: i64| NewPayment {
        kind: PaymentKind::Refund,
        refund_of: Some(PaymentId::new(of)),
        provider_ref: format!("re_{id}"),
        amount: Money::from_euros(euros),
        ..charge(id, "")
    };
    let results = block_on(db.batch(&[
        payments.refund_insert_statement(&refund_of("pay_4", "pay_2", 1)),
        payments.refund_insert_statement(&refund_of("pay_5", "pay_1", 60)),
        payments.refund_insert_statement(&refund_of("pay_6", "pay_1", 60)),
    ]))
    .unwrap();
    assert_eq!(
        [0, 1, 2].map(|i| techdepo_db::rows_affected(&results, i)),
        [0, 1, 0]
    );
    assert_eq!(
        block_on(payments.net_captured(&order)).unwrap(),
        Money::from_euros(40)
    );

    // Invoices: numbering, the number in the HTML, void + credit note + reissue in one batch.
    let invoices = db.invoices(&scope);
    let doc = |id: &str, kind: InvoiceKind, sign: i64| NewInvoice {
        id: InvoiceId::new(id),
        order_id: order.clone(),
        kind,
        supersedes_id: None,
        credit_for_id: None,
        billing_name: "Ada".into(),
        billing_address: None,
        vat_rate: VatRate::DEFAULT,
        issue_date: date!(2026 - 10 - 16),
        issued_at: NOW,
        lines: vec![NewInvoiceLine {
            product_id: None,
            sku: None,
            description: "Thing".into(),
            quantity: 1,
            unit_price: Money::from_euros(127 * sign),
        }],
        html: format!("<h1>{NUMBER_PLACEHOLDER}</h1>"),
    };
    let first = doc("inv_1", InvoiceKind::Invoice, 1);
    let results = block_on(db.batch(&invoices.issue_statements(&first).unwrap())).unwrap();
    assert_eq!(
        issued_number(&results, 0, 2026),
        Some(InvoiceNumber::new(2026, 1))
    );
    let row = block_on(invoices.get(&first.id)).unwrap().unwrap();
    assert_eq!(row.number.to_string(), "TD-2026-000001");
    assert_eq!(row.net, Money::from_euros(100));
    assert!(views::invoice_summary(&row).missing_billing_address);
    assert_eq!(
        block_on(invoices.html(&first.id)).unwrap().as_deref(),
        Some("<h1>TD-2026-000001</h1>")
    );

    let credit = NewInvoice {
        credit_for_id: Some(first.id.clone()),
        ..doc("inv_2", InvoiceKind::CreditNote, -1)
    };
    let reissue = NewInvoice {
        supersedes_id: Some(first.id.clone()),
        billing_address: Some(address()),
        ..doc("inv_3", InvoiceKind::Invoice, 1)
    };
    let mut batch = vec![invoices.void_statement(&first.id, "Missing address", NOW)];
    let credit_at = batch.len();
    batch.extend(invoices.issue_statements(&credit).unwrap());
    let reissue_at = batch.len();
    batch.extend(invoices.issue_statements(&reissue).unwrap());
    let results = block_on(db.batch(&batch)).unwrap();
    assert_eq!(
        issued_number(&results, credit_at, 2026),
        Some(InvoiceNumber::new(2026, 2))
    );
    assert_eq!(
        issued_number(&results, reissue_at, 2026),
        Some(InvoiceNumber::new(2026, 3))
    );
    let voided = block_on(invoices.get(&first.id)).unwrap().unwrap();
    assert_eq!(voided.status, InvoiceStatus::Voided);
    assert_eq!(voided.superseded_by_id, Some(reissue.id.clone()));
    let credit_row = block_on(invoices.get(&credit.id)).unwrap().unwrap();
    assert_eq!(credit_row.gross, Money::from_euros(-127));
    assert_eq!(
        credit_row.credit_for_number,
        Some(InvoiceNumber::new(2026, 1))
    );
    assert_eq!(
        block_on(invoices.lines(&credit.id)).unwrap()[0].vat,
        Money::from_euros(-27)
    );
    let current = block_on(invoices.current_for_order(&order))
        .unwrap()
        .unwrap();
    assert_eq!(current.id, reissue.id);
    assert_eq!(block_on(invoices.for_order(&order)).unwrap().len(), 3);

    // A second void-and-reissue of the same invoice (a racing call that read it before the
    // first committed) fails as a whole: no second credit note, no second replacement.
    let credit2 = NewInvoice {
        credit_for_id: Some(first.id.clone()),
        ..doc("inv_4", InvoiceKind::CreditNote, -1)
    };
    let reissue2 = NewInvoice {
        supersedes_id: Some(first.id.clone()),
        ..doc("inv_5", InvoiceKind::Invoice, 1)
    };
    let mut batch = vec![invoices.void_statement(&first.id, "Again", NOW)];
    batch.extend(invoices.issue_statements(&credit2).unwrap());
    batch.extend(invoices.issue_statements(&reissue2).unwrap());
    assert!(matches!(
        block_on(db.batch(&batch)),
        Err(techdepo_db::SqlError::Constraint {
            kind: techdepo_ports::ConstraintKind::Unique,
            ..
        })
    ));
    assert_eq!(block_on(invoices.for_order(&order)).unwrap().len(), 3);

    // Shipments.
    let shipments = db.shipments(&scope);
    let shipment = ShipmentId::new("shp_1");
    let event = |kind: CarrierEventKind, internal: bool| NewCarrierEvent {
        kind,
        internal,
        description: kind.label().into(),
        location: None,
        at: NOW,
        sim_at: NOW,
    };
    let new = NewShipment {
        id: shipment.clone(),
        order_id: order.clone(),
        carrier: "Kestrel Post".into(),
        tracking_number: TrackingNumber::new("KP00000001"),
        lost_outcome: Some(LostOutcome::Resurfaces),
        shipped_on: date!(2026 - 10 - 16),
        shipped_sim_at: NOW,
        eta: date!(2026 - 10 - 19),
        created_at: NOW,
        events: vec![
            event(CarrierEventKind::LabelCreated, false),
            event(CarrierEventKind::PickedUp, false),
        ],
    };
    block_on(db.batch(&shipments.create_statements(&new))).unwrap();
    block_on(db.batch(&shipments.create_statements(&new))).unwrap();
    let once =
        shipments.event_once_statement(&shipment, &event(CarrierEventKind::TraceOpened, true));
    block_on(db.batch(&[once.clone(), once])).unwrap();
    block_on(db.batch(&[shipments.trace_statement(
        &shipment,
        TraceStatus::Resurfaced,
        Some(date!(2026 - 10 - 20)),
        NOW,
    )]))
    .unwrap();
    block_on(db.batch(&shipments.move_ship_date_statements(
        &shipment,
        date!(2026 - 10 - 09),
        -7 * 86_400_000,
        date!(2026 - 10 - 12),
        NOW,
    )))
    .unwrap();
    let row = block_on(shipments.for_order(&order)).unwrap().unwrap();
    assert_eq!(row.shipped_on, date!(2026 - 10 - 09));
    assert_eq!(row.trace_status, Some(TraceStatus::Resurfaced));
    assert!(!row.is_missing());
    assert_eq!(
        block_on(shipments.events(&shipment, true)).unwrap().len(),
        3
    );
    assert_eq!(
        block_on(shipments.events(&shipment, false)).unwrap().len(),
        2
    );
    block_on(db.batch(&[shipments.deliver_statement(&shipment, date!(2026 - 10 - 13), NOW, NOW)]))
        .unwrap();
    let row = block_on(shipments.by_tracking(&TrackingNumber::new("KP00000001")))
        .unwrap()
        .unwrap();
    assert_eq!(row.delivered_on, Some(date!(2026 - 10 - 13)));

    // Demo reset wipes it all but keeps the sequences.
    let (orders, ..) = block_on(db.orgs(&scope).demo_data_counts()).unwrap();
    assert_eq!(orders, 1);
    block_on(db.batch(&db.orgs(&scope).reset_demo_statements())).unwrap();
    assert!(block_on(db.orders(&scope).get(&order)).unwrap().is_none());
    assert_eq!(
        place(&db, &scope, "ord_9", 1).unwrap(),
        OrderNumber::new(1002)
    );
    expire_and_check(&db, &scope);
}

#[test]
fn read_side_of_the_support_tables() {
    let db = db();
    let scope = OrgScope::verified(OrgId::new("org_1"));
    provision(&db, &scope);
    place(&db, &scope, "ord_1", 1).unwrap();
    let user = UserId::new("user_1");
    let (orders, spent) = block_on(db.orders(&scope).customer_stats(&user)).unwrap();
    assert_eq!((orders, spent), (1, Money::ZERO));
    assert_eq!(block_on(db.investigations(&scope).open_count()).unwrap(), 0);
    assert!(
        block_on(
            db.investigations(&scope)
                .list(None, Some(true), &PageRequest::first())
        )
        .unwrap()
        .0
        .is_empty()
    );
    assert!(
        block_on(
            db.tickets(&scope)
                .list(&Default::default(), &PageRequest::first())
        )
        .unwrap()
        .0
        .is_empty()
    );
    assert_eq!(
        block_on(db.tickets(&scope).open_count(Some(&user), None)).unwrap(),
        0
    );
    assert_eq!(
        block_on(db.tasks(&scope).pending_counts(NOW)).unwrap(),
        (0, 0)
    );
    let (tasks, _) = block_on(db.tasks(&scope).list(
        techdepo_api_types::vocab::HumanTaskStatus::Pending,
        None,
        &PageRequest::first(),
    ))
    .unwrap();
    assert!(tasks.is_empty());
    assert_eq!(
        block_on(db.refunds(&scope).totals()).unwrap(),
        (Money::ZERO, 0)
    );
    assert!(
        block_on(
            db.email_log(&scope)
                .list(None, None, None, &PageRequest::first())
        )
        .unwrap()
        .0
        .is_empty()
    );
    let (members, _) = block_on(db.orgs(&scope).members(None, &PageRequest::first())).unwrap();
    assert!(members.is_empty());
}

#[test]
fn agent_traces_and_ticket_refunds() {
    use techdepo_api_types::vocab::{AgentKind, AgentStepKind};
    use techdepo_domain::TicketId;
    use techdepo_ports::Param;

    let db = db();
    let scope = OrgScope::verified(OrgId::new("org_1"));
    provision(&db, &scope);
    let ticket = TicketId::parse("T-7F2M9Q").unwrap();
    let ms = NOW.unix_timestamp() * 1000;
    let insert = |sql: &str, params: Vec<Param>| {
        block_on(db.sql().execute(sql, &params)).unwrap();
    };
    insert(
        "INSERT INTO tickets (org_id, id, user_id, customer_email, subject, channel, status, created_at, updated_at) \
         VALUES ('org_1', ?1, 'user_1', 'a@b.c', 'Hi', 'web', 'new', ?2, ?2)",
        vec![ticket.to_string().into(), ms.into()],
    );
    insert(
        "INSERT INTO agent_runs (org_id, id, ticket_id, agent, model, status, outcome, outcome_json, step_count, \
         guardrails_json, started_at, finished_at) VALUES ('org_1', 'run_1', ?1, 'router', 'mock', 'completed', \
         'reply', '{\"kind\":\"reply\",\"body\":\"Hello\"}', 1, '[]', ?2, ?2)",
        vec![ticket.to_string().into(), ms.into()],
    );
    insert(
        "INSERT INTO agent_steps (org_id, run_id, idx, kind, name, input_json, output_json, started_at) \
         VALUES ('org_1', 'run_1', 0, 'classification', 'clef', '{\"text\":\"hi\"}', '{\"intent\":\"other\"}', ?1)",
        vec![ms.into()],
    );
    let runs = block_on(db.agent_runs(&scope).for_ticket(&ticket)).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].agent, AgentKind::Router);
    let steps = block_on(db.agent_runs(&scope).steps(&runs[0].id)).unwrap();
    let detail = runs[0].detail(&steps);
    assert_eq!(detail.steps[0].kind, AgentStepKind::Classification);
    assert!(detail.outcome.is_some());
    assert!(
        block_on(db.refunds(&scope).for_ticket(&ticket))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_ticket_object_writes_its_read_model() {
    use techdepo_api_types::AgentRunId;
    use techdepo_api_types::support::{Classification, IntentProbability};
    use techdepo_api_types::vocab::{
        AgentKind, AgentRunStatus, AgentStepKind, Awaiting, Direction, MessageStatus, TicketRoute,
    };
    use techdepo_db::agent_runs::{NewAgentRun, NewAgentStep};
    use techdepo_db::tickets::{NewTicketMessage, TicketRecord};
    use techdepo_domain::{Assignee, Channel, Intent, TicketId, TicketStatus};

    let db = db();
    let scope = OrgScope::verified(OrgId::new("org_1"));
    provision(&db, &scope);
    let number = place(&db, &scope, "ord_1", 1).unwrap();
    let tickets = db.tickets(&scope);
    let ticket = TicketId::parse("T-7F2M9Q").unwrap();
    let mut record = TicketRecord {
        id: ticket.clone(),
        user_id: UserId::new("user_1"),
        customer_email: "Ada@Example.com".into(),
        subject: "Cancel my order".into(),
        channel: Channel::Email,
        status: TicketStatus::Routing,
        assignee: Assignee::Ai,
        order_id: None,
        order_number: None,
        intent: None,
        intent_confidence: None,
        classification: None,
        route: None,
        awaiting: None,
        escalation_reason: None,
        reply_confidence: None,
        message_count: 1,
        last_message_at: Some(NOW),
        created_at: NOW,
        updated_at: NOW,
        resolved_at: None,
    };
    let inbound = NewTicketMessage {
        seq: 1,
        direction: Direction::Inbound,
        author: Actor::Customer,
        author_user_id: Some(UserId::new("user_1")),
        channel: Channel::Email,
        status: MessageStatus::Received,
        subject: "Cancel my order".into(),
        body_text: "Please cancel".into(),
        body_html: None,
        message_id: Some("m1@x".into()),
        email_log_id: Some("eml_1".into()),
        agent_run_id: None,
        created_at: NOW,
        sent_at: None,
    };
    let statements = [
        tickets.save_statement(&record).unwrap(),
        tickets.insert_message_statement(&ticket, &inbound),
        tickets.insert_message_statement(&ticket, &inbound), // a retried step
    ];
    block_on(db.batch(&statements)).unwrap();

    // Classified and waiting for the order number, with a draft held for review.
    record.status = TicketStatus::AwaitingCustomer;
    record.awaiting = Some(Awaiting::OrderNumber);
    record.intent = Some(Intent::CancelOrder);
    record.intent_confidence = Some(0.91);
    record.route = Some(TicketRoute::CancelOrder);
    record.order_id = Some(OrderId::new("ord_1"));
    record.order_number = Some(number);
    record.classification = Some(Classification {
        model: "mock-clef".into(),
        intent: Intent::CancelOrder,
        probabilities: vec![IntentProbability {
            intent: Intent::CancelOrder,
            p: 0.91,
        }],
        contains_request: 0.95,
        wants_human: 0.05,
        legal_threat: 0.02,
        answers_our_question: None,
        order_numbers: vec![number],
        threshold: 0.7,
        margin: 0.15,
        escalation: Vec::new(),
        agent_run_id: Some(AgentRunId::new("run_1")),
        classified_at: NOW,
    });
    record.message_count = 2;
    let draft = NewTicketMessage {
        seq: 2,
        direction: Direction::Outbound,
        author: Actor::Ai,
        author_user_id: None,
        status: MessageStatus::Draft,
        body_text: "Draft".into(),
        message_id: None,
        email_log_id: None,
        ..inbound.clone()
    };
    let runs = db.agent_runs(&scope);
    let run = NewAgentRun {
        id: AgentRunId::new("run_1"),
        ticket_id: Some(ticket.clone()),
        investigation_id: None,
        agent: AgentKind::Router,
        model: "mock-clef".into(),
        status: AgentRunStatus::Completed,
        outcome: None,
        system_prompt: None,
        input_tokens: 40,
        output_tokens: 4,
        step_count: 1,
        attempt: 1,
        confidence: None,
        guardrails: None,
        error: None,
        started_at: NOW,
        finished_at: Some(NOW),
    };
    let step = NewAgentStep {
        idx: 0,
        kind: AgentStepKind::Classification,
        name: "mock-clef".into(),
        journal_name: Some("classify".into()),
        input: Some(serde_json::json!({ "text": "Please cancel" })),
        output: Some(serde_json::json!({ "intent": "cancel_order" })),
        is_error: false,
        input_tokens: 40,
        output_tokens: 4,
        started_at: NOW,
        duration_ms: 12,
    };
    block_on(db.batch(&[
        tickets.save_statement(&record).unwrap(),
        tickets.insert_message_statement(&ticket, &draft),
        runs.insert_run_statement(&run).unwrap(),
        runs.insert_step_statement(&run.id, &step).unwrap(),
        runs.insert_run_statement(&run).unwrap(),
    ]))
    .unwrap();
    let row = block_on(tickets.get(&ticket)).unwrap().unwrap();
    assert_eq!(row.status, TicketStatus::AwaitingCustomer);
    assert_eq!(row.customer_email, "ada@example.com");
    assert_eq!(row.linked_order_number, Some(number));
    assert_eq!(row.classification.unwrap().order_numbers, vec![number]);
    assert_eq!(block_on(tickets.visible_message_count(&ticket)).unwrap(), 1);
    assert_eq!(
        block_on(tickets.find_by_message_ids(&["m1@x".into()])).unwrap(),
        Some(ticket.clone())
    );
    let traces = block_on(runs.for_ticket(&ticket)).unwrap();
    assert_eq!(traces[0].input_tokens, 40);
    assert_eq!(block_on(runs.steps(&run.id)).unwrap()[0].duration_ms, 12);

    // The reviewer edits and sends the draft; a second settle changes nothing.
    let settle = tickets.settle_draft_statement(
        &ticket,
        2,
        MessageStatus::Sent,
        Actor::Human,
        Some(&UserId::new("user_admin")),
        Some(("Edited", Some("<p>Edited</p>"))),
        Some("T-7F2M9Q.2@techdepo.dev"),
        Some(NOW),
    );
    let results = block_on(db.batch(&[settle.clone(), settle])).unwrap();
    assert_eq!(techdepo_db::rows_affected(&results, 0), 1);
    assert_eq!(techdepo_db::rows_affected(&results, 1), 0);
    let messages = block_on(tickets.messages(&ticket, true)).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].body_text, "Edited");
    assert_eq!(messages[1].author, Actor::Human);
    assert_eq!(
        messages[1].message_id.as_deref(),
        Some("T-7F2M9Q.2@techdepo.dev")
    );

    // The awaiting invariant is the schema's too.
    record.awaiting = None;
    assert!(block_on(db.batch(&[tickets.save_statement(&record).unwrap()])).is_err());

    // An escalation task is refreshed when the customer writes again.
    let tasks = db.tasks(&scope);
    let task = techdepo_db::tasks::NewTask {
        id: techdepo_domain::HumanTaskId::new("tsk_1"),
        kind: techdepo_api_types::vocab::HumanTaskKind::Escalation,
        title: "Ticket T-7F2M9Q: Legal threat".into(),
        summary: "Cancel my order".into(),
        context: techdepo_api_types::tasks::TaskContext::Escalation {
            reason: "Legal threat".into(),
            causes: Vec::new(),
            classification: None,
            agent_run_id: None,
            new_customer_message: false,
        },
        evidence: Vec::new(),
        awakeable_id: None,
        sla_due_at: NOW,
        ticket_id: Some(ticket.clone()),
        order_id: None,
        refund_id: None,
        investigation_id: None,
        created_at: NOW,
    };
    let refresh = tasks.refresh_statement(&task.id, "New customer message: hello?", NOW);
    let results = block_on(db.batch(&[tasks.insert_statement(&task).unwrap(), refresh])).unwrap();
    assert_eq!(techdepo_db::rows_affected(&results, 1), 1);
    let row = block_on(tasks.get(&task.id)).unwrap().unwrap();
    assert_eq!(row.summary, "New customer message: hello?");
    assert!(matches!(
        row.context,
        techdepo_api_types::tasks::TaskContext::Escalation {
            new_customer_message: true,
            ..
        }
    ));
    expire_and_check(&db, &scope);
}

#[test]
fn the_investigation_workflow_writes_its_record() {
    use techdepo_api_types::vocab::{
        InvestigationDecision, InvestigationOutcome, InvestigationStatus,
    };
    use techdepo_db::investigations::{InvestigationUpdate, NewInvestigation};
    use techdepo_domain::InvestigationId;

    let db = db();
    let scope = OrgScope::verified(OrgId::new("org_1"));
    provision(&db, &scope);
    place(&db, &scope, "ord_1", 1).unwrap();
    let investigations = db.investigations(&scope);
    let id = InvestigationId::new("inq_1");
    let new = NewInvestigation {
        id: id.clone(),
        order_id: OrderId::new("ord_1"),
        ticket_id: None,
        shipment_id: None,
        opened_by: Actor::System,
        opened_by_user_id: None,
        note: None,
        created_at: NOW,
    };
    let insert = investigations.insert_statement(&new);
    block_on(db.batch(&[insert.clone(), insert])).unwrap();
    let row = block_on(investigations.get(&id)).unwrap().unwrap();
    assert_eq!(row.status, InvestigationStatus::Open);
    assert_eq!(block_on(investigations.open_count()).unwrap(), 1);

    let later = NOW + time::Duration::minutes(5);
    block_on(db.batch(&[investigations.update_statement(
        &id,
        &InvestigationUpdate {
            status: Some(InvestigationStatus::AwaitingCustomer),
            carrier_result: Some(LostOutcome::Gone),
            decision: Some(InvestigationDecision::OfferBoth),
            rounds: Some(1),
            ..InvestigationUpdate::default()
        },
        later,
    )]))
    .unwrap();
    let row = block_on(investigations.get(&id)).unwrap().unwrap();
    assert_eq!(row.carrier_result, Some(LostOutcome::Gone));
    assert_eq!(row.decision, Some(InvestigationDecision::OfferBoth));
    assert_eq!((row.rounds, row.closed_at), (1, None));

    let replacement = place(&db, &scope, "ord_2", 1).unwrap();
    assert_eq!(replacement, OrderNumber::new(1002));
    let close = |at| {
        investigations.update_statement(
            &id,
            &InvestigationUpdate {
                status: Some(InvestigationStatus::Resolved),
                outcome: Some(InvestigationOutcome::Reshipped),
                replacement_order_id: Some(OrderId::new("ord_2")),
                ..InvestigationUpdate::default()
            },
            at,
        )
    };
    block_on(db.batch(&[close(later)])).unwrap();
    block_on(db.batch(&[close(later + time::Duration::minutes(1))])).unwrap();
    let row = block_on(investigations.get(&id)).unwrap().unwrap();
    assert_eq!(row.outcome, Some(InvestigationOutcome::Reshipped));
    assert_eq!(row.replacement_order_id, Some(OrderId::new("ord_2")));
    assert_eq!(row.closed_at, Some(later), "closed once");
    assert_eq!(row.rounds, 1, "kept");
    assert_eq!(block_on(investigations.open_count()).unwrap(), 0);
    expire_and_check(&db, &scope);
}
