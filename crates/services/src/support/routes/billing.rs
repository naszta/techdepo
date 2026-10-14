//! `BillingIssue` (support.md §4): deterministic detectors over the order's ledger and invoices.
//!
//! - **Duplicate charge** (`admin::find_duplicate_charges`: same order, same amount, within 15
//!   minutes, not refunded yet): start `RefundWorkflow` for the duplicate with the evidence
//!   attached. It still needs a person's approval; the ticket waits on it.
//! - **Invoice without billing address**: `Invoicing/void_and_reissue`, automatically, because no
//!   money moves. The corrected invoice goes out as its own email (attached), the reply names it.
//! - **Neither**: escalate. The customer is not told something is wrong when the data says it
//!   is not.

use std::sync::Arc;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::admin::find_duplicate_charges;
use techdepo_api_types::ingress::{StartRefundInput, VoidReissueInput};
use techdepo_api_types::orders::PaymentEntry;
use techdepo_api_types::vocab::RefundReason;
use techdepo_api_types::{Evidence, PaymentId};
use techdepo_db::{OrgScope, views};
use techdepo_domain::{Actor, InvoiceId, InvoiceNumber, Money, RefundId};

use super::super::contract::{OrderBrief, Reply, RouteOutcome, RouteRequest, WorkflowRef};
use super::{Routed, choose_order, escalate, load_order, refund_blocked, reply, report};
use crate::Deps;
use crate::errors::sql;
use crate::invoicing::InvoicingClient;
use crate::notifications::templates::{CorrectedInvoice, SupportTemplate};
use crate::refund::RefundWorkflowClient;

/// An unrefunded duplicate charge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Duplicate {
    /// The legitimate charge.
    pub first: PaymentId,
    /// The duplicate.
    pub duplicate: PaymentId,
    /// Amount of each.
    pub amount: Money,
    /// Seconds between them.
    pub seconds_apart: i64,
}

/// What the detectors found (`detect-billing`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Findings {
    /// A duplicate charge not refunded yet.
    pub duplicate: Option<Duplicate>,
    /// The current invoice, when it lacks a billing address (and can still be voided).
    pub missing_address: Option<(InvoiceId, InvoiceNumber)>,
}

impl Findings {
    /// Whether anything is wrong.
    pub fn any(&self) -> bool {
        self.duplicate.is_some() || self.missing_address.is_some()
    }
}

/// The first unrefunded duplicate among the order's payments.
pub fn unrefunded_duplicate(entries: &[PaymentEntry]) -> Option<Duplicate> {
    find_duplicate_charges(entries)
        .into_iter()
        .find(|d| !d.resolved)
        .map(|d| Duplicate {
            first: d.first,
            duplicate: d.duplicate,
            amount: d.amount,
            seconds_apart: d.seconds_apart,
        })
}

/// The `BillingIssue` route.
pub struct BillingIssueRoute {
    deps: Arc<Deps>,
}

impl BillingIssueRoute {
    /// The route over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }

    async fn detect(
        &self,
        ctx: &Context<'_>,
        req: &RouteRequest,
        order: &OrderBrief,
    ) -> Result<Findings, TerminalError> {
        let db = self.deps.db.clone();
        let (org, id) = (req.org_id.clone(), order.order_id.clone());
        let findings: Json<Findings> = ctx
            .run(|| async move {
                let scope = OrgScope::verified(org);
                let payments = db.payments(&scope).for_order(&id).await.map_err(sql)?;
                let entries: Vec<_> = payments.iter().map(views::payment_entry).collect();
                let invoice = db
                    .invoices(&scope)
                    .current_for_order(&id)
                    .await
                    .map_err(sql)?
                    .filter(|i| i.billing_address.is_none() && views::can_void(i));
                Ok(Json(Findings {
                    duplicate: unrefunded_duplicate(&entries),
                    missing_address: invoice.map(|i| (i.id, i.number)),
                }))
            })
            .name("detect-billing")
            .await?;
        Ok(findings.into_inner())
    }

