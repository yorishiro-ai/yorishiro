use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let [created_at, updated_at] = helpers::timestamps();
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("content_entity_column_preferences"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk())
                    .col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("entity_type")).text().not_null())
                    // Field names, in display order. sea_query's ColumnDef has no
                    // default-expression-plus-json_binary combination that reads well here, so
                    // the DEFAULT '[]'::jsonb goes on as raw SQL right after create_table,
                    // matching templates.rs's TEXT[] precedent.
                    .col(
                        ColumnDef::new(Alias::new("columns"))
                            .json_binary()
                            .not_null(),
                    )
                    .col(created_at)
                    .col(updated_at)
                    // The upsert target: without it, two tabs saving at once leave two rows and
                    // the reader picks one arbitrarily.
                    .index(
                        Index::create()
                            .name("entity_column_preferences_workspace_entity_type_key")
                            .unique()
                            .col(Alias::new("workspace_id"))
                            .col(Alias::new("entity_type")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_content_entity_column_preferences_workspace_id")
                            .from(
                                Alias::new("content_entity_column_preferences"),
                                Alias::new("workspace_id"),
                            )
                            .to(Alias::new("identity_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        let db = manager.get_connection();

        db.execute_unprepared(
            "ALTER TABLE content_entity_column_preferences \
             ALTER COLUMN columns SET DEFAULT '[]'::jsonb;",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE content_entity_column_preferences \
             ADD CONSTRAINT entity_column_preferences_columns_is_array \
             CHECK (jsonb_typeof(columns) = 'array');",
        )
        .await?;

        // Lenient, matching content_fill_proposals: the control-plane pool also reaches this
        // table over a connection that has not named a workspace, and must match nothing
        // rather than raise.
        helpers::enable_rls_with_policy(
            db,
            "content_entity_column_preferences",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            true,
        )
        .await?;

        helpers::grant(
            db,
            "SELECT, INSERT, UPDATE, DELETE",
            "content_entity_column_preferences",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("content_entity_column_preferences"))
                    .to_owned(),
            )
            .await
    }
}
