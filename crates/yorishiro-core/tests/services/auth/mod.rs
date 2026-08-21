use sea_query::{Alias, Expr, Iden, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

use crate::YorishiroError;
use crate::db::{DbHandle, TenantDb};
use crate::services::auth::{
    ApiKeyScope, ApiKeys, DefaultAuthenticator, authenticate, authorize, bearer_credential,
    create_api_key, hex_decode, hex_encode, require_scope,
};
use crate::test_support;

#[derive(Iden)]
enum Users {
    Table,
    Id,
    Email,
    PasswordHash,
}

/// Seeds a tenant plus one workspace under it, returning `(tenant_id, workspace_id)`.
async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
    test_support::seed_tenant_and_workspace(pool).await
}

#[sqlx::test(migrations = "../../migrations")]
async fn authenticates_a_freshly_created_key(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let tenant_db = TenantDb::new(pool.clone());
    let mut conn = tenant_db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let created = create_api_key(&mut *conn, workspace_id, ApiKeyScope::Write, None)
        .await
        .unwrap();

    let ctx = authenticate(&test_support::db_handle(&pool), &created.plaintext)
        .await
        .unwrap();

    assert_eq!(ctx.tenant_id, tenant_id);
    assert_eq!(ctx.workspace_id, workspace_id);
    assert_eq!(ctx.api_key_id, created.id);
    assert_eq!(ctx.scope, ApiKeyScope::Write);
    assert_eq!(ctx.user_id, None);
}

#[sqlx::test(migrations = "../../migrations")]
async fn resolves_the_attributed_user(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), Users::Table))
        .columns([Users::Email, Users::PasswordHash])
        .values_panic(["attributed@example.com".into(), "hash".into()])
        .returning(Query::returning().columns([Users::Id]))
        .build_sqlx(PostgresQueryBuilder);
    let (user_id,): (Uuid,) = sqlx::query_as_with(&sql, values)
        .fetch_one(&pool)
        .await
        .unwrap();

    let tenant_db = TenantDb::new(pool.clone());
    let mut conn = tenant_db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let created = create_api_key(&mut *conn, workspace_id, ApiKeyScope::Write, Some(user_id))
        .await
        .unwrap();

    let ctx = authenticate(&test_support::db_handle(&pool), &created.plaintext)
        .await
        .unwrap();
    assert_eq!(ctx.user_id, Some(user_id));
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_an_unknown_key(pool: PgPool) {
    let err = authenticate(&test_support::db_handle(&pool), "ysr_does_not_exist_at_all")
        .await
        .unwrap_err();

    assert!(matches!(err, YorishiroError::Unauthenticated));
}

#[sqlx::test(migrations = "../../migrations")]
async fn resolves_the_correct_workspace_among_several(pool: PgPool) {
    let (tenant_a, workspace_a) = seed_workspace(&pool).await;
    let (tenant_b, workspace_b) = seed_workspace(&pool).await;
    let tenant_db = TenantDb::new(pool.clone());

    let mut conn_a = tenant_db
        .acquire_for_workspace(tenant_a, workspace_a)
        .await
        .unwrap();
    let key_a = create_api_key(&mut *conn_a, workspace_a, ApiKeyScope::Read, None)
        .await
        .unwrap();

    let mut conn_b = tenant_db
        .acquire_for_workspace(tenant_b, workspace_b)
        .await
        .unwrap();
    let key_b = create_api_key(&mut *conn_b, workspace_b, ApiKeyScope::Read, None)
        .await
        .unwrap();

    let db = test_support::db_handle(&pool);
    let ctx_a = authenticate(&db, &key_a.plaintext).await.unwrap();
    let ctx_b = authenticate(&db, &key_b.plaintext).await.unwrap();

    assert_eq!(ctx_a.workspace_id, workspace_a);
    assert_eq!(ctx_b.workspace_id, workspace_b);
}

