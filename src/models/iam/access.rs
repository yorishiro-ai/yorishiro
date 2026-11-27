//! Cross-table authorization decisions shared by every transport.
//! Credential verification and table mutations remain in their owning models.

use crate::error::OptionExt;
use sea_orm::{ConnectionTrait, DatabaseTransaction, TransactionTrait};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::access_keys::AuthContext;
use crate::models::tenant_memberships;

pub(crate) async fn authorize_workspace_action(
    txn: &DatabaseTransaction,
    identity: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
    action: crate::models::iam::policy::Action,
) -> Result<Option<DatabaseTransaction>, YorishiroError> {
    let authorization = crate::db::begin_authorization(identity).await.internal()?;
    require_iam_action(
        authorization.as_ref().unwrap_or(txn),
        actor,
        action,
        crate::models::iam::policy::Resource::Workspace {
            tenant_id: actor.tenant_id,
            workspace_id: actor.require_workspace()?,
        },
    )
    .await?;
    Ok(authorization)
}

pub(crate) async fn check_workspace_target(
    db: &impl ConnectionTrait,
    actor: &AuthContext,
    workspace_id: Uuid,
) -> Result<Uuid, YorishiroError> {
    let tenant =
        crate::models::workspace_workspaces::get_workspace_tenant(db, workspace_id).await?;
    if tenant != actor.tenant_id
        || actor
            .workspace_id
            .is_some_and(|selected| selected != workspace_id)
    {
        return Err(YorishiroError::not_found("resource was not found"));
    }
    Ok(workspace_id)
}

pub(crate) enum WorkspaceTarget {
    Workspace(Uuid),
    Job(Uuid),
    SnapshotJob(Uuid),
    Schema(Uuid),
    Relation(Uuid),
    Permission(crate::models::iam::policy::Action),
}

impl WorkspaceTarget {
    pub(crate) const fn inferred_resource(&self, actor: &AuthContext) -> bool {
        actor.workspace_id.is_none() && !matches!(self, Self::Permission(_))
    }

    pub(crate) async fn resolve(
        self,
        db: &sea_orm::DatabaseConnection,
        actor: &AuthContext,
    ) -> Result<Uuid, YorishiroError> {
        if let Some(workspace_id) = actor.workspace_id {
            return Ok(workspace_id);
        }
        match self {
            Self::SnapshotJob(id) => {
                use crate::models::_entities::entity_snapshots::{Column, Entity};
                use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
                let workspaces = Entity::find()
                    .select_only()
                    .column(Column::WorkspaceId)
                    .distinct()
                    .filter(Column::JobId.eq(id))
                    .into_tuple::<Uuid>()
                    .all(db)
                    .await
                    .internal()?;
                let permitted = read_workspaces(
                    db,
                    actor,
                    crate::models::iam::policy::Action::UndoMigrationJob,
                )
                .await?
                .into_iter()
                .collect::<std::collections::HashSet<_>>();
                let matching: Vec<_> = workspaces
                    .into_iter()
                    .filter(|workspace| permitted.contains(workspace))
                    .collect();
                match matching.as_slice() {
                    [workspace] => Ok(*workspace),
                    [] => Err(YorishiroError::not_found("snapshot job was not found")),
                    _ => Err(YorishiroError::WorkspaceSelectionRequired {
                        workspaces: matching,
                    }),
                }
            }
            Self::Job(id) => {
                use sea_orm::EntityTrait;
                let record = crate::models::queue_job_lifecycles::Entity::find_by_id(id)
                    .one(db)
                    .await
                    .internal()?
                    .or_not_found("job was not found")?;
                let workspace = record
                    .workspace_id
                    .or_not_found("workspace job was not found")?;
                check_workspace_target(db, actor, workspace).await
            }
            Self::Workspace(id) => check_workspace_target(db, actor, id).await,
            Self::Schema(id) => {
                crate::models::schema_schemas::workspace_for_actor(db, actor, id).await
            }
            Self::Relation(id) => {
                crate::models::entity_relations::workspace_for_actor(db, actor, id).await
            }
            Self::Permission(action) => select_workspace(db, actor, action).await,
        }
    }
}

pub(crate) fn hide_inferred_resource(inferred: bool, error: YorishiroError) -> YorishiroError {
    if inferred
        && matches!(
            error,
            YorishiroError::AccessDenied { .. } | YorishiroError::NotFound { .. }
        )
    {
        YorishiroError::not_found("resource was not found")
    } else {
        error
    }
}

