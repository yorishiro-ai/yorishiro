use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::models::identity_api_keys::Entity as ApiKeys;
use crate::services::auth::ApiKeyScope;

/// `cargo loco task create_api_key workspace_id:<uuid> scope:write`
///
/// `scope` is one of `read`/`write`/`schema`/`migration`.
/// `user_id` is optional; omitted, the key is unattributed (a service/automation key, `user_id = NULL`), same meaning as elsewhere in this codebase.
/// `audit` is optional (`audit:true` to set it, anything else or omitted is `false`): the independent grant that lets this key read `GET /api/audit-log`, regardless of `scope`.
///
/// Wraps `identity_api_keys::Entity::create_api_key`, which does the actual insert (and the workspace-exists check) on `ctx.db`.
pub struct CreateApiKey;

#[async_trait]
impl Task for CreateApiKey {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "create_api_key".to_string(),
            detail: "Issues an API key: cargo loco task create_api_key workspace_id:<uuid> scope:write [user_id:<uuid>] [audit:true]".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let workspace_id: Uuid = vars
            .cli_arg("workspace_id")?
            .parse()
            .map_err(|_| Error::Message("workspace_id is not a valid UUID".to_string()))?;
        let scope_str = vars.cli_arg("scope")?;
        let scope = ApiKeyScope::from_db_str(scope_str)
            .ok_or_else(|| Error::Message(format!("'{scope_str}' is not a valid scope")))?;
        let user_id = match vars.cli_arg("user_id") {
            Ok(raw) => Some(
                raw.parse::<Uuid>()
                    .map_err(|_| Error::Message("user_id is not a valid UUID".to_string()))?,
            ),
            Err(_) => None,
        };
        let audit = vars.cli_arg("audit").map(|v| v == "true").unwrap_or(false);

        let created = ApiKeys::create_api_key(&app_context.db, workspace_id, scope, user_id, audit)
            .await
            .map_err(|err| Error::Message(err.to_string()))?;

        println!("api key id: {}", created.id);
        println!("api key (shown once): {}", created.plaintext);
        if audit {
            println!("audit grant: enabled");
        }
        Ok(())
    }
}
