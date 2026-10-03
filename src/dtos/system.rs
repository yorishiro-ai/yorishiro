use crate::models::system_maintenance::{MaintenanceMode, MaintenanceState};
use serde::{Deserialize, Serialize};

/// The maintenance state as the API reports it.
//
// `MaintenanceState` is the repository's own type and is not serialisable as-is (it holds `MaintenanceMode`, not a plain string), so the wire shape is declared here rather than reused directly.
#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MaintenanceResponse {
    /// `off`, `read-only` or `full-lock`.
    #[cfg_attr(feature = "openapi", schema(value_type = MaintenanceMode))]
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

/// Accepts the CLI spellings (`read-only`, `full-lock`) as well as the stored ones, which the model's own `Deserialize` deliberately does not.
fn deserialize_maintenance_mode<'de, D>(deserializer: D) -> Result<MaintenanceMode, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    value.parse().map_err(serde::de::Error::custom)
}

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetMaintenanceRequest {
    /// `off`, `read-only` or `full-lock`, spelled as the CLI spells them.
    #[serde(deserialize_with = "deserialize_maintenance_mode")]
    pub mode: MaintenanceMode,
    /// Defaults to the same 300 seconds the CLI uses.
    #[serde(default)]
    pub retry_after: Option<u32>,
    /// Shown to refused callers instead of the generic message.
    #[serde(default)]
    pub reason: Option<String>,
}
