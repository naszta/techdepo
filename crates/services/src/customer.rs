//! `Customer` (virtual object, keyed `<org>:<user>`): the customer's cart, saved builds,
//! address book, and checkout. Calls on one customer are serialized, so "add 2 to the cart" is
//! a read-then-write without races.

use std::sync::Arc;

use restate_sdk::prelude::*;
use techdepo_api_types::account::MAX_CART_QUANTITY;
use techdepo_api_types::catalog::MAX_BUILD_PARTS;
use techdepo_api_types::ingress::{
    AddressRefInput, BuildRefInput, CartItemInput, CheckoutInput, CustomerInput, PlacedOrder,
    SaveAddressInput, SaveBuildInput, derive_id, keys,
};
use techdepo_api_types::{AddressId, ApiError, FieldError};
use techdepo_catalog::Catalog;
use techdepo_db::addresses::SaveAddress;
use techdepo_db::orders::NewOrder;
use techdepo_db::{Db, OrgScope, Statement};
use techdepo_domain::{Actor, OrderId, ProductId, UserId};
use techdepo_ports::ConstraintKind;
use time::OffsetDateTime;

use crate::Deps;
use crate::checkout::{self, CheckoutSnapshot};
use crate::errors::{invalid, not_found, reject, sql};
use crate::order::{OrderClient, PlaceOrderInput};
use crate::util::{self, mint};

/// Longest build name.
const MAX_BUILD_NAME: usize = 80;
/// Longest address label.
const MAX_ADDRESS_LABEL: usize = 40;

/// The `Customer` virtual object.
pub struct Customer {
    deps: Arc<Deps>,
}

impl Customer {
    /// The object over `deps`.
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }

    /// The scope, after checking that the key is this customer's (`404` otherwise).
    fn scope(ctx: &ObjectContext<'_>, customer: &CustomerInput) -> Result<OrgScope, HandlerError> {
        if ctx.key() != keys::customer(&customer.org_id, &customer.user_id) {
            return Err(not_found("customer"));
        }
        Ok(OrgScope::verified(customer.org_id.clone()))
    }

    async fn batch(db: &Db, statements: &[Statement]) -> Result<(), HandlerError> {
        match db.batch(statements).await {
            Ok(_) => Ok(()),
            // The org row is missing: it was never provisioned.
            Err(e) if e.is_constraint(ConstraintKind::ForeignKey) => Err(not_found("org")),
            Err(e) => Err(sql(e)),
        }
    }

    /// The cart quantities after adding `add` units of each product, capped at the per-line
    /// maximum (`strict`: over the maximum is an error). Read-only: run it in its own
    /// `ctx.run` and write the absolute result with [`Customer::set_quantities`] in another, so
    /// a write retried after an ambiguous commit sets the same quantities again instead of
    /// adding the units twice.
    async fn added_quantities(
        db: &Db,
        scope: &OrgScope,
        user: &UserId,
        add: &[(ProductId, u32)],
        strict: bool,
    ) -> Result<Vec<(ProductId, u32)>, HandlerError> {
        let current = db.carts(scope).get(user).await.map_err(sql)?;
        let mut targets = Vec::with_capacity(add.len());
        for (product, units) in add {
            let have = current
                .items
                .iter()
                .find(|i| i.product_id == *product)
                .map_or(0, |i| i.quantity);
            let wanted = have.saturating_add(*units);
            if strict && wanted > MAX_CART_QUANTITY {
                return Err(invalid(
                    "quantity",
                    format!("At most {MAX_CART_QUANTITY} of one product"),
                ));
            }
            targets.push((product.clone(), wanted.min(MAX_CART_QUANTITY)));
        }
        Ok(targets)
    }

    /// Sets absolute cart quantities (idempotent).
    async fn set_quantities(
        db: &Db,
        scope: &OrgScope,
        user: &UserId,
        targets: &[(ProductId, u32)],
        at: OffsetDateTime,
    ) -> Result<(), HandlerError> {
        let carts = db.carts(scope);
        let statements: Vec<Statement> = targets
            .iter()
            .flat_map(|(product, quantity)| {
                carts.set_quantity_statements(user, product, *quantity, at)
            })
            .collect();
        Self::batch(db, &statements).await
    }
}

