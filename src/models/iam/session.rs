//! The authenticated workspace transaction shared by REST handlers, MCP tools and model operations.

use sea_orm::{DatabaseConnection, DatabaseTransaction};

use crate::error::YorishiroError;
use crate::models::api_keys::{ApiKeyScope, AuthContext};
use crate::models::iam::access::{
    WorkspaceTarget, authorize_workspace_action, hide_inferred_resource, require_scope,
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
    /// Opens the workspace transaction an authenticated caller already named.
    ///
    /// # Errors
    /// Returns an error if the caller has no workspace or the transaction cannot begin.
    pub async fn begin(
        app: &loco_rs::app::AppContext,
        ctx: AuthContext,
        scope: ApiKeyScope,
    ) -> Result<Self, YorishiroError> {
        let workspace_id = ctx.require_workspace()?;
        // Touch independently: read handlers roll their workspace transaction back on drop.
        crate::models::api_keys::Entity::touch_authenticated(app, &ctx).await?;
        let txn = if scope >= ApiKeyScope::Write {
            crate::db::begin_workspace_write(app, ctx.tenant_id, workspace_id).await?
        } else {
            crate::db::begin_workspace(app, ctx.tenant_id, workspace_id).await?
        };
        Ok(Self {
            ctx,
            txn,
            identity: app.db.clone(),
            authorization: None,
            inferred_resource: false,
            scope,
        })
    }

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
        require_scope(&actor, scope)?;
        let inferred_resource = target.inferred_resource(&actor);
        actor.workspace_id = Some(
            target
                .resolve(&app.db, &actor, scope)
                .await
                .map_err(|error| hide_inferred_resource(inferred_resource, error))?,
        );
        let mut session = Self::begin(app, actor, scope).await?;
        session.inferred_resource = inferred_resource;
        Ok(session)
    }

    /// Opens a session for `target`, optionally checks `action`, runs a writing `op` and commits.
    ///
    /// This is the whole lifecycle of an ordinary write, so REST handlers and MCP tools call one model function each.
    ///
    /// # Errors
    /// Returns the session, authorization, operation or commit error.
    pub(crate) async fn run<T>(
        app: &loco_rs::app::AppContext,
        actor: AuthContext,
        scope: ApiKeyScope,
        target: WorkspaceTarget,
        action: Option<Action>,
        op: impl AsyncFnOnce(&Self) -> Result<T, YorishiroError>,
    ) -> Result<T, YorishiroError> {
        let mut session = Self::open(app, actor, scope, target).await?;
        if let Some(action) = action {
            session.require_action(action).await?;
        }
        let output = op(&session).await?;
        session.commit().await?;
        Ok(output)
    }

    /// Like [`Self::run`] for a read: nothing is committed, so the transaction rolls back on drop.
    ///
    /// # Errors
    /// Returns the session, authorization or operation error.
    pub(crate) async fn read<T>(
        app: &loco_rs::app::AppContext,
        actor: AuthContext,
        scope: ApiKeyScope,
        target: WorkspaceTarget,
        action: Action,
        op: impl AsyncFnOnce(&Self) -> Result<T, YorishiroError>,
    ) -> Result<T, YorishiroError> {
        let mut session = Self::open(app, actor, scope, target).await?;
        session.require_action(action).await?;
        op(&session).await
    }

    /// The workspace the session is bound to.
    ///
    /// # Errors
    /// Returns an error if the caller has no workspace, which `open` rules out.
    pub fn workspace_id(&self) -> Result<uuid::Uuid, YorishiroError> {
        self.ctx.require_workspace()
    }

    pub const fn txn(&self) -> &DatabaseTransaction {
        &self.txn
    }

    /// Checks the IAM action inside the session; the authorization transaction commits with it.
    ///
    /// # Errors
    /// Returns the authorization error, hiding an inferred resource.
    pub async fn require_action(&mut self, action: Action) -> Result<(), YorishiroError> {
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
    pub async fn commit(self) -> Result<(), YorishiroError> {
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
