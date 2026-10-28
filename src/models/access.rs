//! Cross-table authorization decisions shared by every transport.
//! Credential verification and table mutations remain in their owning models.

use sea_orm::{ConnectionTrait, DatabaseTransaction};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::api_keys::{ApiKeyScope, AuthContext};
use crate::models::tenant_memberships::{self, MembershipRole};

/// Current grant summary, not authorization for a later request or a tool-policy decision.
#[derive(serde::Serialize)]
pub struct AccessCapabilities {
    pub scopes: Vec<ApiKeyScope>,
    pub audit: bool,
    pub tenant_admin: bool,
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
    })
}

/// Fences a queued mutation against credential revocation and membership changes.
pub(crate) async fn require_job_actor(
    txn: &DatabaseTransaction,
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
    let current = crate::models::api_keys::Model::current_context(
        txn,
        &AuthContext {
            api_key_id: actor.api_key_id,
            tenant_id: actor.tenant_id,
            workspace_id: actor.workspace_id,
            user_id: actor.user_id,
            scope: required,
            audit: false,
        },
    )
    .await?;
    require_scope(&current, required)
}

/// A role can restrict a stored grant, never widen it.
pub fn effective_scope(stored: ApiKeyScope, role: MembershipRole) -> ApiKeyScope {
    stored.min(role.max_scope())
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
