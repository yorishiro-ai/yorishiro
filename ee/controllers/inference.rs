//! Inferring values for fields an entity is missing, and the per-workspace credentials it runs on.
//!
//! This product does not pay for inference, so a workspace brings its own key.
//! A workspace with none configured gets a 422 rather than a fall back to `default` values: a caller who asked for inference and silently received defaults would have no way to tell that nothing was inferred.

use crate::controllers::ApiError;
use crate::controllers::middleware::auth::require_scope;
use crate::error::YorishiroError;
use crate::models::api_keys::ApiKeyScope;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::edition::ee::controllers::middleware::auth as authz;
use crate::edition::ee::dtos::inference::{
    InferFillResponse, InferFillStatus, InferJobStatusResponse, SetLlmKeyRequest,
};
use crate::edition::ee::models::inference_jobs;
use crate::edition::ee::models::inference_jobs::InferenceJobStatus;
use crate::edition::ee::models::inference_proposals;
use crate::edition::ee::models::workspace_llm_keys;

/// `POST /api/schemas/active/{name}/infer-fill`
///
/// Enqueues an async infer-fill job whose answers are stored as reviewable proposals.
/// Requires `inference_create` on the selected workspace in addition to schema scope.
/// The worker rechecks the credential and IAM policy before inference and proposal storage.
///
/// Poll for completion via `GET /api/inference-jobs/{job_id}`.
#[cfg(feature = "openapi")]
fn infer_fill_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/schemas/active/{name}/infer-fill",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["inference_create"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<InferFillResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "503": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "name", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}]}),
    )
}
async fn infer_fill(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<InferFillResponse>, ApiError> {
    // Gate before authentication so unlicensed requests all receive the same 404.
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    let job_id =
        crate::edition::ee::workers::infer_fill::enqueue_infer_fill(&ctx, &auth_ctx, name).await?;

    Ok(Json(InferFillResponse {
        job_id,
        status: InferFillStatus::Queued,
    }))
}

/// `GET /api/inference-jobs/{job_id}`
///
/// Poll for the durable result of an infer-fill job.
///
/// Requires authentication and read scope, and only returns a job belonging to the
/// authenticated workspace.
#[cfg(feature = "openapi")]
fn infer_job_status_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/inference-jobs/{job_id}",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["inference_job_get"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<InferJobStatusResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "job_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}]}),
    )
}
async fn infer_job_status(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<InferJobStatusResponse>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let parsed_job_id = uuid::Uuid::parse_str(&job_id)
        .map_err(|_| YorishiroError::not_found("infer-fill job not found"))?;
    let result = inference_jobs::get_as(&ctx.db, &auth_ctx, parsed_job_id).await?;

    let status = result.status;
    let completed = status == InferenceJobStatus::Completed;
    Ok(Json(InferJobStatusResponse {
        job_id,
        status,
        applied: completed.then_some(result.applied),
        proposed: completed.then_some(result.proposed),
        skipped: completed.then_some(result.skipped),
        error: result.error,
    }))
}

/// Managing the workspace's own LLM credentials.
///
/// Separate from [`gated_routes`] because the licence gate applies per `Routes` and these three are
/// not gated: storing a credential does not require a licence, only spending it on an inference call
/// does. Merging the two groups would extend the gate over these, which is a product decision rather
/// than a routing detail.
///
/// The two groups also sit under different prefixes: credentials belong with the workspace's other
/// settings, while the inference call itself extends the schema routes it fills against.
pub fn routes() -> Routes {
    Routes::new().prefix("api/workspace").add(
        "/llm-key",
        axum::routing::put(set_llm_key)
            .get(get_llm_key)
            .delete(delete_llm_key),
    )
}

/// The route this module's licence gate covers: the server calls an LLM here, which is what makes
/// it an enterprise feature. See [`routes`] for why the two groups are separate.
pub fn gated_routes() -> Routes {
    Routes::new()
        .prefix("api/schemas")
        .add("/active/{name}/infer-fill", axum::routing::post(infer_fill))
}