    async fn decide(&self, ctx: &Context<'_>, req: &RouteRequest) -> Result<Routed, TerminalError> {
        let (order, _) = match choose_order(req, true) {
            Ok(chosen) => chosen,
            Err(outcome) => return Ok(Routed::without_order(*outcome)),
        };
        let findings = self.detect(ctx, req, &order).await?;
        if !findings.any() {
            return Ok(Routed::about(
                &order,
                escalate(format!(
                    "No duplicate charge or invoice problem found on {}",
                    order.number
                )),
            ));
        }
        let refund = findings
            .duplicate
            .as_ref()
            .map(|d| (RefundReason::DuplicateCharge, Some(d.duplicate.clone())));
        let facts = load_order(&self.deps, ctx, req, &order, refund).await?;
        let n = facts.number;

        // The duplicate's refund (needs approval).
        let mut started: Option<(RefundId, Money)> = None;
        let mut blocked = None;
        if let Some(dup) = &findings.duplicate {
            match &facts.refund {
                Some(Ok(plan)) if !facts.refund_pending => {
                    let refund_id = super::refund_id(req, &facts.order_id);
                    let _ = ctx
                        .workflow_client::<RefundWorkflowClient>(refund_id.as_str())
                        .run(Json(StartRefundInput {
                            org_id: req.org_id.clone(),
                            order_id: facts.order_id.clone(),
                            amount: Some(plan.amount),
                            reason: RefundReason::DuplicateCharge,
                            reason_text: Some(format!(
                                "Charged twice, {}s apart (ticket {})",
                                dup.seconds_apart, req.ticket_id
                            )),
                            evidence: vec![
                                Evidence::DuplicateCharge {
                                    order_number: n,
                                    first: dup.first.clone(),
                                    duplicate: dup.duplicate.clone(),
                                    amount: dup.amount,
                                    seconds_apart: dup.seconds_apart,
                                },
                                Evidence::OrderState {
                                    order_number: n,
                                    status: facts.status,
                                    workdays_since_ship: None,
                                },
                            ],
                            pre_approved: false,
                            requested_by: Actor::Customer,
                            requested_by_user: Some(req.user_id.clone()),
                            charge_payment_id: Some(dup.duplicate.clone()),
                            ticket_id: Some(req.ticket_id.clone()),
                            investigation_id: None,
                        }))
                        .send();
                    started = Some((refund_id, plan.amount));
                }
                Some(Err(error)) => blocked = Some(refund_blocked(&facts, error)),
                _ => blocked = Some(SupportTemplate::RefundUnderWay { order_number: n }),
            }
        }

        // The invoice (automatic: no money moves).
        let mut corrected = None;
        if let Some((invoice_id, _)) = &findings.missing_address {
            let reissued = ctx
                .service_client::<InvoicingClient>()
                .void_and_reissue(Json(VoidReissueInput {
                    org_id: req.org_id.clone(),
                    invoice_id: invoice_id.clone(),
                    reason: format!("Billing address missing (ticket {})", req.ticket_id),
                    billing_address: None,
                    actor: Actor::System,
                    by: None,
                }))
                .call()
                .await?
                .into_inner();
            corrected = Some(CorrectedInvoice {
                voided: reissued.voided.number,
                reissued: reissued.reissued.number,
            });
        }

        let outcome = match (started, corrected, blocked) {
            (Some((refund_id, amount)), invoice, _) => RouteOutcome::WaitingOnWorkflow {
                workflow: WorkflowRef::Refund(refund_id),
                reply: Some(Reply::template(SupportTemplate::BillingFixed {
                    order_number: n,
                    duplicate: Some(amount),
                    invoice,
                })),
            },
            (None, Some(invoice), _) => reply(
                SupportTemplate::BillingFixed {
                    order_number: n,
                    duplicate: None,
                    invoice: Some(invoice),
                },
                true,
            ),
            (None, None, Some(template)) => reply(template, true),
            (None, None, None) => escalate("The billing detectors found nothing to act on"),
        };
        Ok(Routed::about(&order, outcome))
    }
}

#[restate_sdk::service(name = "BillingIssue", ingress_private = true)]
impl BillingIssueRoute {
    /// Handles "I was charged twice" / "my invoice is wrong" and reports to the ticket.
    #[handler]
    async fn run(&self, ctx: Context<'_>, input: Json<RouteRequest>) -> Result<(), HandlerError> {
        let req = input.into_inner();
        let routed = self.decide(&ctx, &req).await;
        report(&ctx, &req, "billing issue", routed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use techdepo_api_types::vocab::{PaymentKind, PaymentMethod, PaymentStatus};
    use techdepo_domain::{OrderId, OrderNumber};
    use time::Duration;
    use time::macros::datetime;

    use super::*;

    fn entry(id: &str, kind: PaymentKind, secs: i64, refund_of: Option<&str>) -> PaymentEntry {
        let at = datetime!(2026-10-16 14:00 UTC) + Duration::seconds(secs);
        PaymentEntry {
            id: PaymentId::new(id),
            order_id: OrderId::new("ord_1"),
            order_number: OrderNumber::new(1042),
            kind,
            status: PaymentStatus::Succeeded,
            amount: Money::from_euros(100),
            method: PaymentMethod::Card,
            provider_ref: id.into(),
            statement_descriptor: "TECHDEPO TD-1042".into(),
            refund_of: refund_of.map(PaymentId::new),
            failure_reason: None,
            created_at: at,
            sim_at: at,
        }
    }

    #[test]
    fn the_duplicate_detector() {
        let once = [entry("pay_1", PaymentKind::Charge, 0, None)];
        assert_eq!(unrefunded_duplicate(&once), None);
        let twice = [
            entry("pay_1", PaymentKind::Charge, 0, None),
            entry("pay_2", PaymentKind::Charge, 4, None),
        ];
        let dup = unrefunded_duplicate(&twice).unwrap();
        assert_eq!(dup.duplicate, PaymentId::new("pay_2"));
        assert_eq!((dup.amount, dup.seconds_apart), (Money::from_euros(100), 4));
        let refunded = [
            twice[0].clone(),
            twice[1].clone(),
            entry("pay_3", PaymentKind::Refund, 60, Some("pay_2")),
        ];
        assert_eq!(unrefunded_duplicate(&refunded), None, "already refunded");
        let apart = [
            entry("pay_1", PaymentKind::Charge, 0, None),
            entry("pay_2", PaymentKind::Charge, 3_600, None),
        ];
        assert_eq!(
            unrefunded_duplicate(&apart),
            None,
            "an hour apart is a new payment"
        );
    }

    #[test]
    fn findings() {
        assert!(!Findings::default().any());
        let invoice = Findings {
            duplicate: None,
            missing_address: Some((InvoiceId::new("inv_1"), InvoiceNumber::new(2026, 1))),
        };
        assert!(invoice.any());
    }
}
