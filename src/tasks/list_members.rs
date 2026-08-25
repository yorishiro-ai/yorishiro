use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::models::tenancy;

/// `cargo loco task list_members tenant_id:<uuid>`
pub struct ListMembers;

#[async_trait]
impl Task for ListMembers {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "list_members".to_string(),
            detail: "Lists a tenant's members: cargo loco task list_members tenant_id:<uuid>"
                .to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let tenant_id: Uuid = vars
            .cli_arg("tenant_id")?
            .parse()
            .map_err(|_| Error::Message("tenant_id is not a valid UUID".to_string()))?;

        // A CLI listing, not a paged UI: shows up to MAX_LIST_LIMIT rather than truncating
        // silently at the smaller default an operator has no way to override here.
        let members = tenancy::list_members(
            &app_context.db,
            tenant_id,
            crate::models::pagination::ListParams {
                limit: crate::models::pagination::MAX_LIST_LIMIT,
                offset: 0,
            },
        )
        .await
        .map_err(|err| Error::Message(err.to_string()))?;

        if members.is_empty() {
            println!("no members for tenant {tenant_id}");
        }
        for member in members {
            println!(
                "{}  {:<8} {}",
                member.user_id,
                member.role.as_db_str(),
                member.email,
            );
        }
        Ok(())
    }
}
