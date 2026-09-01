//! Deployment-wide controls, as opposed to anything scoped to a tenant or a workspace.
//!
//! Only maintenance lives here.
//! It is deployment-wide by nature: one row in `identity_maintenance` decides whether every caller is served, and there is no per-tenant version of it.

use axum::Json;
use axum::extract::State;
use axum::routing::{get, put};
use loco_rs::app::AppContext;
use loco_rs::controller::Routes;
use serde::{Deserialize, Serialize};

use crate::controllers::ApiError;
use crate::controllers::extractors::{Authorized, MigrationScope};
use crate::error::YorishiroError;
use crate::models::identity_api_key_audit_log;
use crate::models::identity_maintenance::{self, MaintenanceMode, MaintenanceState};

/// The state as the API reports it.
/// `MaintenanceState` is the repository's own type and is not serialisable as-is (it holds `MaintenanceMode`, not a plain string), so the wire shape is declared here rather than reused directly.
#[derive(Serialize)]
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

#[derive(Deserialize)]
pub struct SetMaintenanceRequest {
    /// `off`, `read-only` or `full-lock`, spelled as the CLI spells them.
    pub mode: String,
    /// Defaults to the same 300 seconds the CLI uses.
    #[serde(default)]
    pub retry_after: Option<u32>,
    /// Shown to refused callers instead of the generic message.
    #[serde(default)]
    pub reason: Option<String>,
}

const DEFAULT_RETRY_AFTER: u32 = 300;

/// Accepts what an operator types at the CLI.
/// `MaintenanceMode::from_db_str` parses the stored spelling (`read_only`), while an operator typing at this endpoint spells it kebab-cased (`read-only`), same as clap would render it.
/// Both spellings are taken; anything else is refused rather than read as `off`.
pub(crate) fn parse_mode(value: &str) -> Option<MaintenanceMode> {
    match value {
        "off" => Some(MaintenanceMode::Off),
        "read-only" | "read_only" => Some(MaintenanceMode::ReadOnly),
        "full-lock" | "full_lock" => Some(MaintenanceMode::FullLock),
        _ => None,
    }
}

pub async fn get_maintenance(
    State(ctx): State<AppContext>,
    _authorized: Authorized<MigrationScope>,
) -> Result<Json<MaintenanceResponse>, ApiError> {
    let current = identity_maintenance::get(&ctx.db).await?;
    Ok(Json(current.into()))
}

pub async fn set_maintenance(
    State(ctx): State<AppContext>,
    authorized: Authorized<MigrationScope>,
    Json(body): Json<SetMaintenanceRequest>,
) -> Result<Json<MaintenanceResponse>, ApiError> {
    let mode = parse_mode(&body.mode).ok_or_else(|| YorishiroError::ValidationFailed {
        message: format!("unknown maintenance mode '{}'", body.mode),
        details: vec![],
        hint: "one of: off, read-only, full-lock".into(),
    })?;

    let updated = identity_maintenance::set(
        &ctx.db,
        mode,
        body.retry_after.unwrap_or(DEFAULT_RETRY_AFTER),
        body.reason,
    )
    .await?;

    // Recorded on ctx.db, the same migration-role connection the write itself just went through: authorized.txn() is an RLS-scoped transaction this handler never commits (get_maintenance's sibling extractor exists only to gate the scope check, not to hold a connection this deployment-wide write needs), so writing the audit row there would silently discard it the same way an uncommitted write anywhere else in this codebase does.
    identity_api_key_audit_log::record(
        &ctx.db,
        identity_api_key_audit_log::AuditActor {
            workspace_id: authorized.ctx.workspace_id,
            tenant_id: authorized.ctx.tenant_id,
            api_key_id: authorized.ctx.api_key_id,
            user_id: authorized.ctx.user_id,
        },
        identity_api_key_audit_log::AuditAction::SetMaintenance,
        serde_json::json!({ "mode": mode.as_db_str(), "reason": updated.reason }),
    )
    .await?;

    Ok(Json(updated.into()))
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("api/system")
        .add("/maintenance", get(get_maintenance))
        .add("/maintenance", put(set_maintenance))
}
