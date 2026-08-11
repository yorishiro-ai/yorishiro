use sea_query::extension::postgres::{PgBinOper, PgExpr};
use sea_query::{Alias, BinOper, Expr, Func, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::entities::EntityRecord;
use crate::services::embedding::{EmbedKind, EmbeddingProvider};

pub use crate::models::search::*;

#[derive(Iden)]
enum Entities {
    Table,
    Id,
    WorkspaceId,
    SchemaId,
    SchemaVersion,
    EntityType,
    Data,
    Embedding,
    CreatedAt,
    UpdatedAt,
    CreatedBy,
    UpdatedBy,
}

#[derive(sqlx::FromRow)]
struct SearchRow {
    #[sqlx(flatten)]
    entity: EntityRecord,
    distance: Option<f64>,
}

impl SearchRow {
    fn into_hit(self) -> SearchHit {
        SearchHit {
            entity: self.entity,
            distance: self.distance,
        }
    }
}

/// Converts query text into an embedding vector; used together with `search_by_vector`. On
/// request paths, call this before acquiring a DB connection: embedding generation can take
/// a long time (external API calls or waiting on serialized local inference), and holding a
/// connection while waiting would let pool exhaustion spill over onto unrelated endpoints.
pub async fn embed_query(
    provider: &dyn EmbeddingProvider,
    query_text: &str,
) -> Result<Vec<f32>, YorishiroError> {
    provider.embed_as(EmbedKind::Query, query_text).await
}

/// Adds the shared filters both halves of the search apply.
fn scope_to_workspace(
    select: &mut sea_query::SelectStatement,
    workspace_id: Uuid,
    query: &SearchQuery,
) {
    select.and_where(Expr::col(Entities::WorkspaceId).eq(workspace_id));
    if let Some(entity_type) = &query.entity_type {
        select.and_where(Expr::col(Entities::EntityType).eq(entity_type.clone()));
    }
    if let Some(filter) = &query.filter {
        select.and_where(Expr::col(Entities::Data).contains(Expr::val(filter.clone())));
    }
}

/// The columns every hit carries, in the order `SearchRow` expects.
const HIT_COLUMNS: [Entities; 10] = [
    Entities::Id,
    Entities::WorkspaceId,
    Entities::SchemaId,
    Entities::SchemaVersion,
    Entities::EntityType,
    Entities::Data,
    Entities::CreatedAt,
    Entities::UpdatedAt,
    Entities::CreatedBy,
    Entities::UpdatedBy,
];

/// Returns entities ordered by cosine distance between the given embedding vector and the
/// `entities.embedding` column, closest first. As an auxiliary path, entities with no embedding
/// are also included when `query_text` is a pg_trgm fuzzy match (`data::text % query_text`)
/// against their data — this catches keyword/typo matches that vector search would miss (e.g.
/// entity_types with no `x-embed` field, or embedding generation that hasn't run yet). Vector
/// matches are always ranked ahead of trgm-only matches; trgm-only matches are ordered by
/// similarity.
///
/// **Two queries, not one.** pgvector's HNSW index serves exactly one shape —
/// `ORDER BY embedding <=> $q LIMIT k` — and any other leading sort key takes it out of play.
/// Ranking vector hits ahead of trigram-only ones in a single statement needs
/// `ORDER BY (embedding IS NULL), distance`, and that leading key made the planner sort the
/// whole workspace instead: measured as a `Seq Scan` over 5,000 rows where the same data with
/// the vector half on its own gives `Index Scan using entities_embedding_hnsw`. The two halves
/// therefore run separately and merge here, which keeps the ranking the doc comment promises
/// and lets the index do the work it exists for.
pub async fn search_by_vector(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    vector: Vec<f32>,
    query_text: &str,
    query: SearchQuery,
) -> Result<Vec<SearchHit>, YorishiroError> {
    let limit = query.limit.clamp(1, 200);

    let distance = Expr::col(Entities::Embedding).binary(
        BinOper::PgOperator(PgBinOper::CosineDistance),
        Expr::val(pgvector::Vector::from(vector)),
    );

    // Vector half: nothing but the distance in `ORDER BY`, so the HNSW index applies.
    let mut vector_select = Query::select();
    vector_select
        .columns(HIT_COLUMNS)
        .expr_as(distance.clone(), Alias::new("distance"))
        .from((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::Embedding).is_not_null())
        .order_by_expr(distance, Order::Asc)
        .limit(limit as u64);
    scope_to_workspace(&mut vector_select, workspace_id, &query);

    let (sql, values) = vector_select.build_sqlx(PostgresQueryBuilder);
    let mut rows = sqlx::query_as_with::<_, SearchRow, _>(&sql, values)
        .fetch_all(&mut *conn)
        .await
        .internal()?;

    // Trigram half, for what the vector half cannot reach: entities with no embedding at all.
    // Only run when there is room left — a full page of vector hits already outranks every
    // trigram-only match, so the second query would be work whose results are discarded.
    if rows.len() < limit as usize {
        let data_as_text = Expr::col(Entities::Data).cast_as(Alias::new("text"));
        let similarity = Func::cust(Alias::new("similarity"))
            .args([data_as_text.clone(), Expr::val(query_text).into()]);

        let mut trigram_select = Query::select();
        trigram_select
            .columns(HIT_COLUMNS)
            .expr_as(Expr::value(Option::<f64>::None), Alias::new("distance"))
            .from((Alias::new("content"), Entities::Table))
            .and_where(Expr::col(Entities::Embedding).is_null())
            .and_where(data_as_text.binary(
                BinOper::PgOperator(PgBinOper::Similarity),
                Expr::val(query_text),
            ))
            .order_by_expr(similarity.into(), Order::Desc)
            .limit(limit as u64 - rows.len() as u64);
        scope_to_workspace(&mut trigram_select, workspace_id, &query);

        let (sql, values) = trigram_select.build_sqlx(PostgresQueryBuilder);
        rows.extend(
            sqlx::query_as_with::<_, SearchRow, _>(&sql, values)
                .fetch_all(&mut *conn)
                .await
                .internal()?,
        );
    }

    Ok(rows.into_iter().map(SearchRow::into_hit).collect())
}

/// Composes `embed_query` + `search_by_vector`. Because this holds `conn` for the duration
/// of embedding generation, don't use it on request paths — reserve it for tests and batch
/// jobs where holding a connection isn't a problem (request handlers call `embed_query`
/// before acquiring a connection).
pub async fn search_by_text(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    provider: &dyn EmbeddingProvider,
    query_text: &str,
    query: SearchQuery,
) -> Result<Vec<SearchHit>, YorishiroError> {
    let vector = embed_query(provider, query_text).await?;
    search_by_vector(conn, workspace_id, vector, query_text, query).await
}

#[cfg(test)]
#[path = "../../../tests/repositories/search/mod.rs"]
mod tests;
