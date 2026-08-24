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
                    .table(Alias::new("content_fill_proposals"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk())
                    // Batch job identifier, not a row reference: no REFERENCES clause in the old DDL, matching content_entity_snapshots.job_id.
                    .col(ColumnDef::new(Alias::new("job_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
                    // No foreign key: an entity can be deleted between a proposal and its confirmation, per the old DDL having no REFERENCES clause here either.
                    .col(ColumnDef::new(Alias::new("entity_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("field_name")).text().not_null())
                    .col(
                        ColumnDef::new(Alias::new("proposed"))
                            .json_binary()
                            .not_null(),
                    )
                    .col(helpers::created_at())
                    .index(
                        Index::create()
                            .name("fill_proposals_workspace_job_entity_field_key")
                            .unique()
                            .col(Alias::new("workspace_id"))
                            .col(Alias::new("job_id"))
                            .col(Alias::new("entity_id"))
                            .col(Alias::new("field_name")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_content_fill_proposals_workspace_id")
                            .from(
                                Alias::new("content_fill_proposals"),
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
                    .name("fill_proposals_job_idx")
                    .table(Alias::new("content_fill_proposals"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("job_id"))
                    .to_owned(),
            )
            .await?;

        let db = manager.get_connection();

        // Lenient, matching content_entity_snapshots: the control-plane pool also reaches this table over a connection that has not named a workspace, and must match nothing rather than raise.
        helpers::enable_rls_with_policy(
            db,
            "content_fill_proposals",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            true,
        )
        .await?;

        helpers::grant(
            db,
            "SELECT, INSERT, UPDATE, DELETE",
            "content_fill_proposals",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("content_fill_proposals"))
                    .to_owned(),
            )
            .await
    }
}
