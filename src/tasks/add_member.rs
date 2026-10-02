use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
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
        let tenant_id: Uuid =
            vars.cli_arg("tenant_id")?
                .parse()
                .map_err(|_| YorishiroError::ValidationFailed {
                    message: "tenant_id is not a valid UUID".into(),
                    details: vec![],
                    hint: "tenant_id must be a UUID, e.g. 00000000-0000-0000-0000-000000000000"
                        .into(),
                })?;
        let user_id: Uuid =
            vars.cli_arg("user_id")?
                .parse()
                .map_err(|_| YorishiroError::ValidationFailed {
                    message: "user_id is not a valid UUID".into(),
                    details: vec![],
                    hint: "user_id must be a UUID, e.g. 00000000-0000-0000-0000-000000000000"
                        .into(),
                })?;
        let role_str = vars.cli_arg("role")?;
        let role = MembershipRole::from_db_str(role_str).ok_or_else(|| {
            YorishiroError::ValidationFailed {
                message: format!("'{role_str}' is not a valid role"),
                details: vec![],
                hint: String::new(),
            }
        })?;

        tenancy::add_member(&app_context.db, tenant_id, user_id, role)
            .await
            .internal()?;

        println!("membership added: user {user_id} is now {role_str} of tenant {tenant_id}");
        Ok(())
    }
}
