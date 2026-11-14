//! Explicit operations that compose more than one tenancy table.

use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait,
    PaginatorTrait, QueryFilter,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{tenant_memberships, workspace_workspaces};

use crate::models::workspace_workspaces::{WorkspaceSummary, workspace_count_lock_key};

pub(crate) struct SetupAccount {
    pub(crate) user_id: Uuid,
    pub(crate) email: String,
    pub(crate) tenant_id: Uuid,
    pub(crate) workspace_id: Uuid,
}

/// Records the initial root after its tenant membership has been created.
///
/// # Errors
/// Returns an error if the root is not a member or a root already exists.
pub async fn assign_initial_root(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), YorishiroError> {
    crate::models::iam_tenant_roots::Model::create(txn, tenant_id, user_id).await
}

/// Issues a tenant-scoped login credential after rechecking the account and membership.
///
/// # Errors
/// Rejects missing, ambiguous or disabled memberships and changed credentials.
pub async fn issue_login_key(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
    expected_password_hash: Option<&str>,
    expected_tenant: Option<Uuid>,
) -> Result<
    (
        crate::models::api_keys::CreatedTenantApiKey,
        Uuid,
        crate::models::api_keys::ApiKeyScope,
    ),
    YorishiroError,
> {
    let txn = crate::db::begin_write(db).await.internal()?;
    let memberships = tenant_memberships::Entity::find()
        .filter(tenant_memberships::Column::UserId.eq(user_id))
        .all(&txn)
        .await
        .internal()?;
    let [membership] = memberships.as_slice() else {
        return Err(YorishiroError::ScopeInsufficient {
            message: "this account must belong to exactly one tenant".into(),
            hint: "ask a tenant administrator to resolve the account membership".into(),
        });
    };
    let tenant_id = membership.tenant_id;
    if expected_tenant.is_some_and(|expected| expected != tenant_id) {
        return Err(YorishiroError::Unauthenticated);
    }
    crate::db::lock_for_update(&txn, &format!("tenant-membership:{tenant_id}"))
        .await
        .internal()?;
    crate::db::lock_for_update(&txn, &format!("user-membership:{user_id}"))
        .await
        .internal()?;
    let current = tenant_memberships::Entity::find()
        .filter(tenant_memberships::Column::UserId.eq(user_id))
        .all(&txn)
        .await
        .internal()?;
    if current.len() != 1 || current[0].tenant_id != tenant_id {
        return Err(YorishiroError::Unauthenticated);
    }
    let user = crate::models::user_users::Entity::find_by_id(user_id)
        .one(&txn)
        .await
        .internal()?
        .ok_or(YorishiroError::Unauthenticated)?;
    if expected_password_hash.is_some_and(|hash| user.password_hash.as_deref() != Some(hash))
        || user.tenant_id.is_some_and(|id| id != tenant_id)
    {
        return Err(YorishiroError::Unauthenticated);
    }
    let role = crate::models::tenant_memberships::get_membership_role(&txn, tenant_id, user_id)
        .await?
        .ok_or(YorishiroError::Unauthenticated)?;
    let scope = role.max_scope();
    let key =
        crate::models::api_keys::Entity::create_login_key(&txn, tenant_id, user_id, scope).await?;
    txn.commit().await.internal()?;
    Ok((key, tenant_id, scope))
}

/// Creates an account and its membership in the caller's transaction.
///
/// # Errors
/// Returns an error if account creation or membership insertion fails.
pub async fn register_member(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    email: &str,
    password: &str,
    display_name: Option<&str>,
    role: crate::models::tenant_memberships::MembershipRole,
) -> Result<crate::models::user_users::Model, YorishiroError> {
    let user = crate::models::user_users::create_user(txn, email, password, display_name).await?;
    crate::models::tenant_memberships::add_member_in_transaction(txn, tenant_id, user.id, role)
        .await?;
    Ok(user)
}

