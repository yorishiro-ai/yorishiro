//! Binary entry point: registers the SQLite extensions, then hands over to the Loco CLI.

use loco_rs::cli;
use migration::Migrator;
use yorishiro::App;

#[tokio::main]
async fn main() -> loco_rs::Result<()> {
    // Register sqlite-vec before any SQLite connection opens: every CLI subcommand
    // (task, db, scheduler) and the test harness open ctx.db before App::boot runs,
    // so the registration must happen here as well in boot.
    yorishiro::db::register_sqlite_extensions();

    cli::main::<App, Migrator>().await
}
