use loco_rs::prelude::*;
use loco_rs::task::Vars;
use uuid::Uuid;
use yorishiro_core::db::DbHandle;

use crate::services::tenant_auth::create_tenant_api_key;

/// `cargo loco task create_tenant_api_key tenant_id:<uuid> scope:read [user_id:<uuid>]`
///
/// Separate from base's own `create_api_key` task, which always binds a key to one workspace.
/// This issues a key with no bound workspace, naming the workspace per request with the
/// `X-Workspace-Id` header instead: prefer `create_api_key` when a client only ever works in a
/// single workspace, since a key bound to one workspace reaches less if it leaks.
///
/// Wraps the already-ported `services::tenant_auth::create_tenant_api_key`, which does the
/// actual insert (and the tenant-exists/role-cap checks) on the identity pool.
pub struct CreateTenantApiKey;

#[async_trait]
impl Task for CreateTenantApiKey {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "create_tenant_api_key".to_string(),
            detail: "Issues a tenant-scoped API key: cargo loco task create_tenant_api_key tenant_id:<uuid> scope:read [user_id:<uuid>]".to_string(),
        }
    }

    async fn run(&self, app_context: &AppContext, vars: &Vars) -> Result<()> {
        let tenant_id: Uuid = vars
            .cli_arg("tenant_id")?
            .parse()
            .map_err(|_| Error::Message("tenant_id is not a valid UUID".to_string()))?;
        let scope = vars.cli_arg("scope")?;
        let user_id = match vars.cli_arg("user_id") {
            Ok(raw) => Some(
                raw.parse::<Uuid>()
                    .map_err(|_| Error::Message("user_id is not a valid UUID".to_string()))?,
            ),
            Err(_) => None,
        };

        let db = app_context
            .shared_store
            .get::<DbHandle>()
            .ok_or_else(|| Error::Message("DbHandle missing from shared_store".to_string()))?;

        let created = create_tenant_api_key(&db.identity, tenant_id, scope, user_id)
            .await
            .map_err(|err| Error::Message(err.to_string()))?;

        println!(
            "tenant-scoped api key created (the plaintext key is shown ONLY once, store it now)"
        );
        println!("  key:          {}", created.plaintext);
        println!("  key id:       {}", created.id);
        println!("  tenant id:    {tenant_id}");
        println!("  workspace id: (send X-Workspace-Id on each request)");
        println!("  scope:        {scope}");
        Ok(())
    }
}
