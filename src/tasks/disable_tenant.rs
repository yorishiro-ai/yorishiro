use loco_rs::prelude::*;

pub(crate) struct DisableTenant;
#[async_trait]
impl Task for DisableTenant {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "disable_tenant".to_string(),
            detail:
                "Disable and retain tenant data: tenant_id:<uuid> confirmed_tenant_id:<same-uuid>"
                    .to_string(),
        }
    }
    async fn run(&self, ctx: &AppContext, vars: &task::Vars) -> Result<()> {
        let (tenant_id, confirmation) = super::tenant_confirmation(vars)?;
        crate::models::tenant_tenants::retirement::disable_by_operator(
            &ctx.db,
            tenant_id,
            confirmation,
        )
        .await?;
        let inventory =
            crate::models::tenant_tenants::retirement::inventory(&ctx.db, tenant_id).await?;
        println!("{}", serde_json::to_string_pretty(&inventory)?);
        Ok(())
    }
}
