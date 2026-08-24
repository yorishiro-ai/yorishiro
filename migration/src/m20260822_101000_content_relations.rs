use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let table = Table::create()
            .table(Alias::new("content_relations"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
            .col(ColumnDef::new(Alias::new("source_id")).uuid().not_null())
            .col(ColumnDef::new(Alias::new("target_id")).uuid().not_null())
            .col(
                ColumnDef::new(Alias::new("relation_type"))
                    .text()
                    .not_null(),
            )
            .col(
                ColumnDef::new(Alias::new("properties"))
                    .json_binary()
                    .not_null()
                    .default(Expr::cust("'{}'")),
            )
            .col(
                ColumnDef::new(Alias::new("status"))
                    .text()
                    .not_null()
                    .default("active"),
            )
            .col(helpers::created_at())
            .foreign_key(
                ForeignKey::create()
                    .name("fk_content_relations_workspace_id")
                    .from(Alias::new("content_relations"), Alias::new("workspace_id"))
                    .to(Alias::new("identity_workspaces"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_content_relations_source_id")
                    .from(Alias::new("content_relations"), Alias::new("source_id"))
                    .to(Alias::new("content_entities"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_content_relations_target_id")
                    .from(Alias::new("content_relations"), Alias::new("target_id"))
                    .to(Alias::new("content_entities"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .index(
                Index::create()
                    .name("relations_unique")
                    .unique()
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("source_id"))
                    .col(Alias::new("target_id"))
                    .col(Alias::new("relation_type")),
            )
            .to_owned();

        // sea_query's Table::create() check() support was not reached for; a raw
        // ALTER TABLE ... ADD CONSTRAINT is simpler and matches the old DDL's inline CHECK exactly.
        helpers::create_table_with_checks(
            manager,
            "content_relations",
            table,
            &[(
                "content_relations_status_check",
                "status IN ('active', 'deprecated', 'archived')",
            )],
        )
        .await?;

        // `status` is in both indexes because every traversal filters on it.
        // Three-column composite indexes: execute_unprepared is used for both since create_index's builder offers nothing over the raw form here.
        let db = manager.get_connection();
        db.execute_unprepared(
            "CREATE INDEX relations_source_idx ON content_relations (workspace_id, source_id, status);",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX relations_target_idx ON content_relations (workspace_id, target_id, status);",
        )
        .await?;

        // Strict, not lenient: `yorishiro_app` sets both GUCs on every connection, so reaching this table without a workspace set is a bug, and raising surfaces it where matching zero rows would look like an empty workspace.
        helpers::enable_rls_with_policy(
            manager,
            "content_relations",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            false,
        )
        .await?;

        // Individualized per helpers::grant()'s own rule against wildcard grants now that every table shares one schema.
        helpers::grant(
            manager,
            "SELECT, INSERT, UPDATE, DELETE",
            "content_relations",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("content_relations"))
                    .to_owned(),
            )
            .await
    }
}
