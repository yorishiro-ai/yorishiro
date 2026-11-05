//! Vector similarity search over `entity_entities`.
//!
//! Embeddings live in the separate `entity_embeddings` table, so every query here
//! joins `entity_entities e` to that table when searching by vector.
//!
//! Vector search runs first, followed by a literal text fallback for unembedded entities.
//! PostgreSQL uses NFKC-normalized ILIKE with a trigram expression index.
//! SQLite uses quoted FTS5 trigram terms and cannot match terms shorter than three characters.
//! The two halves are merged in Rust.
//!
//! Embedding generation follows two paths.
//! Document embedding is asynchronous: the embedding sync worker (`workers::embedding_sync`) generates and stores vectors after entity writes.
//! Query embedding is not done here: the server holds no model, so `workers::query_embedding` asks a worker for the vector before any DB connection is held for the search.
//! Both use the same provider, so documents and queries share one vector space.

use sea_orm::sea_query::{
    Alias, Expr, ExprTrait, Func, JoinType, Order, Query, SelectStatement,
    extension::postgres::{PgBinOper, PgExpr},
};
use sea_orm::{ConnectionTrait, FromQueryResult, TransactionSession, TransactionTrait};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::entity_entities::EntityRecord;

const DEFAULT_SEARCH_LIMIT: i64 = 10;
pub(crate) const MIN_SEARCH_LIMIT: i64 = 1;
pub(crate) const MAX_SEARCH_LIMIT: i64 = 200;

#[derive(Clone)]
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

pub(crate) async fn ensure_available(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    if super::workspace_workspaces::embedding_reindexing(db, workspace_id).await? {
        return Err(YorishiroError::BackendUnavailable {
            message: "semantic search is temporarily unavailable while workspace embeddings are being reindexed".into(),
        });
    }
    Ok(())
}

pub(crate) async fn search_text(
    ctx: &loco_rs::app::AppContext,
    tenant_id: Uuid,
    workspace_id: Uuid,
    text: &str,
    query: SearchQuery,
) -> Result<Vec<SearchHit>, YorishiroError> {
    // Wait for the worker without holding a workspace connection.
    let vector =
        crate::workers::query_embedding::embed_query(ctx, tenant_id, workspace_id, text).await?;
    check_query_width(&ctx.db, workspace_id, vector.len()).await?;
    let txn = crate::db::begin_workspace(ctx, tenant_id, workspace_id).await?;
    search_by_vector(&txn, workspace_id, vector, text, query).await
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SearchHit {
    pub entity: EntityRecord,
    /// Cosine distance (PostgreSQL `<=>` or sqlite-vec cosine distance).
    /// Closer to 0 means more similar.
    /// `None` when the entity has no embedding and was only surfaced through the
    /// pg_trgm / FTS fallback on `query_text`.
    pub distance: Option<f64>,
}

#[derive(FromQueryResult)]
pub(crate) struct SearchRow {
    #[sea_orm(nested)]
    entity: EntityRecord,
    distance: Option<f64>,
}

impl SearchRow {
    pub(crate) fn into_hit(self) -> SearchHit {
        SearchHit {
            entity: self.entity,
            distance: self.distance,
        }
    }
}

/// Shared projection and workspace scope for vector and text search.
fn scoped_search(workspace_id: Uuid, query: &SearchQuery) -> SelectStatement {
    use crate::models::_entities::entity_entities::{Column, Entity};
    let mut select = Query::select();
    select
        .columns(
            [
                Column::Id,
                Column::WorkspaceId,
                Column::SchemaId,
                Column::SchemaVersion,
                Column::EntityType,
                Column::Data,
                Column::CreatedAt,
                Column::UpdatedAt,
                Column::CreatedBy,
                Column::UpdatedBy,
            ]
            .map(|column| (Alias::new("e"), column)),
        )
        .from_as(Entity, Alias::new("e"))
        .and_where(Expr::col((Alias::new("e"), Column::WorkspaceId)).eq(workspace_id));
    if let Some(entity_type) = &query.entity_type {
        select.and_where(Expr::col((Alias::new("e"), Column::EntityType)).eq(entity_type));
    }
    if let Some(filter) = &query.filter {
        select.and_where(
            Expr::col((Alias::new("e"), Column::Data)).contains(Expr::val(filter.clone())),
        );
    }
    let reindexing = Query::select()
        .expr(Expr::val(1))
        .from_as(Alias::new("workspace_workspaces"), Alias::new("w"))
        .and_where(
            Expr::col((Alias::new("w"), Alias::new("id")))
                .equals((Alias::new("e"), Column::WorkspaceId)),
        )
        .and_where(Expr::col((Alias::new("w"), Alias::new("embedding_reindexing"))).eq(true))
        .to_owned();
    select.and_where(Expr::exists(reindexing).not());
    select
}

fn join_embedding(select: &mut SelectStatement, table: &str, join: JoinType) {
    select.join_as(
        join,
        Alias::new(table),
        Alias::new("ee"),
        Expr::col((Alias::new("ee"), Alias::new("entity_id")))
            .equals((Alias::new("e"), Alias::new("id")))
            .and(
                Expr::col((Alias::new("ee"), Alias::new("workspace_id")))
                    .equals((Alias::new("e"), Alias::new("workspace_id"))),
            ),
    );
}

/// Validates a query against the workspace embedding dimensions.
pub(crate) async fn check_query_width(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    vector_len: usize,
) -> Result<(), YorishiroError> {
    use super::_entities::workspace_workspaces::{Column, Entity};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
    let dimensions = Entity::find()
        .select_only()
        .column(Column::EmbeddingDimensions)
        .filter(Column::Id.eq(workspace_id))
        .into_tuple::<Option<i32>>()
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("workspace {workspace_id} not found")))?;
    if let Some(expected) = dimensions
        && usize::try_from(expected).ok() != Some(vector_len)
    {
        return Err(YorishiroError::ValidationFailed {
            message: format!("workspace expects {expected} dimensions, query has {vector_len}"),
            details: vec![],
            hint: "reindex the workspace with the selected embedding provider".into(),
        });
    }
    if vector_len == 0 {
        return Err(YorishiroError::ValidationFailed {
            message: "query embedding must not be empty".into(),
            details: vec![],
            hint: "configure a provider that returns a nonempty vector".into(),
        });
    }
    Ok(())
}

