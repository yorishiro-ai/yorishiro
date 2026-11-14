//! Vector similarity search over `entity_entities`.
//!
//! Embeddings live in the separate `entity_embeddings` table, so every query here
//! joins `entity_entities e` to that table when searching by vector.
//!
//! PostgreSQL fuses vector and selective literal-text ranks using gated RRF.
//! PostgreSQL uses NFKC-normalized ILIKE with a trigram expression index.
//! SQLite uses quoted FTS5 trigram terms and cannot match terms shorter than three characters.
//! SQLite keeps vector-first results and fills remaining slots from unembedded text matches.
//!
//! Every promoted workspace has one partial HNSW index on its `entity_embeddings` partition.
//! Up to pgvector's full-precision limit the index expression is the original vector; above it a
//! normalized `halfvec`; above the half-precision limit the first 4,000 dimensions.
//! The index selects candidates; the final rank always uses the original full-precision coordinates.
//!
//! Embedding generation follows two paths.
//! Document embedding is asynchronous: the embedding sync worker (`workers::embedding_sync`) generates and stores vectors after entity writes.
//! Query embedding is not done here: the server holds no model, so `workers::query_embedding` asks a worker for the vector before any DB connection is held for the search.
//! Both use the same provider, so documents and queries share one vector space.

use sea_orm::sea_query::{
    Alias, Expr, ExprTrait, Func, JoinType, Order, Query, SelectStatement,
    extension::postgres::{PgBinOper, PgExpr},
};
use sea_orm::{ConnectionTrait, FromQueryResult, Statement, TransactionSession, TransactionTrait};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::entity_embeddings::{
    PGVECTOR_HNSW_HALFVEC_MAX_DIMS, PGVECTOR_HNSW_VECTOR_MAX_DIMS,
};
use crate::models::entity_entities::EntityRecord;

const DEFAULT_SEARCH_LIMIT: i64 = 10;
pub(crate) const MIN_SEARCH_LIMIT: i64 = 1;
pub(crate) const MAX_SEARCH_LIMIT: i64 = 200;

/// Candidates the index materializes per search before original-coordinate reranking.
const ANN_CANDIDATE_LIMIT: i64 = 1000;
/// `hnsw.ef_search` for dimension bands above pgvector's full-precision limit.
const EF_SEARCH_HIGH: i64 = 1000;
/// `hnsw.ef_search` floor for full-precision band searches; the requested limit applies too.
const EF_SEARCH_BASE: i64 = 400;
/// RRF reciprocal-rank constant.
const RRF_K: i64 = 60;
/// Fewer vector matches than this never enter the fusion (the limit floor).
const RRF_VECTOR_CANDIDATE_MIN: i64 = 50;
/// Text matches the RRF list fetches, and the count above which the list stops contributing.
const RRF_TEXT_FETCH_LIMIT: u64 = 21;
const RRF_TEXT_GATE_LIMIT: i64 = 20;

/// Transaction-local GUCs every PostgreSQL search runs under.
///
/// `force_custom_plan` keeps the candidate CTE out of the plan cache; `iterative_scan`
/// lets the HNSW scan keep yielding closer rows until the candidate limit is satisfied.
const SEARCH_GUCS_SQL: &str = "SELECT set_config('plan_cache_mode', 'force_custom_plan', true), set_config('hnsw.ef_search', $1, true), set_config('hnsw.iterative_scan', 'relaxed_order', true)";

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
    actor: &crate::models::api_keys::AuthContext,
    text: &str,
    query: SearchQuery,
) -> Result<Vec<SearchHit>, YorishiroError> {
    let tenant_id = actor.tenant_id;
    let workspace_id = actor.require_workspace()?;
    authorize_search(ctx, actor).await?;
    // Wait for the worker without holding a workspace connection.
    let vector =
        crate::workers::query_embedding::embed_query(ctx, tenant_id, workspace_id, text).await?;
    check_query_width(&ctx.db, workspace_id, vector.len()).await?;
    let txn = crate::db::begin_workspace(ctx, tenant_id, workspace_id).await?;
    let _authorization = super::access::authorize_workspace_action(
        &txn,
        &ctx.db,
        actor,
        super::api_keys::ApiKeyScope::Read,
        super::iam_policy::Action::Search,
    )
    .await?;
    search_by_vector(&txn, workspace_id, vector, text, query).await
}

