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
        let table = Table::create()
            .table(Alias::new("api_key_audit_log"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
            .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
            .col(helpers::uuid_col(manager, Alias::new("api_key_id")))
            .col(helpers::uuid_col(manager, Alias::new("user_id")))
            .col(ColumnDef::new(Alias::new("action")).text().not_null())
            .col(helpers::json_col(manager, Alias::new("detail")).not_null())
            .col(helpers::created_at(manager))
            .foreign_key(
                ForeignKey::create()
                    .name("fk_api_key_audit_log_workspace_id")
                    .from(Alias::new("api_key_audit_log"), Alias::new("workspace_id"))
                    .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_api_key_audit_log_tenant_id")
                    .from(Alias::new("api_key_audit_log"), Alias::new("tenant_id"))
                    .to(Alias::new("tenant_tenants"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_api_key_audit_log_user_id")
                    .from(Alias::new("api_key_audit_log"), Alias::new("user_id"))
                    .to(Alias::new("user_users"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::SetNull),
            )
            .to_owned();

        helpers::create_table_with_checks(
        manager,
        "api_key_audit_log",
        table,
        &[(
            "api_key_audit_log_action_check",
            "action IN ('undo_migration_job', 'set_maintenance', 'reindex_embeddings', 'fill_defaults', 'background_job_completed')",
        )],
    )
    .await?;

        manager
            .create_index(
                Index::create()
                    .name("api_key_audit_log_workspace_id_created_at_idx")
                    .table(Alias::new("api_key_audit_log"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("created_at"))
                    .to_owned(),
            )
            .await?;

        helpers::enable_rls_with_policy(
            manager,
            "api_key_audit_log",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            false,
        )
        .await?;

        helpers::grant(manager, "SELECT, INSERT", "api_key_audit_log").await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("api_key_audit_log"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
