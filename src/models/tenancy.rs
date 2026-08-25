//! Control-plane CRUD for users, invites, and tenant memberships: signup, login, and the `admin create-invite` chain.
//!
//! Everything here runs on `ctx.db` (Loco's own migration-role connection), never the RLS-scoped tenant pool: no workspace exists yet for RLS to scope by, the same reasoning `TenantDb::connect`'s doc comment gives for the identity pool.

use chrono::{DateTime, Duration, Utc};
use loco_rs::hash;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, SqlErr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{
    identity_invites, identity_tenant_memberships, identity_tenants, identity_users,
    identity_workspaces,
};
use crate::services::auth::{ApiKeyScope, hash_key, random_hex};

/// The nil UUID, reserved for infrastructure tenants that own no members and no data of their own (currently `ee/`'s official-templates publisher).
/// Excluded from every count this module takes against `YORISHIRO_MAX_TENANTS`.
pub const INFRASTRUCTURE_TENANT_ID: Uuid = Uuid::nil();

/// Counts real (non-infrastructure) tenants: every row except `INFRASTRUCTURE_TENANT_ID`.
pub async fn count_tenants(conn: &impl ConnectionTrait) -> Result<u64, YorishiroError> {
    identity_tenants::Entity::find()
        .filter(identity_tenants::Column::Id.ne(INFRASTRUCTURE_TENANT_ID))
        .count(conn)
        .await
        .internal()
}

/// Creates a tenant, enforcing a tenant cap against `count_tenants`.
/// On Postgres `conn` must be a transaction: this takes `db::lock_for_update` before counting, to close the TOCTOU gap a bare count-then-insert would leave.
/// On SQLite the lock is a no-op (see `db::lock_for_update`'s doc comment for why that is still race-safe) and the cap is not `YORISHIRO_MAX_TENANTS` but a hardcoded 1.
pub async fn create_tenant(
    conn: &impl ConnectionTrait,
    name: &str,
) -> Result<identity_tenants::Model, YorishiroError> {
    // SQLite has no database-enforced tenant isolation (no RLS, no roles), so a second tenant on that backend would be a silent isolation break rather than merely an unwanted one.
    // The cap is hardcoded rather than read from YORISHIRO_MAX_TENANTS: an operator raising that variable must not be able to loosen a constraint that exists because the isolation mechanism itself is absent, not because of a configurable policy choice.
    let effective_max = if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
        Some(1)
    } else {
        max_tenants_from_env()?
    };

    if let Some(max) = effective_max {
        crate::db::lock_for_update(conn, "create_tenant")
            .await
            .internal()?;
        let count = count_tenants(conn).await?;
        if count >= max as u64 {
            let remedy = if conn.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
                "SQLite deployments are limited to a single tenant, since this backend has no \
                 database-enforced isolation between tenants; use PostgreSQL for more than one"
                    .to_string()
            } else {
                "raise YORISHIRO_MAX_TENANTS or delete an existing tenant".to_string()
            };
            return Err(YorishiroError::Conflict {
                message: format!("this deployment has reached its tenant limit ({max}); {remedy}"),
            });
        }
    }

    let active = identity_tenants::ActiveModel {
        name: ActiveValue::Set(name.to_string()),
        max_workspaces: ActiveValue::Set(None),
        ..Default::default()
    };
    // `id` on SQLite is filled in by identity_tenants::ActiveModel's before_save (crate::db::sqlite_generated_id), not here.
    active.insert(conn).await.internal()
}

/// Reads and parses `YORISHIRO_MAX_TENANTS`.
/// Unset or `0` means unlimited; a negative or non-integer value is a misconfiguration and fails loudly rather than silently falling back to unlimited.
pub fn max_tenants_from_env() -> Result<Option<i32>, YorishiroError> {
    match std::env::var("YORISHIRO_MAX_TENANTS") {
        Ok(raw) => {
            let parsed = raw.parse::<i32>().map_err(|_| {
                YorishiroError::Internal(anyhow::anyhow!(
                    "YORISHIRO_MAX_TENANTS must be an integer, got '{raw}'"
                ))
            })?;
            match parsed {
                0 => Ok(None),
                n if n < 0 => Err(YorishiroError::Internal(anyhow::anyhow!(
                    "YORISHIRO_MAX_TENANTS must not be negative, got '{raw}'"
                ))),
                n => Ok(Some(n)),
            }
        }
        Err(_) => Ok(None),
    }
}