fn check_product(product: &ProductId) -> Result<(), HandlerError> {
    match Catalog::global().get(product) {
        Some(_) => Ok(()),
        None => Err(not_found(format!("product {product}"))),
    }
}

#[restate_sdk::object(name = "Customer")]
impl Customer {
    /// Adds units of a product to the cart (to an existing line).
    #[handler]
    async fn add_to_cart(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<CartItemInput>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        let scope = Self::scope(&ctx, &input.customer)?;
        check_product(&input.product_id)?;
        if !(1..=MAX_CART_QUANTITY).contains(&input.quantity) {
            return Err(invalid(
                "quantity",
                format!("Must be between 1 and {MAX_CART_QUANTITY}"),
            ));
        }
        let at = util::now(&ctx).await?;
        let user = input.customer.user_id.clone();
        let (db, read_scope, read_user) = (self.deps.db.clone(), scope.clone(), user.clone());
        let targets: Json<Vec<(ProductId, u32)>> = ctx
            .run(|| async move {
                let add = [(input.product_id, input.quantity)];
                let targets =
                    Self::added_quantities(&db, &read_scope, &read_user, &add, true).await?;
                Ok(Json(targets))
            })
            .name("read-cart")
            .await?;
        let db = self.deps.db.clone();
        let targets = targets.into_inner();
        ctx.run(|| async move { Self::set_quantities(&db, &scope, &user, &targets, at).await })
            .name("add-to-cart")
            .await?;
        Ok(())
    }

    /// Sets a cart line's quantity; `0` removes it.
    #[handler]
    async fn set_cart_item(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<CartItemInput>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        let scope = Self::scope(&ctx, &input.customer)?;
        if input.quantity > MAX_CART_QUANTITY {
            return Err(invalid(
                "quantity",
                format!("Must be between 0 and {MAX_CART_QUANTITY}"),
            ));
        }
        if input.quantity > 0 {
            check_product(&input.product_id)?;
        }
        let at = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        ctx.run(|| async move {
            let statements = db.carts(&scope).set_quantity_statements(
                &input.customer.user_id,
                &input.product_id,
                input.quantity,
                at,
            );
            Self::batch(&db, &statements).await
        })
        .name("set-cart-item")
        .await?;
        Ok(())
    }

    /// Empties the cart.
    #[handler]
    async fn clear_cart(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<CustomerInput>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        let scope = Self::scope(&ctx, &input)?;
        let at = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        ctx.run(|| async move {
            let statements = db.carts(&scope).clear_statements(&input.user_id, at);
            Self::batch(&db, &statements).await
        })
        .name("clear-cart")
        .await?;
        Ok(())
    }

    /// Creates or updates a saved build (absent fields keep their value; creating needs a
    /// name).
    #[handler]
    async fn save_build(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<SaveBuildInput>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        let scope = Self::scope(&ctx, &input.customer)?;
        let mut fields = Vec::new();
        if let Some(name) = &input.name {
            let len = name.trim().chars().count();
            if !(1..=MAX_BUILD_NAME).contains(&len) {
                fields.push(FieldError::new(
                    "name",
                    format!("1–{MAX_BUILD_NAME} characters"),
                ));
            }
        }
        if let Some(ids) = &input.product_ids {
            if ids.len() > MAX_BUILD_PARTS {
                fields.push(FieldError::new(
                    "product_ids",
                    format!("At most {MAX_BUILD_PARTS} parts"),
                ));
            }
            if let Some(unknown) = ids.iter().find(|id| Catalog::global().get(*id).is_none()) {
                fields.push(FieldError::new(
                    "product_ids",
                    format!("Unknown product {unknown}"),
                ));
            }
        }
        ApiError::check(fields).map_err(reject)?;
        let at = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        ctx.run(|| async move {
            let builds = db.builds(&scope);
            let user = &input.customer.user_id;
            let existing = builds.get(user, &input.build_id).await.map_err(sql)?;
            if existing.is_none() && builds.exists(&input.build_id).await.map_err(sql)? {
                return Err(not_found("build"));
            }
            let name = match (&input.name, &existing) {
                (Some(name), _) => name.trim().to_owned(),
                (None, Some(row)) => row.name.clone(),
                (None, None) => return Err(invalid("name", "Required")),
            };
            let product_ids = match (input.product_ids, existing) {
                (Some(ids), _) => ids,
                (None, Some(row)) => row.product_ids,
                (None, None) => Vec::new(),
            };
            let statement = builds
                .save_statement(user, &input.build_id, &name, &product_ids, at)
                .map_err(sql)?;
            Self::batch(&db, &[statement]).await
        })
        .name("save-build")
        .await?;
        Ok(())
    }

