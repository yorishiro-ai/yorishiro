//! Vector similarity search over `content_entities`.
//!
//! Embeddings live in the separate `content_entity_embeddings` table (see
//! `migration/src/m20260903_000001_extract_embeddings_table.rs`), so every query here
//! joins `content_entities e` to that table when searching by vector.
//!
//! Two queries run for each backend: vector search first, then a trigram (PostgreSQL) or
//! LIKE (SQLite) fallback for entities with no embedding at all.
//! The two halves are merged in Rust.

use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::content_entities::EntityRecord;
use crate::services::embedding::{EmbedKind, EmbeddingProvider};

const DEFAULT_SEARCH_LIMIT: i64 = 10;

pub struct SearchQuery {
    pub entity_type: Option<String>,
    /// JSONB containment filter (`data @> filter`), e.g. `{"status": "active"}`.
    pub filter: Option<Value>,
    pub limit: i64,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            entity_type: None,
            filter: None,
            limit: DEFAULT_SEARCH_LIMIT,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SearchHit {
    pub entity: EntityRecord,
    /// Cosine distance (PostgreSQL `<=>` or sqlite-vec cosine distance).
    /// Closer to 0 means more similar.
    /// `None` when the entity has no embedding and was only surfaced through the
    /// pg_trgm / FTS fallback on `query_text`.
    pub distance: Option<f64>,
}

#[derive(FromQueryResult)]
struct SearchRow {
    id: Uuid,
    workspace_id: Uuid,
    schema_id: Uuid,
    schema_version: i32,
    entity_type: String,
    data: Value,
    created_at: chrono::DateTime<chrono::FixedOffset>,
    updated_at: chrono::DateTime<chrono::FixedOffset>,
    created_by: Option<Uuid>,
    updated_by: Option<Uuid>,
    distance: Option<f64>,
}

impl SearchRow {
    fn into_hit(self) -> SearchHit {
        SearchHit {
            entity: EntityRecord {
                id: self.id,
                workspace_id: self.workspace_id,
                schema_id: self.schema_id,
                schema_version: self.schema_version,
                entity_type: self.entity_type,
                data: self.data,
                created_at: self.created_at.into(),
                updated_at: self.updated_at.into(),
                created_by: self.created_by,
                updated_by: self.updated_by,
            },
            distance: self.distance,
        }
    }
}

/// Converts query text into an embedding vector; used together with [`search_by_vector`].
/// On request paths, call this before acquiring a DB connection: embedding generation can
/// take a long time (an external API call), and holding a connection while waiting would
/// let pool exhaustion spill over onto unrelated endpoints.
pub async fn embed_query(
    provider: &dyn EmbeddingProvider,
    query_text: &str,
) -> Result<Vec<f32>, YorishiroError> {
    provider.embed_as(EmbedKind::Query, query_text).await
}

/// The `WHERE` fragment (and its bound values) both halves of the search apply, appended
/// after `$2` (vector half) / no vector (trigram/LIKE half) so callers pass in the params
/// that come before it and get back the ones to append.
fn scope_clause(
    workspace_id: Uuid,
    query: &SearchQuery,
    next_param: usize,
) -> (String, Vec<sea_orm::Value>) {
    let mut sql = format!(" AND e.workspace_id = ${next_param}");
    let mut values: Vec<sea_orm::Value> = vec![workspace_id.into()];
    let mut n = next_param + 1;

    if let Some(entity_type) = &query.entity_type {
        sql.push_str(&format!(" AND e.entity_type = ${n}"));
        values.push(entity_type.clone().into());
        n += 1;
    }
    if let Some(filter) = &query.filter {
        sql.push_str(&format!(" AND e.data @> ${n}"));
        values.push(filter.clone().into());
    }

    (sql, values)
}

const HIT_COLUMNS: &str = "e.id, e.workspace_id, e.schema_id, e.schema_version, \
    e.entity_type, e.data, e.created_at, e.updated_at, e.created_by, e.updated_by";

/// The vector-search half of [`search_by_vector`]: find entities ordered by cosine
/// distance to a query vector.
///
/// PostgreSQL uses the HNSW `<=>` operator on `content_entity_embeddings.embedding`.
/// SQLite computes cosine distance on the raw LE f32 BLOB with `vec_distance_cosine`.
///
/// Both backends join `content_entity_embeddings` via `entity_id` (not rowid): VACUUM
/// may renumber implicit rowids, so we must never join on them (see
/// sqlite.org/lang_vacuum.html).
struct VectorKnn {
    /// The SQL query to run.
    pub sql: String,
    /// Values bound in order: `[vector_blob, workspace_id, entity_type?, filter?]`.
    pub values: Vec<sea_orm::Value>,
}

impl VectorKnn {
    fn postgres(vector: Vec<f32>, workspace_id: Uuid, query: &SearchQuery, limit: i64) -> Self {
        let (scope_sql, scope_values) = scope_clause(workspace_id, query, 2);
        let sql = format!(
            "SELECT {HIT_COLUMNS}, (ee.embedding <=> $1) AS distance \
             FROM content_entities e \
             JOIN content_entity_embeddings ee ON ee.entity_id = e.id \
             WHERE ee.embedding IS NOT NULL{scope_sql} \
             ORDER BY ee.embedding <=> $1 \
             LIMIT {limit}"
        );
        let mut values: Vec<sea_orm::Value> =
            vec![sea_orm::entity::prelude::PgVector::from(vector).into()];
        values.extend(scope_values);
        Self { sql, values }
    }

