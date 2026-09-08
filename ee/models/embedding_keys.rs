//! A workspace's own embedding provider assignment, for pointing a tenant at a different compute backend than the deployment default.
//!
//! Reads and writes go through `ctx.db` (the migration-role connection), not the RLS-scoped tenant pool: `yorishiro_app` has no GRANT on this table, matching `identity_workspace_llm_keys`.
//!
//! A workspace with no row here uses the deployment default (`WorkspaceEmbeddingResolver::resolve` returns `None`); this module never falls back on its own, so the caller (`EmbeddingKeyResolver`) decides that.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::identity_workspace_embedding_keys::{ActiveModel, Column, Entity};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Statement};
use serde::Serialize;
use uuid::Uuid;

/// What a workspace has configured, without the key itself.
///
/// `api_key` is deliberately absent rather than masked, matching `llm_keys::LlmKeyDescription`: a masked value still travels through logs and proxies, and nothing a caller does needs it back.
#[derive(Debug, Clone, Serialize)]
pub struct EmbeddingKeyDescription {
    pub base_url: String,
    pub model: String,
    pub dimensions: i32,
    /// Always true when present: the row cannot exist without a key.
    /// Callers use the absence of the whole description to mean "not configured".
    pub configured: bool,
}

/// Refuses anything that is not `http://` or `https://`.
/// Same reasoning as `llm_keys::check_scheme`: the value is interpolated into a request URL, and this rules out only what could never be an OpenAI-compatible endpoint.
/// Not SSRF protection.
fn check_scheme(base_url: &str) -> Result<(), YorishiroError> {
    if base_url.starts_with("http://") || base_url.starts_with("https://") {
        return Ok(());
    }
    Err(YorishiroError::ValidationFailed {
        message: "base_url must start with http:// or https://".into(),
        details: vec![],
        hint: "for example https://api.openai.com/v1".into(),
    })
}

/// A workspace's own embedding credentials and model, as `EmbeddingKeyResolver` reads them to build a provider.
pub struct EmbeddingKeyConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub dimensions: i32,
    pub send_dimensions_param: bool,
}

/// Creates or replaces the width-specific embedding table for a workspace that is about to
/// use a new model width.
///
/// `conn` must be a `DatabaseTransaction` obtained from
/// `TenantDb::begin_for_workspace` or `ctx.db.begin()`.
///
/// This is a DDL call inside the same transaction that stores the key row.
/// `yorishiro_app` has no CREATE privilege, so DDL via the request path is impossible
/// outside a transaction that was opened by the migration role (identity pool) or
/// a local transaction (SQLite). The migration role path is what the `embedding_keys::set`
/// call site uses, so this function must live inside that transaction to succeed.
async fn create_width_table(
    conn: &impl ConnectionTrait,
    dimension: i32,
) -> Result<(), YorishiroError> {
    let table_name = format!("content_entity_embeddings_{dimension}");

    // Check if the table already exists.
    let backend = conn.get_database_backend();
    let exists = if backend == sea_orm::DatabaseBackend::Sqlite {
        let rows = conn
            .execute_raw(Statement::from_sql_and_values(
                backend,
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=$1",
                [table_name.clone().into()],
            ))
            .await
            .internal()?;
        rows.rows_affected() > 0
    } else {
        let rows = conn
            .execute_raw(Statement::from_sql_and_values(
                backend,
                "SELECT count(*) FROM information_schema.tables WHERE table_name=$1",
                [table_name.clone().into()],
            ))
            .await
            .internal()?;
        rows.rows_affected() > 0
    };

    if exists {
        return Ok(());
    }

    // Create the table.
    if backend == sea_orm::DatabaseBackend::Sqlite {
        conn.execute_raw(Statement::from_sql_and_values(
            backend,
            &format!(
                "CREATE TABLE {table_name} (\
                 entity_id BLOB PRIMARY KEY, \
                 embedding BLOB, \
                 FOREIGN KEY (entity_id) REFERENCES content_entities(id) ON DELETE CASCADE)"
            ),
            [],
        ))
        .await
        .internal()?;
    } else {
        conn.execute_raw(Statement::from_sql_and_values(
            backend,
            &format!(
                "CREATE TABLE {table_name} (\
                 entity_id UUID PRIMARY KEY, \
                 embedding vector({dimension}))"
            ),
            [],
        ))
        .await
        .internal()?;

        let idx_name = format!("idx_{table_name}_hnsw");
        conn.execute_raw(Statement::from_sql_and_values(
            backend,
            &format!("CREATE INDEX {idx_name} ON {table_name} USING hnsw (embedding vector_cosine_ops)"),
            [],
        ))
        .await
        .internal()?;
    }

    Ok(())
}

