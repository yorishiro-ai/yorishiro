use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    /// One row per recorded call phase (`started`, `authenticated`, `completed`); the application only appends and reads.
    /// `tenant_id` has no foreign key so a retired tenant's trail survives the tenant.
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let mut read_only = ColumnDef::new(Alias::new("read_only"));
        read_only.boolean().not_null();
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("api_events"))
                    .col(helpers::uuidv7_pk(manager))
                    .col(ColumnDef::new(Alias::new("request_id")).text().not_null())
                    .col(ColumnDef::new(Alias::new("phase")).text().not_null())
                    .col(helpers::uuid_col(manager, Alias::new("tenant_id")))
                    .col(helpers::uuid_col(manager, Alias::new("access_key_id")))
                    .col(helpers::uuid_col(manager, Alias::new("user_id")))
                    .col(helpers::uuid_col(manager, Alias::new("workspace_id")))
                    .col(helpers::uuid_col(manager, Alias::new("job_id")))
                    .col(ColumnDef::new(Alias::new("caller")).text())
                    .col(ColumnDef::new(Alias::new("transport")).text().not_null())
                    .col(ColumnDef::new(Alias::new("event_name")).text().not_null())
                    .col(&mut read_only)
                    .col(ColumnDef::new(Alias::new("source_ip")).text())
                    .col(ColumnDef::new(Alias::new("user_agent")).text().not_null())
                    .col(helpers::json_col(manager, Alias::new("permissions")).not_null())
                    .col(ColumnDef::new(Alias::new("identity_type")).text().not_null())
                    .col(ColumnDef::new(Alias::new("correlation_id")).text().not_null())
                    .col(helpers::json_col(manager, Alias::new("resources")).not_null())
                    .col(ColumnDef::new(Alias::new("outcome")).text())
                    .col(ColumnDef::new(Alias::new("error_code")).text())
                    .col(ColumnDef::new(Alias::new("error_message")).text())
                    .col(ColumnDef::new(Alias::new("attempt")).integer())
                    .col(ColumnDef::new(Alias::new("status_code")).integer())
                    .col(ColumnDef::new(Alias::new("latency_ms")).big_integer())
                    .col(helpers::created_at(manager))
                    .check((
                        Alias::new("ck_api_events_phase"),
                        Expr::cust("phase IN ('started', 'authenticated', 'completed')"),
                    ))
                    .check((
                        Alias::new("ck_api_events_outcome"),
                        Expr::cust(
                            "outcome IS NULL OR outcome IN ('success', 'denied', 'failed', 'cancelled', 'unavailable')",
                        ),
                    ))
                    .check((
                        Alias::new("ck_api_events_identity_type"),
                        Expr::cust("identity_type IN ('anonymous', 'access_key', 'service', 'operator')"),
                    ))
                    .to_owned(),
            )
            .await?;
        for (name, columns) in [
            ("idx_api_events_tenant_created", ["tenant_id", "created_at"]),
            ("idx_api_events_request", ["request_id", "created_at"]),
            (
                "idx_api_events_correlation",
                ["correlation_id", "created_at"],
            ),
        ] {
            let mut index = Index::create();
            index.name(name).table(Alias::new("api_events"));
            for column in columns {
                index.col(Alias::new(column));
            }
            manager.create_index(index.clone()).await?;
        }
        helpers::enable_rls_with_policy(
            manager,
            "api_events",
            "tenant_isolation",
            "tenant_id",
            "app.current_tenant",
            true,
        )
        .await?;
        helpers::grant(manager, "SELECT", "api_events").await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Alias::new("api_events")).to_owned())
            .await
    }
}
