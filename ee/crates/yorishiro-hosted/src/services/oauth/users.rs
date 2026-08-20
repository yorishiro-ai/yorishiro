//! Find-or-create for OAuth-provisioned users, plus the tenant/workspace auto-provisioning that happens the first time a given identity logs in.
//!
//! The two queries this needs (looking a user up by `(provider, subject_id)`, inserting a fresh OAuth-provisioned row) live in `models::oauth_users`, since `oauth_provider`/`oauth_subject_id` are columns this repo's own migration adds and `yorishiro-core`'s `UserRecord`/`users` module knows nothing about them.
//! This module owns the decision: which of the two branches a login takes, and how a first login wires a fresh tenant/workspace/membership together.

use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::models::schemas;
use yorishiro_core::models::tenancy::{self, MembershipRole};
use yorishiro_core::{ResultExt, YorishiroError};

use crate::models::oauth_users::{
    CreateOauthUserError, OAuthUser, create_oauth_user, find_by_oauth_identity,
};
use crate::services::plan::Plan;

/// The workspace an OAuth login should issue its API key for, alongside the tenant/membership role that key's scope is derived from: everything `POST /auth/oauth/callback` needs to call `yorishiro_core::services::auth::create_api_key` exactly the way `POST /auth/login` does.
#[derive(Debug)]
pub struct ProvisionedLogin {
    pub user_id: Uuid,
    pub email: String,
    pub workspace_id: Uuid,
    pub role: MembershipRole,
}

/// How many times `find_or_create` retries acquiring the per-identity advisory lock (see `LOCK_RETRY_DELAY`) before giving up.
/// Bounds the wait so a stuck lock holder (e.g. a request whose connection died mid-transaction, though Postgres releases the lock as soon as that connection closes) degrades to a clear error instead of an indefinite hang.
const LOCK_MAX_ATTEMPTS: u32 = 20;
/// Delay between advisory-lock acquisition attempts.
/// `LOCK_MAX_ATTEMPTS` * this is the longest a caller waits behind another first-login for the exact same identity before giving up: 2s, comfortably longer than the handful of statements first-login provisioning runs.
const LOCK_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

/// Finds the user for `(provider, subject_id)`, creating both the user and a fresh tenant/workspace/membership if this is the identity's first login.
/// `email` is required (the design's whole point is looking users up by the ID token's `email` claim); a provider that omits it fails with `ValidationFailed` rather than silently provisioning an unusable account.
///
/// New tenants/workspaces are named after the email's local part (e.g. `alice` from `alice@example.com`) since there is no signup form here to ask for a name.
/// An admin can rename either afterward through the existing dashboard/API.
/// The new member's role is always `member` per the design doc; an existing identity keeps whatever role it already holds.
///
/// Every call (including a returning user's routine login) takes a `pg_advisory_xact_lock` keyed on `(provider, subject_id)` (see `identity_lock_key`) before looking the identity up at all, and only looks it up once, under that lock.
/// This still exists purely to serialize concurrent first logins for the *same* identity, not for the transaction it happens to also provide (see the atomicity paragraph below): `tenancy::create_tenant`/`create_workspace`/ `schemas::create_schema` are `yorishiro-core`/this-crate functions whose signature can't join `lock_conn`'s transaction (the first two remain on `&PgPool` upstream; `create_schema` has its own unrelated connection lifecycle), so first-login provisioning as a whole (user row, then tenant, schema, workspace, membership) still can't be one shared transaction end to end.
/// Without the lock, two callers racing the same brand-new identity (e.g. a double-submitted callback) could interleave: the loser could look the identity up between the winner's user-row insert and its later tenant/workspace/membership creation, and fail with a confusing "not a member of any tenant" error.
/// A lookup done *before* acquiring the lock (a tempting "fast path" for the common returning-user case) reopens exactly that window, so there is deliberately no such fast path: every login pays for the lock acquisition, which is cheap and never contended across distinct identities.
///
/// The user-row insert and `add_member` both run on `lock_conn`'s transaction (see `acquire_identity_lock`), so a crash between them rolls back the insert instead of leaving an orphaned user row with no tenant membership, which every later login for that identity would otherwise resolve to a permanent `ScopeInsufficient`.
/// This mirrors how base's `yorishiro_core::models::tenancy::create_user`/`add_member` compose in one transaction (see their doc comments); `tenancy::create_tenant`/`create_workspace` remain on `&PgPool` (base hasn't changed their signature), so tenant/schema/workspace creation in between is still its own set of separate commits.
/// An orphaned *tenant* with no membership is still possible, but that's harmless (nothing ever looks a tenant up by "was it ever joined"), unlike the orphaned *user* row this closes.
/// The embedding model name and width to stamp a workspace with, resolved the same way `yorishiro_server::embedding_model_name()` and `build_embedding_provider()` resolve them.
///
/// Duplicated rather than called: this crate's lib must not depend on the server crate (see CLAUDE.md), only the binary may.
/// The duplication is the reason `the_stamp_matches_what_the_server_would_resolve` exists: the stamp is what tells a workspace whose model changed apart from one provisioned under a different one, so two paths writing different names for the same deployment would make that comparison meaningless.
pub(crate) fn embedding_stamp() -> (String, i32) {
    resolve_embedding_stamp(
        std::env::var("YORISHIRO_EMBEDDING_MODEL").ok().as_deref(),
        std::env::var("YORISHIRO_EMBEDDING_PROVIDER")
            .ok()
            .as_deref(),
        std::env::var("YORISHIRO_EMBEDDING_DIMENSIONS")
            .ok()
            .as_deref(),
    )
}

