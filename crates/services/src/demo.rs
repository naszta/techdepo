//! `Demo` (service): one-click scenario presets (M7). Each creates an order for the caller,
//! built from the cheapest complete compatible in-stock build, and drives it through the real
//! `Order` handlers into the chosen state, so the timeline, ledger and documents are exactly
//! what the manual demo steps would have produced.

use std::collections::HashMap;
use std::sync::Arc;

use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use techdepo_api_types::demo::{ClockStep, DemoScenario};
use techdepo_api_types::ingress::{
    AdvanceClockInput, ClockAdvanced, CreateScenarioInput, DemoActionInput, PlacedOrder,
};
use techdepo_api_types::vocab::PaymentMethod;
use techdepo_api_types::{Address, ApiError, ErrorCode};
use techdepo_catalog::{Catalog, Category, Product};
use techdepo_compat::{Build, Severity, check_build};
use techdepo_db::OrgScope;
use techdepo_db::orders::{NewOrder, NewOrderItem};
use techdepo_domain::{
    Actor, FaultToggles, InvoiceFault, LostOutcome, Money, OrderAction, OrderId, OrderStatus,
    OrgSettings, PaymentFault, ProductId, ShipmentFault,
};

use crate::Deps;
use crate::errors::{internal, not_found, reject, sql};
use crate::order::{OrderClient, PlaceOrderInput};
use crate::util::mint;

/// Most `+1 workday` steps the "delivered" preset takes (ETAs are at most 3 workdays out).
const MAX_DELIVERY_STEPS: usize = 6;

/// A step a preset drives the order through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScenarioStep {
    /// `Order/demo_action` pay.
    Pay,
    /// `Order/demo_action` ship.
    Ship,
    /// `Order/advance_clock` ship date to last Friday.
    ShipLastFriday,
    /// `Order/advance_clock` `+1 workday` until delivered.
    AdvanceUntilDelivered,
}

/// What a preset arms and does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioPlan {
    /// Faults armed at placement.
    pub faults: FaultToggles,
    /// Steps after placement.
    pub steps: Vec<ScenarioStep>,
    /// The status the order ends in.
    pub expected: OrderStatus,
}

/// The plan of a preset.
pub fn scenario_plan(scenario: DemoScenario) -> ScenarioPlan {
    use ScenarioStep as S;
    let lost = |outcome| FaultToggles {
        shipment: ShipmentFault::Lost { outcome },
        ..FaultToggles::default()
    };
    let shipped_last_friday = vec![S::Pay, S::Ship, S::ShipLastFriday];
    let (faults, steps, expected) = match scenario {
        DemoScenario::Unpaid => (
            FaultToggles::default(),
            vec![],
            OrderStatus::AwaitingPayment,
        ),
        DemoScenario::DoubleCharged => (
            FaultToggles {
                payment: PaymentFault::ChargeTwice,
                ..FaultToggles::default()
            },
            vec![S::Pay],
            OrderStatus::Processing,
        ),
        DemoScenario::InvoiceWithoutAddress => (
            FaultToggles {
                invoice: InvoiceFault::MissingBillingAddress,
                ..FaultToggles::default()
            },
            vec![S::Pay],
            OrderStatus::Processing,
        ),
        DemoScenario::ShippedLastFriday => (
            FaultToggles::default(),
            shipped_last_friday,
            OrderStatus::Shipped,
        ),
        DemoScenario::LostResurfaces => (
            lost(LostOutcome::Resurfaces),
            shipped_last_friday,
            OrderStatus::Shipped,
        ),
        DemoScenario::LostGone => (
            lost(LostOutcome::Gone),
            shipped_last_friday,
            OrderStatus::Shipped,
        ),
        DemoScenario::Delivered => (
            FaultToggles::default(),
            vec![S::Pay, S::Ship, S::ShipLastFriday, S::AdvanceUntilDelivered],
            OrderStatus::Delivered,
        ),
    };
    ScenarioPlan {
        faults,
        steps,
        expected,
    }
}

