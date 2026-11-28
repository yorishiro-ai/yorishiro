use loco_rs::prelude::*;
use loco_rs::task::Vars;

pub struct PurgeAuditEvents;

#[async_trait]
impl Task for PurgeAuditEvents {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "purge_audit_events".into(),
            detail: "Delete API trail rows older than the configured retention".into(),
        }
    }

    async fn run(&self, ctx: &AppContext, _vars: &Vars) -> Result<()> {
        let days = crate::data::settings::configured(ctx)?
            .limits
            .audit_retention_days;
        let cutoff =
            (crate::data::time::now() - chrono::Duration::days(i64::from(days))).fixed_offset();
        let deleted = crate::models::api_events::purge_before(&ctx.db, cutoff).await?;
        // A body whose import never completed is kept for a week, then removed.
        let abandoned = crate::models::import_staging::purge_before(
            &ctx.db,
            (crate::data::time::now() - chrono::Duration::days(7)).fixed_offset(),
        )
        .await?;
        println!(
            "{}",
            serde_json::json!({"deleted": deleted, "abandoned_imports": abandoned})
        );
        Ok(())
    }
}
