use sqlx::PgPool;

use super::*;
use crate::services::auth::{ApiKeyScope, create_api_key};
use crate::test_support;

/// A key that was never issued must be rejected as unauthenticated rather than, say, panicking on a malformed shape: unauthenticated input is the normal case on a public endpoint.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_key_is_rejected(pool: PgPool) {
    let error = authenticate(&pool, "ysr_deadbeef_notarealkey")
        .await
        .unwrap_err();

    assert!(matches!(error, YorishiroError::Unauthenticated));
}

/// Malformed input reaches this function directly from an `Authorization` header, so shapes that never came from `create_api_key` must be rejected the same way rather than mis-parsed.
#[sqlx::test(migrations = "../../migrations")]
async fn malformed_keys_are_rejected_without_panicking(pool: PgPool) {
    for candidate in ["", "ysr_", "ysr_onlyprefix", "no-prefix-at-all", "ysr__"] {
        let error = authenticate(&pool, candidate).await.unwrap_err();
        assert!(
            matches!(error, YorishiroError::Unauthenticated),
            "candidate {candidate:?}"
        );
    }
}

/// The happy path: a freshly issued key authenticates and resolves the workspace, tenant and scope the handler then authorizes against.
#[sqlx::test(migrations = "../../migrations")]
async fn a_freshly_issued_key_resolves_its_workspace_and_scope(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let issued = create_api_key(&mut conn, workspace_id, ApiKeyScope::Write, None)
        .await
        .unwrap();
    drop(conn);

    let ctx = authenticate(&pool, &issued.plaintext).await.unwrap();

    assert_eq!(ctx.workspace_id, workspace_id);
    assert_eq!(ctx.tenant_id, tenant_id);
    assert_eq!(ctx.scope, ApiKeyScope::Write);
}

/// Only the hash is stored, so a key that differs from the issued one by a single character must not authenticate: this is what makes the stored hash worth anything.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tampered_secret_does_not_authenticate(pool: PgPool) {
    let (_, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let issued = create_api_key(&mut conn, workspace_id, ApiKeyScope::Read, None)
        .await
        .unwrap();
    drop(conn);

    let mut tampered = issued.plaintext.clone();
    tampered.pop();
    tampered.push(if issued.plaintext.ends_with('a') {
        'b'
    } else {
        'a'
    });

    let error = authenticate(&pool, &tampered).await.unwrap_err();

    assert!(matches!(error, YorishiroError::Unauthenticated));
}