/// The cheapest complete build without compatibility errors whose parts are all in stock:
/// CPUs cheapest first, then for each slot the cheapest part that keeps the build error-free.
/// The GPU and cooler are added only when the CPU needs them.
pub fn pick_build<'c>(
    catalog: &'c Catalog,
    stock: &HashMap<ProductId, u32>,
) -> Option<Vec<&'c Product>> {
    let in_stock = |p: &&Product| stock.get(&p.id).copied().unwrap_or(0) > 0;
    let by_price = |category: Category| {
        let mut parts: Vec<&Product> = catalog
            .by_category(category)
            .iter()
            .filter(in_stock)
            .collect();
        parts.sort_by_key(|p| (p.price, p.id.clone()));
        parts
    };
    for cpu in by_price(Category::Cpu) {
        let Some(spec) = cpu.spec.as_cpu() else {
            continue;
        };
        let mut slots = vec![Category::Motherboard, Category::Memory, Category::Storage];
        if !spec.integrated_graphics {
            slots.push(Category::Gpu);
        }
        if !spec.bundled_cooler {
            slots.push(Category::Cooler);
        }
        slots.extend([Category::Case, Category::Psu]);
        let mut parts = vec![cpu];
        let complete = slots.iter().all(|&category| {
            let pick = by_price(category).into_iter().find(|candidate| {
                let mut trial = parts.clone();
                trial.push(candidate);
                Build::from_products(trial.iter().copied())
                    .map(|b| check_build(&b).worst() != Some(Severity::Error))
                    .unwrap_or(false)
            });
            pick.map(|p| parts.push(p)).is_some()
        });
        if complete {
            return Some(parts);
        }
    }
    None
}

/// What the preset reads in one step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ScenarioSnapshot {
    settings: OrgSettings,
    stock: Vec<(ProductId, u32)>,
    address: Option<Address>,
}

/// A stand-in address for a customer without an address book.
fn demo_address(name: &str) -> Address {
    Address {
        name: name.to_owned(),
        line1: "Váci utca 12".to_owned(),
        postal_code: "1052".to_owned(),
        city: "Budapest".to_owned(),
        country: "HU".to_owned(),
        ..Address::default()
    }
}

/// The `Demo` service.
pub struct Demo {
    deps: Arc<Deps>,
}

impl Demo {
    /// The service over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }
}

#[restate_sdk::service(name = "Demo")]
impl Demo {
    /// Creates an order for the caller in a preset state.
    #[handler]
    async fn create_scenario(
        &self,
        mut ctx: Context<'_>,
        input: Json<CreateScenarioInput>,
    ) -> Result<Json<PlacedOrder>, HandlerError> {
        let input = input.into_inner();
        let org_id = input.customer.org_id.clone();
        let user_id = input.customer.user_id.clone();
        let plan = scenario_plan(input.scenario);

        let db = self.deps.db.clone();
        let (scope, user) = (OrgScope::verified(org_id.clone()), user_id.clone());
        let snapshot: Json<Option<ScenarioSnapshot>> = ctx
            .run(|| async move {
                let Some(org) = db.orgs(&scope).get().await.map_err(sql)? else {
                    return Ok(Json(None));
                };
                let stock = db.inventory(&scope).stock_map().await.map_err(sql)?;
                let address = db
                    .addresses(&scope)
                    .preferred_shipping(&user)
                    .await
                    .map_err(sql)?;
                Ok(Json(Some(ScenarioSnapshot {
                    settings: org.settings,
                    stock: stock.into_iter().collect(),
                    address: address.map(|a| a.address),
                })))
            })
            .name("load-scenario")
            .await?;
        let snapshot = snapshot.into_inner().ok_or_else(|| not_found("org"))?;
        let stock: HashMap<ProductId, u32> = snapshot.stock.iter().cloned().collect();
        let parts = pick_build(Catalog::global(), &stock).ok_or_else(|| {
            reject(ApiError::new(
                ErrorCode::OutOfStock,
                "Not enough stock for a complete build; restock or reset the demo",
            ))
        })?;
        let name = input
            .customer_name
            .clone()
            .unwrap_or_else(|| "Demo Customer".to_owned());
        let shipping_address = snapshot
            .address
            .clone()
            .unwrap_or_else(|| demo_address(&name));

        let order_id = OrderId::new(mint("ord", ctx.rand_uuid()));
        let order = NewOrder {
            id: order_id.clone(),
            user_id: user_id.clone(),
            customer_email: input.customer_email.clone(),
            customer_name: input.customer_name.clone(),
            items: parts
                .iter()
                .map(|p| NewOrderItem {
                    product_id: p.id.clone(),
                    sku: p.sku.clone(),
                    name: p.name.clone(),
                    category: p.category,
                    unit_price: p.price,
                    quantity: 1,
                })
                .collect(),
            shipping: Money::ZERO,
            vat_rate: snapshot.settings.vat_rate(),
            payment_method: PaymentMethod::Card,
            shipping_address,
            billing_address: None,
            compat_override: false,
            replacement_for: None,
        };
        let mut placed: PlacedOrder = ctx
            .object_client::<OrderClient>(order_id.as_str())
            .place(Json(PlaceOrderInput {
                org_id: org_id.clone(),
                order,
                faults: plan.faults,
                actor: Actor::Customer,
                actor_user_id: Some(user_id.clone()),
            }))
            .call()
            .await?
            .into_inner();

        for step in &plan.steps {
            let order = ctx.object_client::<OrderClient>(order_id.as_str());
            placed.status = match step {
                ScenarioStep::Pay | ScenarioStep::Ship => {
                    let action = if *step == ScenarioStep::Pay {
                        OrderAction::Pay
                    } else {
                        OrderAction::Ship
                    };
                    order
                        .demo_action(Json(DemoActionInput {
                            org_id: org_id.clone(),
                            by: user_id.clone(),
                            action,
                        }))
                        .call()
                        .await?
                        .into_inner()
                }
                ScenarioStep::ShipLastFriday => {
                    let advanced: ClockAdvanced = order
                        .advance_clock(Json(AdvanceClockInput {
                            org_id: org_id.clone(),
                            by: user_id.clone(),
                            step: ClockStep::ShipLastFriday,
                        }))
                        .call()
                        .await?
                        .into_inner();
                    advanced.status
                }
                ScenarioStep::AdvanceUntilDelivered => {
                    let mut status = placed.status;
                    for _ in 0..MAX_DELIVERY_STEPS {
                        if status == OrderStatus::Delivered {
                            break;
                        }
                        let advanced: ClockAdvanced = ctx
                            .object_client::<OrderClient>(order_id.as_str())
                            .advance_clock(Json(AdvanceClockInput {
                                org_id: org_id.clone(),
                                by: user_id.clone(),
                                step: ClockStep::Workday,
                            }))
                            .call()
                            .await?
                            .into_inner();
                        status = advanced.status;
                    }
                    status
                }
            };
        }
        if placed.status != plan.expected {
            return Err(internal(format!(
                "scenario {} ended in {} instead of {}",
                input.scenario.as_str(),
                placed.status.as_str(),
                plan.expected.as_str()
            )));
        }
        Ok(Json(placed))
    }
}

