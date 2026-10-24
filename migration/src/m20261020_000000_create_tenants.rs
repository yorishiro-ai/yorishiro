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
        helpers::pg_only(
            manager,
            "DO $$ BEGIN \
                 CREATE ROLE yorishiro_app NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION \
                 NOBYPASSRLS NOLOGIN; \
                 EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$; \
                 GRANT yorishiro_app TO CURRENT_USER;",
        )
        .await?;

        manager
            .create_table(
                Table::create()
                    .table(Alias::new("tenant_tenants"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(ColumnDef::new(Alias::new("name")).text().not_null())
                    .col(ColumnDef::new(Alias::new("max_workspaces")).integer())
                    .col(helpers::created_at(manager))
                    .col(ColumnDef::new(Alias::new("embedding_model")).text())
                    .col(ColumnDef::new(Alias::new("embedding_dimensions")).integer())
                    .to_owned(),
            )
            .await?;

        helpers::enable_rls_with_policy(
            manager,
            "tenant_tenants",
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
            .drop_table(Table::drop().table(Alias::new("tenant_tenants")).to_owned())
            .await?;

        Ok(())
    }
}
