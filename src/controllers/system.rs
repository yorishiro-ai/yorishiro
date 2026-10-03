//! Deployment-wide controls, as opposed to anything scoped to a tenant or a workspace.
//!
//! Only maintenance lives here.
//! It is deployment-wide by nature: one row in `system_maintenance` decides whether every caller is served, and there is no per-tenant version of it.

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::routing::{get, put};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, MigrationScope};
use crate::dtos::system::{MaintenanceResponse, SetMaintenanceRequest};
use crate::error::YorishiroError;
use crate::models::api_key_audit_log;
use crate::models::system_maintenance::{self, DEFAULT_RETRY_AFTER_SECONDS};

#[cfg_attr(feature = "openapi", utoipa::path(get, path = "/api/system/maintenance", responses((status = 200, body = crate::dtos::system::MaintenanceResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["migration"]))), tag = "community"))]
pub(crate) async fn get_maintenance(
    State(ctx): State<AppContext>,
    _authorized: Authorized<MigrationScope>,
) -> Result<Json<MaintenanceResponse>, ApiError> {
    let current = system_maintenance::get(&ctx.db).await?;
    Ok(Json(current.into()))
}

#[cfg_attr(feature = "openapi", utoipa::path(put, path = "/api/system/maintenance", request_body = crate::dtos::system::SetMaintenanceRequest, responses((status = 200, body = crate::dtos::system::MaintenanceResponse), (status = 401, body = super::openapi::ApiErrorBody), (status = 403, body = super::openapi::ApiErrorBody), (status = 422, body = super::openapi::ApiErrorBody)), security(("bearer_auth" = [])), extensions(("x-yorishiro-required-scopes" = json!(["migration"]))), tag = "community"))]
pub(crate) async fn set_maintenance(
    State(ctx): State<AppContext>,
    authorized: Authorized<MigrationScope>,
    body: Result<Json<SetMaintenanceRequest>, JsonRejection>,
) -> Result<Json<MaintenanceResponse>, ApiError> {
    let Json(body) = body.map_err(|err| {
        ApiError(YorishiroError::ValidationFailed {
            message: err.body_text(),
            details: vec![],
            hint: "one of: off, read-only, full-lock".into(),
        })
    })?;
    let mode = body.mode;
    let updated = system_maintenance::set(
        &ctx.db,
        body.mode,
        body.retry_after.unwrap_or(DEFAULT_RETRY_AFTER_SECONDS),
        body.reason,
    )
    .await?;

    // Recorded on ctx.db, the same migration-role connection the write itself just went through: authorized.txn() is an RLS-scoped transaction this handler never commits (get_maintenance's sibling extractor exists only to gate the scope check, not to hold a connection this deployment-wide write needs), so writing the audit row there would silently discard it the same way an uncommitted write anywhere else in this codebase does.
    api_key_audit_log::record(
        &ctx.db,
        api_key_audit_log::AuditActor {
            workspace_id: authorized.ctx.workspace_id,
            tenant_id: authorized.ctx.tenant_id,
            api_key_id: authorized.ctx.api_key_id,
            user_id: authorized.ctx.user_id,
        },
        api_key_audit_log::AuditAction::SetMaintenance,
        serde_json::json!({ "mode": mode.as_db_str(), "reason": updated.reason }),
    )
    .await?;

    Ok(Json(updated.into()))
}

#[cfg(feature = "openapi")]
pub(crate) fn openapi_docs() -> Vec<super::route_inventory::RouteDoc> {
    vec![
        super::route_inventory::path_doc(__path_get_maintenance),
        super::route_inventory::path_doc(__path_set_maintenance),
    ]
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/system")
        .add("/maintenance", get(get_maintenance))
        .add("/maintenance", put(set_maintenance))
}
