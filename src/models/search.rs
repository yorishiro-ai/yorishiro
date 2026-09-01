//! Vector similarity search over `content_entities`.
//!
//! Two queries run here, not one.
//! pgvector's HNSW index serves exactly one shape (`ORDER BY embedding <=> $q LIMIT k`), and any other leading sort key takes it out of play.
//! Ranking vector hits ahead of trigram-only ones in a single statement needs `ORDER BY (embedding IS NULL), distance`, which makes the planner sort the whole workspace instead of using the index.
//! The two halves run separately and merge here.

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
    /// pgvector cosine distance (the `<=>` operator).
    /// Closer to 0 means more similar.
    /// `None` when the entity has no embedding and was only surfaced through the pg_trgm fuzzy text match on `query_text`.
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
/// On request paths, call this before acquiring a DB connection: embedding generation can take a long time (an external API call), and holding a connection while waiting would let pool exhaustion spill over onto unrelated endpoints.
pub async fn embed_query(
    provider: &dyn EmbeddingProvider,
    query_text: &str,
) -> Result<Vec<f32>, YorishiroError> {
    provider.embed_as(EmbedKind::Query, query_text).await
}

/// The `WHERE` fragment (and its bound values) both halves of the search apply, appended after `$3` (vector half) / no vector (trigram half) so callers pass in the params that come before it and get back the ones to append.
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

const HIT_COLUMNS: &str = "e.id, e.workspace_id, e.schema_id, e.schema_version, e.entity_type, \
     e.data, e.created_at, e.updated_at, e.created_by, e.updated_by";

/// Returns entities ordered by cosine distance between the given embedding vector and the `content_entities.embedding` column, closest first.
/// As an auxiliary path, entities with no embedding are also included when `query_text` is a pg_trgm fuzzy match (`data::text % query_text`) against their data.
/// Vector matches are always ranked ahead of trgm-only matches; trgm-only matches are ordered by similarity.
pub async fn search_by_vector(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    vector: Vec<f32>,
    query_text: &str,
    query: SearchQuery,
) -> Result<Vec<SearchHit>, YorishiroError> {
    let limit = query.limit.clamp(1, 200);

    let (scope_sql, scope_values) = scope_clause(workspace_id, &query, 2);
    let vector_sql = format!(
        "SELECT {HIT_COLUMNS}, (e.embedding <=> $1) AS distance \
         FROM content_entities e \
         WHERE e.embedding IS NOT NULL{scope_sql} \
         ORDER BY e.embedding <=> $1 \
         LIMIT {limit}"
    );
    let mut vector_values: Vec<sea_orm::Value> = vec![pgvector::Vector::from(vector).into()];
    vector_values.extend(scope_values);

    let mut rows = SearchRow::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        &vector_sql,
        vector_values,
    ))
    .all(conn)
    .await
    .internal()?;

    // Trigram half, for what the vector half cannot reach: entities with no embedding at all.
    // Only run when there is room left: a full page of vector hits already outranks every trigram-only match.
    if (rows.len() as i64) < limit {
        let remaining = limit - rows.len() as i64;
        let (scope_sql, mut scope_values) = scope_clause(workspace_id, &query, 2);
        let trigram_sql = format!(
            "SELECT {HIT_COLUMNS}, NULL::float8 AS distance \
             FROM content_entities e \
             WHERE e.embedding IS NULL{scope_sql} AND (e.data::text) % $1 \
             ORDER BY similarity(e.data::text, $1) DESC \
             LIMIT {remaining}"
        );
        let mut trigram_values: Vec<sea_orm::Value> = vec![query_text.into()];
        trigram_values.append(&mut scope_values);

        let trigram_rows = SearchRow::find_by_statement(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            &trigram_sql,
            trigram_values,
        ))
        .all(conn)
        .await
        .internal()?;
        rows.extend(trigram_rows);
    }

    Ok(rows.into_iter().map(SearchRow::into_hit).collect())
}
