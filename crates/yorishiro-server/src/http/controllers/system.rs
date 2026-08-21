//! Deployment-wide controls, as opposed to anything scoped to a tenant or a workspace.
//!
//! Only maintenance lives here.
//! It is deployment-wide by nature: one row in `identity.maintenance` decides whether every caller is served, and there is no per-tenant version of it.

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use yorishiro_core::models::maintenance::{self, MaintenanceMode, MaintenanceState};

use crate::error::ApiError;
use crate::http::middleware::auth::{Authorized, MigrationScope};
use crate::state::AppState;

/// The state as the API reports it.
/// `MaintenanceState` is the repository's own type and is not serialisable, so the wire shape is declared here rather than deriving onto core's model: the two are free to differ, and this one is a published contract.
#[derive(Debug, Serialize, ToSchema)]
pub struct MaintenanceResponse {
    /// `off`, `read-only` or `full-lock`.
    pub mode: String,
    /// Seconds a refused caller is told to wait, sent as `Retry-After` on the refusal itself.
    pub retry_after: u32,
    /// Why, when whoever set it said.
    /// Absent when nothing was given.
    pub reason: Option<String>,
}

impl From<MaintenanceState> for MaintenanceResponse {
    fn from(state: MaintenanceState) -> Self {
        Self {
            mode: state.mode.as_db_str().to_string(),
            retry_after: state.retry_after,
            reason: state.reason,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetMaintenanceRequest {
    /// `off`, `read-only` or `full-lock`, spelled as the CLI spells them.
    pub mode: String,
    /// Defaults to the same 300 seconds `admin maintenance` uses.
    #[serde(default)]
    pub retry_after: Option<u32>,
    /// Shown to refused callers instead of the generic message.
    #[serde(default)]
    pub reason: Option<String>,
}

/// The CLI's default, repeated here so the two entry points do not drift.
const DEFAULT_RETRY_AFTER: u32 = 300;

/// Accepts what an operator types at the CLI.
/// `MaintenanceMode::from_db_str` parses the stored spelling (`read_only`), while clap renders the same variants kebab-cased (`read-only`), and an operator moving between the two entry points should not have to know which is which.
/// Both spellings are taken; anything else is refused rather than read as `off`.
pub(crate) fn parse_mode(value: &str) -> Option<MaintenanceMode> {
    match value {
        "off" => Some(MaintenanceMode::Off),
        "read-only" | "read_only" => Some(MaintenanceMode::ReadOnly),
        "full-lock" | "full_lock" => Some(MaintenanceMode::FullLock),
        _ => None,
    }
}

#[utoipa::path(
    get,
    path = "/api/system/maintenance",
    responses(
        (status = 200, description = "The current state", body = MaintenanceResponse),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
    ),
    tag = "system",
)]
pub async fn get_maintenance(
    State(state): State<AppState>,
    _authorized: Authorized<MigrationScope>,
) -> Result<Json<MaintenanceResponse>, ApiError> {
    let mut conn = state
        .identity_pool()?
        .acquire()
        .await
        .map_err(|err| ApiError::from(yorishiro_core::YorishiroError::Internal(err.into())))?;
    let current = maintenance::get(&mut *conn).await?;
    Ok(Json(current.into()))
}

#[utoipa::path(
    put,
    path = "/api/system/maintenance",
    request_body = SetMaintenanceRequest,
    responses(
        (status = 200, description = "The state as it now stands", body = MaintenanceResponse),
        (status = 401, description = "Invalid or missing credentials", body = crate::error::ApiErrorBody),
        (status = 403, description = "Insufficient scope", body = crate::error::ApiErrorBody),
        (status = 422, description = "Unknown mode", body = crate::error::ApiErrorBody),
    ),
    tag = "system",
)]
pub async fn set_maintenance(
    State(state): State<AppState>,
    _authorized: Authorized<MigrationScope>,
    Json(body): Json<SetMaintenanceRequest>,
) -> Result<Json<MaintenanceResponse>, ApiError> {
    let mode =
        parse_mode(&body.mode).ok_or_else(|| yorishiro_core::YorishiroError::ValidationFailed {
            message: format!("unknown maintenance mode '{}'", body.mode),
            details: vec![],
            hint: "one of: off, read-only, full-lock".into(),
        })?;

    // `maintenance::set` takes the pool rather than the request's connection because the request role has SELECT only on this table: entering maintenance is an operator action, and the GRANT says so.
    // Authorization has already happened above, in the extractor, and the pool is handed straight to the repository rather than used to compose anything here.
    let updated = maintenance::set(
        state.identity_pool()?,
        mode,
        body.retry_after.unwrap_or(DEFAULT_RETRY_AFTER),
        body.reason,
    )
    .await?;

    Ok(Json(updated.into()))
}

#[cfg(test)]
#[path = "../../../tests/http/controllers/system.rs"]
mod tests;
