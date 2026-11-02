use super::helpers;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Only counts leave the destination workspace's RLS boundary, never fork contents or identifiers.
        helpers::pg_only(manager, "CREATE FUNCTION schema_fork_reference_counts(p_schema_id uuid)
            RETURNS TABLE(forks bigint, fork_heads bigint)
            LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog, public, pg_temp AS $$
              SELECT
                (SELECT count(*) FROM workspace_schema_forks f WHERE f.tenant_id = s.tenant_id AND (f.source_schema_id = s.id OR f.fork_schema_id = s.id)),
                (SELECT count(*) FROM workspace_schema_fork_heads h WHERE h.tenant_id = s.tenant_id AND h.schema_id = s.id)
              FROM schema_schemas s WHERE s.id = p_schema_id
                AND s.workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid
                AND s.tenant_id = NULLIF(current_setting('app.current_tenant', true), '')::uuid;
            $$;
            REVOKE ALL ON FUNCTION schema_fork_reference_counts(uuid) FROM PUBLIC;
            GRANT EXECUTE ON FUNCTION schema_fork_reference_counts(uuid) TO yorishiro_app;").await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        helpers::pg_only(manager, "DROP FUNCTION schema_fork_reference_counts(uuid)").await
    }
}