/// The resolution itself, over explicit inputs rather than the environment, so the defaults can be tested without mutating process-global state that every other test shares.
pub(crate) fn resolve_embedding_stamp(
    model: Option<&str>,
    provider: Option<&str>,
    dimensions: Option<&str>,
) -> (String, i32) {
    let model = model.map(str::to_owned).unwrap_or_else(|| match provider {
        Some("openai") => "openai".into(),
        _ => "multilingual-e5-large".into(),
    });
    let dimensions = dimensions.and_then(|v| v.parse().ok()).unwrap_or(1024);
    (model, dimensions)
}

pub async fn find_or_create(
    pool: &PgPool,
    provider: &str,
    subject_id: &str,
    email: Option<&str>,
    display_name: Option<&str>,
) -> Result<ProvisionedLogin, YorishiroError> {
    let lock_key = identity_lock_key(provider, subject_id);
    let mut lock_conn = acquire_identity_lock(pool, &lock_key).await?;

    if let Some(existing) = find_by_oauth_identity(pool, provider, subject_id).await? {
        return resolve_existing_login(pool, existing).await;
    }

    let email = email.ok_or_else(|| YorishiroError::ValidationFailed {
        message: "the identity provider did not return an email claim".into(),
        details: vec![],
        hint: "this OAuth provider/app registration must be configured to release the 'email' \
               claim"
            .into(),
    })?;

    let user =
        match create_oauth_user(&mut lock_conn, email, display_name, provider, subject_id).await {
            Ok(user) => user,
            // Some unique constraint on `identity.users` rejected the insert.
            // The lookup above (under this same lock) confirmed no row exists for this `(provider, subject_id)` yet, so this can only be a genuine email collision with an unrelated account.
            // No other caller can be concurrently inserting the same identity while this one holds the lock.
            Err(CreateOauthUserError::UniqueViolation) => {
                return Err(YorishiroError::Conflict {
                    message: format!(
                        "a user with email '{email}' already exists (sign in with password, or \
                         ask a tenant admin to link this SSO identity)"
                    ),
                });
            }
            Err(CreateOauthUserError::Other(err)) => return Err(err),
        };

    let tenant_name = tenant_name_from_email(email);
    // `create_tenant`/`create_workspace` enforce `YORISHIRO_MAX_TENANTS`/`max_workspaces` the same way every other tenant-provisioning path does (signup-via-invite, the setup wizard, Stripe checkout): auto-provisioning through SSO is not a backdoor around those caps.
    let tenant = tenancy::create_tenant(pool, &tenant_name, None).await?;

    let definition = yorishiro_core::templates::get_template("general-notes")?;

    // A tenant this path just created has no Stripe subscription yet (`plan` stays unset until one lands, see `services::plan`'s doc comment), so it's Free in every way that matters until then.
    // `default_max_entities` is Free's cap, not `None` (unlimited): auto-provisioning through SSO must not be a backdoor around the same entity cap every other Free workspace gets.
    // Stamped with the deployment's embedding model and width, as every other workspace-creating path does (base's REST controller, the setup wizard, the admin CLI).
    // A workspace provisioned through SSO stores embeddings like any other, and an unstamped one cannot later be told apart from one whose model changed underneath it.
    //
    let (embedding_model, embedding_dimensions) = embedding_stamp();

    let workspace = tenancy::create_workspace(
        pool,
        tenant.id,
        "default",
        Plan::Free.caps().default_max_entities,
        // Linked below: a schema belongs to a workspace, so the workspace has to exist first.
        None,
        Some((&embedding_model, embedding_dimensions)),
    )
    .await?;

    let mut conn = pool.acquire().await.internal()?;
    let (schema, _diff) =
        schemas::create_schema(&mut conn, tenant.id, workspace.id, definition).await?;
    drop(conn);
    tenancy::set_workspace_schema(pool, workspace.id, schema.id).await?;
    tenancy::add_member(&mut *lock_conn, tenant.id, user.id, MembershipRole::Member).await?;

    // Commits the user row and membership (see the doc comment above), and releases the advisory lock: the latter is the signal a waiting `acquire_identity_lock` retry loop is polling for, so this must not happen until provisioning has fully committed.
    lock_conn.commit().await.internal()?;

    Ok(ProvisionedLogin {
        user_id: user.id,
        email: user.email,
        workspace_id: workspace.id,
        role: MembershipRole::Member,
    })
}