pub(crate) async fn authorize_search(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::api_keys::AuthContext,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(&ctx.db).await.internal()?;
    super::access::require_iam_action(
        &txn,
        actor,
        super::api_keys::ApiKeyScope::Read,
        super::iam_policy::Action::Search,
        super::iam_policy::Resource::Workspace {
            tenant_id: actor.tenant_id,
            workspace_id: actor.require_workspace()?,
        },
    )
    .await
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
        // Workspace is fixed above; an uncorrelated guard avoids distorting ANN join estimates.
        .and_where(Expr::col((Alias::new("w"), Alias::new("id"))).eq(workspace_id))
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
        let ef_search = if vector.len() > PGVECTOR_HNSW_VECTOR_MAX_DIMS {
            EF_SEARCH_HIGH
        } else {
            std::cmp::max(
                query.limit.clamp(MIN_SEARCH_LIMIT, MAX_SEARCH_LIMIT),
                EF_SEARCH_BASE,
            )
        };
        // SeaORM cannot express transaction-local PostgreSQL settings.
        txn.execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            SEARCH_GUCS_SQL,
            [ef_search.to_string().into()],
        ))
        .await
        .internal()?;
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
        let started = std::time::Instant::now();
        let rows = SearchRow::find_by_statement(conn.get_database_backend().build(&statement))
            .all(conn)
            .await
            .internal()?;
        tracing::debug!(%workspace_id, rows = rows.len(), rerank_ms = started.elapsed().as_secs_f64() * 1000.0, "PostgreSQL search results ranked");
        return Ok(rows.into_iter().map(SearchRow::into_hit).collect());
    }
    let embed_table = super::entity_embeddings::TABLE;
    let terms: Vec<_> = query_text.split_whitespace().collect();
    let sqlite_match = terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ");
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
        && terms.iter().all(|term| term.chars().count() >= 3)
    {
        let mut fallback = scoped_search(workspace_id, &query);
        join_embedding(&mut fallback, embed_table, JoinType::LeftJoin);
        fallback.expr_as(Expr::val(None::<f64>), "distance");
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
            )
            .limit((limit - hits.len() as i64) as u64);
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

/// Original full-precision cosine distance between a stored embedding and a query vector.
fn original_distance(embedding: Expr, dimensions: usize, query: Expr) -> Expr {
    embedding
        .cast_as(Alias::new(format!("vector({dimensions})")))
        .binary(PgBinOper::CosineDistance, query)
}

/// The expression a workspace HNSW index ranks by, for the query's dimension band.
///
/// Up to pgvector's full-precision limit this is the original vector.
/// Above it a normalized `halfvec`: normalizing keeps every value inside the half-float range,
/// and cosine ranking is invariant under it. Above the half-precision limit, the first
/// 4,000 dimensions carry the index and the search reranks in original coordinates.
fn hnsw_distance(embedding: Expr, query: Expr, dimensions: usize) -> Expr {
    if dimensions <= PGVECTOR_HNSW_VECTOR_MAX_DIMS {
        return original_distance(embedding, dimensions, query);
    }
    let index_dims = dimensions.min(PGVECTOR_HNSW_HALFVEC_MAX_DIMS);
    let truncated = dimensions > PGVECTOR_HNSW_HALFVEC_MAX_DIMS;
    let truncated_side = |side: Expr| {
        if truncated {
            Func::cust(Alias::new("subvector"))
                .args([side, Expr::val(1), Expr::val(index_dims as i32)])
                .into()
        } else {
            side
        }
    };
    let normalized_side = |side: Expr| {
        Expr::from(Func::cust(Alias::new("l2_normalize")).arg(truncated_side(side)))
            .cast_as(Alias::new(format!("halfvec({index_dims})")))
    };
    normalized_side(embedding).binary(PgBinOper::CosineDistance, normalized_side(query))
}

