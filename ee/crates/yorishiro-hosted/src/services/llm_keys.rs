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

use crate::services::inference::InferenceConfig;
use yorishiro_core::{ResultExt, YorishiroError};

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

/// Refuses anything that is not `http://` or `https://`.
///
/// The value is interpolated into a request URL, so a `file://` or `gopher://` there points
/// reqwest at something that is not an HTTP conversation at all, and a scheme-less string
/// silently becomes a relative path. Checked here, at the point a person types it, so the
/// refusal names the field rather than surfacing later as a failed inference run.
///
/// **This is not SSRF protection.** Which hosts a workspace may name is unrestricted and is a
/// policy question for the operator; see docs/api.md. This only rules out URLs that could never
/// be a chat-completions endpoint.
pub(crate) fn check_scheme(base_url: &str) -> Result<(), YorishiroError> {
    let trimmed = base_url.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Ok(());
    }
    Err(YorishiroError::ValidationFailed {
        message: "base_url must start with http:// or https://".into(),
        details: vec![],
        hint: "for example https://api.openai.com/v1".into(),
    })
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
    // Normalise once, then validate and store the same string. Checking `base_url.trim()` and
    // storing `base_url` would let "  https://host  " pass and be persisted with its padding,
    // which `InferenceClient` then interpolates straight into a request URL -- the check and the
    // stored value have to be the same value.
    let base_url = base_url.trim().trim_end_matches('/');
    check_scheme(base_url)?;

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
            base_url.into(),
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
#[path = "../../tests/services/llm_keys.rs"]
mod tests;