/// A deterministic key for `pg_advisory_xact_lock`, one per `(provider, subject_id)` pair.
fn identity_lock_key(provider: &str, subject_id: &str) -> String {
    format!("oauth:{provider}:{subject_id}")
}

/// Acquires the advisory lock for `lock_key`, retrying with `pg_try_advisory_xact_lock` (a non-blocking attempt, unlike `pg_advisory_xact_lock`) up to `LOCK_MAX_ATTEMPTS` times rather than blocking on one held connection.
/// A caller blocked on the plain blocking variant would hold a `PgPool` connection the whole time it waits; enough concurrent first logins for the exact same identity could then exhaust the pool and starve the lock holder's own later connection needs (`create_tenant`/`create_workspace`/`schemas::create_schema` each acquire one of their own; `create_oauth_user`/`add_member` reuse this connection instead).
/// Trying, and returning the connection to the pool between attempts, keeps a same-identity pile-up from starving anyone.
/// Distinct identities hash to distinct lock keys and never contend, so this only matters under an adversarial burst against one identity, not normal traffic.
///
/// The returned connection's transaction must be committed (releasing the lock) once the caller's critical section is done: see `find_or_create`'s doc comment, since committing this is also what commits the user-row insert and its membership.
/// Dropping it without committing rolls both back and still releases the lock; it never leaves the lock held past the connection's lifetime, but a caller that reaches that path after a successful `create_oauth_user` must expect the insert to be undone, not just the lock freed.
async fn acquire_identity_lock(
    pool: &PgPool,
    lock_key: &str,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, YorishiroError> {
    for attempt in 0..LOCK_MAX_ATTEMPTS {
        let mut conn = pool.begin().await.internal()?;
        let (acquired,): (bool,) =
            sqlx::query_as("SELECT pg_try_advisory_xact_lock(hashtext($1)::bigint)")
                .bind(lock_key)
                .fetch_one(&mut *conn)
                .await
                .internal()?;
        if acquired {
            return Ok(conn);
        }
        // Rolling back (rather than holding `conn` while sleeping) returns this connection to the pool immediately, so a pile-up of waiters for one identity can't starve the identity's own lock holder (or anyone else) of a connection while it waits.
        conn.rollback().await.internal()?;
        if attempt + 1 < LOCK_MAX_ATTEMPTS {
            tokio::time::sleep(LOCK_RETRY_DELAY).await;
        }
    }
    Err(YorishiroError::Internal(anyhow::anyhow!(
        "timed out waiting for another concurrent first login for this identity to finish \
         provisioning"
    )))
}

/// Resolves a previously-provisioned OAuth identity to the workspace/role its login should use.
/// Called by `find_or_create` for its one identity lookup, whether that finds a returning user or a first-login race's winner (this call is always made under the identity's advisory lock, so a row found here is guaranteed to have finished provisioning its tenant/workspace/membership, see `find_or_create`'s doc comment).
async fn resolve_existing_login(
    pool: &PgPool,
    user: OAuthUser,
) -> Result<ProvisionedLogin, YorishiroError> {
    let mut workspaces = tenancy::list_workspaces_for_user(pool, user.id).await?;
    let workspace = workspaces
        .pop()
        .ok_or_else(|| YorishiroError::ScopeInsufficient {
            message: "this account is not a member of any tenant".into(),
            hint: "ask a tenant admin to add you as a member first".into(),
        })?;
    let role = tenancy::get_membership_role(pool, workspace.tenant_id, user.id)
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

#[cfg(test)]
#[path = "../../../tests/services/oauth/users.rs"]
mod tests;
