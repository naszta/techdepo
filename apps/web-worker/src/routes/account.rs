//! The caller's cart, saved builds, address book and checkout (`Customer` object, keyed
//! `<org>:<user>`).

use techdepo_api_types::account::{
    AddressList, BuildDetail, BuildList, Cart, PlaceOrderResponse, SavedAddress,
};
use techdepo_api_types::endpoints::{
    AddBuildToCart, AddToCart, ClearCart, CreateAddress, CreateBuild, DeleteAddress, DeleteBuild,
    GetBuild, GetCart, ListAddresses, ListBuilds, PlaceOrder, RemoveCartItem, SetCartItem,
    UpdateAddress, UpdateBuild,
};
use techdepo_api_types::ingress::{
    AddressRefInput, BuildRefInput, CartItemInput, CheckoutInput, CustomerAddBuildToCart,
    CustomerAddToCart, CustomerCheckout, CustomerClearCart, CustomerDeleteAddress,
    CustomerDeleteBuild, CustomerSaveAddress, CustomerSaveBuild, CustomerSetCartItem,
    SaveAddressInput, SaveBuildInput, derive_id, idempotency_key,
};
use techdepo_api_types::{
    AddressId, ApiError, BuildId, Deleted, Endpoint, OrderRef, ProductId, Validate,
};
use techdepo_db::views;

use crate::app::App;
use crate::auth::Caller;
use crate::endpoint::Input;
use crate::error::{ApiResult, DbResultExt, FoundExt};
use crate::throttle;

// -- cart ---------------------------------------------------------------------------------------

/// The caller's cart, priced from the catalog with the org's stock.
pub async fn read_cart(app: &App, caller: &Caller) -> ApiResult<Cart> {
    let scope = caller.scope();
    let rows = app.db.carts(&scope).get(&caller.user_id).await.db()?;
    let stock = app.db.inventory(&scope).stock_map().await.db()?;
    Ok(views::cart(&rows, &stock, app.catalog()))
}

pub async fn cart(app: App, caller: Caller, _: Input<GetCart>) -> ApiResult<Cart> {
    read_cart(&app, &caller).await
}

fn known_product(app: &App, id: &ProductId) -> ApiResult<()> {
    app.catalog()
        .get(id)
        .map(drop)
        .found(format!("Product {id}"))
}

async fn set_line<H>(
    app: &App,
    caller: &Caller,
    product_id: ProductId,
    quantity: u32,
) -> ApiResult<Cart>
where
    H: techdepo_api_types::ingress::Handler<Input = CartItemInput, Output = ()>,
{
    let input = CartItemInput {
        customer: caller.customer(),
        product_id,
        quantity,
    };
    app.ingress
        .call::<H>(Some(&caller.customer_key()), &input, None)
        .await?;
    read_cart(app, caller).await
}

pub async fn add_to_cart(app: App, caller: Caller, input: Input<AddToCart>) -> ApiResult<Cart> {
    let req = input.body;
    req.validate()?;
    known_product(&app, &req.product_id)?;
    set_line::<CustomerAddToCart>(&app, &caller, req.product_id, req.quantity).await
}

pub async fn set_cart_item(app: App, caller: Caller, input: Input<SetCartItem>) -> ApiResult<Cart> {
    input.body.validate()?;
    let product_id = ProductId::new(input.param(0));
    known_product(&app, &product_id)?;
    set_line::<CustomerSetCartItem>(&app, &caller, product_id, input.body.quantity).await
}

pub async fn remove_cart_item(
    app: App,
    caller: Caller,
    input: Input<RemoveCartItem>,
) -> ApiResult<Cart> {
    let product_id = ProductId::new(input.param(0));
    set_line::<CustomerSetCartItem>(&app, &caller, product_id, 0).await
}

pub async fn clear_cart(app: App, caller: Caller, _: Input<ClearCart>) -> ApiResult<Cart> {
    app.ingress
        .call::<CustomerClearCart>(Some(&caller.customer_key()), &caller.customer(), None)
        .await?;
    read_cart(&app, &caller).await
}

// -- builds -------------------------------------------------------------------------------------

