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
                    .table(Alias::new("identity_stripe_processed_events"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("event_id"))
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("event_type")).text().not_null())
                    .col(ColumnDef::new(Alias::new("customer_id")).text())
                    .col(
                        ColumnDef::new(Alias::new("stripe_created"))
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(helpers::created_at())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("stripe_processed_events_customer_id_idx")
                    .table(Alias::new("identity_stripe_processed_events"))
                    .col(Alias::new("customer_id"))
                    .col(Alias::new("stripe_created"))
                    .and_where(Expr::col(Alias::new("customer_id")).is_not_null())
                    .to_owned(),
            )
            .await?;

        // yorishiro_app gets no GRANT at all here, matching identity_tenants/
        // identity_tenant_billing: this table is reached only through ctx.db (the migration-role
        // connection, the Stripe webhook handler's only DB access), never a tenant-scoped
        // request connection. No RLS either: unlike tenant_billing this table has no tenant_id
        // column to scope a policy on (it's keyed by Stripe's own event id), and it is never
        // reached by anything but ctx.db regardless.
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("identity_stripe_processed_events"))
                    .to_owned(),
            )
            .await
    }
}
