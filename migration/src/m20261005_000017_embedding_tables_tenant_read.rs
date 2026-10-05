//! Let the tenant-scoped role read the width-partitioned embedding tables.
//!
//! `GET /api/search` and the MCP search tool run their vector query inside `TenantDb::begin_for_workspace`, which executes as `yorishiro_app`.
//! The width migration created `entity_embeddings_<width>` with no grant, so that query failed with "permission denied" on PostgreSQL.
//!
//! The grant is `SELECT` only: vectors are written by the embedding worker on the migration-role pool, never by a request connection.
//! The policy admits a vector row only when its entity is visible, and `entity_entities` already filters by `app.current_workspace`, so one workspace's vectors stay invisible to another.
//! The policy is raw SQL because `helpers::enable_rls_with_policy` expresses only a single-column equality.
//!
//! SQLite has no second role and no RLS, so this migration does nothing there.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DbBackend;

use super::m20260909_000001_embedding_width_partitions::{WIDTHS, table_name};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        crate::helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::Postgres {
            return Ok(());
        }
        for &width in WIDTHS {
            let table = table_name(width);
            manager
                .get_connection()
                .execute_unprepared(&format!(
                    "ALTER TABLE {table} ENABLE ROW LEVEL SECURITY;
                     CREATE POLICY workspace_isolation ON {table} FOR SELECT \
                     USING (EXISTS (SELECT 1 FROM entity_entities e WHERE e.id = {table}.entity_id));"
                ))
                .await?;
            crate::helpers::grant(manager, "SELECT", &table).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::Postgres {
            return Ok(());
        }
        for &width in WIDTHS {
            let table = table_name(width);
            manager
                .get_connection()
                .execute_unprepared(&format!(
                    "REVOKE SELECT ON {table} FROM yorishiro_app;
                     DROP POLICY IF EXISTS workspace_isolation ON {table};
                     ALTER TABLE {table} DISABLE ROW LEVEL SECURITY;"
                ))
                .await?;
        }
        Ok(())
    }
}