pub(crate) async fn signup_account(
    db: &sea_orm::DatabaseConnection,
    max_tenants: Option<i32>,
    email: &str,
    password: &str,
    display_name: Option<&str>,
) -> Result<(crate::models::user_users::Model, Uuid), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    let tenant = crate::models::tenant_tenants::create_tenant(
        &txn,
        display_name.unwrap_or(email),
        max_tenants,
    )
    .await?;
    let user = register_member(
        &txn,
        tenant.id,
        email,
        password,
        display_name,
        crate::models::tenant_memberships::MembershipRole::Owner,
    )
    .await?;
    assign_initial_root(&txn, tenant.id, user.id).await?;
    txn.commit().await.internal()?;
    Ok((user, tenant.id))
}

pub(crate) async fn setup_first_account(
    db: &sea_orm::DatabaseConnection,
    max_tenants: Option<i32>,
    email: &str,
    password: &str,
    display_name: Option<&str>,
) -> Result<SetupAccount, YorishiroError> {
    use crate::models::{tenant_memberships, tenant_tenants};

    if max_tenants.is_none() {
        return Err(YorishiroError::not_found(
            "the setup wizard is not enabled on this deployment",
        ));
    }
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::db::lock_for_update(&txn, "setup").await.internal()?;
    if tenant_tenants::count_tenants(&txn).await? > 0 {
        return Err(YorishiroError::Conflict {
            message: "this deployment has already been set up".into(),
        });
    }
    let tenant = tenant_tenants::create_tenant(&txn, "default", max_tenants).await?;
    let workspace = create_workspace(&txn, tenant.id, "default", None, None).await?;
    let user = register_member(
        &txn,
        tenant.id,
        email,
        password,
        display_name,
        tenant_memberships::MembershipRole::Owner,
    )
    .await?;
    assign_initial_root(&txn, tenant.id, user.id).await?;
    txn.commit().await.internal()?;
    Ok(SetupAccount {
        user_id: user.id,
        email: user.root_email(),
        tenant_id: tenant.id,
        workspace_id: workspace.id,
    })
}

/// Every workspace `user_id` can log into through inherited authority or a direct grant.
/// Used by sign-in to list explicit workspace choices.
///
/// Deliberately unpaginated: sign-in must show every reachable workspace.
/// A default `LIMIT` here would silently hide a membership from a user who holds more workspaces than the page size, rather than list them.
///
/// # Errors
/// Returns an error if the database query fails.
pub async fn list_workspaces_for_user(
    conn: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<Vec<WorkspaceSummary>, YorishiroError> {
    let (_, workspaces) = login_options(conn, user_id).await?;
    Ok(workspaces
        .into_iter()
        .map(|workspace| WorkspaceSummary {
            id: workspace.workspace_id,
            name: workspace.workspace_name,
        })
        .collect())
}

/// A login candidate carries tenant labels because workspace names are tenant-local.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoginWorkspace {
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub tenant_id: Uuid,
    pub tenant_name: String,
}

/// Discovers workspaces within the authenticated credential's tenant boundary.
///
/// # Errors
/// Rejects revoked credentials or membership and propagates discovery failures.
pub async fn discover_workspaces(
    conn: &impl ConnectionTrait,
    actor: &crate::models::api_keys::AuthContext,
) -> Result<Vec<LoginWorkspace>, YorishiroError> {
    let current = crate::models::api_keys::Model::current_context(conn, actor).await?;
    let key = crate::models::api_keys::Entity::find_by_id(current.api_key_id)
        .one(conn)
        .await
        .internal()?
        .ok_or(YorishiroError::Unauthenticated)?;
    if let Some(user_id) = current.user_id {
        let (_, workspaces) = login_options(conn, user_id).await?;
        return Ok(workspaces
            .into_iter()
            .filter(|workspace| {
                workspace.tenant_id == current.tenant_id
                    && key
                        .workspace_id
                        .is_none_or(|id| id == workspace.workspace_id)
            })
            .collect());
    }
    let workspaces = workspace_workspaces::Entity::find()
        .filter(workspace_workspaces::Column::TenantId.eq(current.tenant_id))
        .find_also_related(crate::models::_entities::tenant_tenants::Entity)
        .all(conn)
        .await
        .internal()?;
    workspaces
        .into_iter()
        .filter(|(workspace, _)| key.workspace_id.is_none_or(|id| id == workspace.id))
        .map(|(workspace, tenant)| {
            let tenant = tenant.ok_or(YorishiroError::Unauthenticated)?;
            Ok(LoginWorkspace {
                workspace_id: workspace.id,
                workspace_name: workspace.name,
                tenant_id: tenant.id,
                tenant_name: tenant.name,
            })
        })
        .collect()
}

