//! Persist one semantic-search query embedding request and its result.
//!
//! The API and MCP server holds no embedding model.
//! It writes a pending row, enqueues a job that names the row, and polls the row until the worker stores a vector or the request times out.
//! The row carries the query text and the result vector, so it is short-lived: the server deletes a finished row when it consumes it, and an expired row is purged.
//!
//! The server reads and writes through the tenant pool, so PostgreSQL scopes the table by `app.current_workspace`.
//! The worker uses the migration-role pool, which bypasses that policy, and only completes rows that are still pending.
//! SQLite is single-tenant and has neither the policy nor the grant.

use super::helpers;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const TABLE: &str = "query_embedding_requests";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let [created_at, updated_at] = helpers::timestamps(manager);
        let table = Table::create()
            .table(Alias::new(TABLE))
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
            .col(helpers::ts_tz_col(manager, Alias::new("completed_at")))
            .col(created_at)
            .col(updated_at)
            .foreign_key(
                ForeignKey::create()
                    .name("fk_query_embedding_requests_workspace_id")
                    .from(Alias::new(TABLE), Alias::new("workspace_id"))
                    .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            )
            .to_owned();

        helpers::create_table_with_checks(
            manager,
            TABLE,
            table,
            &[(
                "query_embedding_requests_status_check",
                "status IN ('pending', 'succeeded', 'failed', 'expired')",
            )],
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("query_embedding_requests_workspace_id_idx")
                    .table(Alias::new(TABLE))
                    .col(Alias::new("workspace_id"))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("query_embedding_requests_expires_at_idx")
                    .table(Alias::new(TABLE))
                    .col(Alias::new("expires_at"))
                    .to_owned(),
            )
            .await?;

        helpers::enable_rls_with_policy(
            manager,
            TABLE,
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            false,
        )
        .await?;
        helpers::grant(manager, "SELECT, INSERT, UPDATE, DELETE", TABLE).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new(TABLE))
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}
