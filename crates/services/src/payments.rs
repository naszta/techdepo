//! `Payments` (service, ingress-private): the fake payment service provider.
//!
//! Captures write the `payments` ledger; the `Order` VO turns the outcome into its state and
//! timeline. The "charge twice" fault captures two charges a few seconds apart (what the
//! duplicate detector looks for), "declined" writes one failed charge. Refunds are executed
//! only by `RefundWorkflow` (support pipeline) through [`Payments`]'s `refund` handler.

use std::sync::Arc;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::PaymentId;
use techdepo_api_types::vocab::{PaymentKind, PaymentMethod, PaymentStatus};
use techdepo_db::payments::NewPayment;
use techdepo_db::{OrgScope, rows_affected};
use techdepo_domain::{Money, OrderId, OrgId, PaymentFault, RefundId};
use time::{Duration, OffsetDateTime};

use crate::Deps;
use crate::errors::{invalid, not_found, not_now, sql};
use crate::util::{self, mint};

/// Seconds between the two captures of the "charge twice" fault.
pub const DUPLICATE_CAPTURE_GAP_SECS: i64 = 3;

/// `Payments/capture` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureInput {
    /// Org.
    pub org_id: OrgId,
    /// Order.
    pub order_id: OrderId,
    /// Amount to capture.
    pub amount: Money,
    /// The order's method (`card` or `bank_transfer`).
    pub method: PaymentMethod,
    /// The armed payment fault.
    pub fault: PaymentFault,
    /// The order's simulated now.
    #[serde(with = "time::serde::rfc3339")]
    pub sim_at: OffsetDateTime,
}

/// A captured charge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Charge {
    /// Ledger entry.
    pub id: PaymentId,
    /// Fake PSP reference.
    pub provider_ref: String,
    /// Amount.
    pub amount: Money,
}

/// `Payments/capture` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureOutcome {
    /// Captured charges: one, or two for the "charge twice" fault.
    pub charges: Vec<Charge>,
    /// The declined charge, for the "declined" fault.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declined: Option<Declined>,
}

/// A declined charge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declined {
    /// Ledger entry (status `failed`).
    pub id: PaymentId,
    /// Why.
    pub reason: String,
}

/// `Payments/refund` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundInput {
    /// Org.
    pub org_id: OrgId,
    /// Order.
    pub order_id: OrderId,
    /// The succeeded charge to reverse.
    pub charge_payment_id: PaymentId,
    /// Amount (at most what is left of the charge).
    pub amount: Money,
    /// The refund workflow.
    pub refund_id: RefundId,
    /// The order's simulated now.
    #[serde(with = "time::serde::rfc3339")]
    pub sim_at: OffsetDateTime,
}

/// The pure part of a capture: which ledger entries to write.
pub fn plan_capture(
    input: &CaptureInput,
    ids: [(PaymentId, String); 2],
    at: OffsetDateTime,
) -> (Vec<NewPayment>, CaptureOutcome) {
    let [(first_id, first_ref), (second_id, second_ref)] = ids;
    let entry = |id: PaymentId,
                 provider_ref: String,
                 offset: i64,
                 status: PaymentStatus,
                 failure: Option<String>| {
        NewPayment {
            id,
            order_id: input.order_id.clone(),
            kind: PaymentKind::Charge,
            status,
            amount: input.amount,
            method: input.method,
            provider_ref,
            refund_of: None,
            refund_id: None,
            failure_reason: failure,
            created_at: at + Duration::seconds(offset),
            sim_at: input.sim_at + Duration::seconds(offset),
        }
    };
    match input.fault {
        PaymentFault::Declined => {
            let reason = "card_declined: insufficient funds".to_owned();
            let failed = entry(
                first_id.clone(),
                first_ref,
                0,
                PaymentStatus::Failed,
                Some(reason.clone()),
            );
            (
                vec![failed],
                CaptureOutcome {
                    charges: Vec::new(),
                    declined: Some(Declined {
                        id: first_id,
                        reason,
                    }),
                },
            )
        }
        PaymentFault::None | PaymentFault::ChargeTwice => {
            let mut entries = vec![entry(
                first_id,
                first_ref,
                0,
                PaymentStatus::Succeeded,
                None,
            )];
            if input.fault == PaymentFault::ChargeTwice {
                entries.push(entry(
                    second_id,
                    second_ref,
                    DUPLICATE_CAPTURE_GAP_SECS,
                    PaymentStatus::Succeeded,
                    None,
                ));
            }
            let charges = entries
                .iter()
                .map(|e| Charge {
                    id: e.id.clone(),
                    provider_ref: e.provider_ref.clone(),
                    amount: e.amount,
                })
                .collect();
            (
                entries,
                CaptureOutcome {
                    charges,
                    declined: None,
                },
            )
        }
    }
}

/// The `Payments` service.
pub struct Payments {
    deps: Arc<Deps>,
}

impl Payments {
    /// The service over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }
}

