//! REST routes for entities, plus the migration-job routes (undo, status, reindex and fill-defaults) that operate on them.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use uuid::Uuid;

use crate::controllers::ApiError;
use crate::controllers::extractors::{
    AuthContext, Authorized, MigrationScope, Verified, WriteScope,
};
use crate::dtos::entities::{
    CreateEntityRequest, FillDefaultsRequest, FillDefaultsResponse, ListEntitiesParams,
    ReindexResponse, UndoResponse, UpdateEntityRequest,
};
use crate::models::entity_entities::{self, EntityRecord};
use crate::workers::reindex;

#[cfg(feature = "openapi")]
fn create_entity_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/entities",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<CreateEntityRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["write"]), "x-yorishiro-required-actions" :  serde_json::json!(["entity_create"] ), "responses": {"201": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<EntityRecord>()}}}), "400": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
pub(crate) async fn create_entity(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Json(body): Json<CreateEntityRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let record = entity_entities::create_for_actor(&ctx, &verified.ctx, body.into()).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[cfg(feature = "openapi")]
fn get_entity_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/entities/{id}",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["entity_get"] ), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<EntityRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn get_entity(
    State(ctx): State<AppContext>,
    AuthContext(actor): AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<EntityRecord>, ApiError> {
    let record = entity_entities::get_for_actor(&ctx, &actor, id).await?;
    Ok(Json(record))
}

#[cfg(feature = "openapi")]
fn update_entity_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/entities/{id}",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<UpdateEntityRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["write"]), "x-yorishiro-required-actions" :  serde_json::json!(["entity_update"] ), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<EntityRecord>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "409": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
pub(crate) async fn update_entity(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Path(id): Path<Uuid>,
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

#[cfg(feature = "openapi")]
fn delete_entity_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/entities/{id}",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["write"]), "x-yorishiro-required-actions" :  serde_json::json!(["entity_delete"] ), "responses": {"204": serde_json::json!({"description": "Entity deleted"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
pub(crate) async fn delete_entity(
    State(ctx): State<AppContext>,
    verified: Verified<WriteScope>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    entity_entities::delete_for_actor(&ctx, &verified.ctx, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
fn list_entities_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/entities",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["entity_list"] ), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<EntityRecord>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "entity_type", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}, {"name": "filter", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}, {"name": "schema_version", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}, {"name": "page", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}, {"name": "page_size", "in": "query", "required": false, "schema": crate::controllers::openapi::route_inventory::schema::<i32>()}]}),
    )
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
#[cfg(feature = "openapi")]
fn undo_migration_job_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/migration-jobs/{job_id}/undo",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["migration"]), "x-yorishiro-required-actions" :  serde_json::json!(["migration_undo"]), "responses": {"202": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<UndoResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "job_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
pub(crate) async fn undo_migration_job(
    State(ctx): State<AppContext>,
    verified: Verified<MigrationScope>,
    Path(job_id): Path<Uuid>,
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

#[cfg(feature = "openapi")]
fn migration_job_status_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/migration-jobs/{job_id}",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["migration"]), "x-yorishiro-required-actions" :  serde_json::json!(["migration_job_get"] ), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::models::queue_job_lifecycles::BulkJobStatus>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "job_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<Uuid>()}]}),
    )
}
pub(crate) async fn migration_job_status(
    State(ctx): State<AppContext>,
    verified: Verified<MigrationScope>,
    Path(job_id): Path<Uuid>,
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
#[cfg(feature = "openapi")]
fn reindex_workspace_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/migration-jobs/reindex",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["migration"]), "x-yorishiro-required-actions" :  serde_json::json!(["workspace_reindex"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<ReindexResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "503": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
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
#[cfg(feature = "openapi")]
fn fill_defaults_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/migration-jobs/fill-defaults",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["community"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<FillDefaultsRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["migration"]), "x-yorishiro-required-actions" :  serde_json::json!(["migration_fill_defaults"]), "responses": {"202": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<FillDefaultsResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<super::openapi::ApiErrorBody>()}}})}}),
    )
}
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
        .add("/", post(create_entity))
        .add("/", get(list_entities))
        .add("/{id}", get(get_entity))
        .add("/{id}", put(update_entity))
        .add("/{id}", delete(delete_entity))
}

pub fn migration_routes() -> Routes {
    Routes::new()
        .prefix("api/migration-jobs")
        .add("/{job_id}", get(migration_job_status))
        .add("/{job_id}/undo", post(undo_migration_job))
        .add("/reindex", post(reindex_workspace))
        .add("/fill-defaults", post(fill_defaults))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::openapi::route_inventory::RouteDoc> {
    vec![
        create_entity_doc(),
        get_entity_doc(),
        update_entity_doc(),
        delete_entity_doc(),
        list_entities_doc(),
        undo_migration_job_doc(),
        migration_job_status_doc(),
        reindex_workspace_doc(),
        fill_defaults_doc(),
    ]
}
