use std::env;

use loco_rs::cli;
use migration::Migrator;
use yorishiro::{App, worker_tags};

#[tokio::main]
async fn main() -> loco_rs::Result<()> {
    // Register sqlite-vec before any SQLite connection opens: every CLI subcommand
    // (task, db, scheduler) and the test harness open ctx.db before App::boot runs,
    // so the registration must happen here as well in boot.
    yorishiro::db::register_sqlite_extensions();

    // Handle `worker-tags` before Loco's CLI: print exactly one comma-separated
    // line with every registered tag and exit.  This output is designed for
    // direct substitution into `--worker=`.
    let args: Vec<String> = env::args().collect();
    if args.len() == 2 && args[1] == "worker-tags" {
        println!("{}", worker_tags().join(","));
        return Ok(());
    }

    cli::main::<App, Migrator>().await
}
