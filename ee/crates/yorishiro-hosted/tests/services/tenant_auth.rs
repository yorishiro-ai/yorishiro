use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::YorishiroError;
use yorishiro_core::services::auth::{ApiKeyScope, Authenticator};

use super::*;

use crate::tests::test_helpers;

/// Issues a key directly.
/// The community edition's `create_api_key` always records a workspace, so a tenant-scoped key (the whole point here) cannot be made through it.
async fn issue_key(
    pool: &PgPool,
    tenant_id: Uuid,
    workspace_id: Option<Uuid>,
    scope: ApiKeyScope,
) -> String {
    let secret = format!("ysr_test_{}", Uuid::new_v4().simple());
    let hash = yorishiro_core::services::auth::hash_key(&secret);
    sqlx::query(
        "INSERT INTO identity.api_keys (tenant_id, workspace_id, key_hash, key_prefix, scope) \
         VALUES ($1, $2, $3, 'ysr_test', $4)",
    )
    .bind(tenant_id)
    .bind(workspace_id)
    .bind(hash)
    .bind(scope.as_db_str())
    .execute(pool)
    .await
    .unwrap();
    secret
}

fn workspace_header(id: Uuid) -> Vec<(String, String)> {
    vec![(WORKSPACE_HEADER.to_string(), id.to_string())]
}

/// A tenant-scoped key carries no workspace of its own, so the one named by the header becomes the request's workspace.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_tenant_key_resolves_the_requested_workspace(pool: PgPool) {
    let (tenant_id, workspace_a) = test_helpers::seed_tenant_and_workspace(&pool).await;
    let workspace_b = test_helpers::seed_workspace(&pool, tenant_id, "second").await;
    let key = issue_key(&pool, tenant_id, None, ApiKeyScope::Write).await;

    for workspace_id in [workspace_a, workspace_b] {
        let ctx = TenantScopedAuthenticator
            .authenticate(&pool, &key, &workspace_header(workspace_id))
            .await
            .unwrap();
        assert_eq!(ctx.workspace_id, workspace_id);
        assert_eq!(ctx.tenant_id, tenant_id);
    }
}

/// **The tenant isolation boundary for these keys.** The workspace is named per request, so without this check the header alone would carry a caller into any tenant's data.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_tenant_key_cannot_reach_another_tenants_workspace(pool: PgPool) {
    let (tenant_a, _) = test_helpers::seed_tenant_and_workspace(&pool).await;
    let (_, workspace_b) = test_helpers::seed_tenant_and_workspace(&pool).await;
    let key = issue_key(&pool, tenant_a, None, ApiKeyScope::Write).await;

    let err = TenantScopedAuthenticator
        .authenticate(&pool, &key, &workspace_header(workspace_b))
        .await
        .unwrap_err();

    assert!(matches!(err, YorishiroError::Unauthenticated));
}

/// A tenant-scoped key has no workspace to fall back on, so omitting the header cannot silently resolve to one: it must fail rather than pick a workspace on the caller's behalf.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_tenant_key_without_a_requested_workspace_is_rejected(pool: PgPool) {
    let (tenant_id, _) = test_helpers::seed_tenant_and_workspace(&pool).await;
    let key = issue_key(&pool, tenant_id, None, ApiKeyScope::Write).await;

    let err = TenantScopedAuthenticator
        .authenticate(&pool, &key, &[])
        .await
        .unwrap_err();

    assert!(matches!(err, YorishiroError::Unauthenticated));
}

/// A workspace-scoped key still works with no header at all: the community edition's own behaviour has to survive replacing the authenticator.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_workspace_key_still_authenticates_without_a_header(pool: PgPool) {
    let (tenant_id, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;
    let key = issue_key(&pool, tenant_id, Some(workspace_id), ApiKeyScope::Read).await;

    let ctx = TenantScopedAuthenticator
        .authenticate(&pool, &key, &[])
        .await
        .unwrap();

    assert_eq!(ctx.workspace_id, workspace_id);
    assert_eq!(ctx.scope, ApiKeyScope::Read);
}

/// Naming a *different* workspace on a workspace-scoped key is refused rather than ignored:
/// acting on the key's own workspace instead would put a write somewhere the client never named.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_workspace_key_naming_another_workspace_is_rejected(pool: PgPool) {
    let (tenant_id, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;
    let other = test_helpers::seed_workspace(&pool, tenant_id, "other").await;
    let key = issue_key(&pool, tenant_id, Some(workspace_id), ApiKeyScope::Read).await;

    let err = TenantScopedAuthenticator
        .authenticate(&pool, &key, &workspace_header(other))
        .await
        .unwrap_err();

    assert!(matches!(err, YorishiroError::ValidationFailed { .. }));
}

/// An unparseable header is an error rather than an omission: treating it as "not sent" would send a request meant for one workspace to whichever one the key happens to carry.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_malformed_header_is_rejected(pool: PgPool) {
    let (tenant_id, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;
    let key = issue_key(&pool, tenant_id, Some(workspace_id), ApiKeyScope::Read).await;

    let err = TenantScopedAuthenticator
        .authenticate(
            &pool,
            &key,
            &[(WORKSPACE_HEADER.to_string(), "not-a-uuid".to_string())],
        )
        .await
        .unwrap_err();

    assert!(matches!(err, YorishiroError::ValidationFailed { .. }));
}

/// An unknown key is rejected whatever the header says.
#[sqlx::test(migrations = "../../../migrations")]
async fn an_unknown_key_is_rejected(pool: PgPool) {
    let (_, workspace_id) = test_helpers::seed_tenant_and_workspace(&pool).await;

    let err = TenantScopedAuthenticator
        .authenticate(&pool, "ysr_nope_nope", &workspace_header(workspace_id))
        .await
        .unwrap_err();

    assert!(matches!(err, YorishiroError::Unauthenticated));
}