async fn read_build(app: &App, caller: &Caller, id: &BuildId) -> ApiResult<BuildDetail> {
    let row = app
        .db
        .builds(&caller.scope())
        .get(&caller.user_id, id)
        .await
        .found(format!("Build {id}"))?;
    Ok(views::build_detail(&row, app.catalog()))
}

fn unknown_parts(app: &App, ids: &[ProductId]) -> ApiResult<()> {
    match ids.iter().find(|id| app.catalog().get(*id).is_none()) {
        Some(id) => Err(ApiError::invalid_field(
            "product_ids",
            format!("Unknown product {id}"),
        )),
        None => Ok(()),
    }
}

pub async fn list_builds(app: App, caller: Caller, _: Input<ListBuilds>) -> ApiResult<BuildList> {
    let rows = app
        .db
        .builds(&caller.scope())
        .list(&caller.user_id)
        .await
        .db()?;
    Ok(BuildList {
        builds: rows
            .iter()
            .map(|r| views::build_summary(r, app.catalog()))
            .collect(),
    })
}

pub async fn create_build(
    app: App,
    caller: Caller,
    input: Input<CreateBuild>,
) -> ApiResult<BuildDetail> {
    let req = input.body;
    req.validate()?;
    unknown_parts(&app, &req.product_ids)?;
    let id = BuildId::new(derive_id(
        "bld",
        &[
            caller.org_id().as_str(),
            caller.user_id.as_str(),
            &req.idempotency_key,
        ],
    ));
    let key = idempotency_key(
        caller.org_id(),
        &caller.user_id,
        CreateBuild::ROUTE.name,
        &req.idempotency_key,
    );
    let save = SaveBuildInput {
        customer: caller.customer(),
        build_id: id.clone(),
        name: Some(req.name),
        product_ids: Some(req.product_ids),
    };
    app.ingress
        .call::<CustomerSaveBuild>(Some(&caller.customer_key()), &save, Some(&key))
        .await?;
    read_build(&app, &caller, &id).await
}

pub async fn get_build(app: App, caller: Caller, input: Input<GetBuild>) -> ApiResult<BuildDetail> {
    read_build(&app, &caller, &BuildId::new(input.param(0))).await
}

pub async fn update_build(
    app: App,
    caller: Caller,
    input: Input<UpdateBuild>,
) -> ApiResult<BuildDetail> {
    input.body.validate()?;
    let id = BuildId::new(input.param(0));
    let req = input.body;
    read_build(&app, &caller, &id).await?;
    if let Some(ids) = &req.product_ids {
        unknown_parts(&app, ids)?;
    }
    let save = SaveBuildInput {
        customer: caller.customer(),
        build_id: id.clone(),
        name: req.name,
        product_ids: req.product_ids,
    };
    app.ingress
        .call::<CustomerSaveBuild>(Some(&caller.customer_key()), &save, None)
        .await?;
    read_build(&app, &caller, &id).await
}

pub async fn delete_build(
    app: App,
    caller: Caller,
    input: Input<DeleteBuild>,
) -> ApiResult<Deleted> {
    let id = BuildId::new(input.param(0));
    read_build(&app, &caller, &id).await?;
    let del = BuildRefInput {
        customer: caller.customer(),
        build_id: id.clone(),
    };
    app.ingress
        .call::<CustomerDeleteBuild>(Some(&caller.customer_key()), &del, None)
        .await?;
    Ok(Deleted {
        id: id.as_str().to_owned(),
    })
}

pub async fn add_build_to_cart(
    app: App,
    caller: Caller,
    input: Input<AddBuildToCart>,
) -> ApiResult<Cart> {
    let id = BuildId::new(input.param(0));
    read_build(&app, &caller, &id).await?;
    let add = BuildRefInput {
        customer: caller.customer(),
        build_id: id,
    };
    app.ingress
        .call::<CustomerAddBuildToCart>(Some(&caller.customer_key()), &add, None)
        .await?;
    read_cart(&app, &caller).await
}

// -- addresses ----------------------------------------------------------------------------------

