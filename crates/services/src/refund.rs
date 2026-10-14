//! `RefundWorkflow` (workflow, keyed by refund ID): the only way money goes back to a customer
//! (support.md §4).
//!
//! ```text
//! run(StartRefundInput):
//!   ctx.run open-refund        plan (which charge, how much, full or partial) + insert `refunds` row
//!   Order/record_event         refund_requested (sets refund_pending)
//!   unless pre_approved:
//!     HumanTasks/create        approve_refund task carrying this workflow's awakeable
//!     await awakeable          HumanTasks/decide resolves it (SLA nudges come from HumanTasks)
//!     reject → record-rejection, Order/record_event refund_rejected, tell the customer → end
//!     approve → record-approval, Order/record_event refund_approved
//!   Payments/refund            idempotency key = refund ID; refused (terminal: less left of
//!                              the charge than planned) → record-failure (`rejected`),
//!                              Order/record_event refund_rejected, tell the customer → end
//!   Invoicing/credit_note      when invoiced, unless the reason is a duplicate charge
//!   ctx.run complete-refund
//!   Order/mark_refunded        full: order → refunded; partial: timeline entry only
//!   tell the customer          the ticket (support seam) or Notifications/order
//! on a terminal failure:       record-error (`rejected`, clears refund_pending) unless the money
//!                              went back; the ticket escalates (`support::refund_failed`)
//! ```
//!
//! Who pays back what is a pure function ([`plan_refund`]): by default the order's oldest
//! captured charge with something left; for a duplicate charge, the latest extra charge. A refund of everything that
//! is left of the first charge is a *full* refund and moves the order to `refunded`.

use std::sync::Arc;

use restate_ext::ContextClockExt;
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::ingress::{RefundFinal, StartRefundInput, api_error_from_terminal};
use techdepo_api_types::orders::InvoiceRef;
use techdepo_api_types::tasks::{HumanDecision, TaskContext};
use techdepo_api_types::vocab::{OrderEventKind, RefKind, RefundStatus};
use techdepo_db::OrgScope;
use techdepo_db::refunds::NewRefund;
pub use techdepo_db::refunds::{RefundPlan, plan_refund};
use techdepo_domain::{Actor, OrderNumber, OrgId, RefundId, UserId};
use time::OffsetDateTime;

use crate::Deps;
use crate::errors::{not_found, reject, sql};
use crate::human_tasks::{CreateTaskInput, HumanTasksClient, TaskResolution};
use crate::invoicing::{CreditNoteInput, InvoicingClient};
use crate::notifications::{NotificationsClient, OrderNotice, OrderNoticeInput};
use crate::order::{MarkRefundedInput, OrderClient, RecordEventInput};
use crate::payments::{PaymentsClient, RefundInput};
use crate::support::{self, RefundOutcome};
use crate::util;

/// What `open-refund` journals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Opened {
    plan: RefundPlan,
    number: OrderNumber,
}

/// The `RefundWorkflow` workflow.
pub struct RefundWorkflow {
    deps: Arc<Deps>,
}

impl RefundWorkflow {
    /// The workflow over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }
}

/// A timeline entry about this refund.
#[allow(clippy::too_many_arguments)]
fn refund_event(
    org: &OrgId,
    refund: &RefundId,
    kind: OrderEventKind,
    actor: Actor,
    by: Option<UserId>,
    summary: String,
    refund_pending: Option<bool>,
    invocation: &str,
) -> RecordEventInput {
    RecordEventInput {
        org_id: org.clone(),
        kind,
        actor,
        actor_user_id: by,
        summary,
        reference: Some((RefKind::Refund, refund.to_string())),
        data: serde_json::Value::Null,
        refund_pending,
        investigation_open: None,
        invocation_id: Some(invocation.to_owned()),
    }
}

/// How far a refund got, for the failure path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Nothing recorded yet.
    Planning,
    /// The `refunds` row exists (and the order may be flagged `refund_pending`).
    Opened,
    /// The money went back: only the bookkeeping is left.
    Paid,
}

