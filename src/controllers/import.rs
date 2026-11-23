use crate::controllers::Accepted;
use axum::extract::Path;
use axum::{Json, extract::State};
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::extractors::{SchemaScope, Verified};
use crate::controllers::openapi::Routes;
use crate::dtos::jobs::QueuedJobResponse;

/// Line-delimited JSON import in the same format `GET /api/export.jsonl` produces: one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line.
/// Requires `SchemaScope` (rather than `WriteScope`) since an import can create schemas, which is itself a schema-scope-only operation elsewhere in the API.
///
/// Accepted imports run all-or-nothing on a worker; poll `/api/import-jobs/{job_id}` for completion.
pub(crate) async fn import_jsonl(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    body: String,
) -> Result<Accepted<QueuedJobResponse>, ApiError> {
    let job_id = crate::workers::import::enqueue_for_actor(&ctx, verified.ctx, body).await?;
    Ok(Accepted(QueuedJobResponse { job_id }))
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .add(
            "/api/import.jsonl",
            aide::axum::routing::post_with(import_jsonl, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["import_create"]),
                ))
            }),
        )
        .add(
            "/api/import-jobs/{job_id}",
            aide::axum::routing::get_with(import_job_status, |op| {
                op.with(crate::controllers::openapi::extension(
                    "x-yorishiro-required-actions",
                    serde_json::json!(["import_job_get"]),
                ))
            }),
        )
}

async fn import_job_status(
    State(ctx): State<AppContext>,
    verified: Verified<SchemaScope>,
    Path(crate::dtos::common::JobIdPath { job_id: id }): Path<
        crate::dtos::common::JobIdPath<uuid::Uuid>,
    >,
) -> Result<Json<crate::models::queue_job_lifecycles::BulkJobStatus>, ApiError> {
    Ok(Json(
        crate::models::queue_job_lifecycles::import_job_status_for_actor(&ctx, verified.ctx, id)
            .await?,
    ))
}
