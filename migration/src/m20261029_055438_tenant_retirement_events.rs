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
        // No cascading foreign keys: retirement evidence outlives the tenant and its users.
        let table = Table::create()
            .table(Alias::new("tenant_retirement_events"))
            .col(helpers::uuidv7_pk(manager))
            .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
            .col(helpers::uuid_col(manager, Alias::new("actor_user_id")))
            .col(ColumnDef::new(Alias::new("action")).text().not_null())
            .col(helpers::created_at(manager))
            .to_owned();
        helpers::create_table_with_checks(
            manager,
            "tenant_retirement_events",
            table,
            &[(
                "tenant_retirement_events_action_check",
                "action IN ('disabled', 'deleted')",
            )],
        )
        .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_tenant_retirement_events_tenant_id")
                    .table(Alias::new("tenant_retirement_events"))
                    .col(Alias::new("tenant_id"))
                    .to_owned(),
            )
            .await?;
        // Operator-only control-plane evidence is never granted to the tenant role.
        helpers::enable_rls_with_policy(
            manager,
            "tenant_retirement_events",
            "tenant_isolation",
            "tenant_id",
            "app.current_tenant",
            false,
        )
        .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("tenant_retirement_events"))
                    .to_owned(),
            )
            .await
    }
}
