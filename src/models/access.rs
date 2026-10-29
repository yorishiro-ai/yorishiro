//! Cross-table authorization decisions shared by every transport.
//! Credential verification and table mutations remain in their owning models.

use sea_orm::{ConnectionTrait, DatabaseTransaction};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::api_keys::{ApiKeyScope, AuthContext};
use crate::models::tenant_memberships::{self, MembershipRole};

/// Persisted, secret-free identity and target information shared by transports.
pub struct AccessIdentity {
    pub current: AuthContext,
    pub workspace_name: String,
    pub tenant_name: String,
    pub stored_scope: ApiKeyScope,
    pub bound_workspace_id: Option<Uuid>,
    pub workspace_access: WorkspaceAccess,
}

/// Resolves names and binding after revalidating the credential and membership.
///
/// # Errors
/// Rejects stale credentials and invalid target bindings or propagates lookup failures.
pub async fn identity(
    conn: &impl ConnectionTrait,
    actor: &AuthContext,
) -> Result<AccessIdentity, YorishiroError> {
    use sea_orm::EntityTrait;
    let current = crate::models::api_keys::Model::current_context(conn, actor).await?;
    let workspace = crate::models::workspace_workspaces::Entity::find_by_id(current.workspace_id)
        .one(conn)
        .await
        .internal()?
        .filter(|workspace| workspace.tenant_id == current.tenant_id)
        .ok_or(YorishiroError::Unauthenticated)?;
    let tenant = crate::models::tenant_tenants::Model::find_by_id(conn, current.tenant_id).await?;
    let key = crate::models::api_keys::Entity::find_by_id(current.api_key_id)
        .one(conn)
        .await
        .internal()?
        .ok_or(YorishiroError::Unauthenticated)?;
    let grant = key.grant_metadata(conn).await?;
    let workspace_access = current_workspace_access(conn, &current).await?;
    Ok(AccessIdentity {
        current,
        workspace_name: workspace.name,
        tenant_name: tenant.name,
        stored_scope: grant.scope,
        bound_workspace_id: grant.workspace_id,
        workspace_access,
    })
}

/// Current grant summary, not authorization for a later request or a tool-policy decision.
#[derive(serde::Serialize)]
pub struct AccessCapabilities {
    pub scopes: Vec<ApiKeyScope>,
    pub audit: bool,
    pub tenant_admin: bool,
    pub workspace_access: WorkspaceAccess,
}

/// Inspects current grants using the same decisions as protected operations.
///
/// # Errors
/// Rejects revoked credentials or missing membership and propagates database failures.
pub async fn capabilities(
    conn: &impl ConnectionTrait,
    actor: &AuthContext,
) -> Result<AccessCapabilities, YorishiroError> {
    let current = crate::models::api_keys::Model::current_context(conn, actor).await?;
    let scopes = [
        ApiKeyScope::Read,
        ApiKeyScope::Write,
        ApiKeyScope::Schema,
        ApiKeyScope::Migration,
    ]
    .into_iter()
    .filter(|scope| require_scope(&current, *scope).is_ok())
    .collect();
    let tenant_admin = match require_tenant_admin(conn, current.tenant_id, current.user_id).await {
        Ok(()) => true,
        Err(YorishiroError::Unauthenticated | YorishiroError::ScopeInsufficient { .. }) => false,
        Err(error) => return Err(error),
    };
    Ok(AccessCapabilities {
        scopes,
        audit: require_audit(&current).is_ok(),
        tenant_admin,
        workspace_access: current_workspace_access(conn, &current).await?,
    })
}