pub(crate) async fn require_iam_action(
    txn: &DatabaseTransaction,
    actor: &AuthContext,
    action: crate::models::iam::policy::Action,
    target: crate::models::iam::policy::Resource,
) -> Result<(), YorishiroError> {
    IamEvaluation::load(txn, actor)
        .await?
        .require(action, target)
        .await
}

pub(crate) async fn select_workspace(
    db: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
    action: crate::models::iam::policy::Action,
) -> Result<Uuid, YorishiroError> {
    if let Some(workspace) = actor.workspace_id {
        return Ok(workspace);
    }
    let txn = db.begin().await.internal()?;
    let evaluation = IamEvaluation::load(&txn, actor).await?;
    let workspaces = evaluation.workspace_ids(action).await?;
    txn.commit().await.internal()?;
    match workspaces.as_slice() {
        [workspace] => Ok(*workspace),
        [] => Err(YorishiroError::WorkspaceAccessDenied),
        _ => Err(YorishiroError::WorkspaceSelectionRequired { workspaces }),
    }
}

pub(crate) async fn read_workspaces(
    db: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
    action: crate::models::iam::policy::Action,
) -> Result<Vec<Uuid>, YorishiroError> {
    let txn = db.begin().await.internal()?;
    let evaluation = IamEvaluation::load(&txn, actor).await?;
    let workspaces = evaluation.selected_or_allowed(action).await?;
    txn.commit().await.internal()?;
    Ok(workspaces)
}

pub(crate) struct IamEvaluation<'a> {
    txn: &'a DatabaseTransaction,
    current: AuthContext,
    root: bool,
    root_session: bool,
    policies: Vec<crate::models::iam::policy::Policy>,
    policy_names: Vec<String>,
}

/// Rejects a caller that neither is the tenant root nor holds `AdministratorAccess`.
/// Granting `AdministratorAccess` is the one grant that cannot exceed what the grantor already holds.
pub(crate) async fn require_administrator(
    txn: &DatabaseTransaction,
    actor: &AuthContext,
) -> Result<(), YorishiroError> {
    let evaluation = IamEvaluation::load(txn, actor).await?;
    if evaluation.holds_administrator_access() {
        Ok(())
    } else {
        let principal = evaluation.principal_yrn().await;
        tracing::warn!(
            caller = %evaluation.current.yrn(),
            principal = %principal,
            "granting AdministratorAccess denied",
        );
        Err(YorishiroError::AccessDenied {
            message: "granting AdministratorAccess requires holding AdministratorAccess".into(),
            hint: "ask a tenant administrator to make this change".into(),
        })
    }
}

/// The result of asking whether a user's policies allow an action on a resource.
pub struct Simulation {
    pub decision: crate::models::iam::policy::Decision,
    pub matched_policies: Vec<String>,
}