/// Mirrors the `identity_tenant_memberships.role` check constraint (`owner`/`admin`/`member`/`viewer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MembershipRole {
    Owner,
    Admin,
    Member,
    Viewer,
}

impl MembershipRole {
    pub fn as_db_str(self) -> &'static str {
        match self {
            MembershipRole::Owner => "owner",
            MembershipRole::Admin => "admin",
            MembershipRole::Member => "member",
            MembershipRole::Viewer => "viewer",
        }
    }

    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(MembershipRole::Owner),
            "admin" => Some(MembershipRole::Admin),
            "member" => Some(MembershipRole::Member),
            "viewer" => Some(MembershipRole::Viewer),
            _ => None,
        }
    }

    /// The highest API key scope a member with this role may be issued.
    pub fn max_scope(self) -> ApiKeyScope {
        match self {
            MembershipRole::Owner | MembershipRole::Admin => ApiKeyScope::Migration,
            MembershipRole::Member => ApiKeyScope::Write,
            MembershipRole::Viewer => ApiKeyScope::Read,
        }
    }

    /// Whether this role may manage the tenant itself: members, workspaces, and everything `require_tenant_admin` gates.
    pub fn administers_tenant(self) -> bool {
        matches!(self, MembershipRole::Owner | MembershipRole::Admin)
    }
}

/// Creates a human user account.
/// The password is hashed with `loco_rs::hash` (Argon2id) before ever reaching the database.
///
/// Takes `&impl ConnectionTrait` rather than a pool handle so a caller can compose this with `add_member` in one transaction: the two must succeed or fail together, or a failure between them leaves an orphaned user row that can never join a tenant (see `signup`, which wraps both in one transaction).
pub async fn create_user(
    conn: &impl ConnectionTrait,
    email: &str,
    password: &str,
    display_name: Option<&str>,
) -> Result<identity_users::Model, YorishiroError> {
    let password_hash =
        hash::hash_password(password).map_err(|err| YorishiroError::Internal(err.into()))?;

    let active = identity_users::ActiveModel {
        email: ActiveValue::Set(email.to_string()),
        password_hash: ActiveValue::Set(Some(password_hash)),
        display_name: ActiveValue::Set(display_name.map(str::to_string)),
        ..Default::default()
    };

    active.insert(conn).await.map_err(|err| {
        if matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
            YorishiroError::Conflict {
                message: format!("a user with email '{email}' already exists"),
            }
        } else {
            YorishiroError::Internal(err.into())
        }
    })
}

/// Verifies an email/password pair against the stored Argon2id hash, returning the matching user on success.
/// An OAuth-only account (`password_hash = NULL`) never matches, same as a wrong password: `loco_rs::hash::verify_password` needs a hash to compare against.
pub async fn verify_login(
    conn: &impl ConnectionTrait,
    email: &str,
    password: &str,
) -> Result<Option<identity_users::Model>, YorishiroError> {
    let user = identity_users::Entity::find()
        .filter(identity_users::Column::Email.eq(email))
        .one(conn)
        .await
        .internal()?;

    let Some(user) = user else {
        return Ok(None);
    };

    let matches = user
        .password_hash
        .as_deref()
        .is_some_and(|hash| hash::verify_password(password, hash));

    Ok(matches.then_some(user))
}

