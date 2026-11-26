use crate::controllers::Accepted;
use axum::extract::Path;
use axum::{Json, extract::State};
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::extractors::Verified;
use crate::controllers::openapi::Routes;
use crate::dtos::jobs::{ImportJobAccepted, ImportJobStatus};

/// Line-delimited JSON import in the same format `GET /api/export` produces: one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line.
/// The worker applies every record in one transaction.
/// Poll `/api/import-jobs/{import_job_id}` for completion.
pub(crate) async fn import_jsonl(
    State(ctx): State<AppContext>,
    verified: Verified,
    body: String,
) -> Result<Accepted<ImportJobAccepted>, ApiError> {
    let import_job_id = crate::workers::import::enqueue_for_actor(&ctx, verified.ctx, body).await?;
    Ok(Accepted(ImportJobAccepted { import_job_id }))
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .add(
            "/api/import-jobs",
            aide::axum::routing::post_with(import_jsonl, |op| op.id("CreateImportJob")),
        )
        .add(
            "/api/import-jobs/{import_job_id}",
            aide::axum::routing::get_with(import_job_status, |op| op.id("GetImportJob")),
        )
}

async fn import_job_status(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::ImportJobIdPath { import_job_id }): Path<
        crate::dtos::common::ImportJobIdPath,
    >,
) -> Result<Json<ImportJobStatus>, ApiError> {
    Ok(Json(ImportJobStatus {
        import_job_id,
        status: crate::models::queue_job_lifecycles::import_job_status_for_actor(
            &ctx,
            verified.ctx,
            import_job_id,
        )
        .await?,
    }))
}
