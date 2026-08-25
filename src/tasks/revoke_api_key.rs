use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;

use crate::models::identity_api_keys::Entity as ApiKeys;

/// `cargo loco task revoke_api_key key_id:<uuid>`
///
/// Authentication looks up the key in the database on every request, so deleting the row revokes it immediately (takes effect on the next request).
pub struct RevokeApiKey;

#[async_trait]
impl Task for RevokeApiKey {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "revoke_api_key".to_string(),
            detail: "Revokes an API key: cargo loco task revoke_api_key key_id:<uuid>".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let key_id: Uuid = vars
            .cli_arg("key_id")?
            .parse()
            .map_err(|_| Error::Message("key_id is not a valid UUID".to_string()))?;

        ApiKeys::revoke(&app_context.db, key_id)
            .await
            .map_err(|err| Error::Message(err.to_string()))?;

        println!("api key {key_id} revoked (takes effect on the next request)");
        Ok(())
    }
}
