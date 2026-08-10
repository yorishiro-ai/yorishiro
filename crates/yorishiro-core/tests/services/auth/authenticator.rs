use async_trait::async_trait;
use sqlx::PgPool;

use crate::YorishiroError;
use crate::db::TenantDb;
use crate::services::auth::{
    ApiKeyScope, AuthContext, Authenticator, authenticate, authorize, create_api_key,
};
use crate::test_support;

/// An authenticator that ignores the presented key entirely and resolves whatever the caller
/// named in a header. Deliberately unlike the default rule in every respect -- a test that only
/// varied it slightly could pass while the seam was being bypassed.
struct HeaderAuthenticator {
    ctx: AuthContext,
}

#[async_trait]
impl Authenticator for HeaderAuthenticator {
    async fn authenticate(
        &self,
        _pool: &PgPool,
        _presented_key: &str,
        headers: &[(String, String)],
    ) -> Result<AuthContext, YorishiroError> {
        if headers
            .iter()
            .any(|(name, value)| name == "x-test-auth" && value == "let-me-in")
        {
            Ok(self.ctx.clone())
        } else {
            Err(YorishiroError::Unauthenticated)
        }
    }
}

/// The seam has to actually replace the rule: a key the default authenticator would reject must
/// authenticate when a replacement accepts it, and vice versa. Otherwise a downstream deployment
/// silently keeps this crate's behaviour while believing it has replaced it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replaced_authenticator_decides_instead_of_the_default(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let issued = create_api_key(&mut conn, workspace_id, ApiKeyScope::Write, None)
        .await
        .unwrap();
    drop(conn);

    let ctx = authenticate(&pool, &issued.plaintext).await.unwrap();
    let authenticator = HeaderAuthenticator { ctx };

    // A key the default rule accepts is rejected, because the replacement was not satisfied.
    let err = authorize(
        &db,
        &authenticator,
        &issued.plaintext,
        ApiKeyScope::Read,
        &[],
    )
    .await
    .unwrap_err();
    assert!(matches!(err, YorishiroError::Unauthenticated));

    // A key the default rule would reject outright authenticates, because the replacement is
    // what decides. This is the direction that proves the default is not still running.
    let (ctx, _conn) = authorize(
        &db,
        &authenticator,
        "ysr_not_a_real_key_at_all",
        ApiKeyScope::Read,
        &[("x-test-auth".into(), "let-me-in".into())],
    )
    .await
    .unwrap();
    assert_eq!(ctx.workspace_id, workspace_id);
    assert_eq!(ctx.tenant_id, tenant_id);
}

/// Scope is still enforced against whatever context the replacement returns -- replacing
/// authentication must not become a way past authorization.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replaced_authenticator_does_not_bypass_scope(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let issued = create_api_key(&mut conn, workspace_id, ApiKeyScope::Read, None)
        .await
        .unwrap();
    drop(conn);

    let ctx = authenticate(&pool, &issued.plaintext).await.unwrap();
    let authenticator = HeaderAuthenticator { ctx };

    let err = authorize(
        &db,
        &authenticator,
        "anything",
        ApiKeyScope::Schema,
        &[("x-test-auth".into(), "let-me-in".into())],
    )
    .await
    .unwrap_err();
    assert!(matches!(err, YorishiroError::ScopeInsufficient { .. }));
}
