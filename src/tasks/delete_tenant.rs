use loco_rs::prelude::*;

pub(crate) struct DeleteTenant;
#[async_trait]
impl Task for DeleteTenant {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "delete_tenant".to_string(),
            detail: "Irreversibly delete a disabled tenant, retaining users and audit: tenant_id:<uuid> confirmed_tenant_id:<same-uuid>".to_string(),
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
                "identity_policy": "retain_all_user_accounts",
            })
        );
        Ok(())
    }
}