/// Adds (or updates the role of) a user's membership in a tenant.
///
/// Takes `&impl ConnectionTrait` so a caller can compose this with `create_user` in one transaction, same reasoning as `create_user`'s doc comment.
pub async fn add_member(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<(), YorishiroError> {
    use sea_orm::sea_query::OnConflict;

    // `Entity::insert(...).on_conflict(...).exec(...)` builds its query eagerly from `active` and never calls `ActiveModelBehavior::before_save`, unlike plain `ActiveModel::insert()`: this is the one insert path in this file that needs `sqlite_generated_id` called directly rather than relying on the hook.
    let active = identity_tenant_memberships::ActiveModel {
        id: crate::db::sqlite_generated_id(conn, ActiveValue::NotSet),
        tenant_id: ActiveValue::Set(tenant_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(role.as_db_str().to_string()),
        ..Default::default()
    };

    identity_tenant_memberships::Entity::insert(active)
        .on_conflict(
            OnConflict::columns([
                identity_tenant_memberships::Column::TenantId,
                identity_tenant_memberships::Column::UserId,
            ])
            .update_column(identity_tenant_memberships::Column::Role)
            .to_owned(),
        )
        .exec(conn)
        .await
        .internal()?;

    Ok(())
}

/// Looks up a user by email, for `POST /api/members` (which attaches an *existing* account by email, never creates one).
pub async fn get_user_by_email(
    conn: &impl ConnectionTrait,
    email: &str,
) -> Result<Option<identity_users::Model>, YorishiroError> {
    identity_users::Entity::find()
        .filter(identity_users::Column::Email.eq(email))
        .one(conn)
        .await
        .internal()
}

/// Every member of a tenant, joined against their user row.
pub async fn list_members(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    page: super::pagination::ListParams,
) -> Result<Vec<MembershipRecord>, YorishiroError> {
    let memberships = identity_tenant_memberships::Entity::find()
        .filter(identity_tenant_memberships::Column::TenantId.eq(tenant_id))
        .find_also_related(identity_users::Entity)
        .order_by_asc(identity_tenant_memberships::Column::CreatedAt)
        .limit(page.limit() as u64)
        .offset(page.offset() as u64)
        .all(conn)
        .await
        .internal()?;

    Ok(memberships
        .into_iter()
        .filter_map(|(membership, user)| {
            let user = user?;
            let role = MembershipRole::from_db_str(&membership.role)?;
            Some(MembershipRecord {
                user_id: user.id,
                email: user.email,
                display_name: user.display_name,
                role,
            })
        })
        .collect())
}

/// Looks up a single user's role within a tenant, or `None` if they aren't a member.
pub async fn get_membership_role(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Option<MembershipRole>, YorishiroError> {
    let membership = identity_tenant_memberships::Entity::find()
        .filter(identity_tenant_memberships::Column::TenantId.eq(tenant_id))
        .filter(identity_tenant_memberships::Column::UserId.eq(user_id))
        .one(conn)
        .await
        .internal()?;

    Ok(membership.and_then(|m| MembershipRole::from_db_str(&m.role)))
}

const INVITE_TOKEN_BYTES: usize = 24;

/// Creates an invite token for `email` to join `tenant_id` with `role`.
/// Returns the record alongside the plaintext token: like API keys, only its SHA-256 hash is persisted, so this is the only place the plaintext is ever available.
/// Callers must surface it themselves (printed by the admin CLI today; a transactional-email integration is not provided).
pub async fn create_invite(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    email: &str,
    role: MembershipRole,
    ttl: Duration,
) -> Result<(identity_invites::Model, String), YorishiroError> {
    let token = random_hex(INVITE_TOKEN_BYTES);
    let token_hash = hash_key(&token);
    let expires_at = Utc::now() + ttl;

    let active = identity_invites::ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        email: ActiveValue::Set(email.to_string()),
        role: ActiveValue::Set(role.as_db_str().to_string()),
        token_hash: ActiveValue::Set(token_hash),
        expires_at: ActiveValue::Set(expires_at.into()),
        ..Default::default()
    };

    let invite = active.insert(conn).await.internal()?;
    Ok((invite, token))
}

/// A tenant member as reported to a caller: `GET /api/members`, and the response body of adding one via `POST /api/members`.
#[derive(Debug, Serialize)]
pub struct MembershipRecord {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub role: MembershipRole,
}

/// What a redeemed invite grants: resolved once, since the invite row is consumed by the same call that reads it.
pub struct RedeemedInvite {
    pub tenant_id: Uuid,
    pub email: String,
    pub role: MembershipRole,
}

/// Redeems an invite token: atomically marks it used and returns the tenant/email/role it grants, or `None` if the token doesn't match any invite, is already used, or has expired.
///
/// The lookup and the `used_at` update happen in a single statement (`UpdateMany` with all three conditions in its `WHERE`), so two concurrent redemptions of the same token can't both succeed: whichever commits first's `used_at IS NULL` no longer holds for the loser.
pub async fn redeem_invite(
    conn: &impl ConnectionTrait,
    raw_token: &str,
) -> Result<Option<RedeemedInvite>, YorishiroError> {
    let token_hash = hash_key(raw_token);
    let now = Utc::now();

    // Read first to build the response: the update itself does not return rows affected as model data, and a second SELECT after the UPDATE could observe a different row (e.g. one this same call just marked used) if invites were ever deletable, which they are not, so this is safe, not merely convenient.
    let invite = identity_invites::Entity::find()
        .filter(identity_invites::Column::TokenHash.eq(token_hash.clone()))
        .filter(identity_invites::Column::UsedAt.is_null())
        .filter(identity_invites::Column::ExpiresAt.gt(now))
        .one(conn)
        .await
        .internal()?;

    let Some(invite) = invite else {
        return Ok(None);
    };

    let update_result = identity_invites::Entity::update_many()
        .col_expr(
            identity_invites::Column::UsedAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .filter(identity_invites::Column::Id.eq(invite.id))
        .filter(identity_invites::Column::UsedAt.is_null())
        .filter(identity_invites::Column::ExpiresAt.gt(now))
        .exec(conn)
        .await
        .internal()?;

    if update_result.rows_affected == 0 {
        // Lost the race: another concurrent redemption already claimed this token between the read above and this UPDATE.
        return Ok(None);
    }

    let role = MembershipRole::from_db_str(&invite.role).ok_or_else(|| {
        YorishiroError::Internal(anyhow::anyhow!(
            "unknown membership role in database: {}",
            invite.role
        ))
    })?;

    Ok(Some(RedeemedInvite {
        tenant_id: invite.tenant_id,
        email: invite.email,
        role,
    }))
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceSummary {
    pub id: Uuid,
    pub name: String,
}

/// Every workspace under `tenant_id`, for the signup response (which workspaces the new member can now log into).
pub async fn list_workspaces(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    page: super::pagination::ListParams,
) -> Result<Vec<WorkspaceSummary>, YorishiroError> {
    use crate::models::_entities::identity_workspaces;

    let workspaces = identity_workspaces::Entity::find()
        .filter(identity_workspaces::Column::TenantId.eq(tenant_id))
        .order_by_asc(identity_workspaces::Column::CreatedAt)
        .limit(page.limit() as u64)
        .offset(page.offset() as u64)
        .all(conn)
        .await
        .internal()?;

    Ok(workspaces
        .into_iter()
        .map(|w| WorkspaceSummary {
            id: w.id,
            name: w.name,
        })
        .collect())
}

/// Every workspace `user_id` can log into: the union of workspaces under every tenant they hold a membership in.
/// Used by `/auth/login` to resolve `workspace_id` automatically when the caller can only reach one.
///
/// Deliberately unpaginated: this drives sign-in, not a browsing UI, and its own caller
/// (`resolve_login_workspace`) needs the true, complete set to tell "exactly one, resolve to it"
/// from "more than one, the caller must say which." A default `LIMIT` here would silently hide a
/// membership from a user who holds more workspaces than the page size, rather than list them.
pub async fn list_workspaces_for_user(
    conn: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<Vec<WorkspaceSummary>, YorishiroError> {
    use crate::models::_entities::identity_workspaces;

    let memberships = identity_tenant_memberships::Entity::find()
        .filter(identity_tenant_memberships::Column::UserId.eq(user_id))
        .all(conn)
        .await
        .internal()?;

    let tenant_ids: Vec<Uuid> = memberships.into_iter().map(|m| m.tenant_id).collect();
    if tenant_ids.is_empty() {
        return Ok(vec![]);
    }

    let workspaces = identity_workspaces::Entity::find()
        .filter(identity_workspaces::Column::TenantId.is_in(tenant_ids))
        .all(conn)
        .await
        .internal()?;

    Ok(workspaces
        .into_iter()
        .map(|w| WorkspaceSummary {
            id: w.id,
            name: w.name,
        })
        .collect())
}

/// A workspace's id and owning tenant, for `/auth/login`'s explicit `workspace_id` path.
pub async fn get_workspace_tenant(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Uuid, YorishiroError> {
    use crate::models::_entities::identity_workspaces;

    identity_workspaces::Entity::find_by_id(workspace_id)
        .one(conn)
        .await
        .internal()?
        .map(|w| w.tenant_id)
        .ok_or_else(|| YorishiroError::not_found("workspace not found"))
}

/// Creates a workspace under `tenant_id`, enforcing the tenant's `max_workspaces` cap.
/// `None` means unlimited, which is the default so self-hosted deployments are never capped unless an operator explicitly sets a limit.
///
/// `embedding` is the deployment's model and dimension count, stamped onto the workspace so a later write produced by a different model can be refused where it happens rather than at query time.
/// `None` leaves the workspace on "whatever the deployment is configured for".
pub async fn create_workspace(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    name: &str,
    max_entities: Option<i32>,
    schema_id: Option<Uuid>,
    embedding: Option<(&str, i32)>,
) -> Result<identity_workspaces::Model, YorishiroError> {
    use crate::models::_entities::identity_tenants;
    use crate::models::identity_workspaces::{
        WORKSPACE_STATUS_ACTIVE, WORKSPACE_STATUS_SCHEMA_PENDING,
    };

    let tenant = identity_tenants::Entity::find_by_id(tenant_id)
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("tenant '{tenant_id}' was not found")))?;

    if let Some(max) = tenant.max_workspaces {
        let count = identity_workspaces::Entity::find()
            .filter(identity_workspaces::Column::TenantId.eq(tenant_id))
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

    let active = identity_workspaces::ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        name: ActiveValue::Set(name.to_string()),
        max_entities: ActiveValue::Set(max_entities),
        schema_id: ActiveValue::Set(schema_id),
        status: ActiveValue::Set(
            if schema_id.is_some() {
                WORKSPACE_STATUS_ACTIVE
            } else {
                WORKSPACE_STATUS_SCHEMA_PENDING
            }
            .to_string(),
        ),
        embedding_model: ActiveValue::Set(embedding.map(|(model, _)| model.to_string())),
        embedding_dimensions: ActiveValue::Set(embedding.map(|(_, dimensions)| dimensions)),
        ..Default::default()
    };

    active.insert(conn).await.internal()
}

/// Sets a tenant's `max_workspaces` cap.
///
/// A self-hosted deployment never calls this (its tenants keep whatever cap they were created with, `None` by default): the only caller is `ee/`'s Stripe integration, applying the cap that comes with a plan change.
pub async fn set_tenant_max_workspaces(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    max_workspaces: Option<i32>,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::identity_tenants;

    let tenant = identity_tenants::Entity::find_by_id(tenant_id)
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| YorishiroError::not_found(format!("tenant '{tenant_id}' was not found")))?;

    let mut active: identity_tenants::ActiveModel = tenant.into();
    active.max_workspaces = ActiveValue::Set(max_workspaces);
    active.update(conn).await.internal()?;
    Ok(())
}

/// Fetches a workspace by id.
pub async fn get_workspace(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<identity_workspaces::Model, YorishiroError> {
    use crate::models::_entities::identity_workspaces;

    identity_workspaces::Entity::find_by_id(workspace_id)
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| {
            YorishiroError::not_found(format!("workspace '{workspace_id}' was not found"))
        })
}

/// Deletes a workspace, refusing to remove a tenant's last one.
///
/// `db::lock_for_update` serializes concurrent deletes against the same tenant before counting its workspaces, so two requests racing to delete the tenant's last two workspaces cannot both see a spare one and proceed: a plain `DELETE ... WHERE EXISTS (another workspace)` reads a snapshot each transaction takes independently, which is exactly the race this avoids.
pub async fn delete_workspace(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<(), YorishiroError> {
    use crate::models::_entities::identity_workspaces;

    let workspace = identity_workspaces::Entity::find_by_id(workspace_id)
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| {
            YorishiroError::not_found(format!("workspace '{workspace_id}' was not found"))
        })?;

    crate::db::lock_for_update(conn, &format!("workspace-delete:{}", workspace.tenant_id))
        .await
        .internal()?;

    let remaining = identity_workspaces::Entity::find()
        .filter(identity_workspaces::Column::TenantId.eq(workspace.tenant_id))
        .count(conn)
        .await
        .internal()?;
    if remaining <= 1 {
        return Err(YorishiroError::Conflict {
            message: "cannot delete a tenant's only remaining workspace".into(),
        });
    }

    identity_workspaces::Entity::delete_by_id(workspace_id)
        .exec(conn)
        .await
        .internal()?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct UserRecord {
    pub id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<identity_users::Model> for UserRecord {
    fn from(m: identity_users::Model) -> Self {
        Self {
            id: m.id,
            email: m.email,
            display_name: m.display_name,
            created_at: m.created_at.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use migration::{Migrator, MigratorTrait};
    use sea_orm::Database;
    use serial_test::serial;

    use super::create_tenant;

    /// A fresh in-memory SQLite database, migrated. Each test gets its own, so nothing but the process-wide `YORISHIRO_MAX_TENANTS` env var is shared between them.
    async fn sqlite_db() -> sea_orm::DatabaseConnection {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("connect to in-memory sqlite");
        Migrator::up(&db, None).await.expect("run migrations");
        db
    }

    /// Sets `YORISHIRO_MAX_TENANTS` for the duration of `fut`, restoring whatever was there before.
    /// Callers must be `#[serial]`: this mutates process-wide state, and `create_tenant`'s own SQLite path ignores it regardless, but the Postgres branch inside `create_tenant` still reads it, so a concurrent test observing an unexpected value would be a real (if here unlikely) source of flakiness.
    async fn with_max_tenants<T>(value: &str, fut: impl std::future::Future<Output = T>) -> T {
        let previous = std::env::var("YORISHIRO_MAX_TENANTS").ok();
        // SAFETY: serialized by every test that touches this env var being #[serial] on the default key.
        unsafe {
            std::env::set_var("YORISHIRO_MAX_TENANTS", value);
        }
        let result = fut.await;
        unsafe {
            match &previous {
                Some(v) => std::env::set_var("YORISHIRO_MAX_TENANTS", v),
                None => std::env::remove_var("YORISHIRO_MAX_TENANTS"),
            }
        }
        result
    }

    #[tokio::test]
    #[serial]
    async fn a_first_tenant_can_be_created_on_sqlite() {
        let db = sqlite_db().await;
        let tenant = create_tenant(&db, "first tenant")
            .await
            .expect("first tenant should be created");
        assert_eq!(tenant.name, "first tenant");
    }

    #[tokio::test]
    #[serial]
    async fn a_second_tenant_is_refused_on_sqlite_even_with_a_large_max_tenants() {
        let db = sqlite_db().await;
        // A generous limit: if SQLite's cap were reading this instead of being hardcoded to 1, the second create below would wrongly succeed.
        with_max_tenants("1000", async {
            create_tenant(&db, "first tenant")
                .await
                .expect("first tenant should be created");

            let err = create_tenant(&db, "second tenant")
                .await
                .expect_err("a second tenant must be refused on sqlite");
            assert!(
                matches!(err, crate::error::YorishiroError::Conflict { .. }),
                "expected Conflict, got {err:?}"
            );
        })
        .await;
    }
}