    /// Deletes a saved build.
    #[handler]
    async fn delete_build(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<BuildRefInput>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        let scope = Self::scope(&ctx, &input.customer)?;
        let db = self.deps.db.clone();
        ctx.run(|| async move {
            match db
                .builds(&scope)
                .delete(&input.customer.user_id, &input.build_id)
                .await
                .map_err(sql)?
            {
                true => Ok(()),
                false => Err(not_found("build")),
            }
        })
        .name("delete-build")
        .await?;
        Ok(())
    }

    /// Adds every part of a saved build to the cart (parts no longer sold are skipped).
    #[handler]
    async fn add_build_to_cart(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<BuildRefInput>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        let scope = Self::scope(&ctx, &input.customer)?;
        let at = util::now(&ctx).await?;
        let user = input.customer.user_id.clone();
        let (db, read_scope, read_user) = (self.deps.db.clone(), scope.clone(), user.clone());
        let targets: Json<Vec<(ProductId, u32)>> = ctx
            .run(|| async move {
                let (scope, user) = (&read_scope, &read_user);
                let build = db
                    .builds(scope)
                    .get(user, &input.build_id)
                    .await
                    .map_err(sql)?
                    .ok_or_else(|| not_found("build"))?;
                let mut add: Vec<(ProductId, u32)> = Vec::new();
                for id in build
                    .product_ids
                    .iter()
                    .filter(|id| Catalog::global().get(*id).is_some())
                {
                    match add.iter_mut().find(|(p, _)| p == id) {
                        Some((_, n)) => *n += 1,
                        None => add.push((id.clone(), 1)),
                    }
                }
                Ok(Json(
                    Self::added_quantities(&db, scope, user, &add, false).await?,
                ))
            })
            .name("read-build")
            .await?;
        let db = self.deps.db.clone();
        let targets = targets.into_inner();
        ctx.run(|| async move { Self::set_quantities(&db, &scope, &user, &targets, at).await })
            .name("add-build-to-cart")
            .await?;
        Ok(())
    }

    /// Creates or replaces a saved address.
    #[handler]
    async fn save_address(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<SaveAddressInput>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        let scope = Self::scope(&ctx, &input.customer)?;
        let mut fields = Vec::new();
        let label_len = input.label.trim().chars().count();
        if !(1..=MAX_ADDRESS_LABEL).contains(&label_len) {
            fields.push(FieldError::new(
                "label",
                format!("1–{MAX_ADDRESS_LABEL} characters"),
            ));
        }
        input.address.check("address", &mut fields);
        ApiError::check(fields).map_err(reject)?;
        let at = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        ctx.run(|| async move {
            let addresses = db.addresses(&scope);
            let user = &input.customer.user_id;
            let owned = addresses
                .get(user, &input.address_id)
                .await
                .map_err(sql)?
                .is_some();
            if !owned && addresses.exists(&input.address_id).await.map_err(sql)? {
                return Err(not_found("address"));
            }
            let save = SaveAddress {
                id: &input.address_id,
                label: input.label.trim(),
                address: &input.address,
                default_shipping: input.default_shipping,
                default_billing: input.default_billing,
            };
            let statements = addresses.save_statements(user, &save, at).map_err(sql)?;
            Self::batch(&db, &statements).await
        })
        .name("save-address")
        .await?;
        Ok(())
    }

    /// Deletes a saved address.
    #[handler]
    async fn delete_address(
        &self,
        ctx: ObjectContext<'_>,
        input: Json<AddressRefInput>,
    ) -> Result<(), HandlerError> {
        let input = input.into_inner();
        let scope = Self::scope(&ctx, &input.customer)?;
        let db = self.deps.db.clone();
        ctx.run(|| async move {
            match db
                .addresses(&scope)
                .delete(&input.customer.user_id, &input.address_id)
                .await
                .map_err(sql)?
            {
                true => Ok(()),
                false => Err(not_found("address")),
            }
        })
        .name("delete-address")
        .await?;
        Ok(())
    }