#[restate_sdk::service(name = "Payments", ingress_private = true)]
impl Payments {
    /// Captures the order's amount, applying the armed fault.
    #[handler]
    async fn capture(
        &self,
        mut ctx: Context<'_>,
        input: Json<CaptureInput>,
    ) -> Result<Json<CaptureOutcome>, HandlerError> {
        let input = input.into_inner();
        if !PaymentMethod::CUSTOMER.contains(&input.method) {
            return Err(not_now("The store pays for this order"));
        }
        let at = util::now(&ctx).await?;
        let ids = [
            (
                PaymentId::new(mint("pay", ctx.rand_uuid())),
                mint("ch", ctx.rand_uuid()),
            ),
            (
                PaymentId::new(mint("pay", ctx.rand_uuid())),
                mint("ch", ctx.rand_uuid()),
            ),
        ];
        let (entries, outcome) = plan_capture(&input, ids, at);
        let db = self.deps.db.clone();
        let scope = OrgScope::verified(input.org_id.clone());
        let order_id = input.order_id.clone();
        ctx.run(|| async move {
            if db
                .orders(&scope)
                .get(&order_id)
                .await
                .map_err(sql)?
                .is_none()
            {
                return Err(not_found("order"));
            }
            let payments = db.payments(&scope);
            let statements: Vec<_> = entries
                .iter()
                .map(|e| payments.insert_statement(e))
                .collect();
            db.batch(&statements).await.map_err(sql)?;
            Ok(())
        })
        .name("capture")
        .await?;
        Ok(Json(outcome))
    }

    /// Reverses (part of) a succeeded charge. Only `RefundWorkflow` calls this.
    #[handler]
    async fn refund(
        &self,
        mut ctx: Context<'_>,
        input: Json<RefundInput>,
    ) -> Result<Json<PaymentId>, HandlerError> {
        let input = input.into_inner();
        if !input.amount.is_positive() {
            return Err(invalid("amount", "Must be positive"));
        }
        let at = util::now(&ctx).await?;
        let id = PaymentId::new(mint("pay", ctx.rand_uuid()));
        let provider_ref = mint("re", ctx.rand_uuid());
        let db = self.deps.db.clone();
        let scope = OrgScope::verified(input.org_id.clone());
        let result: Json<PaymentId> = ctx
            .run(|| async move {
                let payments = db.payments(&scope);
                let entries = payments.for_order(&input.order_id).await.map_err(sql)?;
                if let Some(done) = entries
                    .iter()
                    .find(|e| e.refund_id.as_ref() == Some(&input.refund_id))
                {
                    return Ok(Json(done.id.clone()));
                }
                let charge = entries
                    .iter()
                    .find(|e| e.id == input.charge_payment_id)
                    .ok_or_else(|| not_found("charge"))?;
                if charge.kind != PaymentKind::Charge || charge.status != PaymentStatus::Succeeded {
                    return Err(not_now("Only a captured charge can be refunded"));
                }
                let already: Money = entries
                    .iter()
                    .filter(|e| {
                        e.refund_of.as_ref() == Some(&charge.id)
                            && e.status == PaymentStatus::Succeeded
                    })
                    .map(|e| e.amount)
                    .sum();
                if already + input.amount > charge.amount {
                    return Err(invalid(
                        "amount",
                        format!("At most {} is left of this charge", charge.amount - already),
                    ));
                }
                let entry = NewPayment {
                    id: id.clone(),
                    order_id: input.order_id.clone(),
                    kind: PaymentKind::Refund,
                    status: PaymentStatus::Succeeded,
                    amount: input.amount,
                    method: charge.method,
                    provider_ref,
                    refund_of: Some(charge.id.clone()),
                    refund_id: Some(input.refund_id.clone()),
                    failure_reason: None,
                    created_at: at,
                    sim_at: input.sim_at,
                };
                // The check above is a read; the insert re-checks atomically, so a refund
                // racing for the same charge cannot over-commit it.
                let results = db
                    .batch(&[payments.refund_insert_statement(&entry)])
                    .await
                    .map_err(sql)?;
                if rows_affected(&results, 0) == 0 {
                    return Err(invalid(
                        "amount",
                        "Another refund of this charge got there first: less is left of it now",
                    ));
                }
                Ok(Json(id))
            })
            .name("refund")
            .await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn input(fault: PaymentFault) -> CaptureInput {
        CaptureInput {
            org_id: OrgId::new("org_1"),
            order_id: OrderId::new("ord_1"),
            amount: Money::from_euros(100),
            method: PaymentMethod::Card,
            fault,
            sim_at: datetime!(2026-10-16 14:00 UTC),
        }
    }

    fn ids() -> [(PaymentId, String); 2] {
        [
            (PaymentId::new("pay_1"), "ch_1".into()),
            (PaymentId::new("pay_2"), "ch_2".into()),
        ]
    }

    #[test]
    fn capture_plans() {
        let at = datetime!(2026-10-16 14:00 UTC);
        let (entries, outcome) = plan_capture(&input(PaymentFault::None), ids(), at);
        assert_eq!(entries.len(), 1);
        assert_eq!(outcome.charges.len(), 1);
        assert!(outcome.declined.is_none());

        let (entries, outcome) = plan_capture(&input(PaymentFault::ChargeTwice), ids(), at);
        assert_eq!(outcome.charges.len(), 2);
        assert_eq!(
            (entries[1].created_at - entries[0].created_at).whole_seconds(),
            DUPLICATE_CAPTURE_GAP_SECS
        );
        const {
            assert!(
                DUPLICATE_CAPTURE_GAP_SECS
                    < techdepo_api_types::admin::DUPLICATE_CHARGE_WINDOW_SECS
            )
        };

        let (entries, outcome) = plan_capture(&input(PaymentFault::Declined), ids(), at);
        assert_eq!(entries[0].status, PaymentStatus::Failed);
        assert!(outcome.charges.is_empty());
        assert_eq!(outcome.declined.unwrap().id, PaymentId::new("pay_1"));
    }
}
