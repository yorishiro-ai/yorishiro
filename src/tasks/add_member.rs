use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::models::tenancy::{self, MembershipRole};

/// `cargo loco task add_member tenant_id:<uuid> user_id:<uuid> role:owner`
///
/// `role` is one of `owner`/`admin`/`member`/`viewer`.
/// Adds a new membership, or updates the role of an existing one (see `tenancy::add_member`'s upsert).
pub struct AddMember;

#[async_trait]
impl Task for AddMember {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "add_member".to_string(),
            detail: "Adds or updates a tenant membership: cargo loco task add_member tenant_id:<uuid> user_id:<uuid> role:<owner|admin|member|viewer>".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let tenant_id: Uuid = vars
            .cli_arg("tenant_id")?
            .parse()
            .map_err(|_| Error::Message("tenant_id is not a valid UUID".to_string()))?;
        let user_id: Uuid = vars
            .cli_arg("user_id")?
            .parse()
            .map_err(|_| Error::Message("user_id is not a valid UUID".to_string()))?;
        let role_str = vars.cli_arg("role")?;
        let role = MembershipRole::from_db_str(role_str)
            .ok_or_else(|| Error::Message(format!("'{role_str}' is not a valid role")))?;

        tenancy::add_member(&app_context.db, tenant_id, user_id, role)
            .await
            .map_err(|err| Error::Message(err.to_string()))?;

        println!("membership added: role updated to {role_str}");
        Ok(())
    }
}
