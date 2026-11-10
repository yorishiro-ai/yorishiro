#![allow(elided_lifetimes_in_paths)]
#![allow(clippy::wildcard_imports)]
pub use sea_orm_migration::prelude::*;
mod helpers;
mod m20261020_000000_tenancy;
mod m20261020_000001_workspaces;
mod m20261020_000002_templates;
mod m20261020_000003_schemas;
mod m20261020_000004_api_keys;
mod m20261020_000005_entities;
mod m20261020_000006_entity_extras;
mod m20261020_000007_billing;
mod m20261020_000008_workspace_credentials;
mod m20261020_000009_operations;
mod m20261020_000010_workspace_schema_forks;
mod m20261020_000011_workspace_schema_fork_heads;
mod m20261020_000012_inference;
mod m20261020_000013_queue_jobs;
mod m20261020_000014_entity_embeddings;
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        Migrations::migrations()
    }

    async fn up<'c, C>(db: C, steps: Option<u32>) -> Result<(), DbErr>
    where
        C: sea_orm_migration::IntoSchemaManagerConnection<'c>,
    {
        use sea_orm_migration::sea_orm::{
            SqliteTransactionMode, TransactionOptions, TransactionTrait,
        };

        let db = db.into_database_executor();
        // The fence and migrations use one transaction, including through transaction-pooling proxies.
        let txn = db
            .begin_with_options(TransactionOptions {
                sqlite_transaction_mode: Some(SqliteTransactionMode::Immediate),
                ..Default::default()
            })
            .await?;
        if txn.get_database_backend() == sea_orm_migration::sea_orm::DbBackend::Postgres {
            txn.execute_unprepared(
                "SELECT pg_advisory_xact_lock(hashtextextended('yorishiro:boot-migrations', 0))",
            )
            .await?;
        }
        Migrations::up(&txn, steps).await?;
        txn.commit().await
    }
}

// Keeps the framework's migration execution without recursively entering our fence.
struct Migrations;

#[async_trait::async_trait]
impl MigratorTrait for Migrations {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20261020_000000_tenancy::Migration),
            Box::new(m20261020_000001_workspaces::Migration),
            Box::new(m20261020_000002_templates::Migration),
            Box::new(m20261020_000003_schemas::Migration),
            Box::new(m20261020_000004_api_keys::Migration),
            Box::new(m20261020_000005_entities::Migration),
            Box::new(m20261020_000006_entity_extras::Migration),
            Box::new(m20261020_000007_billing::Migration),
            Box::new(m20261020_000008_workspace_credentials::Migration),
            Box::new(m20261020_000009_operations::Migration),
            Box::new(m20261020_000010_workspace_schema_forks::Migration),
            Box::new(m20261020_000011_workspace_schema_fork_heads::Migration),
            Box::new(m20261020_000012_inference::Migration),
            Box::new(m20261020_000013_queue_jobs::Migration),
            Box::new(m20261020_000014_entity_embeddings::Migration),
            // inject-above (do not remove this comment)
        ]
    }
}
