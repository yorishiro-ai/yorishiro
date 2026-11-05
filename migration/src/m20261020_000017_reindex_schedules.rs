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
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("tenant_reindex_schedules"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
