//! REST routes for entities, plus the migration-job routes (undo, status, reindex and fill-defaults) that operate on them.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{
    AuthContext, Authorized, MigrationScope, Verified, WriteScope,
};
use crate::controllers::openapi::Routes;
use crate::dtos::entities::{
    CreateEntityRequest, FillDefaultsRequest, FillDefaultsResponse, ListEntitiesParams,
    ReindexResponse, UndoResponse, UpdateEntityRequest,
};
use crate::models::entity_entities::{self, EntityRecord};
use crate::workers::reindex;

pub(crate) async fn create_entity(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Json(body): Json<CreateEntityRequest>,
) -> Result<(StatusCode, Json<EntityRecord>), ApiError> {
    let record = entity_entities::create_for_actor(&ctx, &verified.ctx, body.into()).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn get_entity(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
) -> Result<Json<EntityRecord>, ApiError> {
    let record = entity_entities::get_for_actor(&ctx, &actor, id).await?;
    Ok(Json(record))
}

pub(crate) async fn update_entity(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
    Json(body): Json<UpdateEntityRequest>,
) -> Result<Json<EntityRecord>, ApiError> {
    let actor = verified.ctx;
    let record = entity_entities::update_for_actor(
        &ctx,
        &actor,
        entity_entities::UpdateEntityInput {
            id,
            data: body.data,
            updated_by: actor.user_id,
            expected_updated_at: body.expected_updated_at,
        },
    )
    .await?;
    Ok(Json(record))
}

pub(crate) async fn delete_entity(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Path(crate::dtos::common::IdPath { id }): Path<crate::dtos::common::IdPath<Uuid>>,
) -> Result<StatusCode, ApiError> {
    entity_entities::delete_for_actor(&ctx, &verified.ctx, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn list_entities(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Query(params): Query<ListEntitiesParams>,
) -> Result<Json<Vec<EntityRecord>>, ApiError> {
    let query = params.try_into()?;

    let records = entity_entities::list_for_actor(&ctx, &actor, query).await?;
    Ok(Json(records))
}

/// Puts every entity a job's snapshots cover back to what it held before the job overwrote it.
///
/// `MigrationScope`, not `WriteScope`: undoing a batch is a migration operation (the same scope that would gate the job that produced the snapshots, e.g. confirming a proposal that writes entities), not an ordinary entity write.
pub(crate) async fn undo_migration_job(
    State(ctx): State<AppContext>,
    verified: Verified<MigrationScope>,
    Path(crate::dtos::common::JobIdPath { job_id }): Path<crate::dtos::common::JobIdPath<Uuid>>,
) -> Result<(StatusCode, Json<UndoResponse>), ApiError> {
    let authorized = Authorized::<MigrationScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::SnapshotJob(job_id),
    )
    .await?;
    let queued_id =
        crate::workers::undo::enqueue(&ctx, authorized.txn(), &authorized.ctx, job_id).await?;
    authorized.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(UndoResponse {
            job_id: queued_id,
            snapshot_job_id: job_id,
        }),
    ))
}

pub(crate) async fn migration_job_status(
    State(ctx): State<AppContext>,
    verified: Verified<MigrationScope>,
    Path(crate::dtos::common::JobIdPath { job_id }): Path<crate::dtos::common::JobIdPath<Uuid>>,
) -> Result<Json<crate::models::queue_job_lifecycles::BulkJobStatus>, ApiError> {
    let mut authorized = Authorized::<MigrationScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Job(job_id),
    )
    .await?;
    authorized
        .require_action(crate::models::iam::policy::Action::MigrationJobGet)
        .await?;
    Ok(Json(
        crate::models::queue_job_lifecycles::migration_job_status(
            authorized.txn(),
            authorized.ctx.require_workspace()?,
            job_id,
        )
        .await?,
    ))
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
pub(crate) async fn reindex_workspace(
    State(ctx): State<AppContext>,
    verified: Verified<MigrationScope>,
) -> Result<Json<ReindexResponse>, ApiError> {
    let authorized = Authorized::<MigrationScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::WorkspaceReindex,
        ),
    )
    .await?;
    let job_id = reindex::enqueue_requested(&ctx, authorized.txn(), &authorized.ctx).await?;
    authorized.commit().await?;

    Ok(Json(ReindexResponse { job_id }))
}

/// Fills absent required fields in entities that fall behind the active schema version.
///
/// Accepts a durable fill-defaults job and returns its ID before migration starts.
/// The worker records completed counts in the audit log and uses this ID for undo snapshots.
pub(crate) async fn fill_defaults(
    State(ctx): State<AppContext>,
    verified: Verified<MigrationScope>,
    Json(body): Json<FillDefaultsRequest>,
) -> Result<(StatusCode, Json<FillDefaultsResponse>), ApiError> {
    let authorized = Authorized::<MigrationScope>::for_target(
        &ctx,
        verified.ctx,
        crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::MigrationFillDefaults,
        ),
    )
    .await?;
    let job_id = crate::workers::fill_defaults::enqueue(
        &ctx,
        authorized.txn(),
        &authorized.ctx,
        body.schema_name.clone(),
    )
    .await?;
    authorized.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(FillDefaultsResponse {
            schema_name: body.schema_name,
            job_id,
        }),
    ))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/entities")
        .add(
            "/",
            aide::axum::routing::post_with(create_entity, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["write"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["entity_create"]),
                    ))
                    .response::<201, Json<EntityRecord>>()
                    .response::<400, Json<super::openapi::ApiErrorBody>>()
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<422, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/",
            aide::axum::routing::get_with(list_entities, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["read"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["entity_list"]),
                    ))
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<422, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::get_with(get_entity, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["read"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["entity_get"]),
                    ))
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<404, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::put_with(update_entity, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["write"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["entity_update"]),
                    ))
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<404, Json<super::openapi::ApiErrorBody>>()
                    .response::<409, Json<super::openapi::ApiErrorBody>>()
                    .response::<422, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{id}",
            aide::axum::routing::delete_with(delete_entity, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["write"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["entity_delete"]),
                    ))
                    .response_with::<204, (), _>(|op| op.description("Entity deleted"))
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<404, Json<super::openapi::ApiErrorBody>>()
            }),
        )
}

pub fn migration_routes() -> Routes {
    Routes::new()
        .prefix("api/migration-jobs")
        .add(
            "/{job_id}",
            aide::axum::routing::get_with(migration_job_status, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["migration"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["migration_job_get"]),
                    ))
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<404, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/{job_id}/undo",
            aide::axum::routing::post_with(undo_migration_job, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["migration"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["migration_undo"]),
                    ))
                    .response::<202, Json<UndoResponse>>()
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<404, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/reindex",
            aide::axum::routing::post_with(reindex_workspace, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["migration"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["workspace_reindex"]),
                    ))
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<503, Json<super::openapi::ApiErrorBody>>()
            }),
        )
        .add(
            "/fill-defaults",
            aide::axum::routing::post_with(fill_defaults, |op| {
                op.tag("community")
                    .security_requirement("bearer_auth")
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-scopes",
                        serde_json::json!(["migration"]),
                    ))
                    .with(crate::controllers::openapi::extension(
                        "x-yorishiro-required-actions",
                        serde_json::json!(["migration_fill_defaults"]),
                    ))
                    .response::<202, Json<FillDefaultsResponse>>()
                    .response::<401, Json<super::openapi::ApiErrorBody>>()
                    .response::<422, Json<super::openapi::ApiErrorBody>>()
            }),
        )
}
