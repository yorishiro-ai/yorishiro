//! Cross-table authorization decisions shared by every transport.
//! Credential verification and table mutations remain in their owning models.

use sea_orm::{ConnectionTrait, DatabaseTransaction};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::api_keys::{ApiKeyScope, AuthContext};
use crate::models::tenant_memberships;

pub(crate) async fn authorize_workspace_action(
    txn: &DatabaseTransaction,
    identity: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
    scope: ApiKeyScope,
    action: crate::models::iam_policy::Action,
) -> Result<Option<DatabaseTransaction>, YorishiroError> {
    let authorization = crate::db::begin_authorization(identity).await.internal()?;
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
        Self::load_with(txn, actor, required, false).await
    }

    pub(crate) async fn load_for_assumption(
        txn: &'a DatabaseTransaction,
        actor: &AuthContext,
    ) -> Result<Self, YorishiroError> {
        Self::load_with(txn, actor, ApiKeyScope::Read, true).await
    }

    async fn load_with(
        txn: &'a DatabaseTransaction,
        actor: &AuthContext,
        required: ApiKeyScope,
        assumption: bool,
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
        if assumption && key.role_id.is_some() {
            return Err(YorishiroError::ScopeInsufficient {
                message: "a role session cannot assume another role".into(),
                hint: "assume the role with the original credential".into(),
            });
        }
        let mut root = false;
        let documents = if let Some(role_id) = key.role_id {
            // A role session is evaluated by the role's policies alone; a deleted role leaves nothing to evaluate.
            crate::models::iam_policies::Model::for_role(txn, current.tenant_id, role_id).await?
        } else if let Some(user_id) = current.user_id {
            root =
                crate::models::iam_tenant_roots::Model::contains(txn, current.tenant_id, user_id)
                    .await?;
            if assumption && root {
                return Err(YorishiroError::ScopeInsufficient {
                    message: "the tenant root cannot assume roles".into(),
                    hint: "the root already holds every permission".into(),
                });
            }
            if root {
                Vec::new()
            } else {
                crate::models::iam_policies::Model::for_user(txn, current.tenant_id, user_id)
                    .await?
            }
        } else if assumption {
            Vec::new()
        } else {
            return Err(YorishiroError::Unauthenticated);
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
            Resource::Tenant { tenant_id }
            | Resource::Role { tenant_id, .. }
            | Resource::AllRoles { tenant_id }
                if tenant_id == self.current.tenant_id =>
            {
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

#[derive(serde::Serialize)]
pub struct AccessCapabilities {
    pub database_backend: &'static str,
    pub row_level_security: bool,
    pub scopes: Vec<ApiKeyScope>,
    pub audit: bool,
    pub tenant_root: bool,
    pub credential_scope: ApiKeyScope,
}

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
        row_level_security: crate::db::is_postgres(conn),
        scopes,
        audit: require_audit(&current).is_ok(),
        tenant_root: identity.tenant_root,
        credential_scope: identity.stored_scope,
    })
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
    if let Some(user_id) = user_id
        && !tenant_memberships::is_member(txn, tenant_id, user_id).await?
    {
        return Err(YorishiroError::Unauthenticated);
    }
    Ok(())
}

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