async fn read_address(app: &App, caller: &Caller, id: &AddressId) -> ApiResult<SavedAddress> {
    let row = app
        .db
        .addresses(&caller.scope())
        .get(&caller.user_id, id)
        .await
        .found(format!("Address {id}"))?;
    Ok(views::saved_address(&row))
}

pub async fn list_addresses(
    app: App,
    caller: Caller,
    _: Input<ListAddresses>,
) -> ApiResult<AddressList> {
    let rows = app
        .db
        .addresses(&caller.scope())
        .list(&caller.user_id)
        .await
        .db()?;
    Ok(AddressList {
        addresses: rows.iter().map(views::saved_address).collect(),
    })
}

async fn save_address(
    app: &App,
    caller: &Caller,
    id: AddressId,
    req: techdepo_api_types::account::SaveAddressRequest,
    idempotency: Option<String>,
) -> ApiResult<SavedAddress> {
    let save = SaveAddressInput {
        customer: caller.customer(),
        address_id: id.clone(),
        label: req.label,
        address: req.address,
        default_shipping: req.default_shipping,
        default_billing: req.default_billing,
    };
    app.ingress
        .call::<CustomerSaveAddress>(Some(&caller.customer_key()), &save, idempotency.as_deref())
        .await?;
    read_address(app, caller, &id).await
}

pub async fn create_address(
    app: App,
    caller: Caller,
    input: Input<CreateAddress>,
) -> ApiResult<SavedAddress> {
    let req = input.body;
    req.validate()?;
    let client_key = req
        .idempotency_key
        .clone()
        .ok_or_else(|| ApiError::invalid_field("idempotency_key", "Required"))?;
    let id = AddressId::new(derive_id(
        "adr",
        &[
            caller.org_id().as_str(),
            caller.user_id.as_str(),
            &client_key,
        ],
    ));
    let key = idempotency_key(
        caller.org_id(),
        &caller.user_id,
        CreateAddress::ROUTE.name,
        &client_key,
    );
    save_address(&app, &caller, id, req, Some(key)).await
}

pub async fn update_address(
    app: App,
    caller: Caller,
    input: Input<UpdateAddress>,
) -> ApiResult<SavedAddress> {
    input.body.validate()?;
    let id = AddressId::new(input.param(0));
    let req = input.body;
    read_address(&app, &caller, &id).await?;
    save_address(&app, &caller, id, req, None).await
}

pub async fn delete_address(
    app: App,
    caller: Caller,
    input: Input<DeleteAddress>,
) -> ApiResult<Deleted> {
    let id = AddressId::new(input.param(0));
    read_address(&app, &caller, &id).await?;
    let del = AddressRefInput {
        customer: caller.customer(),
        address_id: id.clone(),
    };
    app.ingress
        .call::<CustomerDeleteAddress>(Some(&caller.customer_key()), &del, None)
        .await?;
    Ok(Deleted {
        id: id.as_str().to_owned(),
    })
}

// -- checkout -----------------------------------------------------------------------------------

/// Places the order: `Customer/checkout` re-checks stock, the expected total and compatibility
/// (`409`s), calls `Order/place` and empties the cart.
pub async fn checkout(
    app: App,
    caller: Caller,
    input: Input<PlaceOrder>,
) -> ApiResult<PlaceOrderResponse> {
    let req = input.body;
    req.validate()?;
    throttle::order_quota(&app, &caller).await?;
    let key = idempotency_key(
        caller.org_id(),
        &caller.user_id,
        PlaceOrder::ROUTE.name,
        &req.idempotency_key,
    );
    let checkout = CheckoutInput {
        customer: caller.customer(),
        customer_email: caller.email.clone(),
        customer_name: caller.name.clone(),
        shipping_address: req.shipping_address,
        billing_address: req.billing_address,
        payment_method: req.payment_method,
        expected_total: req.expected_total,
        override_compat: req.override_compat,
        save_address: req.save_address,
    };
    let placed = app
        .ingress
        .call::<CustomerCheckout>(Some(&caller.customer_key()), &checkout, Some(&key))
        .await?;
    Ok(PlaceOrderResponse {
        order: OrderRef {
            id: placed.order_id,
            number: placed.number,
            status: placed.status,
        },
    })
}
