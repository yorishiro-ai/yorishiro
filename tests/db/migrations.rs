use futures::FutureExt;
use loco_rs::app::Hooks;
use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};
use yorishiro::App;

async fn race_migrations(url: &str) {
    let options = || {
        let mut options = sea_orm::ConnectOptions::new(url);
        options.min_connections(1).max_connections(1);
        options
    };
    let first = Database::connect(options()).await.unwrap();
    let second = Database::connect(options()).await.unwrap();
    let barrier = tokio::sync::Barrier::new(2);
    let migrate = async |conn: &sea_orm::DatabaseConnection| {
        barrier.wait().await;
        Migrator::up(conn, None).await
    };
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(120), async {
        tokio::join!(migrate(&first), migrate(&second))
    })
    .await;
    let versions = Migrator::get_applied_migrations(&first).await;
    first.close().await.unwrap();
    second.close().await.unwrap();
    let (first, second) = outcome.expect("migrations must not wait for an extra pool connection");
    first.unwrap();
    second.unwrap();
    assert_eq!(versions.unwrap().len(), Migrator::migrations().len());
}

#[tokio::test]
async fn concurrent_first_postgres_migrations_use_one_connection_each() {
    if !crate::require_postgres_backend() {
        return;
    }
    let _environment = crate::EnvironmentRead::enter();
    let config = App::load_config(&loco_rs::environment::Environment::Test)
        .await
        .unwrap();
    let database = loco_rs::testing::db::init_test_db_creation(&config.database.uri).unwrap();
    database.init_db().await;
    let outcome = std::panic::AssertUnwindSafe(race_migrations(database.get_connection_str()))
        .catch_unwind()
        .await;
    database.cleanup_db();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn concurrent_first_sqlite_migrations_serialize_before_reading_versions() {
    if !crate::require_sqlite_backend() {
        return;
    }
    yorishiro::db::register_sqlite_extensions();
    let directory = tempfile::tempdir().unwrap();
    let uri = format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("migrations.sqlite").display()
    );
    race_migrations(&uri).await;
}

#[tokio::test]
async fn cli_create_app_keeps_migrations_on_the_serving_in_memory_database() {
    if !crate::require_sqlite_backend() {
        return;
    }
    let _environment = crate::EnvironmentRead::enter();
    yorishiro::db::register_sqlite_extensions();
    let environment = loco_rs::environment::Environment::Test;
    let mut config = App::load_config(&environment).await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    config.database.uri = "sqlite::memory:".into();
    config.database.auto_migrate = true;
    config.database.min_connections = 2;
    config.database.max_connections = 2;
    if let Some(loco_rs::config::QueueConfig::Sqlite(queue)) = config.queue.as_mut() {
        queue.uri = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("queue.sqlite").display()
        );
    }
    // This is the CLI start entry point, which does not call Hooks::boot.
    let boot = loco_rs::boot::create_app::<App, Migrator>(
        loco_rs::boot::StartMode::ServerOnly,
        &environment,
        config,
    )
    .await
    .unwrap();
    let query = boot
        .app_context
        .db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT COUNT(*) AS n FROM schema_schemas",
        ))
        .await;
    crate::requests::close_app_pools(&boot.app_context).await;
    assert!(
        query.unwrap().is_some(),
        "the serving database retains its migrated tables"
    );
}