#[derive(serde::Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoginMembership {
    pub tenant_id: Uuid,
    pub tenant_name: String,
    pub role: crate::models::tenant_memberships::MembershipRole,
}

/// Discovers recovery options only after the caller has verified credentials.
///
/// # Errors
/// Returns an error when membership or workspace discovery fails.
async fn login_options(
    conn: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<(Vec<LoginMembership>, Vec<LoginWorkspace>), YorishiroError> {
    use crate::models::_entities::{tenant_tenants, workspace_memberships};
    use crate::models::tenant_memberships::MembershipRole;
    let rows = tenant_memberships::Entity::find()
        .filter(tenant_memberships::Column::UserId.eq(user_id))
        .find_also_related(tenant_tenants::Entity)
        .all(conn)
        .await
        .internal()?;
    let mut memberships = Vec::with_capacity(rows.len());
    for (membership, tenant) in rows {
        let tenant = tenant.ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!("membership tenant missing"))
        })?;
        if tenant.disabled_at.is_some() {
            continue;
        }
        let role = crate::models::tenant_memberships::MembershipRole::from_db_str(&membership.role)
            .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("unknown membership role")))?;
        memberships.push(LoginMembership {
            tenant_id: tenant.id,
            tenant_name: tenant.name,
            role,
        });
    }
    memberships.sort_by_key(|membership| membership.tenant_id);
    let mut candidates = Vec::new();
    if !memberships.is_empty() {
        let workspaces = workspace_workspaces::Entity::find()
            .filter(
                workspace_workspaces::Column::TenantId
                    .is_in(memberships.iter().map(|membership| membership.tenant_id)),
            )
            .all(conn)
            .await
            .internal()?;
        let names: std::collections::BTreeMap<_, _> = memberships
            .iter()
            .map(|membership| (membership.tenant_id, &membership.tenant_name))
            .collect();
        let inherited: std::collections::BTreeSet<_> = memberships
            .iter()
            .filter(|membership| {
                matches!(
                    membership.role,
                    MembershipRole::Owner | MembershipRole::Admin
                )
            })
            .map(|membership| membership.tenant_id)
            .collect();
        let grants: std::collections::BTreeSet<_> = workspace_memberships::Entity::find()
            .filter(workspace_memberships::Column::UserId.eq(user_id))
            .all(conn)
            .await
            .internal()?
            .into_iter()
            .map(|grant| grant.workspace_id)
            .collect();
        for workspace in workspaces {
            if !inherited.contains(&workspace.tenant_id) && !grants.contains(&workspace.id) {
                continue;
            }
            let tenant_name = names.get(&workspace.tenant_id).ok_or_else(|| {
                YorishiroError::Internal(anyhow::anyhow!("workspace membership missing"))
            })?;
            candidates.push(LoginWorkspace {
                workspace_id: workspace.id,
                workspace_name: workspace.name,
                tenant_id: workspace.tenant_id,
                tenant_name: (*tenant_name).clone(),
            });
        }
    }
    candidates.sort_by_key(|candidate| (candidate.tenant_id, candidate.workspace_id));
    Ok((memberships, candidates))
}
/// Creates a workspace under `tenant_id`, enforcing the tenant's `max_workspaces` cap.
/// `None` means unlimited.
///
/// `conn` is a `&DatabaseTransaction` rather than a `&impl ConnectionTrait` because this takes `db::lock_for_update` before counting, and `pg_advisory_xact_lock` is transaction-scoped: handed a pool the lock would be released by the end of its own implicit transaction, before the count and insert it is meant to guard.
/// Taking the transaction in the signature makes passing a pool a compile error instead of a lock that silently does nothing.
/// It shares `delete_workspace`'s per-tenant lock key, so a create and a delete racing on the same tenant serialize against each other rather than each counting a total the other is about to change.
///
/// # Errors
/// Returns an error if the tenant is missing, its workspace limit is reached, or a database operation fails.
pub async fn create_workspace(
    conn: &DatabaseTransaction,
    tenant_id: Uuid,
    name: &str,
    max_entities: Option<i32>,
    schema_id: Option<Uuid>,
) -> Result<workspace_workspaces::Model, YorishiroError> {
    create_workspace_with_quota(conn, tenant_id, name, max_entities, schema_id, None).await
}

