use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::workspace_workspaces;

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
        let tenant_id: Uuid =
            vars.cli_arg("tenant_id")?
                .parse()
                .map_err(|_| YorishiroError::ValidationFailed {
                    message: "tenant_id is not a valid UUID".into(),
                    details: vec![],
                    hint: "tenant_id must be a UUID, e.g. 00000000-0000-0000-0000-000000000000"
                        .into(),
                })?;

        let workspaces = workspace_workspaces::Entity::find()
            .filter(workspace_workspaces::Column::TenantId.eq(tenant_id))
            .all(&app_context.db)
            .await
            .internal()?;

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
