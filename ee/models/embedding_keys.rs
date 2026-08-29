//! A workspace's own embedding provider assignment, for pointing a tenant at a different compute backend than the deployment default.
//!
//! Reads and writes go through `ctx.db` (the migration-role connection), not the RLS-scoped tenant pool: `yorishiro_app` has no GRANT on this table, matching `identity_workspace_llm_keys`.
//!
//! A workspace with no row here uses the deployment default (`WorkspaceEmbeddingResolver::resolve` returns `None`); this module never falls back on its own, so the caller (`EmbeddingKeyResolver`) decides that.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::identity_workspace_embedding_keys::{ActiveModel, Column, Entity};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
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

/// Stores or replaces a workspace's own embedding provider assignment.
///
/// `expected_dimensions` is the workspace's own stamped `identity_workspaces.embedding_dimensions`, when it has one: a workspace created before this assignment existed carries the deployment default's dimension count, and pointing it at a provider that produces a different width would leave existing vectors and any newly-embedded ones at different widths in the same column, discovered only when `sync_embedding`'s own write-time guard (`services/embedding/sync.rs`) rejects a write.
/// Checking here, at the point an operator assigns the provider, surfaces the same mismatch immediately instead of on the next entity write.
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
