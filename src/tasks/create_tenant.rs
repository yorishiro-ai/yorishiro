use loco_rs::prelude::*;
use loco_rs::task::Vars;

use crate::error::ResultExt;

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

        let settings = app_context
            .config
            .settings::<crate::data::settings::Settings>()?;
        let max_tenants = (settings.max_tenants > 0).then_some(settings.max_tenants);
        let tenant =
            crate::models::tenant_tenants::create_tenant(&app_context.db, name, max_tenants)
                .await
                .internal()?;

        println!("tenant id: {}", tenant.id);
        Ok(())
    }
}
