use loco_rs::prelude::*;
use loco_rs::task::Vars;

use crate::error::YorishiroError;

/// `cargo loco task create_tenant name:acme`
///
/// Runs on `ctx.db` (Loco's own connection, not the RLS-scoped tenant pool): a control-plane operation with no workspace to scope RLS to.
pub struct CreateTenant;

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
            crate::models::tenancy::create_tenant_with_limit(&app_context.db, name, max_tenants)
                .await
                .map_err(|err| YorishiroError::Internal(err.into()))?;

        println!("tenant id: {}", tenant.id);
        Ok(())
    }
}
