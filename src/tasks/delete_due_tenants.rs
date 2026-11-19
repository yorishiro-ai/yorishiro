use loco_rs::prelude::*;

pub(crate) struct DeleteDueTenants;

#[async_trait]
impl Task for DeleteDueTenants {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "delete_due_tenants".into(),
            detail: "Delete tenants whose root account deletion has passed its grace period".into(),
        }
    }

    async fn run(&self, ctx: &AppContext, _vars: &task::Vars) -> Result<()> {
        let deleted =
            crate::models::tenant_tenants::retirement::reap_due(&ctx.db, crate::data::time::now())
                .await?;
        println!("{}", serde_json::json!({"deleted": deleted}));
        Ok(())
    }
}
