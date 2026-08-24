//! A workspace's own LLM credentials, for the one feature that infers values.
//!
//! Reads and writes go through `ctx.db` (the migration-role connection), not the RLS-scoped tenant pool: `yorishiro_app` has no GRANT on this table, matching `identity_templates`.
//!
//! [`get`] returns the key so the inference client can send it.
//! Nothing else does: [`describe`] is what an endpoint calls, and it reports the endpoint and model without the secret.

use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
use serde::Serialize;
use uuid::Uuid;
use yorishiro_core::error::{ResultExt, YorishiroError};

use crate::services::inference::InferenceConfig;

/// What a workspace has configured, without the key itself.
///
/// `api_key` is deliberately absent rather than masked: a masked value still travels through logs and proxies, and nothing a caller does needs it back.
#[derive(Debug, Clone, Serialize)]
pub struct LlmKeyDescription {
    pub base_url: String,
    pub model: String,
    /// Always true when present: the row cannot exist without a key.
    /// Callers use the absence of the whole description to mean "not configured".
    pub configured: bool,
}

#[derive(FromQueryResult)]
struct DescribeRow {
    base_url: String,
    model: String,
}

#[derive(FromQueryResult)]
struct GetRow {
    base_url: String,
    model: String,
    api_key: String,
}

/// Refuses anything that is not `http://` or `https://`.
///
/// The value is interpolated into a request URL, so a `file://` or `gopher://` there points reqwest at something that is not an HTTP conversation at all, and a scheme-less string silently becomes a relative path.
/// Checked here, at the point a person types it, so the refusal names the field rather than surfacing later as a failed inference run.
///
/// **This is not SSRF protection.** Which hosts a workspace may name is unrestricted and is a policy question for the operator; see `ee/docs/api.md`.
/// This only rules out URLs that could never be a chat-completions endpoint.
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

/// Stores or replaces a workspace's credentials.
pub async fn set(
    conn: &impl ConnectionTrait,
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
    // Normalise once, then validate and store the same string: checking `base_url.trim()` and storing `base_url` would let "  https://host  " pass and be persisted with its padding, which `InferenceClient` then interpolates straight into a request URL, so the check and the stored value have to be the same value.
    let base_url = base_url.trim().trim_end_matches('/');
    check_scheme(base_url)?;

    conn.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO identity_workspace_llm_keys (workspace_id, base_url, model, api_key) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (workspace_id) DO UPDATE \
         SET base_url = EXCLUDED.base_url, model = EXCLUDED.model, api_key = EXCLUDED.api_key, \
             updated_at = now()",
        [
            workspace_id.into(),
            base_url.into(),
            model.into(),
            api_key.into(),
        ],
    ))
    .await
    .internal()?;
    Ok(())
}

/// Removes a workspace's credentials. Inference then refuses until one is configured again.
pub async fn clear(conn: &impl ConnectionTrait, workspace_id: Uuid) -> Result<(), YorishiroError> {
    conn.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "DELETE FROM identity_workspace_llm_keys WHERE workspace_id = $1",
        [workspace_id.into()],
    ))
    .await
    .internal()?;
    Ok(())
}

/// What is configured, for an endpoint to report. Never includes the key.
pub async fn describe(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<LlmKeyDescription>, YorishiroError> {
    let row = DescribeRow::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT base_url, model FROM identity_workspace_llm_keys WHERE workspace_id = $1",
        [workspace_id.into()],
    ))
    .one(conn)
    .await
    .internal()?;

    Ok(row.map(|row| LlmKeyDescription {
        base_url: row.base_url,
        model: row.model,
        configured: true,
    }))
}

/// The credentials themselves, for making a call.
///
/// `None` means the workspace has configured none.
/// Callers turn that into a refusal rather than a fallback: inferring nothing and filling defaults instead would look, to the caller, like inference that produced default-shaped answers.
pub async fn get(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<InferenceConfig>, YorishiroError> {
    let row = GetRow::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT base_url, model, api_key FROM identity_workspace_llm_keys WHERE workspace_id = $1",
        [workspace_id.into()],
    ))
    .one(conn)
    .await
    .internal()?;

    Ok(row.map(|row| InferenceConfig {
        base_url: row.base_url,
        model: row.model,
        api_key: row.api_key,
    }))
}
