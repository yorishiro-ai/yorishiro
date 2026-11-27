//! A workspace's own LLM credentials, for the one feature that infers values.
//!
//! Reads and writes go through `ctx.db` (the migration-role connection), not the RLS-scoped tenant pool: `yorishiro_app` has no GRANT on this table, matching `template_templates`.
//!
//! `get` returns the key so the inference client can send it.
//! Nothing else does: `describe` is what an endpoint calls, and it reports the endpoint and model without the secret.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::workspace_llm_keys::{ActiveModel, Column, Entity};
use crate::models::access_keys::AuthContext;
use crate::models::iam::policy::Action;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;
use uuid::Uuid;

use crate::edition::ee::services::inference::{InferenceClient, InferenceConfig};

use super::super::data::provider::{check_client, check_error, validate_credentials};

/// What a workspace has configured, without the key itself.
///
/// `api_key` is absent rather than masked: a masked value still travels through logs and proxies.
#[derive(Clone, Serialize, sea_orm::FromQueryResult, schemars::JsonSchema)]
#[schemars(rename = "LlmKeyResponse")]
pub(crate) struct LlmKeyDescription {
    pub(crate) base_url: String,
    pub(crate) model: String,
    /// Always true when present: the row cannot exist without a key.
    /// Callers use the absence of the whole description to mean "not configured".
    pub(crate) configured: bool,
}

pub(crate) async fn set_as(
    db: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
    base_url: &str,
    model: &str,
    api_key: &str,
) -> Result<(), YorishiroError> {
    let action = Action::UpdateWorkspaceLlmProvider;
    super::iam::access::workspace_transaction(db, actor, action)
        .await?
        .commit()
        .await
        .internal()?;
    let base_url = validate_credentials(base_url, api_key)?;
    let config = InferenceConfig {
        base_url: base_url.into(),
        model: model.into(),
        api_key: api_key.into(),
    };
    InferenceClient::with_client(config.clone(), check_client()?)
        .propose_fields(&serde_json::json!({"sample": "provider check"}), &["label"])
        .await
        .map_err(|error| check_error(base_url, error))?;
    let txn = super::iam::access::workspace_transaction(db, actor, action).await?;
    set(&txn, actor.require_workspace()?, config).await?;
    txn.commit().await.internal()
}

pub(crate) async fn clear_as(
    db: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
) -> Result<(), YorishiroError> {
    let txn =
        super::iam::access::workspace_transaction(db, actor, Action::DeleteWorkspaceLlmProvider)
            .await?;
    clear(&txn, actor.require_workspace()?).await?;
    txn.commit().await.internal()
}

pub(crate) async fn describe_as(
    db: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
) -> Result<Option<LlmKeyDescription>, YorishiroError> {
    let txn = super::iam::access::workspace_transaction(db, actor, Action::GetWorkspaceLlmProvider)
        .await?;
    describe(&txn, actor.require_workspace()?).await
}

/// Stores or replaces a workspace's credentials.
async fn set(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    config: InferenceConfig,
) -> Result<(), YorishiroError> {
    let active = ActiveModel {
        workspace_id: ActiveValue::Set(workspace_id),
        base_url: ActiveValue::Set(config.base_url),
        model: ActiveValue::Set(config.model),
        api_key: ActiveValue::Set(config.api_key),
        updated_at: ActiveValue::Set(crate::data::time::now().into()),
        ..Default::default()
    };
    let active = sea_orm::ActiveModelBehavior::before_save(active, conn, true)
        .await
        .internal()?;
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
async fn clear(conn: &impl ConnectionTrait, workspace_id: Uuid) -> Result<(), YorishiroError> {
    Entity::delete_many()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

/// What is configured, for an endpoint to report.
/// Never includes the key.
async fn describe(
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
/// Callers refuse the request instead of falling back, so missing credentials never pass for inference.
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
