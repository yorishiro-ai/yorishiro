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
        let [created_at, updated_at] = helpers::timestamps(manager);
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("workspace_llm_keys"))
                    .if_not_exists()
                    .col(
                        helpers::uuid_col(manager, Alias::new("workspace_id"))
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("base_url")).text().not_null())
                    .col(ColumnDef::new(Alias::new("model")).text().not_null())
                    .col(ColumnDef::new(Alias::new("api_key")).text().not_null())
                    .col(created_at)
                    .col(updated_at)
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_workspace_llm_keys_workspace_id")
                            .from(Alias::new("workspace_llm_keys"), Alias::new("workspace_id"))
                            .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        let [created_at, updated_at] = helpers::timestamps(manager);
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("workspace_embedding_keys"))
                    .if_not_exists()
                    .col(
                        helpers::uuid_col(manager, Alias::new("workspace_id"))
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("base_url")).text().not_null())
                    .col(ColumnDef::new(Alias::new("model")).text().not_null())
                    .col(ColumnDef::new(Alias::new("api_key")).text().not_null())
                    .col(
                        ColumnDef::new(Alias::new("dimensions"))
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("send_dimensions_param"))
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(created_at)
                    .col(updated_at)
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_workspace_embedding_keys_workspace_id")
                            .from(
                                Alias::new("workspace_embedding_keys"),
                                Alias::new("workspace_id"),
                            )
                            .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        let [created_at, updated_at] = helpers::timestamps(manager);

        manager
            .create_table(
                Table::create()
                    .table(Alias::new("workspace_worker_classes"))
                    .if_not_exists()
                    .col(
                        helpers::uuid_col(manager, Alias::new("workspace_id"))
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("worker_class")).text().not_null())
                    .col(created_at)
                    .col(updated_at)
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_workspace_worker_classes_workspace_id")
                            .from(
                                Alias::new("workspace_worker_classes"),
                                Alias::new("workspace_id"),
                            )
                            .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .check(Expr::cust(
                        "worker_class IN ('tenant_private', 'official', 'shared')",
                    ))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("workspace_worker_classes"))
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("workspace_embedding_keys"))
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("workspace_llm_keys"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
