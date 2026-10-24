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
        let table = Table::create()
            .table(Alias::new(REQUESTS))
            .if_not_exists()
            .col(helpers::uuidv7_pk(manager))
            .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
            .col(ColumnDef::new(Alias::new("query_text")).text().not_null())
            .col(ColumnDef::new(Alias::new("status")).text().not_null())
            .col(helpers::json_col(manager, Alias::new("result_vector")))
            .col(ColumnDef::new(Alias::new("dimensions")).integer())
            .col(ColumnDef::new(Alias::new("model")).text())
            .col(ColumnDef::new(Alias::new("error")).text())
            .col(helpers::ts_tz_col(manager, Alias::new("expires_at")).not_null())
            .col(helpers::ts_tz_col(manager, Alias::new("deadline_at")))
            .col(helpers::ts_tz_col(manager, Alias::new("completed_at")))
            .col(created_at)
            .col(updated_at)
            .foreign_key(
                ForeignKey::create()
                    .name("fk_query_embedding_requests_workspace_id")
                    .from(Alias::new(REQUESTS), Alias::new("workspace_id"))
                    .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .to_owned();

        helpers::create_table_with_checks(
            manager,
            REQUESTS,
            table,
            &[(
                "query_embedding_requests_status_check",
                "status IN ('pending', 'succeeded', 'failed', 'expired')",
            )],
        )
        .await?;

        for (name, column) in [
            ("query_embedding_requests_workspace_id_idx", "workspace_id"),
            ("query_embedding_requests_expires_at_idx", "expires_at"),
        ] {
            manager
                .create_index(
                    Index::create()
                        .name(name)
                        .table(Alias::new(REQUESTS))
                        .col(Alias::new(column))
                        .to_owned(),
                )
                .await?;
        }

        helpers::enable_rls_with_policy(
            manager,
            REQUESTS,
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            false,
        )
        .await?;
        helpers::grant(manager, "SELECT, INSERT, UPDATE, DELETE", REQUESTS).await?;
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("query_embedding_requests"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
const REQUESTS: &str = "query_embedding_requests";
