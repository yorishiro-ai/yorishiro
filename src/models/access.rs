//! Cross-table authorization decisions shared by every transport.
//! Credential verification and table mutations remain in their owning models.

use sea_orm::{ConnectionTrait, DatabaseTransaction};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::api_keys::{ApiKeyScope, AuthContext};
use crate::models::tenant_memberships::{self, MembershipRole};

/// Keeps the identity authorization fence alive until the workspace operation finishes.
pub(crate) async fn authorize_workspace_action(
    txn: &DatabaseTransaction,
    identity: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
    scope: ApiKeyScope,
    action: crate::models::iam_policy::Action,
) -> Result<Option<DatabaseTransaction>, YorishiroError> {
    let authorization = if txn.get_database_backend() == sea_orm::DatabaseBackend::Postgres {
        Some(crate::db::begin_write(identity).await.internal()?)
    } else {
        None
    };
    require_iam_action(
        authorization.as_ref().unwrap_or(txn),
        actor,
        scope,
        action,
        crate::models::iam_policy::Resource::Workspace {
            tenant_id: actor.tenant_id,
            workspace_id: actor.require_workspace()?,
        },
    )
    .await?;
    Ok(authorization)
}

/// Checks a concrete IAM action while holding the tenant membership fence.
pub(crate) async fn require_iam_action(
    txn: &DatabaseTransaction,
    actor: &AuthContext,
    required: ApiKeyScope,
    action: crate::models::iam_policy::Action,
    target: crate::models::iam_policy::Resource,
) -> Result<(), YorishiroError> {
    IamEvaluation::load(txn, actor, required)
        .await?
        .require(action, target)
        .await
}

/// Validated authorization state usable only while its transaction holds the membership fence.
/// Reload after changing credentials, root, policies or attachments in that transaction.
pub(crate) struct IamEvaluation<'a> {
    txn: &'a DatabaseTransaction,
    current: AuthContext,
    bound_workspace_id: Option<Uuid>,
    root: bool,
    policies: Vec<crate::models::iam_policy::Policy>,
}

impl<'a> IamEvaluation<'a> {
    pub(crate) async fn load(
        txn: &'a DatabaseTransaction,
        actor: &AuthContext,
        required: ApiKeyScope,
    ) -> Result<Self, YorishiroError> {
        use crate::models::iam_policy::Policy;
        use sea_orm::EntityTrait;
        crate::db::lock_for_update(txn, &format!("tenant-membership:{}", actor.tenant_id))
            .await
            .internal()?;
        let current = crate::models::api_keys::Model::current_context(txn, actor).await?;
        let key = crate::models::api_keys::Entity::find_by_id(current.api_key_id)
            .one(txn)
            .await
            .internal()?
            .ok_or(YorishiroError::Unauthenticated)?;
        let mut credential = current.clone();
        credential.scope = ApiKeyScope::from_db_str(&key.scope).ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!("unknown api key scope in database"))
        })?;
        require_scope(&credential, required)?;
        let user_id = current.user_id.ok_or(YorishiroError::Unauthenticated)?;
        let root =
            crate::models::iam_tenant_roots::Model::contains(txn, current.tenant_id, user_id)
                .await?;
        let documents = if root {
            Vec::new()
        } else {
            crate::models::iam_policies::Model::for_user(txn, current.tenant_id, user_id).await?
        };
        let mut policies = Vec::with_capacity(documents.len());
        for document in documents {
            let json = serde_json::to_string(&document.document).internal()?;
            // Ownership is verified for the concrete target; stale unrelated selectors must not block it.
            policies.push(Policy::parse(&json, current.tenant_id, |tenant, _| {
                tenant == current.tenant_id
            })?);
        }
        Ok(Self {
            txn,
            current,
            bound_workspace_id: key.workspace_id,
            root,
            policies,
        })
    }

    pub(crate) async fn require(
        &self,
        action: crate::models::iam_policy::Action,
        target: crate::models::iam_policy::Resource,
    ) -> Result<(), YorishiroError> {
        use crate::models::iam_policy::{Policy, Resource};
        if !action.applies_to(target) {
            return Err(YorishiroError::Unauthenticated);
        }
        if let Resource::Workspace { workspace_id, .. } = target
            && (self
                .current
                .workspace_id
                .is_some_and(|id| id != workspace_id)
                || self.bound_workspace_id.is_some_and(|id| id != workspace_id))
        {
            return Err(YorishiroError::ScopeInsufficient {
                message: "credential workspace does not match the target workspace".into(),
                hint: "use a credential for the target workspace".into(),
            });
        }
        let workspace = match target {
            Resource::Tenant { tenant_id } if tenant_id == self.current.tenant_id => {
                if self.bound_workspace_id.is_some() {
                    return Err(YorishiroError::ScopeInsufficient {
                        message: "workspace-bound credentials cannot access tenant resources"
                            .into(),
                        hint: "use a tenant-scoped credential".into(),
                    });
                }
                None
            }
            Resource::Workspace {
                tenant_id,
                workspace_id,
            } if tenant_id == self.current.tenant_id
                && self
                    .current
                    .workspace_id
                    .is_none_or(|id| id == workspace_id)
                && self.bound_workspace_id.is_none_or(|id| id == workspace_id) =>
            {
                let owner = crate::models::workspace_workspaces::get_workspace_tenant(
                    self.txn,
                    workspace_id,
                )
                .await?;
                if owner != tenant_id {
                    return Err(YorishiroError::Unauthenticated);
                }
                Some(workspace_id)
            }
            _ => return Err(YorishiroError::Unauthenticated),
        };
        if self.root {
            return Ok(());
        }
        if Policy::allows(
            &self.policies,
            self.current.tenant_id,
            action,
            target,
            |tenant, id| tenant == self.current.tenant_id && Some(id) == workspace,
        ) {
            Ok(())
        } else {
            Err(YorishiroError::ScopeInsufficient {
                message: "IAM policy does not permit this operation".into(),
                hint: "request an applicable IAM policy grant".into(),
            })
        }
    }
}

