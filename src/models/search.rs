//! Vector similarity search over `entity_entities`.
//!
//! Embeddings live in the separate `entity_embeddings` table, so every query here
//! joins `entity_entities e` to that table when searching by vector.
//!
//! Two queries run for each backend: vector search first, then a trigram (PostgreSQL pg_trgm)
//! or trigram (SQLite FTS5 `tokenize='trigram'`) fallback for entities with no embedding at
//! all.  Both backends therefore use character-n-gram fuzzy matching for the fallback path.
//! The two halves are merged in Rust.
//!
//! Embedding generation follows two paths.
//! Document embedding is asynchronous: the embedding sync worker (`workers::embedding_sync`) generates and stores vectors after entity writes.
//! Query embedding is not done here: the server holds no model, so `workers::query_embedding` asks a worker for the vector before any DB connection is held for the search.
//! Both use the same provider, so documents and queries share one vector space.

use sea_orm::sea_query::{
    Alias, BinOper, Expr, ExprTrait, Func, JoinType, Order, Query, SelectStatement,
    extension::postgres::{PgBinOper, PgExpr},
};
use sea_orm::{ConnectionTrait, FromQueryResult};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::entity_entities::EntityRecord;

const DEFAULT_SEARCH_LIMIT: i64 = 10;
pub(crate) const MIN_SEARCH_LIMIT: i64 = 1;
pub(crate) const MAX_SEARCH_LIMIT: i64 = 200;

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
    pub(crate) fn into_hit(self) -> SearchHit {
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
            .equals((Alias::new("e"), Alias::new("id"))),
    );
}

/// Returns entities ordered by cosine distance between the given embedding vector and the
/// stored embedding, closest first.
/// As an auxiliary path, entities with no embedding are also included when `query_text` is
/// a pg_trgm fuzzy match (PostgreSQL) or FTS5 match (SQLite) against their data.
/// Vector matches are always ranked ahead of trigram-only matches; trigram-only matches are
/// ordered by similarity.
/// Resolves the workspace's effective embedding width for selecting the correct
/// width-specific table. Returns the width and the table name.
///
/// Falls back to the deployment default (YORISHIRO_EMBEDDING_DIMENSIONS, default 768).
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn resolve_search_table(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<(usize, String), YorishiroError> {
    resolve_search_table_with_dimensions(
        conn,
        workspace_id,
        crate::services::embedding::DEFAULT_EMBEDDING_DIMENSIONS,
    )
    .await
    .map(|(dimension, table)| (dimension, table.to_string()))
}

/// Resolves the embedding table a query vector of `vector_len` components is searched in.
///
/// Reads `tenant_tenants` for the tenant default, which the tenant-scoped role may not touch.
/// Call it on the identity connection (`ctx.db`) before opening the request's tenant transaction, then pass the result to [`search_in_table`].
///
/// # Errors
/// Returns an error if the workspace is missing or no table exists for the resolved width.
/// Checks that a freshly embedded query vector has the width the workspace's vectors have, and returns that width's table.
///
/// The worker calls this before it stores a result, so a model that disagrees with the workspace fails as a diagnostic on the request instead of as a database error in the search that would have used the vector.
/// A width with no table is refused by the same registry the writes use.
///
/// # Errors
/// Returns `ValidationFailed` when the width differs from the workspace's or has no table.
pub(crate) async fn check_query_width(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    vector_len: usize,
) -> Result<&'static str, YorishiroError> {
    let (dimension, table) =
        resolve_search_table_with_dimensions(conn, workspace_id, vector_len).await?;
    if dimension != vector_len {
        return Err(YorishiroError::ValidationFailed {
            message: format!(
                "this workspace holds {dimension}-dimensional vectors, but the configured embedding provider produced a {vector_len}-dimensional query vector"
            ),
            details: vec![],
            hint: "point the deployment at the workspace's model, or re-embed the workspace".into(),
        });
    }
    Ok(table)
}

pub(crate) async fn resolve_query_table(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    vector_len: usize,
) -> Result<&'static str, YorishiroError> {
    resolve_search_table_with_dimensions(conn, workspace_id, vector_len)
        .await
        .map(|(_, table)| table)
}

