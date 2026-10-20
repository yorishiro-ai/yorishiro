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
        let [created_at, updated_at] = helpers::timestamps(manager);
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("tenant_billing"))
                    .if_not_exists()
                    .col(
                        helpers::uuid_col(manager, Alias::new("tenant_id"))
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
                            .from(Alias::new("tenant_billing"), Alias::new("tenant_id"))
                            .to(Alias::new("tenant_tenants"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        helpers::enable_rls_with_policy(
            manager,
            "tenant_billing",
            "tenant_billing_isolation",
            "tenant_id",
            "app.current_tenant",
            false,
        )
        .await?;

        manager
            .create_table(
                Table::create()
                    .table(Alias::new("stripe_events"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("event_id"))
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("event_type")).text().not_null())
                    .col(ColumnDef::new(Alias::new("customer_id")).text())
                    .col(helpers::ts_tz_col(manager, Alias::new("stripe_created")).not_null())
                    .col(helpers::created_at(manager))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("stripe_events_customer_id_idx")
                    .if_not_exists()
                    .table(Alias::new("stripe_events"))
                    .col(Alias::new("customer_id"))
                    .col(Alias::new("stripe_created"))
                    .and_where(Expr::col(Alias::new("customer_id")).is_not_null())
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Alias::new("stripe_events")).to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(Alias::new("tenant_billing")).to_owned())
            .await?;

        Ok(())
    }
}
