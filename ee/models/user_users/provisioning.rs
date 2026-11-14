//! Find-or-create for OAuth-provisioned users, plus the tenant/workspace auto-provisioning that happens the first time a given identity logs in.
//! Tenant, workspace, user and membership are all committed together in one transaction, guarded by `crate::db::lock_for_update`.
//!
//! The two queries this needs (looking a user up by `(provider, subject_id)`, inserting a fresh OAuth-provisioned row) live in `ee::models::user_users` rather than beside the base model, since `user_users` knows nothing about the OAuth-specific lookups over `oauth_provider`/`oauth_subject_id`.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::workspace_workspaces as workspace_workspaces_entity;
use crate::models::schema_schemas;
use crate::models::tenant_memberships::{self, MembershipRole};
use crate::models::workspace_workspaces::WorkspaceStatus;
use crate::models::{tenancy, tenant_tenants};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseTransaction};
use uuid::Uuid;

use super::{CreateOauthUserError, create_oauth_user, find_by_oauth_identity};

/// Finds the user for `(provider, subject_id)`, creating both the user and a fresh tenant/workspace/membership if this is the identity's first login.
/// `email` is required (the design's whole point is looking users up by the ID token's `email` claim); a provider that omits it fails with `ValidationFailed` rather than silently provisioning an unusable account.
///
/// New tenants/workspaces are named after the email's local part (e.g. `alice` from `alice@example.com`) since there is no signup form here to ask for a name.
/// An admin can rename either afterward through the existing dashboard/API.
/// The creator is the new tenant's owner; an existing identity keeps its current role.
/// The new workspace gets a schema from the built-in `general-notes` template rather than being left `schema_pending`, since an auto-provisioned SSO login has no admin present afterward to choose one.
/// The whole first-login path (user row, tenant, workspace, membership) runs in one transaction, guarded by `lock_for_update` keyed on `(provider, subject_id)`: a crash partway through rolls everything back rather than leaving an orphaned user row with no tenant membership, and the lock also serializes concurrent first logins for the same identity.
/// `conn` is a `&DatabaseTransaction` rather than a `&impl ConnectionTrait` so that requirement is enforced by the signature: every advisory lock taken here and in the `create_workspace` it calls is transaction-scoped, and handed a pool each one would be released before the work it guards.
///
/// The configured tenant limit is enforced here too, so auto-provisioning through SSO cannot bypass a deployment's tenant cap.
/// New tenants use the base model's backend constraints and tenant-count lock as well as this identity lock.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn find_or_create(
    conn: &DatabaseTransaction,
    provider: &str,
    subject_id: &str,
    email: Option<&str>,
    display_name: Option<&str>,
    max_tenants: Option<i32>,
) -> Result<Uuid, YorishiroError> {
    crate::db::lock_for_update(conn, &identity_lock_key(provider, subject_id))
        .await
        .internal()?;

    if let Some(existing) = find_by_oauth_identity(conn, provider, subject_id).await? {
        return Ok(existing.id);
    }

    let email = email.ok_or_else(|| YorishiroError::ValidationFailed {
        message: "the identity provider did not return an email claim".into(),
        details: vec![],
        hint: "this OAuth provider/app registration must be configured to release the 'email' \
               claim"
            .into(),
    })?;

    let user = match create_oauth_user(conn, email, display_name, provider, subject_id).await {
        Ok(user) => user,
        // The lookup above, under this same lock, confirmed no row exists for this identity yet, so this can only be a genuine email collision with an unrelated account.
        Err(CreateOauthUserError::UniqueViolation) => {
            return Err(YorishiroError::Conflict {
                message: format!(
                    "a user with email '{email}' already exists (sign in with password, or ask \
                     a tenant admin to link this SSO identity)"
                ),
            });
        }
        Err(CreateOauthUserError::Other(err)) => return Err(err),
    };

    let tenant_name = tenant_name_from_email(email);
    let tenant = tenant_tenants::create_tenant(conn, &tenant_name, max_tenants).await?;
    let workspace = tenancy::create_workspace(conn, tenant.id, "default", None, None).await?;

    // A schema requires an existing `workspace_id`, so it is created after the workspace and the workspace is then updated to point at it, moving its `status` to `active`.
    let definition = crate::data::templates::get_template("general-notes")?;
    let (schema, _diff) =
        schema_schemas::create_schema(conn, tenant.id, workspace.id, definition, None, None)
            .await?;

    let workspace_active = workspace_workspaces_entity::ActiveModel {
        id: ActiveValue::Unchanged(workspace.id),
        schema_id: ActiveValue::Set(Some(schema.id)),
        status: ActiveValue::Set(WorkspaceStatus::Active.as_db_str().to_string()),
        ..Default::default()
    };
    workspace_active.update(conn).await.internal()?;

    tenant_memberships::add_member_in_transaction(conn, tenant.id, user.id, MembershipRole::Owner)
        .await?;
    tenancy::assign_initial_root(conn, tenant.id, user.id).await?;

    Ok(user.id)
}

/// A deterministic key for `lock_for_update`, one per `(provider, subject_id)` pair.
fn identity_lock_key(provider: &str, subject_id: &str) -> String {
    format!("oauth:{provider}:{subject_id}")
}

fn tenant_name_from_email(email: &str) -> String {
    email.split('@').next().unwrap_or(email).to_string()
}
