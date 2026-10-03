use loco_rs::cli;
use migration::Migrator;
use yorishiro::app::App;

#[tokio::main]
async fn main() -> loco_rs::Result<()> {
    // Register sqlite-vec before any SQLite connection opens: every CLI subcommand
    // (task, db, scheduler) and the test harness open ctx.db before App::boot runs,
    // so the registration must happen here as well as in boot.
    yorishiro::db::register_sqlite_extensions();
    cli::main::<App, Migrator>().await
}
