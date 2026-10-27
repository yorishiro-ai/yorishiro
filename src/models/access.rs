//! Cross-table authorization decisions shared by every transport.
//! Credential verification and table mutations remain in their owning models.

use sea_orm::ConnectionTrait;
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::models::api_keys::{ApiKeyScope, AuthContext};
use crate::models::tenant_memberships::{self, MembershipRole};

/// A role can restrict a stored grant, never widen it.
pub fn effective_scope(stored: ApiKeyScope, role: MembershipRole) -> ApiKeyScope {
    stored.min(role.max_scope())
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
