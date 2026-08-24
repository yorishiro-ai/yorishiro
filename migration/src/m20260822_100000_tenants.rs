use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // Idempotent (`duplicate_object` swallowed): every `GRANT ... TO yorishiro_app` in a later migration, and every RLS-scoped connection's `after_connect` (`src/db.rs`), requires this role to already exist.
        // PostgreSQL 16+ doesn't let even the role's creator `SET ROLE` to it automatically, so the migration also grants membership to itself right after creating it.
        // A superuser migration role masks a missing grant here (it can `SET ROLE` regardless of membership), so verify this on a non-superuser role.
        db.execute_unprepared(
            "DO $$ BEGIN \
             CREATE ROLE yorishiro_app NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION \
             NOBYPASSRLS NOLOGIN; \
             EXCEPTION WHEN duplicate_object THEN NULL; END $$; \
             GRANT yorishiro_app TO CURRENT_USER;",
        )
        .await?;

        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_tenants"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk())
                    .col(ColumnDef::new(Alias::new("name")).text().not_null())
                    .col(ColumnDef::new(Alias::new("max_workspaces")).integer())
                    .col(helpers::created_at())
                    .to_owned(),
            )
            .await?;

        // yorishiro_app gets no GRANT at all here: this table is reached only through the migration-role identity pool (see src/db.rs), never a tenant-scoped request connection.
        // Enabling RLS anyway is defense in depth against a future grant added without re-deriving this reasoning.
        helpers::enable_rls_with_policy(
            db,
            "identity_tenants",
            "tenant_isolation",
            "id",
            "app.current_tenant",
            false,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("identity_tenants"))
                    .to_owned(),
            )
            .await
    }
}
