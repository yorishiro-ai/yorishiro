//! A workspace's own LLM credentials, for the one feature that infers values.
//!
//! Reads and writes go through `ctx.db` (the migration-role connection), not the RLS-scoped tenant pool: `yorishiro_app` has no GRANT on this table, matching `template_templates`.
//!
//! `get` returns the key so the inference client can send it.
//! Nothing else does: `describe` is what an endpoint calls, and it reports the endpoint and model without the secret.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::workspace_llm_keys::{ActiveModel, Column, Entity};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;
use uuid::Uuid;

use crate::edition::ee::services::inference::InferenceConfig;

/// What a workspace has configured, without the key itself.
///
/// `api_key` is deliberately absent rather than masked: a masked value still travels through logs and proxies, and nothing a caller does needs it back.
#[derive(Clone, Serialize, sea_orm::FromQueryResult)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = LlmKeyResponse))]
pub(crate) struct LlmKeyDescription {
    pub(crate) base_url: String,
    pub(crate) model: String,
    /// Always true when present: the row cannot exist without a key.
    /// Callers use the absence of the whole description to mean "not configured".
    pub(crate) configured: bool,
}

/// Stores or replaces a workspace's credentials.
pub(crate) async fn set(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    base_url: &str,
    model: &str,
    api_key: &str,
) -> Result<(), YorishiroError> {
    let base_url = crate::edition::ee::data::provider::validate_credentials(base_url, api_key)?;

    let active = ActiveModel {
        workspace_id: ActiveValue::Set(workspace_id),
        base_url: ActiveValue::Set(base_url.to_string()),
        model: ActiveValue::Set(model.to_string()),
        api_key: ActiveValue::Set(api_key.to_string()),
        updated_at: ActiveValue::Set(crate::time::now().into()),
        ..Default::default()
    };
    Entity::insert(active)
        .on_conflict(
            OnConflict::column(Column::WorkspaceId)
                .update_columns([
                    Column::BaseUrl,
                    Column::Model,
                    Column::ApiKey,
                    Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// Removes a workspace's credentials.
/// Inference then refuses until one is configured again.
pub(crate) async fn clear(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    Entity::delete_many()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// What is configured, for an endpoint to report.
/// Never includes the key.
pub(crate) async fn describe(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<LlmKeyDescription>, YorishiroError> {
    Entity::find()
        .select_only()
        .columns([Column::BaseUrl, Column::Model])
        .expr_as(sea_orm::sea_query::Expr::value(true), "configured")
        .filter(Column::WorkspaceId.eq(workspace_id))
        .into_model::<LlmKeyDescription>()
        .one(conn)
        .await
        .internal()
}

/// The credentials themselves, for making a call.
///
/// `None` means the workspace has configured none.
/// Callers turn that into a refusal rather than a fallback: inferring nothing and filling defaults instead would look, to the caller, like inference that produced default-shaped answers.
pub(crate) async fn get(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<InferenceConfig>, YorishiroError> {
    let row = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .one(conn)
        .await
        .internal()?;

    Ok(row.map(|row| InferenceConfig {
        base_url: row.base_url,
        model: row.model,
        api_key: row.api_key,
    }))
}
