use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DbBackend;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let [created_at, updated_at] = helpers::timestamps();
        let sqlite = manager.get_database_backend() == DbBackend::Sqlite;

        // Field names, in display order.
        // sea_query's ColumnDef has no default-expression-plus-json_binary combination that reads well here, so the DEFAULT goes on as a follow-up ALTER COLUMN on Postgres.
        // SQLite has no ALTER COLUMN at all (only rename/add/drop-column), so the default is set inline in the CREATE TABLE there instead; `'[]'::jsonb` is Postgres cast syntax with no SQLite equivalent, so the SQLite default is the bare JSON literal.
        let mut columns_col = ColumnDef::new(Alias::new("columns"))
            .json_binary()
            .not_null()
            .to_owned();
        if sqlite {
            columns_col.default(Expr::cust("'[]'"));
        }

        let table = Table::create()
            .table(Alias::new("content_entity_column_preferences"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
            .col(ColumnDef::new(Alias::new("entity_type")).text().not_null())
            .col(columns_col)
            .col(created_at)
            .col(updated_at)
            // The upsert target: without it, two tabs saving at once leave two rows and the reader picks one arbitrarily.
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
            .to_owned();

        // `jsonb_typeof(columns) = 'array'` is Postgres's jsonb function; SQLite's JSON1 extension spells the same check `json_type(columns) = 'array'`, so the two backends get different check expressions under the same constraint name rather than one shared string.
        helpers::create_table_with_checks(
            manager,
            "content_entity_column_preferences",
            table,
            &[(
                "entity_column_preferences_columns_is_array",
                if sqlite {
                    "json_type(columns) = 'array'"
                } else {
                    "jsonb_typeof(columns) = 'array'"
                },
            )],
        )
        .await?;

        // No-op on SQLite: the default is already inline in the CREATE TABLE above.
        helpers::pg_only(
            manager,
            "ALTER TABLE content_entity_column_preferences \
             ALTER COLUMN columns SET DEFAULT '[]'::jsonb;",
        )
        .await?;

        // Lenient: the control-plane pool also reaches this table over a connection that has not named a workspace, and must match nothing rather than raise.
        helpers::enable_rls_with_policy(
            manager,
            "content_entity_column_preferences",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            true,
        )
        .await?;

        helpers::grant(
            manager,
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
