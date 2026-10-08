use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::FutureExt;
use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TryGetable};
use serial_test::serial;

/// Returns the 1-indexed position of the named migration in `Migrator::migrations()`.
/// Panics with a clear message if the migration is missing or renamed.
fn migrations_through(name: &str) -> u32 {
    Migrator::migrations()
        .iter()
        .position(|m| m.name() == name)
        .unwrap_or_else(|| panic!("no migration named {name} in Migrator::migrations()")) as u32
        + 1
}

static DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const TENANT: &str = "11111111-1111-4111-8111-111111111111";
const USER: &str = "22222222-2222-4222-8222-222222222222";
const WORKSPACE: &str = "33333333-3333-4333-8333-333333333333";
const SCHEMA: &str = "44444444-4444-4444-8444-444444444444";
const ENTITY: &str = "55555555-5555-4555-8555-555555555555";
const MEMBERSHIP: &str = "66666666-6666-4666-8666-666666666666";
const LEDGER: &str = "77777777-7777-4777-8777-777777777777";
const JOB: &str = "88888888-8888-4888-8888-888888888888";
const FORK: &str = "99999999-9999-4999-8999-999999999999";
const HEAD: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const TARGET_WORKSPACE: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const TARGET_SCHEMA: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

fn id(db: &DatabaseConnection, value: &str) -> String {
    match db.get_database_backend() {
        DbBackend::Sqlite => format!("X'{}'", value.replace('-', "")),
        DbBackend::Postgres => format!("'{value}'"),
        backend => panic!("unsupported migration test backend: {backend:?}"),
    }
}

async fn execute(db: &DatabaseConnection, sql: impl Into<String>) {
    db.execute_unprepared(&sql.into())
        .await
        .expect("migration fixture SQL");
}

async fn value<T>(db: &DatabaseConnection, sql: impl Into<String>) -> T
where
    T: TryGetable,
{
    db.query_one_raw(Statement::from_string(
        db.get_database_backend(),
        sql.into(),
    ))
    .await
    .expect("migration assertion query")
    .expect("migration assertion row")
    .try_get_by_index(0)
    .expect("migration assertion value")
}

async fn drop_postgres_database(base: &str, database_name: &str) {
    let (without_query, query) = match base.split_once('?') {
        Some((head, tail)) => (head, format!("?{tail}")),
        None => (base, String::new()),
    };
    let prefix = without_query
        .rsplit_once('/')
        .expect("DATABASE_URL database segment")
        .0;
    let admin = Database::connect(format!("{prefix}/postgres{query}"))
        .await
        .expect("reconnect PostgreSQL admin database");
    admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS {database_name}"))
        .await
        .expect("drop PostgreSQL migration database");
    admin.close().await.expect("close PostgreSQL admin pool");
}

async fn with_database<F>(name: &str, test: F)
where
    F: for<'a> FnOnce(&'a DatabaseConnection) -> futures::future::BoxFuture<'a, ()>,
{
    let (db, sqlite_dir, postgres_name, postgres_base) = if super::super::require_sqlite_backend() {
        let dir = tempfile::tempdir().expect("create migration test directory");
        let path = dir.path().join(format!("{name}.sqlite3"));
        let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .expect("connect migration SQLite database");
        (db, Some(dir), None, None)
    } else {
        assert!(
            super::super::require_postgres_backend(),
            "migration tests require SQLite or PostgreSQL"
        );
        let base = std::env::var("DATABASE_URL").expect("PostgreSQL DATABASE_URL");
        let (without_query, query) = match base.split_once('?') {
            Some((head, tail)) => (head, format!("?{tail}")),
            None => (base.as_str(), String::new()),
        };
        let prefix = without_query
            .rsplit_once('/')
            .expect("DATABASE_URL database segment")
            .0;
        let suffix = DATABASE_SEQUENCE.fetch_add(1, Ordering::SeqCst);
        let database_name = format!(
            "yorishiro_mig_upgrade_{}_{}_{}",
            std::process::id(),
            suffix,
            name
        );
        let admin_url = format!("{prefix}/postgres{query}");
        let target_url = format!("{prefix}/{database_name}{query}");
        let admin = Database::connect(&admin_url)
            .await
            .expect("connect PostgreSQL admin database");
        admin
            .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
            .await
            .expect("create unique PostgreSQL migration database");
        drop(admin);
        let db = match Database::connect(&target_url).await {
            Ok(db) => db,
            Err(error) => {
                drop_postgres_database(&base, &database_name).await;
                panic!("connect unique PostgreSQL migration database: {error}");
            }
        };
        (db, None, Some(database_name), Some(base))
    };

    let outcome = AssertUnwindSafe(test(&db)).catch_unwind().await;
    let _ = db.close().await;

    if let (Some(database_name), Some(base)) = (postgres_name, postgres_base) {
        drop_postgres_database(&base, &database_name).await;
    }
    drop(sqlite_dir);

    if let Err(payload) = outcome {
        std::panic::resume_unwind(payload);
    }
}

