use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let table = Table::create()
            .table(Alias::new("identity_maintenance"))
            .if_not_exists()
            // One row, enforced by the primary key.
            // Not the usual uuidv7_pk() shape: the PK is a boolean singleton, checked below to be exactly TRUE.
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
            // No `created_at`: this is a singleton row that exists from migration time, not a created record.
            .col(
                ColumnDef::new(Alias::new("updated_at"))
                    .timestamp_with_time_zone()
                    .not_null()
                    .default(Expr::current_timestamp()),
            )
            .to_owned();

        // CHECK (id): sea_query's ColumnDef has no boolean-must-be-true constraint, so this is a table-level constraint added right after create_table rather than a column modifier.
        // `mode` and `retry_after` CHECKs follow the same path for the same reason.
        helpers::create_table_with_checks(
            manager,
            "identity_maintenance",
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

        // The single seed row: `read_only` sheds writes, `full_lock` sheds everything but the health probes, and this row is what every request checks to decide which applies.
        manager
            .get_connection()
            .execute_unprepared("INSERT INTO identity_maintenance (id) VALUES (TRUE);")
            .await?;

        // No RLS on this table: it is a global singleton, not tenant- or workspace-scoped data.
        //
        // GRANT is SELECT only: yorishiro_app reads the maintenance flag on every request but never writes it (writes go through the migration-role/admin path instead).
        helpers::grant(manager, "SELECT", "identity_maintenance").await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("identity_maintenance"))
                    .to_owned(),
            )
            .await
    }
}
