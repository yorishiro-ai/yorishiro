use axum::extract::Path;
use axum::http::StatusCode;
use axum::{Json, extract::State};
use loco_rs::app::AppContext;

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, SchemaScope, Verified};
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

pub fn routes() -> Routes {
    Routes::new()
        .add(
            "/api/import.jsonl",
            aide::axum::routing::post_with(import_jsonl, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["schema"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["import_create"]),
                    ))
                    .response::<202, Json<QueuedJobResponse>>()
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<422, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/api/import-jobs/{job_id}",
            aide::axum::routing::get_with(import_job_status, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["schema"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["import_job_get"]),
                    ))
                    .response::<403, Json<super::openapi::ApiErrorBody>>()
                    .response::<404, Json<super::openapi::ApiErrorBody>>()
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
