//! Expand the audit action check for the fill-defaults batch migration.

use super::helpers;
use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DbBackend;

#[derive(DeriveMigrationName)]
pub struct Migration;

const OLD_CHECK: &str = "action IN ('undo_migration_job', 'set_maintenance', 'reindex_embeddings')";
const NEW_CHECK: &str =
    "action IN ('undo_migration_job', 'set_maintenance', 'reindex_embeddings', 'fill_defaults')";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        replace_check(manager, NEW_CHECK).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        replace_check(manager, OLD_CHECK).await
    }
}

async fn replace_check(manager: &SchemaManager<'_>, new_check: &str) -> Result<(), DbErr> {
    if manager.get_database_backend() == DbBackend::Sqlite {
        let create = format!(
            "CREATE TABLE api_key_audit_log_new (\
                id BLOB NOT NULL PRIMARY KEY,\
                workspace_id BLOB NOT NULL,\
                tenant_id BLOB NOT NULL,\
                api_key_id BLOB,\
                user_id BLOB,\
                action TEXT NOT NULL CHECK ({new_check}),\
                detail TEXT NOT NULL,\
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\
                FOREIGN KEY (workspace_id) REFERENCES workspace_workspaces(id) ON DELETE CASCADE,\
                FOREIGN KEY (tenant_id) REFERENCES tenant_tenants(id) ON DELETE CASCADE,\
                FOREIGN KEY (user_id) REFERENCES user_users(id) ON DELETE SET NULL\
            );"
        );
        helpers::sqlite_only(manager, &create).await?;
        helpers::sqlite_only(
            manager,
            "INSERT INTO api_key_audit_log_new (id, workspace_id, tenant_id, api_key_id, user_id, action, detail, created_at) SELECT id, workspace_id, tenant_id, api_key_id, user_id, action, detail, created_at FROM api_key_audit_log;",
        )
        .await?;
        helpers::sqlite_only(manager, "DROP TABLE api_key_audit_log;").await?;
        helpers::sqlite_only(
            manager,
            "ALTER TABLE api_key_audit_log_new RENAME TO api_key_audit_log;",
        )
        .await?;
        helpers::sqlite_only(
            manager,
            "CREATE INDEX api_key_audit_log_workspace_id_created_at_idx ON api_key_audit_log (workspace_id, created_at);",
        )
        .await
    } else {
        let sql = format!(
            "ALTER TABLE api_key_audit_log DROP CONSTRAINT api_key_audit_log_action_check;\
             ALTER TABLE api_key_audit_log ADD CONSTRAINT api_key_audit_log_action_check CHECK ({new_check});"
        );
        helpers::pg_only(manager, &sql).await
    }
}
