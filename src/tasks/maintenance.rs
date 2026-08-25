use loco_rs::prelude::*;
use loco_rs::task::Vars;

use crate::models::identity_maintenance::{self, MaintenanceMode};

/// `cargo loco task maintenance mode:full_lock [retry_after:300] [reason:"upgrading"]`
///
/// `mode` is one of `off`/`read_only`/`full_lock`.
/// `read_only` refuses writes with 423; `full_lock` refuses everything with 503; `off` serves normally.
/// The state is shared by every node (one row in the database), and `/_ping`/`/_health`/`/_readiness` keep answering so an orchestrator does not restart a server that is deliberately paused (see `services::maintenance::always_served`).
pub struct Maintenance;

#[async_trait]
impl Task for Maintenance {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "maintenance".to_string(),
            detail: "Sets maintenance mode: cargo loco task maintenance mode:<off|read_only|full_lock> [retry_after:<secs>] [reason:<text>]".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let mode_str = vars.cli_arg("mode")?;
        let mode = MaintenanceMode::from_db_str(mode_str)
            .ok_or_else(|| Error::Message(format!("'{mode_str}' is not a maintenance mode")))?;
        let retry_after: u32 = match vars.cli_arg("retry_after") {
            Ok(raw) => raw
                .parse()
                .map_err(|_| Error::Message("retry_after is not a number".to_string()))?,
            Err(_) => 300,
        };
        let reason = vars.cli_arg("reason").ok().map(str::to_string);

        let state = identity_maintenance::set(&app_context.db, mode, retry_after, reason)
            .await
            .map_err(|err| Error::Message(err.to_string()))?;

        match state.mode {
            MaintenanceMode::Off => println!("maintenance off; serving normally"),
            MaintenanceMode::ReadOnly => println!(
                "maintenance read-only: writes refused with 423, Retry-After {}s",
                state.retry_after
            ),
            MaintenanceMode::FullLock => println!(
                "maintenance full lock: all requests refused with 503, Retry-After {}s \
                 (/_ping, /_health, /_readiness keep answering)",
                state.retry_after
            ),
        }
        if let Some(reason) = state.reason {
            println!("reason shown to callers: {reason}");
        }
        Ok(())
    }
}