/// Stores or replaces a workspace's own embedding provider assignment.
///
/// `expected_dimensions` is the workspace's own stamped `identity_workspaces.embedding_dimensions`,
/// or the deployment default's width for a workspace carrying no stamp. Assigning a provider of a
/// different width would leave old and new vectors at different widths in one column, surfacing only
/// when `sync_embedding`'s write-time guard (`services/embedding/sync.rs`) rejects a write.
/// Checking here, at the point an operator assigns the provider, surfaces the same mismatch immediately instead of on the next entity write.
///
/// **Table creation**: if the width-specific table (e.g. `content_entity_embeddings_1024`) does not
/// yet exist, this function creates it within the same transaction.
/// `yorishiro_app` has no CREATE privilege, so DDL via the request path is impossible
/// outside the migration-role connection. The controller calls this through `ctx.db`, which
/// is the identity pool (migration role), so the transaction has the required privileges.
#[allow(clippy::too_many_arguments)]
pub async fn set(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    base_url: &str,
    model: &str,
    api_key: &str,
    dimensions: i32,
    send_dimensions_param: bool,
    expected_dimensions: Option<i32>,
) -> Result<(), YorishiroError> {
    if api_key.trim().is_empty() {
        return Err(YorishiroError::ValidationFailed {
            message: "api_key must not be empty".into(),
            details: vec![],
            hint: "remove the configuration instead of storing an empty key".into(),
        });
    }
    if dimensions <= 0 {
        return Err(YorishiroError::ValidationFailed {
            message: "dimensions must be a positive integer".into(),
            details: vec![],
            hint: "set it to the embedding model's own output width".into(),
        });
    }
    if let Some(expected) = expected_dimensions
        && expected != dimensions
    {
        return Err(YorishiroError::ValidationFailed {
            message: format!(
                "this workspace holds {expected}-dimensional vectors, but the provider being \
                 assigned produces {dimensions}"
            ),
            details: vec![],
            hint: "assign a provider that matches the workspace's existing vectors, or \
                   re-embed the workspace after assigning this one"
                .into(),
        });
    }

    // Create the width-specific table if it does not yet exist.
    // This must run inside the same transaction as the key insert so that the
    // table is available before any entity write can use it, and because
    // yorishiro_app has no CREATE privilege (DDL must go through the
    // migration-role identity pool).
    create_width_table(conn, dimensions).await?;

    let base_url = base_url.trim().trim_end_matches('/');
    check_scheme(base_url)?;

    let active = ActiveModel {
        workspace_id: ActiveValue::Set(workspace_id),
        base_url: ActiveValue::Set(base_url.to_string()),
        model: ActiveValue::Set(model.to_string()),
        api_key: ActiveValue::Set(api_key.to_string()),
        dimensions: ActiveValue::Set(dimensions),
        send_dimensions_param: ActiveValue::Set(send_dimensions_param),
        updated_at: ActiveValue::Set(chrono::Utc::now().into()),
        ..Default::default()
    };
    Entity::insert(active)
        .on_conflict(
            OnConflict::column(Column::WorkspaceId)
                .update_columns([
                    Column::BaseUrl,
                    Column::Model,
                    Column::ApiKey,
                    Column::Dimensions,
                    Column::SendDimensionsParam,
                    Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// Removes a workspace's own assignment.
/// It falls back to the deployment default afterward.
pub async fn clear(conn: &impl ConnectionTrait, workspace_id: Uuid) -> Result<(), YorishiroError> {
    Entity::delete_many()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// What is configured, for an endpoint to report.
/// Never includes the key.
pub async fn describe(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<EmbeddingKeyDescription>, YorishiroError> {
    let row = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .one(conn)
        .await
        .internal()?;

    Ok(row.map(|row| EmbeddingKeyDescription {
        base_url: row.base_url,
        model: row.model,
        dimensions: row.dimensions,
        configured: true,
    }))
}

/// The credentials themselves, for building a provider.
/// `None` means the workspace has configured none, which `EmbeddingKeyResolver` reads as "fall back to the deployment default".
pub async fn get(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<EmbeddingKeyConfig>, YorishiroError> {
    let row = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .one(conn)
        .await
        .internal()?;

    Ok(row.map(|row| EmbeddingKeyConfig {
        base_url: row.base_url,
        api_key: row.api_key,
        model: row.model,
        dimensions: row.dimensions,
        send_dimensions_param: row.send_dimensions_param,
    }))
}
