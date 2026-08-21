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
/// This takes `&dyn Storage<Db = sqlx::Postgres>`, so it compiles against the trait alone: another `TenantDb`-shaped Postgres implementation satisfies the same signature without touching this function.
async fn count_through_the_seam(
    storage: &dyn Storage<Db = sqlx::Postgres>,
    tenant_id: Uuid,
    workspace_id: Uuid,
) -> i64 {
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
    let storage: &dyn Storage<Db = sqlx::Postgres> = &db;

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

/// Proves the generic bounds `models/entities::get`/`count`/`update`, `models/relations::create`, and `models/export::export_all` carry are satisfiable by `SqliteConnection`, at the type level only.
/// Naming the function items is enough: it does not run, so a mismatched bound is a compile error here rather than a monomorphization failure deep in some future SQLite-only caller.
/// `update` calls into `schemas::get_by_id`, so this is also the check that a cross-module call's transcribed field bounds (see `entities::update`'s where clause) actually hold for a second engine, not just for Postgres.
/// `relations::create` is the deepest chain so far: it calls `entities::get` and, through `validate_relation_type`, `schemas::get_by_id`, so it exercises the transcription on top of another module's own bounds.
/// `export::export_all` composes all three modules' `export_all` in one function, so it proves the three bound sets combine without conflict rather than only each holding on its own.
/// This says nothing about the SQL those functions build or how it behaves on Sqlite (`Alias::new("content")` schema-qualification, the RLS substitute): that is step 4's problem, not this one.
#[cfg(feature = "sqlite")]
#[test]
fn sqlite_satisfies_the_generic_bounds() {
    let _ = crate::models::entities::get::<sqlx::SqliteConnection>;
    let _ = crate::models::entities::count::<sqlx::SqliteConnection>;
    let _ = crate::models::entities::update::<sqlx::SqliteConnection>;
    let _ = crate::models::entities::migration_dry_run::<sqlx::SqliteConnection>;
    let _ = crate::models::entities::snapshot::<sqlx::SqliteConnection>;
    let _ = crate::models::entities::snapshots_for_job::<sqlx::SqliteConnection>;
    let _ = crate::models::entities::undo_job::<sqlx::SqliteConnection>;
    let _ = crate::models::relations::create::<sqlx::SqliteConnection>;
    let _ = crate::models::export::export_all::<sqlx::SqliteConnection>;
}

/// `Engine::schema_table` must actually change what SQL comes out for Postgres, not just compile.
#[test]
fn schema_table_qualifies_on_postgres() {
    let pg_sql = Query::select()
        .column(Workspaces::Name)
        .from(<sqlx::PgConnection as crate::db::Engine>::schema_table(
            "identity",
            Workspaces::Table,
        ))
        .to_string(PostgresQueryBuilder);
    assert!(
        pg_sql.contains(r#""identity"."workspaces""#),
        "Postgres output must carry the schema qualification: {pg_sql}"
    );
}

/// The Sqlite half of the same check: `Engine::schema_table` must drop the qualifier, through the trait impl itself rather than a hand-built `TableRef` that only looks like what the impl would produce.
/// Sqlite has no schema concept for a single-file database, so `identity`/`content` qualifying a table there is a syntax error, not a no-op.
#[cfg(feature = "sqlite")]
#[test]
fn schema_table_stays_bare_on_sqlite() {
    let sqlite_sql = Query::select()
        .column(Workspaces::Name)
        .from(<sqlx::SqliteConnection as crate::db::Engine>::schema_table(
            "identity",
            Workspaces::Table,
        ))
        .to_string(sea_query::SqliteQueryBuilder);
    assert!(
        !sqlite_sql.contains("identity"),
        "the qualifier must be absent: {sqlite_sql}"
    );
    assert!(
        sqlite_sql.contains(r#""workspaces""#),
        "the bare table name must still be present: {sqlite_sql}"
    );
}

/// Runs PR #213's four rewritten functions against a real Sqlite database, not just the type-level
/// bounds `sqlite_satisfies_the_generic_bounds` proves or the rendered-SQL strings
/// `migration_dry_run_rendering` checks.
///
/// There is no Sqlite migration set yet (that is item 4's actual driver work, not this
/// verification), so the DDL here is test-local: bare `schemas`/`entities`/`entity_snapshots`
/// tables carrying exactly the columns these functions' SELECTs and INSERTs name, no `content.`
/// prefix (Sqlite has no schema concept for a single-file database, matching `schema_table_stays_bare_on_sqlite`
/// above).
///
/// Fixture choices worth recording as open item-4 questions rather than settled here, each
/// discovered by actually running these functions rather than assumed up front:
/// - IDs are seeded as explicit `Uuid`s because Sqlite has no `uuidv7()`. This doesn't reopen
///   「アプリ側でIDを採番しない」, which governs production write paths; how a real Sqlite driver
///   gets IDs is exactly the kind of thing item 4 has to decide, not something this test can settle.
///   `snapshot()`'s own INSERT..SELECT never names `entity_snapshots.id` either (mirroring
///   Postgres's `DEFAULT uuidv7()`), so the fixture table needs its own default; `randomblob(16)`
///   is not a real UUID, but PK uniqueness is all this test needs from it.
/// - `sqlx-sqlite` encodes `Uuid` as 16 raw BLOB bytes, not the hyphenated string form, so every
///   Uuid column here is `BLOB` and every fixture insert binds the `Uuid` value directly rather
///   than `.to_string()`ing it: binding the string form silently produces zero query matches
///   instead of a decode error, since Sqlite's dynamic typing accepts either into a `BLOB` column
///   without complaint.
/// - `created_at` uses `strftime('%Y-%m-%dT%H:%M:%fZ','now')` for millisecond precision rather than
///   Sqlite's default `CURRENT_TIMESTAMP` (second granularity). Millisecond precision is still not
///   enough to guarantee two `snapshot()` calls a few lines apart land in different ticks, so
///   `snapshots_for_job`'s `ORDER BY created_at DESC` (no tiebreaker column) is asserted as a set
///   below, not by position: which of two same-millisecond snapshots sorts first is genuinely
///   unspecified today, an actual constraint for item 4's real driver, not a fixture timing bug.
///   `undo_job`'s own `ORDER BY created_at ASC` shares the same gap (only untested here because
///   each entity below has exactly one snapshot, so replay order can't change the outcome).
#[cfg(feature = "sqlite")]
mod sqlite_execution {
    use serde_json::json;
    use sqlx::{Connection, SqliteConnection};
    use uuid::Uuid;

    use crate::YorishiroError;
    use crate::models::entities;

    async fn seeded_db() -> SqliteConnection {
        let mut conn = SqliteConnection::connect("sqlite::memory:").await.unwrap();

        // Uuid columns are BLOB, matching sqlx-sqlite's own Encode<Sqlite> for Uuid (16 raw bytes,
        // not the hyphenated string form): the discovery this test exists to make is exactly that
        // mismatch, so the fixture has to bind the same way the product code's queries do.
        sqlx::query(
            "CREATE TABLE schemas ( \
                 id BLOB PRIMARY KEY, \
                 tenant_id BLOB NOT NULL, \
                 workspace_id BLOB NOT NULL, \
                 name TEXT NOT NULL, \
                 version INTEGER NOT NULL, \
                 definition TEXT NOT NULL, \
                 status TEXT NOT NULL, \
                 origin_template_id BLOB, \
                 origin_status TEXT NOT NULL, \
                 origin_snapshot TEXT, \
                 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')) \
             )",
        )
        .execute(&mut conn)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE entities ( \
                 id BLOB PRIMARY KEY, \
                 workspace_id BLOB NOT NULL, \
                 schema_id BLOB NOT NULL, \
                 schema_version INTEGER NOT NULL, \
                 entity_type TEXT NOT NULL, \
                 data TEXT NOT NULL, \
                 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')), \
                 updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')), \
                 created_by BLOB, \
                 updated_by BLOB \
             )",
        )
        .execute(&mut conn)
        .await
        .unwrap();

        // `snapshot()` never names `id` in its INSERT..SELECT (matching Postgres's own
        // `DEFAULT uuidv7()`, which this crate's app code never assigns around, see
        // 「アプリ側でIDを採番しない」), so the fixture needs its own default here. `randomblob(16)`
        // is not a real UUID, but PK uniqueness is all this test needs from it; a real Sqlite
        // driver's actual ID strategy is an open item-4 question, not something this test settles.
        sqlx::query(
            "CREATE TABLE entity_snapshots ( \
                 id BLOB PRIMARY KEY DEFAULT (randomblob(16)), \
                 job_id BLOB NOT NULL, \
                 workspace_id BLOB NOT NULL, \
                 entity_id BLOB NOT NULL, \
                 schema_id BLOB NOT NULL, \
                 schema_version INTEGER NOT NULL, \
                 data TEXT NOT NULL, \
                 created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')) \
             )",
        )
        .execute(&mut conn)
        .await
        .unwrap();

        conn
    }

    async fn insert_schema(
        conn: &mut SqliteConnection,
        id: Uuid,
        workspace_id: Uuid,
        name: &str,
        version: i32,
        definition: &serde_json::Value,
        status: &str,
    ) {
        sqlx::query(
            "INSERT INTO schemas \
                 (id, tenant_id, workspace_id, name, version, definition, status, origin_status) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 'none')",
        )
        .bind(id)
        .bind(Uuid::nil())
        .bind(workspace_id)
        .bind(name)
        .bind(version)
        .bind(definition.to_string())
        .bind(status)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_entity(
        conn: &mut SqliteConnection,
        id: Uuid,
        workspace_id: Uuid,
        schema_id: Uuid,
        schema_version: i32,
        entity_type: &str,
        data: &serde_json::Value,
    ) {
        sqlx::query(
            "INSERT INTO entities \
                 (id, workspace_id, schema_id, schema_version, entity_type, data) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(schema_id)
        .bind(schema_version)
        .bind(entity_type)
        .bind(data.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    fn v1_definition() -> serde_json::Value {
        json!({
            "name": "task-management",
            "entity_types": {
                "task": {
                    "fields": {
                        "title": { "type": "string", "required": true }
                    }
                }
            }
        })
    }

    /// `priority` already exists here, so an entity on this version that supplies it is
    /// behind but valid once v3 makes the field required, unlike one still on v1.
    fn v2_definition() -> serde_json::Value {
        json!({
            "name": "task-management",
            "entity_types": {
                "task": {
                    "fields": {
                        "title": { "type": "string", "required": true },
                        "priority": { "type": "integer", "required": false }
                    }
                }
            }
        })
    }

    fn v3_definition() -> serde_json::Value {
        json!({
            "name": "task-management",
            "entity_types": {
                "task": {
                    "fields": {
                        "title": { "type": "string", "required": true },
                        "priority": { "type": "integer", "required": true }
                    }
                }
            }
        })
    }

    /// The four functions PR #213 rewrote, exercised end to end against a real Sqlite database:
    /// seed a schema at two versions plus entities under each, run `migration_dry_run`, `snapshot`,
    /// `snapshots_for_job` and `undo_job` in sequence, and check each one's actual effect rather
    /// than only that it returns `Ok`.
    #[tokio::test]
    async fn the_four_rewritten_functions_execute_and_behave_on_sqlite() {
        let mut conn = seeded_db().await;

        let workspace_id = Uuid::new_v4();
        let schema_v1 = Uuid::new_v4();
        let schema_v2 = Uuid::new_v4();
        let schema_v3 = Uuid::new_v4();
        insert_schema(
            &mut conn,
            schema_v1,
            workspace_id,
            "task-management",
            1,
            &v1_definition(),
            "archived",
        )
        .await;
        insert_schema(
            &mut conn,
            schema_v2,
            workspace_id,
            "task-management",
            2,
            &v2_definition(),
            "archived",
        )
        .await;
        insert_schema(
            &mut conn,
            schema_v3,
            workspace_id,
            "task-management",
            3,
            &v3_definition(),
            "active",
        )
        .await;

        // One entity on v1 (missing `priority`, which v1 never declared: needs a value filled in),
        // one on v2 (already supplies `priority`, which v2 already declared as optional: behind but
        // valid), one already on v3, the active version.
        let old_missing = Uuid::new_v4();
        let old_valid = Uuid::new_v4();
        let current = Uuid::new_v4();
        insert_entity(
            &mut conn,
            old_missing,
            workspace_id,
            schema_v1,
            1,
            "task",
            &json!({ "title": "no priority yet" }),
        )
        .await;
        insert_entity(
            &mut conn,
            old_valid,
            workspace_id,
            schema_v2,
            2,
            "task",
            &json!({ "title": "already has one", "priority": 3 }),
        )
        .await;
        insert_entity(
            &mut conn,
            current,
            workspace_id,
            schema_v3,
            3,
            "task",
            &json!({ "title": "already current", "priority": 1 }),
        )
        .await;

        // migration_dry_run: the JOIN + GROUP BY query, on Sqlite.
        let dry_run = entities::migration_dry_run(&mut conn, workspace_id, "task-management")
            .await
            .unwrap();
        assert_eq!(dry_run.total_entities, 3);
        assert_eq!(dry_run.current, 1);
        assert_eq!(dry_run.behind_but_valid, 1);
        assert_eq!(dry_run.needs_values, 1);
        assert_eq!(dry_run.by_entity_type.len(), 1);
        assert_eq!(
            dry_run.by_entity_type[0].missing_required,
            vec!["priority".to_string()]
        );

        // snapshot: the atomic INSERT..SELECT, on the two old-version entities, under one job.
        let job_id = Uuid::new_v4();
        entities::snapshot(&mut conn, workspace_id, old_missing, job_id)
            .await
            .unwrap();
        entities::snapshot(&mut conn, workspace_id, old_valid, job_id)
            .await
            .unwrap();

        // snapshot on a nonexistent entity is NotFound, not a silent no-op insert.
        let missing_err = entities::snapshot(&mut conn, workspace_id, Uuid::new_v4(), job_id)
            .await
            .unwrap_err();
        assert!(matches!(missing_err, YorishiroError::NotFound { .. }));

        // snapshots_for_job: both rows come back. Order is asserted as a set, not by position:
        // `ORDER BY created_at DESC` has no tiebreaker, and Sqlite's `strftime('%f')` is
        // millisecond-precision, which two `snapshot()` calls from the same test can land in
        // within, making DESC ordering between them genuinely unspecified rather than a fixture
        // timing problem to paper over. This is an actual constraint for item 4's real driver to
        // decide (a tiebreaker column, or accepting the ambiguity), recorded in the migration
        // spec rather than solved here.
        let snapshots = entities::snapshots_for_job(&mut conn, workspace_id, job_id)
            .await
            .unwrap();
        assert_eq!(snapshots.len(), 2);
        let snapshot_entity_ids: std::collections::HashSet<_> =
            snapshots.iter().map(|s| s.entity_id).collect();
        assert_eq!(
            snapshot_entity_ids,
            [old_missing, old_valid].into_iter().collect()
        );

        // Mutate the entity, as a migration filling in defaults would, so undo_job has something
        // to actually revert rather than restoring what was already there.
        sqlx::query("UPDATE entities SET data = ? WHERE id = ?")
            .bind(json!({ "title": "overwritten" }).to_string())
            .bind(old_missing)
            .execute(&mut conn)
            .await
            .unwrap();

        // undo_job: restores both entities' original data, deletes the snapshots, and a second
        // undo of the same job is NotFound rather than a silent zero-effect success.
        let report = entities::undo_job(&mut conn, workspace_id, job_id)
            .await
            .unwrap();
        assert_eq!(report.restored, 2);
        assert_eq!(report.missing, 0);

        let restored = entities::get(&mut conn, workspace_id, old_missing)
            .await
            .unwrap();
        assert_eq!(restored.data["title"], "no priority yet");

        let remaining_snapshots: (i64,) =
            sqlx::query_as("SELECT count(*) FROM entity_snapshots WHERE job_id = ?")
                .bind(job_id)
                .fetch_one(&mut conn)
                .await
                .unwrap();
        assert_eq!(remaining_snapshots.0, 0);

        let second_undo = entities::undo_job(&mut conn, workspace_id, job_id)
            .await
            .unwrap_err();
        assert!(matches!(second_undo, YorishiroError::NotFound { .. }));
    }

    /// `undo_job` counts an entity deleted since its snapshot rather than failing the whole batch.
    #[tokio::test]
    async fn undo_job_counts_a_deleted_entity_as_missing_on_sqlite() {
        let mut conn = seeded_db().await;

        let workspace_id = Uuid::new_v4();
        let schema_id = Uuid::new_v4();
        insert_schema(
            &mut conn,
            schema_id,
            workspace_id,
            "task-management",
            1,
            &v1_definition(),
            "active",
        )
        .await;

        let entity_id = Uuid::new_v4();
        insert_entity(
            &mut conn,
            entity_id,
            workspace_id,
            schema_id,
            1,
            "task",
            &json!({ "title": "will be deleted" }),
        )
        .await;

        let job_id = Uuid::new_v4();
        entities::snapshot(&mut conn, workspace_id, entity_id, job_id)
            .await
            .unwrap();

        sqlx::query("DELETE FROM entities WHERE id = ?")
            .bind(entity_id)
            .execute(&mut conn)
            .await
            .unwrap();

        let report = entities::undo_job(&mut conn, workspace_id, job_id)
            .await
            .unwrap();
        assert_eq!(report.restored, 0);
        assert_eq!(report.missing, 1);
    }
}
