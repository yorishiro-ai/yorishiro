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
        let [created_at, updated_at] = helpers::timestamps(manager);
        let sqlite = manager.get_database_backend() == DbBackend::Sqlite;

        let columns_col = helpers::json_col(manager, Alias::new("columns"))
            .not_null()
            .default(if sqlite {
                Expr::cust("'[]'")
            } else {
                Expr::cust("'[]'::jsonb")
            })
            .to_owned();

        let table = Table::create()
            .table(Alias::new("entity_column_preferences"))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
            .col(ColumnDef::new(Alias::new("entity_type")).text().not_null())
            .col(columns_col)
            .col(created_at)
            .col(updated_at)
            .index(
                Index::create()
                    .name("entity_column_preferences_workspace_entity_type_key")
                    .unique()
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("entity_type")),
            )
            .foreign_key(
                ForeignKey::create()
                    .name("fk_entity_column_preferences_workspace_id")
                    .from(
                        Alias::new("entity_column_preferences"),
                        Alias::new("workspace_id"),
                    )
                    .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .to_owned();

        helpers::create_table_with_checks(
            manager,
            "entity_column_preferences",
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

        helpers::enable_rls_with_policy(
            manager,
            "entity_column_preferences",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            true,
        )
        .await?;

        helpers::grant(
            manager,
            "SELECT, INSERT, UPDATE, DELETE",
            "entity_column_preferences",
        )
        .await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("entity_column_preferences"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
