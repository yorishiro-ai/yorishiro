//! REST endpoints for workspace-owned schema forks.

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, ReadScope, SchemaScope};
use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use loco_rs::controller::Routes;
use uuid::Uuid;

use crate::ee::models::workspace_schema_forks as model;

#[utoipa::path(get, path = "/api/schema-forks", responses((status = 200, body = [crate::controllers::openapi::ForkRecord]), (status = 401, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"]))), tag = "enterprise")]
async fn list(authorized: Authorized<ReadScope>) -> Result<Json<Vec<model::ForkRecord>>, ApiError> {
    Ok(Json(
        model::list(
            authorized.txn(),
            authorized.ctx.tenant_id,
            authorized.ctx.workspace_id,
        )
        .await?,
    ))
}

#[utoipa::path(get, path = "/api/schema-forks/{fork_id}", params(("fork_id" = Uuid, Path)), responses((status = 200, body = crate::controllers::openapi::ForkRecord), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["read"]))), tag = "enterprise")]
async fn get(
    authorized: Authorized<ReadScope>,
    Path(fork_id): Path<Uuid>,
) -> Result<Json<model::ForkRecord>, ApiError> {
    Ok(Json(
        model::get(
            authorized.txn(),
            authorized.ctx.tenant_id,
            authorized.ctx.workspace_id,
            fork_id,
        )
        .await?,
    ))
}

#[utoipa::path(post, path = "/api/schema-forks", request_body = crate::controllers::openapi::CreateForkRequest, responses((status = 201, body = crate::controllers::openapi::ForkRecord), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 422, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "enterprise")]
async fn create(
    authorized: Authorized<SchemaScope>,
    Json(input): Json<model::CreateInput>,
) -> Result<(StatusCode, Json<model::ForkRecord>), ApiError> {
    let fork = model::create(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.workspace_id,
        input,
    )
    .await?;
    authorized.commit().await?;
    Ok((StatusCode::CREATED, Json(fork)))
}

#[utoipa::path(put, path = "/api/schema-forks/{fork_id}", params(("fork_id" = Uuid, Path)), request_body = crate::controllers::openapi::UpdateForkRequest, responses((status = 200, body = crate::controllers::openapi::ForkRecord), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody), (status = 422, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "enterprise")]
async fn update(
    authorized: Authorized<SchemaScope>,
    Path(fork_id): Path<Uuid>,
    Json(input): Json<model::UpdateInput>,
) -> Result<Json<model::ForkRecord>, ApiError> {
    let fork = model::update(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.workspace_id,
        fork_id,
        input,
    )
    .await?;
    authorized.commit().await?;
    Ok(Json(fork))
}

#[utoipa::path(delete, path = "/api/schema-forks/{fork_id}", params(("fork_id" = Uuid, Path)), responses((status = 204, description = "Schema fork deleted"), (status = 401, body = crate::controllers::openapi::ApiErrorBody), (status = 404, body = crate::controllers::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "enterprise")]
async fn delete(
    authorized: Authorized<SchemaScope>,
    Path(fork_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    model::delete(
        authorized.txn(),
        authorized.ctx.tenant_id,
        authorized.ctx.workspace_id,
        fork_id,
    )
    .await?;
    authorized.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn openapi_docs() -> Vec<crate::controllers::route_inventory::RouteDoc> {
    vec![
        crate::controllers::route_inventory::path_doc(__path_list),
        crate::controllers::route_inventory::path_doc(__path_create),
        crate::controllers::route_inventory::path_doc(__path_get),
        crate::controllers::route_inventory::path_doc(__path_update),
        crate::controllers::route_inventory::path_doc(__path_delete),
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
