//! The authenticated workspace transaction shared by REST handlers and MCP tools.

use sea_orm::{DatabaseConnection, DatabaseTransaction};

use crate::controllers::middleware::auth;
use crate::error::YorishiroError;
use crate::models::api_keys::{ApiKeyScope, AuthContext};
use crate::models::iam::access::{
    WorkspaceTarget, authorize_workspace_action, hide_inferred_resource,
};
use crate::models::iam::policy::Action;

/// An authenticated caller bound to one workspace, with a transaction whose RLS context is already set.
///
/// There is no way to obtain a `DatabaseTransaction` on the tenant pool except through this type, which structurally prevents forgetting the scope check.
///
/// **A write handler must call `.commit().await?` before returning, or every write it made is silently discarded** (`DatabaseTransaction` rolls back on drop).
/// A read-only handler can just let it drop; a rollback of nothing-written is a no-op.
pub struct WorkspaceSession {
    pub ctx: AuthContext,
    txn: DatabaseTransaction,
    identity: DatabaseConnection,
    authorization: Option<DatabaseTransaction>,
    inferred_resource: bool,
    scope: ApiKeyScope,
}

impl WorkspaceSession {
    /// Verifies the scope, resolves the target workspace and opens the transaction.
    ///
    /// # Errors
    /// Returns the scope, target-resolution or transaction error.
    pub(crate) async fn open(
        app: &loco_rs::app::AppContext,
        mut actor: AuthContext,
        scope: ApiKeyScope,
        target: WorkspaceTarget,
    ) -> Result<Self, YorishiroError> {
        auth::require_scope(&actor, scope)?;
        let inferred_resource = target.inferred_resource(&actor);
        actor.workspace_id = Some(
            target
                .resolve(&app.db, &actor, scope)
                .await
                .map_err(|error| hide_inferred_resource(inferred_resource, error))?,
        );
        let (ctx, txn) = auth::RequestAuth::new(app)
            .begin(actor, scope >= ApiKeyScope::Write)
            .await?;
        Ok(Self {
            ctx,
            txn,
            identity: app.db.clone(),
            authorization: None,
            inferred_resource,
            scope,
        })
    }

    /// Wraps a transaction that authentication already opened for an explicit workspace.
    pub(crate) fn authenticated(
        app: &loco_rs::app::AppContext,
        (ctx, txn): (AuthContext, DatabaseTransaction),
        scope: ApiKeyScope,
    ) -> Self {
        Self {
            ctx,
            txn,
            identity: app.db.clone(),
            authorization: None,
            inferred_resource: false,
            scope,
        }
    }

    pub(crate) const fn txn(&self) -> &DatabaseTransaction {
        &self.txn
    }

    /// Checks the IAM action inside the session; the authorization transaction commits with it.
    ///
    /// # Errors
    /// Returns the authorization error, hiding an inferred resource.
    pub(crate) async fn require_action(&mut self, action: Action) -> Result<(), YorishiroError> {
        self.authorization =
            authorize_workspace_action(&self.txn, &self.identity, &self.ctx, self.scope, action)
                .await
                .map_err(|error| hide_inferred_resource(self.inferred_resource, error))?;
        Ok(())
    }

    /// Commits the workspace transaction and then the authorization one.
    ///
    /// # Errors
    /// Returns an internal error if either commit fails.
    pub(crate) async fn commit(self) -> Result<(), YorishiroError> {
        self.txn
            .commit()
            .await
            .map_err(|error| YorishiroError::Internal(error.into()))?;
        if let Some(authorization) = self.authorization {
            authorization
                .commit()
                .await
                .map_err(|error| YorishiroError::Internal(error.into()))?;
        }
        Ok(())
    }
}
