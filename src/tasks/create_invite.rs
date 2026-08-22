use chrono::Duration;
use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::models::tenancy::{self, MembershipRole};

/// `cargo loco task create_invite tenant_id:<uuid> email:user@example.com role:owner`
///
/// `role` is one of `owner`/`admin`/`member`/`viewer`. `ttl_hours` is optional, defaulting to 72.
/// The real invite→signup→login path this exists for: `.claude/rules/dogfooding.md` in
/// `yorishiro-specs`.
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
        let tenant_id: Uuid = vars
            .cli_arg("tenant_id")?
            .parse()
            .map_err(|_| Error::Message("tenant_id is not a valid UUID".to_string()))?;
        let email = vars.cli_arg("email")?;
        let role_str = vars.cli_arg("role")?;
        let role = MembershipRole::from_db_str(role_str)
            .ok_or_else(|| Error::Message(format!("'{role_str}' is not a valid role")))?;
        let ttl_hours: i64 = match vars.cli_arg("ttl_hours") {
            Ok(raw) => raw
                .parse()
                .map_err(|_| Error::Message("ttl_hours is not a valid integer".to_string()))?,
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
        .map_err(|err| Error::Message(err.to_string()))?;

        println!("invite id: {}", invite.id);
        println!("invite token (shown once): {token}");
        Ok(())
    }
}
