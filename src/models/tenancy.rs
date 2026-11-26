//! Explicit operations that compose more than one tenancy table.

use crate::error::OptionExt;
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
}

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
) -> Result<(crate::models::access_keys::CreatedTenantAccessKey, Uuid), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    let memberships = tenant_memberships::Entity::find()
        .filter(tenant_memberships::Column::UserId.eq(user_id))
        .all(&txn)
        .await
        .internal()?;
    let [membership] = memberships.as_slice() else {
        return Err(YorishiroError::AccessDenied {
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
    if !crate::models::tenant_memberships::is_member(&txn, tenant_id, user_id).await? {
        return Err(YorishiroError::Unauthenticated);
    }
    let key =
        crate::models::access_keys::Entity::create_login_key(&txn, tenant_id, user_id).await?;
    txn.commit().await.internal()?;
    Ok((key, tenant_id))
}

/// # Errors
/// Returns an error if account creation or membership insertion fails.
pub async fn register_member(
    txn: &DatabaseTransaction,
    tenant_id: Uuid,
    email: &str,
    password: &str,
    display_name: Option<&str>,
) -> Result<crate::models::user_users::Model, YorishiroError> {
    let user = crate::models::user_users::create_user(txn, email, password, display_name).await?;
    crate::models::tenant_memberships::add_member_in_transaction(txn, tenant_id, user.id).await?;
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
    let user = register_member(&txn, tenant.id, email, password, display_name).await?;
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
    use crate::models::tenant_tenants;

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
    let user = register_member(&txn, tenant.id, email, password, display_name).await?;
    assign_initial_root(&txn, tenant.id, user.id).await?;
    txn.commit().await.internal()?;
    Ok(SetupAccount {
        user_id: user.id,
        email: user.root_email(),
        tenant_id: tenant.id,
    })
}

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
#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct LoginWorkspace {
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub tenant_id: Uuid,
    pub tenant_name: String,
}

/// Discovers workspaces within the authenticated credential's tenant boundary.
///
/// # Errors
/// Rejects callers without `workspace:DiscoverWorkspaces` and propagates discovery failures.
pub(crate) async fn discover_workspaces(
    conn: &sea_orm::DatabaseConnection,
    actor: &crate::models::access_keys::AuthContext,
) -> Result<Vec<LoginWorkspace>, YorishiroError> {
    use crate::models::iam::access::IamEvaluation;
    use crate::models::iam::policy::{Action, Resource};
    let txn = crate::db::begin_write(conn).await.internal()?;
    let evaluation = IamEvaluation::load(&txn, actor).await?;
    evaluation
        .require(
            Action::DiscoverWorkspaces,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
    let workspaces = workspace_workspaces::Entity::find()
        .filter(workspace_workspaces::Column::TenantId.eq(actor.tenant_id))
        .find_also_related(crate::models::_entities::tenant_tenants::Entity)
        .all(&txn)
        .await
        .internal()?;
    let mut candidates = Vec::new();
    for (workspace, tenant) in workspaces {
        match evaluation
            .require(
                Action::GetWorkspace,
                Resource::Workspace {
                    tenant_id: actor.tenant_id,
                    workspace_id: workspace.id,
                },
            )
            .await
        {
            Ok(()) => {
                let tenant = tenant.ok_or(YorishiroError::Unauthenticated)?;
                candidates.push(LoginWorkspace {
                    workspace_id: workspace.id,
                    workspace_name: workspace.name,
                    tenant_id: tenant.id,
                    tenant_name: tenant.name,
                });
            }
            Err(YorishiroError::AccessDenied { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    txn.commit().await.internal()?;
    Ok(candidates)
}

#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct LoginMembership {
    pub tenant_id: Uuid,
    pub tenant_name: String,
}

/// Discovers recovery options only after the caller has verified credentials.
///
/// # Errors
/// Returns an error when membership or workspace discovery fails.
async fn login_options(
    conn: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<(Vec<LoginMembership>, Vec<LoginWorkspace>), YorishiroError> {
    use crate::models::_entities::tenant_tenants;
    let rows = tenant_memberships::Entity::find()
        .filter(tenant_memberships::Column::UserId.eq(user_id))
        .find_also_related(tenant_tenants::Entity)
        .all(conn)
        .await
        .internal()?;
    let mut memberships = Vec::with_capacity(rows.len());
    for (_, tenant) in rows {
        let tenant = tenant.or_internal("membership tenant missing")?;
        if tenant.disabled_at.is_some() {
            continue;
        }
        memberships.push(LoginMembership {
            tenant_id: tenant.id,
            tenant_name: tenant.name,
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
        let mut permissions = std::collections::BTreeMap::new();
        for membership in &memberships {
            let root = crate::models::iam_tenant_roots::Model::contains(
                conn,
                membership.tenant_id,
                user_id,
            )
            .await?;
            let mut policies = Vec::new();
            if !root {
                for record in crate::models::iam_policies::Model::effective_for_user(
                    conn,
                    membership.tenant_id,
                    user_id,
                )
                .await?
                {
                    policies.push(record.parse(membership.tenant_id)?);
                }
            }
            permissions.insert(membership.tenant_id, (root, policies));
        }
        for workspace in workspaces {
            let (root, policies) = &permissions[&workspace.tenant_id];
            if !root
                && !crate::models::iam::policy::Policy::allows(
                    policies,
                    workspace.tenant_id,
                    crate::models::iam::policy::Action::GetWorkspace,
                    crate::models::iam::policy::Resource::Workspace {
                        tenant_id: workspace.tenant_id,
                        workspace_id: workspace.id,
                    },
                    |tenant, id| tenant == workspace.tenant_id && id == workspace.id,
                )
            {
                continue;
            }
            let tenant_name = names
                .get(&workspace.tenant_id)
                .or_internal("workspace membership missing")?;
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
        .or_not_found(format!("tenant '{tenant_id}' was not found"))?;

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
        if count >= u64::from(max.max(0).cast_unsigned()) {
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

    active.insert(conn).await.map_err(|error| {
        YorishiroError::conflict_on_unique(error, "a workspace with this name already exists")
    })
}

pub(crate) async fn create_workspace_in_context(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::access_keys::AuthContext,
    name: &str,
    max_entities: Option<i32>,
    schema_id: Option<Uuid>,
) -> Result<workspace_workspaces::Model, YorishiroError> {
    let txn = crate::db::begin_write(&ctx.db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
        crate::models::iam::policy::Action::CreateWorkspace,
        crate::models::iam::policy::Resource::Tenant {
            tenant_id: actor.tenant_id,
        },
    )
    .await?;
    let quota = crate::data::quotas::for_tenant(ctx, actor.tenant_id).await?;
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

/// # Errors
/// Rejects revoked credentials, missing permission, foreign targets or the last workspace.
pub async fn delete_workspace_as(
    db: &sea_orm::DatabaseConnection,
    actor: &crate::models::access_keys::AuthContext,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    if crate::models::workspace_workspaces::get_workspace_tenant(&txn, workspace_id).await?
        != actor.tenant_id
    {
        return Err(YorishiroError::not_found("workspace not found"));
    }
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
        crate::models::iam::policy::Action::DeleteWorkspace,
        crate::models::iam::policy::Resource::Workspace {
            tenant_id: actor.tenant_id,
            workspace_id,
        },
    )
    .await?;
    crate::models::workspace_workspaces::delete_workspace(&txn, workspace_id).await?;
    txn.commit().await.internal()?;
    Ok(())
}