#[tokio::test]
async fn iam_constraints_reject_cross_tenant_and_nonmember_attachments() {
    if !crate::require_sqlite_backend() {
        return;
    }
    yorishiro::db::register_sqlite_extensions();
    let db = Database::connect("sqlite::memory:").await.unwrap();
    db.execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .unwrap();
    Migrator::up(&db, None).await.unwrap();
    for sql in [
        "INSERT INTO tenant_tenants (id, name) VALUES (X'01', 'one'), (X'02', 'two')",
        "INSERT INTO user_users (id, tenant_id, user_name, password_hash) VALUES (X'11', X'01', 'user', 'hash')",
        "INSERT INTO user_users (id, email, password_hash) VALUES (X'12', 'root@example.com', 'hash')",
        "INSERT INTO tenant_memberships (id, tenant_id, user_id, role) VALUES (X'61', X'01', X'11', 'member')",
        "INSERT INTO iam_groups (id, tenant_id, name) VALUES (X'21', X'01', 'group'), (X'22', X'02', 'group')",
        "INSERT INTO iam_policies (id, tenant_id, name, document) VALUES (X'31', X'01', 'policy', '{}'), (X'32', X'02', 'policy', '{}')",
        "INSERT INTO iam_group_memberships (id, tenant_id, group_id, user_id) VALUES (X'41', X'01', X'21', X'11')",
        "INSERT INTO iam_user_policy_attachments (id, tenant_id, user_id, policy_id) VALUES (X'42', X'01', X'11', X'31')",
        "INSERT INTO iam_group_policy_attachments (id, tenant_id, group_id, policy_id) VALUES (X'43', X'01', X'21', X'31')",
    ] {
        db.execute_unprepared(sql).await.unwrap();
    }
    for sql in [
        "INSERT INTO iam_group_memberships (id, tenant_id, group_id, user_id) VALUES (X'51', X'01', X'22', X'11')",
        "INSERT INTO iam_group_memberships (id, tenant_id, group_id, user_id) VALUES (X'52', X'01', X'21', X'12')",
        "INSERT INTO iam_user_policy_attachments (id, tenant_id, user_id, policy_id) VALUES (X'53', X'01', X'11', X'32')",
        "INSERT INTO iam_user_policy_attachments (id, tenant_id, user_id, policy_id) VALUES (X'54', X'01', X'12', X'31')",
        "INSERT INTO iam_group_policy_attachments (id, tenant_id, group_id, policy_id) VALUES (X'55', X'01', X'21', X'32')",
        "INSERT INTO iam_group_memberships (id, tenant_id, group_id, user_id) VALUES (X'56', X'01', X'21', X'11')",
        "INSERT INTO iam_policies (id, tenant_id, name, document) VALUES (X'57', X'01', 'policy', '{}')",
    ] {
        assert!(db.execute_unprepared(sql).await.is_err(), "accepted: {sql}");
    }
    db.close().await.unwrap();
}

