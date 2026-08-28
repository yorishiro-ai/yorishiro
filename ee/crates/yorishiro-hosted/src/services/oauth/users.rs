//! Find-or-create for OAuth-provisioned users, plus the tenant/workspace auto-provisioning that happens the first time a given identity logs in.
//! Tenant, workspace, user and membership are all committed together in one transaction, guarded by `yorishiro_core::db::lock_for_update`.
//!
//! The two queries this needs (looking a user up by `(provider, subject_id)`, inserting a fresh OAuth-provisioned row) live in `models::oauth_users`, since `oauth_provider`/`oauth_subject_id` are columns this repo's own migration adds and `yorishiro_core`'s `identity_users` model knows nothing about the OAuth-specific lookups over them.

use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseTransaction, EntityTrait,
    PaginatorTrait,
};
use uuid::Uuid;
use yorishiro_core::error::{ResultExt, YorishiroError};
use yorishiro_core::models::_entities::identity_tenants;
use yorishiro_core::models::_entities::identity_workspaces as identity_workspaces_entity;
use yorishiro_core::models::content_schemas;
use yorishiro_core::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro_core::models::tenancy::{self, MembershipRole};

use crate::models::oauth_users::{
    CreateOauthUserError, OAuthUser, create_oauth_user, find_by_oauth_identity,
};

/// The workspace an OAuth login should issue its API key for, alongside the tenant/membership role that key's scope is derived from: everything the callback controller needs to call `IdentityApiKeys::create_api_key` exactly the way `POST /auth/login` does.
#[derive(Debug)]
pub struct ProvisionedLogin {
    pub user_id: Uuid,
    pub email: String,
    pub workspace_id: Uuid,
    pub role: MembershipRole,
}

