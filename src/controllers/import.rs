use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, extract::State};
use axum::{extract::Path, routing::get};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, SchemaScope};
use crate::dtos::jobs::QueuedJobResponse;

/// Line-delimited JSON import in the same format `GET /api/export.jsonl` produces: one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line.
/// Requires `SchemaScope` (rather than `WriteScope`) since an import can create schemas, which is itself a schema-scope-only operation elsewhere in the API.
///
/// Accepted imports run all-or-nothing on a worker; poll `/api/import-jobs/{job_id}` for completion.
#[cfg_attr(feature = "openapi", utoipa::path(post, path = "/api/import.jsonl", request_body(content = String, content_type = "application/x-ndjson"), responses((status = 202, body = crate::dtos::jobs::QueuedJobResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "community"))]
pub(crate) async fn import_jsonl(
    State(ctx): State<AppContext>,
    authorized: Authorized<SchemaScope>,
    body: String,
) -> Result<(StatusCode, Json<QueuedJobResponse>), ApiError> {
    let job_id =
        crate::workers::import::enqueue(&ctx, authorized.txn(), &authorized.ctx, body).await?;
    authorized.commit().await?;
    Ok((StatusCode::ACCEPTED, Json(QueuedJobResponse { job_id })))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_import_jsonl),
        super::route_inventory::path_doc(__path_import_job_status),
    ]
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/api/import.jsonl", post(import_jsonl))
        .add("/api/import-jobs/{job_id}", get(import_job_status))
}

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/import-jobs/{job_id}", params(("job_id" = uuid::Uuid, Path)), responses((status = 200, body = crate::models::queue_job_lifecycles::BulkJobStatus), (status = 404, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["schema"]))), tag = "community"))]
async fn import_job_status(
    authorized: Authorized<SchemaScope>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<crate::models::queue_job_lifecycles::BulkJobStatus>, ApiError> {
    Ok(Json(
        crate::models::queue_job_lifecycles::bulk_job_status(
            authorized.txn(),
            authorized.ctx.workspace_id,
            id,
            &["import"],
        )
        .await?,
    ))
}
