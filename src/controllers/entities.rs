use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, MigrationScope, ReadScope, WriteScope};
use crate::models::content_entities::{self, EntityRecord, UndoReport};
use crate::models::identity_api_key_audit_log;
use crate::workers::embedding_sync;
use crate::workers::reindex;

#[derive(Debug, Deserialize)]
pub struct CreateEntityRequest {
    pub schema_name: String,
    pub entity_type: String,
    pub data: Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateEntityRequest {
    pub data: Value,
}

#[derive(Debug, Deserialize)]
pub struct ListEntitiesParams {
    pub entity_type: Option<String>,
    /// JSON-encoded containment filter, e.g. `{"status":"active"}`.
    pub filter: Option<String>,
    /// Restricts results to entities created against this schema version.
    pub schema_version: Option<i32>,
    #[serde(flatten)]
    pub page: crate::controllers::PageParams,
}

pub async fn create_entity(
    State(ctx): State<AppContext>,
    authorized: Authorized<WriteScope>,
    Json(body): Json<CreateEntityRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let input = content_entities::CreateEntityInput {
        schema_name: body.schema_name,
        entity_type: body.entity_type,
        data: body.data,
    };
    let created_by = authorized.ctx.user_id;
    let record =
        content_entities::create(authorized.txn(), workspace_id, input, created_by).await?;
    authorized.commit().await?;
    embedding_sync::enqueue_after_write(&ctx, workspace_id, record.id).await;
    Ok((StatusCode::CREATED, Json(record)))
}

pub async fn get_entity(
    authorized: Authorized<ReadScope>,
    Path(id): Path<Uuid>,
) -> Result<Json<EntityRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let record = content_entities::get(authorized.txn(), workspace_id, id).await?;
    Ok(Json(record))
}

pub async fn update_entity(
    State(ctx): State<AppContext>,
    authorized: Authorized<WriteScope>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateEntityRequest>,
) -> Result<Json<EntityRecord>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let updated_by = authorized.ctx.user_id;
    let record =
        content_entities::update(authorized.txn(), workspace_id, id, body.data, updated_by).await?;
    authorized.commit().await?;
    embedding_sync::enqueue_after_write(&ctx, workspace_id, record.id).await;
    Ok(Json(record))
}

pub async fn delete_entity(
    authorized: Authorized<WriteScope>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    content_entities::delete(authorized.txn(), workspace_id, id).await?;
    authorized.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_entities(
    authorized: Authorized<ReadScope>,
    Query(params): Query<ListEntitiesParams>,
) -> Result<Json<Vec<EntityRecord>>, ApiError> {
    let query = content_entities::ListEntitiesQuery {
        entity_type: params.entity_type,
        filter: crate::controllers::parse_filter_param(params.filter)?,
        schema_version: params.schema_version,
        page: params.page.into(),
    };

    let workspace_id = authorized.ctx.workspace_id;
    let records = content_entities::list(authorized.txn(), workspace_id, query).await?;
    Ok(Json(records))
}

/// Puts every entity a job's snapshots cover back to what it held before the job overwrote it.
///
/// `MigrationScope`, not `WriteScope`: undoing a batch is a migration operation (the same scope that would gate the job that produced the snapshots, e.g. `ee/`'s fill-proposal confirmation), not an ordinary entity write.
pub async fn undo_migration_job(
    authorized: Authorized<MigrationScope>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<UndoReport>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;
    let report = content_entities::undo_job(authorized.txn(), workspace_id, job_id).await?;
    // Recorded on the same transaction as the undo itself, before commit: a rollback of the undo
    // must also roll back the record that it happened, or a failed request could still leave an
    // audit trail claiming it succeeded.
    identity_api_key_audit_log::record(
        authorized.txn(),
        identity_api_key_audit_log::AuditActor {
            workspace_id,
            tenant_id: authorized.ctx.tenant_id,
            api_key_id: authorized.ctx.api_key_id,
            user_id: authorized.ctx.user_id,
        },
        identity_api_key_audit_log::AuditAction::UndoMigrationJob,
        serde_json::json!({ "job_id": job_id, "restored": report.restored, "missing": report.missing }),
    )
    .await?;
    authorized.commit().await?;
    Ok(Json(report))
}

/// Enqueues a reindex job for this workspace.
///
/// `MigrationScope`, not `WriteScope`: reindex is a migration operation (replacing vectors from
/// an old model with a new one), and the same audit trail that gates `undo` applies here too.
///
/// `POST /api/migration-jobs/reindex` returns 202 immediately with the job ID; the actual
/// reindex runs asynchronously in a background worker, serialised by an advisory lock.
/// A second request for the same workspace while the first is still running enqueues another
/// job that blocks on the same lock; the REST API cannot reject the second without knowing
/// how long the first has left, and a reindex over a large workspace may legitimately take
/// a long time.
///
/// Returns 503 when no queue provider is configured: `perform_later` would silently return
/// a job ID while discarding the job, which is worse than no endpoint at all.
pub async fn reindex_workspace(
    State(ctx): State<AppContext>,
    authorized: Authorized<MigrationScope>,
) -> Result<Json<ReindexResponse>, ApiError> {
    let workspace_id = authorized.ctx.workspace_id;

    // Check queue provider before enqueue: a missing provider means `perform_later` would
    // return a job ID while silently discarding the job, which is worse than no endpoint
    // at all.  A 503 with a clear message lets the operator know this needs a queue config
    // rather than puzzling over a successful 202 with no actual work happening.
    if ctx.queue_provider.is_none() {
        return Err(ApiError(crate::error::YorishiroError::not_found(
            "reindex requires a queue provider (configure queue: in the server config)",
        )));
    }

    let job_id = reindex::enqueue_reindex(&ctx, workspace_id).await.map_err(|e| {
        ApiError(crate::error::YorishiroError::Internal(anyhow::anyhow!(e.to_string())))
    })?;

    // Recorded on the same RLS-scoped transaction as the request itself: this is a
    // migration-scope operation that the audit log must capture.
    identity_api_key_audit_log::record(
        authorized.txn(),
        identity_api_key_audit_log::AuditActor {
            workspace_id,
            tenant_id: authorized.ctx.tenant_id,
            api_key_id: authorized.ctx.api_key_id,
            user_id: authorized.ctx.user_id,
        },
        identity_api_key_audit_log::AuditAction::ReindexEmbeddings,
        serde_json::json!({ "job_id": job_id }),
    )
    .await?;
    authorized.commit().await?;

    Ok(Json(ReindexResponse { job_id }))
}

#[derive(Debug, Serialize)]
pub struct ReindexResponse {
    /// The job ID assigned by the queue provider.
    pub job_id: String,
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/entities")
        .add("/", post(create_entity))
        .add("/", get(list_entities))
        .add("/{id}", get(get_entity))
        .add("/{id}", put(update_entity))
        .add("/{id}", delete(delete_entity))
}

pub fn migration_routes() -> Routes {
    Routes::new()
        .prefix("api/migration-jobs")
        .add("/{job_id}/undo", post(undo_migration_job))
        .add("/reindex", post(reindex_workspace))
}