/// Builds the production PostgreSQL gated RRF query.
/// Input text must already be NFKC-normalized and trimmed.
///
/// # Errors
/// Returns an error when the query vector is invalid.
pub fn postgres_rrf_query(
    workspace_id: Uuid,
    vector: Vec<f32>,
    text: &str,
    query: &SearchQuery,
) -> Result<SelectStatement, YorishiroError> {
    use sea_orm::sea_query::{Asterisk, CommonTableExpression, UnionType, WithClause};
    let limit = query.limit.clamp(MIN_SEARCH_LIMIT, MAX_SEARCH_LIMIT);
    let mut candidates = query.clone();
    candidates.limit = std::cmp::max(limit, RRF_VECTOR_CANDIDATE_MIN);
    let dimensions = vector.len();
    let mut vectors = vector_candidates_query(
        sea_orm::DatabaseBackend::Postgres,
        workspace_id,
        vector.clone(),
        &candidates,
    )?;
    // Rank IDs and distances only; entity payloads are fetched after rank fusion.
    vectors
        .clear_selects()
        .column((Alias::new("e"), Alias::new("id")));
    let embedding = Expr::col((Alias::new("ee"), Alias::new("embedding")));
    let distance = original_distance(
        embedding,
        dimensions,
        Expr::val(sea_orm::entity::prelude::PgVector::from(vector)),
    );
    vectors.expr_as(distance, "distance");
    // Every band up to the application maximum has a partial HNSW index, so the candidate
    // set is always the index output: materialize it, then rerank by original coordinates
    // (relaxed HNSW can emit closer rows after the initial result batch).
    vectors.limit(ANN_CANDIDATE_LIMIT as u64);
    let ann_candidates = vectors;
    let vectors = Query::select()
        .column(Asterisk)
        .from(Alias::new("ann_candidates"))
        .order_by_expr(Expr::cust("distance + 0"), Order::Asc)
        .order_by(Alias::new("id"), Order::Asc)
        .limit(candidates.limit as u64)
        .to_owned();
    let mut texts = scoped_search(workspace_id, query);
    texts
        .clear_selects()
        .column((Alias::new("e"), Alias::new("id")));
    let normalized = Expr::cust("normalize(\"e\".\"data\"::text, nfkc)");
    let pattern = format!(
        "%{}%",
        text.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    // Script properties cover extended Latin without treating Greek/Cyrillic as Latin.
    let latin = tokenizers::utils::SysRegex::new(r"(?:\p{Latin}\p{M}*)+").map_err(|error| {
        YorishiroError::Internal(anyhow::anyhow!("Latin term pattern: {error}"))
    })?;
    let latin_terms: Vec<_> = latin
        .find_iter(text)
        .map(|(start, end)| &text[start..end])
        .filter(|term| term.chars().filter(|ch| ch.is_alphabetic()).count() >= 3)
        .collect();
    let text_score = if latin_terms.is_empty() {
        Func::cust(Alias::new("similarity")).args([normalized.clone(), Expr::val(text)])
    } else {
        // word_similarity is directional: find query-word extents in the stored document.
        Func::cust(Alias::new("word_similarity"))
            .args([Expr::val(latin_terms.join(" ")), normalized.clone()])
    };
    texts
        .expr_as(text_score, "text_similarity")
        .and_where(normalized.ilike(pattern))
        .and_where(Expr::val(!text.is_empty()))
        .limit(RRF_TEXT_FETCH_LIMIT);
    let ranked_vectors = Query::select()
        .column(Alias::new("id"))
        .expr_as(
            Expr::cust(format!(
                "1.0 / ({RRF_K} + row_number() OVER (ORDER BY distance, id))"
            )),
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
                Expr::cust(format!(
                    "1.0 / ({RRF_K} + row_number() OVER (ORDER BY text_similarity DESC, id))"
                )),
                "score",
            )
            .from(Alias::new("texts"))
            .and_where(Expr::SubQuery(None, Box::new(count.into())).lte(RRF_TEXT_GATE_LIMIT))
            .to_owned(),
    );
    let scores = Query::select()
        .column(Alias::new("id"))
        .expr_as(Expr::col(Alias::new("score")).sum(), "score")
        .from(Alias::new("ranks"))
        .group_by_col(Alias::new("id"))
        .to_owned();
    let mut with = WithClause::new();
    with.cte(
        CommonTableExpression::new()
            .table_name(Alias::new("ann_candidates"))
            .query(ann_candidates)
            .materialized(true)
            .to_owned(),
    );
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
    let dimensions = vector.len();
    let embedding = Expr::col((Alias::new("ee"), Alias::new("embedding")));
    let distance = if sqlite {
        Func::cust(Alias::new("vec_distance_cosine"))
            .args([
                Expr::val(crate::db::sqlite_vec_blob(&vector)),
                embedding.clone(),
            ])
            .into()
    } else {
        original_distance(
            embedding.clone(),
            dimensions,
            Expr::val(sea_orm::entity::prelude::PgVector::from(vector.clone())),
        )
    };
    // The order expression is the index expression: HNSW selects the candidates,
    // while the distance column keeps original coordinates for the final ranking.
    let order = if sqlite {
        distance.clone()
    } else {
        hnsw_distance(
            embedding.clone(),
            Expr::val(sea_orm::entity::prelude::PgVector::from(vector.clone())),
            dimensions,
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
    knn.expr_as(distance, "distance")
        .and_where(embedding.clone().is_not_null())
        .order_by_expr(order, Order::Asc)
        .limit(limit as u64);
    // Unpromoted workspaces fall back to exact search over the (below-threshold) workspace rows.
    Ok(knn)
}