pub(crate) async fn create_workspace_with_quota(
    conn: &DatabaseTransaction,
    tenant_id: Uuid,
    name: &str,
    max_entities: Option<i32>,
    schema_id: Option<Uuid>,
    workspace_quota: Option<i32>,
) -> Result<workspace_workspaces::Model, YorishiroError> {
    use crate::models::_entities::tenant_tenants;
    use crate::models::workspace_workspaces::WorkspaceStatus;

    crate::db::lock_for_update(conn, &format!("tenant-membership:{tenant_id}"))
        .await
        .internal()?;
    crate::models::tenant_tenants::Model::require_active(conn, tenant_id).await?;
    let tenant = tenant_tenants::Entity::find_by_id(tenant_id)
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("tenant '{tenant_id}' was not found")))?;

    if let Some(max) = crate::data::quotas::effective_limit(tenant.max_workspaces, workspace_quota)
    {
        crate::db::lock_for_update(conn, &workspace_count_lock_key(tenant_id))
            .await
            .internal()?;
        let count = workspace_workspaces::Entity::find()
            .filter(workspace_workspaces::Column::TenantId.eq(tenant_id))
            .count(conn)
            .await
            .internal()?;
        if count >= max.max(0) as u64 {
            return Err(YorishiroError::Conflict {
                message: format!(
                    "tenant '{tenant_id}' has reached its workspace limit ({max}); \
                     raise max_workspaces or delete an existing workspace"
                ),
            });
        }
    }

    let active = workspace_workspaces::ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        name: ActiveValue::Set(name.to_string()),
        max_entities: ActiveValue::Set(max_entities),
        schema_id: ActiveValue::Set(schema_id),
        status: ActiveValue::Set(
            if schema_id.is_some() {
                WorkspaceStatus::Active.as_db_str()
            } else {
                WorkspaceStatus::SchemaPending.as_db_str()
            }
            .to_string(),
        ),
        ..Default::default()
    };

    active.insert(conn).await.internal()
}

pub(crate) async fn create_workspace_in_context(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::api_keys::AuthContext,
    name: &str,
    max_entities: Option<i32>,
    schema_id: Option<Uuid>,
) -> Result<workspace_workspaces::Model, YorishiroError> {
    let txn = crate::db::begin_write(&ctx.db).await.internal()?;
    crate::models::access::require_iam_action(
        &txn,
        actor,
        crate::models::api_keys::ApiKeyScope::Read,
        crate::models::iam_policy::Action::WorkspaceCreate,
        crate::models::iam_policy::Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
    let quota = crate::quotas(ctx, actor.tenant_id).await?;
    let workspace = create_workspace_with_quota(
        &txn,
        actor.tenant_id,
        name,
        max_entities,
        schema_id,
        quota.workspaces,
    )
    .await?;
    txn.commit().await.internal()?;
    Ok(workspace)
}

/// Deletes a workspace after revalidating IAM permission and target ownership.
///
/// # Errors
/// Rejects revoked credentials, missing permission, foreign targets or the last workspace.
pub async fn delete_workspace_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::api_keys::AuthContext,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    if crate::models::workspace_workspaces::get_workspace_tenant(&txn, workspace_id).await?
        != actor.tenant_id
    {
        return Err(YorishiroError::not_found("workspace not found"));
    }
    crate::models::access::require_iam_action(
        &txn,
        actor,
        crate::models::api_keys::ApiKeyScope::Read,
        crate::models::iam_policy::Action::WorkspaceDelete,
        crate::models::iam_policy::Resource::Workspace {
            tenant_id: actor.tenant_id,
            workspace_id,
        },
    )
    .await?;
    crate::models::workspace_workspaces::delete_workspace(&txn, workspace_id).await?;
    txn.commit().await.internal()?;
    Ok(())
}
