//! Explicit operations that compose more than one tenancy table.

use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait,
    PaginatorTrait, QueryFilter, QuerySelect, QueryTrait,
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
    crate::models::tenant_memberships::add_member(txn, tenant_id, user.id, role).await?;
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
    txn.commit().await.internal()?;
    Ok(SetupAccount {
        user_id: user.id,
        email: user.email,
        tenant_id: tenant.id,
        workspace_id: workspace.id,
    })
}

/// Every workspace `user_id` can log into: the union of workspaces under every tenant they hold a membership in.
/// Used by `/auth/login` to resolve `workspace_id` automatically when the caller can only reach one.
///
/// Deliberately unpaginated: this drives sign-in, not a browsing UI, and its own caller (`resolve_login_workspace`) needs the true, complete set to tell "exactly one, resolve to it" from "more than one, the caller must say which."
/// A default `LIMIT` here would silently hide a membership from a user who holds more workspaces than the page size, rather than list them.
///
/// # Errors
/// Returns an error if the database query fails.
pub async fn list_workspaces_for_user(
    conn: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<Vec<WorkspaceSummary>, YorishiroError> {
    let memberships = tenant_memberships::Entity::find()
        .select_only()
        .column(tenant_memberships::Column::TenantId)
        .filter(tenant_memberships::Column::UserId.eq(user_id))
        .into_query();
    workspace_workspaces::Entity::find()
        .select_only()
        .columns([
            workspace_workspaces::Column::Id,
            workspace_workspaces::Column::Name,
        ])
        .filter(workspace_workspaces::Column::TenantId.in_subquery(memberships))
        .into_model::<WorkspaceSummary>()
        .all(conn)
        .await
        .internal()
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
    use crate::models::_entities::tenant_tenants;
    use crate::models::workspace_workspaces::WorkspaceStatus;

    let tenant = tenant_tenants::Entity::find_by_id(tenant_id)
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("tenant '{tenant_id}' was not found")))?;

    if let Some(max) = tenant.max_workspaces {
        crate::db::lock_for_update(conn, &workspace_count_lock_key(tenant_id))
            .await
            .internal()?;
        let count = workspace_workspaces::Entity::find()
            .filter(workspace_workspaces::Column::TenantId.eq(tenant_id))
            .count(conn)
            .await
            .internal()?;
        if count >= max as u64 {
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
