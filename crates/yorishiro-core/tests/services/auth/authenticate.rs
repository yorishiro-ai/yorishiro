use sqlx::PgPool;

use super::*;
use crate::services::auth::{ApiKeyScope, create_api_key};
use crate::test_support;

/// A key that was never issued must be rejected as unauthenticated rather than, say, panicking
/// on a malformed shape -- unauthenticated input is the normal case on a public endpoint.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_key_is_rejected(pool: PgPool) {
    let error = authenticate(&pool, "ysr_deadbeef_notarealkey", None)
        .await
        .unwrap_err();

    assert!(matches!(error, YorishiroError::Unauthenticated));
}

/// Malformed input reaches this function directly from an `Authorization` header, so shapes that
/// never came from `create_api_key` must be rejected the same way rather than mis-parsed.
#[sqlx::test(migrations = "../../migrations")]
async fn malformed_keys_are_rejected_without_panicking(pool: PgPool) {
    for candidate in ["", "ysr_", "ysr_onlyprefix", "no-prefix-at-all", "ysr__"] {
        let error = authenticate(&pool, candidate, None).await.unwrap_err();
        assert!(
            matches!(error, YorishiroError::Unauthenticated),
            "candidate {candidate:?}"
        );
    }
}

/// The happy path: a freshly issued key authenticates and resolves the workspace, tenant and
/// scope the handler then authorizes against.
#[sqlx::test(migrations = "../../migrations")]
async fn a_freshly_issued_key_resolves_its_workspace_and_scope(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let issued = create_api_key(
        &mut conn,
        tenant_id,
        Some(workspace_id),
        ApiKeyScope::Write,
        None,
    )
    .await
    .unwrap();
    drop(conn);

    let ctx = authenticate(&pool, &issued.plaintext, None).await.unwrap();

    assert_eq!(ctx.workspace_id, workspace_id);
    assert_eq!(ctx.tenant_id, tenant_id);
    assert_eq!(ctx.scope, ApiKeyScope::Write);
}

/// A tenant-scoped key carries no workspace of its own, so the one named by `X-Workspace-Id`
/// becomes the request's workspace.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tenant_key_resolves_the_requested_workspace(pool: PgPool) {
    let (tenant_id, workspace_a) = test_support::seed_tenant_and_workspace(&pool).await;
    let workspace_b = test_support::seed_workspace(&pool, tenant_id, "second-workspace").await;
    let mut conn = pool.acquire().await.unwrap();
    let issued = create_api_key(&mut conn, tenant_id, None, ApiKeyScope::Write, None)
        .await
        .unwrap();
    drop(conn);

    // The same key reaches either workspace, depending only on the header.
    for workspace_id in [workspace_a, workspace_b] {
        let ctx = authenticate(&pool, &issued.plaintext, Some(workspace_id))
            .await
            .unwrap();
        assert_eq!(ctx.workspace_id, workspace_id);
        assert_eq!(ctx.tenant_id, tenant_id);
    }
}

/// **The tenant isolation boundary for these keys.** A tenant-scoped key names its workspace per
/// request, so without this check the header alone would carry a caller into any tenant's data.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tenant_key_cannot_reach_another_tenants_workspace(pool: PgPool) {
    let (tenant_a, _) = test_support::seed_tenant_and_workspace(&pool).await;
    let (_, workspace_b) = test_support::seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let issued = create_api_key(&mut conn, tenant_a, None, ApiKeyScope::Write, None)
        .await
        .unwrap();
    drop(conn);

    let error = authenticate(&pool, &issued.plaintext, Some(workspace_b))
        .await
        .unwrap_err();

    assert!(matches!(error, YorishiroError::Unauthenticated));
}

/// A tenant-scoped key has no workspace to fall back on, so omitting the header cannot silently
/// resolve to one -- it must fail rather than pick a workspace on the caller's behalf.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tenant_key_without_a_requested_workspace_is_rejected(pool: PgPool) {
    let (tenant_id, _) = test_support::seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let issued = create_api_key(&mut conn, tenant_id, None, ApiKeyScope::Write, None)
        .await
        .unwrap();
    drop(conn);

    let error = authenticate(&pool, &issued.plaintext, None)
        .await
        .unwrap_err();

    assert!(matches!(error, YorishiroError::Unauthenticated));
}

/// A workspace-scoped key ignores the header entirely -- it resolves to its own workspace, and
/// the adapter (not this function) is what rejects a mismatched request.
#[sqlx::test(migrations = "../../migrations")]
async fn a_workspace_key_ignores_the_requested_workspace(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let other = test_support::seed_workspace(&pool, tenant_id, "second-workspace").await;
    let mut conn = pool.acquire().await.unwrap();
    let issued = create_api_key(
        &mut conn,
        tenant_id,
        Some(workspace_id),
        ApiKeyScope::Read,
        None,
    )
    .await
    .unwrap();
    drop(conn);

    let ctx = authenticate(&pool, &issued.plaintext, Some(other))
        .await
        .unwrap();

    assert_eq!(ctx.workspace_id, workspace_id);
}

/// Only the hash is stored, so a key that differs from the issued one by a single character must
/// not authenticate -- this is what makes the stored hash worth anything.
#[sqlx::test(migrations = "../../migrations")]
async fn a_tampered_secret_does_not_authenticate(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let issued = create_api_key(
        &mut conn,
        tenant_id,
        Some(workspace_id),
        ApiKeyScope::Read,
        None,
    )
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

    let error = authenticate(&pool, &tampered, None).await.unwrap_err();

    assert!(matches!(error, YorishiroError::Unauthenticated));
}
