//! Allow one running infer-fill job per workspace, on both backends.
//!
//! The advisory lock that serialised a workspace's runs is a no-op on SQLite, which has no named locks.
//! A partial unique index is the same exclusion as a conditional update: moving a second job to `running` fails, and the worker defers it.
//!
//! Duplicate running jobs abort the migration without changing their state: drain workers and resolve duplicates before retrying.
//! The transaction holds a PostgreSQL SHARE ROW EXCLUSIVE lock or SQLite's writer lock through validation and index creation, preventing a new claim between them.
//! SQLite acquires that lock with a zero-row UPDATE; it changes no job and avoids upgrading a read snapshot after validation.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{DbBackend, Statement};

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        helpers::pg_only(
            manager,
            "LOCK TABLE inference_jobs IN SHARE ROW EXCLUSIVE MODE",
        )
        .await?;
        helpers::sqlite_only(
            manager,
            "UPDATE inference_jobs SET status = status WHERE 1 = 0",
        )
        .await?;
        let backend = connection.get_database_backend();
        let workspace = if backend == DbBackend::Postgres {
            "workspace_id::text"
        } else {
            "hex(workspace_id)"
        };
        let duplicates = connection
            .query_all_raw(Statement::from_string(
                backend,
                format!(
                    "SELECT {workspace} AS workspace, COUNT(*) AS jobs FROM inference_jobs \
             WHERE status = 'running' GROUP BY workspace_id HAVING COUNT(*) > 1"
                ),
            ))
            .await?;
        if !duplicates.is_empty() {
            let details = duplicates
                .iter()
                .map(|row| {
                    Ok(format!(
                        "{} ({} running jobs)",
                        row.try_get::<String>("", "workspace")?,
                        row.try_get::<i64>("", "jobs")?
                    ))
                })
                .collect::<Result<Vec<_>, DbErr>>()?
                .join(", ");
            return Err(DbErr::Custom(format!(
                "duplicate running infer-fill jobs: {details}; drain workers and resolve duplicates before retrying migration 000020"
            )));
        }
        connection
            .execute_unprepared(
                "CREATE UNIQUE INDEX inference_jobs_one_running_per_workspace_idx \
                    ON inference_jobs (workspace_id) WHERE status = 'running'",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS inference_jobs_one_running_per_workspace_idx")
            .await?;
        Ok(())
    }
}
