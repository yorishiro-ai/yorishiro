use sea_query::{Alias, Iden, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use sqlx::Row;

use crate::test_support;

#[derive(Iden)]
enum Workspaces {
    Table,
    Name,
}

/// The pool `sqlx::test` provides is connected as the admin role (superuser) that ran
/// the migrations, so `TenantDb::new` alone won't make RLS take effect. This test
/// explicitly switches to `yorishiro_app` via `SET ROLE` and verifies that RLS actually
/// blocks cross-tenant access — confirming the effect of the switch `TenantDb::connect`
/// performs in production.
/// `identity.tenants` itself has no grant for `yorishiro_app` (see the role-separation
/// migration), so this exercises RLS through `identity.workspaces` instead, which the
/// app role has a read-only grant on and which is scoped by the same
/// `app.current_tenant` policy.
#[sqlx::test(migrations = "../../migrations")]
async fn rls_blocks_cross_tenant_access_under_restricted_role(pool: PgPool) {
    let tenant_a = test_support::seed_tenant(&pool, "tenant-a").await;
    let tenant_b = test_support::seed_tenant(&pool, "tenant-b").await;
    test_support::seed_workspace(&pool, tenant_a, "workspace-a").await;
    test_support::seed_workspace(&pool, tenant_b, "workspace-b").await;

    let mut conn = pool.acquire().await.unwrap();
    // Same session/connection-control statements as `TenantDb::connect`/
    // `acquire_for_workspace` above -- no query-builder form, stays raw SQL.
    sqlx::query("SET ROLE yorishiro_app")
        .execute(conn.as_mut())
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.current_tenant', $1, false)")
        .bind(tenant_a.to_string())
        .execute(conn.as_mut())
        .await
        .unwrap();

    let (sql, values) = Query::select()
        .column(Workspaces::Name)
        .from((Alias::new("identity"), Workspaces::Table))
        .build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values)
        .fetch_all(conn.as_mut())
        .await
        .unwrap();
    let names: Vec<String> = rows.iter().map(|row| row.get("name")).collect();

    assert_eq!(names, vec!["workspace-a".to_string()]);
}

/// Schemas are isolated per workspace, not per tenant.
///
/// Like the test above, this switches to `yorishiro_app` explicitly: the pool `sqlx::test`
/// hands over is the migration superuser, which bypasses RLS even under FORCE, so a policy
/// test that skips `SET ROLE` passes whatever the policy says.
#[sqlx::test(migrations = "../../migrations")]
async fn rls_blocks_cross_workspace_schema_access_under_restricted_role(pool: PgPool) {
    let tenant = test_support::seed_tenant(&pool, "one-tenant").await;
    let workspace_a = test_support::seed_workspace(&pool, tenant, "workspace-a").await;
    let workspace_b = test_support::seed_workspace(&pool, tenant, "workspace-b").await;

    // Two schemas under the SAME tenant, one per workspace. Under the old tenant-scoped
    // policy both would be visible to either workspace.
    for (workspace, name) in [(workspace_a, "schema-a"), (workspace_b, "schema-b")] {
        sqlx::query(
            "INSERT INTO content.schemas (tenant_id, workspace_id, name, definition) \
             VALUES ($1, $2, $3, '{}'::jsonb)",
        )
        .bind(tenant)
        .bind(workspace)
        .bind(name)
        .execute(&pool)
        .await
        .unwrap();
    }

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SET ROLE yorishiro_app")
        .execute(conn.as_mut())
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.current_tenant', $1, false)")
        .bind(tenant.to_string())
        .execute(conn.as_mut())
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.current_workspace', $1, false)")
        .bind(workspace_a.to_string())
        .execute(conn.as_mut())
        .await
        .unwrap();

    let rows = sqlx::query("SELECT name FROM content.schemas")
        .fetch_all(conn.as_mut())
        .await
        .unwrap();
    let names: Vec<String> = rows.iter().map(|row| row.get("name")).collect();

    assert_eq!(names, vec!["schema-a".to_string()]);
}
