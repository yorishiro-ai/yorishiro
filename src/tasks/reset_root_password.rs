use std::io::{IsTerminal, Read};

use loco_rs::prelude::*;

pub(crate) struct ResetRootPassword;
#[async_trait]
impl Task for ResetRootPassword {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "reset_root_password".to_string(),
            detail: "Reset an existing root password and revoke all attributed keys: user_id:<uuid> confirmed_user_id:<same-uuid>; read the new password from piped stdin only".to_string(),
        }
    }
    async fn run(&self, ctx: &AppContext, vars: &task::Vars) -> Result<()> {
        if vars.cli.len() != 2
            || vars.args().len() != 2
            || vars
                .cli
                .keys()
                .any(|key| !matches!(key.as_str(), "user_id" | "confirmed_user_id"))
        {
            return Err(Error::string(
                "only user_id and confirmed_user_id arguments are accepted; supply the password through piped stdin",
            ));
        }
        let user_id = vars
            .cli_arg("user_id")?
            .parse::<uuid::Uuid>()
            .map_err(|_| Error::string("user_id must be a UUID"))?;
        let confirmation = vars
            .cli_arg("confirmed_user_id")?
            .parse::<uuid::Uuid>()
            .map_err(|_| Error::string("confirmed_user_id must be a UUID"))?;
        if user_id != confirmation {
            return Err(Error::string("confirmed_user_id must match user_id"));
        }
        let password = tokio::task::spawn_blocking(|| -> Result<String> {
            let stdin = std::io::stdin();
            // Refuse an echoing terminal rather than exposing the secret during entry.
            if stdin.is_terminal() {
                return Err(Error::string(
                    "supply the password through piped stdin, not a terminal",
                ));
            }
            let mut password = String::new();
            stdin
                .lock()
                .take(4097)
                .read_to_string(&mut password)
                .map_err(|_| Error::string("could not read password from stdin"))?;
            if password.len() > 4096 {
                return Err(Error::string("stdin password exceeds 4096 bytes"));
            }
            if password.ends_with('\n') {
                password.pop();
                if password.ends_with('\r') {
                    password.pop();
                }
            }
            if password.contains(['\n', '\r']) {
                return Err(Error::string("stdin must contain one password line"));
            }
            Ok(password)
        })
        .await
        .map_err(|_| Error::string("password input failed"))??;
        crate::models::user_users::reset_root_password_by_operator(
            &ctx.db,
            user_id,
            confirmation,
            &password,
        )
        .await?;
        println!("root password reset; all attributed API keys revoked");
        Ok(())
    }
}