async fn now(ctx: &WorkflowContext<'_>) -> Result<OffsetDateTime, TerminalError> {
    Ok(util::to_offset(ctx.now().await?))
}

#[restate_sdk::workflow(name = "RefundWorkflow")]
impl RefundWorkflow {
    /// Executes one refund, after approval unless pre-approved. A terminal failure closes the
    /// refund (unless the money already went back) and hands the ticket that asked for it to a
    /// person, so the ticket never waits on a dead workflow.
    #[handler]
    async fn run(
        &self,
        ctx: WorkflowContext<'_>,
        input: Json<StartRefundInput>,
    ) -> Result<Json<RefundFinal>, HandlerError> {
        let input = input.into_inner();
        let mut stage = Stage::Planning;
        match self.execute(&ctx, &input, &mut stage).await {
            Ok(done) => Ok(Json(done)),
            Err(error) => {
                let message = api_error_from_terminal(error.code(), error.message()).message;
                if stage == Stage::Opened {
                    self.record_error(&ctx, &input, &message).await?;
                }
                if let Some(ticket) = &input.ticket_id {
                    let reason = if stage == Stage::Paid {
                        format!("The refund was paid but could not be finished: {message}")
                    } else {
                        format!("The refund could not be completed: {message}")
                    };
                    support::refund_failed(&ctx, &input.org_id, ticket, reason);
                }
                Err(error.into())
            }
        }
    }
}

impl RefundWorkflow {
    /// Closes a refund that failed before paying (`rejected`) and clears the order's flag.
    async fn record_error(
        &self,
        ctx: &WorkflowContext<'_>,
        input: &StartRefundInput,
        message: &str,
    ) -> Result<(), TerminalError> {
        let refund_id = RefundId::new(ctx.key());
        let reason = format!("The refund could not be completed: {message}");
        let db = self.deps.db.clone();
        let (scope, id, why) = (
            OrgScope::verified(input.org_id.clone()),
            refund_id.clone(),
            reason.clone(),
        );
        let failed_at = now(ctx).await?;
        ctx.run(|| async move {
            db.batch(&[db.refunds(&scope).fail_statement(&id, &why, failed_at)])
                .await
                .map_err(sql)?;
            Ok(())
        })
        .name("record-error")
        .await?;
        ctx.object_client::<OrderClient>(input.order_id.as_str())
            .record_event(Json(refund_event(
                &input.org_id,
                &refund_id,
                OrderEventKind::RefundRejected,
                Actor::System,
                None,
                reason,
                Some(false),
                ctx.invocation_id(),
            )))
            .call()
            .await
    }

