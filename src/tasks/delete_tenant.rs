use loco_rs::prelude::*;

pub(crate) struct DeleteTenant;
#[async_trait]
impl Task for DeleteTenant {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "delete_tenant".to_string(),
            detail: "Irreversibly delete a disabled tenant, retaining root accounts and retirement evidence: tenant_id:<uuid> confirmed_tenant_id:<same-uuid>".to_string(),
        }
    }
    async fn run(&self, ctx: &AppContext, vars: &task::Vars) -> Result<()> {
        let (tenant_id, confirmation) = super::tenant_confirmation(vars)?;
        crate::models::tenant_tenants::retirement::delete_by_operator(
            &ctx.db,
            tenant_id,
            confirmation,
        )
        .await?;
        println!(
            "{}",
            serde_json::json!({
                "tenant_id": tenant_id,
                "status": "deleted",
                "identity_policy": "retain_root_accounts_delete_tenant_users",
            })
        );
        Ok(())
    }
}
