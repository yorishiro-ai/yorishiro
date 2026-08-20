use sqlx::PgPool;
use uuid::Uuid;

use crate::YorishiroError;
use crate::models::tenancy::{create_tenant, create_workspace, list_workspaces};

#[sqlx::test(migrations = "../../migrations")]
async fn creates_tenant_and_workspace(pool: PgPool) {
    let tenant = create_tenant(&pool, "acme", None).await.unwrap();
    let workspace = create_workspace(&pool, tenant.id, "default", None, None, None)
        .await
        .unwrap();
    assert_eq!(workspace.tenant_id, tenant.id);

    let workspaces = list_workspaces(&pool, tenant.id).await.unwrap();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].id, workspace.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn enforces_max_workspaces(pool: PgPool) {
    let tenant = create_tenant(&pool, "capped", Some(1)).await.unwrap();
    create_workspace(&pool, tenant.id, "first", None, None, None)
        .await
        .unwrap();

    let err = create_workspace(&pool, tenant.id, "second", None, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::Conflict { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_workspace_rejects_unknown_tenant(pool: PgPool) {
    let err = create_workspace(&pool, Uuid::nil(), "orphan", None, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

/// A tenant must keep one workspace: with none left it cannot issue itself an API key through the
/// REST API at all, so this is a self-lockout rather than a reversible mistake.
#[sqlx::test(migrations = "../../migrations")]
async fn delete_workspace_refuses_the_last_one(pool: PgPool) {
    use crate::models::tenancy::delete_workspace;

    let tenant = create_tenant(&pool, "acme", None).await.unwrap();
    let first = create_workspace(&pool, tenant.id, "first", None, None, None)
        .await
        .unwrap();
    let second = create_workspace(&pool, tenant.id, "second", None, None, None)
        .await
        .unwrap();

    delete_workspace(&pool, first.id).await.unwrap();

    let err = delete_workspace(&pool, second.id).await.unwrap_err();
    assert!(matches!(err, YorishiroError::Conflict { .. }));
    assert_eq!(list_workspaces(&pool, tenant.id).await.unwrap().len(), 1);

    // A workspace that never existed is still a 404, rather than the conflict above.
    let err = delete_workspace(&pool, Uuid::nil()).await.unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

/// The count and the delete are one statement, so requests racing to delete a tenant's workspaces
/// cannot all read "one spare left" and all proceed.
/// Spawned behind a barrier, since awaiting them in sequence never races.
#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_deletes_cannot_empty_a_tenant(pool: PgPool) {
    use crate::models::tenancy::delete_workspace;
    use std::sync::Arc;
    use tokio::sync::Barrier;

    let tenant = create_tenant(&pool, "acme", None).await.unwrap();
    let mut ids = Vec::new();
    for n in 0..8 {
        ids.push(
            create_workspace(&pool, tenant.id, &format!("ws{n}"), None, None, None)
                .await
                .unwrap()
                .id,
        );
    }

    let barrier = Arc::new(Barrier::new(ids.len()));
    let mut handles = Vec::new();
    for id in ids {
        let pool = pool.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            delete_workspace(&pool, id).await
        }));
    }
    for handle in handles {
        // Which delete won and which lost is the race itself; the count left behind is the claim.
        let _ = handle.await.unwrap();
    }

    let remaining = list_workspaces(&pool, tenant.id).await.unwrap();
    assert_eq!(
        remaining.len(),
        1,
        "concurrent deletes emptied the tenant: {remaining:?}"
    );
}
