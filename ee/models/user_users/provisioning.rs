//! Provisions an OAuth user and tenant together on first login without creating a workspace.
//!
//! The two queries this needs (looking a user up by `(provider, subject_id)`, inserting a fresh OAuth-provisioned row) live in `ee::models::user_users` rather than beside the base model, since `user_users` knows nothing about the OAuth-specific lookups over `oauth_provider`/`oauth_subject_id`.

use crate::error::{ResultExt, YorishiroError};
use crate::models::tenant_memberships;
use crate::models::{tenancy, tenant_tenants};
use sea_orm::DatabaseTransaction;
use uuid::Uuid;

use super::{CreateOauthUserError, create_oauth_user, find_by_oauth_identity};

/// Finds the user for `(provider, subject_id)` or creates a new tenant with that user as its root.
///
/// `email` is required (the design's whole point is looking users up by the ID token's `email` claim); a provider that omits it fails with `ValidationFailed` rather than silently provisioning an unusable account.
///
/// The new tenant is named after the email's local part.
/// The identity lock serializes concurrent first logins, and the transaction keeps account creation and membership atomic.
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
    tenant_memberships::add_member_in_transaction(conn, tenant.id, user.id).await?;
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