/// `GET /api/inference-jobs/{job_id}`
///
/// Poll for the result of an infer-fill job. Follows the same pattern as
/// `migration_routes()` (`api/migration-jobs/{job_id}/undo`): a dedicated prefix for
/// async-job polling, under the licence gate because results include workspace
/// activity counts that reveal whether a workspace has data to infer.
pub fn inference_job_status_routes() -> Routes {
    Routes::new()
        .prefix("api/inference-jobs")
        .add("/{job_id}", axum::routing::get(infer_job_status))
        .add("/{job_id}/proposals", axum::routing::get(list_proposals))
        .add("/{job_id}/confirm", axum::routing::post(confirm_proposals))
        .add("/{job_id}/reject", axum::routing::post(reject_proposals))
        .add("/{job_id}/discard", axum::routing::post(discard_proposals))
        .add(
            "/confirmations/{job_id}",
            axum::routing::get(confirmation_status),
        )
}

#[cfg(feature = "openapi")]
fn confirmation_status_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/inference-jobs/confirmations/{job_id}",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["inference_confirmation_get"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::models::queue_job_lifecycles::BulkJobStatus>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "job_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<uuid::Uuid>()}]}),
    )
}
async fn confirmation_status(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(job_id): Path<uuid::Uuid>,
) -> Result<Json<crate::models::queue_job_lifecycles::BulkJobStatus>, ApiError> {
    let auth = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth, ApiKeyScope::Schema)?;
    Ok(Json(
        inference_jobs::confirmation_status_as(&ctx.db, &auth, job_id).await?,
    ))
}

fn parse_job_id(job_id: &str) -> Result<uuid::Uuid, YorishiroError> {
    uuid::Uuid::parse_str(job_id).map_err(|_| YorishiroError::not_found("infer-fill job not found"))
}

/// `GET /api/inference-jobs/{job_id}/proposals`
#[cfg(feature = "openapi")]
fn list_proposals_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/inference-jobs/{job_id}/proposals",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["inference_proposal_list"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<Vec<crate::edition::ee::controllers::openapi::InferenceProposalResponse>>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "job_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}]}),
    )
}
async fn list_proposals(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<Vec<inference_proposals::ProposalRecord>>, ApiError> {
    let auth = authz::authenticate_workspace(&ctx, &headers).await?;
    Ok(Json(
        inference_proposals::for_job_as(&ctx.db, &auth, parse_job_id(&job_id)?).await?,
    ))
}

#[cfg(feature = "openapi")]
fn reject_proposals_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/inference-jobs/{job_id}/reject",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["inference_reject"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::edition::ee::controllers::openapi::ProposalActionResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "job_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}]}),
    )
}
async fn reject_proposals(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<inference_proposals::ProposalActionReport>, ApiError> {
    let auth = authz::authenticate_workspace(&ctx, &headers).await?;
    Ok(Json(
        inference_proposals::reject_as(&ctx.db, &auth, parse_job_id(&job_id)?).await?,
    ))
}

#[cfg(feature = "openapi")]
fn discard_proposals_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/inference-jobs/{job_id}/discard",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["inference_discard"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::edition::ee::controllers::openapi::ProposalActionResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "job_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}]}),
    )
}
async fn discard_proposals(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<inference_proposals::ProposalActionReport>, ApiError> {
    let auth = authz::authenticate_workspace(&ctx, &headers).await?;
    Ok(Json(
        inference_proposals::discard_as(&ctx.db, &auth, parse_job_id(&job_id)?).await?,
    ))
}

/// Enqueues confirmation with `inference_confirm` on the selected workspace and schema scope.
/// The worker rechecks the credential and IAM policy before applying proposals.
#[cfg(feature = "openapi")]
fn confirm_proposals_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/inference-jobs/{job_id}/confirm",
        axum::http::Method::POST,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["inference_confirm"]), "responses": {"202": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::dtos::jobs::QueuedJobResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "409": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}, "parameters": [{"name": "job_id", "in": "path", "required": true, "schema": crate::controllers::openapi::route_inventory::schema::<String>()}]}),
    )
}
async fn confirm_proposals(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<(StatusCode, Json<crate::dtos::jobs::QueuedJobResponse>), ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    let job_id = parse_job_id(&job_id)?;
    let job_id =
        crate::edition::ee::workers::confirm_proposals::enqueue(&ctx, &auth_ctx, job_id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(crate::dtos::jobs::QueuedJobResponse { job_id }),
    ))
}

