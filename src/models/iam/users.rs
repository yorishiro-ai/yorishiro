//! User operations that span the user, membership, group, policy and access key tables.

use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::access_keys::AuthContext;
use crate::models::iam::policy::{Action, Resource};
use crate::models::user_users;

const fn tenant_resource(actor: &AuthContext) -> Resource {
    Resource::Tenant {
        tenant_id: actor.tenant_id,
    }
}

/// # Errors
/// Rejects callers without `iam:ListUsers` and propagates persistence failures.
pub(crate) async fn list_as(
    db: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
    page: crate::models::pagination::ListParams,
) -> Result<Vec<user_users::Model>, YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
        Action::ListUsers,
        tenant_resource(actor),
    )
    .await?;
    let users =
        crate::models::tenant_memberships::list_member_users(&txn, actor.tenant_id, page).await?;
    txn.commit().await.internal()?;
    Ok(users)
}

/// # Errors
/// Rejects callers without `iam:GetUser` and users outside the tenant.
pub(crate) async fn get_as(
    db: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
    user_id: Uuid,
) -> Result<user_users::Model, YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
        Action::GetUser,
        tenant_resource(actor),
    )
    .await?;
    let user = user_users::find_in_tenant(&txn, actor.tenant_id, user_id).await?;
    txn.commit().await.internal()?;
    Ok(user)
}

/// Creates a user bound to the caller's tenant, signed in with tenant id, user name and password.
///
/// # Errors
/// Rejects callers without `iam:CreateUser`, invalid names or passwords and a name already used in the tenant.
pub(crate) async fn create_as(
    db: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
    user_name: &str,
    password: &str,
    display_name: Option<&str>,
) -> Result<user_users::Model, YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
        Action::CreateUser,
        tenant_resource(actor),
    )
    .await?;
    let user = crate::models::tenant_memberships::create_tenant_user_member(
        &txn,
        actor.tenant_id,
        user_name,
        password,
        display_name,
    )
    .await?;
    crate::models::api_events::record(
        &txn,
        actor,
        Action::CreateUser,
        serde_json::json!({"user_id": user.id, "user_name": user.user_name}),
    )
    .await?;
    txn.commit().await.internal()?;
    Ok(user)
}

/// Changes a user's display name; `None` clears it.
///
/// # Errors
/// Rejects callers without `iam:UpdateUser`, an invalid display name and users outside the tenant.
pub(crate) async fn update_as(
    db: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
    user_id: Uuid,
    display_name: Option<String>,
) -> Result<user_users::Model, YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
        Action::UpdateUser,
        tenant_resource(actor),
    )
    .await?;
    let user =
        user_users::update_display_name(&txn, actor.tenant_id, user_id, display_name).await?;
    crate::models::api_events::record(
        &txn,
        actor,
        Action::UpdateUser,
        serde_json::json!({"user_id": user_id}),
    )
    .await?;
    txn.commit().await.internal()?;
    Ok(user)
}

/// Deletes a user, removes their IAM associations and revokes their access keys.
///
/// # Errors
/// Rejects callers without `iam:DeleteUser` and protects the tenant root.
pub(crate) async fn delete_as(
    db: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
    user_id: Uuid,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
        Action::DeleteUser,
        tenant_resource(actor),
    )
    .await?;
    let user = user_users::find_in_tenant(&txn, actor.tenant_id, user_id).await?;
    crate::models::tenant_memberships::remove_member_in_transaction(&txn, actor.tenant_id, user_id)
        .await?;
    crate::models::api_events::record(
        &txn,
        actor,
        Action::DeleteUser,
        serde_json::json!({"user_id": user_id, "user_name": user.user_name}),
    )
    .await?;
    txn.commit().await.internal()?;
    Ok(())
}

/// Sets a user's password and revokes every key issued to them.
///
/// # Errors
/// Rejects callers without `iam:UpdateLoginProfile`, a weak password, the tenant root and users outside the tenant.
pub(crate) async fn update_login_profile_as(
    db: &sea_orm::DatabaseConnection,
    actor: &AuthContext,
    user_id: Uuid,
    password: &str,
) -> Result<(), YorishiroError> {
    let txn = crate::db::begin_write(db).await.internal()?;
    crate::models::iam::access::require_iam_action(
        &txn,
        actor,
        Action::UpdateLoginProfile,
        tenant_resource(actor),
    )
    .await?;
    if crate::models::iam_tenant_roots::Model::contains(&txn, actor.tenant_id, user_id).await? {
        return Err(YorishiroError::Conflict {
            message: "tenant root password recovery requires the administrator CLI".into(),
        });
    }
    user_users::reset_tenant_password(&txn, actor.tenant_id, user_id, password).await?;
    crate::models::api_events::record(
        &txn,
        actor,
        Action::UpdateLoginProfile,
        serde_json::json!({"user_id": user_id}),
    )
    .await?;
    txn.commit().await.internal()?;
    Ok(())
}