    /// The refund itself; `stage` says how far it got when it fails.
    async fn execute(
        &self,
        ctx: &WorkflowContext<'_>,
        input: &StartRefundInput,
        stage: &mut Stage,
    ) -> Result<RefundFinal, TerminalError> {
        let refund_id = RefundId::new(ctx.key());
        let org = input.org_id.clone();
        let order_id = input.order_id.clone();
        let at = now(ctx).await?;

        // -- open: plan and record ----------------------------------------------------------------
        let db = self.deps.db.clone();
        let (req, id) = (input.clone(), refund_id.clone());
        let opened: Json<Opened> = ctx
            .run(|| async move {
                let scope = OrgScope::verified(req.org_id.clone());
                let order = db
                    .orders(&scope)
                    .get(&req.order_id)
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| not_found("order"))?;
                let refunds = db.refunds(&scope);
                let payments = db
                    .payments(&scope)
                    .for_order(&order.id)
                    .await
                    .map_err(sql)?;
                let others: Vec<_> = refunds
                    .for_order(&order.id)
                    .await
                    .map_err(sql)?
                    .into_iter()
                    .filter(|r| r.id != id)
                    .collect();
                if let Some(existing) = refunds.get(&id).await.map_err(sql)? {
                    // A retry after the insert: the plan as recorded.
                    let charge = existing
                        .charge_payment_id
                        .ok_or_else(|| crate::errors::internal("refund without its charge"))?;
                    let mut plan = plan_refund(
                        &payments,
                        &others,
                        req.reason,
                        Some(&charge),
                        Some(existing.amount),
                    )
                    .unwrap_or(RefundPlan {
                        charge_payment_id: charge,
                        amount: existing.amount,
                        full: false,
                    });
                    plan.amount = existing.amount;
                    return Ok(Json(Opened {
                        plan,
                        number: order.number,
                    }));
                }
                let plan = plan_refund(
                    &payments,
                    &others,
                    req.reason,
                    req.charge_payment_id.as_ref(),
                    req.amount,
                )
                .map_err(reject)?;
                let record = NewRefund {
                    id: id.clone(),
                    order_id: order.id.clone(),
                    ticket_id: req.ticket_id.clone(),
                    investigation_id: req.investigation_id.clone(),
                    amount: plan.amount,
                    reason: req.reason,
                    reason_text: req.reason_text.clone(),
                    pre_approved: req.pre_approved,
                    charge_payment_id: Some(plan.charge_payment_id.clone()),
                    evidence: req.evidence.clone(),
                    created_at: at,
                };
                db.batch(&[refunds.insert_statement(&record).map_err(sql)?])
                    .await
                    .map_err(sql)?;
                Ok(Json(Opened {
                    plan,
                    number: order.number,
                }))
            })
            .name("open-refund")
            .await?;
        let Opened { plan, number } = opened.into_inner();
        *stage = Stage::Opened;
        let amount = plan.amount;
        let order = ctx.object_client::<OrderClient>(order_id.as_str());
        order
            .record_event(Json(refund_event(
                &org,
                &refund_id,
                OrderEventKind::RefundRequested,
                input.requested_by,
                input.requested_by_user.clone(),
                format!(
                    "Refund of {amount} requested ({}){}",
                    input.reason.label().to_lowercase(),
                    if input.pre_approved {
                        ", pre-approved"
                    } else {
                        ""
                    }
                ),
                Some(true),
                ctx.invocation_id(),
            )))
            .call()
            .await?;