/// Finds the user for `(provider, subject_id)`, creating both the user and a fresh tenant/workspace/membership if this is the identity's first login.
/// `email` is required (the design's whole point is looking users up by the ID token's `email` claim); a provider that omits it fails with `ValidationFailed` rather than silently provisioning an unusable account.
///
/// New tenants/workspaces are named after the email's local part (e.g. `alice` from `alice@example.com`) since there is no signup form here to ask for a name.
/// An admin can rename either afterward through the existing dashboard/API.
/// The new member's role is always `member`; an existing identity keeps whatever role it already holds.
/// The new workspace gets a schema from the built-in `general-notes` template rather than being left `schema_pending`, since an auto-provisioned SSO login has no admin present afterward to choose one.
/// `embedding` must be the deployment's actual model and width: the `content_entities.embedding` index is a fixed width, so a workspace stamped with the wrong one would fail every entity write's dimension check.
///
/// The whole first-login path (user row, tenant, workspace, membership) runs in one transaction, guarded by `lock_for_update` keyed on `(provider, subject_id)`: a crash partway through rolls everything back rather than leaving an orphaned user row with no tenant membership, and the lock also serializes concurrent first logins for the same identity.
/// `conn` is a `&DatabaseTransaction` rather than a `&impl ConnectionTrait` so that requirement is enforced by the signature: every advisory lock taken here and in the `create_workspace` it calls is transaction-scoped, and handed a pool each one would be released before the work it guards.
///
/// `YORISHIRO_MAX_TENANTS` is enforced here too, so auto-provisioning through SSO cannot bypass a deployment's tenant cap.
/// The check is not race-free against a *different* identity's first login landing between the count and the insert, since this function's lock is keyed per-identity, not globally; closing it fully is not worth a global lock on every OAuth login for a race window this narrow.
pub async fn find_or_create(
    conn: &DatabaseTransaction,
    provider: &str,
    subject_id: &str,
    email: Option<&str>,
    display_name: Option<&str>,
    embedding: (&str, i32),
) -> Result<ProvisionedLogin, YorishiroError> {
    yorishiro_core::db::lock_for_update(conn, &identity_lock_key(provider, subject_id))
        .await
        .internal()?;

    if let Some(existing) = find_by_oauth_identity(conn, provider, subject_id).await? {
        return resolve_existing_login(conn, existing).await;
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

    if let Some(max) = tenancy::max_tenants_from_env()? {
        let count = identity_tenants::Entity::find()
            .count(conn)
            .await
            .internal()?;
        if count >= max as u64 {
            return Err(YorishiroError::ScopeInsufficient {
                message: "this deployment has reached its tenant limit".into(),
                hint: "ask the operator to raise YORISHIRO_MAX_TENANTS, or sign in to an \
                       existing tenant with password instead"
                    .into(),
            });
        }
    }

    let tenant_name = tenant_name_from_email(email);
    let tenant_active = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set(tenant_name),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant_active, conn)
        .await
        .internal()?;

    let workspace = tenancy::create_workspace(
        conn,
        tenant.id,
        "default",
        crate::services::plan::Plan::Free
            .caps()
            .default_max_entities,
        None,
        Some(embedding),
    )
    .await?;

    // A schema requires an existing `workspace_id`, so it is created after the workspace and the workspace is then updated to point at it, moving its `status` to `active`.
    let definition = yorishiro_core::templates::get_template("general-notes")?;
    let (schema, _diff) =
        content_schemas::create_schema(conn, tenant.id, workspace.id, definition, None, None)
            .await?;

    let workspace_active = identity_workspaces_entity::ActiveModel {
        id: ActiveValue::Unchanged(workspace.id),
        schema_id: ActiveValue::Set(Some(schema.id)),
        status: ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    let workspace = workspace_active.update(conn).await.internal()?;

    tenancy::add_member(conn, tenant.id, user.id, MembershipRole::Member).await?;

    Ok(ProvisionedLogin {
        user_id: user.id,
        email: user.email,
        workspace_id: workspace.id,
        role: MembershipRole::Member,
    })
}

/// A deterministic key for `lock_for_update`, one per `(provider, subject_id)` pair.
fn identity_lock_key(provider: &str, subject_id: &str) -> String {
    format!("oauth:{provider}:{subject_id}")
}

/// Resolves a previously-provisioned OAuth identity to the workspace/role its login should use.
/// Always called under the identity's advisory lock, so a row found here is guaranteed to have finished provisioning its tenant, workspace and membership.
///
/// An account reaching more than one workspace is refused rather than silently resolved to an arbitrary one, which could hand a login to the wrong tenant's workspace.
async fn resolve_existing_login(
    conn: &impl ConnectionTrait,
    user: OAuthUser,
) -> Result<ProvisionedLogin, YorishiroError> {
    let mut workspaces = tenancy::list_workspaces_for_user(conn, user.id).await?;
    let workspace = match workspaces.len() {
        1 => workspaces.pop().expect("len() == 1 checked above"),
        0 => {
            return Err(YorishiroError::ScopeInsufficient {
                message: "this account is not a member of any tenant".into(),
                hint: "ask a tenant admin to add you as a member first".into(),
            });
        }
        _ => {
            return Err(YorishiroError::ValidationFailed {
                message: "this account has access to more than one workspace".into(),
                details: vec![],
                hint: "sign in with password and specify workspace_id explicitly".into(),
            });
        }
    };

    let tenant_id = tenancy::get_workspace_tenant(conn, workspace.id).await?;
    let role = tenancy::get_membership_role(conn, tenant_id, user.id)
        .await?
        .ok_or_else(|| YorishiroError::ScopeInsufficient {
            message: "this account is not a member of the tenant that owns its workspace".into(),
            hint: "ask a tenant admin to add you as a member first".into(),
        })?;

    Ok(ProvisionedLogin {
        user_id: user.id,
        email: user.email,
        workspace_id: workspace.id,
        role,
    })
}

fn tenant_name_from_email(email: &str) -> String {
    email.split('@').next().unwrap_or(email).to_string()
}
