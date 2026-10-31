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
        let parse_target = |name: &str| -> Result<uuid::Uuid> {
            vars.cli_arg(name)?.parse::<uuid::Uuid>().map_err(|_| {
                crate::YorishiroError::ValidationFailed {
                    message: format!("{name} is not a valid UUID"),
                    details: vec![],
                    hint: "provide matching tenant_id and confirmed_tenant_id UUIDs".into(),
                }
                .into()
            })
        };
        let tenant_id = parse_target("tenant_id")?;
        let confirmation = parse_target("confirmed_tenant_id")?;
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