    /// Places an order from the cart: re-checks stock, the total the customer saw and
    /// compatibility (`409`s), calls `Order/place` (which reserves the stock), then empties the
    /// cart.
    #[handler]
    async fn checkout(
        &self,
        mut ctx: ObjectContext<'_>,
        input: Json<CheckoutInput>,
    ) -> Result<Json<PlacedOrder>, HandlerError> {
        let input = input.into_inner();
        let scope = Self::scope(&ctx, &input.customer)?;
        let at = util::now(&ctx).await?;
        let db = self.deps.db.clone();
        let (load_scope, user) = (scope.clone(), input.customer.user_id.clone());
        let snapshot: Json<Option<CheckoutSnapshot>> = ctx
            .run(|| async move {
                let Some(org) = db.orgs(&load_scope).get().await.map_err(sql)? else {
                    return Ok(Json(None));
                };
                let cart = db.carts(&load_scope).get(&user).await.map_err(sql)?;
                let stock = db.inventory(&load_scope).stock_map().await.map_err(sql)?;
                let lines: Vec<(ProductId, u32)> = cart
                    .items
                    .into_iter()
                    .map(|i| (i.product_id, i.quantity))
                    .collect();
                let stock = lines
                    .iter()
                    .map(|(id, _)| (id.clone(), stock.get(id).copied().unwrap_or(0)))
                    .collect();
                Ok(Json(Some(CheckoutSnapshot {
                    lines,
                    stock,
                    settings: org.settings,
                })))
            })
            .name("load-cart")
            .await?;
        let snapshot = snapshot.into_inner().ok_or_else(|| not_found("org"))?;
        let plan = checkout::plan(&snapshot, &input, Catalog::global()).map_err(reject)?;

        let order_id = OrderId::new(mint("ord", ctx.rand_uuid()));
        if input.save_address {
            let address_id = AddressId::new(derive_id(
                "adr",
                &[
                    input.customer.org_id.as_str(),
                    input.customer.user_id.as_str(),
                    order_id.as_str(),
                ],
            ));
            let db = self.deps.db.clone();
            let (scope, user, address) = (
                scope.clone(),
                input.customer.user_id.clone(),
                input.shipping_address.clone(),
            );
            ctx.run(|| async move {
                let addresses = db.addresses(&scope);
                let first = addresses.list(&user).await.map_err(sql)?.is_empty();
                let save = SaveAddress {
                    id: &address_id,
                    label: "Shipping",
                    address: &address,
                    default_shipping: first,
                    default_billing: false,
                };
                let statements = addresses.save_statements(&user, &save, at).map_err(sql)?;
                Self::batch(&db, &statements).await
            })
            .name("save-address")
            .await?;
        }

        let placed: Json<PlacedOrder> = ctx
            .object_client::<OrderClient>(order_id.as_str())
            .place(Json(PlaceOrderInput {
                org_id: input.customer.org_id.clone(),
                order: NewOrder {
                    id: order_id.clone(),
                    user_id: input.customer.user_id.clone(),
                    customer_email: input.customer_email.clone(),
                    customer_name: input.customer_name.clone(),
                    items: plan.items,
                    shipping: plan.shipping,
                    vat_rate: snapshot.settings.vat_rate(),
                    payment_method: input.payment_method,
                    shipping_address: input.shipping_address.clone(),
                    billing_address: input.billing_address.clone(),
                    compat_override: plan.compat_override,
                    replacement_for: None,
                },
                faults: snapshot.settings.faults,
                actor: Actor::Customer,
                actor_user_id: Some(input.customer.user_id.clone()),
            }))
            .call()
            .await?;

        let db = self.deps.db.clone();
        let user = input.customer.user_id.clone();
        ctx.run(|| async move {
            let statements = db.carts(&scope).clear_statements(&user, at);
            Self::batch(&db, &statements).await
        })
        .name("clear-cart")
        .await?;
        Ok(placed)
    }
}
