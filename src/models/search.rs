//! Vector similarity search over `entity_entities`.
//!
//! Embeddings live in the separate `entity_embeddings` table, so vector queries join `entity_entities e` to it.
//!
//! `search_by_vector` validates the query and then hands it to one backend module.
//! `postgres` selects candidates from the workspace HNSW index.
//! `sqlite` selects candidates by exact sqlite-vec distance.
//! Both use the same final ranking and filtering, though approximate candidate selection can omit matches.
//!
//! Every promoted workspace has one partial HNSW index on its `entity_embeddings` partition.
//! The index expression follows the dimension bands described in `entity_embeddings::indexes`.
//! The index selects candidates, and the final rank always uses the original full-precision coordinates.
//!
//! Embedding generation follows two paths.
//! Document embedding is asynchronous: the embedding sync worker (`workers::embedding_sync`) generates and stores vectors after entity writes.
//! Query embedding is not done here: the server holds no model, so `workers::query_embedding` asks a worker for the vector before any DB connection is held for the search.
//! Both use the same provider, so documents and queries share one vector space.

mod postgres;
mod sqlite;

use crate::error::OptionExt;
use sea_orm::sea_query::{Alias, Expr, ExprTrait, JoinType, Query, SelectStatement};
use sea_orm::{ConnectionTrait, FromQueryResult, TransactionTrait};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::entity_entities::EntityRecord;

const DEFAULT_SEARCH_LIMIT: i64 = 10;
pub(crate) const MIN_SEARCH_LIMIT: i64 = 1;
pub(crate) const MAX_SEARCH_LIMIT: i64 = 200;
const MAX_SEARCH_WORKSPACES: usize = 8;
/// Nearest candidates fetched before the content filter and the limit apply.
const ANN_CANDIDATE_LIMIT: i64 = 1000;

#[derive(Clone)]
pub struct SearchQuery {
    pub entity_type: Option<String>,
    /// JSON containment filter applied to the nearest candidates, e.g. `{"status": "active"}`.
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
        return Err(search_generation_changed());
    }
    // The trail keeps which published index the search ran against, never the query or the hits.
    let embedding_revision =
        super::workspace_workspaces::embedding_revision(db, workspace_id).await?;
    crate::controllers::middleware::access_log::noted(
        crate::models::iam::policy::Action::SearchEntities,
        &serde_json::json!({"workspace_id": workspace_id, "embedding_revision": embedding_revision}),
    );
    Ok(())
}

fn search_generation_changed() -> YorishiroError {
    YorishiroError::BackendUnavailable {
        message: "workspace embeddings are changing; retry semantic search".into(),
    }
}

async fn search_text(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::access_keys::AuthContext,
    text: &str,
    query: SearchQuery,
    deployment_vector: &mut Option<Vec<f32>>,
) -> Result<Vec<SearchHit>, YorishiroError> {
    let tenant_id = actor.tenant_id;
    let workspace_id = actor.require_workspace()?;
    let revision = super::workspace_workspaces::embedding_revision(&ctx.db, workspace_id).await?;
    let uses_deployment_provider = crate::workspace_embedding_metadata(&ctx.db, workspace_id)
        .await?
        .is_none();
    // Matching model names are not enough to share a vector across independently configured providers.
    let embedded =
        if let (true, Some(vector)) = (uses_deployment_provider, deployment_vector.as_ref()) {
            Ok(vector.clone())
        } else {
            // Wait for the worker without holding a workspace connection.
            crate::workers::query_embedding::embed_query(ctx, tenant_id, workspace_id, text).await
        };
    let txn = crate::db::begin_workspace(ctx, tenant_id, workspace_id).await?;
    let _authorization = super::iam::access::authorize_workspace_action(
        &txn,
        &ctx.db,
        actor,
        super::iam::policy::Action::SearchEntities,
    )
    .await?;
    if super::workspace_workspaces::embedding_revision(&txn, workspace_id).await? != revision {
        return Err(search_generation_changed());
    }
    let vector = embedded?;
    if uses_deployment_provider && deployment_vector.is_none() {
        *deployment_vector = Some(vector.clone());
    }
    search_by_vector(&txn, workspace_id, vector, query).await
}

