use super::helpers;
use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::prelude::DateTimeUtc;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        Box::pin(up_maintenance(manager)).await?;
        Box::pin(up_tenant_retirement_events(manager)).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        Box::pin(down_tenant_retirement_events(manager)).await?;
        Box::pin(down_maintenance(manager)).await?;
        Ok(())
    }
}

async fn up_maintenance(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let table = Table::create()
        .table(Alias::new("system_maintenance"))
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
        .col(helpers::ts_tz_col(manager, Alias::new("updated_at")).not_null())
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
                .columns([Alias::new("id"), Alias::new("updated_at")])
                .values_panic([
                    true.into(),
                    DateTimeUtc::from(std::time::SystemTime::now())
                        .fixed_offset()
                        .into(),
                ])
                .to_owned(),
        )
        .await?;

    helpers::grant(manager, "SELECT", "system_maintenance").await?;

    Ok(())
}

async fn down_maintenance(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_table(
            Table::drop()
                .table(Alias::new("system_maintenance"))
                .to_owned(),
        )
        .await?;

    Ok(())
}

async fn up_tenant_retirement_events(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
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
            "action IN ('disabled', 'delete_scheduled', 'deleted')",
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

async fn down_tenant_retirement_events(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_table(
            Table::drop()
                .table(Alias::new("tenant_retirement_events"))
                .to_owned(),
        )
        .await
}
