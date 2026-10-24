use super::helpers;
use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DbBackend;
#[derive(DeriveMigrationName)]
pub struct Migration;
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let mut table = Table::create()
            .table(Alias::new("workspace_workspaces"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
            .col(ColumnDef::new(Alias::new("name")).text().not_null())
            .col(ColumnDef::new(Alias::new("max_entities")).integer())
            .col(
                ColumnDef::new(Alias::new("status"))
                    .text()
                    .not_null()
                    .default("schema_pending"),
            )
            .col(ColumnDef::new(Alias::new("embedding_model")).text())
            .col(ColumnDef::new(Alias::new("embedding_dimensions")).integer())
            .col(helpers::uuid_col(manager, Alias::new("schema_id")))
            .col(helpers::created_at(manager))
            .col(
                ColumnDef::new(Alias::new("embedding_reindexing"))
                    .boolean()
                    .not_null()
                    .default(false),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_workspace_workspaces_tenant_id")
                    .from(Alias::new("workspace_workspaces"), Alias::new("tenant_id"))
                    .to(Alias::new("tenant_tenants"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .to_owned();

        if manager.get_database_backend() == DbBackend::Sqlite {
            table = table
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_workspace_workspaces_schema_id")
                        .from(Alias::new("workspace_workspaces"), Alias::new("schema_id"))
                        .to(Alias::new("schema_schemas"), Alias::new("id")),
                )
                .to_owned();
        }

        helpers::create_table_with_checks(
            manager,
            "workspace_workspaces",
            table,
            &[
                (
                    "workspace_workspaces_status_check",
                    "status IN ('schema_pending', 'active')",
                ),
                (
                    "workspace_workspaces_embedding_dimensions_check",
                    "embedding_dimensions IS NULL OR embedding_dimensions > 0",
                ),
            ],
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("workspace_workspaces_tenant_id_name_key")
                    .table(Alias::new("workspace_workspaces"))
                    .col(Alias::new("tenant_id"))
                    .col(Alias::new("name"))
                    .unique()
                    .to_owned(),
            )
            .await?;

        helpers::enable_rls_with_policy(
            manager,
            "workspace_workspaces",
            "tenant_isolation",
            "tenant_id",
            "app.current_tenant",
            false,
        )
        .await?;

        helpers::grant(manager, "SELECT", "workspace_workspaces").await?;

        helpers::pg_only(
            manager,
            "GRANT UPDATE (status, schema_id) ON workspace_workspaces TO yorishiro_app;",
        )
        .await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("workspace_workspaces"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