/// Fences a queued mutation against credential revocation and membership changes.
pub(crate) async fn require_job_actor(
    txn: &DatabaseTransaction,
    identity: &impl ConnectionTrait,
    actor: &crate::models::api_key_audit_log::AuditActor,
    required: ApiKeyScope,
) -> Result<(), YorishiroError> {
    crate::db::lock_for_update(txn, &format!("tenant-membership:{}", actor.tenant_id))
        .await
        .internal()?;
    let tenant =
        crate::models::workspace_workspaces::get_workspace_tenant(txn, actor.workspace_id).await?;
    if tenant != actor.tenant_id {
        return Err(YorishiroError::Unauthenticated);
    }
    let presented = AuthContext {
        api_key_id: actor.api_key_id,
        tenant_id: actor.tenant_id,
        workspace_id: actor.workspace_id,
        user_id: actor.user_id,
        scope: required,
        audit: false,
    };
    // PostgreSQL membership is control-plane data; SQLite must read its own write transaction.
    let current = if txn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        crate::models::api_keys::Model::current_context(txn, &presented).await?
    } else {
        crate::models::api_keys::Model::current_context(identity, &presented).await?
    };
    require_scope(&current, required)
}

/// A role can restrict a stored grant, never widen it.
pub fn effective_scope(stored: ApiKeyScope, role: MembershipRole) -> ApiKeyScope {
    stored.min(role.max_scope())
}

/// Workspace authority and its provenance, independently of any credential's stored scope.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WorkspaceAccess {
    pub max_scope: ApiKeyScope,
    pub source: &'static str,
    pub workspace_admin: bool,
}

async fn current_workspace_access(
    conn: &impl ConnectionTrait,
    current: &AuthContext,
) -> Result<WorkspaceAccess, YorishiroError> {
    match current.user_id {
        Some(user_id) => workspace_access(conn, current.tenant_id, current.workspace_id, user_id)
            .await?
            .ok_or(YorishiroError::Unauthenticated),
        None => Ok(WorkspaceAccess {
            max_scope: current.scope,
            source: "service_key",
            workspace_admin: false,
        }),
    }
}