        // -- approval ------------------------------------------------------------------------------
        let approver: Option<UserId> = if input.pre_approved {
            input.requested_by_user.clone()
        } else {
            let (awakeable_id, decision) = ctx.awakeable::<Json<TaskResolution>>();
            let task_id = ctx
                .service_client::<HumanTasksClient>()
                .create(Json(CreateTaskInput {
                    org_id: org.clone(),
                    context: TaskContext::ApproveRefund {
                        refund_id: refund_id.clone(),
                        amount,
                        reason: input.reason,
                        reason_text: input.reason_text.clone(),
                    },
                    title: format!("Approve {amount} refund for {number}"),
                    summary: match &input.reason_text {
                        Some(text) if !text.trim().is_empty() => {
                            format!("{}: {}", input.reason.label(), text.trim())
                        }
                        _ => input.reason.label().to_owned(),
                    },
                    evidence: input.evidence.clone(),
                    awakeable_id: Some(awakeable_id),
                    ticket_id: input.ticket_id.clone(),
                    order_id: Some(order_id.clone()),
                    refund_id: Some(refund_id.clone()),
                    investigation_id: input.investigation_id.clone(),
                }))
                .call()
                .await?
                .into_inner();
            let db = self.deps.db.clone();
            let (scope, id, task) = (
                OrgScope::verified(org.clone()),
                refund_id.clone(),
                task_id.clone(),
            );
            ctx.run(|| async move {
                db.batch(&[db.refunds(&scope).set_task_statement(&id, &task, at)])
                    .await
                    .map_err(sql)?;
                Ok(())
            })
            .name("link-task")
            .await?;
            if input.ticket_id.is_none() {
                let _ = ctx
                    .service_client::<NotificationsClient>()
                    .order(Json(OrderNoticeInput {
                        org_id: org.clone(),
                        order_id: order_id.clone(),
                        notice: OrderNotice::RefundStarted {
                            refund_id: refund_id.clone(),
                            amount,
                        },
                    }))
                    .send();
            }

            let resolution = decision.await?.into_inner();
            let decided_at = now(ctx).await?;
            let approved = matches!(resolution.decision, HumanDecision::Approve);
            let reason = match &resolution.decision {
                HumanDecision::Reject { reason } => Some(reason.trim().to_owned()),
                _ => None,
            };
            let db = self.deps.db.clone();
            let (scope, id, by, why) = (
                OrgScope::verified(org.clone()),
                refund_id.clone(),
                resolution.decided_by.clone(),
                reason.clone(),
            );
            ctx.run(|| async move {
                db.batch(&[db.refunds(&scope).decide_statement(
                    &id,
                    approved,
                    Some(&by),
                    why.as_deref(),
                    decided_at,
                )])
                .await
                .map_err(sql)?;
                Ok(())
            })
            .name(if approved {
                "record-approval"
            } else {
                "record-rejection"
            })
            .await?;

            if !approved {
                let reason = reason.unwrap_or_else(|| "Not approved".to_owned());
                order
                    .record_event(Json(refund_event(
                        &org,
                        &refund_id,
                        OrderEventKind::RefundRejected,
                        Actor::Human,
                        Some(resolution.decided_by.clone()),
                        format!("Refund of {amount} rejected: {reason}"),
                        Some(false),
                        ctx.invocation_id(),
                    )))
                    .call()
                    .await?;
                let outcome = RefundOutcome {
                    refund_id: refund_id.clone(),
                    status: RefundStatus::Rejected,
                    amount,
                    reason: Some(reason.clone()),
                };
                if !self.ticket_handles(ctx, input, &outcome) {
                    let _ = ctx
                        .service_client::<NotificationsClient>()
                        .order(Json(OrderNoticeInput {
                            org_id: org.clone(),
                            order_id: order_id.clone(),
                            notice: OrderNotice::RefundRejected {
                                refund_id: refund_id.clone(),
                                amount,
                                reason,
                            },
                        }))
                        .send();
                }
                return Ok(RefundFinal {
                    status: RefundStatus::Rejected,
                });
            }
            order
                .record_event(Json(refund_event(
                    &org,
                    &refund_id,
                    OrderEventKind::RefundApproved,
                    Actor::Human,
                    Some(resolution.decided_by.clone()),
                    format!("Refund of {amount} approved"),
                    None,
                    ctx.invocation_id(),
                )))
                .call()
                .await?;
            Some(resolution.decided_by)
        };

