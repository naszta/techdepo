//! Catalog, stock, product pages and the (pure) compatibility API.

use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get};
use techdepo_api_types::catalog::{
    AlternativesResponse, CatalogResponse, CompatCheckResponse, ProductContentDto, ProductDetail,
    Stock, StockEntry, StockResponse,
};
use techdepo_api_types::endpoints::{CheckCompat, FindAlternatives, GetProduct, GetStock};
use techdepo_api_types::{ApiError, Category, Product, ProductId, Validate};
use techdepo_catalog::{CATALOG_VERSION, Catalog};
use techdepo_compat::{
    Build, Slot, alternatives as compat_alternatives, check_build, suggest_fixes,
};
use techdepo_domain::Money;

use crate::app::App;
use crate::auth::Caller;
use crate::endpoint::Input;
use crate::error::{ApiResult, DbResultExt, FoundExt};

/// "Works well with": at most this many products per other category.
const WORKS_WELL_PER_CATEGORY: usize = 2;

/// `GET /api/catalog`: public, `ETag: "<CATALOG_VERSION>"`, cacheable for a day (the version
/// changes with every catalog change, and the SPA compares it with `/api/config`).
pub fn catalog_route() -> MethodRouter<App> {
    get(|req: Request| async move {
        let etag = format!("\"{CATALOG_VERSION}\"");
        let cached = req
            .headers()
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').any(|t| t.trim() == etag));
        let mut response: Response = if cached {
            StatusCode::NOT_MODIFIED.into_response()
        } else {
            axum::Json(CatalogResponse {
                version: CATALOG_VERSION.to_owned(),
                products: Catalog::global().all().to_vec(),
            })
            .into_response()
        };
        let headers = response.headers_mut();
        if let Ok(value) = HeaderValue::from_str(&etag) {
            headers.insert(header::ETAG, value);
        }
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=86400"),
        );
        response
    })
}

pub async fn stock(app: App, caller: Caller, _: Input<GetStock>) -> ApiResult<StockResponse> {
    let stock = app.db.inventory(&caller.scope()).stock_map().await.db()?;
    Ok(StockResponse {
        items: app
            .catalog()
            .all()
            .iter()
            .map(|p| StockEntry {
                product_id: p.id.clone(),
                stock: Stock::new(stock.get(&p.id).copied().unwrap_or(0)),
            })
            .collect(),
    })
}

pub async fn product(
    app: App,
    caller: Caller,
    input: Input<GetProduct>,
) -> ApiResult<ProductDetail> {
    let catalog = app.catalog();
    let product = catalog
        .get(input.param(0))
        .found(format!("Product {}", input.param(0)))?;
    let stock = app
        .db
        .inventory(&caller.scope())
        .get(&product.id)
        .await
        .db()?
        .map_or(0, |r| r.stock);
    Ok(ProductDetail {
        product: product.clone(),
        content: catalog
            .content(&product.id)
            .map(ProductContentDto::from)
            .unwrap_or_default(),
        stock: Stock::new(stock),
        works_well_with: works_well_with(catalog, product),
    })
}

/// Compatible products from the other categories in the same tier, cheapest first.
fn works_well_with(catalog: &Catalog, product: &Product) -> Vec<ProductId> {
    let mut out = Vec::new();
    for &category in Category::ALL {
        if category == product.category {
            continue;
        }
        let mut candidates: Vec<&Product> = catalog
            .by_category(category)
            .iter()
            .filter(|c| c.tier == product.tier)
            .filter(|c| {
                Build::from_products([product, *c]).is_ok_and(|b| check_build(&b).is_compatible())
            })
            .collect();
        candidates.sort_by_key(|c| c.price);
        out.extend(
            candidates
                .into_iter()
                .take(WORKS_WELL_PER_CATEGORY)
                .map(|c| c.id.clone()),
        );
    }
    out
}

fn build<'a>(catalog: &'a Catalog, ids: &[ProductId]) -> ApiResult<Build<'a>> {
    Build::from_ids(catalog, ids.iter().map(ProductId::as_str))
        .map_err(|e| ApiError::invalid_field("product_ids", e.to_string()))
}

pub async fn check_compat(
    app: App,
    _: Caller,
    input: Input<CheckCompat>,
) -> ApiResult<CompatCheckResponse> {
    let req = input.body;
    req.validate()?;
    let catalog = app.catalog();
    let build = build(catalog, &req.product_ids)?;
    let report = check_build(&build);
    let fixes = if req.include_fixes {
        suggest_fixes(catalog, &build, &report)
    } else {
        Vec::new()
    };
    Ok(CompatCheckResponse {
        missing: build.missing_required(),
        total: build.total_price(),
        report,
        fixes,
    })
}

pub async fn alternatives(
    app: App,
    _: Caller,
    input: Input<FindAlternatives>,
) -> ApiResult<AlternativesResponse> {
    let req = input.body;
    req.validate()?;
    let catalog = app.catalog();
    let build = build(catalog, &req.product_ids)?;
    let min = req.price_min.unwrap_or(Money::ZERO);
    let max = req
        .price_max
        .map_or(Money::from_cents(i64::MAX), |m| m + Money::from_cents(1));
    Ok(AlternativesResponse {
        candidates: compat_alternatives(catalog, &build, Slot::from(req.category), min..max),
    })
}