/// Resolves inherited tenant authority or an explicit workspace grant.
///
/// # Errors
/// Propagates workspace and membership lookup failures or rejects invalid stored roles.
pub async fn workspace_access(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<WorkspaceAccess>, YorishiroError> {
    let workspace_tenant =
        crate::models::workspace_workspaces::get_workspace_tenant(conn, workspace_id).await?;
    if workspace_tenant != tenant_id {
        return Ok(None);
    }
    crate::models::tenant_tenants::Model::require_active(conn, tenant_id).await?;
    let Some(tenant_role) =
        tenant_memberships::get_membership_role(conn, tenant_id, user_id).await?
    else {
        return Ok(None);
    };
    if tenant_role.administers_tenant() {
        return Ok(Some(WorkspaceAccess {
            max_scope: tenant_role.max_scope(),
            source: "tenant_role",
            workspace_admin: true,
        }));
    }
    let Some(role) =
        crate::models::workspace_memberships::Model::role(conn, workspace_id, user_id).await?
    else {
        return Ok(None);
    };
    Ok(Some(WorkspaceAccess {
        max_scope: tenant_role.max_scope().min(role.max_scope()),
        source: "workspace_membership",
        workspace_admin: role == crate::models::workspace_memberships::WorkspaceRole::Admin,
    }))
}

/// Revalidates the actor after acquiring the membership mutation fence.
pub(crate) async fn require_current_workspace_admin(
    txn: &DatabaseTransaction,
    actor: &AuthContext,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    crate::db::lock_for_update(txn, &format!("tenant-membership:{}", actor.tenant_id))
        .await
        .internal()?;
    let current = crate::models::api_keys::Model::current_context(txn, actor).await?;
    if let Some(user_id) = current.user_id
        && workspace_access(txn, current.tenant_id, workspace_id, user_id)
            .await?
            .is_some_and(|access| access.workspace_admin)
    {
        return Ok(());
    }
    Err(YorishiroError::ScopeInsufficient {
        message: "workspace administration required".into(),
        hint: "request an administrator grant for this workspace".into(),
    })
}

/// Revalidates the actor after acquiring the membership mutation fence.
pub(crate) async fn require_current_tenant_admin(
    txn: &DatabaseTransaction,
    actor: &AuthContext,
) -> Result<(), YorishiroError> {
    crate::db::lock_for_update(txn, &format!("tenant-membership:{}", actor.tenant_id))
        .await
        .internal()?;
    let current = crate::models::api_keys::Model::current_context(txn, actor).await?;
    require_tenant_admin(txn, current.tenant_id, current.user_id).await
}

/// Requires current owner authority under the shared membership fence.
pub(crate) async fn require_current_tenant_owner(
    txn: &DatabaseTransaction,
    actor: &AuthContext,
) -> Result<Uuid, YorishiroError> {
    require_current_tenant_admin(txn, actor).await?;
    let user_id = actor.user_id.ok_or(YorishiroError::Unauthenticated)?;
    if tenant_memberships::get_membership_role(txn, actor.tenant_id, user_id).await?
        != Some(MembershipRole::Owner)
    {
        return Err(YorishiroError::ScopeInsufficient {
            message: "only a tenant owner can retire the tenant".into(),
            hint: "ask a current tenant owner to perform this operation".into(),
        });
    }
    Ok(user_id)
}

/// Serializes credential issuance with membership updates and checks attribution.
pub(crate) async fn require_attributed_key_grant(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
    scope: ApiKeyScope,
) -> Result<(), YorishiroError> {
    crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
        .await
        .internal()?;
    crate::models::tenant_tenants::Model::require_active(txn, tenant_id).await?;
    if let Some(user_id) = user_id {
        let role = tenant_memberships::get_membership_role(txn, tenant_id, user_id)
            .await?
            .ok_or(YorishiroError::Unauthenticated)?;
        if effective_scope(scope, role) != scope {
            return Err(YorishiroError::ScopeInsufficient {
                message: format!(
                    "this user's tenant role permits at most {:?} scope keys",
                    role.max_scope()
                ),
                hint: "issue a lower-scoped key, or raise the user's tenant role".into(),
            });
        }
    }
    Ok(())
}

/// Delegated credentials cannot exceed either the issuer's grant or current role.
///
/// # Errors
/// Rejects non-administrators and grants the issuer does not hold.
pub async fn require_key_grant(
    conn: &impl ConnectionTrait,
    issuer: &AuthContext,
    scope: ApiKeyScope,
    audit: bool,
) -> Result<(), YorishiroError> {
    require_tenant_admin(conn, issuer.tenant_id, issuer.user_id).await?;
    require_scope(issuer, scope)?;
    if audit {
        require_audit(issuer)?;
    }
    Ok(())
}

/// Requires current tenant administration authority, independently of content scope.
///
/// # Errors
/// Rejects service identities, missing memberships and non-administrative roles.
pub async fn require_tenant_admin(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<(), YorishiroError> {
    let user_id = user_id.ok_or(YorishiroError::Unauthenticated)?;
    tenant_memberships::get_membership_role(conn, tenant_id, user_id)
        .await?
        .filter(|role| role.administers_tenant())
        .ok_or_else(|| YorishiroError::ScopeInsufficient {
            message: "this operation is restricted to tenant owners/admins".into(),
            hint: "ask a tenant owner to grant you the admin role".into(),
        })?;
    Ok(())
}

/// Enforces the effective content grant.
///
/// # Errors
/// Returns scope insufficient when the grant cannot perform the operation.
pub fn require_scope(ctx: &AuthContext, required: ApiKeyScope) -> Result<(), YorishiroError> {
    if ctx.scope.satisfies(required) {
        Ok(())
    } else {
        Err(YorishiroError::ScopeInsufficient {
            message: format!(
                "this operation requires {required:?} scope but the API key has {:?} scope",
                ctx.scope
            ),
            hint: "Reissue an API key with sufficient scope".into(),
        })
    }
}

/// Audit is an independent operator grant, not an ordered content scope.
///
/// # Errors
/// Returns scope insufficient when the explicit audit grant is absent.
pub fn require_audit(ctx: &AuthContext) -> Result<(), YorishiroError> {
    if ctx.audit {
        Ok(())
    } else {
        Err(YorishiroError::ScopeInsufficient {
            message: "this operation requires the audit grant, which this API key does not hold"
                .into(),
            hint: "reissue an API key with audit:true".into(),
        })
    }
}
