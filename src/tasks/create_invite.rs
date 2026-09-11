use chrono::Duration;
use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::tenancy::{self, MembershipRole};

/// `cargo loco task create_invite tenant_id:<uuid> email:user@example.com role:owner`
///
/// `role` is one of `owner`/`admin`/`member`/`viewer`.
/// `ttl_hours` is optional, defaulting to 72.
/// This is the invite step of the real invite→signup→login path: a key minted directly instead carries no `user_id`, so writes made with it are unattributed.
pub struct CreateInvite;

#[async_trait]
impl Task for CreateInvite {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "create_invite".to_string(),
            detail: "Issues a signup invite: cargo loco task create_invite tenant_id:<uuid> email:user@example.com role:owner [ttl_hours:72]".to_string(),
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
        let email = vars.cli_arg("email")?;
        let role_str = vars.cli_arg("role")?;
        let role = MembershipRole::from_db_str(role_str).ok_or_else(|| {
            YorishiroError::ValidationFailed {
                message: format!("'{role_str}' is not a valid role"),
                details: vec![],
                hint: String::new(),
            }
        })?;
        let ttl_hours: i64 = match vars.cli_arg("ttl_hours") {
            Ok(raw) => raw.parse().map_err(|_| YorishiroError::ValidationFailed {
                message: "ttl_hours is not a valid integer".into(),
                details: vec![],
                hint: String::new(),
            })?,
            Err(_) => 72,
        };

        let (invite, token) = tenancy::create_invite(
            &app_context.db,
            tenant_id,
            email,
            role,
            Duration::hours(ttl_hours),
        )
        .await
        .internal()?;

        println!("invite id: {}", invite.id);
        println!("invite token (shown once): {token}");
        Ok(())
    }
}
