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
            .table(Alias::new("entity_relations"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
            .col(helpers::uuid_col(manager, Alias::new("source_id")).not_null())
            .col(helpers::uuid_col(manager, Alias::new("target_id")).not_null())
            .col(
                ColumnDef::new(Alias::new("relation_type"))
                    .text()
                    .not_null(),
            )
            .col(
                helpers::json_col(manager, Alias::new("properties"))
                    .not_null()
                    .default(Expr::cust("'{}'")),
            )
            .col(
                ColumnDef::new(Alias::new("status"))
                    .text()
                    .not_null()
                    .default("active"),
            )
            .col(helpers::created_at(manager))
            .foreign_key(
                ForeignKey::create()
                    .name("fk_entity_relations_workspace_id")
                    .from(Alias::new("entity_relations"), Alias::new("workspace_id"))
                    .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_entity_relations_source_id")
                    .from(Alias::new("entity_relations"), Alias::new("source_id"))
                    .to(Alias::new("entity_entities"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_entity_relations_target_id")
                    .from(Alias::new("entity_relations"), Alias::new("target_id"))
                    .to(Alias::new("entity_entities"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .index(
                Index::create()
                    .name("entity_relations_unique")
                    .unique()
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("source_id"))
                    .col(Alias::new("target_id"))
                    .col(Alias::new("relation_type")),
            )
            .to_owned();

        helpers::create_table_with_checks(
            manager,
            "entity_relations",
            table,
            &[(
                "content_relations_status_check",
                "status IN ('active', 'deprecated', 'archived')",
            )],
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("entity_relations_source_idx")
                    .table(Alias::new("entity_relations"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("source_id"))
                    .col(Alias::new("status"))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("entity_relations_target_idx")
                    .table(Alias::new("entity_relations"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("target_id"))
                    .col(Alias::new("status"))
                    .to_owned(),
            )
            .await?;

        helpers::enable_rls_with_policy(
            manager,
            "entity_relations",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            false,
        )
        .await?;

        helpers::grant(
            manager,
            "SELECT, INSERT, UPDATE, DELETE",
            "entity_relations",
        )
        .await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("entity_relations"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
