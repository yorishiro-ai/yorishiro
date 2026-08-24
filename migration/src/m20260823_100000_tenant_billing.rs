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
                    .table(Alias::new("identity_tenant_billing"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("tenant_id"))
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("plan")).text())
                    .col(
                        ColumnDef::new(Alias::new("stripe_customer_id"))
                            .text()
                            .unique_key(),
                    )
                    .col(created_at)
                    .col(updated_at)
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_tenant_billing_tenant_id")
                            .from(
                                Alias::new("identity_tenant_billing"),
                                Alias::new("tenant_id"),
                            )
                            .to(Alias::new("identity_tenants"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        let db = manager.get_connection();

        // yorishiro_app gets no GRANT at all here, matching identity_tenants: this table is reached only through ctx.db (the migration-role connection), never a tenant-scoped request connection.
        // Enabling RLS anyway is defense in depth against a future grant added without re-deriving this reasoning.
        helpers::enable_rls_with_policy(
            db,
            "identity_tenant_billing",
            "tenant_billing_isolation",
            "tenant_id",
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
                    .table(Alias::new("identity_tenant_billing"))
                    .to_owned(),
            )
            .await
    }
}
