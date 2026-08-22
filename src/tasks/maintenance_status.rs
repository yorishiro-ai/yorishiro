use loco_rs::prelude::*;
use loco_rs::task::Vars;

use crate::models::identity_maintenance;

/// `cargo loco task maintenance_status`
pub struct MaintenanceStatus;

#[async_trait]
impl Task for MaintenanceStatus {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "maintenance_status".to_string(),
            detail: "Shows the current maintenance state: cargo loco task maintenance_status"
                .to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, _vars: &Vars) -> Result<()> {
        let state = identity_maintenance::get(&app_context.db)
            .await
            .map_err(|err| Error::Message(err.to_string()))?;

        println!(
            "mode={} retry_after={}s reason={}",
            state.mode.as_db_str(),
            state.retry_after,
            state.reason.as_deref().unwrap_or("(none)"),
        );
        Ok(())
    }
}
