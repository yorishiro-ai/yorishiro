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
        up_maintenance(manager).await?;
        up_reindex_schedules(manager).await?;
        up_tenant_retirement_events(manager).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        down_tenant_retirement_events(manager).await?;
        down_reindex_schedules(manager).await?;
        down_maintenance(manager).await?;
        Ok(())
    }
}

async fn up_maintenance(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let table = Table::create()
        .table(Alias::new("system_maintenance"))
        .if_not_exists()
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
        .col(
            helpers::ts_tz_col(manager, Alias::new("updated_at"))
                .not_null()
                .default(Expr::current_timestamp()),
        )
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
                .columns([Alias::new("id")])
                .values_panic([true.into()])
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

async fn up_reindex_schedules(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let [created_at, updated_at] = helpers::timestamps(manager);
    let ts_tz_col = helpers::ts_tz_col(manager, Alias::new("scheduled_for"));
    manager
        .create_table(
            Table::create()
                .table(Alias::new("tenant_reindex_schedules"))
                .if_not_exists()
                .col(
                    helpers::uuid_col(manager, Alias::new("tenant_id"))
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("interval")).text().not_null())
                .col(ColumnDef::new(Alias::new("timezone")).text())
                .col(ts_tz_col)
                .col(created_at)
                .col(updated_at)
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_tenant_reindex_schedules_tenant_id")
                        .from(
                            Alias::new("tenant_reindex_schedules"),
                            Alias::new("tenant_id"),
                        )
                        .to(Alias::new("tenant_tenants"), Alias::new("id"))
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;

    helpers::pg_only(
        manager,
        "REVOKE ALL ON TABLE tenant_reindex_schedules FROM PUBLIC;
                 GRANT SELECT ON TABLE tenant_reindex_schedules TO yorishiro_app;",
    )
    .await?;

    Ok(())
}

async fn down_reindex_schedules(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_table(
            Table::drop()
                .table(Alias::new("tenant_reindex_schedules"))
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

async fn down_tenant_retirement_events(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_table(
            Table::drop()
                .table(Alias::new("tenant_retirement_events"))
                .to_owned(),
        )
        .await
}