        // -- execute -------------------------------------------------------------------------------
        let db = self.deps.db.clone();
        let (scope, oid) = (OrgScope::verified(org.clone()), order_id.clone());
        let clock_now = now(ctx).await?;
        let sim_at: Json<OffsetDateTime> = ctx
            .run(|| async move {
                let clock = db.orders(&scope).clock(&oid).await.map_err(sql)?;
                Ok(Json(clock.now(clock_now)))
            })
            .name("order-clock")
            .await?;
        let refunded = ctx
            .service_client::<PaymentsClient>()
            .refund(Json(RefundInput {
                org_id: org.clone(),
                order_id: order_id.clone(),
                charge_payment_id: plan.charge_payment_id.clone(),
                amount,
                refund_id: refund_id.clone(),
                sim_at: sim_at.into_inner(),
            }))
            .idempotency_key(refund_id.as_str())
            .call()
            .await;
        let refund_payment = match refunded {
            Ok(payment) => {
                *stage = Stage::Paid;
                payment.into_inner()
            }
            Err(refused) => {
                // Terminal: the charge has less left than planned (another refund got there
                // first). Close the refund instead of leaving it approved forever.
                let error = api_error_from_terminal(refused.code(), refused.message());
                let reason = format!("The payment could not be refunded: {}", error.message);
                let db = self.deps.db.clone();
                let (scope, id, why) = (
                    OrgScope::verified(org.clone()),
                    refund_id.clone(),
                    reason.clone(),
                );
                let failed_at = now(ctx).await?;
                ctx.run(|| async move {
                    db.batch(&[db.refunds(&scope).fail_statement(&id, &why, failed_at)])
                        .await
                        .map_err(sql)?;
                    Ok(())
                })
                .name("record-failure")
                .await?;
                order
                    .record_event(Json(refund_event(
                        &org,
                        &refund_id,
                        OrderEventKind::RefundRejected,
                        Actor::System,
                        None,
                        format!("Refund of {amount} failed: {}", error.message),
                        Some(false),
                        ctx.invocation_id(),
                    )))
                    .call()
                    .await?;
                let outcome = RefundOutcome {
                    refund_id: refund_id.clone(),
                    status: RefundStatus::Rejected,
                    amount,
                    reason: Some(reason.clone()),
                };
                // A pre-approved refund was never announced to the customer (the admin sees the
                // failure on the order and in the refund list); an announced one is closed.
                if !self.ticket_handles(ctx, input, &outcome) && !input.pre_approved {
                    let _ = ctx
                        .service_client::<NotificationsClient>()
                        .order(Json(OrderNoticeInput {
                            org_id: org.clone(),
                            order_id: order_id.clone(),
                            notice: OrderNotice::RefundRejected {
                                refund_id: refund_id.clone(),
                                amount,
                                reason,
                            },
                        }))
                        .send();
                }
                return Ok(RefundFinal {
                    status: RefundStatus::Rejected,
                });
            }
        };
        let credit_note: Option<InvoiceRef> = if input.reason.credits_invoice() {
            ctx.service_client::<InvoicingClient>()
                .credit_note(Json(CreditNoteInput {
                    org_id: org.clone(),
                    order_id: order_id.clone(),
                    amount,
                    reason: match &input.reason_text {
                        Some(text) if !text.trim().is_empty() => text.trim().to_owned(),
                        _ => input.reason.label().to_owned(),
                    },
                    refund_id: Some(refund_id.clone()),
                }))
                .call()
                .await?
                .into_inner()
        } else {
            None
        };
        let db = self.deps.db.clone();
        let (scope, id, payment, credit) = (
            OrgScope::verified(org.clone()),
            refund_id.clone(),
            refund_payment.clone(),
            credit_note.as_ref().map(|c| c.id.clone()),
        );
        let done_at = now(ctx).await?;
        ctx.run(|| async move {
            db.batch(&[db.refunds(&scope).complete_statement(
                &id,
                &payment,
                credit.as_ref(),
                done_at,
            )])
            .await
            .map_err(sql)?;
            Ok(())
        })
        .name("complete-refund")
        .await?;
        order
            .mark_refunded(Json(MarkRefundedInput {
                org_id: org.clone(),
                refund_id: refund_id.clone(),
                amount,
                full: plan.full,
                actor: if approver.is_some() {
                    Actor::Human
                } else {
                    input.requested_by
                },
                by: approver,
            }))
            .call()
            .await?;

        let outcome = RefundOutcome {
            refund_id: refund_id.clone(),
            status: RefundStatus::Completed,
            amount,
            reason: None,
        };
        if !self.ticket_handles(ctx, input, &outcome) {
            let _ = ctx
                .service_client::<NotificationsClient>()
                .order(Json(OrderNoticeInput {
                    org_id: org,
                    order_id,
                    notice: OrderNotice::RefundDone {
                        refund_id,
                        amount,
                        full: plan.full,
                        credit_note: credit_note.map(|c| c.number),
                    },
                }))
                .send();
        }
        Ok(RefundFinal {
            status: RefundStatus::Completed,
        })
    }

    /// Whether the ticket that asked for the refund tells the customer (support seam).
    fn ticket_handles(
        &self,
        ctx: &WorkflowContext<'_>,
        input: &StartRefundInput,
        outcome: &RefundOutcome,
    ) -> bool {
        input
            .ticket_id
            .as_ref()
            .is_some_and(|ticket| support::refund_finished(ctx, &input.org_id, ticket, outcome))
    }
}

