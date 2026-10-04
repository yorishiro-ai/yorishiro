mod dispatch;
mod lifecycle;
mod queue;

mod embedding_sync;
#[cfg(feature = "enterprise")]
mod infer_fill;
mod queue_routing;
mod reindex;

use loco_rs::app::{AppContext, Hooks};
use loco_rs::environment::Environment;
use sea_orm::Database;
use yorishiro::app::App;

/// An application context over an in-memory SQLite database, for tests that need `AppContext` but no HTTP server or queue.
pub(crate) async fn test_context() -> AppContext {
    let config = App::load_config(&Environment::Any("test_sqlite".into()))
        .await
        .expect("load test configuration");
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("connect test database");
    AppContext::builder(Environment::Test, db, config).build()
}
