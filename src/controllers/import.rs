use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, extract::State};
use axum::{extract::Path, routing::get};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, SchemaScope, Verified};
use crate::dtos::jobs::QueuedJobResponse;

/// Line-delimited JSON import in the same format `GET /api/export.jsonl` produces: one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line.
/// Requires `SchemaScope` (rather than `WriteScope`) since an import can create schemas, which is itself a schema-scope-only operation elsewhere in the API.
///
/// Accepted imports run all-or-nothing on a worker; poll `/api/import-jobs/{job_id}` for completion.
#[cfg(feature = "openapi")]
fn import_jsonl_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/import.jsonl",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/x-ndjson": {"schema": crate::controllers::openapi::route_inventory::schema::<String>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["import_create"]), "responses": {"202": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<QueuedJobResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
pub(crate) async fn import_jsonl(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    body: String,
) -> Result<(StatusCode, Json<QueuedJobResponse>), ApiError> {
    let authorized = Authorized::<SchemaScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::ImportCreate,
        ),
    )
    .await?;
    let job_id =
        crate::workers::import::enqueue(&ctx, authorized.txn(), &authorized.ctx, body).await?;
    authorized.commit().await?;
    Ok((StatusCode::ACCEPTED, Json(QueuedJobResponse { job_id })))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![import_jsonl_doc(), import_job_status_doc()]
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/api/import.jsonl", post(import_jsonl))
        .add("/api/import-jobs/{job_id}", get(import_job_status))
}

#[cfg(feature = "openapi")]
fn import_job_status_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/import-jobs/{job_id}",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["import_job_get"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::models::queue_job_lifecycles::BulkJobStatus>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "job_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}, {"name": "X-Workspace-Id", "in": "header", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
async fn import_job_status(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<crate::models::queue_job_lifecycles::BulkJobStatus>, ApiError> {
    let mut authorized = Authorized::<SchemaScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Job(id),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::ImportJobGet)
        .await?;
    Ok(Json(
        crate::models::queue_job_lifecycles::bulk_job_status(
            authorized.txn(),
            authorized.ctx.require_workspace()?,
            id,
            &["import"],
        )
        .await?,
    ))
}
