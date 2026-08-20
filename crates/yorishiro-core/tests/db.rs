use sea_query::{Alias, Iden, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use sqlx::Row;

use crate::db::{Storage, TenantDb};
use crate::test_support;
use uuid::Uuid;

#[derive(Iden)]
enum Workspaces {
    Table,
    Name,
}

/// The pool `sqlx::test` provides is connected as the admin role (superuser) that ran the migrations, so `TenantDb::new` alone won't make RLS take effect.
/// This test explicitly switches to `yorishiro_app` via `SET ROLE` and verifies that RLS actually blocks cross-tenant access: confirming the effect of the switch `TenantDb::connect` performs in production.
/// `identity.tenants` itself has no grant for `yorishiro_app` (see the role-separation migration), so this exercises RLS through `identity.workspaces` instead, which the app role has a read-only grant on and which is scoped by the same `app.current_tenant` policy.
#[sqlx::test(migrations = "../../migrations")]
async fn rls_blocks_cross_tenant_access_under_restricted_role(pool: PgPool) {
    let tenant_a = test_support::seed_tenant(&pool, "tenant-a").await;
    let tenant_b = test_support::seed_tenant(&pool, "tenant-b").await;
    test_support::seed_workspace(&pool, tenant_a, "workspace-a").await;
    test_support::seed_workspace(&pool, tenant_b, "workspace-b").await;

    let mut conn = pool.acquire().await.unwrap();
    // Same session/connection-control statements as `TenantDb::connect`/ `acquire_for_workspace` above: no query-builder form, stays raw SQL.
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
/// Like the test above, this switches to `yorishiro_app` explicitly: the pool `sqlx::test` hands over is the migration superuser, which bypasses RLS even under FORCE, so a policy test that skips `SET ROLE` passes whatever the policy says.
#[sqlx::test(migrations = "../../migrations")]
async fn rls_blocks_cross_workspace_schema_access_under_restricted_role(pool: PgPool) {
    let tenant = test_support::seed_tenant(&pool, "one-tenant").await;
    let workspace_a = test_support::seed_workspace(&pool, tenant, "workspace-a").await;
    let workspace_b = test_support::seed_workspace(&pool, tenant, "workspace-b").await;

    // Two schemas under the SAME tenant, one per workspace.
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

/// The seam is only a seam if a caller can hold it without naming the implementation.
/// This takes `&dyn Storage`, so it compiles against the trait alone: an engine added later satisfies the same signature without touching this function.
async fn count_through_the_seam(storage: &dyn Storage, tenant_id: Uuid, workspace_id: Uuid) -> i64 {
    let mut conn = storage
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM content.entities")
        .fetch_one(conn.as_mut())
        .await
        .unwrap();
    count
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_storage_trait_scopes_a_connection_like_the_concrete_type(pool: PgPool) {
    let (tenant_id, workspace_id) = test_support::seed_tenant_and_workspace(&pool).await;
    let db = TenantDb::new(pool);

    // Through the trait object, not the struct.
    let via_trait = count_through_the_seam(&db, tenant_id, workspace_id).await;

    // And directly, for comparison.
    // Both run with the same session variables set, so an implementation that forgot to scope the connection would differ here rather than silently returning another workspace's rows.
    let mut conn = db
        .acquire_for_workspace(tenant_id, workspace_id)
        .await
        .unwrap();
    let (direct,): (i64,) = sqlx::query_as("SELECT count(*) FROM content.entities")
        .fetch_one(conn.as_mut())
        .await
        .unwrap();

    assert_eq!(via_trait, direct);
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_storage_trait_exposes_the_unscoped_pool_for_the_control_plane(pool: PgPool) {
    let db = TenantDb::new(pool);
    let storage: &dyn Storage = &db;

    // The control-plane paths need a pool that is not workspace-scoped; the trait has to keep offering one or signup and setup have nowhere to run.
    let (one,): (i32,) = sqlx::query_as("SELECT 1")
        .fetch_one(storage.pool())
        .await
        .unwrap();
    assert_eq!(one, 1);
}

/// The lock has to actually exclude, not just execute.
/// Two transactions take the same key; the second must wait for the first to commit rather than proceeding beside it.
#[sqlx::test(migrations = "../../migrations")]
async fn lock_for_update_serializes_transactions_on_the_same_key(pool: PgPool) {
    use sqlx::Acquire;

    let mut first = pool.acquire().await.unwrap();
    let mut first_tx = first.begin().await.unwrap();
    crate::db::lock_for_update(&mut first_tx, "same-key")
        .await
        .unwrap();

    // A second transaction wanting the same key cannot get it while the first holds it.
    // Bounded by a timeout so a regression fails here instead of hanging the suite.
    // The connection is dropped with the future: a timed-out attempt is still queued for the lock server-side, and reusing it would have the next attempt wait behind its own ghost.
    {
        let mut second = pool.acquire().await.unwrap();
        let blocked = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            let mut second_tx = second.begin().await.unwrap();
            crate::db::lock_for_update(&mut second_tx, "same-key")
                .await
                .unwrap();
            second_tx.commit().await.unwrap();
        })
        .await;
        assert!(
            blocked.is_err(),
            "the second transaction should have waited"
        );
    }

    // Releasing the first lets a fresh attempt through, which is what makes this exclusion rather than a deadlock.
    first_tx.commit().await.unwrap();
    let mut third = pool.acquire().await.unwrap();
    let proceeds = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut third_tx = third.begin().await.unwrap();
        crate::db::lock_for_update(&mut third_tx, "same-key")
            .await
            .unwrap();
        third_tx.commit().await.unwrap();
    })
    .await;
    assert!(proceeds.is_ok(), "the lock should release on commit");
}

/// Different keys do not block each other: otherwise the lock would serialize every workspace's writes against every other's.
#[sqlx::test(migrations = "../../migrations")]
async fn lock_for_update_does_not_serialize_different_keys(pool: PgPool) {
    use sqlx::Acquire;

    let mut first = pool.acquire().await.unwrap();
    let mut first_tx = first.begin().await.unwrap();
    crate::db::lock_for_update(&mut first_tx, "key-a")
        .await
        .unwrap();

    let mut second = pool.acquire().await.unwrap();
    let proceeds = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut second_tx = second.begin().await.unwrap();
        crate::db::lock_for_update(&mut second_tx, "key-b")
            .await
            .unwrap();
        second_tx.commit().await.unwrap();
    })
    .await;
    assert!(proceeds.is_ok(), "a different key should not be blocked");

    first_tx.commit().await.unwrap();
}

