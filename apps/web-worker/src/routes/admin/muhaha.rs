//! `/api/admin/muhaha/*` (`Access::Operator`, checked by `endpoint::authed`): the operator's
//! tools across every org, and the app-wide model settings. The org list is read from D1
//! (unscoped, `techdepo_db::orgs::list_all`); everything else goes through the `Muhaha` service.

use techdepo_api_types::admin::DemoResetResponse;
use techdepo_api_types::endpoints::{
    ClearOrg, GetModels, ListMuhahaOrgs, ReseedOrg, ResetAllOrgs, UpdateModels,
};
use techdepo_api_types::ingress::{
    MuhahaClearOrg, MuhahaModels, MuhahaReseedOrg, MuhahaResetAll, MuhahaUpdateModels,
    OperatorInput, OrgTargetInput, UpdateModelsInput,
};
use techdepo_api_types::muhaha::{
    ModelsResponse, MuhahaOrg, MuhahaOrgList, ReseedResponse, ResetAllResponse,
};
use techdepo_api_types::{OrgId, Validate};
use techdepo_catalog::CATALOG_VERSION;

use crate::app::App;
use crate::auth::Caller;
use crate::endpoint::Input;
use crate::error::{ApiResult, DbResultExt};

pub async fn orgs(app: App, caller: Caller, _: Input<ListMuhahaOrgs>) -> ApiResult<MuhahaOrgList> {
    let rows = techdepo_db::orgs::list_all(app.db.sql()).await.db()?;
    Ok(MuhahaOrgList {
        orgs: rows
            .into_iter()
            .map(|row| MuhahaOrg {
                current: &row.id == caller.org_id(),
                catalog_current: row.catalog_version.as_deref() == Some(CATALOG_VERSION),
                id: row.id,
                public_id: row.public_id,
                name: row.name,
                created_at: row.created_at,
                expires_at: row.expires_at,
                members: row.members,
                orders: row.orders,
                tickets: row.tickets,
            })
            .collect(),
    })
}

pub async fn reset_all(
    app: App,
    caller: Caller,
    input: Input<ResetAllOrgs>,
) -> ApiResult<ResetAllResponse> {
    input.body.validate()?;
    let by = OperatorInput {
        by: caller.user_id.clone(),
    };
    app.ingress.call::<MuhahaResetAll>(None, &by, None).await
}

pub async fn clear_org(
    app: App,
    caller: Caller,
    input: Input<ClearOrg>,
) -> ApiResult<DemoResetResponse> {
    input.body.validate()?;
    let target = OrgTargetInput {
        by: caller.user_id.clone(),
        org_id: OrgId::new(input.param(0)),
    };
    app.ingress
        .call::<MuhahaClearOrg>(None, &target, None)
        .await
}

pub async fn reseed_org(
    app: App,
    caller: Caller,
    input: Input<ReseedOrg>,
) -> ApiResult<ReseedResponse> {
    let target = OrgTargetInput {
        by: caller.user_id.clone(),
        org_id: OrgId::new(input.param(0)),
    };
    app.ingress
        .call::<MuhahaReseedOrg>(None, &target, None)
        .await
}

pub async fn models(app: App, caller: Caller, _: Input<GetModels>) -> ApiResult<ModelsResponse> {
    let by = OperatorInput {
        by: caller.user_id.clone(),
    };
    app.ingress.call::<MuhahaModels>(None, &by, None).await
}

pub async fn update_models(
    app: App,
    caller: Caller,
    input: Input<UpdateModels>,
) -> ApiResult<ModelsResponse> {
    input.body.validate()?;
    let update = UpdateModelsInput {
        by: caller.user_id.clone(),
        settings: input.body.settings,
    };
    app.ingress
        .call::<MuhahaUpdateModels>(None, &update, None)
        .await
}
