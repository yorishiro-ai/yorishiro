use sea_query::{Alias, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};

use super::{
    ApiKeyScope, ApiKeys, CreatedApiKey, KEY_PREFIX_BYTES, KEY_SECRET_BYTES, hash_key, random_hex,
};

/// Issues a new API key of the form `ysr_<prefix>_<secret>`, where only the `secret` part
/// (192 bits) is the actual credential. SHA-256 is sufficient here rather than a slow KDF
/// like bcrypt/argon2, since API keys already carry enough entropy that offline
/// brute-forcing isn't a realistic threat.
///
/// `workspace_id` of `None` issues a **tenant-scoped** key: it can act on any workspace in
/// `tenant_id`, chosen per request with the `X-Workspace-Id` header. A key bound to one
/// workspace stays the default -- a client that only ever works in one workspace should not
/// have to name it on every call, and a leaked key should reach as little as possible.
pub async fn create_api_key(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    workspace_id: Option<Uuid>,
    scope: ApiKeyScope,
    user_id: Option<Uuid>,
) -> Result<CreatedApiKey, YorishiroError> {
    let prefix = format!("ysr_{}", random_hex(KEY_PREFIX_BYTES));
    let secret = random_hex(KEY_SECRET_BYTES);
    let plaintext = format!("{prefix}_{secret}");
    let key_hash = hash_key(&plaintext);

    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), ApiKeys::Table))
        .columns([
            ApiKeys::TenantId,
            ApiKeys::WorkspaceId,
            ApiKeys::KeyHash,
            ApiKeys::KeyPrefix,
            ApiKeys::Scope,
            ApiKeys::UserId,
        ])
        .values_panic([
            tenant_id.into(),
            workspace_id.into(),
            key_hash.into(),
            prefix.into(),
            scope.as_db_str().into(),
            user_id.into(),
        ])
        .returning(Query::returning().columns([ApiKeys::Id]))
        .build_sqlx(PostgresQueryBuilder);

    let (id,): (Uuid,) = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .internal()?;

    Ok(CreatedApiKey {
        id,
        tenant_id,
        workspace_id,
        scope,
        user_id,
        plaintext,
    })
}

#[cfg(test)]
#[path = "../../../tests/services/auth/keys.rs"]
mod tests;
