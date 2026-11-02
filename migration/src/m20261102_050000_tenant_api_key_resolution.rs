use super::helpers;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// The tenant-scoped overload now returns the key's own `tenant_id` and leaves `workspace_id` null for keys without one, so a tenant-scoped key can authenticate.
// A workspace-scoped key still only authenticates when its workspace belongs to its stored tenant.
const TENANT_KEY_AUTHENTICATOR: &str = "
CREATE OR REPLACE FUNCTION authenticate_api_key(p_key_hash bytea)
RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, scope text, user_id uuid, audit boolean)
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog, public, pg_temp AS $$
  SELECT k.id, k.workspace_id, k.tenant_id, k.scope, k.user_id, k.audit
  FROM api_keys k LEFT JOIN workspace_workspaces w ON w.id = k.workspace_id
  WHERE k.key_hash = p_key_hash AND k.revoked_at IS NULL
    AND (k.expires_at IS NULL OR k.expires_at > CURRENT_TIMESTAMP)
    AND (k.workspace_id IS NULL OR w.tenant_id = k.tenant_id)
$$;";

const PREVIOUS_AUTHENTICATOR: &str = "
CREATE OR REPLACE FUNCTION authenticate_api_key(p_key_hash bytea)
RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, scope text, user_id uuid, audit boolean)
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog, public, pg_temp AS $$
  SELECT k.id, k.workspace_id, w.tenant_id, k.scope, k.user_id, k.audit
  FROM api_keys k JOIN workspace_workspaces w ON w.id = k.workspace_id
  WHERE k.key_hash = p_key_hash AND k.revoked_at IS NULL
    AND (k.expires_at IS NULL OR k.expires_at > CURRENT_TIMESTAMP)
$$;";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        helpers::pg_only(manager, TENANT_KEY_AUTHENTICATOR).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        helpers::pg_only(manager, PREVIOUS_AUTHENTICATOR).await
    }
}
