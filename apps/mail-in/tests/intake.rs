//! The intake pipeline against SQLite with the real migrations and the dev directory.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use futures::executor::block_on;
use techdepo_api_types::vocab::{Direction, EmailStatus, MemberSource, OrgRole, RejectReason};
use techdepo_db::email_log::NewEmail;
use techdepo_db::orgs::MemberUpsert;
use techdepo_db::{Db, OrgScope, PageRequest, SqliteExecutor};
use techdepo_domain::{OrgId, OrgSettings, PublicOrgId, TicketId, UserId};
use techdepo_mail_in::directory::D1Directory;
use techdepo_mail_in::intake::{Envelope, Intake, IntakeConfig, Outcome};
use time::OffsetDateTime;
use time::macros::datetime;

const NOW: OffsetDateTime = datetime!(2026-10-14 10:00 UTC);
const TO: &str = "support+k3x9q2ab@techdepo.dev";

fn setup() -> (Db, OrgScope) {
    let db = Db::new(Arc::new(SqliteExecutor::in_memory().expect("migrations")));
    let scope = OrgScope::verified(OrgId::new("org_dev_1"));
    let orgs = db.orgs(&scope);
    block_on(
        db.batch(&[orgs
            .insert_statement(
                &PublicOrgId::parse("k3x9q2ab").unwrap(),
                "Demo",
                &OrgSettings::default(),
                "v1",
                NOW,
                None,
            )
            .unwrap()]),
    )
    .unwrap();
    for (user, email) in [
        ("user_ada", "ada@example.com"),
        ("user_bob", "bob@example.com"),
    ] {
        block_on(orgs.upsert_member(
            &MemberUpsert {
                user_id: UserId::new(user),
                email: email.into(),
                name: None,
                role: OrgRole::Member,
                source: MemberSource::Dev,
            },
            NOW,
        ))
        .unwrap();
    }
    (db, scope)
}

