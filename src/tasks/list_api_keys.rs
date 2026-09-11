use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::api_keys::Entity as ApiKeys;

/// `cargo loco task list_api_keys workspace_id:<uuid>`
pub struct ListApiKeys;

#[async_trait]
impl Task for ListApiKeys {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "list_api_keys".to_string(),
            detail:
                "Lists a workspace's API keys: cargo loco task list_api_keys workspace_id:<uuid>"
                    .to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let workspace_id: Uuid = vars.cli_arg("workspace_id")?.parse().map_err(|_| {
            YorishiroError::ValidationFailed {
                message: "workspace_id is not a valid UUID".into(),
                details: vec![],
                hint: "workspace_id must be a UUID, e.g. 00000000-0000-0000-0000-000000000000"
                    .into(),
            }
        })?;

        // A CLI listing, not a paged UI: shows up to MAX_LIST_LIMIT rather than truncating
        // silently at the smaller default an operator has no way to override here.
        let keys = ApiKeys::list_for_workspace(
            &app_context.db,
            workspace_id,
            crate::models::pagination::ListParams {
                limit: crate::models::pagination::MAX_LIST_LIMIT,
                offset: 0,
            },
        )
        .await
        .internal()?;

        if keys.is_empty() {
            println!("no api keys for workspace {workspace_id}");
        }
        for key in keys {
            println!(
                "{}  {:<8} audit={}  prefix={}  user={}  created={}  last_used={}",
                key.id,
                key.scope,
                key.audit,
                key.key_prefix,
                key.user_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "-".into()),
                key.created_at.format("%Y-%m-%d %H:%M"),
                key.last_used_at
                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "never".into()),
            );
        }
        Ok(())
    }
}
