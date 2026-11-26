//! SQLite candidate retrieval: exact sqlite-vec cosine distance over the workspace's embeddings.

use super::{ANN_CANDIDATE_LIMIT, SearchQuery, SearchRow, join_embedding, scoped_search};
use crate::error::{ResultExt, YorishiroError};
use sea_orm::sea_query::{Alias, Expr, ExprTrait, Func, JoinType, Order, SelectStatement};
use sea_orm::{ConnectionTrait, FromQueryResult};
use uuid::Uuid;

/// Fetches the nearest candidates by exact distance.
///
/// # Errors
/// Returns an error when the database fails.
pub(super) async fn candidates(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    vector: &[f32],
    query: &SearchQuery,
) -> Result<Vec<SearchRow>, YorishiroError> {
    let statement = vector_candidates(workspace_id, vector, query);
    SearchRow::find_by_statement(conn.get_database_backend().build(&statement))
        .all(conn)
        .await
        .internal()
}

/// The candidate query: sqlite-vec cosine distance over the workspace's embeddings.
pub(super) fn vector_candidates(
    workspace_id: Uuid,
    vector: &[f32],
    query: &SearchQuery,
) -> SelectStatement {
    let embedding = Expr::col((Alias::new("ee"), Alias::new("embedding")));
    let distance: Expr = Func::cust(Alias::new("vec_distance_cosine"))
        .args([
            Expr::val(crate::db::sqlite_vec_blob(vector)),
            embedding.clone(),
        ])
        .into();
    let mut knn = scoped_search(workspace_id, query);
    join_embedding(
        &mut knn,
        crate::models::entity_embeddings::TABLE,
        JoinType::InnerJoin,
    );
    knn.and_where(Expr::col((Alias::new("ee"), Alias::new("workspace_id"))).eq(workspace_id));
    knn.expr_as(distance.clone(), "distance")
        .and_where(embedding.is_not_null())
        .order_by_expr(distance, Order::Asc)
        .limit(ANN_CANDIDATE_LIMIT.cast_unsigned());
    knn
}