pub(crate) async fn search_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::access_keys::AuthContext,
    text: &str,
    query: SearchQuery,
    limiter: &crate::controllers::middleware::rate_limit::RateLimiter,
) -> Result<Vec<SearchHit>, YorishiroError> {
    let workspaces = super::iam::access::read_workspaces(
        &ctx.db,
        actor,
        super::iam::policy::Action::SearchEntities,
    )
    .await?;
    if workspaces.len() > MAX_SEARCH_WORKSPACES {
        return Err(YorishiroError::ValidationFailed {
            message: format!(
                "cross-workspace search is limited to {MAX_SEARCH_WORKSPACES} workspaces"
            ),
            details: vec![],
            hint: "select a workspace with workspace_id or x-workspace-id".into(),
        });
    }
    for &workspace_id in &workspaces {
        ensure_available(&ctx.db, workspace_id).await?;
    }
    crate::controllers::middleware::rate_limit::charge_workspace_search_tokens(
        limiter,
        &workspaces,
        text,
    )?;
    let mut ranked = Vec::new();
    let mut deployment_vector = None;
    for workspace_id in workspaces {
        let mut selected = actor.clone();
        selected.workspace_id = Some(workspace_id);
        for (rank, hit) in search_text(ctx, &selected, text, query.clone(), &mut deployment_vector)
            .await?
            .into_iter()
            .enumerate()
        {
            ranked.push((rank, hit));
        }
    }
    // Local ranks are comparable across providers; cosine distances from different models are not.
    ranked.sort_by_key(|(rank, hit)| (*rank, hit.entity.id));
    Ok(ranked
        .into_iter()
        .take(usize::try_from(query.limit.clamp(MIN_SEARCH_LIMIT, MAX_SEARCH_LIMIT)).internal()?)
        .map(|(_, hit)| hit)
        .collect())
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SearchHit {
    pub entity: EntityRecord,
    /// Cosine distance (PostgreSQL `<=>` or sqlite-vec cosine distance).
    /// Closer to 0 means more similar.
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

/// Shared projection and workspace scope for vector search.
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
        .or_not_found(format!("workspace {workspace_id} not found"))?;
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

/// Searches a workspace by query vector.
///
/// The content filter and result limit apply after candidate selection.
///
/// # Errors
/// Returns an error when the vector is invalid, the workspace is reindexing, or the database fails.
pub async fn search_by_vector(
    conn: &(impl ConnectionTrait + TransactionTrait),
    workspace_id: Uuid,
    vector: Vec<f32>,
    query: SearchQuery,
) -> Result<Vec<SearchHit>, YorishiroError> {
    super::entity_embeddings::validate_vector(&vector, vector.len())?;
    ensure_available(conn, workspace_id).await?;
    check_query_width(conn, workspace_id, vector.len()).await?;
    let mut rows = match conn.get_database_backend() {
        sea_orm::DatabaseBackend::Postgres => {
            postgres::candidates(conn, workspace_id, vector, &query).await?
        }
        sea_orm::DatabaseBackend::Sqlite => {
            sqlite::candidates(conn, workspace_id, &vector, &query).await?
        }
        _ => {
            return Err(YorishiroError::BackendUnsupported {
                message: "vector search is not supported on this database backend".into(),
            });
        }
    };
    rows.retain(|row| row.distance.is_some_and(f64::is_finite));
    if let Some(filter) = &query.filter {
        rows.retain(|row| contains(&row.entity.data, filter));
    }
    rows.sort_by(|a, b| {
        a.distance
            .unwrap_or(f64::INFINITY)
            .total_cmp(&b.distance.unwrap_or(f64::INFINITY))
            .then_with(|| a.entity.id.cmp(&b.entity.id))
    });
    let limit = query.limit.clamp(MIN_SEARCH_LIMIT, MAX_SEARCH_LIMIT);
    rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    Ok(rows.into_iter().map(SearchRow::into_hit).collect())
}

/// Matches objects recursively and requires every requested array element to be present.
fn contains(data: &Value, filter: &Value) -> bool {
    match (data, filter) {
        (Value::Object(data), Value::Object(filter)) => filter
            .iter()
            .all(|(key, value)| data.get(key).is_some_and(|found| contains(found, value))),
        (Value::Array(data), Value::Array(filter)) => filter
            .iter()
            .all(|wanted| data.iter().any(|found| contains(found, wanted))),
        (Value::Array(data), scalar) if !scalar.is_array() && !scalar.is_object() => {
            data.iter().any(|found| found == scalar)
        }
        (left, right) => left == right,
    }
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
    Ok(match backend {
        sea_orm::DatabaseBackend::Sqlite => sqlite::vector_candidates(workspace_id, &vector, query),
        _ => postgres::vector_candidates(workspace_id, vector, query),
    })
}