async fn seed_initial_rows(db: &DatabaseConnection) {
    let tenant = id(db, TENANT);
    let user = id(db, USER);
    let workspace = id(db, WORKSPACE);
    let schema = id(db, SCHEMA);
    let entity = id(db, ENTITY);
    execute(
        db,
        format!(
            "INSERT INTO tenant_tenants (id, name) VALUES ({tenant}, 'upgrade tenant');
             INSERT INTO user_users (id, email, password_hash) VALUES ({user}, 'upgrade@example.test', 'hash');
             INSERT INTO workspace_workspaces (id, tenant_id, name, status) VALUES ({workspace}, {tenant}, 'upgrade workspace', 'active');
             INSERT INTO schema_schemas (id, tenant_id, workspace_id, name, version, definition, origin_status) VALUES ({schema}, {tenant}, {workspace}, 'upgrade schema', 1, '{{}}', 'linked');
             UPDATE workspace_workspaces SET schema_id = {schema} WHERE id = {workspace};
             INSERT INTO entity_entities (id, workspace_id, schema_id, schema_version, entity_type, data, created_by) VALUES ({entity}, {workspace}, {schema}, 1, 'document', '{{\"title\":\"survives\"}}', {user});"
        ),
    )
    .await;
}

async fn assert_table_exists(db: &DatabaseConnection, table: &str) {
    let found: i64 = match db.get_database_backend() {
        DbBackend::Sqlite => {
            value(
                db,
                format!(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '{table}'"
                ),
            )
            .await
        }
        DbBackend::Postgres => {
            value(
                db,
                format!(
                    "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'public' AND table_name = '{table}'"
                ),
            )
            .await
        }
        backend => panic!("unsupported migration test backend: {backend:?}"),
    };
    assert_eq!(found, 1, "expected table {table}");
}

async fn assert_index_exists(db: &DatabaseConnection, index: &str) {
    let found: i64 =
        match db.get_database_backend() {
            DbBackend::Sqlite => value(
                db,
                format!(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = '{index}'"
                ),
            )
            .await,
            DbBackend::Postgres => {
                value(
                    db,
                    format!("SELECT COUNT(*) FROM pg_indexes WHERE indexname = '{index}'"),
                )
                .await
            }
            backend => panic!("unsupported migration test backend: {backend:?}"),
        };
    assert_eq!(found, 1, "expected index {index}");
}

