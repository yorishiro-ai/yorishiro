use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // SECURITY DEFINER so the lookup can read rows RLS would hide: the caller has not been identified yet, so there is no workspace to scope to.
        // Two overloads: the one-argument form resolves a key bound to a single workspace (returns nothing for a key that carries none); the two-argument form additionally resolves a tenant-scoped key (workspace_id NULL) against a requested workspace, membership-checked so a tenant-scoped key cannot be pointed at a workspace outside its own tenant.
        db.execute_unprepared(
            "CREATE FUNCTION authenticate_api_key(p_key_hash bytea)
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
             GRANT EXECUTE ON FUNCTION authenticate_api_key(bytea) TO yorishiro_app;",
        )
        .await?;

        // No DEFAULT on the second argument, deliberately: with one, a single-argument call matches both overloads and Postgres refuses it as ambiguous ("function is not unique").
        // Requiring both makes the arity unambiguous, so the one-argument form above keeps resolving on its own.
        db.execute_unprepared(
            "CREATE FUNCTION authenticate_api_key(
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

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DROP FUNCTION IF EXISTS authenticate_api_key(bytea);
                 DROP FUNCTION IF EXISTS authenticate_api_key(bytea, uuid);",
            )
            .await?;
        Ok(())
    }
}
