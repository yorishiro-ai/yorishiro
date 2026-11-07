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
            .table(Alias::new("system_maintenance"))
            .if_not_exists()
            .col(
                ColumnDef::new(Alias::new("id"))
                    .boolean()
                    .not_null()
                    .primary_key()
                    .default(true),
            )
            .col(
                ColumnDef::new(Alias::new("mode"))
                    .text()
                    .not_null()
                    .default("off"),
            )
            .col(
                ColumnDef::new(Alias::new("retry_after"))
                    .integer()
                    .not_null()
                    .default(300),
            )
            .col(ColumnDef::new(Alias::new("reason")).text())
            .col(
                helpers::ts_tz_col(manager, Alias::new("updated_at"))
                    .not_null()
                    .default(Expr::current_timestamp()),
            )
            .to_owned();

        helpers::create_table_with_checks(
            manager,
            "system_maintenance",
            table,
            &[
                ("maintenance_id_check", "id"),
                (
                    "maintenance_mode_check",
                    "mode IN ('off', 'read_only', 'full_lock')",
                ),
                ("maintenance_retry_after_check", "retry_after > 0"),
            ],
        )
        .await?;

        manager
            .get_connection()
            .execute(
                &Query::insert()
                    .into_table(Alias::new("system_maintenance"))
                    .columns([Alias::new("id")])
                    .values_panic([true.into()])
                    .to_owned(),
            )
            .await?;

        helpers::grant(manager, "SELECT", "system_maintenance").await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("system_maintenance"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