#[cfg(test)]
mod tests {
    use techdepo_api_types::vocab::{PaymentKind, PaymentMethod, PaymentStatus, RefundReason};
    use techdepo_api_types::{ApiError, ErrorCode, PaymentId};
    use techdepo_db::payments::PaymentRow;
    use techdepo_db::refunds::RefundRow;
    use techdepo_domain::{Money, OrderId, OrderStatus};
    use time::macros::datetime;

    use super::*;

    fn payment(
        id: &str,
        kind: PaymentKind,
        amount: i64,
        refund_of: Option<&str>,
        secs: i64,
    ) -> PaymentRow {
        PaymentRow {
            id: PaymentId::new(id),
            order_id: OrderId::new("ord_1"),
            order_number: OrderNumber::new(1042),
            user_id: UserId::new("user_1"),
            customer_email: "a@b.c".into(),
            customer_name: None,
            kind,
            status: PaymentStatus::Succeeded,
            amount: Money::from_euros(amount),
            method: PaymentMethod::Card,
            provider_ref: "x".into(),
            refund_of: refund_of.map(PaymentId::new),
            refund_id: None,
            failure_reason: None,
            created_at: datetime!(2026-10-16 14:00 UTC) + time::Duration::seconds(secs),
            sim_at: datetime!(2026-10-16 14:00 UTC),
        }
    }

    #[test]
    fn full_refund_of_the_payment() {
        let ledger = [payment("pay_1", PaymentKind::Charge, 100, None, 0)];
        let plan = plan_refund(&ledger, &[], RefundReason::Cancellation, None, None).unwrap();
        assert_eq!(plan.charge_payment_id, PaymentId::new("pay_1"));
        assert_eq!(plan.amount, Money::from_euros(100));
        assert!(plan.full);
        let partial = plan_refund(
            &ledger,
            &[],
            RefundReason::Goodwill,
            None,
            Some(Money::from_euros(10)),
        )
        .unwrap();
        assert!(!partial.full);
        let too_much = plan_refund(
            &ledger,
            &[],
            RefundReason::Goodwill,
            None,
            Some(Money::from_euros(101)),
        )
        .unwrap_err();
        assert_eq!(too_much.code, ErrorCode::Validation);
    }

    #[test]
    fn duplicate_charge_refunds_the_extra_charge_only() {
        let ledger = [
            payment("pay_1", PaymentKind::Charge, 100, None, 0),
            payment("pay_2", PaymentKind::Charge, 100, None, 3),
        ];
        let plan = plan_refund(&ledger, &[], RefundReason::DuplicateCharge, None, None).unwrap();
        assert_eq!(plan.charge_payment_id, PaymentId::new("pay_2"));
        assert!(!plan.full, "the order stays paid");
        let mut refunded = ledger.to_vec();
        refunded.push(payment("pay_3", PaymentKind::Refund, 100, Some("pay_2"), 9));
        let none =
            plan_refund(&refunded, &[], RefundReason::DuplicateCharge, None, None).unwrap_err();
        assert_eq!(none.code, ErrorCode::InvalidTransition);
        let unpaid = plan_refund(&[], &[], RefundReason::Cancellation, None, None).unwrap_err();
        assert_eq!(unpaid.status(), 409);
    }

