//! Cron entry point for tenant reindex scheduling.

use loco_rs::prelude::*;
use loco_rs::task::Vars;

pub struct TenantReindexScheduler;

#[async_trait]
impl Task for TenantReindexScheduler {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "tenant_reindex_scheduler".into(),
            detail: "Enqueue reindex jobs for scheduled tenants".into(),
        }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        self.run_at(ctx, vars, crate::data::time::now()).await
    }
}

impl TenantReindexScheduler {
    /// Runs the scheduler at the supplied instant.
    ///
    /// # Errors
    /// Returns database, ownership or dispatch failures.
    pub async fn run_at(
        &self,
        ctx: &AppContext,
        _vars: &Vars,
        tick: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        crate::run_reindex_schedule(ctx, tick).await
    }
}

pub(crate) async fn run_default(ctx: &AppContext) -> Result<()> {
    crate::workers::startup_reindex::run_metadata_scan(ctx).await
}