/// The lock must actually exclude, and the check has to be able to fail.
///
/// Eight racers rather than two: a race that reproduces one run in three reads as a flaky test rather than a broken guard, and two contenders are not enough to lose reliably.
/// Spawned behind a barrier because `tokio::join!` polls on one thread and never overlaps the critical section.
///
/// What it measures is the shape the Stripe webhook needs: each racer takes its own connection, reads a value, and writes back one higher.
/// Without exclusion the reads interleave and the final count is below eight; with it, every increment is serialised and the count is exactly eight.
#[sqlx::test(migrations = "../../migrations")]
async fn eight_racers_holding_the_same_key_do_not_interleave(pool: sqlx::PgPool) {
    sqlx::query("CREATE TABLE counter (id INT PRIMARY KEY, n INT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO counter (id, n) VALUES (1, 0)")
        .execute(&pool)
        .await
        .unwrap();

    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(8));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let pool = pool.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            let mut guard = super::SessionLock::acquire(&pool, "race-test")
                .await
                .unwrap();

            // On the guard's own connection, not a second one from the pool: eight holders each
            // taking two connections exhaust it, and the test would then fail on `PoolTimedOut`
            // whether or not the lock works. Measured, not assumed.
            let (n,): (i32,) = sqlx::query_as("SELECT n FROM counter WHERE id = 1")
                .fetch_one(guard.conn())
                .await
                .unwrap();
            // The window the lock exists to close: without it every racer reads the same `n`.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            sqlx::query("UPDATE counter SET n = $1 WHERE id = 1")
                .bind(n + 1)
                .execute(guard.conn())
                .await
                .unwrap();

            guard.release().await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let (n,): (i32,) = sqlx::query_as("SELECT n FROM counter WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 8, "increments interleaved: the lock did not exclude");
}

/// Two different keys must not block each other, or the Stripe webhook would serialise every customer behind whichever one arrived first.
#[sqlx::test(migrations = "../../migrations")]
async fn different_keys_do_not_block_each_other(pool: sqlx::PgPool) {
    let first = super::SessionLock::acquire(&pool, "customer-a")
        .await
        .unwrap();

    // Would hang rather than fail if the key were ignored, so it is bounded: a timeout here is the failure.
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        super::SessionLock::acquire(&pool, "customer-b"),
    )
    .await
    .expect("a lock on a different key blocked, so the key is not part of the exclusion")
    .unwrap();

    second.release().await.unwrap();
    first.release().await.unwrap();
}

/// Dropping the guard without calling `release` must still free the lock.
///
/// The whole exclusion rests on this: a task that panics inside the guarded section never reaches `release`, and if the lock outlived the connection's return to the pool, every later delivery for that customer would queue behind a holder that no longer exists.
/// `pg_advisory_lock` is session-scoped, so what actually frees it is the session ending or the pool resetting the connection.
/// Which of those sqlx does is an unstated property of the library rather than of this code, so it is measured here instead of assumed.
#[sqlx::test(migrations = "../../migrations")]
async fn a_dropped_guard_frees_the_lock_without_release(pool: sqlx::PgPool) {
    {
        let _guard = super::SessionLock::acquire(&pool, "dropped-key")
            .await
            .unwrap();
        // Falls out of scope without `release`, which is what a panicking task would do.
    }

    // Bounded: if the lock survived the drop this blocks forever rather than failing, and a
    // hanging test reads as a stuck runner rather than as a broken guarantee.
    let regained = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        super::SessionLock::acquire(&pool, "dropped-key"),
    )
    .await
    .expect("a dropped guard left its lock held, so a panicking holder would block the key forever")
    .unwrap();

    regained.release().await.unwrap();
}

/// Proves the generic bounds `models/entities::get`/`count` carry are satisfiable by `SqliteConnection`, at the type level only.
/// Naming the function items is enough: it does not run, so a mismatched bound is a compile error here rather than a monomorphization failure deep in some future SQLite-only caller.
/// This says nothing about the SQL those functions build or how it behaves on Sqlite (`Alias::new("content")` schema-qualification, the RLS substitute): that is step 4's problem, not this one.
#[cfg(feature = "sqlite")]
#[test]
fn sqlite_satisfies_the_generic_bounds() {
    let _ = crate::models::entities::get::<sqlx::SqliteConnection>;
    let _ = crate::models::entities::count::<sqlx::SqliteConnection>;
}
