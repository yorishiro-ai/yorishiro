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
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("entity_snapshots"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(helpers::uuid_col(manager, Alias::new("job_id")).not_null())
                    .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
                    .col(helpers::uuid_col(manager, Alias::new("entity_id")).not_null())
                    .col(helpers::uuid_col(manager, Alias::new("schema_id")).not_null())
                    .col(
                        ColumnDef::new(Alias::new("schema_version"))
                            .integer()
                            .not_null(),
                    )
                    .col(helpers::json_col(manager, Alias::new("data")).not_null())
                    .col(helpers::created_at(manager))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_entity_snapshots_workspace_id")
                            .from(Alias::new("entity_snapshots"), Alias::new("workspace_id"))
                            .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("entity_snapshots_job_idx")
                    .table(Alias::new("entity_snapshots"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("job_id"))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("entity_snapshots_entity_idx")
                    .table(Alias::new("entity_snapshots"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("entity_id"))
                    .col((Alias::new("created_at"), IndexOrder::Desc))
                    .to_owned(),
            )
            .await?;

        helpers::enable_rls_with_policy(
            manager,
            "entity_snapshots",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            true,
        )
        .await?;

        helpers::grant(
            manager,
            "SELECT, INSERT, UPDATE, DELETE",
            "entity_snapshots",
        )
        .await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("entity_snapshots"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