/// Searches workspace vectors by cosine distance.
///
/// # Errors
/// Returns an error when the query or workspace embedding state is invalid.
pub async fn search_by_vector(
    conn: &(impl ConnectionTrait + TransactionTrait),
    workspace_id: Uuid,
    vector: Vec<f32>,
    query_text: &str,
    query: SearchQuery,
) -> Result<Vec<SearchHit>, YorishiroError> {
    if conn.get_database_backend() == sea_orm::DatabaseBackend::Postgres {
        let txn = conn.begin().await.internal()?;
        let ef_search = std::cmp::max(query.limit.clamp(MIN_SEARCH_LIMIT, MAX_SEARCH_LIMIT), 200);
        // SeaORM cannot express transaction-local PostgreSQL settings.
        txn.execute_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT set_config('plan_cache_mode', 'force_custom_plan', true), set_config('hnsw.ef_search', $1, true), set_config('hnsw.iterative_scan', 'relaxed_order', true)",
            [ef_search.to_string().into()],
        )).await.internal()?;
        let hits = search_vectors(&txn, workspace_id, vector, query_text, query).await?;
        // Roll back the read-only savepoint so settings do not leak to an outer RLS transaction.
        txn.rollback().await.internal()?;
        return Ok(hits);
    }
    search_vectors(conn, workspace_id, vector, query_text, query).await
}

