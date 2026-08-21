use sea_query::{IntoIden, Query};
use sea_query_binder::SqlxBinder;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};

use super::{
    ApiKeyScope, ApiKeys, CreatedApiKey, KEY_PREFIX_BYTES, KEY_SECRET_BYTES, hash_key, random_hex,
};

/// Issues a new API key of the form `ysr_<prefix>_<secret>`, where only the `secret` part (192 bits) is the actual credential.
/// SHA-256 is sufficient here rather than a slow KDF like bcrypt/argon2, since API keys already carry enough entropy that offline brute-forcing isn't a realistic threat.
pub async fn create_api_key<C>(
    conn: &mut C,
    workspace_id: Uuid,
    scope: ApiKeyScope,
    user_id: Option<Uuid>,
) -> Result<CreatedApiKey, YorishiroError>
where
    C: crate::db::Engine,
    for<'e> &'e mut C: sqlx::Executor<'e, Database = C::Db>,
    for<'q> sea_query_binder::SqlxValues: sqlx::IntoArguments<'q, C::Db>,
    (Uuid,): for<'r> sqlx::FromRow<'r, <C::Db as sqlx::Database>::Row>,
{
    let prefix = format!("ysr_{}", random_hex(KEY_PREFIX_BYTES));
    let secret = random_hex(KEY_SECRET_BYTES);
    let plaintext = format!("{prefix}_{secret}");
    let key_hash = hash_key(&plaintext);

    let (cols, vals) = crate::db::with_generated_id::<C, _>(
        ApiKeys::Id,
        vec![
            ApiKeys::WorkspaceId.into_iden(),
            ApiKeys::KeyHash.into_iden(),
            ApiKeys::KeyPrefix.into_iden(),
            ApiKeys::Scope.into_iden(),
            ApiKeys::UserId.into_iden(),
        ],
        vec![
            workspace_id.into(),
            key_hash.into(),
            prefix.into(),
            scope.as_db_str().into(),
            user_id.into(),
        ],
    );
    let (sql, values) = Query::insert()
        .into_table(C::schema_table("identity", ApiKeys::Table))
        .columns(cols)
        .values_panic(vals)
        .returning(Query::returning().columns([ApiKeys::Id]))
        .build_sqlx(C::builder());

    let (id,): (Uuid,) = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .internal()?;

    Ok(CreatedApiKey {
        id,
        workspace_id,
        scope,
        user_id,
        plaintext,
    })
}

#[cfg(test)]
#[path = "../../../tests/services/auth/keys.rs"]
mod tests;
