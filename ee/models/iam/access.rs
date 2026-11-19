use sea_orm::{DatabaseConnection, DatabaseTransaction};

use crate::error::{ResultExt, YorishiroError};
use crate::models::api_keys::{ApiKeyScope, AuthContext};

pub(crate) async fn workspace_transaction(
    db: &DatabaseConnection,
    actor: &AuthContext,
    scope: ApiKeyScope,
    action: crate::models::iam::policy::Action,
) -> Result<DatabaseTransaction, YorishiroError> {
    let workspace_id = actor.require_workspace()?;
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
        scope,
        action,
        crate::models::iam::policy::Resource::Workspace {
            tenant_id: actor.tenant_id,
            workspace_id,
        },
    )
    .await?;
    Ok(txn)
}

/// # Errors
/// Rejects inactive credentials, missing membership, foreign workspaces or insufficient scope.
pub(crate) async fn authorize_job_actor(
    txn: &DatabaseTransaction,
    identity: &DatabaseConnection,
    actor: &crate::models::api_key_audit_log::AuditActor,
    required: ApiKeyScope,
    action: crate::models::iam::policy::Action,
) -> Result<Option<DatabaseTransaction>, YorishiroError> {
    let presented = AuthContext {
        api_key_id: actor.api_key_id,
        tenant_id: actor.tenant_id,
        workspace_id: Some(actor.workspace_id),
        user_id: actor.user_id,
        scope: required,
        audit: false,
    };
    crate::models::iam::access::authorize_workspace_action(
        txn, identity, &presented, required, action,
    )
    .await
}
