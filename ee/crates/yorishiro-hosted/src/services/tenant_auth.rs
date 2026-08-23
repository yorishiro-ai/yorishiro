//! Resolves a key that may be bound to a tenant rather than to one workspace.
//! Ported from master's `ee/crates/yorishiro-hosted/src/services/tenant_auth.rs`, simplified for
//! this branch's `DbHandle`: master's has a `Postgres`/`Sqlite` split (the community edition runs
//! on both), this one is a plain Postgres struct, since `ee/` never runs on Sqlite (see
//! `services::authz`'s doc comment for the same reasoning).
//!
//! Base binds every key to exactly one workspace, which means a client working across several
//! has to hold one key per workspace and swap between them. A key stored with a NULL
//! `workspace_id` is instead bound to its tenant, and names the workspace per request with
//! [`WORKSPACE_HEADER`].
//!
//! Installing this replaces base's own `default_authenticator()` in `shared_store`
//! (`Arc<dyn Authenticator>` is keyed by `TypeId`, so the later insert wins), which means it is
//! honoured on every authenticated path in the process, REST and MCP alike, not only the routes
//! this crate adds.

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::YorishiroError;
use yorishiro_core::db::DbHandle;
use yorishiro_core::error::ResultExt;
use yorishiro_core::services::auth::{ApiKeyScope, AuthContext, Authenticator};

/// The header naming which workspace a tenant-scoped API key should act on.
pub const WORKSPACE_HEADER: &str = "x-workspace-id";

pub struct TenantScopedAuthenticator;

/// The outcome of reading the workspace header.
enum RequestedWorkspace {
    Absent,
    Present(Uuid),
    /// Present but not a UUID. Distinct from `Absent` on purpose: treating an unparseable value
    /// as "not sent" would send a request meant for one workspace to whichever one the key
    /// happens to carry.
    Malformed,
}

fn requested_workspace(headers: &[(String, String)]) -> RequestedWorkspace {
    match headers
        .iter()
        .find(|(name, _)| name == WORKSPACE_HEADER)
        .map(|(_, value)| value.trim())
    {
        None => RequestedWorkspace::Absent,
        Some(value) => match Uuid::parse_str(value) {
            Ok(id) => RequestedWorkspace::Present(id),
            Err(_) => RequestedWorkspace::Malformed,
        },
    }
}

#[async_trait]
impl Authenticator for TenantScopedAuthenticator {
    async fn authenticate(
        &self,
        db: &DbHandle,
        presented_key: &str,
        headers: &[(String, String)],
    ) -> Result<AuthContext, YorishiroError> {
        let pool = db.tenant.pool();
        let requested = match requested_workspace(headers) {
            RequestedWorkspace::Absent => None,
            RequestedWorkspace::Present(id) => Some(id),
            RequestedWorkspace::Malformed => {
                return Err(YorishiroError::ValidationFailed {
                    message: format!("{WORKSPACE_HEADER} is not a valid UUID"),
                    details: Vec::new(),
                    hint: "send the workspace's UUID, or omit the header to use a \
                           workspace-scoped key"
                        .into(),
                });
            }
        };

        let key_hash = yorishiro_core::services::auth::hash_key(presented_key);

        // The two-argument overload the identity migration adds (m20260822_101200).
        // `p_requested_workspace` is only consulted for a key with no workspace of its own, and
        // resolves only when the named workspace belongs to that key's tenant: the tenant
        // isolation boundary for these keys.
        let row: Option<(Uuid, Uuid, Uuid, String, Option<Uuid>)> = sqlx::query_as(
            "SELECT id, workspace_id, tenant_id, scope, user_id \
             FROM authenticate_api_key($1, $2)",
        )
        .bind(key_hash)
        .bind(requested)
        .fetch_optional(pool)
        .await
        .internal()?;

        let (api_key_id, workspace_id, tenant_id, scope_str, user_id) =
            row.ok_or(YorishiroError::Unauthenticated)?;

        // A workspace-scoped key ignores the header, so a client that sends one naming a
        // different workspace is asking for something it will not get. Rejecting says so;
        // proceeding would act on the key's own workspace instead: a write landing where the
        // client never named, answered with a 2xx.
        if let Some(requested) = requested
            && requested != workspace_id
        {
            return Err(YorishiroError::ValidationFailed {
                message: format!("{WORKSPACE_HEADER} names a workspace this key cannot act on"),
                details: Vec::new(),
                hint: "this key is scoped to a single workspace; omit the header, or use a \
                       tenant-scoped key to choose a workspace per request"
                    .into(),
            });
        }

        let scope = ApiKeyScope::from_db_str(&scope_str).ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!(
                "unknown api key scope in database: {scope_str}"
            ))
        })?;

        Ok(AuthContext {
            api_key_id,
            workspace_id,
            tenant_id,
            scope,
            user_id,
        })
    }
}