/// A message as our MTA hands it over: its `Authentication-Results` (DMARC passed for the From
/// domain) on top, unless `extra` brings its own.
fn eml(from: &str, message_id: &str, extra: &str, body: &str) -> Vec<u8> {
    let auth = if extra.contains("Authentication-Results") {
        String::new()
    } else {
        let domain = from.rsplit('@').next().unwrap_or("").to_ascii_lowercase();
        format!("Authentication-Results: mx.cloudflare.net; dmarc=pass header.from={domain}\r\n")
    };
    format!(
        "{auth}From: Ada Lovelace <{from}>\r\nTo: {TO}\r\nSubject: Cancel my order\r\nMessage-ID: <{message_id}>\r\n{extra}\
Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

fn envelope(to: &str, raw: Vec<u8>) -> Envelope {
    Envelope {
        from: "bounce@example.com".into(),
        to: to.into(),
        raw_size: raw.len(),
        raw,
        received_at: NOW,
    }
}

fn run(db: &Db, env: &Envelope) -> Outcome {
    let directory = D1Directory::new(db.clone());
    let config = IntakeConfig::default();
    block_on(Intake::new(db, &directory, &config).process(env)).unwrap()
}

fn rejected(outcome: &Outcome) -> RejectReason {
    match outcome {
        Outcome::Rejected { reason, .. } => *reason,
        Outcome::Accepted { .. } => panic!("expected a rejection, got {outcome:?}"),
    }
}

fn accepted(outcome: Outcome) -> (String, techdepo_api_types::ingress::InboundMessage) {
    match outcome {
        Outcome::Accepted { key, message } => (key, *message),
        Outcome::Rejected { reason, detail } => panic!("rejected: {reason:?} {detail}"),
    }
}

#[test]
fn a_member_opens_a_ticket() {
    let (db, scope) = setup();
    let raw = eml(
        "Ada@Example.com",
        "m1@mail.example.com",
        "",
        "Please cancel TD-1042.\r\n\r\n-- \r\nAda",
    );
    let (key, message) = accepted(run(&db, &envelope(TO, raw.clone())));
    let ticket = TicketId::derive(
        &PublicOrgId::parse("k3x9q2ab").unwrap(),
        "m1@mail.example.com",
    );
    assert_eq!(key, format!("org_dev_1:{ticket}"));
    assert_eq!(message.ticket_id, ticket);
    assert_eq!(message.user_id.as_str(), "user_ada");
    assert_eq!(message.customer_email, "ada@example.com");
    assert_eq!(message.customer_name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(message.body_text, "Please cancel TD-1042.");
    let log_id = message.email_log_id.clone().unwrap();
    let row = block_on(db.email_log(&scope).get(&log_id))
        .unwrap()
        .unwrap();
    assert_eq!(row.status, EmailStatus::Accepted);
    assert_eq!(row.ticket_id.as_ref(), Some(&ticket));
    assert!(
        row.text_body.unwrap().contains("-- \nAda"),
        "the log keeps the full text"
    );

    // A redelivery reuses the log entry and the ticket.
    let (_, again) = accepted(run(&db, &envelope(TO, raw)));
    assert_eq!(again.ticket_id, ticket);
    assert_eq!(again.email_log_id, Some(log_id));
    let (rows, _) = block_on(db.email_log(&scope).list(
        Some(Direction::Inbound),
        None,
        None,
        &PageRequest::first(),
    ))
    .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn strangers_unknown_orgs_and_robots_are_dropped_and_logged() {
    let (db, scope) = setup();
    let stranger = run(
        &db,
        &envelope(TO, eml("eve@example.com", "e1@x", "", "Refund TD-1 now")),
    );
    assert_eq!(rejected(&stranger), RejectReason::NotMember);
    let unknown = run(
        &db,
        &envelope(
            "support+zzzzzzzz@techdepo.dev",
            eml("ada@example.com", "u1@x", "", "Hi"),
        ),
    );
    assert_eq!(rejected(&unknown), RejectReason::UnknownOrg);
    let robot = run(
        &db,
        &envelope(
            TO,
            eml(
                "ada@example.com",
                "r1@x",
                "Auto-Submitted: auto-replied\r\n",
                "I am away",
            ),
        ),
    );
    assert_eq!(rejected(&robot), RejectReason::AutoReply);
    let ours = run(
        &db,
        &envelope(TO, eml("support+k3x9q2ab@techdepo.dev", "o1@x", "", "loop")),
    );
    assert_eq!(rejected(&ours), RejectReason::OwnDomain);
    let spoofed = run(
        &db,
        &envelope(
            TO,
            eml(
                "ada@example.com",
                "d1@x",
                "Authentication-Results: mx.cloudflare.net; dmarc=fail\r\n",
                "Refund",
            ),
        ),
    );
    assert_eq!(rejected(&spoofed), RejectReason::DmarcFail);
    // No result from our MTA (a header the sender wrote does not count): fail closed.
    let unauthenticated = run(
        &db,
        &envelope(
            TO,
            eml(
                "ada@example.com",
                "d2@x",
                "Authentication-Results: x; dmarc=pass header.from=example.com\r\n",
                "Cancel TD-1042",
            ),
        ),
    );
    assert_eq!(rejected(&unauthenticated), RejectReason::DmarcFail);
    let big = Envelope {
        raw_size: 10 * 1024 * 1024,
        raw: Vec::new(),
        ..envelope(TO, Vec::new())
    };
    assert_eq!(rejected(&run(&db, &big)), RejectReason::Oversized);

    // The org's log shows every rejection with its reason (the unknown org's is unscoped).
    let (rows, _) = block_on(db.email_log(&scope).list(
        Some(Direction::Inbound),
        Some(EmailStatus::Rejected),
        None,
        &PageRequest::first(),
    ))
    .unwrap();
    let mut reasons: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.reject_reason.map(|r| r.as_str()))
        .collect();
    reasons.sort_unstable();
    assert_eq!(
        reasons,
        vec![
            "auto_reply",
            "dmarc_fail",
            "dmarc_fail",
            "not_member",
            "oversized",
            "own_domain"
        ]
    );
}

#[test]
fn replies_find_their_ticket() {
    let (db, scope) = setup();
    let (_, first) = accepted(run(
        &db,
        &envelope(TO, eml("ada@example.com", "m1@x", "", "Cancel please")),
    ));
    let ticket = first.ticket_id.clone();
    // Our reply went out as `<ticket>.2@techdepo.dev` (logged by `Email/send`).
    let outbound_id = format!("{ticket}.2@techdepo.dev");
    block_on(
        db.batch(&[db
            .email_log(&scope)
            .insert_statement(&NewEmail {
                id: "eml_out_1".into(),
                direction: Some(Direction::Outbound),
                status: Some(EmailStatus::Sent),
                message_id: Some(outbound_id.clone()),
                from_address: TO.into(),
                to_address: "ada@example.com".into(),
                subject: "Re: Cancel my order".into(),
                ticket_id: Some(ticket.clone()),
                created_at_ms: NOW.unix_timestamp() * 1000,
                ..NewEmail::default()
            })
            .unwrap()]),
    )
    .unwrap();

    // 1. Threading headers, sent to the plain support address.
    let reply = eml(
        "ada@example.com",
        "m2@x",
        &format!("In-Reply-To: <{outbound_id}>\r\nReferences: <m1@x> <{outbound_id}>\r\n"),
        "TD-1042\r\n\r\nOn Mon, TechDepó Support wrote:\r\n> Which order?",
    );
    let (_, second) = accepted(run(&db, &envelope(TO, reply)));
    assert_eq!(second.ticket_id, ticket);
    assert_eq!(second.body_text, "TD-1042");
    assert_eq!(second.in_reply_to.as_deref(), Some(outbound_id.as_str()));

    // 2. The ticket's own address.
    let tagged = format!(
        "support+k3x9q2ab.{}@techdepo.dev",
        ticket.code().to_lowercase()
    );
    let (_, third) = accepted(run(
        &db,
        &envelope(&tagged, eml("ada@example.com", "m3@x", "", "Any news?")),
    ));
    assert_eq!(third.ticket_id, ticket);

    // 3. Another member replying into Ada's thread gets a ticket of their own once Ada's
    //    ticket is in the read model.
    block_on(
        db.batch(&[db
            .tickets(&scope)
            .save_statement(&techdepo_db::tickets::TicketRecord {
                id: ticket.clone(),
                user_id: UserId::new("user_ada"),
                customer_email: "ada@example.com".into(),
                subject: "Cancel my order".into(),
                channel: techdepo_domain::Channel::Email,
                status: techdepo_domain::TicketStatus::Routing,
                assignee: techdepo_domain::Assignee::Ai,
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
            })
            .unwrap()]),
    )
    .unwrap();
    let (_, bobs) = accepted(run(
        &db,
        &envelope(&tagged, eml("bob@example.com", "b1@x", "", "Me too")),
    ));
    assert_ne!(bobs.ticket_id, ticket);
    assert_eq!(bobs.user_id.as_str(), "user_bob");
}

#[test]
fn the_hourly_limit_applies_per_org() {
    let (db, _) = setup();
    let directory = D1Directory::new(db.clone());
    let config = IntakeConfig {
        max_per_org_per_hour: 2,
        ..IntakeConfig::default()
    };
    let intake = Intake::new(&db, &directory, &config);
    for i in 0..2 {
        let raw = eml("ada@example.com", &format!("n{i}@x"), "", "Hello?");
        assert!(matches!(
            block_on(intake.process(&envelope(TO, raw))).unwrap(),
            Outcome::Accepted { .. }
        ));
    }
    let raw = eml("ada@example.com", "n3@x", "", "Hello?");
    assert_eq!(
        rejected(&block_on(intake.process(&envelope(TO, raw))).unwrap()),
        RejectReason::RateLimited
    );
}
