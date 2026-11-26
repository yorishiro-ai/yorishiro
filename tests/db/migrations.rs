use futures::FutureExt;
use loco_rs::app::Hooks;
use migration::{Migrator, MigratorTrait};
use sea_orm::Database;
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
