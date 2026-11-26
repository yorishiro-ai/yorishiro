//! REST routes for entities, plus the migration-job routes (undo, status, reindex and fill-defaults) that operate on them.

use crate::controllers::{Accepted, Created};
use axum::Json;
use axum::extract::{Path, Query, State};
use loco_rs::app::AppContext;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{AuthContext, Verified};
use crate::controllers::openapi::Routes;
use crate::dtos::entities::{
    CreateEntityRequest, FillDefaultsRequest, FillDefaultsResponse, ListEntitiesParams,
    ReindexResponse, UndoResponse, UpdateEntityRequest,
};
use crate::models::entity_entities::{self, EntityRecord};
use crate::workers::reindex;

pub(crate) async fn create_entity(
    State(ctx): State<AppContext>,
    verified: Verified,
    Json(body): Json<CreateEntityRequest>,
) -> Result<Created<EntityRecord>, ApiError> {
    let record = entity_entities::create_for_actor(&ctx, &verified.ctx, body.into()).await?;
    Ok(Created(record))
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn get_entity(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(crate::dtos::common::EntityIdPath { entity_id }): Path<
        crate::dtos::common::EntityIdPath<Uuid>,
    >,
) -> Result<Json<EntityRecord>, ApiError> {
    let record = entity_entities::get_for_actor(&ctx, &actor, entity_id).await?;
    Ok(Json(record))
}

pub(crate) async fn update_entity(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::EntityIdPath { entity_id }): Path<
        crate::dtos::common::EntityIdPath<Uuid>,
    >,
    Json(body): Json<UpdateEntityRequest>,
) -> Result<Json<EntityRecord>, ApiError> {
    let actor = verified.ctx;
    let record = entity_entities::update_for_actor(
        &ctx,
        &actor,
        entity_entities::UpdateEntityInput {
            id: entity_id,
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
    verified: Verified,
    Path(crate::dtos::common::EntityIdPath { entity_id }): Path<
        crate::dtos::common::EntityIdPath<Uuid>,
    >,
) -> Result<axum::response::NoContent, ApiError> {
    entity_entities::delete_for_actor(&ctx, &verified.ctx, entity_id).await?;
    Ok(axum::response::NoContent)
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
/// Undoing a batch is a migration operation, authorized by its own IAM action rather than an ordinary entity write.
pub(crate) async fn undo_migration_job(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::MigrationJobIdPath { migration_job_id }): Path<
        crate::dtos::common::MigrationJobIdPath<String>,
    >,
) -> Result<Accepted<UndoResponse>, ApiError> {
    let migration_job_id = super::action_resource_id(&migration_job_id, ":undo")?;
    let queued_id =
        crate::workers::undo::enqueue_for_actor(&ctx, verified.ctx, migration_job_id).await?;
    Ok(Accepted(UndoResponse {
        migration_job_id: queued_id,
        snapshot_job_id: migration_job_id,
    }))
}

pub(crate) async fn migration_job_status(
    State(ctx): State<AppContext>,
    verified: Verified,
    Path(crate::dtos::common::MigrationJobIdPath { migration_job_id }): Path<
        crate::dtos::common::MigrationJobIdPath,
    >,
) -> Result<Json<crate::dtos::jobs::MigrationJobStatus>, ApiError> {
    Ok(Json(crate::dtos::jobs::MigrationJobStatus {
        migration_job_id,
        status: crate::models::queue_job_lifecycles::migration_job_status_for_actor(
            &ctx,
            verified.ctx,
            migration_job_id,
        )
        .await?,
    }))
}

/// Enqueues a reindex job for this workspace.
///
/// Reindex is a migration operation (replacing vectors from an old model with a new one), authorized by its own IAM action.
///
/// `POST /api/migration-jobs/reindex` returns 202 with the job ID, and a background worker reindexes under an advisory lock.
/// A second request for the same workspace while the first runs enqueues another job that waits on the same lock.
///
/// Returns 503 when no queue provider is configured, because `perform_later` would otherwise return a job ID and discard the job.
pub(crate) async fn reindex_workspace(
    State(ctx): State<AppContext>,
    verified: Verified,
) -> Result<Json<ReindexResponse>, ApiError> {
    let job_id = reindex::enqueue_requested_for_actor(&ctx, verified.ctx).await?;
    Ok(Json(ReindexResponse {
        migration_job_id: job_id,
    }))
}

/// Fills absent required fields in entities that fall behind the active schema version.
///
/// Accepts a durable fill-defaults job and returns its ID before migration starts.
/// The worker records completed counts in the audit log and uses this ID for undo snapshots.
pub(crate) async fn fill_defaults(
    State(ctx): State<AppContext>,
    verified: Verified,
    Json(body): Json<FillDefaultsRequest>,
) -> Result<Accepted<FillDefaultsResponse>, ApiError> {
    let job_id = crate::workers::fill_defaults::enqueue_for_actor(
        &ctx,
        verified.ctx,
        body.schema_name.clone(),
    )
    .await?;
    Ok(Accepted(FillDefaultsResponse {
        schema_name: body.schema_name,
        migration_job_id: job_id,
    }))
}

pub fn routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/entities")
        .add(
            "/",
            aide::axum::routing::post_with(create_entity, |op| op.id("CreateEntity")),
        )
        .add(
            "/",
            aide::axum::routing::get_with(list_entities, |op| op.id("ListEntities")),
        )
        .add(
            "/{entity_id}",
            aide::axum::routing::get_with(get_entity, |op| op.id("GetEntity")),
        )
        .add(
            "/{entity_id}",
            aide::axum::routing::put_with(update_entity, |op| op.id("UpdateEntity")),
        )
        .add(
            "/{entity_id}",
            aide::axum::routing::delete_with(delete_entity, |op| op.id("DeleteEntity")),
        )
}

pub fn migration_routes() -> Routes {
    Routes::new()
        .tag("community")
        .prefix("api/migration-jobs")
        .add(
            "/{migration_job_id}",
            aide::axum::routing::get_with(migration_job_status, |op| op.id("GetMigrationJob")),
        )
        .add_action(
            "/{migration_job_id}",
            ":undo",
            aide::axum::routing::post_with(undo_migration_job, |op| op.id("UndoMigrationJob")),
        )
        .add(
            "/reindex",
            aide::axum::routing::post_with(reindex_workspace, |op| op.id("ReindexWorkspace")),
        )
        .add(
            "/fill-defaults",
            aide::axum::routing::post_with(fill_defaults, |op| op.id("FillEntityDefaults")),
        )
}
