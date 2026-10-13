use loco_rs::prelude::*;
use loco_rs::task::Vars;

use crate::error::{ResultExt, YorishiroError};

/// `cargo loco task create_tenant name:acme`
///
/// Runs on `ctx.db` (Loco's own connection, not the RLS-scoped tenant pool): a control-plane operation with no workspace to scope RLS to.
pub(crate) struct CreateTenant;

#[async_trait]
impl Task for CreateTenant {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "create_tenant".to_string(),
            detail: "Creates a tenant: cargo loco task create_tenant name:acme".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let name = vars.cli_arg("name")?;

        let max_tenants = app_context
            .shared_store
            .get::<crate::data::tenant_policy::TenantCap>()
            .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("tenant cap policy missing")))?
            .resolve();
        let tenant =
            crate::models::tenant_tenants::create_tenant(&app_context.db, name, max_tenants)
                .await
                .internal()?;

        println!("tenant id: {}", tenant.id);
        Ok(())
    }
}