async fn search_vectors(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    vector: Vec<f32>,
    query_text: &str,
    query: SearchQuery,
) -> Result<Vec<SearchHit>, YorishiroError> {
    super::entity_embeddings::validate_vector(&vector, vector.len())?;
    check_query_width(conn, workspace_id, vector.len()).await?;
    ensure_available(conn, workspace_id).await?;
    let embed_table = super::entity_embeddings::TABLE;
    let limit = query.limit.clamp(MIN_SEARCH_LIMIT, MAX_SEARCH_LIMIT);

    // SQLite has no JSONB containment operator to replace `data @> filter`.
    if query.filter.is_some() && conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        return Err(YorishiroError::BackendUnsupported {
            message: "content filtering is not supported on SQLite (no JSONB @> operator)"
                .to_string(),
        });
    }

    let sqlite = match conn.get_database_backend() {
        sea_orm::DatabaseBackend::Postgres => false,
        sea_orm::DatabaseBackend::Sqlite => true,
        _ => {
            return Err(YorishiroError::BackendUnsupported {
                message: "vector search is not supported on this database backend".into(),
            });
        }
    };
    let mut normalized = tokenizers::NormalizedString::from(query_text);
    normalized.nfkc();
    let query_text = normalized.get().trim();
    if !sqlite {
        let statement = postgres_rrf_query(workspace_id, vector, query_text, &query)?;
        return Ok(
            SearchRow::find_by_statement(conn.get_database_backend().build(&statement))
                .all(conn)
                .await
                .internal()?
                .into_iter()
                .map(SearchRow::into_hit)
                .collect(),
        );
    }
    let terms: Vec<_> = query_text.split_whitespace().collect();
    let sqlite_match = terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ");
    let embedding = Expr::col((Alias::new("ee"), Alias::new("embedding")));
    let knn = vector_candidates_query(conn.get_database_backend(), workspace_id, vector, &query)?;
    let mut hits: Vec<SearchHit> =
        SearchRow::find_by_statement(conn.get_database_backend().build(&knn))
            .all(conn)
            .await
            .internal()?
            .into_iter()
            .map(SearchRow::into_hit)
            .collect();

    // Vector hits outrank every text-only match.
    if (hits.len() as i64) < limit
        && !terms.is_empty()
        && (!sqlite || terms.iter().all(|term| term.chars().count() >= 3))
    {
        let mut fallback = scoped_search(workspace_id, &query);
        join_embedding(&mut fallback, embed_table, JoinType::LeftJoin);
        fallback.expr_as(
            if sqlite {
                Expr::val(None::<f64>)
            } else {
                Expr::val(None::<f64>).cast_as(Alias::new("float8"))
            },
            "distance",
        );
        if sqlite {
            use sea_orm::sea_query::extension::sqlite::SqliteBinOper;
            fallback
                .join(
                    JoinType::InnerJoin,
                    Alias::new("entity_fts"),
                    Expr::col((Alias::new("e"), Alias::new("id")))
                        .cast_as(Alias::new("text"))
                        .equals((Alias::new("entity_fts"), Alias::new("entity_id"))),
                )
                .and_where(
                    Expr::col(Alias::new("entity_fts"))
                        .binary(SqliteBinOper::Match, Expr::val(sqlite_match)),
                )
                .and_where(Expr::col((Alias::new("ee"), Alias::new("entity_id"))).is_null())
                .order_by_expr(
                    Func::cust(Alias::new("bm25"))
                        .arg(Expr::col(Alias::new("entity_fts")))
                        .into(),
                    Order::Asc,
                );
        } else {
            let text = Expr::cust("normalize(\"e\".\"data\"::text, nfkc)");
            let pattern = format!(
                "%{}%",
                query_text
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            );
            fallback
                .and_where(embedding.is_null())
                .and_where(text.clone().ilike(pattern))
                .order_by_expr(
                    Func::cust(Alias::new("similarity"))
                        .args([text, Expr::val(query_text)])
                        .into(),
                    Order::Desc,
                );
        }
        fallback.limit((limit - hits.len() as i64) as u64);
        hits.extend(
            SearchRow::find_by_statement(conn.get_database_backend().build(&fallback))
                .all(conn)
                .await
                .internal()?
                .into_iter()
                .map(SearchRow::into_hit),
        );
    }
    Ok(hits)
}

