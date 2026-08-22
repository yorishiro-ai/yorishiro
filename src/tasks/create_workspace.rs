use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::models::_entities::identity_workspaces::ActiveModel;

/// `cargo loco task create_workspace tenant_id:<uuid> name:acme-prod`
///
/// Runs on `ctx.db`, same reasoning as `create_tenant`. Leaves `status` unset so the column's
/// own default (`'schema_pending'`) applies; a workspace only moves to `active` once
/// `content_schemas::create_schema` gives it its first schema.
pub struct CreateWorkspace;

#[async_trait]
impl Task for CreateWorkspace {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "create_workspace".to_string(),
            detail: "Creates a workspace: cargo loco task create_workspace tenant_id:<uuid> name:acme-prod".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let tenant_id: Uuid = vars
            .cli_arg("tenant_id")?
            .parse()
            .map_err(|_| Error::Message("tenant_id is not a valid UUID".to_string()))?;
        let name = vars.cli_arg("name")?;

        let active = ActiveModel {
            tenant_id: ActiveValue::Set(tenant_id),
            name: ActiveValue::Set(name.to_string()),
            ..Default::default()
        };
        let workspace = active
            .insert(&app_context.db)
            .await
            .map_err(|err| Error::Message(err.to_string()))?;

        println!("workspace id: {}", workspace.id);
        Ok(())
    }
}
