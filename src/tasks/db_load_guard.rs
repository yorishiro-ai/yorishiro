//! Performs one opt-in database load check.

use loco_rs::prelude::*;
use loco_rs::task::Vars;

use crate::services::db_load_guard;

pub struct DbLoadGuard;

#[async_trait]
impl Task for DbLoadGuard {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "db_load_guard".to_string(),
            detail: "Checks database load for the automatic read-only guard".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, _vars: &Vars) -> Result<()> {
        match db_load_guard::check_once(app_context).await {
            Ok(()) => {
                println!("db_load_guard: check completed");
                Ok(())
            }
            Err(err) => {
                tracing::error!(error = %err, "db_load_guard: check failed");
                Err(err)
            }
        }
    }
}
