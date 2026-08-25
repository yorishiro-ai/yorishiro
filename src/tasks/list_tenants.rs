use loco_rs::prelude::*;
use loco_rs::task::Vars;

use crate::models::_entities::identity_tenants;

/// `cargo loco task list_tenants`
pub struct ListTenants;

#[async_trait]
impl Task for ListTenants {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "list_tenants".to_string(),
            detail: "Lists every tenant: cargo loco task list_tenants".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, _vars: &Vars) -> Result<()> {
        let tenants = identity_tenants::Entity::find()
            .all(&app_context.db)
            .await
            .map_err(|err| Error::Message(err.to_string()))?;

        if tenants.is_empty() {
            println!("no tenants (create one with `cargo loco task create_tenant name:acme`)");
        }
        for tenant in tenants {
            println!(
                "{}  {:<24} max_workspaces={}",
                tenant.id,
                tenant.name,
                format_limit(tenant.max_workspaces),
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
