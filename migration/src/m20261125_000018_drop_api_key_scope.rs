//! Drops `api_keys.scope`: IAM policies are the only authority over what a key may do.
//!
//! The `authenticate_api_key` SECURITY DEFINER functions return the stored columns, so they are recreated without `scope`.

use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        helpers::pg_only(
            manager,
            "DROP FUNCTION authenticate_api_key(bytea);
             DROP FUNCTION authenticate_api_key(bytea, uuid);

             CREATE FUNCTION authenticate_api_key(p_key_hash bytea)
             RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, user_id uuid, audit boolean)
             LANGUAGE sql
             SECURITY DEFINER
             SET search_path = pg_catalog, public, pg_temp
             AS $$
                SELECT k.id, k.workspace_id, k.tenant_id, k.user_id, k.audit
                FROM api_keys k
                LEFT JOIN workspace_workspaces w ON w.id = k.workspace_id
                WHERE k.key_hash = p_key_hash AND k.revoked_at IS NULL
                  AND (k.expires_at IS NULL OR k.expires_at > CURRENT_TIMESTAMP)
                  AND (k.workspace_id IS NULL OR w.tenant_id = k.tenant_id)
             $$;

             REVOKE ALL ON FUNCTION authenticate_api_key(bytea) FROM PUBLIC;
             GRANT EXECUTE ON FUNCTION authenticate_api_key(bytea) TO yorishiro_app;

             CREATE FUNCTION authenticate_api_key(
               p_key_hash bytea,
               p_requested_workspace uuid
             )
             RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, user_id uuid, audit boolean)
             LANGUAGE sql
             SECURITY DEFINER
             SET search_path = pg_catalog, public, pg_temp
             AS $$
               SELECT k.id,
                      COALESCE(k.workspace_id, w.id) AS workspace_id,
                      k.tenant_id,
                      k.user_id,
                      k.audit
               FROM api_keys k
               LEFT JOIN workspace_workspaces w
                      ON k.workspace_id IS NULL
                     AND w.id = p_requested_workspace
                     AND w.tenant_id = k.tenant_id
               WHERE k.key_hash = p_key_hash AND k.revoked_at IS NULL
                 AND (k.expires_at IS NULL OR k.expires_at > CURRENT_TIMESTAMP)
                 AND (k.workspace_id IS NOT NULL OR w.id IS NOT NULL)
             $$;

             REVOKE ALL ON FUNCTION authenticate_api_key(bytea, uuid) FROM PUBLIC;
             GRANT EXECUTE ON FUNCTION authenticate_api_key(bytea, uuid) TO yorishiro_app;",
        )
        .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("api_keys"))
                    .drop_column(Alias::new("scope"))
                    .to_owned(),
            )
            .await
    }

    /// The column is not restored: the stored scopes carried no authority beyond IAM.
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
