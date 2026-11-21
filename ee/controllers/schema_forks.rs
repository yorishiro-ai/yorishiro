//! REST endpoints for workspace-owned schema forks.

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, SchemaScope, Verified};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use uuid::Uuid;

use crate::edition::ee::models::workspace_schema_forks as model;

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/schema-forks", responses((status = 200, body = [crate::edition::ee::controllers::openapi::ForkRecord]), (status = 401, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["schema_fork_list"]))), tag = "enterprise"))]
async fn list(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Query(page): Query<crate::dtos::common::PageParams>,
) -> Result<Json<Vec<model::ForkRecord>>, ApiError> {
    Ok(Json(
        model::list_for_actor(&ctx, &verified.ctx, page.try_into()?).await?,
    ))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/schema-forks/{fork_id}", params(("fork_id" = Uuid, Path)), responses((status = 200, body = crate::edition::ee::controllers::openapi::ForkRecord), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"])), ("x-yorishiro-required-actions" = json!(["schema_fork_get"]))), tag = "enterprise"))]
async fn get(
    State(ctx): State<AppContext>,
    verified: Verified<ReadScope>,
    Path(fork_id): Path<Uuid>,
) -> Result<Json<model::ForkRecord>, ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Workspace(
        model::workspace_for_actor(&ctx.db, &verified.ctx, fork_id).await?,
    );
    let mut authorized = Authorized::<ReadScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaForkGet)
        .await?;
    Ok(Json(
        model::get(
            authorized.txn(),
            authorized.ctx.tenant_id,
            authorized.ctx.require_workspace()?,
            fork_id,
        )
        .await?,
    ))
}

#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/schema-forks", request_body = crate::edition::ee::controllers::openapi::CreateForkRequest, responses((status = 201, body = crate::edition::ee::controllers::openapi::ForkRecord), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 422, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"])), ("x-yorishiro-required-actions" = json!(["schema_fork_create"]))), tag = "enterprise"))]
async fn create(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Json(input): Json<model::CreateInput>,
) -> Result<(StatusCode, Json<model::ForkRecord>), ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Permission(
        crate::models::iam::policy::Action::SchemaForkCreate,
    );
    let mut authorized = Authorized::<SchemaScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaForkCreate)
        .await?;
    let fork = model::create(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.require_workspace()?,
        input,
    )
    .await?;
    authorized.commit().await?;
    Ok((StatusCode::CREATED, Json(fork)))
}

#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/schema-forks/{fork_id}", params(("fork_id" = Uuid, Path)), request_body = crate::edition::ee::controllers::openapi::UpdateForkRequest, responses((status = 200, body = crate::edition::ee::controllers::openapi::ForkRecord), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody), (status = 422, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"])), ("x-yorishiro-required-actions" = json!(["schema_fork_update"]))), tag = "enterprise"))]
async fn update(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Path(fork_id): Path<Uuid>,
    Json(input): Json<model::UpdateInput>,
) -> Result<Json<model::ForkRecord>, ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Workspace(
        model::workspace_for_actor(&ctx.db, &verified.ctx, fork_id).await?,
    );
    let mut authorized = Authorized::<SchemaScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaForkUpdate)
        .await?;
    let fork = model::update(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.require_workspace()?,
        fork_id,
        input,
    )
    .await?;
    authorized.commit().await?;
    Ok(Json(fork))
}

#[cfg_attr(feature = "openapi", utoipa::path(delete, path = "/api/schema-forks/{fork_id}", params(("fork_id" = Uuid, Path)), responses((status = 204, description = "Schema fork deleted"), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"])), ("x-yorishiro-required-actions" = json!(["schema_fork_delete"]))), tag = "enterprise"))]
async fn delete(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Path(fork_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let target = crate::models::iam::access::WorkspaceTarget::Workspace(
        model::workspace_for_actor(&ctx.db, &verified.ctx, fork_id).await?,
    );
    let mut authorized = Authorized::<SchemaScope>::for_target(&ctx, verified.ctx, target).await?;
    authorized
        .require_action(crate::models::iam::policy::Action::SchemaForkDelete)
        .await?;
    model::delete(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.require_workspace()?,
        fork_id,
    )
    .await?;
    authorized.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::openapi::route_inventory::RouteDoc> {
    vec![
        crate::controllers::openapi::route_inventory::path_doc(__path_list),
        crate::controllers::openapi::route_inventory::path_doc(__path_create),
        crate::controllers::openapi::route_inventory::path_doc(__path_get),
        crate::controllers::openapi::route_inventory::path_doc(__path_update),
        crate::controllers::openapi::route_inventory::path_doc(__path_delete),
    ]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/schema-forks")
        .add("", axum::routing::get(list).post(create))
        .add(
            "/{fork_id}",
            axum::routing::get(get).put(update).delete(delete),
        )
}
