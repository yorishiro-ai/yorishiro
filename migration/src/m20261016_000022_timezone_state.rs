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
                    .table(Alias::new("timezone_state"))
                    .if_not_exists()
                    .check(Expr::col(Alias::new("id")).eq(1))
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("active_timezone"))
                            .string()
                            .not_null(),
                    )
                    .col(helpers::created_at(manager))
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "INSERT INTO timezone_state (id, active_timezone) VALUES (1, 'UTC') ON CONFLICT (id) DO NOTHING",
            )
            .await?;
        helpers::grant(manager, "SELECT, UPDATE", "timezone_state").await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Alias::new("timezone_state")).to_owned())
            .await
    }
}
