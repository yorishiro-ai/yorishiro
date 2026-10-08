//! Schema for embedding work that outlives one request or one process.
//!
//! - `workspace_workspaces.embedding_reindexing`: tracks whether workspace vectors are being replaced.
//! - `query_embedding_requests`: one semantic-search query embedding request and its result.
//!   The API and MCP server holds no embedding model, so it writes a pending row, enqueues a job that names the row, and polls the row until the worker stores a vector or the request times out.
//!   The server reads and writes through the tenant pool, so PostgreSQL scopes the table by `app.current_workspace`.
//!   The worker uses the migration-role pool, which bypasses that policy, and only completes rows that are still pending.
//!   SQLite is single-tenant and has neither the policy nor the grant.
//! - `queue_startup_reindex_active_workspace_idx`: stops two processes from admitting an active reindex for one workspace.
//! - `queue_job_dispatch_outbox`: stores enough worker arguments to recover queue dispatches after process or provider failures.
//!
//! The column and the new tables are two migrations on purpose.
//! A SQLite pool connection that was already open when a lone `ADD COLUMN` committed keeps compiling statements against the old schema, so the `RETURNING "embedding_reindexing"` of an insert reads as a string literal and the decoded row has no such column.
//! Migrations are spread over the pool's connections, and each one that starts afterwards reloads its connection's schema, so a second migration after the column is what keeps every connection current.

use super::helpers;
use sea_orm_migration::prelude::*;

/// Adds `workspace_workspaces.embedding_reindexing`.
#[derive(DeriveMigrationName)]
pub struct Migration;

/// Creates the tables and the index; named by hand because the derive names a migration after its file.
pub struct QueueState;

impl MigrationName for QueueState {
    fn name(&self) -> &str {
        "m20261006_000019_embedding_queue_state"
    }
}

const REQUESTS: &str = "query_embedding_requests";
const OUTBOX: &str = "queue_job_dispatch_outbox";
const ACTIVE_REINDEX_INDEX: &str = "queue_startup_reindex_active_workspace_idx";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("workspace_workspaces"))
                    .add_column(
                        ColumnDef::new(Alias::new("embedding_reindexing"))
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("workspace_workspaces"))
                    .drop_column(Alias::new("embedding_reindexing"))
                    .to_owned(),
            )
            .await
    }
}

#[async_trait::async_trait]
impl MigrationTrait for QueueState {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        create_query_embedding_requests(manager).await?;
        create_active_reindex_index(manager).await?;
        create_dispatch_outbox(manager).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new(OUTBOX))
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name(ACTIVE_REINDEX_INDEX)
                    .table(Alias::new("queue_job_lifecycles"))
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new(REQUESTS))
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

async fn create_query_embedding_requests(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
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
    helpers::grant(manager, "SELECT, INSERT, UPDATE, DELETE", REQUESTS).await
}

async fn create_active_reindex_index(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_index(
            Index::create()
                .name(ACTIVE_REINDEX_INDEX)
                .unique()
                .table(Alias::new("queue_job_lifecycles"))
                .col(Alias::new("job_name"))
                .col(Alias::new("workspace_id"))
                .and_where(
                    Expr::col(Alias::new("job_name"))
                        .eq("startup_reindex")
                        .and(Expr::col(Alias::new("workspace_id")).is_not_null())
                        .and(
                            Expr::col(Alias::new("status"))
                                .is_in(["queued", "running", "retrying"]),
                        ),
                )
                .to_owned(),
        )
        .await
}

async fn create_dispatch_outbox(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(Alias::new(OUTBOX))
                .if_not_exists()
                .col(helpers::uuid_col(manager, Alias::new("lifecycle_id")).primary_key())
                .col(helpers::json_col(manager, Alias::new("payload")).not_null())
                .col(
                    ColumnDef::new(Alias::new("worker_name"))
                        .string()
                        .not_null(),
                )
                .col(ColumnDef::new(Alias::new("queue_name")).string())
                .col(helpers::json_col(manager, Alias::new("tags")))
                .col(helpers::ts_tz_col(manager, Alias::new("last_attempt_at")))
                .to_owned(),
        )
        .await
}