/// Persisted, secret-free identity and target information shared by transports.
pub struct AccessIdentity {
    pub current: AuthContext,
    pub workspace_name: Option<String>,
    pub tenant_name: String,
    pub stored_scope: ApiKeyScope,
    pub bound_workspace_id: Option<Uuid>,
    pub tenant_root: bool,
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
    let workspace = if let Some(workspace_id) = current.workspace_id {
        Some(
            crate::models::workspace_workspaces::Entity::find_by_id(workspace_id)
                .one(conn)
                .await
                .internal()?
                .filter(|workspace| workspace.tenant_id == current.tenant_id)
                .ok_or(YorishiroError::Unauthenticated)?,
        )
    } else {
        None
    };
    let tenant = crate::models::tenant_tenants::Model::find_by_id(conn, current.tenant_id).await?;
    let key = crate::models::api_keys::Entity::find_by_id(current.api_key_id)
        .one(conn)
        .await
        .internal()?
        .ok_or(YorishiroError::Unauthenticated)?;
    let stored_scope = ApiKeyScope::from_db_str(&key.scope).ok_or_else(|| {
        YorishiroError::Internal(anyhow::anyhow!("unknown api key scope in database"))
    })?;
    let tenant_root = match current.user_id {
        Some(user_id) => {
            crate::models::iam_tenant_roots::Model::contains(conn, current.tenant_id, user_id)
                .await?
        }
        None => false,
    };
    Ok(AccessIdentity {
        current,
        workspace_name: workspace.map(|workspace| workspace.name),
        tenant_name: tenant.name,
        stored_scope,
        bound_workspace_id: key.workspace_id,
        tenant_root,
    })
}

/// Credential ceilings and root identity, not authorization for a later request.
#[derive(serde::Serialize)]
pub struct AccessCapabilities {
    pub database_backend: &'static str,
    pub row_level_security: bool,
    pub scopes: Vec<ApiKeyScope>,
    pub audit: bool,
    pub tenant_root: bool,
    pub credential_scope: ApiKeyScope,
}

/// Inspects credential limits without inferring IAM authority from legacy roles.
///
/// # Errors
/// Rejects revoked credentials or missing membership and propagates database failures.
pub async fn capabilities(
    conn: &impl ConnectionTrait,
    actor: &AuthContext,
) -> Result<AccessCapabilities, YorishiroError> {
    let identity = identity(conn, actor).await?;
    let current = identity.current;
    let scopes = [
        ApiKeyScope::Read,
        ApiKeyScope::Write,
        ApiKeyScope::Schema,
        ApiKeyScope::Migration,
    ]
    .into_iter()
    .filter(|scope| require_scope(&current, *scope).is_ok())
    .collect();
    Ok(AccessCapabilities {
        database_backend: match conn.get_database_backend() {
            sea_orm::DatabaseBackend::Postgres => "postgres",
            sea_orm::DatabaseBackend::Sqlite => "sqlite",
            sea_orm::DatabaseBackend::MySql => "mysql",
            _ => "unknown",
        },
        row_level_security: conn.get_database_backend() == sea_orm::DatabaseBackend::Postgres,
        scopes,
        audit: require_audit(&current).is_ok(),
        tenant_root: identity.tenant_root,
        credential_scope: identity.stored_scope,
    })
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
    workspace_access_for_role(conn, workspace_id, user_id, tenant_role).await
}

pub(crate) async fn workspace_access_for_role(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    user_id: Uuid,
    tenant_role: tenant_memberships::MembershipRole,
) -> Result<Option<WorkspaceAccess>, YorishiroError> {
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

/// Serializes credential issuance with membership updates and checks attribution.
pub(crate) async fn require_key_attribution(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<(), YorishiroError> {
    crate::db::lock_for_update(txn, &format!("tenant-membership:{tenant_id}"))
        .await
        .internal()?;
    crate::models::tenant_tenants::Model::require_active(txn, tenant_id).await?;
    if let Some(user_id) = user_id {
        tenant_memberships::get_membership_role(txn, tenant_id, user_id)
            .await?
            .ok_or(YorishiroError::Unauthenticated)?;
    }
    Ok(())
}

/// Delegated credentials cannot exceed the issuer's scope or audit grant.
///
/// # Errors
/// Rejects inactive tenants and grants the issuer does not hold.
pub async fn require_key_grant(
    conn: &impl ConnectionTrait,
    issuer: &AuthContext,
    scope: ApiKeyScope,
    audit: bool,
) -> Result<(), YorishiroError> {
    crate::models::tenant_tenants::Model::require_active(conn, issuer.tenant_id).await?;
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
