//! A workspace's own LLM credentials, for the one feature that infers values (§FR-8-2).
//!
//! Reads and writes go through the migration-role pool, not the request role: `yorishiro_app`
//! has no GRANT on this table, so a query issued on a request connection fails at the
//! permission check rather than relying on an RLS policy being right. See the migration.
//!
//! [`get`] returns the key so the inference client can send it. Nothing else does: [`describe`]
//! is what an endpoint calls, and it reports the endpoint and model without the secret.

use sea_query::{Alias, Expr, Iden, OnConflict, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use serde::Serialize;
use sqlx::PgPool;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::services::inference::InferenceConfig;

#[derive(Iden)]
enum WorkspaceLlmKeys {
    Table,
    WorkspaceId,
    BaseUrl,
    Model,
    ApiKey,
    UpdatedAt,
}

/// What a workspace has configured, without the key itself.
///
/// The shape an endpoint returns. `api_key` is deliberately absent rather than masked: a masked
/// value still travels through logs and proxies, and nothing a caller does needs it back.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LlmKeyDescription {
    pub base_url: String,
    pub model: String,
    /// Always true when present -- the row cannot exist without a key. Callers use the absence
    /// of the whole description to mean "not configured".
    pub configured: bool,
}

/// Stores or replaces a workspace's credentials.
pub async fn set(
    pool: &PgPool,
    workspace_id: Uuid,
    base_url: &str,
    model: &str,
    api_key: &str,
) -> Result<(), YorishiroError> {
    if api_key.trim().is_empty() {
        return Err(YorishiroError::ValidationFailed {
            message: "api_key must not be empty".into(),
            details: vec![],
            hint: "remove the configuration instead of storing an empty key".into(),
        });
    }

    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), WorkspaceLlmKeys::Table))
        .columns([
            WorkspaceLlmKeys::WorkspaceId,
            WorkspaceLlmKeys::BaseUrl,
            WorkspaceLlmKeys::Model,
            WorkspaceLlmKeys::ApiKey,
        ])
        .values_panic([
            workspace_id.into(),
            base_url.trim_end_matches('/').into(),
            model.into(),
            api_key.into(),
        ])
        .on_conflict(
            OnConflict::column(WorkspaceLlmKeys::WorkspaceId)
                .update_columns([
                    WorkspaceLlmKeys::BaseUrl,
                    WorkspaceLlmKeys::Model,
                    WorkspaceLlmKeys::ApiKey,
                ])
                .value(WorkspaceLlmKeys::UpdatedAt, Expr::current_timestamp())
                .to_owned(),
        )
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_with(&sql, values)
        .execute(pool)
        .await
        .internal()?;
    Ok(())
}

/// Removes a workspace's credentials. Inference then refuses until one is configured again.
pub async fn clear(pool: &PgPool, workspace_id: Uuid) -> Result<(), YorishiroError> {
    let (sql, values) = Query::delete()
        .from_table((Alias::new("identity"), WorkspaceLlmKeys::Table))
        .and_where(Expr::col(WorkspaceLlmKeys::WorkspaceId).eq(workspace_id))
        .build_sqlx(PostgresQueryBuilder);

    sqlx::query_with(&sql, values)
        .execute(pool)
        .await
        .internal()?;
    Ok(())
}

/// What is configured, for an endpoint to report. Never includes the key.
pub async fn describe(
    pool: &PgPool,
    workspace_id: Uuid,
) -> Result<Option<LlmKeyDescription>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns([WorkspaceLlmKeys::BaseUrl, WorkspaceLlmKeys::Model])
        .from((Alias::new("identity"), WorkspaceLlmKeys::Table))
        .and_where(Expr::col(WorkspaceLlmKeys::WorkspaceId).eq(workspace_id))
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<(String, String)> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?;

    Ok(row.map(|(base_url, model)| LlmKeyDescription {
        base_url,
        model,
        configured: true,
    }))
}

/// The credentials themselves, for making a call.
///
/// `None` means the workspace has configured none. Callers turn that into a refusal rather than
/// a fallback -- inferring nothing and filling defaults instead would look, to the caller, like
/// inference that produced default-shaped answers.
pub async fn get(
    pool: &PgPool,
    workspace_id: Uuid,
) -> Result<Option<InferenceConfig>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns([
            WorkspaceLlmKeys::BaseUrl,
            WorkspaceLlmKeys::Model,
            WorkspaceLlmKeys::ApiKey,
        ])
        .from((Alias::new("identity"), WorkspaceLlmKeys::Table))
        .and_where(Expr::col(WorkspaceLlmKeys::WorkspaceId).eq(workspace_id))
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<(String, String, String)> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?;

    Ok(row.map(|(base_url, model, api_key)| InferenceConfig {
        base_url,
        model,
        api_key,
    }))
}

#[cfg(test)]
#[path = "../../tests/repositories/llm_keys.rs"]
mod tests;
