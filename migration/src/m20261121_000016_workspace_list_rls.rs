use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

const TABLES: [&str; 6] = [
    "entity_entities",
    "entity_relations",
    "schema_schemas",
    "api_key_audit_log",
    "entity_column_preferences",
    "workspace_schema_forks",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in TABLES {
            helpers::pg_only(
                manager,
                &format!(
                    "CREATE POLICY authorized_workspace_list ON {table} FOR SELECT TO yorishiro_app
                     USING (workspace_id = ANY(NULLIF(current_setting('app.workspace_list', true), '')::uuid[])
                     AND EXISTS (SELECT 1 FROM workspace_workspaces w WHERE w.id = {table}.workspace_id
                         AND w.tenant_id = NULLIF(current_setting('app.current_tenant', true), '')::uuid));"
                ),
            )
            .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in TABLES {
            helpers::pg_only(
                manager,
                &format!("DROP POLICY IF EXISTS authorized_workspace_list ON {table};"),
            )
            .await?;
        }
        Ok(())
    }
}