async fn assert_rejected(db: &DatabaseConnection, sql: &str) {
    assert!(
        db.execute_unprepared(sql).await.is_err(),
        "expected SQL to be rejected: {sql}"
    );
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn upgrade_000001_preserves_embeddings_and_creates_width_tables() {
    with_database("upgrade_000001", |db| Box::pin(async move {
        Migrator::up(db, Some(1)).await.expect("initial migration");
        seed_initial_rows(db).await;
        let entity = id(db, ENTITY);
        execute(
            db,
            match db.get_database_backend() {
                DbBackend::Sqlite => format!(
                    "INSERT INTO entity_embeddings (entity_id, embedding) VALUES ({entity}, zeroblob(16))"
                ),
                DbBackend::Postgres => format!(
                    "INSERT INTO entity_embeddings (entity_id, embedding) VALUES ({entity}, array_fill(0::real, ARRAY[768])::vector(768))"
                ),
                backend => panic!("unsupported migration test backend: {backend:?}"),
            },
        )
        .await;
        Migrator::up(db, Some(2)).await.expect("embedding migration");

        assert_table_exists(db, "entity_embeddings_768").await;
        assert_table_exists(db, "entity_embeddings_1024").await;
        assert_table_exists(db, "entity_embeddings_1536").await;
        assert_eq!(
            value::<i64>(
                db,
                format!("SELECT COUNT(*) FROM entity_entities WHERE id = {entity}"),
            )
            .await,
            1
        );
        assert_eq!(
            value::<i64>(
                db,
                format!("SELECT COUNT(*) FROM entity_embeddings_768 WHERE entity_id = {entity}"),
            )
            .await,
            1
        );
        let old_table_count: i64 = value(
            db,
            match db.get_database_backend() {
                DbBackend::Sqlite => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'entity_embeddings'"
                }
                DbBackend::Postgres => {
                    "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'entity_embeddings'"
                }
                backend => panic!("unsupported migration test backend: {backend:?}"),
            },
        )
        .await;
        assert_eq!(old_table_count, 0);
        match db.get_database_backend() {
            DbBackend::Sqlite => {
                let column_type: String = value(
                    db,
                    "SELECT type FROM pragma_table_info('entity_embeddings_768') WHERE name = 'embedding'",
                )
                .await;
                assert_eq!(column_type.to_ascii_lowercase(), "blob");
            }
            DbBackend::Postgres => {
                assert_index_exists(db, "idx_entity_embeddings_768_hnsw").await;
                let udt: String = value(
                    db,
                    "SELECT udt_name FROM information_schema.columns WHERE table_name = 'entity_embeddings_768' AND column_name = 'embedding'",
                )
                .await;
                assert_eq!(udt, "vector");
            }
            backend => panic!("unsupported migration test backend: {backend:?}"),
        }
    }))
    .await;
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn upgrade_000002_preserves_memberships_and_adds_user_index() {
    with_database("upgrade_000002", |db| Box::pin(async move {
        Migrator::up(db, Some(2)).await.expect("initial and embedding migrations");
        let tenant = id(db, TENANT);
        let user = id(db, USER);
        let membership = id(db, MEMBERSHIP);
        seed_initial_rows(db).await;
        execute(
            db,
            format!(
                "INSERT INTO tenant_memberships (id, tenant_id, user_id, role) VALUES ({membership}, {tenant}, {user}, 'owner')"
            ),
        )
        .await;
        Migrator::up(db, Some(3)).await.expect("membership index migration");
        assert_eq!(
            value::<i64>(
                db,
                format!("SELECT COUNT(*) FROM tenant_memberships WHERE id = {membership}"),
            )
            .await,
            1
        );
        assert_index_exists(db, "idx_tenant_memberships_user_id").await;
    }))
    .await;
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn upgrade_000003_preserves_schemas_and_adds_nullable_origin_timestamp() {
    with_database("upgrade_000003", |db| Box::pin(async move {
        Migrator::up(db, Some(3)).await.expect("first two migrations");
        seed_initial_rows(db).await;
        Migrator::up(db, Some(4)).await.expect("schema timestamp migration");
        let schema = id(db, SCHEMA);
        assert_eq!(
            value::<i64>(
                db,
                format!("SELECT COUNT(*) FROM schema_schemas WHERE id = {schema}"),
            )
            .await,
            1
        );
        assert_eq!(
            value::<i64>(
                db,
                format!("SELECT COUNT(*) FROM schema_schemas WHERE id = {schema} AND origin_updated_at IS NULL"),
            )
            .await,
            1
        );
        let nullable: String = match db.get_database_backend() {
            DbBackend::Sqlite => value(
                db,
                "SELECT CASE WHEN "
                    .to_owned()
                    + "\"notnull\" = 0 THEN 'YES' ELSE 'NO' END FROM pragma_table_info('schema_schemas') WHERE name = 'origin_updated_at'",
            )
            .await,
            DbBackend::Postgres => value(
                db,
                "SELECT is_nullable FROM information_schema.columns WHERE table_name = 'schema_schemas' AND column_name = 'origin_updated_at'",
            )
            .await,
            backend => panic!("unsupported migration test backend: {backend:?}"),
        };
        assert_eq!(nullable, "YES");
    }))
    .await;
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn upgrade_000004_creates_credit_ledger_with_constraints_and_grant() {
    with_database("upgrade_000004", |db| Box::pin(async move {
        Migrator::up(db, Some(4)).await.expect("first three migrations");
        seed_initial_rows(db).await;
        Migrator::up(db, Some(5)).await.expect("credit ledger migration");
        let ledger = id(db, LEDGER);
        let workspace = id(db, WORKSPACE);
        execute(
            db,
            format!(
                "INSERT INTO compute_credit_ledger (id, workspace_id, amount, transaction_type) VALUES ({ledger}, {workspace}, 100, 'earn')"
            ),
        )
        .await;
        assert_eq!(
            value::<i64>(
                db,
                format!("SELECT amount FROM compute_credit_ledger WHERE id = {ledger}"),
            )
            .await,
            100
        );
        assert_rejected(
            db,
            &format!(
                "INSERT INTO compute_credit_ledger (id, workspace_id, amount, transaction_type) VALUES ({}, {}, 1, 'invalid')",
                id(db, "dddddddd-dddd-4ddd-8ddd-dddddddddddd"),
                workspace
            ),
        )
        .await;
        assert_index_exists(db, "compute_credit_ledger_workspace_id_created_at_idx").await;
        match db.get_database_backend() {
            DbBackend::Sqlite => {
                let default_value: Option<String> = value(
                    db,
                    "SELECT dflt_value FROM pragma_table_info('compute_credit_ledger') WHERE name = 'id'",
                )
                .await;
                assert_eq!(default_value, None);
            }
            DbBackend::Postgres => {
                let default_value: String = value(
                    db,
                    "SELECT column_default FROM information_schema.columns WHERE table_name = 'compute_credit_ledger' AND column_name = 'id'",
                )
                .await;
                assert!(default_value.contains("uuidv7()"));
                let granted: bool = value(
                    db,
                    "SELECT has_table_privilege('yorishiro_app', 'compute_credit_ledger', 'SELECT')",
                )
                .await;
                assert!(granted);
            }
            backend => panic!("unsupported migration test backend: {backend:?}"),
        }
    }))
    .await;
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn upgrade_000005_preserves_audit_rows_and_expands_action_check() {
    with_database("upgrade_000005", |db| Box::pin(async move {
        Migrator::up(db, Some(5)).await.expect("first four migrations");
        seed_initial_rows(db).await;
        let workspace = id(db, WORKSPACE);
        let tenant = id(db, TENANT);
        execute(
            db,
            format!(
                "INSERT INTO api_key_audit_log (id, workspace_id, tenant_id, action, detail) VALUES ({}, {workspace}, {tenant}, 'set_maintenance', '{{}}')",
                id(db, "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee")
            ),
        )
        .await;
        Migrator::up(db, Some(6)).await.expect("audit action migration");
        assert_eq!(
            value::<i64>(
                db,
                "SELECT COUNT(*) FROM api_key_audit_log WHERE action = 'set_maintenance'",
            )
            .await,
            1
        );
        execute(
            db,
            format!(
                "INSERT INTO api_key_audit_log (id, workspace_id, tenant_id, action, detail) VALUES ({}, {workspace}, {tenant}, 'fill_defaults', '{{}}')",
                id(db, "ffffffff-ffff-4fff-8fff-ffffffffffff")
            ),
        )
        .await;
        assert_rejected(
            db,
            &format!(
                "INSERT INTO api_key_audit_log (id, workspace_id, tenant_id, action, detail) VALUES ({}, {workspace}, {tenant}, 'not_an_action', '{{}}')",
                id(db, "12121212-1212-4121-8121-121212121212")
            ),
        )
        .await;
    }))
    .await;
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn upgrade_000006_creates_inference_jobs_with_defaults_and_status_check() {
    with_database("upgrade_000006", |db| Box::pin(async move {
        Migrator::up(db, Some(6)).await.expect("first five migrations");
        seed_initial_rows(db).await;
        Migrator::up(db, Some(7)).await.expect("inference jobs migration");
        let job = id(db, JOB);
        let workspace = id(db, WORKSPACE);
        execute(
            db,
            format!(
                "INSERT INTO inference_jobs (id, workspace_id, schema_name, status) VALUES ({job}, {workspace}, 'upgrade schema', 'queued')"
            ),
        )
        .await;
        assert_eq!(
            value::<i64>(
                db,
                format!("SELECT applied FROM inference_jobs WHERE id = {job}"),
            )
            .await,
            0
        );
        assert_eq!(
            value::<i64>(
                db,
                format!("SELECT skipped FROM inference_jobs WHERE id = {job}"),
            )
            .await,
            0
        );
        assert_rejected(
            db,
            &format!(
                "INSERT INTO inference_jobs (id, workspace_id, schema_name, status) VALUES ({}, {workspace}, 'upgrade schema', 'invalid')",
                id(db, "13131313-1313-4131-8131-131313131313")
            ),
        )
        .await;
        assert_index_exists(db, "inference_jobs_workspace_id_created_at_idx").await;
    }))
    .await;
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn upgrade_000007_creates_fork_history_and_integrity_objects() {
    with_database("upgrade_000007", |db| Box::pin(async move {
        Migrator::up(db, Some(7)).await.expect("first six migrations");
        seed_initial_rows(db).await;
        let tenant = id(db, TENANT);
        let source_workspace = id(db, WORKSPACE);
        let source_schema = id(db, SCHEMA);
        let target_workspace = id(db, TARGET_WORKSPACE);
        let target_schema = id(db, TARGET_SCHEMA);
        execute(
            db,
            format!(
                "INSERT INTO workspace_workspaces (id, tenant_id, name, status) VALUES ({target_workspace}, {tenant}, 'target workspace', 'active');
                 INSERT INTO schema_schemas (id, tenant_id, workspace_id, name, version, definition) VALUES ({target_schema}, {tenant}, {target_workspace}, 'upgrade schema', 1, '{{}}');
                 UPDATE workspace_workspaces SET schema_id = {target_schema} WHERE id = {target_workspace}"
            ),
        )
        .await;
        Migrator::up(db, None).await.expect("workspace fork migration");
        let fork = id(db, FORK);
        execute(
            db,
            format!(
                "INSERT INTO workspace_schema_forks (id, tenant_id, workspace_id, source_workspace_id, source_schema_id, source_schema_version, source_schema_name, fork_schema_id) VALUES ({fork}, {tenant}, {target_workspace}, {source_workspace}, {source_schema}, 1, 'upgrade schema', {target_schema})"
            ),
        )
        .await;
        execute(
            db,
            format!(
                "INSERT INTO workspace_schema_fork_heads (id, tenant_id, workspace_id, fork_id, schema_id) VALUES ({}, {tenant}, {target_workspace}, {fork}, {target_schema})",
                id(db, HEAD)
            ),
        )
        .await;
        assert_eq!(
            value::<i64>(
                db,
                format!("SELECT COUNT(*) FROM workspace_schema_forks WHERE id = {fork}"),
            )
            .await,
            1
        );
        match db.get_database_backend() {
            DbBackend::Sqlite => {
                assert_eq!(
                    value::<i64>(
                        db,
                        format!("SELECT CASE WHEN customized THEN 1 ELSE 0 END FROM workspace_schema_forks WHERE id = {fork}"),
                    )
                    .await,
                    0
                );
                assert_eq!(
                    value::<Option<i64>>(
                        db,
                        "SELECT MAX(source_schema_version) FROM workspace_schema_forks",
                    )
                    .await,
                    Some(1)
                );
            }
            DbBackend::Postgres => {
                assert_eq!(
                    value::<i32>(
                        db,
                        format!("SELECT CASE WHEN customized THEN 1 ELSE 0 END FROM workspace_schema_forks WHERE id = {fork}"),
                    )
                    .await,
                    0
                );
                assert_eq!(
                    value::<Option<i32>>(
                        db,
                        "SELECT MAX(source_schema_version) FROM workspace_schema_forks",
                    )
                    .await,
                    Some(1)
                );
            }
            backend => panic!("unsupported migration test backend: {backend:?}"),
        }
        assert_index_exists(db, "workspace_schema_forks_workspace_source_name_key").await;
        assert_index_exists(db, "workspace_schema_fork_heads_fork_schema_key").await;
        assert_rejected(
            db,
            &format!(
                "INSERT INTO workspace_schema_forks (id, tenant_id, workspace_id, source_workspace_id, source_schema_id, source_schema_version, source_schema_name, fork_schema_id) VALUES ({}, {tenant}, {target_workspace}, {source_workspace}, {source_schema}, 99, 'upgrade schema', {target_schema})",
                id(db, "14141414-1414-4141-8141-141414141414")
            ),
        )
        .await;
        match db.get_database_backend() {
            DbBackend::Sqlite => {
                let trigger_count: i64 = value(
                    db,
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name LIKE 'workspace_schema_fork%'",
                )
                .await;
                assert_eq!(trigger_count, 4);
            }
            DbBackend::Postgres => {
                let rls: bool = value(
                    db,
                    "SELECT relrowsecurity FROM pg_class WHERE oid = 'workspace_schema_forks'::regclass",
                )
                .await;
                assert!(rls);
                let granted: bool = value(
                    db,
                    "SELECT has_table_privilege('yorishiro_app', 'workspace_schema_forks', 'SELECT')",
                )
                .await;
                assert!(granted);
                let function_count: i64 = value(
                    db,
                    "SELECT COUNT(*) FROM pg_proc WHERE proname IN ('workspace_schema_fork_source_by_id', 'workspace_schema_fork_latest_source', 'workspace_schema_fork_edges')",
                )
                .await;
                assert_eq!(function_count, 3);
            }
            backend => panic!("unsupported migration test backend: {backend:?}"),
        }
    }))
    .await;
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn incremental_migration_rollbacks_are_separate_from_fresh_upgrades() {
    with_database("upgrade_rollbacks", |db| Box::pin(async move {
        Migrator::up(db, None).await.expect("all migrations");
        for _ in 0..migration::Migrator::migrations().len() {
            Migrator::down(db, Some(1)).await.expect("one rollback step");
        }
        let tenant_table_count: i64 = match db.get_database_backend() {
            DbBackend::Sqlite => {
                value(db, "SELECT COUNT(*) FROM sqlite_master WHERE name = 'tenant_tenants'").await
            }
            DbBackend::Postgres => {
                value(
                    db,
                    "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'tenant_tenants'",
                )
                .await
            }
            backend => panic!("unsupported migration test backend: {backend:?}"),
        };
        assert_eq!(tenant_table_count, 0, "rollback should remove the initial schema");
    }))
    .await;
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn queue_starvation_index_matches_admission_predicate() {
    with_database("queue_starvation_index", |db| Box::pin(async move {
        let through_starvation =
            migrations_through("m20261001_000014_queue_lifecycle_starvation_index");
        Migrator::up(db, Some(through_starvation))
            .await
            .expect("migrations through starvation index");
        assert_index_exists(db, "queue_job_lifecycles_starvation_idx").await;
        let definition: String = match db.get_database_backend() {
            DbBackend::Sqlite => {
                value(
                    db,
                    "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'queue_job_lifecycles_starvation_idx'",
                )
                .await
            }
            DbBackend::Postgres => {
                value(
                    db,
                    "SELECT indexdef FROM pg_indexes WHERE indexname = 'queue_job_lifecycles_starvation_idx'",
                )
                .await
            }
            backend => panic!("unsupported migration test backend: {backend:?}"),
        };
        for column in ["status", "worker_class", "enqueue_at"] {
            assert!(
                definition.contains(column),
                "index definition missing {column}: {definition}"
            );
        }
        Migrator::down(db, Some(1))
            .await
            .expect("roll back starvation index migration");
        let absent: i64 = match db.get_database_backend() {
            DbBackend::Sqlite => value(
                db,
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'queue_job_lifecycles_starvation_idx'",
            )
            .await,
            DbBackend::Postgres => value(
                db,
                "SELECT COUNT(*) FROM pg_indexes WHERE indexname = 'queue_job_lifecycles_starvation_idx'",
            )
            .await,
            backend => panic!("unsupported migration test backend: {backend:?}"),
        };
        assert_eq!(absent, 0, "rollback must remove starvation index");
        Migrator::up(db, Some(through_starvation))
            .await
            .expect("reapply starvation index migration");
        assert_index_exists(db, "queue_job_lifecycles_starvation_idx").await;
    }))
    .await;
}

#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn startup_admission_upgrade_preserves_ordinary_active_duplicates() {
    with_database("startup_admission_upgrade", |db| Box::pin(async move {
        let through_requests = migrations_through("m20261005_000017_embedding_tables_tenant_read");
        Migrator::up(db, Some(through_requests))
            .await
            .expect("migrations before the embedding queue state");
        seed_initial_rows(db).await;
        let workspace = id(db, WORKSPACE);
        execute(
            db,
            format!(
                "INSERT INTO queue_job_lifecycles (id, job_name, worker_class, workspace_id, status, enqueue_at) VALUES ({}, 'reindex', 'shared', {workspace}, 'queued', CURRENT_TIMESTAMP), ({}, 'reindex', 'shared', {workspace}, 'queued', CURRENT_TIMESTAMP)",
                id(db, "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
                id(db, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb")
            ),
        )
        .await;
        Migrator::up(db, None)
            .await
            .expect("startup admission migration must not reject ordinary duplicates");
        let count: i64 = value(
            db,
            format!("SELECT COUNT(*) FROM queue_job_lifecycles WHERE job_name = 'reindex' AND workspace_id = {workspace} AND status = 'queued'"),
        )
        .await;
        assert_eq!(count, 2);
        assert_index_exists(db, "queue_startup_reindex_active_workspace_idx").await;
    }))
    .await;
}

/// The embedding-sync token column is present, non-empty, and rotates on every write.
#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn embedding_sync_token_initializes_on_insert_and_rotates_on_update() {
    with_database("embedding_sync_token", |db| Box::pin(async move {
        let through_token =
            migrations_through("m20261005_000016_add_entity_embedding_sync_token");
        // Apply through the migration before the token column.
        Migrator::up(db, Some(through_token - 1))
            .await
            .expect("migrations before token");

        let tenant = id(db, TENANT);
        let user = id(db, USER);
        let workspace = id(db, WORKSPACE);
        let schema = id(db, SCHEMA);
        let entity = id(db, ENTITY);

        // Create base tables needed for the entity (same order as seed_initial_rows).
        for sql in [
            format!("INSERT INTO tenant_tenants (id, name) VALUES ({tenant}, 'token-test')"),
            format!("INSERT INTO user_users (id, email, password_hash) VALUES ({user}, 'token@example.test', 'hash')"),
            format!("INSERT INTO workspace_workspaces (id, tenant_id, name, status) VALUES ({workspace}, {tenant}, 'token-workspace', 'active')"),
            format!("INSERT INTO schema_schemas (id, tenant_id, workspace_id, name, version, definition) VALUES ({schema}, {tenant}, {workspace}, 'token-schema', 1, '{{}}')"),
            format!("UPDATE workspace_workspaces SET schema_id = {schema} WHERE id = {workspace}"),
        ] {
            execute(db, sql).await;
        }

        // Apply the token migration.
        Migrator::up(db, Some(through_token))
            .await
            .expect("apply token migration");

        // After the migration, the column exists and is non-null.
        match db.get_database_backend() {
            DbBackend::Sqlite => {
                let col_exists: i64 = value(
                    db,
                    "SELECT COUNT(*) FROM pragma_table_info('entity_entities') WHERE name = 'embedding_sync_token'",
                )
                .await;
                assert_eq!(col_exists, 1, "token column must exist on SQLite");
            }
            DbBackend::Postgres => {
                let col_exists: i64 = value(
                    db,
                    "SELECT COUNT(*) FROM information_schema.columns WHERE table_name = 'entity_entities' AND column_name = 'embedding_sync_token'",
                )
                .await;
                assert_eq!(col_exists, 1, "token column must exist on Postgres");
            }
            backend => panic!("unsupported backend: {backend:?}"),
        }

        // Insert a new entity: the triggers must generate a non-empty token.
        execute(
            db,
            format!(
                "INSERT INTO entity_entities (id, workspace_id, schema_id, schema_version, entity_type, data, created_by) VALUES ({entity}, {workspace}, {schema}, 1, 'doc', '{{}}', {user})"
            ),
        )
        .await;

        // Verify the token is non-empty and non-default.
        let token: String = value(
            db,
            format!("SELECT embedding_sync_token FROM entity_entities WHERE id = {entity}"),
        )
        .await;
        assert!(
            !token.is_empty(),
            "new entity must have a non-empty token"
        );
        if db.get_database_backend() == DbBackend::Postgres {
            assert_ne!(
                token, "00000000-0000-0000-0000-000000000000",
                "token must not be the default zero UUID"
            );
        }

        // Update the entity: the UPDATE trigger must rotate the token.
        // On SQLite the AFTER UPDATE trigger with INSERT OR REPLACE handles
        // raw SQL UPDATEs; on PostgreSQL the BEFORE UPDATE trigger does.
        let token_before: String = value(
            db,
            format!("SELECT embedding_sync_token FROM entity_entities WHERE id = {entity}"),
        )
        .await;

        execute(
            db,
            format!(
                "UPDATE entity_entities SET data = '{{\"updated\": true}}' WHERE id = {entity}"
            ),
        )
        .await;

        let token_after: String = value(
            db,
            format!("SELECT embedding_sync_token FROM entity_entities WHERE id = {entity}"),
        )
        .await;
        assert_ne!(
            token_after, token_before,
            "UPDATE must rotate the token"
        );
    }))
    .await;
}

/// Roll back the token migration and verify triggers/columns are cleaned up.
#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn embedding_sync_token_migration_down_removes_column_and_triggers() {
    with_database("embedding_sync_token_down", |db| Box::pin(async move {
        let through_token =
            migrations_through("m20261005_000016_add_entity_embedding_sync_token");

        // Apply through token migration.
        Migrator::up(db, Some(through_token))
            .await
            .expect("apply token migration");

        // Roll back exactly through this migration.
        Migrator::down(db, Some(1))
            .await
            .expect("roll back token migration");

        // Column must be gone.
        match db.get_database_backend() {
            DbBackend::Sqlite => {
                let col_exists: i64 = value(
                    db,
                    "SELECT COUNT(*) FROM pragma_table_info('entity_entities') WHERE name = 'embedding_sync_token'",
                )
                .await;
                assert_eq!(col_exists, 0, "token column must be dropped on SQLite");
            }
            DbBackend::Postgres => {
                let col_exists: i64 = value(
                    db,
                    "SELECT COUNT(*) FROM information_schema.columns WHERE table_name = 'entity_entities' AND column_name = 'embedding_sync_token'",
                )
                .await;
                assert_eq!(col_exists, 0, "token column must be dropped on Postgres");
            }
            backend => panic!("unsupported backend: {backend:?}"),
        }

        if db.get_database_backend() == DbBackend::Sqlite {
            let matching: i64 = value(
                db,
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'entity_fts_au' AND sql LIKE '%AFTER UPDATE ON entity_entities%'",
            )
            .await;
            assert_eq!(matching, 1, "rollback must restore the original entity_fts_au trigger");
        }
        // Re-apply: must succeed cleanly because the triggers are created fresh.
        Migrator::up(db, Some(through_token))
            .await
            .expect("reapply token migration");
        if db.get_database_backend() == DbBackend::Sqlite {
            let matching: i64 = value(
                db,
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'entity_fts_au' AND sql LIKE '%AFTER UPDATE OF data ON entity_entities%'",
            )
            .await;
            assert_eq!(matching, 1, "reapply must narrow entity_fts_au to data updates");
        }

        // Column must exist again.
        match db.get_database_backend() {
            DbBackend::Sqlite => {
                let col_exists: i64 = value(
                    db,
                    "SELECT COUNT(*) FROM pragma_table_info('entity_entities') WHERE name = 'embedding_sync_token'",
                )
                .await;
                assert_eq!(col_exists, 1, "token column must exist after reapply");
            }
            DbBackend::Postgres => {
                let col_exists: i64 = value(
                    db,
                    "SELECT COUNT(*) FROM information_schema.columns WHERE table_name = 'entity_entities' AND column_name = 'embedding_sync_token'",
                )
                .await;
                assert_eq!(col_exists, 1, "token column must exist after reapply");
            }
            backend => panic!("unsupported backend: {backend:?}"),
        }
    }))
    .await;
}

/// The tenant-scoped role reads the embedding tables only after the grant migration, and loses the grant on rollback.
#[tokio::test]
#[serial(postgres_cluster)]
#[serial(process_environment)]
async fn embedding_tables_tenant_read_grant_is_applied_and_rolled_back() {
    with_database("embedding_tables_tenant_read", |db| Box::pin(async move {
        if db.get_database_backend() != DbBackend::Postgres {
            return;
        }
        let through_grant = migrations_through("m20261005_000017_embedding_tables_tenant_read");
        let can_select = "SELECT COUNT(*) FROM (SELECT 1 WHERE has_table_privilege('yorishiro_app', 'entity_embeddings_768', 'SELECT')) t";

        Migrator::up(db, Some(through_grant - 1))
            .await
            .expect("migrations before the grant");
        assert_eq!(value::<i64>(db, can_select).await, 0, "no grant before the migration");

        // `Some(n)` is a number of steps, not a target version, so exactly one step applies the grant migration whatever follows it.
        Migrator::up(db, Some(1))
            .await
            .expect("apply the grant migration");
        assert_eq!(value::<i64>(db, can_select).await, 1, "SELECT granted");
        let policies: i64 = value(
            db,
            "SELECT COUNT(*) FROM pg_policies WHERE tablename LIKE 'entity_embeddings_%' AND cmd = 'SELECT'",
        )
        .await;
        assert_eq!(policies, 3, "one SELECT policy per width table");

        Migrator::down(db, Some(1))
            .await
            .expect("roll back the grant migration");
        assert_eq!(value::<i64>(db, can_select).await, 0, "grant revoked on rollback");
    }))
    .await;
}