fn postgres_rrf_query(
    workspace_id: Uuid,
    vector: Vec<f32>,
    text: &str,
    query: &SearchQuery,
) -> Result<SelectStatement, YorishiroError> {
    use sea_orm::sea_query::{CommonTableExpression, UnionType, WithClause};
    let limit = query.limit.clamp(MIN_SEARCH_LIMIT, MAX_SEARCH_LIMIT);
    let mut candidates = query.clone();
    candidates.limit = std::cmp::max(limit, 50);
    let vectors = vector_candidates_query(
        sea_orm::DatabaseBackend::Postgres,
        workspace_id,
        vector,
        &candidates,
    )?;
    let mut texts = scoped_search(workspace_id, query);
    let normalized = Expr::cust("normalize(\"e\".\"data\"::text, nfkc)");
    let pattern = format!(
        "%{}%",
        text.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    texts
        .expr_as(
            Func::cust(Alias::new("similarity")).args([normalized.clone(), Expr::val(text)]),
            "text_similarity",
        )
        .and_where(normalized.ilike(pattern))
        .and_where(Expr::val(!text.is_empty()))
        .limit(21);
    let ranked_vectors = Query::select()
        .column(Alias::new("id"))
        .expr_as(
            Expr::cust("1.0 / (60 + row_number() OVER (ORDER BY distance, id))"),
            "score",
        )
        .from(Alias::new("vectors"))
        .to_owned();
    let count = Query::select()
        .expr(Expr::col(Alias::new("id")).count())
        .from(Alias::new("texts"))
        .to_owned();
    let mut ranks = ranked_vectors;
    ranks.union(
        UnionType::All,
        Query::select()
            .column(Alias::new("id"))
            .expr_as(
                Expr::cust("1.0 / (60 + row_number() OVER (ORDER BY text_similarity DESC, id))"),
                "score",
            )
            .from(Alias::new("texts"))
            .and_where(Expr::SubQuery(None, Box::new(count.into())).lte(20))
            .to_owned(),
    );
    let scores = Query::select()
        .column(Alias::new("id"))
        .expr_as(Expr::col(Alias::new("score")).sum(), "score")
        .from(Alias::new("ranks"))
        .group_by_col(Alias::new("id"))
        .to_owned();
    let mut with = WithClause::new();
    for (name, statement) in [
        ("vectors", vectors),
        ("texts", texts),
        ("ranks", ranks),
        ("scores", scores),
    ] {
        with.cte(
            CommonTableExpression::new()
                .table_name(Alias::new(name))
                .query(statement)
                .to_owned(),
        );
    }
    let mut merged = scoped_search(workspace_id, query);
    merged
        .join(
            JoinType::InnerJoin,
            Alias::new("scores"),
            Expr::col((Alias::new("scores"), Alias::new("id")))
                .equals((Alias::new("e"), Alias::new("id"))),
        )
        .join(
            JoinType::LeftJoin,
            Alias::new("vectors"),
            Expr::col((Alias::new("vectors"), Alias::new("id")))
                .equals((Alias::new("e"), Alias::new("id"))),
        )
        .expr_as(
            Expr::col((Alias::new("vectors"), Alias::new("distance"))),
            "distance",
        )
        .order_by((Alias::new("scores"), Alias::new("score")), Order::Desc)
        .order_by((Alias::new("e"), Alias::new("id")), Order::Asc)
        .limit(limit as u64)
        .with_cte(with);
    Ok(merged)
}

/// Builds the production vector candidate query for execution or EXPLAIN diagnostics.
///
/// # Errors
/// Returns an error when the query vector is invalid.
pub fn vector_candidates_query(
    backend: sea_orm::DatabaseBackend,
    workspace_id: Uuid,
    vector: Vec<f32>,
    query: &SearchQuery,
) -> Result<SelectStatement, YorishiroError> {
    super::entity_embeddings::validate_vector(&vector, vector.len())?;
    let sqlite = backend == sea_orm::DatabaseBackend::Sqlite;
    let embed_table = super::entity_embeddings::TABLE;
    let limit = query.limit.clamp(MIN_SEARCH_LIMIT, MAX_SEARCH_LIMIT);
    let embedding = Expr::col((Alias::new("ee"), Alias::new("embedding")));
    let dimensions = vector.len();
    let distance = if sqlite {
        Func::cust(Alias::new("vec_distance_cosine"))
            .args([
                Expr::val(crate::db::sqlite_vec_blob(&vector)),
                embedding.clone(),
            ])
            .into()
    } else {
        embedding
            .clone()
            .cast_as(Alias::new(format!("vector({dimensions})")))
            .binary(
                PgBinOper::CosineDistance,
                Expr::val(sea_orm::entity::prelude::PgVector::from(vector)),
            )
    };
    let mut knn = scoped_search(workspace_id, query);
    join_embedding(&mut knn, embed_table, JoinType::InnerJoin);
    knn.and_where(Expr::col((Alias::new("ee"), Alias::new("workspace_id"))).eq(workspace_id));
    if !sqlite {
        // A literal typmod and dimension predicate match workspace partial expression indexes.
        knn.and_where(
            Func::cust(Alias::new("vector_dims"))
                .arg(embedding.clone())
                .eq(Expr::cust(dimensions.to_string())),
        );
    }
    knn.expr_as(distance.clone(), "distance")
        .and_where(embedding.clone().is_not_null())
        .order_by_expr(distance, Order::Asc)
        .limit(limit as u64);
    Ok(knn)
}
