//! PostgreSQL candidate retrieval: the workspace HNSW index selects candidates and the distance is the original full-precision cosine distance.

use super::{ANN_CANDIDATE_LIMIT, SearchQuery, SearchRow, join_embedding, scoped_search};
use crate::error::{ResultExt, YorishiroError};
use crate::models::entity_embeddings::{PGVECTOR_HNSW_VECTOR_MAX_DIMS, hnsw_halfvec_dimensions};
use sea_orm::sea_query::{
    Alias, Expr, ExprTrait, Func, JoinType, Order, SelectStatement, extension::postgres::PgBinOper,
};
use sea_orm::{ConnectionTrait, FromQueryResult, Statement, TransactionSession, TransactionTrait};
use uuid::Uuid;

/// `hnsw.ef_search` for dimension bands above pgvector's full-precision limit.
const EF_SEARCH_HIGH: i64 = 1000;
/// `hnsw.ef_search` for full-precision band searches.
const EF_SEARCH_BASE: i64 = 400;

/// Transaction-local GUCs every PostgreSQL search runs under.
///
/// `force_custom_plan` keeps the candidate query out of the plan cache.
/// `iterative_scan` lets the HNSW scan keep yielding rows until the candidate limit is satisfied.
const SEARCH_GUCS_SQL: &str = "SELECT set_config('plan_cache_mode', 'force_custom_plan', true), set_config('hnsw.ef_search', $1, true), set_config('hnsw.iterative_scan', 'relaxed_order', true)";

/// Fetches the nearest candidates under the transaction-local index settings.
///
/// # Errors
/// Returns an error when the database fails.
pub(super) async fn candidates(
    conn: &(impl ConnectionTrait + TransactionTrait),
    workspace_id: Uuid,
    vector: Vec<f32>,
    query: &SearchQuery,
) -> Result<Vec<SearchRow>, YorishiroError> {
    let txn = conn.begin().await.internal()?;
    let ef_search = if vector.len() > PGVECTOR_HNSW_VECTOR_MAX_DIMS {
        EF_SEARCH_HIGH
    } else {
        EF_SEARCH_BASE
    };
    // SeaORM cannot express transaction-local PostgreSQL settings.
    txn.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        SEARCH_GUCS_SQL,
        [ef_search.to_string().into()],
    ))
    .await
    .internal()?;
    let statement = vector_candidates(workspace_id, vector, query);
    let rows = SearchRow::find_by_statement(txn.get_database_backend().build(&statement))
        .all(&txn)
        .await
        .internal()?;
    // Roll back the read-only savepoint so settings do not leak to an outer RLS transaction.
    txn.rollback().await.internal()?;
    Ok(rows)
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
/// Above it a normalized `halfvec`: normalizing keeps every value inside the half-float range, and cosine ranking is invariant under it.
/// Above the half-precision limit, the first 4,000 dimensions carry the index and the search reranks in original coordinates.
fn hnsw_distance(embedding: Expr, query: Expr, dimensions: usize) -> Expr {
    let Some(index_dims) = hnsw_halfvec_dimensions(dimensions) else {
        return original_distance(embedding, dimensions, query);
    };
    let truncated = dimensions > index_dims;
    let truncated_side = |side: Expr| {
        if truncated {
            Func::cust(Alias::new("subvector"))
                .args([side, Expr::val(1), Expr::cust(index_dims.to_string())])
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

/// The candidate query: nearest vectors by the workspace index expression, with the distance in original coordinates.
pub(super) fn vector_candidates(
    workspace_id: Uuid,
    vector: Vec<f32>,
    query: &SearchQuery,
) -> SelectStatement {
    let dimensions = vector.len();
    let embedding = Expr::col((Alias::new("ee"), Alias::new("embedding")));
    let distance = original_distance(
        embedding.clone(),
        dimensions,
        Expr::val(sea_orm::entity::prelude::PgVector::from(vector.clone())),
    );
    // The order expression is the index expression, while the distance column keeps original coordinates.
    let order = hnsw_distance(
        embedding.clone(),
        Expr::val(sea_orm::entity::prelude::PgVector::from(vector)),
        dimensions,
    );
    let mut knn = scoped_search(workspace_id, query);
    join_embedding(
        &mut knn,
        crate::models::entity_embeddings::TABLE,
        JoinType::InnerJoin,
    );
    knn.and_where(Expr::col((Alias::new("ee"), Alias::new("workspace_id"))).eq(workspace_id));
    // A literal typmod and dimension predicate match workspace partial expression indexes.
    knn.and_where(
        Func::cust(Alias::new("vector_dims"))
            .arg(embedding.clone())
            .eq(Expr::cust(dimensions.to_string())),
    );
    knn.expr_as(distance, "distance")
        .and_where(embedding.is_not_null())
        .order_by_expr(order, Order::Asc)
        .limit(ANN_CANDIDATE_LIMIT.cast_unsigned());
    knn
}
