//! Add workspace-owned fork metadata for versioned schema definitions.

use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let [created_at, updated_at] = helpers::timestamps(manager);
        let table = Table::create()
            .table(Alias::new("workspace_schema_forks"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
            .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
            .col(helpers::uuid_col(manager, Alias::new("source_workspace_id")).not_null())
            .col(helpers::uuid_col(manager, Alias::new("source_schema_id")).not_null())
            .col(
                ColumnDef::new(Alias::new("source_schema_version"))
                    .integer()
                    .not_null(),
            )
            .col(
                ColumnDef::new(Alias::new("source_schema_name"))
                    .text()
                    .not_null(),
            )
            .col(helpers::uuid_col(manager, Alias::new("fork_schema_id")).not_null())
            .col(
                ColumnDef::new(Alias::new("customized"))
                    .boolean()
                    .not_null()
                    .default(false),
            )
            .col(created_at)
            .col(updated_at)
            .foreign_key(
                ForeignKey::create()
                    .name("fk_workspace_schema_forks_tenant_id")
                    .from(
                        Alias::new("workspace_schema_forks"),
                        Alias::new("tenant_id"),
                    )
                    .to(Alias::new("tenant_tenants"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_workspace_schema_forks_workspace_id")
                    .from(
                        Alias::new("workspace_schema_forks"),
                        Alias::new("workspace_id"),
                    )
                    .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_workspace_schema_forks_source_workspace_id")
                    .from(
                        Alias::new("workspace_schema_forks"),
                        Alias::new("source_workspace_id"),
                    )
                    .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Restrict),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_workspace_schema_forks_source_schema_id")
                    .from(
                        Alias::new("workspace_schema_forks"),
                        Alias::new("source_schema_id"),
                    )
                    .to(Alias::new("schema_schemas"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Restrict),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_workspace_schema_forks_fork_schema_id")
                    .from(
                        Alias::new("workspace_schema_forks"),
                        Alias::new("fork_schema_id"),
                    )
                    .to(Alias::new("schema_schemas"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Restrict),
            )
            .to_owned();

        helpers::create_table_with_checks(
            manager,
            "workspace_schema_forks",
            table,
            &[(
                "workspace_schema_forks_source_schema_version_check",
                "source_schema_version > 0",
            )],
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("workspace_schema_forks_workspace_source_name_key")
                    .table(Alias::new("workspace_schema_forks"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("source_workspace_id"))
                    .col(Alias::new("source_schema_name"))
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("workspace_schema_forks_workspace_idx")
                    .table(Alias::new("workspace_schema_forks"))
                    .col(Alias::new("workspace_id"))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("workspace_schema_forks_source_schema_idx")
                    .table(Alias::new("workspace_schema_forks"))
                    .col(Alias::new("source_schema_id"))
                    .col(Alias::new("source_schema_version"))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("workspace_schema_forks_fork_schema_idx")
                    .table(Alias::new("workspace_schema_forks"))
                    .col(Alias::new("fork_schema_id"))
                    .to_owned(),
            )
            .await?;

        helpers::enable_rls_with_policy(
            manager,
            "workspace_schema_forks",
            "workspace_schema_forks_isolation",
            "workspace_id",
            "app.current_workspace",
            true,
        )
        .await?;
        helpers::grant(
            manager,
            "SELECT, INSERT, UPDATE, DELETE",
            "workspace_schema_forks",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("workspace_schema_forks"))
                    .to_owned(),
            )
            .await
    }
}
