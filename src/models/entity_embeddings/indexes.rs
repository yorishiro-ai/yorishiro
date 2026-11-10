//! Per-workspace partial HNSW indexes on the `entity_embeddings` partitions.
//!
//! Every workspace at or above the promotion threshold gets one partial HNSW index on the
//! partition that holds its vectors, rebuilt when the workspace's stamped width changes.
//! Up to pgvector's full-precision limit the index expression is the original vector;
//! above it a normalized `halfvec`; above the half-precision limit the first 4,000 dimensions.
//! The index selects candidates; every search reranks them in original full-precision coordinates.
//!
//! Maintenance is opportunistic and idempotent: a second concurrent invocation skips on the
//! session advisory lock, and workspaces mid-reindex keep their current valid indexes until
//! atomic vector publication finishes.

use std::collections::HashSet;

use sea_orm::{DatabaseBackend, DatabaseConnection, FromQueryResult, Statement};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::entity_embeddings::{
    PGVECTOR_HNSW_HALFVEC_MAX_DIMS, PGVECTOR_HNSW_VECTOR_MAX_DIMS,
};

/// Name prefix of every index this maintenance creates and retires.
const INDEX_PREFIX: &str = "ysr_hnsw_";
/// A workspace earns an index once it stores at least this many vectors.
const PROMOTION_THRESHOLD: i64 = 2000;
/// HNSW graph degree and construction effort for every index this maintenance builds.
const HNSW_M: i32 = 32;
const HNSW_EF_CONSTRUCTION: i32 = 128;

/// One workspace row of the promotion aggregate: stamped width, reindex fence, stored vector
/// count, and whether the workspace already carries an owned index.
#[derive(FromQueryResult)]
struct CandidateRow {
    workspace_id: Uuid,
    dimensions: Option<i32>,
    reindexing: bool,
    embedded: i64,
    indexed: bool,
}

/// Promotes workspaces that crossed the threshold, reconciles indexes after width changes,
/// and retires the indexes of deleted or demoted workspaces.
/// SQLite has no HNSW indexes and is a no-op.
///
/// A second concurrent invocation skips: the maintenance session holds a session-scoped
/// advisory lock, and the skip is the documented outcome, not an error.
///
/// # Errors
/// Returns an error on catalog, connection, or concurrent index DDL failure.
pub async fn maintain_workspace_indexes(db: &DatabaseConnection) -> Result<(), YorishiroError> {
    use sea_orm::sqlx::AssertSqlSafe;
    if db.get_database_backend() != DatabaseBackend::Postgres {
        return Ok(());
    }
    // Concurrent index DDL cannot run in a transaction; close the detached session on every exit.
    let mut conn = db
        .get_postgres_connection_pool()
        .acquire()
        .await
        .internal()?
        .detach();
    let outcome: Result<(), YorishiroError> = async {
        let locked: bool = sea_orm::sqlx::query_scalar(
            "SELECT pg_try_advisory_lock(hashtextextended('embedding-index-maintenance', 0))",
        )
        .fetch_one(&mut conn)
        .await
        .internal()?;
        if !locked {
            return Ok(());
        }
        // One aggregate over workspaces, stored vectors, and the index catalog: only workspaces
        // above the threshold or already indexed come back, so a busy database does not pay a
        // per-workspace count for the workspaces that need nothing.
        let candidates = CandidateRow::find_by_statement(Statement::from_string(
            DatabaseBackend::Postgres,
            format!(
                "SELECT w.id AS workspace_id, w.embedding_dimensions AS dimensions, w.embedding_reindexing AS reindexing, count(e.embedding) AS embedded, bool_or(owned.index_name IS NOT NULL) AS indexed FROM workspace_workspaces w LEFT JOIN entity_embeddings e ON e.workspace_id = w.id AND e.embedding IS NOT NULL LEFT JOIN (SELECT c.relname AS index_name, i.indisvalid FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid JOIN pg_namespace n ON n.oid = c.relnamespace JOIN pg_inherits h ON h.inhrelid = i.indrelid WHERE n.nspname = 'public' AND h.inhparent = 'entity_embeddings'::regclass AND c.relname LIKE 'ysr_hnsw_%') owned ON owned.index_name LIKE 'ysr_hnsw_' || w.id::text || '_d%' GROUP BY w.id, w.embedding_dimensions, w.embedding_reindexing HAVING count(e.embedding) >= {PROMOTION_THRESHOLD} OR bool_or(owned.index_name IS NOT NULL)"
            ),
        ))
        .all(db)
        .await
        .internal()?;
        // Every owned index in the catalog, valid or not: the cleanup pass retires the ones no candidate wants.
        let existing: Vec<(String, bool)> = sea_orm::sqlx::query_as(
            "SELECT c.relname::text, i.indisvalid FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid JOIN pg_class t ON t.oid=i.indrelid JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_inherits h ON h.inhrelid=t.oid WHERE n.nspname='public' AND h.inhparent='entity_embeddings'::regclass AND c.relname LIKE 'ysr_hnsw_%'",
        )
        .fetch_all(&mut conn)
        .await
        .internal()?;
        let mut wanted = HashSet::new();
        for candidate in &candidates {
            let prefix = format!("{INDEX_PREFIX}{}_", candidate.workspace_id.simple());
            if candidate.reindexing {
                // Vectors are being swapped atomically: keep whatever is valid, build nothing.
                for (name, valid) in &existing {
                    if *valid && name.starts_with(&prefix) {
                        wanted.insert(name.clone());
                    }
                }
                continue;
            }
            let Some(dimensions) = candidate.dimensions else {
                // No declared width: no index is wanted; owned leftovers are retired in cleanup.
                continue;
            };
            if !(1..=PGVECTOR_HNSW_HALFVEC_MAX_DIMS as i32).contains(&dimensions) {
                continue;
            }
            if candidate.embedded < PROMOTION_THRESHOLD && !candidate.indexed {
                continue;
            }
            // Promotion is sticky: a promoted workspace keeps its index through shrinkage
            // until its stamped width changes to another value.
            let name = format!("{prefix}d{dimensions}");
            wanted.insert(name.clone());
            if existing.iter().any(|(index, valid)| index == &name && *valid) {
                continue;
            }
            if existing.iter().any(|(index, _)| index == &name) {
                // A failed concurrent build leaves the name behind as an invalid index; drop it before recreating.
                sea_orm::sqlx::query(AssertSqlSafe(format!(
                    "DROP INDEX CONCURRENTLY public.\"{name}\""
                )))
                .execute(&mut conn)
                .await
                .internal()?;
            }
            create_index(&mut conn, &name, candidate.workspace_id, dimensions as usize).await?;
        }
        for (name, _) in &existing {
            if wanted.contains(name) {
                continue;
            }
            // Catalog names are not trusted as SQL identifiers.
            if name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
                sea_orm::sqlx::query(AssertSqlSafe(format!(
                    "DROP INDEX CONCURRENTLY public.\"{name}\""
                )))
                .execute(&mut conn)
                .await
                .internal()?;
            }
        }
        Ok(())
    }
    .await;
    use sea_orm::sqlx::Connection;
    let closed = conn.close().await.internal();
    outcome?;
    closed
}