/// Evaluates a hypothetical request for `user_id`, or for the caller when it is absent, with the code real requests use.
///
/// # Errors
/// Rejects callers without `iam:SimulatePrincipalPolicy`, unknown users and malformed resources.
pub(crate) async fn simulate_principal_policy(
    db: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
    user_id: Option<Uuid>,
    action: crate::models::iam::policy::Action,
    resource: Option<&str>,
) -> Result<Simulation, YorishiroError> {
    use crate::models::iam::policy::{Action, Decision, Policy, Resource};
    let txn = crate::db::begin_write(db).await.internal()?;
    require_iam_action(
        &txn,
        actor,
        Action::SimulatePrincipalPolicy,
        Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
    let tenant_id = actor.tenant_id;
    let user_id = user_id
        .or(actor.user_id)
        .ok_or(YorishiroError::Unauthenticated)?;
    if !tenant_memberships::is_member(&txn, tenant_id, user_id).await? {
        return Err(YorishiroError::not_found("user was not found"));
    }
    let target = match resource {
        None | Some("*") => {
            let tenant = Resource::Tenant { tenant_id };
            if action.applies_to(tenant) {
                tenant
            } else {
                Resource::AllWorkspaces { tenant_id }
            }
        }
        Some(yrn) => {
            let target = Resource::parse(yrn)?;
            if target.tenant_id() != tenant_id {
                return Err(YorishiroError::not_found("resource was not found"));
            }
            if !action.applies_to(target) {
                return Err(YorishiroError::ValidationFailed {
                    message: "the resource is not of the kind this action applies to".into(),
                    details: vec![],
                    hint: "use the tenant YRN or * for tenant actions, and a workspace YRN or * for workspace actions".into(),
                });
            }
            if let Resource::Workspace { workspace_id, .. } = target
                && crate::models::workspace_workspaces::get_workspace_tenant(&txn, workspace_id)
                    .await?
                    != tenant_id
            {
                return Err(YorishiroError::not_found("resource was not found"));
            }
            target
        }
    };
    if crate::models::iam_tenant_roots::Model::contains(&txn, tenant_id, user_id).await? {
        txn.commit().await.internal()?;
        return Ok(Simulation {
            decision: Decision::Allowed,
            matched_policies: Vec::new(),
        });
    }
    let effective =
        crate::models::iam_policies::Model::effective_for_user(&txn, tenant_id, user_id).await?;
    let policies = effective
        .iter()
        .map(|policy| policy.parse(tenant_id))
        .collect::<Result<Vec<Policy>, _>>()?;
    let evaluation = Policy::simulate(&policies, tenant_id, action, target);
    txn.commit().await.internal()?;
    Ok(Simulation {
        decision: evaluation.decision,
        matched_policies: evaluation
            .matched
            .into_iter()
            .map(|index| effective[index].name.clone())
            .collect(),
    })
}

impl<'a> IamEvaluation<'a> {
    async fn selected_or_allowed(
        &self,
        action: crate::models::iam::policy::Action,
    ) -> Result<Vec<Uuid>, YorishiroError> {
        if let Some(workspace_id) = self.current.workspace_id {
            self.require(
                action,
                crate::models::iam::policy::Resource::Workspace {
                    tenant_id: self.current.tenant_id,
                    workspace_id,
                },
            )
            .await?;
            Ok(vec![workspace_id])
        } else {
            self.workspace_ids(action).await
        }
    }
    pub(crate) async fn workspace_ids(
        &self,
        action: crate::models::iam::policy::Action,
    ) -> Result<Vec<Uuid>, YorishiroError> {
        use crate::models::_entities::workspace_workspaces::{Column, Entity};
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
        let workspaces = Entity::find()
            .filter(Column::TenantId.eq(self.current.tenant_id))
            .order_by_asc(Column::Id)
            .all(self.txn)
            .await
            .internal()?;
        let mut allowed = Vec::new();
        for workspace in workspaces {
            match self
                .require_with_owner(
                    action,
                    crate::models::iam::policy::Resource::Workspace {
                        tenant_id: self.current.tenant_id,
                        workspace_id: workspace.id,
                    },
                    true,
                )
                .await
            {
                Ok(()) => allowed.push(workspace.id),
                Err(YorishiroError::AccessDenied { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(allowed)
    }

    pub(crate) async fn load(
        txn: &'a DatabaseTransaction,
        actor: &AuthContext,
    ) -> Result<Self, YorishiroError> {
        use sea_orm::EntityTrait;
        crate::db::lock_for_update(txn, &format!("tenant-membership:{}", actor.tenant_id))
            .await
            .internal()?;
        let current = crate::models::access_keys::Model::current_context(txn, actor).await?;
        let key = crate::models::access_keys::Entity::find_by_id(current.access_key_id)
            .one(txn)
            .await
            .internal()?
            .ok_or(YorishiroError::Unauthenticated)?;
        let user_id = current.user_id.ok_or(YorishiroError::Unauthenticated)?;
        let root = key.has_root_principal(txn).await?;
        let effective = if root {
            Vec::new()
        } else {
            crate::models::iam_policies::Model::effective_for_user(txn, current.tenant_id, user_id)
                .await?
        };
        let mut policies = Vec::with_capacity(effective.len());
        let mut policy_names = Vec::with_capacity(effective.len());
        for policy in effective {
            policies.push(policy.parse(current.tenant_id)?);
            policy_names.push(policy.name);
        }
        Ok(Self {
            txn,
            root_session: key.is_login_session() && root,
            current,
            root,
            policies,
            policy_names,
        })
    }

    /// Whether the caller is the tenant root or holds the managed `AdministratorAccess`.
    fn holds_administrator_access(&self) -> bool {
        self.root
            || self
                .policy_names
                .iter()
                .any(|name| name == crate::models::iam::builtin::ADMINISTRATOR_ACCESS)
    }

    pub(crate) async fn require(
        &self,
        action: crate::models::iam::policy::Action,
        target: crate::models::iam::policy::Resource,
    ) -> Result<(), YorishiroError> {
        let result = self.require_with_owner(action, target, false).await;
        match &result {
            Ok(()) if tracing::Span::current().has_field("request_id") => tracing::info!(
                action = %action.qualified(),
                target = %target.yrn(),
                "IAM authorized the operation",
            ),
            Err(YorishiroError::AccessDenied { message, .. }) => {
                let principal = self.principal_yrn().await;
                tracing::warn!(
                    caller = %self.current.yrn(),
                    principal = %principal,
                    action = %action.qualified(),
                    target = %target.yrn(),
                    reason = %message,
                    "IAM denied the request",
                );
            }
            _ => {}
        }
        result
    }

    async fn require_with_owner(
        &self,
        action: crate::models::iam::policy::Action,
        target: crate::models::iam::policy::Resource,
        tenant_owns_workspace: bool,
    ) -> Result<(), YorishiroError> {
        use crate::models::iam::policy::{Decision, Policy, Resource};
        if !action.applies_to(target) {
            return Err(YorishiroError::Unauthenticated);
        }
        if let Resource::Workspace { workspace_id, .. } = target
            && self
                .current
                .workspace_id
                .is_some_and(|id| id != workspace_id)
        {
            return Err(YorishiroError::AccessDenied {
                message: "the selected workspace does not match the target workspace".into(),
                hint: "select the target workspace, or omit the selection".into(),
            });
        }
        let workspace = match target {
            Resource::Tenant { tenant_id } if tenant_id == self.current.tenant_id => None,
            Resource::Workspace {
                tenant_id,
                workspace_id,
            } if tenant_id == self.current.tenant_id
                && self
                    .current
                    .workspace_id
                    .is_none_or(|id| id == workspace_id) =>
            {
                if !tenant_owns_workspace
                    && crate::models::workspace_workspaces::get_workspace_tenant(
                        self.txn,
                        workspace_id,
                    )
                    .await?
                        != tenant_id
                {
                    return Err(YorishiroError::Unauthenticated);
                }
                Some(workspace_id)
            }
            _ => return Err(YorishiroError::Unauthenticated),
        };
        if self.root_session || (self.root && workspace.is_some()) {
            return Ok(());
        }
        let evaluation = Policy::simulate(&self.policies, self.current.tenant_id, action, target);
        let (message, hint) = match evaluation.decision {
            Decision::Allowed => return Ok(()),
            Decision::ImplicitDeny => (
                format!(
                    "no IAM policy allows {} on {}",
                    action.qualified(),
                    target.yrn()
                ),
                "request an applicable IAM policy grant",
            ),
            Decision::ExplicitDeny => {
                let policies = evaluation
                    .matched
                    .iter()
                    .map(|&index| self.policy_names[index].as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                (
                    format!(
                        "IAM policy explicitly denies {} on {}: {policies}",
                        action.qualified(),
                        target.yrn()
                    ),
                    "review the denying IAM policies",
                )
            }
        };
        Err(YorishiroError::AccessDenied {
            message,
            hint: hint.into(),
        })
    }

    /// The YRN of the user behind the credential, or of the access key when it has none.
    /// Only denials need it, so the lookup is best effort and never fails a request.
    async fn principal_yrn(&self) -> String {
        use sea_orm::EntityTrait;
        if let Some(user_id) = self.current.user_id
            && let Ok(Some(user)) = crate::models::user_users::Entity::find_by_id(user_id)
                .one(self.txn)
                .await
        {
            return user.yrn(self.current.tenant_id);
        }
        self.current.yrn()
    }
}

/// Persisted, secret-free identity and target information shared by transports.
pub struct AccessIdentity {
    pub current: AuthContext,
    pub yrn: String,
    pub workspace_name: Option<String>,
    pub tenant_name: String,
    pub tenant_root: bool,
}

/// Resolves names and the target workspace after revalidating the credential and membership.
///
/// # Errors
/// Rejects stale credentials and invalid target bindings or propagates lookup failures.
pub async fn identity(
    conn: &impl ConnectionTrait,
    actor: &AuthContext,
) -> Result<AccessIdentity, YorishiroError> {
    use sea_orm::EntityTrait;
    let current = crate::models::access_keys::Model::current_context(conn, actor).await?;
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
    let key = crate::models::access_keys::Entity::find_by_id(current.access_key_id)
        .one(conn)
        .await
        .internal()?
        .ok_or(YorishiroError::Unauthenticated)?;
    let tenant_root = key.is_login_session() && key.has_root_principal(conn).await?;
    let yrn = if let Some(user_id) = current.user_id {
        crate::models::user_users::Entity::find_by_id(user_id)
            .one(conn)
            .await
            .internal()?
            .ok_or(YorishiroError::Unauthenticated)?
            .yrn(current.tenant_id)
    } else {
        current.yrn()
    };
    Ok(AccessIdentity {
        current,
        yrn,
        workspace_name: workspace.map(|workspace| workspace.name),
        tenant_name: tenant.name,
        tenant_root,
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