    fn sqlite(vector: Vec<f32>, workspace_id: Uuid, query: &SearchQuery, limit: i64) -> Self {
        // Convert the vector to raw LE f32 bytes for the BLOB column.
        let blob_bytes =
            unsafe { std::slice::from_raw_parts(vector.as_ptr() as *const u8, vector.len() * 4) };
        // Plain table, not vec0 virtual table: no MATCH/k = operators.
        // Full scan ordered by cosine distance — fine at current scale.
        // SQLite's Statement::from_sql_and_values only supports plain `?` placeholders.
        let mut sql = format!(
            "SELECT {HIT_COLUMNS}, \
             vec_distance_cosine(ee.embedding, ?) AS distance \
             FROM content_entities e \
             JOIN content_entity_embeddings ee ON ee.entity_id = e.id \
             WHERE ee.embedding IS NOT NULL AND e.workspace_id = ?"
        );
        let mut values: Vec<sea_orm::Value> = vec![
            sea_orm::Value::from(blob_bytes.to_vec()),
            workspace_id.into(),
        ];
        if let Some(entity_type) = &query.entity_type {
            sql.push_str(" AND e.entity_type = ?");
            values.push(entity_type.clone().into());
        }
        let sql = format!("{sql} ORDER BY distance LIMIT {limit}");
        Self { sql, values }
    }
}

/// Returns entities ordered by cosine distance between the given embedding vector and the
/// stored embedding, closest first.
/// As an auxiliary path, entities with no embedding are also included when `query_text` is
/// a pg_trgm fuzzy match (PostgreSQL) or LIKE match (SQLite) against their data.
/// Vector matches are always ranked ahead of trigram-only matches; trigram-only matches are
/// ordered by similarity.
pub async fn search_by_vector(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    vector: Vec<f32>,
    query_text: &str,
    query: SearchQuery,
) -> Result<Vec<SearchHit>, YorishiroError> {
    let limit = query.limit.clamp(1, 200);

    // SQLite has no JSONB containment operator to replace `data @> filter`.
    if query.filter.is_some() && conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return Err(YorishiroError::BackendUnsupported {
            message: "content filtering is not supported on SQLite (no JSONB @> operator)"
                .to_string(),
        });
    }

    let knn = match conn.get_database_backend() {
        sea_orm::DatabaseBackend::Postgres => {
            VectorKnn::postgres(vector, workspace_id, &query, limit)
        }
        sea_orm::DatabaseBackend::Sqlite => VectorKnn::sqlite(vector, workspace_id, &query, limit),
        // _ includes MySQL and any future backends: vector search is Postgres/SQLite only.
        _ => {
            return Err(YorishiroError::BackendUnsupported {
                message: "vector search is not supported on this database backend".to_string(),
            });
        }
    };

    // Both backends store UUIDs as BLOB (SQLite) or native uuid (PostgreSQL);
    // SearchRow decodes them directly on both.
    let mut hits: Vec<SearchHit> = SearchRow::find_by_statement(Statement::from_sql_and_values(
        conn.get_database_backend(),
        &knn.sql,
        knn.values,
    ))
    .all(conn)
    .await
    .internal()?
    .into_iter()
    .map(SearchRow::into_hit)
    .collect();

    // Only run when there is room left: a full page of vector hits already outranks
    // every trigram-only match.
    if (hits.len() as i64) < limit {
        let remaining = limit - hits.len() as i64;
        let (scope_sql, scope_values) = scope_clause(workspace_id, &query, 2);

        if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
            let like_pattern = query_text.replace('%', r"\%").replace('_', r"\_");
            let mut sql = format!(
                "SELECT {HIT_COLUMNS}, NULL AS distance \
                 FROM content_entities e \
                 LEFT JOIN content_entity_embeddings ee ON ee.entity_id = e.id \
                 WHERE ee.embedding IS NULL AND e.workspace_id = ? \
                   AND e.data LIKE ? ESCAPE '\\' \
                 LIMIT {remaining}"
            );
            let like_value = format!("%{like_pattern}%");
            let mut values: Vec<sea_orm::Value> = vec![workspace_id.into(), like_value.into()];
            if let Some(entity_type) = &query.entity_type {
                sql = sql.replace("LIMIT", "AND e.entity_type = ? LIMIT");
                values.insert(2, entity_type.clone().into());
            }

            let rows = SearchRow::find_by_statement(Statement::from_sql_and_values(
                conn.get_database_backend(),
                &sql,
                values,
            ))
            .all(conn)
            .await
            .internal()?;

            hits.extend(rows.into_iter().map(SearchRow::into_hit));
        } else {
            // PostgreSQL uses pg_trgm for fuzzy text matching.
            // `data::text` casts the JSONB column to text for trigram comparison.
            // `similarity()` returns a float between 0 and 1 for ranking.
            // LEFT JOIN to content_entity_embeddings so we can exclude entities that
            // already have an embedding (ee.embedding IS NULL).
            let trigram_sql = format!(
                "SELECT {HIT_COLUMNS}, NULL::float8 AS distance \
                 FROM content_entities e \
                 LEFT JOIN content_entity_embeddings ee ON ee.entity_id = e.id \
                 WHERE ee.embedding IS NULL{scope_sql} \
                   AND (e.data::text) % $1 \
                 ORDER BY similarity(e.data::text, $1) DESC \
                 LIMIT {remaining}"
            );
            let mut trigram_values: Vec<sea_orm::Value> = vec![query_text.into()];
            trigram_values.extend(scope_values);

            let trigram_rows = SearchRow::find_by_statement(Statement::from_sql_and_values(
                conn.get_database_backend(),
                &trigram_sql,
                trigram_values,
            ))
            .all(conn)
            .await
            .internal()?;

            hits.extend(
                trigram_rows
                    .into_iter()
                    .map(SearchRow::into_hit)
                    .collect::<Vec<_>>(),
            );
        }
    }

    Ok(hits)
}