/// Creates the workspace's partial HNSW index on the partition that holds its vectors.
///
/// The partition is resolved through the catalog, not the `p0..p63` naming convention:
/// an attached partition may carry any name.
///
/// # Errors
/// Returns an error on catalog, connection, or concurrent index DDL failure.
async fn create_index(
    conn: &mut sea_orm::sqlx::PgConnection,
    name: &str,
    workspace_id: Uuid,
    dimensions: usize,
) -> Result<(), YorishiroError> {
    use sea_orm::sqlx::AssertSqlSafe;
    let partition: Option<String> = sea_orm::sqlx::query_scalar(
        "SELECT c.relname::text FROM entity_embeddings e JOIN pg_class c ON c.oid=e.tableoid JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_inherits h ON h.inhrelid=c.oid WHERE e.workspace_id=$1 AND h.inhparent='public.entity_embeddings'::regclass AND n.nspname='public' LIMIT 1",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *conn)
    .await
    .internal()?;
    let Some(partition) = partition else {
        return Ok(());
    };
    // Catalog membership identifies partitions; quote names independently of their spelling.
    let partition = partition.replace('"', "\"\"");
    sea_orm::sqlx::query("SET max_parallel_maintenance_workers=0")
        .execute(&mut *conn)
        .await
        .internal()?;
    let expression = index_expression(dimensions);
    sea_orm::sqlx::query(AssertSqlSafe(format!(
        "CREATE INDEX CONCURRENTLY \"{name}\" ON public.\"{partition}\" USING hnsw ({expression}) WITH (m={HNSW_M}, ef_construction={HNSW_EF_CONSTRUCTION}) WHERE workspace_id='{workspace_id}'::uuid AND vector_dims(embedding)={dimensions}"
    )))
    .execute(&mut *conn)
    .await
    .internal()?;
    Ok(())
}

/// The indexed expression for a workspace width, per pgvector's per-type dimension limits.
///
/// Up to the full-precision limit the original vector is indexed directly.
/// Above it a normalized `halfvec`: normalizing keeps every component inside the half-float
/// range, and cosine ranking is invariant under normalization.
/// Above the half-precision limit the first 4,000 dimensions carry the index; the search
/// reranks candidates in original full-precision coordinates.
fn index_expression(dimensions: usize) -> String {
    if dimensions <= PGVECTOR_HNSW_VECTOR_MAX_DIMS {
        return format!("(embedding::vector({dimensions})) vector_cosine_ops");
    }
    let indexed_dims = dimensions.min(PGVECTOR_HNSW_HALFVEC_MAX_DIMS);
    if indexed_dims == dimensions {
        format!("(l2_normalize(embedding)::halfvec({dimensions})) halfvec_cosine_ops")
    } else {
        format!(
            "(l2_normalize(subvector(embedding, 1, {indexed_dims}))::halfvec({indexed_dims})) halfvec_cosine_ops"
        )
    }
}