#[tokio::test]
async fn postgres_iam_rls_grants_and_tenant_foreign_keys() {
    if !crate::require_postgres_backend() {
        return;
    }
    let _environment = crate::EnvironmentRead::enter();
    let config = App::load_config(&loco_rs::environment::Environment::Test)
        .await
        .unwrap();
    let database = loco_rs::testing::db::init_test_db_creation(&config.database.uri).unwrap();
    database.init_db().await;
    let mut options = sea_orm::ConnectOptions::new(database.get_connection_str());
    options.min_connections(1).max_connections(1);
    let db = Database::connect(options).await.unwrap();
    let outcome = std::panic::AssertUnwindSafe(async {
        Migrator::up(&db, None).await.unwrap();
        for sql in [
            "INSERT INTO tenant_tenants (id, name) VALUES ('00000000-0000-0000-0000-000000000001', 'one'), ('00000000-0000-0000-0000-000000000002', 'two')",
            "INSERT INTO user_users (id, tenant_id, user_name, password_hash) VALUES ('00000000-0000-0000-0000-000000000011', '00000000-0000-0000-0000-000000000001', 'user', 'hash'), ('00000000-0000-0000-0000-000000000012', '00000000-0000-0000-0000-000000000002', 'user', 'hash')",
            "INSERT INTO user_users (id, email, password_hash) VALUES ('00000000-0000-0000-0000-000000000013', 'root@example.com', 'hash')",
            "INSERT INTO tenant_memberships (tenant_id, user_id, role) VALUES ('00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000011', 'member'), ('00000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-000000000012', 'member')",
            "INSERT INTO iam_groups (id, tenant_id, name) VALUES ('00000000-0000-0000-0000-000000000021', '00000000-0000-0000-0000-000000000001', 'group'), ('00000000-0000-0000-0000-000000000022', '00000000-0000-0000-0000-000000000002', 'group')",
            "INSERT INTO iam_policies (id, tenant_id, name, document) VALUES ('00000000-0000-0000-0000-000000000031', '00000000-0000-0000-0000-000000000001', 'policy', '{}'), ('00000000-0000-0000-0000-000000000032', '00000000-0000-0000-0000-000000000002', 'policy', '{}')",
            "INSERT INTO iam_group_memberships (tenant_id, group_id, user_id) VALUES ('00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000021', '00000000-0000-0000-0000-000000000011'), ('00000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-000000000022', '00000000-0000-0000-0000-000000000012')",
            "INSERT INTO iam_user_policy_attachments (tenant_id, user_id, policy_id) VALUES ('00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000011', '00000000-0000-0000-0000-000000000031'), ('00000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-000000000012', '00000000-0000-0000-0000-000000000032')",
            "INSERT INTO iam_group_policy_attachments (tenant_id, group_id, policy_id) VALUES ('00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000021', '00000000-0000-0000-0000-000000000031'), ('00000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-000000000022', '00000000-0000-0000-0000-000000000032')",
            "SET ROLE yorishiro_app",
        ] {
            db.execute_unprepared(sql).await.unwrap();
        }
        let role = db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres,
            "SELECT current_user = 'yorishiro_app' AND NOT rolsuper AND NOT rolbypassrls AS safe FROM pg_roles WHERE rolname = current_user"))
            .await.unwrap().unwrap();
        assert!(role.try_get::<bool>("", "safe").unwrap());
        assert!(db.execute_unprepared("SELECT * FROM iam_groups").await.is_err());
        db.execute_unprepared("SELECT set_config('app.current_tenant', '00000000-0000-0000-0000-000000000001', false)").await.unwrap();
        for table in ["iam_groups", "iam_policies", "iam_group_memberships", "iam_user_policy_attachments", "iam_group_policy_attachments"] {
            let row = db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres,
                format!("SELECT count(*) AS n, has_table_privilege(current_user, '{table}', 'SELECT,INSERT,UPDATE,DELETE') AS granted FROM {table}")))
                .await.unwrap().unwrap();
            assert_eq!(row.try_get::<i64>("", "n").unwrap(), 1, "{table}");
            assert!(row.try_get::<bool>("", "granted").unwrap(), "{table}");
        }
        for sql in [
            "INSERT INTO iam_groups (tenant_id, name) VALUES ('00000000-0000-0000-0000-000000000002', 'forbidden')",
            "INSERT INTO iam_group_memberships (tenant_id, group_id, user_id) VALUES ('00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000022', '00000000-0000-0000-0000-000000000011')",
            "INSERT INTO iam_group_memberships (tenant_id, group_id, user_id) VALUES ('00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000021', '00000000-0000-0000-0000-000000000012')",
            "INSERT INTO iam_group_memberships (tenant_id, group_id, user_id) VALUES ('00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000021', '00000000-0000-0000-0000-000000000013')",
            "INSERT INTO iam_user_policy_attachments (tenant_id, user_id, policy_id) VALUES ('00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000011', '00000000-0000-0000-0000-000000000032')",
            "INSERT INTO iam_user_policy_attachments (tenant_id, user_id, policy_id) VALUES ('00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000013', '00000000-0000-0000-0000-000000000031')",
            "INSERT INTO iam_group_policy_attachments (tenant_id, group_id, policy_id) VALUES ('00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000021', '00000000-0000-0000-0000-000000000032')",
            "INSERT INTO iam_group_memberships (tenant_id, group_id, user_id) VALUES ('00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000021', '00000000-0000-0000-0000-000000000011')",
        ] {
            assert!(db.execute_unprepared(sql).await.is_err(), "accepted: {sql}");
        }
        db.execute_unprepared("SELECT set_config('app.current_tenant', '00000000-0000-0000-0000-000000000002', false)").await.unwrap();
        let row = db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, "SELECT name FROM iam_groups"))
            .await.unwrap().unwrap();
        assert_eq!(row.try_get::<String>("", "name").unwrap(), "group");
        let updated = db.execute_unprepared("UPDATE iam_groups SET name = 'forbidden' WHERE tenant_id = '00000000-0000-0000-0000-000000000001'").await.unwrap();
        assert_eq!(updated.rows_affected(), 0);
    }).catch_unwind().await;
    db.close().await.unwrap();
    database.cleanup_db();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
