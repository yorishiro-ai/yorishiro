//! Standalone index on `tenant_memberships.user_id` for issue #334.
//!
//! `tenant_memberships` has a UNIQUE index on `(tenant_id, user_id)`,
//! which does not help queries that filter on `user_id` alone.
//! `list_workspaces_for_user` (`src/models/tenancy.rs`) resolves all
//! memberships for a user across tenants — a `WHERE user_id = ...`
//! filter without `tenant_id`. This index satisfies that pattern.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                sea_orm_migration::sea_query::Index::create()
                    .name("idx_tenant_memberships_user_id")
                    .table(Alias::new("tenant_memberships"))
                    .col(Alias::new("user_id"))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                sea_orm_migration::sea_query::Index::drop()
                    .name("idx_tenant_memberships_user_id")
                    .table(Alias::new("tenant_memberships"))
                    .to_owned(),
            )
            .await
    }
}