#[test]
fn scope_hierarchy_allows_higher_scopes_to_satisfy_lower_requirements() {
    assert!(ApiKeyScope::Write.satisfies(ApiKeyScope::Read));
    assert!(ApiKeyScope::Schema.satisfies(ApiKeyScope::Write));
    assert!(!ApiKeyScope::Read.satisfies(ApiKeyScope::Write));
    assert!(!ApiKeyScope::Write.satisfies(ApiKeyScope::Schema));
}

#[sqlx::test(migrations = "../../migrations")]
async fn require_scope_rejects_insufficient_scope(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let tenant_db = TenantDb::new(pool.clone());
    let mut conn = tenant_db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();

    let created = create_api_key(&mut *conn, workspace_id, ApiKeyScope::Read, None)
        .await
        .unwrap();
    let ctx = authenticate(&test_support::db_handle(&pool), &created.plaintext)
        .await
        .unwrap();

    let err = require_scope(&ctx, ApiKeyScope::Write).unwrap_err();
    assert!(matches!(err, YorishiroError::ScopeInsufficient { .. }));
}

/// Verifies that `authenticate_api_key` is actually needed: authentication must still succeed over a connection that went through the same `SET ROLE yorishiro_app` that `TenantDb::connect` uses in production (which can't bypass RLS and has no `app.current_tenant`/`app.current_workspace` set).
#[sqlx::test(migrations = "../../migrations")]
async fn authenticates_over_a_connection_that_cannot_bypass_rls(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let tenant_db = TenantDb::new(pool.clone());
    let mut conn = tenant_db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let created = create_api_key(&mut *conn, workspace_id, ApiKeyScope::Read, None)
        .await
        .unwrap();

    let restricted_pool = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                // Same session-control statement as `db.rs`'s `TenantDb::connect`:
                // no query-builder form, stays raw SQL.
                sqlx::query("SET ROLE yorishiro_app")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .unwrap();

    // The restricted pool plays both roles here: this test's whole point is that `authenticate`'s Postgres arm runs on `yorishiro_app` (via `tenant`), not the migration role, so `identity` is set to the same pool rather than to a genuinely separate one.
    let restricted_db = DbHandle::Postgres {
        tenant: TenantDb::new(restricted_pool.clone()),
        identity: restricted_pool,
    };
    let ctx = authenticate(&restricted_db, &created.plaintext)
        .await
        .unwrap();

    assert_eq!(ctx.tenant_id, tenant_id);
    assert_eq!(ctx.workspace_id, workspace_id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn authorize_returns_a_usable_connection_for_a_sufficient_scope(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let tenant_db = TenantDb::new(pool.clone());
    let mut conn = tenant_db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let created = create_api_key(&mut *conn, workspace_id, ApiKeyScope::Write, None)
        .await
        .unwrap();
    drop(conn);

    let db = test_support::db_handle(&pool);
    let (ctx, mut conn) = authorize(
        &db,
        &DefaultAuthenticator,
        &created.plaintext,
        ApiKeyScope::Read,
        &[],
    )
    .await
    .unwrap();

    assert_eq!(ctx.tenant_id, tenant_id);
    assert_eq!(ctx.workspace_id, workspace_id);
    // The returned connection already has its RLS context set, so it can read this workspace's own api_keys row without issue.
    let (sql, values) = Query::select()
        .expr(sea_query::Func::count(Expr::col(sea_query::Asterisk)))
        .from((Alias::new("identity"), ApiKeys::Table))
        .build_sqlx(PostgresQueryBuilder);
    let count: (i64,) = sqlx::query_as_with(&sql, values)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(count.0, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn authorize_rejects_insufficient_scope_without_acquiring_a_connection(pool: PgPool) {
    let (tenant_id, workspace_id) = seed_workspace(&pool).await;
    let tenant_db = TenantDb::new(pool.clone());
    let mut conn = tenant_db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let created = create_api_key(&mut *conn, workspace_id, ApiKeyScope::Read, None)
        .await
        .unwrap();
    drop(conn);

    let db = test_support::db_handle(&pool);
    let err = authorize(
        &db,
        &DefaultAuthenticator,
        &created.plaintext,
        ApiKeyScope::Write,
        &[],
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::ScopeInsufficient { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn authorize_rejects_an_unknown_key(pool: PgPool) {
    let db = test_support::db_handle(&pool);

    let err = authorize(
        &db,
        &DefaultAuthenticator,
        "ysr_does_not_exist_at_all",
        ApiKeyScope::Read,
        &[],
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::Unauthenticated));
}

#[test]
fn hex_encode_decode_round_trips() {
    let bytes = [0xde, 0xad, 0xbe, 0xef, 0x00, 0x01];

    let encoded = hex_encode(&bytes);

    assert_eq!(encoded, "deadbeef0001");
    assert_eq!(hex_decode(&encoded).unwrap(), bytes);
}

#[test]
fn hex_encode_of_empty_bytes_is_empty_string() {
    assert_eq!(hex_encode(&[]), "");
    assert_eq!(hex_decode("").unwrap(), Vec::<u8>::new());
}

#[test]
fn hex_decode_rejects_odd_length_input() {
    assert_eq!(hex_decode("abc"), None);
}

#[test]
fn hex_decode_rejects_non_hex_characters() {
    assert_eq!(hex_decode("zz"), None);
    assert_eq!(hex_decode("gg"), None);
}

#[test]
fn hex_decode_rejects_non_ascii_input_without_panicking() {
    assert_eq!(hex_decode("é0"), None);
}

#[test]
fn hex_decode_accepts_uppercase() {
    assert_eq!(hex_decode("DEADBEEF").unwrap(), [0xde, 0xad, 0xbe, 0xef]);
}

/// Every adapter that authenticates a request routes its `Authorization` header through this, so the shapes it accepts and rejects are the shapes the whole server accepts and rejects.
/// The empty-credential case is why this is shared at all: `Authorization: Bearer ` with nothing after it must be rejected the same way by every caller, since hashing the empty string into a lookup can never match and a caller that instead accepted it would disagree silently with every other path.
#[test]
fn bearer_credential_accepts_only_a_non_empty_bearer_token() {
    assert_eq!(
        bearer_credential(Some("Bearer ysr_abc123")),
        Some("ysr_abc123")
    );

    for rejected in [
        None,
        Some(""),
        // A `Bearer` with nothing after it.
        // Hashing the empty string is a lookup that cannot match any key, so accepting it only costs a query.
        // But two adapters disagreeing on the same request is the thing worth preventing.
        Some("Bearer "),
        Some("Bearer"),
        // Another scheme entirely.
        Some("Basic ysr_abc123"),
        // The scheme is case-sensitive here, matching what every client actually sends.
        Some("bearer ysr_abc123"),
        // No space after the scheme, so `ysr_abc123` is not a credential this header carries.
        Some("Bearerysr_abc123"),
    ] {
        assert_eq!(
            bearer_credential(rejected),
            None,
            "{rejected:?} must not yield a credential"
        );
    }
}

/// Whitespace inside the credential is preserved rather than trimmed: an API key never contains a space, so a token that has one is wrong and must fail the key lookup rather than be silently repaired into a different string.
#[test]
fn bearer_credential_does_not_trim_the_token() {
    assert_eq!(bearer_credential(Some("Bearer  padded")), Some(" padded"));
}

/// `migration` sits above `schema`, and round-trips through the column.
///
/// The operations it guards (running a batch migration, undoing one) rewrite rows already stored, where registering a schema adds a version nothing has been written against yet.
/// A `schema` key must not reach them.
#[test]
fn migration_outranks_schema_and_round_trips() {
    assert!(ApiKeyScope::Migration > ApiKeyScope::Schema);
    assert!(ApiKeyScope::Migration.satisfies(ApiKeyScope::Schema));
    assert!(
        !ApiKeyScope::Schema.satisfies(ApiKeyScope::Migration),
        "a schema key must not run a migration"
    );
    assert_eq!(ApiKeyScope::Migration.as_db_str(), "migration");
    assert_eq!(
        ApiKeyScope::from_db_str("migration"),
        Some(ApiKeyScope::Migration)
    );
    assert_eq!(
        ApiKeyScope::from_db_str("audit"),
        None,
        "audit is not defined yet"
    );
}
