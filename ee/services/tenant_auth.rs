//! Resolves a key that may be bound to a tenant rather than to one workspace.
//! This uses a plain Postgres pool, because the paid edition is a PostgreSQL product: three queries elsewhere in this crate hardcode `DatabaseBackend::Postgres` and fail on any other backend.
//! That is a statement about what is supported rather than what is prevented; `HostedApp::after_context` warns at boot on a SQLite database and lets the process continue, so a deployment can reach this code on the wrong backend if an operator chooses to.
//!
//! Base binds every key to exactly one workspace, which means a client working across several has to hold one key per workspace and swap between them.
//! A key stored with a NULL `workspace_id` is instead bound to its tenant, and names the workspace per request with [`WORKSPACE_HEADER`].
//!
//! Installing this replaces base's own `default_authenticator()` in `shared_store` (`Arc<dyn Authenticator>` is keyed by `TypeId`, so the later insert wins), which means it is honoured on every authenticated path in the process, REST and MCP alike, not only the routes this crate adds.

use crate::YorishiroError;
use crate::db::DbHandle;
use crate::error::ResultExt;
use crate::models::_entities::{identity_api_keys, identity_tenants};
use crate::services::auth::{ApiKeyScope, AuthContext, Authenticator};
use async_trait::async_trait;
use sea_orm::{ActiveValue, EntityTrait, PaginatorTrait};
use uuid::Uuid;

/// The header naming which workspace a tenant-scoped API key should act on.
pub const WORKSPACE_HEADER: &str = "x-workspace-id";

pub struct TenantScopedAuthenticator;

/// The outcome of reading the workspace header.
enum RequestedWorkspace {
    Absent,
    Present(Uuid),
    /// Present but not a UUID.
    /// Distinct from `Absent` on purpose: treating an unparseable value as "not sent" would send a request meant for one workspace to whichever one the key happens to carry.
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

        let key_hash = crate::services::auth::hash_key(presented_key);

        // The two-argument overload of the `authenticate_api_key` SECURITY DEFINER function the schema migration creates.
        // `p_requested_workspace` is only consulted for a key with no workspace of its own, and resolves only when the named workspace belongs to that key's tenant: the tenant isolation boundary for these keys.
        let row: Option<(Uuid, Uuid, Uuid, String, Option<Uuid>, bool)> = sqlx::query_as(
            "SELECT id, workspace_id, tenant_id, scope, user_id, audit \
             FROM authenticate_api_key($1, $2)",
        )
        .bind(key_hash)
        .bind(requested)
        .fetch_optional(pool)
        .await
        .internal()?;

        let (api_key_id, workspace_id, tenant_id, scope_str, user_id, audit) =
            row.ok_or(YorishiroError::Unauthenticated)?;

        // A workspace-scoped key ignores the header, so a client that sends one naming a different workspace is asking for something it will not get.
        // Rejecting says so; proceeding would act on the key's own workspace instead: a write landing where the client never named, answered with a 2xx.
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
            audit,
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
/// Base's own `create_api_key` always records a workspace, so a key with none cannot be made through it: this writes the row directly.
/// The role cap is the same one that command applies: a key attributed to a user may not exceed what that user's tenant role permits, since the key can act as them.
///
/// **`conn` must be the identity pool (`DbHandle::identity`, wrapped as a `sea_orm::DatabaseConnection`), not the tenant pool.** This reads `identity_tenants` and `identity_tenant_memberships`, and neither is granted to `yorishiro_app` (the tenant pool's role): calling this against the tenant pool fails with "permission denied for table identity_tenants".
pub async fn create_tenant_api_key(
    conn: &sea_orm::DatabaseConnection,
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

    let exists = identity_tenants::Entity::find_by_id(tenant_id)
        .count(conn)
        .await
        .internal()?
        > 0;
    if !exists {
        return Err(YorishiroError::not_found(format!(
            "tenant '{tenant_id}' does not exist"
        )));
    }

    if let Some(user_id) = user_id {
        let role = crate::models::tenancy::get_membership_role(conn, tenant_id, user_id)
            .await?
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

    // Same shape as base's own keys, so nothing downstream has to tell them apart by their text.
    // The randomness is two v4 UUIDs: 122 bits each, from the same CSPRNG base's own generator draws on, and `uuid` is already a dependency here.
    let prefix = format!("ysr_{}", &Uuid::new_v4().simple().to_string()[..12]);
    let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let plaintext = format!("{prefix}_{secret}");

    let active = identity_api_keys::ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        workspace_id: ActiveValue::Set(None),
        key_hash: ActiveValue::Set(crate::services::auth::hash_key(&plaintext)),
        key_prefix: ActiveValue::Set(prefix),
        scope: ActiveValue::Set(scope.as_db_str().to_string()),
        user_id: ActiveValue::Set(user_id),
        ..Default::default()
    };
    let inserted = identity_api_keys::Entity::insert(active)
        .exec_with_returning(conn)
        .await
        .internal()?;

    Ok(CreatedTenantApiKey {
        id: inserted.id,
        plaintext,
    })
}
