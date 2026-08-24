use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // `RETURNS TABLE`'s column list can't be widened with ALTER FUNCTION, so both overloads are dropped and recreated with `audit` added, otherwise identical to `m20260822_101200_authenticate_api_key.rs`.
        // `authenticate` (services::auth) needs this column to populate `AuthContext`, which is what `require_audit` reads to decide whether a key may reach the audit-log read endpoint.
        db.execute_unprepared(
            "DROP FUNCTION authenticate_api_key(bytea);
             DROP FUNCTION authenticate_api_key(bytea, uuid);

             CREATE FUNCTION authenticate_api_key(p_key_hash bytea)
             RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, scope text, user_id uuid, audit boolean)
             LANGUAGE sql
             SECURITY DEFINER
             SET search_path = pg_catalog, public
             AS $$
               SELECT k.id, k.workspace_id, w.tenant_id, k.scope, k.user_id, k.audit
               FROM identity_api_keys k
               JOIN identity_workspaces w ON w.id = k.workspace_id
               WHERE k.key_hash = p_key_hash
             $$;

             REVOKE ALL ON FUNCTION authenticate_api_key(bytea) FROM PUBLIC;
             GRANT EXECUTE ON FUNCTION authenticate_api_key(bytea) TO yorishiro_app;

             CREATE FUNCTION authenticate_api_key(
               p_key_hash bytea,
               p_requested_workspace uuid
             )
             RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, scope text, user_id uuid, audit boolean)
             LANGUAGE sql
             SECURITY DEFINER
             SET search_path = pg_catalog, public
             AS $$
               SELECT k.id,
                      COALESCE(k.workspace_id, w.id) AS workspace_id,
                      k.tenant_id,
                      k.scope,
                      k.user_id,
                      k.audit
               FROM identity_api_keys k
               LEFT JOIN identity_workspaces w
                      ON k.workspace_id IS NULL
                     AND w.id = p_requested_workspace
                     AND w.tenant_id = k.tenant_id
               WHERE k.key_hash = p_key_hash
                 AND (k.workspace_id IS NOT NULL OR w.id IS NOT NULL)
             $$;

             REVOKE ALL ON FUNCTION authenticate_api_key(bytea, uuid) FROM PUBLIC;
             GRANT EXECUTE ON FUNCTION authenticate_api_key(bytea, uuid) TO yorishiro_app;",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            "DROP FUNCTION authenticate_api_key(bytea);
             DROP FUNCTION authenticate_api_key(bytea, uuid);

             CREATE FUNCTION authenticate_api_key(p_key_hash bytea)
             RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, scope text, user_id uuid)
             LANGUAGE sql
             SECURITY DEFINER
             SET search_path = pg_catalog, public
             AS $$
               SELECT k.id, k.workspace_id, w.tenant_id, k.scope, k.user_id
               FROM identity_api_keys k
               JOIN identity_workspaces w ON w.id = k.workspace_id
               WHERE k.key_hash = p_key_hash
             $$;

             REVOKE ALL ON FUNCTION authenticate_api_key(bytea) FROM PUBLIC;
             GRANT EXECUTE ON FUNCTION authenticate_api_key(bytea) TO yorishiro_app;

             CREATE FUNCTION authenticate_api_key(
               p_key_hash bytea,
               p_requested_workspace uuid
             )
             RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, scope text, user_id uuid)
             LANGUAGE sql
             SECURITY DEFINER
             SET search_path = pg_catalog, public
             AS $$
               SELECT k.id,
                      COALESCE(k.workspace_id, w.id) AS workspace_id,
                      k.tenant_id,
                      k.scope,
                      k.user_id
               FROM identity_api_keys k
               LEFT JOIN identity_workspaces w
                      ON k.workspace_id IS NULL
                     AND w.id = p_requested_workspace
                     AND w.tenant_id = k.tenant_id
               WHERE k.key_hash = p_key_hash
                 AND (k.workspace_id IS NOT NULL OR w.id IS NOT NULL)
             $$;

             REVOKE ALL ON FUNCTION authenticate_api_key(bytea, uuid) FROM PUBLIC;
             GRANT EXECUTE ON FUNCTION authenticate_api_key(bytea, uuid) TO yorishiro_app;",
        )
        .await?;

        Ok(())
    }
}
