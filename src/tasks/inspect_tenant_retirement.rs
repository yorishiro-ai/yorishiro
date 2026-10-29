use loco_rs::prelude::*;

pub(crate) struct InspectTenantRetirement;
#[async_trait]
impl Task for InspectTenantRetirement {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "inspect_tenant_retirement".to_string(),
            detail: "Read-only tenant retirement inventory: tenant_id:<uuid>".to_string(),
        }
    }
    async fn run(&self, app_context: &AppContext, vars: &task::Vars) -> Result<()> {
        let tenant_id = vars
            .cli_arg("tenant_id")?
            .parse::<uuid::Uuid>()
            .map_err(|_| crate::YorishiroError::ValidationFailed {
                message: "tenant_id is not a valid UUID".into(),
                details: vec![],
                hint: "provide the target tenant UUID".into(),
            })?;
        let inventory =
            crate::models::tenant_tenants::retirement::inventory(&app_context.db, tenant_id)
                .await?;
        println!("{}", serde_json::to_string_pretty(&inventory)?);
        Ok(())
    }
}
