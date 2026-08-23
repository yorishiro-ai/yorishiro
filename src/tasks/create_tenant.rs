use loco_rs::prelude::*;
use loco_rs::task::Vars;

use crate::models::_entities::identity_tenants::ActiveModel;

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

        let active = ActiveModel {
            name: ActiveValue::Set(name.to_string()),
            ..Default::default()
        };
        let tenant = active
            .insert(&app_context.db)
            .await
            .map_err(|err| Error::Message(err.to_string()))?;

        println!("tenant id: {}", tenant.id);
        Ok(())
    }
}
