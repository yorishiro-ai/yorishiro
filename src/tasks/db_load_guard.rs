//! Periodic check for database load that triggers automatic read-only mode.

use loco_rs::prelude::*;
use loco_rs::task::Vars;

use crate::services::db_load_guard;

pub struct DbLoadGuard;

#[async_trait]
impl Task for DbLoadGuard {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "db_load_guard".to_string(),
            detail: "Checks database load and enables automatic read-only when sustained above threshold (80% of max_connections)".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, _vars: &Vars) -> Result<()> {
        match db_load_guard::check_and_maybe_enable_readonly(app_context).await {
            Ok(()) => {
                println!("db_load_guard: check completed");
                Ok(())
            }
            Err(err) => {
                tracing::error!(error = %err, "db_load_guard: check failed");
                Err(err.into())
            }
        }
    }
}
