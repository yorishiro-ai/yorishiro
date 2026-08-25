use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::models::_entities::identity_workspaces;

/// `cargo loco task list_workspaces tenant_id:<uuid>`
pub struct ListWorkspaces;

#[async_trait]
impl Task for ListWorkspaces {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "list_workspaces".to_string(),
            detail: "Lists a tenant's workspaces: cargo loco task list_workspaces tenant_id:<uuid>"
                .to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let tenant_id: Uuid = vars
            .cli_arg("tenant_id")?
            .parse()
            .map_err(|_| Error::Message("tenant_id is not a valid UUID".to_string()))?;

        let workspaces = identity_workspaces::Entity::find()
            .filter(identity_workspaces::Column::TenantId.eq(tenant_id))
            .all(&app_context.db)
            .await
            .map_err(|err| Error::Message(err.to_string()))?;

        if workspaces.is_empty() {
            println!("no workspaces for tenant {tenant_id}");
        }
        for workspace in workspaces {
            println!(
                "{}  {:<24} status={} max_entities={}",
                workspace.id,
                workspace.name,
                workspace.status,
                format_limit(workspace.max_entities),
            );
        }
        Ok(())
    }
}

fn format_limit(limit: Option<i32>) -> String {
    match limit {
        Some(n) => n.to_string(),
        None => "unlimited".to_string(),
    }
}
