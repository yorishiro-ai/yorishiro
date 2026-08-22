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
                    .table(Alias::new("identity_tenants"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk())
                    .col(ColumnDef::new(Alias::new("name")).text().not_null())
                    .col(ColumnDef::new(Alias::new("max_workspaces")).integer())
                    .col(helpers::created_at())
                    .to_owned(),
            )
            .await?;

        let db = manager.get_connection();
        // yorishiro_app gets no GRANT at all here: this table is reached only through the
        // migration-role identity pool (see src/db.rs), never a tenant-scoped request
        // connection. Enabling RLS anyway is defense in depth against a future grant added
        // without re-deriving this reasoning.
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
            .drop_table(Table::drop().table(Alias::new("identity_tenants")).to_owned())
            .await
    }
}
