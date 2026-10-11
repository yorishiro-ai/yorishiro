mod dispatch;
mod lifecycle;
mod queue;

mod embedding_sync;
mod query_embedding;
mod queue_routing;
mod reindex;
mod startup_reindex;
mod tag_discovery;

use loco_rs::app::AppContext;
use loco_rs::environment::Environment;
use sea_orm::Database;

/// An application context over an in-memory SQLite database, for tests that need `AppContext` but no HTTP server or queue.
pub(crate) async fn test_context() -> AppContext {
    let config = crate::sqlite_fixture_config("sqlite::memory:");
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("connect test database");
    let ctx = AppContext::builder(Environment::Test, db, config.clone()).build();
    ctx.shared_store.insert(
        config
            .settings::<yorishiro::data::settings::Settings>()
            .expect("test settings"),
    );
    ctx
}
