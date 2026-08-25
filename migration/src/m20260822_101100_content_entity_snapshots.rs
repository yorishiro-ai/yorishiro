use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("content_entity_snapshots"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    // Batch job identifier, not a row reference: no REFERENCES clause in the old DDL, so no foreign key here either.
                    .col(ColumnDef::new(Alias::new("job_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
                    // The entity a snapshot was taken of.
                    // No foreign key: an entity can be deleted while a snapshot of it remains, per the old DDL having no REFERENCES clause on this column.
                    .col(ColumnDef::new(Alias::new("entity_id")).uuid().not_null())
                    // No foreign key in the old DDL either.
                    .col(ColumnDef::new(Alias::new("schema_id")).uuid().not_null())
                    .col(
                        ColumnDef::new(Alias::new("schema_version"))
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("data")).json_binary().not_null())
                    .col(helpers::created_at())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_content_entity_snapshots_workspace_id")
                            .from(
                                Alias::new("content_entity_snapshots"),
                                Alias::new("workspace_id"),
                            )
                            .to(Alias::new("identity_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("entity_snapshots_job_idx")
                    .table(Alias::new("content_entity_snapshots"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("job_id"))
                    .to_owned(),
            )
            .await?;

        let db = manager.get_connection();

        // DESC ordering on created_at: create_index has no per-column sort-direction support, so raw SQL per the porting instructions.
        db.execute_unprepared(
            "CREATE INDEX entity_snapshots_entity_idx \
             ON content_entity_snapshots (workspace_id, entity_id, created_at DESC);",
        )
        .await?;

        // Lenient: grouped with content.schemas and content.fill_proposals in the old DDL's strict/lenient split, since the control-plane pool also reaches this table over a connection that has not named a workspace, and must match nothing rather than raise.
        helpers::enable_rls_with_policy(
            manager,
            "content_entity_snapshots",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            true,
        )
        .await?;

        // The old DDL granted this table via a schema-wide "GRANT ... ON ALL TABLES IN SCHEMA content", now individualized per-table since every table shares one schema after the public-schema unification.
        helpers::grant(
            manager,
            "SELECT, INSERT, UPDATE, DELETE",
            "content_entity_snapshots",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("content_entity_snapshots"))
                    .to_owned(),
            )
            .await
    }
}
