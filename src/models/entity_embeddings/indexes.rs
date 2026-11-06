use sea_orm::{
    ColumnTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{entity_embeddings, workspace_workspaces};

/// Promotes large workspaces and removes obsolete or interrupted PostgreSQL ANN indexes.
/// SQLite has no HNSW indexes and is a no-op.
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
        let locked: bool = sea_orm::sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended('embedding-index-maintenance', 0))")
            .fetch_one(&mut conn).await.internal()?;
        if !locked { return Ok(()); }
        // SeaORM has no PostgreSQL index-catalog or concurrent HNSW DDL API.
        let existing: Vec<(String, bool)> = sea_orm::sqlx::query_as(
            "SELECT c.relname::text, i.indisvalid FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid JOIN pg_class t ON t.oid=i.indrelid JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_inherits h ON h.inhrelid=t.oid WHERE n.nspname='public' AND h.inhparent='entity_embeddings'::regclass AND c.relname LIKE 'ysr_hnsw_%'")
            .fetch_all(&mut conn).await.internal()?;
        let workspaces = workspace_workspaces::Entity::find().all(db).await.internal()?;
        let mut wanted = std::collections::HashSet::new();
        for workspace in workspaces {
            if workspace.embedding_reindexing {
                // Keep the old graph until atomic vector publication finishes.
                for (name, valid) in &existing {
                    if *valid && name.starts_with(&format!("ysr_hnsw_{}_d", workspace.id.simple())) { wanted.insert(name.clone()); }
                }
                continue;
            }
            let Some(dimensions @ 1..=4000) = workspace.embedding_dimensions else { continue; };
            let name = format!("ysr_hnsw_{}_d{dimensions}", workspace.id.simple());
            let valid = existing.iter().any(|(index, valid)| index == &name && *valid);
            let count = entity_embeddings::Entity::find()
                .select_only().column(entity_embeddings::Column::EntityId)
                .filter(entity_embeddings::Column::WorkspaceId.eq(workspace.id))
                .filter(entity_embeddings::Column::Embedding.is_not_null())
                .limit(2000).into_tuple::<Uuid>().all(db).await.internal()?.len();
            // Promotion is sticky: shrinking a workspace does not discard a valid graph.
            let promoted = existing.iter().any(|(index, _)| index.starts_with(&format!("ysr_hnsw_{}_d", workspace.id.simple())));
            if !valid && !promoted && count < 2000 { continue; }
            wanted.insert(name.clone());
            if valid { continue; }
            if existing.iter().any(|(index, _)| index == &name) {
                sea_orm::sqlx::query(AssertSqlSafe(format!("DROP INDEX CONCURRENTLY public.\"{name}\"")))
                    .execute(&mut conn).await.internal()?;
            }
            let partition: Option<String> = sea_orm::sqlx::query_scalar(
                "SELECT c.relname::text FROM entity_embeddings e JOIN pg_class c ON c.oid=e.tableoid JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_inherits h ON h.inhrelid=c.oid WHERE e.workspace_id=$1 AND h.inhparent='public.entity_embeddings'::regclass AND n.nspname='public' LIMIT 1")
                .bind(workspace.id).fetch_optional(&mut conn).await.internal()?;
            if let Some(partition) = partition {
                // Catalog membership identifies partitions; quote names independently of their spelling.
                let partition = partition.replace('"', "\"\"");
                sea_orm::sqlx::query("SET max_parallel_maintenance_workers=0").execute(&mut conn).await.internal()?;
                let expression = if dimensions > 2000 {
                    format!("(binary_quantize(embedding)::bit({dimensions})) bit_hamming_ops")
                } else {
                    format!("(embedding::vector({dimensions})) vector_cosine_ops")
                };
                sea_orm::sqlx::query(AssertSqlSafe(format!("CREATE INDEX CONCURRENTLY \"{name}\" ON public.\"{partition}\" USING hnsw ({expression}) WITH (m=32, ef_construction=128) WHERE workspace_id='{}'::uuid AND vector_dims(embedding)={dimensions}", workspace.id)))
                    .execute(&mut conn).await.internal()?;
            }
        }
        for (name, _) in existing {
            if wanted.contains(&name) { continue; }
            // Catalog names are not trusted as SQL identifiers.
            if name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
                sea_orm::sqlx::query(AssertSqlSafe(format!("DROP INDEX CONCURRENTLY public.\"{name}\"")))
                    .execute(&mut conn).await.internal()?;
            }
        }
        Ok(())
    }.await;
    use sea_orm::sqlx::Connection;
    let closed = conn.close().await.internal();
    outcome?;
    closed
}
