use loco_rs::prelude::*;
use loco_rs::task::Vars;

use crate::error::ResultExt;
use crate::models::tenant_memberships::MembershipRole;
use crate::models::user_users;

/// `cargo loco task create_user email:owner@example.com password:hunter2-hunter2 [display_name:Alice]`
///
/// Wraps `user_users::create_user`, which hashes the password (Argon2id) before writing it.
/// A created user holds no tenant membership yet; follow with `add_member`.
pub(crate) struct CreateUser;

#[async_trait]
impl Task for CreateUser {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "create_user".to_string(),
            detail: "Creates a user: cargo loco task create_user email:<email> password:<password> [display_name:<name>], or a tenant user: tenant_id:<uuid> user_name:<name> password:<password> [role:admin|member|viewer]".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let password = vars.cli_arg("password")?;
        let display_name = vars.cli_arg("display_name").ok();

        if let Ok(user_name) = vars.cli_arg("user_name") {
            let tenant_id: uuid::Uuid = vars
                .cli_arg("tenant_id")?
                .parse()
                .map_err(|_| Error::string("tenant_id must be a UUID"))?;
            let role = match vars.cli_arg("role") {
                Ok(role) => MembershipRole::from_db_str(role)
                    .ok_or_else(|| Error::string("role must be admin, member or viewer"))?,
                Err(_) => MembershipRole::Member,
            };
            let txn = crate::db::begin_write(&app_context.db).await.internal()?;
            let user = crate::models::tenant_memberships::create_tenant_user_member(
                &txn,
                tenant_id,
                user_name,
                password,
                display_name,
                role,
            )
            .await
            .internal()?;
            txn.commit().await.internal()?;
            println!("user id:   {}", user.id);
            println!("tenant id: {tenant_id}");
            println!("user name: {user_name}");
            return Ok(());
        }
        let email = vars.cli_arg("email")?;
        let user = user_users::create_user(&app_context.db, email, password, display_name)
            .await
            .internal()?;

        println!("user id: {}", user.id);
        println!("email:   {}", user.root_email());
        Ok(())
    }
}