/// A freshly issued tenant-scoped key.
/// The plaintext exists only here: only its hash is stored.
pub struct CreatedTenantApiKey {
    pub id: Uuid,
    pub plaintext: String,
}

/// Issues a tenant-scoped key.
///
/// Base's own `create_api_key` always records a workspace, so a key with none cannot be made
/// through it: this writes the row directly. The role cap is the same one that command applies:
/// a key attributed to a user may not exceed what that user's tenant role permits, since the key
/// can act as them.
///
/// **`pool` must be the identity pool (`DbHandle::identity`), not the tenant pool.** This reads
/// `identity_tenants` and `identity_tenant_memberships`, and neither is granted to
/// `yorishiro_app` (the tenant pool's role): calling this with `DbHandle::tenant.pool()` fails
/// with "permission denied for table identity_tenants".
pub async fn create_tenant_api_key(
    pool: &PgPool,
    tenant_id: Uuid,
    scope: &str,
    user_id: Option<Uuid>,
) -> Result<CreatedTenantApiKey, YorishiroError> {
    let scope =
        ApiKeyScope::from_db_str(scope).ok_or_else(|| YorishiroError::ValidationFailed {
            message: format!("unknown scope '{scope}'"),
            details: Vec::new(),
            hint: "use one of: read, write, schema".into(),
        })?;

    let exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM identity_tenants WHERE id = $1")
        .bind(tenant_id)
        .fetch_optional(pool)
        .await
        .internal()?;
    if exists.is_none() {
        return Err(YorishiroError::not_found(format!(
            "tenant '{tenant_id}' does not exist"
        )));
    }

    if let Some(user_id) = user_id {
        // `tenancy::get_membership_role` takes `&impl sea_orm::ConnectionTrait`, not a raw sqlx
        // pool: this function operates on the raw tenant pool (matching
        // `TenantScopedAuthenticator`, since it issues a NULL-workspace key directly), so the
        // role is read the same raw-SQL way as everything else here rather than pulling in a
        // SeaORM connection just for this one lookup.
        let role_str: Option<(String,)> = sqlx::query_as(
            "SELECT role FROM identity_tenant_memberships WHERE tenant_id = $1 AND user_id = $2",
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .internal()?;
        let role = role_str
            .and_then(|(s,)| yorishiro_core::models::tenancy::MembershipRole::from_db_str(&s))
            .ok_or_else(|| {
                YorishiroError::not_found(format!(
                    "user '{user_id}' is not a member of tenant '{tenant_id}'"
                ))
            })?;
        if scope > role.max_scope() {
            return Err(YorishiroError::ScopeInsufficient {
                message: format!(
                    "this user's tenant role permits at most {:?} scope keys",
                    role.max_scope()
                ),
                hint: "issue a lower-scoped key, or raise the user's tenant role".into(),
            });
        }
    }

    // Same shape as base's own keys, so nothing downstream has to tell them apart by their
    // text. The randomness is two v4 UUIDs: 122 bits each, from the same CSPRNG base's own
    // generator draws on, and `uuid` is already a dependency here.
    let prefix = format!("ysr_{}", &Uuid::new_v4().simple().to_string()[..12]);
    let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let plaintext = format!("{prefix}_{secret}");

    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO identity_api_keys (tenant_id, workspace_id, key_hash, key_prefix, scope, user_id) \
         VALUES ($1, NULL, $2, $3, $4, $5) RETURNING id",
    )
    .bind(tenant_id)
    .bind(yorishiro_core::services::auth::hash_key(&plaintext))
    .bind(&prefix)
    .bind(scope.as_db_str())
    .bind(user_id)
    .fetch_one(pool)
    .await
    .internal()?;

    Ok(CreatedTenantApiKey {
        id: row.0,
        plaintext,
    })
}