#[cfg(test)]
mod tests {
    use techdepo_compat::Verdict;

    use super::*;

    fn all_in_stock() -> HashMap<ProductId, u32> {
        Catalog::global()
            .all()
            .iter()
            .map(|p| (p.id.clone(), p.default_stock.max(1)))
            .collect()
    }

    #[test]
    fn picks_a_complete_compatible_build() {
        let catalog = Catalog::global();
        let parts = pick_build(catalog, &all_in_stock()).expect("the catalog has a complete build");
        let build = Build::from_products(parts.iter().copied()).unwrap();
        assert!(
            build.missing_required().is_empty(),
            "{:?}",
            build.missing_required()
        );
        let report = check_build(&build);
        assert_ne!(report.verdict, Verdict::Incompatible, "{report:#?}");
        // The cheapest CPU that can be completed comes first.
        let cheapest_cpu = catalog
            .by_category(Category::Cpu)
            .iter()
            .map(|p| p.price)
            .min()
            .unwrap();
        assert!(parts[0].price >= cheapest_cpu);
    }

    #[test]
    fn respects_stock() {
        let catalog = Catalog::global();
        let mut stock = all_in_stock();
        let first = pick_build(catalog, &stock).unwrap();
        stock.insert(first[0].id.clone(), 0);
        let second = pick_build(catalog, &stock).unwrap();
        assert_ne!(second[0].id, first[0].id);
        assert!(pick_build(catalog, &HashMap::new()).is_none());
    }

    #[test]
    fn scenario_plans() {
        let plan = scenario_plan(DemoScenario::DoubleCharged);
        assert_eq!(plan.faults.payment, PaymentFault::ChargeTwice);
        assert_eq!(plan.steps, vec![ScenarioStep::Pay]);
        let plan = scenario_plan(DemoScenario::LostGone);
        assert_eq!(
            plan.faults.shipment,
            ShipmentFault::Lost {
                outcome: LostOutcome::Gone
            }
        );
        assert_eq!(plan.steps.last(), Some(&ScenarioStep::ShipLastFriday));
        assert_eq!(plan.expected, OrderStatus::Shipped);
        assert!(scenario_plan(DemoScenario::Unpaid).steps.is_empty());
        assert_eq!(
            scenario_plan(DemoScenario::Delivered).expected,
            OrderStatus::Delivered
        );
        // Every preset's steps are valid transitions from placement to its expected status.
        for scenario in DemoScenario::ALL {
            let plan = scenario_plan(*scenario);
            let mut status = OrderStatus::AwaitingPayment;
            for step in &plan.steps {
                status = match step {
                    ScenarioStep::Pay => status
                        .apply(OrderAction::Pay)
                        .and_then(|s| s.apply(OrderAction::StartProcessing))
                        .unwrap(),
                    ScenarioStep::Ship => status.apply(OrderAction::Ship).unwrap(),
                    ScenarioStep::ShipLastFriday => status,
                    ScenarioStep::AdvanceUntilDelivered => {
                        status.apply(OrderAction::Deliver).unwrap()
                    }
                };
            }
            assert_eq!(status, plan.expected, "{scenario:?}");
        }
    }
}
