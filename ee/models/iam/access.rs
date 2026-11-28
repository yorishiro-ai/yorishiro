use sea_orm::{DatabaseConnection, DatabaseTransaction};

use crate::error::{ResultExt, YorishiroError};
use crate::models::access_keys::AuthContext;

pub(crate) async fn workspace_transaction(
    db: &DatabaseConnection,
    actor: &AuthContext,
    action: crate::models::iam::policy::Action,
) -> Result<DatabaseTransaction, YorishiroError> {
    let workspace_id = actor.require_workspace()?;
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
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
/// Rejects inactive credentials, missing membership, or foreign workspaces.
pub(crate) async fn authorize_job_actor(
    txn: &DatabaseTransaction,
    identity: &DatabaseConnection,
    actor: &crate::models::job_results::JobActor,
    action: crate::models::iam::policy::Action,
) -> Result<Option<DatabaseTransaction>, YorishiroError> {
    let presented = AuthContext {
        access_key_id: actor.access_key_id,
        tenant_id: actor.tenant_id,
        workspace_id: Some(actor.workspace_id),
        user_id: actor.user_id,
    };
    crate::models::iam::access::authorize_workspace_action(txn, identity, &presented, action).await
}