async fn resolve_search_table_with_dimensions(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    deployment_dimensions: usize,
) -> Result<(usize, &'static str), YorishiroError> {
    use crate::models::entity_embeddings::resolve_embedding_chain;

    let chain = resolve_embedding_chain(conn, workspace_id, deployment_dimensions).await?;
    let dimension = chain
        .workspace_dimensions
        .or(chain.tenant_dimensions)
        .or(Some(i32::try_from(chain.deployment_dimensions).unwrap_or(
            crate::services::embedding::DEFAULT_EMBEDDING_DIMENSIONS as i32,
        )))
        .unwrap_or(crate::services::embedding::DEFAULT_EMBEDDING_DIMENSIONS as i32)
        as usize;
    Ok((
        dimension,
        crate::models::entity_embeddings::embedding_table(dimension)?,
    ))
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn search_by_vector(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    vector: Vec<f32>,
    query_text: &str,
    query: SearchQuery,
) -> Result<Vec<SearchHit>, YorishiroError> {
    let embed_table = resolve_query_table(conn, workspace_id, vector.len()).await?;
    search_in_table(conn, workspace_id, vector, query_text, query, embed_table).await
}

/// [`search_by_vector`] against an `embed_table` already resolved by [`resolve_query_table`].
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub(crate) async fn search_in_table(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    vector: Vec<f32>,
    query_text: &str,
    query: SearchQuery,
    embed_table: &str,
) -> Result<Vec<SearchHit>, YorishiroError> {
    if crate::models::workspace_workspaces::embedding_reindexing(conn, workspace_id).await? {
        return Err(YorishiroError::BackendUnavailable {
            message: "semantic search is temporarily unavailable while workspace embeddings are being reindexed".into(),
        });
    }
    // The table name is interpolated into SQL, so only a known embedding table is accepted.
    if !crate::models::entity_embeddings::embedding_tables().any(|table| table == embed_table) {
        return Err(YorishiroError::ValidationFailed {
            message: format!("{embed_table:?} is not an embedding table"),
            details: vec![],
            hint: "use the table returned by resolve_query_table".into(),
        });
    }
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
    let embedding = Expr::col((Alias::new("ee"), Alias::new("embedding")));
    let distance = if sqlite {
        Func::cust(Alias::new("vec_distance_cosine"))
            .args([
                Expr::val(crate::db::sqlite_vec_blob(&vector)),
                embedding.clone(),
            ])
            .into()
    } else {
        // SeaQuery has no pgvector cosine-distance operator.
        embedding.clone().binary(
            BinOper::Custom("<=>"),
            Expr::val(sea_orm::entity::prelude::PgVector::from(vector)),
        )
    };
    let mut knn = scoped_search(workspace_id, &query);
    join_embedding(&mut knn, embed_table, JoinType::InnerJoin);
    knn.expr_as(distance.clone(), "distance")
        .and_where(embedding.clone().is_not_null())
        .order_by_expr(distance, Order::Asc)
        .limit(limit as u64);
    let mut hits: Vec<SearchHit> =
        SearchRow::find_by_statement(conn.get_database_backend().build(&knn))
            .all(conn)
            .await
            .internal()?
            .into_iter()
            .map(SearchRow::into_hit)
            .collect();

    // Vector hits outrank every text-only match.
    if (hits.len() as i64) < limit {
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
                        .binary(SqliteBinOper::Match, Expr::val(query_text)),
                )
                .and_where(Expr::col((Alias::new("ee"), Alias::new("entity_id"))).is_null())
                .order_by_expr(
                    Func::cust(Alias::new("bm25"))
                        .arg(Expr::col(Alias::new("entity_fts")))
                        .into(),
                    Order::Asc,
                );
        } else {
            let text = Expr::col((Alias::new("e"), Alias::new("data"))).cast_as(Alias::new("text"));
            fallback
                .and_where(embedding.is_null())
                .and_where(
                    text.clone()
                        .binary(PgBinOper::Similarity, Expr::val(query_text)),
                )
                .order_by_expr(
                    Func::cust(Alias::new("similarity"))
                        .args([text, Expr::val(query_text)])
                        .into(),
                    Order::Desc,
                );
        }
        fallback.limit((limit - hits.len() as i64) as u64);
        let rows = SearchRow::find_by_statement(conn.get_database_backend().build(&fallback))
            .all(conn)
            .await
            .internal()?;
        hits.extend(rows.into_iter().map(SearchRow::into_hit));
    }

    Ok(hits)
}
