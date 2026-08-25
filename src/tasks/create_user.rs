use loco_rs::prelude::*;
use loco_rs::task::Vars;

use crate::models::tenancy;

/// `cargo loco task create_user email:owner@example.com password:hunter2-hunter2 [display_name:Alice]`
///
/// Wraps `tenancy::create_user`, which hashes the password (Argon2id) before writing it.
/// A created user holds no tenant membership yet; follow with `add_member`.
pub struct CreateUser;

#[async_trait]
impl Task for CreateUser {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "create_user".to_string(),
            detail: "Creates a user: cargo loco task create_user email:<email> password:<password> [display_name:<name>]".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let email = vars.cli_arg("email")?;
        let password = vars.cli_arg("password")?;
        let display_name = vars.cli_arg("display_name").ok();

        let user = tenancy::create_user(&app_context.db, email, password, display_name)
            .await
            .map_err(|err| Error::Message(err.to_string()))?;

        println!("user id: {}", user.id);
        println!("email:   {}", user.email);
        Ok(())
    }
}
