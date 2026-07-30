use sqlx::PgPool;
use uuid::Uuid;

use yorishiro_core::YorishiroError;
use yorishiro_core::repositories::tenancy::{create_tenant, create_workspace, list_workspaces};

#[sqlx::test(migrations = "../../migrations")]
async fn creates_tenant_and_workspace(pool: PgPool) {
    let tenant = create_tenant(&pool, "acme", None).await.unwrap();
    let workspace = create_workspace(&pool, tenant.id, "default", None, None)
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
    create_workspace(&pool, tenant.id, "first", None, None)
        .await
        .unwrap();

    let err = create_workspace(&pool, tenant.id, "second", None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::Conflict { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_workspace_rejects_unknown_tenant(pool: PgPool) {
    let err = create_workspace(&pool, Uuid::nil(), "orphan", None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}
