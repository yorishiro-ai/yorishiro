use loco_rs::cli;
use migration::Migrator;
use yorishiro_hosted::HostedApp;

#[tokio::main]
async fn main() -> loco_rs::Result<()> {
    cli::main::<HostedApp, Migrator>().await
}
