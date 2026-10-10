//! Allow one running infer-fill job per workspace, on both backends.
//!
//! The advisory lock that serialised a workspace's runs is a no-op on SQLite, which has no named locks.
//! A partial unique index is the same exclusion as a conditional update: moving a second job to `running` fails, and the worker defers it.
//!
//! Rows already in breach are resolved first, keeping the most recently updated running job per workspace and failing the rest, so the index can be built on a database that ran the old code.

use sea_orm_migration::prelude::*;

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
        connection
            .execute_unprepared(
                "UPDATE inference_jobs \
                    SET status = 'failed', error = 'superseded by another running job in the workspace' \
                  WHERE status = 'running' \
                    AND id NOT IN ( \
                        SELECT id FROM ( \
                            SELECT id, row_number() OVER ( \
                                PARTITION BY workspace_id ORDER BY updated_at DESC, id DESC) AS position \
                              FROM inference_jobs WHERE status = 'running') AS ranked \
                         WHERE position = 1)",
            )
            .await?;
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