/// `PUT /api/workspace/llm-key`
#[cfg(feature = "openapi")]
fn set_llm_key_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspace/llm-key",
        axum::http::Method::PUT,
        || serde_json::json!({"tags": ["enterprise"], "requestBody": {"required": true, "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::edition::ee::controllers::openapi::LlmKeyRequest>()}}}, "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["llm_provider_update"]), "responses": {"204": serde_json::json!({"description": "LLM credentials saved"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "422": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn set_llm_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(body): Json<SetLlmKeyRequest>,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    workspace_llm_keys::set_as(
        &ctx.db,
        &auth_ctx,
        &body.base_url,
        &body.model,
        &body.api_key,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/workspace/llm-key`
#[cfg(feature = "openapi")]
fn get_llm_key_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspace/llm-key",
        axum::http::Method::GET,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["read"]), "x-yorishiro-required-actions" :  serde_json::json!(["llm_provider_get"]), "responses": {"200": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::edition::ee::controllers::openapi::LlmKeyResponse>()}}}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "404": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn get_llm_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Json<workspace_llm_keys::LlmKeyDescription>, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Read)?;
    let described = workspace_llm_keys::describe_as(&ctx.db, &auth_ctx)
        .await?
        .ok_or_else(|| YorishiroError::not_found("no LLM credentials configured"))?;
    Ok(Json(described))
}

/// `DELETE /api/workspace/llm-key`
#[cfg(feature = "openapi")]
fn delete_llm_key_doc() -> crate::controllers::openapi::route_inventory::RouteDoc {
    crate::controllers::openapi::route_inventory::path_doc(
        "/api/workspace/llm-key",
        axum::http::Method::DELETE,
        || serde_json::json!({"tags": ["enterprise"], "security": [{"bearer_auth": []}], "x-yorishiro-required-scopes" :  serde_json::json!(["schema"]), "x-yorishiro-required-actions" :  serde_json::json!(["llm_provider_delete"]), "responses": {"204": serde_json::json!({"description": "LLM credentials removed"}), "401": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}}), "403": serde_json::json!({"description": "", "content": {"application/json": {"schema": crate::controllers::openapi::route_inventory::schema::<crate::controllers::openapi::ApiErrorBody>()}}})}}),
    )
}
async fn delete_llm_key(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let auth_ctx = authz::authenticate_workspace(&ctx, &headers).await?;
    require_scope(&auth_ctx, ApiKeyScope::Schema)?;
    workspace_llm_keys::clear_as(&ctx.db, &auth_ctx).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<crate::controllers::openapi::route_inventory::RouteDoc> {
    vec![set_llm_key_doc(), get_llm_key_doc(), delete_llm_key_doc()]
}

#[cfg(feature = "openapi")]
pub(crate) fn gated_openapi_docs() -> Vec<crate::controllers::openapi::route_inventory::RouteDoc> {
    vec![infer_fill_doc()]
}

#[cfg(feature = "openapi")]
pub(crate) fn job_status_openapi_docs()
-> Vec<crate::controllers::openapi::route_inventory::RouteDoc> {
    vec![
        infer_job_status_doc(),
        list_proposals_doc(),
        confirm_proposals_doc(),
        confirmation_status_doc(),
        reject_proposals_doc(),
        discard_proposals_doc(),
    ]
}