    #[test]
    fn default_charge_moves_on_once_the_original_is_refunded() {
        // Charged twice; the original was refunded (e.g. a goodwill refund of the wrong charge):
        // what is left is the later charge, and an explicit charge is capped by what is left of it.
        let ledger = [
            payment("pay_1", PaymentKind::Charge, 100, None, 0),
            payment("pay_2", PaymentKind::Charge, 100, None, 3),
            payment("pay_3", PaymentKind::Refund, 100, Some("pay_1"), 9),
        ];
        let plan = plan_refund(&ledger, &[], RefundReason::Goodwill, None, None).unwrap();
        assert_eq!(plan.charge_payment_id, PaymentId::new("pay_2"));
        assert_eq!(plan.amount, Money::from_euros(100));
        assert!(
            !plan.full,
            "only the first charge's refund is the order's full refund"
        );
        let over = plan_refund(
            &ledger,
            &[],
            RefundReason::Goodwill,
            Some(&PaymentId::new("pay_2")),
            Some(Money::from_euros(150)),
        )
        .unwrap_err();
        assert_eq!(over.code, ErrorCode::Validation);
        let named = plan_refund(
            &ledger[..2],
            &[],
            RefundReason::Goodwill,
            Some(&PaymentId::new("pay_2")),
            None,
        )
        .unwrap();
        assert_eq!(named.charge_payment_id, PaymentId::new("pay_2"));
        assert!(!named.full);
    }

    fn open_refund(id: &str, charge: &str, amount: i64, status: RefundStatus) -> RefundRow {
        let at = datetime!(2026-10-16 15:00 UTC);
        RefundRow {
            id: RefundId::new(id),
            order_id: OrderId::new("ord_1"),
            order_number: OrderNumber::new(1042),
            order_status: OrderStatus::Delivered,
            user_id: UserId::new("user_1"),
            customer_email: "a@b.c".into(),
            customer_name: None,
            ticket_id: None,
            investigation_id: None,
            amount: Money::from_euros(amount),
            reason: RefundReason::Return,
            reason_text: None,
            status,
            pre_approved: false,
            charge_payment_id: Some(PaymentId::new(charge)),
            refund_payment_id: None,
            credit_note_id: None,
            human_task_id: None,
            decided_by: None,
            decided_at: None,
            rejection_reason: None,
            evidence: Vec::new(),
            created_at: at,
            updated_at: at,
            completed_at: None,
        }
    }

    #[test]
    fn open_refunds_claim_their_amount() {
        let ledger = [payment("pay_1", PaymentKind::Charge, 100, None, 0)];
        // A return waiting for approval claims the whole charge: nothing is left for another.
        let waiting = [open_refund("ref_1", "pay_1", 100, RefundStatus::Requested)];
        let err: ApiError =
            plan_refund(&ledger, &waiting, RefundReason::Goodwill, None, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidTransition);
        assert!(err.message.contains("in progress"));
        let partial = [open_refund("ref_1", "pay_1", 30, RefundStatus::Approved)];
        let plan = plan_refund(&ledger, &partial, RefundReason::Goodwill, None, None).unwrap();
        assert_eq!(plan.amount, Money::from_euros(70));
        assert!(!plan.full);
        // Rejected and completed refunds claim nothing more than the ledger shows.
        let closed = [
            open_refund("ref_1", "pay_1", 100, RefundStatus::Rejected),
            open_refund("ref_2", "pay_1", 100, RefundStatus::Completed),
        ];
        assert!(
            plan_refund(&ledger, &closed, RefundReason::Cancellation, None, None)
                .unwrap()
                .full
        );
        // An approved refund already in the ledger is counted once.
        let mut paid = ledger.to_vec();
        let mut entry = payment("pay_2", PaymentKind::Refund, 30, Some("pay_1"), 9);
        entry.refund_id = Some(RefundId::new("ref_1"));
        paid.push(entry);
        let plan = plan_refund(&paid, &partial, RefundReason::Goodwill, None, None).unwrap();
        assert_eq!(plan.amount, Money::from_euros(70));
    }
}
