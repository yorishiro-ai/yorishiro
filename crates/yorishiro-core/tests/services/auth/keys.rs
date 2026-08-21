use sqlx::PgPool;

use super::*;
use crate::test_support;

#[sqlx::test(migrations = "../../migrations")]
async fn create_api_key_issues_a_prefixed_plaintext_key(pool: PgPool) {
    let (_, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();

    let issued = create_api_key(&mut *conn, workspace_id, ApiKeyScope::Write, None)
        .await
        .unwrap();

    assert!(issued.plaintext.starts_with("ysr_"));
}
